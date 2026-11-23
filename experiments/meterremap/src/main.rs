//! Meter remap feasibility prototype for docs/meter.md. Throwaway: not wired into the app.
//!
//!   cargo run --release --manifest-path experiments/meterremap/Cargo.toml
//!
//! prints results.md. METERREMAP_WAV=<dir> also writes every source, render and ideal as a WAV
//! file. METERREMAP_LOOPS=<dir> adds real recordings: a directory made by `loops.py` from the
//! Apple Loops that GarageBand installs (4/4, starting on their downbeat), used to measure
//! downbeat detection and to remap a band of them.

mod bars;
mod groove;
mod metrics;
mod remap;

use groove::{Band, Groove, Rng, SR, STEMS};
use klont::stretch::{MODE_HITS, MODE_SMOOTH, Stretcher};
use remap::Run;
use std::time::Instant;

const METERS: [usize; 6] = [2, 3, 4, 5, 6, 7];

fn write_wav(dir: &Option<String>, name: &str, x: &[f32], gain: f32) {
    let Some(dir) = dir else { return };
    let spec = hound::WavSpec { channels: 1, sample_rate: SR as u32, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(format!("{dir}/{name}.wav"), spec).unwrap();
    for &v in x {
        w.write_sample(((v * gain).clamp(-1.0, 1.0) * 32767.0).round() as i16).unwrap();
    }
    w.finalize().unwrap();
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

fn add(a: &mut [f32], b: &[f32]) {
    for (x, y) in a.iter_mut().zip(b) {
        *x += y;
    }
}

fn pct(a: usize, b: usize) -> String {
    if b == 0 { "-".into() } else { format!("{:.0}% ({a} of {b})", 100.0 * a as f64 / b as f64) }
}

fn pattern_str(p: &[usize]) -> String {
    p.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(" ")
}

/// Runs for one take: cut at its own anchors (`anchored`) or at the grid.
fn runs_for(x: &[f32], g: &Groove, src: &[usize], anchored: bool) -> Vec<Run> {
    let lead = (g.lead * SR as f64).round() as i64;
    let spb = (g.spb() * SR as f64).round() as i64;
    let w = (0.06f64).min(g.spb() / 4.0);
    let an = if anchored { remap::anchors(x) } else { Vec::new() };
    remap::runs(src, lead, spb, |t| match remap::refine(&an, t as f64 / SR as f64, w) {
        Some(a) => ((a * SR as f64).round() as i64, true),
        None => (t, false),
    })
}

/// High-band energy in the 8 ms before each join against the median of the same before every
/// beat of the source: how much spikier a join is than the take's own beat starts, dB. Needs no
/// ideal render, so it works on real recordings too.
fn join_spike(out: &[f32], joins: &[i64], src: &[f32], beats: &[i64]) -> (f64, f64) {
    let hf = |x: &[f32], u: i64| -> f64 {
        let a = (u - (0.008 * SR) as i64).max(2);
        (a..u.min(x.len() as i64)).map(|i| {
            let i = i as usize;
            ((x[i] - 2.0 * x[i - 1] + x[i - 2]) as f64).powi(2)
        }).sum::<f64>()
    };
    let mut base: Vec<f64> = beats.iter().map(|&u| hf(src, u)).collect();
    base.sort_by(|a, b| a.total_cmp(b));
    let med = base[base.len() / 2].max(1e-12);
    let v: Vec<f64> = joins.iter().map(|&u| 10.0 * (hf(out, u) / med).max(1e-12).log10()).collect();
    let n = v.len().max(1) as f64;
    (v.iter().sum::<f64>() / n, v.iter().copied().fold(f64::NEG_INFINITY, f64::max))
}

struct Case {
    name: &'static str,
    groove: Groove,
    pattern: Vec<usize>,
    what: &'static str,
}

fn cases() -> Vec<Case> {
    vec![
        Case { name: "five-drop5", groove: groove::five(4), pattern: vec![1, 2, 3, 4], what: "5/4 to 4/4, beat 5 dropped" },
        Case { name: "five-drop4", groove: groove::five(4), pattern: vec![1, 2, 3, 5], what: "5/4 to 4/4, beat 4 dropped" },
        Case { name: "four-drop4", groove: groove::four(4), pattern: vec![1, 2, 3], what: "4/4 to 3/4, beat 4 dropped" },
        Case { name: "three-repeat3", groove: groove::three(4), pattern: vec![1, 2, 3, 3], what: "3/4 to 4/4, beat 3 twice" },
        Case { name: "three-repeat1", groove: groove::three(4), pattern: vec![1, 1, 2, 3], what: "3/4 to 4/4, beat 1 twice" },
        Case { name: "four-shuffle", groove: groove::four(4), pattern: vec![1, 3, 4, 2], what: "4/4 shuffled 1 3 4 2" },
    ]
}

struct Variant {
    name: &'static str,
    out: Vec<f32>,
    runs: Vec<Run>,
}

fn remap_cases(wav: &Option<String>) {
    println!("## Remapped grooves\n");
    println!("Each groove is 4 bars, rendered as performed (5 ms timing spread, the snare late, the bass a little late), then remapped. The ideal is the same events re-sequenced by the pattern and rendered, held notes released where the new order breaks them and the room running on: what the band would play in the new meter. Distances are against the ideal; lower is better.\n");
    println!("- **engine, one take**: the whole mix as one take, cut at its hard anchors, played by the engine's stretcher (hits mode) unchanged.");
    println!("- **engine, stems**: drums, bass and keys as three layers locked to one meter, each cut at its own anchors (or at the beat where it has none), summed.");
    println!("- **grid cut**: the mix cut exactly at the beat times instead of at anchors, same stretcher.");
    println!("- **slicer, stems**: as engine, stems, but a 30 ms equal-power crossfade where a stem has no anchor to cut at (held keys and bass); 5 ms at anchors.\n");
    println!("| Case | Pattern | Joins per bar | Render | Spectral distance, all / near joins (dB) | Click before joins, mean / worst (dB over ideal) | Residual after joins (dB) | Residual, all (dB) |");
    println!("| --- | --- | --- | --- | --- | --- | --- | --- |");
    let mut notes_rows = Vec::new();
    let mut detect_rows = Vec::new();
    for c in cases() {
        let g = &c.groove;
        let (perf, _) = groove::performed(g);
        let (ide, _) = groove::ideal(g, &c.pattern);
        let src = groove::source_beats(g.meter, g.bars, &c.pattern);
        let len = ide.mix.len();
        let end = (g.at(src.len() as f64) * SR as f64) as usize;
        let mix_runs = runs_for(&perf.mix, g, &src, true);
        let mut vars = vec![Variant { name: "engine, one take", out: remap::engine_hits(&perf.mix, &mix_runs, len), runs: mix_runs.clone() }];
        let (mut eng, mut sl) = (vec![0.0f32; len], vec![0.0f32; len]);
        let mut soft = 0;
        for s in 0..STEMS {
            let r = runs_for(&perf.stems[s], g, &src, true);
            soft += r.iter().skip(1).filter(|x| !x.hard).count();
            add(&mut eng, &remap::engine_hits(&perf.stems[s], &r, len));
            add(&mut sl, &remap::slicer(&perf.stems[s], &r, len, (0.005 * SR) as i64, (0.030 * SR) as i64));
        }
        vars.push(Variant { name: "engine, stems", out: eng, runs: mix_runs.clone() });
        let grid_runs = runs_for(&perf.mix, g, &src, false);
        vars.push(Variant { name: "grid cut", out: remap::engine_hits(&perf.mix, &grid_runs, len), runs: grid_runs });
        vars.push(Variant { name: "slicer, stems", out: sl, runs: mix_runs.clone() });
        let joins_per_bar = remap::joins(&mix_runs).len() as f64 / g.bars as f64;
        for v in &vars {
            let joins = remap::joins(&v.runs);
            let all = metrics::lsd_where(&v.out[..end], &ide.mix[..end], |_| true);
            let near = metrics::lsd_where(&v.out[..end], &ide.mix[..end], metrics::near_join(&joins));
            let j = metrics::joins(&v.out, &ide.mix, &joins, end);
            println!(
                "| {} | {} | {:.0} | {} | {:.2} / {:.2} | {:+.1} / {:+.1} | {:.1} | {:.1} |",
                c.what, pattern_str(&c.pattern), joins_per_bar, v.name, all, near, j.click_mean, j.click_max, j.resid_join, j.resid_all
            );
        }
        // Held notes the new order breaks, and notes a slice starts in the middle of.
        let spb = g.spb();
        let mut cut = 0;
        let mut tied = 0;
        for r in mix_runs.iter() {
            let k_end = g.at((r.k + r.b - r.a) as f64);
            let k0 = g.at(r.k as f64);
            for e in g.events.iter().filter(|e| groove::stem(e.inst) > 0) {
                let t = g.at((e.bar * g.meter) as f64 + e.beat) + e.off;
                let t_end = t + e.len * spb;
                if t >= k0 - 0.03 && t < k_end - 0.03 && t_end > k_end + spb / 4.0 {
                    cut += 1;
                }
                if r.a > 0 && t < k0 - 0.03 && t_end > k0 + spb / 4.0 {
                    tied += 1;
                }
            }
        }
        notes_rows.push(format!("| {} | {} | {} | {} | {soft} |", c.what, remap::joins(&mix_runs).len(), cut, tied));
        // Does the result read as the new meter?
        let beats: Vec<f64> = (0..src.len()).map(|j| g.at(j as f64)).collect();
        let f = bars::features(&vars[1].out, SR, &beats, g.spb());
        let b = bars::detect(&f, &METERS);
        detect_rows.push(format!("| {} | {} | {} | {} |", c.what, c.pattern.len(), b.meter, if b.phase == 0 { "beat 1".to_string() } else { format!("beat {}", b.phase + 1) }));
        // Listening files.
        let gain = 0.9 / vars.iter().map(|v| peak(&v.out)).chain([peak(&perf.mix), peak(&ide.mix)]).fold(0.0, f32::max);
        write_wav(wav, &format!("{}-0-source", c.name), &perf.mix, gain);
        for (i, v) in vars.iter().enumerate() {
            write_wav(wav, &format!("{}-{}-{}", c.name, i + 1, v.name.replace(", ", "-").replace(' ', "-")), &v.out, gain);
        }
        write_wav(wav, &format!("{}-9-ideal", c.name), &ide.mix, gain);
        // The short set kept in docs/meter-examples: the source and the engine on stems, tails trimmed.
        let ex = std::env::var("METERREMAP_EXAMPLES").ok();
        let cut = (end + (0.4 * SR) as usize).min(len);
        let src_end = ((g.at(g.beats() as f64) + 0.4) * SR as f64) as usize;
        if !c.name.ends_with("repeat1") {
            if c.name != "five-drop4" && c.name != "four-shuffle" {
                write_wav(&ex, &format!("{}-source", g.name), &perf.mix[..src_end.min(perf.mix.len())], gain);
            }
            write_wav(&ex, &format!("{}-{}", c.name, pattern_str(&c.pattern).replace(' ', "")), &vars[1].out[..cut], gain);
        }
    }
    println!("\n### Notes at the joins\n");
    println!("Bass and keys notes in the 4 bars. A note is cut when it runs more than a quarter beat past the end of its run; a slice starts inside a note when the note began before the slice and sounds on for more than a quarter beat. Soft cuts: run starts where a stem had no anchor near the beat (summed over the three stems).\n");
    println!("| Case | Joins | Notes cut | Slices starting inside a note | Soft cuts |");
    println!("| --- | --- | --- | --- | --- |");
    for r in notes_rows {
        println!("{r}");
    }
    println!("\n### Does the result read as the new meter?\n");
    println!("The meter detector below, run on the engine-stems render with the output beats known.\n");
    println!("| Case | Beats per bar wanted | Detected | Downbeat found at |");
    println!("| --- | --- | --- | --- |");
    for r in detect_rows {
        println!("{r}");
    }
    println!();
}

/// Detection on a take with known beats, starting `rot` beats into it.
fn detect_rotated(x: &[f32], g: &Groove, rot: usize) -> (bars::Bars, (usize, usize), std::time::Duration) {
    let beats: Vec<f64> = (rot..g.beats()).map(|b| g.at(b as f64)).collect();
    let t0 = Instant::now();
    let f = bars::features(x, SR, &beats, g.spb());
    let b = bars::detect(&f, &METERS);
    let el = t0.elapsed();
    let truth = (g.meter - rot % g.meter) % g.meter;
    let given = bars::phase(&f, g.meter);
    (b, (truth, given), el)
}

#[derive(Default)]
struct Tally {
    n: usize,
    meter: usize,
    folded: usize,
    phase_detected: usize,
    phase_given: usize,
}

impl Tally {
    fn row(&self, name: &str) -> String {
        format!(
            "| {name} | {} | {} | {} | {} | {} |",
            self.n,
            pct(self.meter, self.n),
            pct(self.folded, self.n),
            pct(self.phase_detected, self.meter),
            pct(self.phase_given, self.n)
        )
    }
    /// `tg`: the true downbeat, and the one found with the true meter typed.
    fn add(&mut self, b: &bars::Bars, truth_meter: usize, tg: (usize, usize)) {
        let (truth, given) = tg;
        self.n += 1;
        if b.meter == truth_meter {
            self.meter += 1;
            if b.phase == truth {
                self.phase_detected += 1;
            }
        } else if b.meter * 2 == truth_meter || b.meter == truth_meter * 2 {
            self.folded += 1;
        }
        if given == truth {
            self.phase_given += 1;
        }
    }
}

fn detection_synthetic() -> f64 {
    println!("## Bars from audio: synthetic grooves\n");
    println!("Random grooves in 3 to 7 beats per bar, 80 to 170 bpm, 8 bars, each started a random number of beats into the groove (a pickup), so the downbeat is not at the start. The beats are given (as the section grid gives them); the detector finds the beats per bar from 2 to 7 and the downbeat. Ten grooves per meter and band.\n");
    println!("| Takes | Count | Beats per bar right | Half or double | Downbeat right, when beats per bar right | Downbeat right, beats per bar typed |");
    println!("| --- | --- | --- | --- | --- | --- |");
    let mut all = Tally::default();
    let mut per_meter: Vec<Tally> = (0..8).map(|_| Tally::default()).collect();
    let mut cost = (0.0f64, 0.0f64);
    for band in [Band::Full, Band::DrumsOnly, Band::NoDrums, Band::KeysOnly] {
        let mut t = Tally::default();
        for meter in [3, 4, 5, 6, 7] {
            for seed in 0..10u64 {
                let g = groove::random(seed * 1000 + meter as u64 * 10 + band as u64, meter, 8, band);
                let (perf, _) = groove::performed(&g);
                let rot = Rng::new(seed ^ 0x77 ^ meter as u64).below(meter);
                let (b, tg, el) = detect_rotated(&perf.mix, &g, rot);
                cost.0 += el.as_secs_f64();
                cost.1 += perf.mix.len() as f64 / SR as f64;
                t.add(&b, meter, tg);
                all.add(&b, meter, tg);
                per_meter[meter].add(&b, meter, tg);
            }
        }
        println!("{}", t.row(&format!("{band:?}")));
    }
    println!("{}", all.row("**All**"));
    for m in 3..=7 {
        println!("{}", per_meter[m].row(&format!("{m} beats per bar")));
    }
    for (name, g) in [("The 5/4 groove above", groove::five(8)), ("The 4/4 groove above", groove::four(8)), ("The 3/4 groove above", groove::three(8))] {
        let (perf, _) = groove::performed(&g);
        let mut t = Tally::default();
        for rot in 0..g.meter {
            let (b, tg, _) = detect_rotated(&perf.mix, &g, rot);
            t.add(&b, g.meter, tg);
        }
        println!("{}", t.row(&format!("{name}, every start beat")));
    }
    let ms_per_s = cost.0 * 1000.0 / cost.1;
    println!("\nCost: {ms_per_s:.1} ms per second of audio (features and detection, one core).\n");
    ms_per_s
}

struct Loop {
    file: String,
    category: String,
    genre: String,
    beats: usize,
}

fn read_index(dir: &str) -> Vec<Loop> {
    let Ok(s) = std::fs::read_to_string(format!("{dir}/index.tsv")) else { return Vec::new() };
    s.lines()
        .skip(1)
        .filter_map(|l| {
            let c: Vec<&str> = l.split('\t').collect();
            if c.len() < 5 || c[3] != "4/4" {
                return None;
            }
            Some(Loop { file: c[0].into(), category: c[1].into(), genre: c[4].into(), beats: c[2].parse().ok()? })
        })
        .collect()
}

fn read_wav(path: &str) -> Option<Vec<f32>> {
    let mut r = hound::WavReader::open(path).ok()?;
    if r.spec().sample_rate != SR as u32 || r.spec().channels != 1 {
        return None;
    }
    Some(r.samples::<i16>().map(|s| s.unwrap_or(0) as f32 / 32768.0).collect())
}

/// A loop as a groove: tiled to at least 16 beats, with the beat length from its length.
fn tiled(x: &[f32], beats: usize, min_beats: usize) -> (Vec<f32>, f64, usize) {
    let reps = min_beats.div_ceil(beats).max(2);
    let mut out = Vec::with_capacity(x.len() * reps);
    for _ in 0..reps {
        out.extend_from_slice(x);
    }
    (out, x.len() as f64 / SR as f64 / beats as f64, beats * reps)
}

fn detection_real(dir: &str) {
    let loops = read_index(dir);
    println!("## Bars from audio: real recordings\n");
    println!("{} Apple Loops in 4/4 (the loops GarageBand installs), each tiled to at least 16 beats and started a random number of beats in. The truth is 4 beats per bar with the downbeat where the loop starts, as Apple loops are cut.\n", loops.len());
    println!("| Loops | Count | Beats per bar right (4) | Read as 2 or 8 | Downbeat right, when 4 found | Downbeat right, 4 typed |");
    println!("| --- | --- | --- | --- | --- | --- |");
    let mut by: std::collections::BTreeMap<String, Tally> = Default::default();
    let mut all = Tally::default();
    let mut wrong: std::collections::BTreeMap<usize, usize> = Default::default();
    for (i, l) in loops.iter().enumerate() {
        if l.beats % 4 != 0 {
            continue;
        }
        let Some(x) = read_wav(&format!("{dir}/{}", l.file)) else { continue };
        let (y, spb, nb) = tiled(&x, l.beats, 16);
        let rot = Rng::new(i as u64).below(4);
        let beats: Vec<f64> = (rot..nb).map(|b| b as f64 * spb).collect();
        let f = bars::features(&y, SR, &beats, spb);
        let b = bars::detect(&f, &METERS);
        let given = bars::phase(&f, 4);
        let truth = (4 - rot % 4) % 4;
        let cat = match l.category.as_str() {
            "Drums" | "Bass" | "Keyboards" | "Guitars" => l.category.clone(),
            _ => "Other".into(),
        };
        let tg = (truth, given);
        by.entry(cat).or_default().add(&b, 4, tg);
        all.add(&b, 4, tg);
        if b.meter != 4 {
            *wrong.entry(b.meter).or_default() += 1;
        }
    }
    for (k, t) in &by {
        println!("{}", t.row(k));
    }
    println!("{}", all.row("**All**"));
    // Bands: a drum loop, a bass loop and a keys or guitar loop from one genre folder at one
    // tempo, each tiled to 32 beats and summed, as a mix would be.
    let mut groups: std::collections::BTreeMap<(String, i64), [Vec<&Loop>; 3]> = Default::default();
    for l in loops.iter().filter(|l| l.beats % 4 == 0 && 32 % l.beats == 0) {
        let slot = match l.category.as_str() {
            "Drums" => 0,
            "Bass" => 1,
            "Keyboards" | "Guitars" => 2,
            _ => continue,
        };
        let Ok(meta) = std::fs::metadata(format!("{dir}/{}", l.file)) else { continue };
        let n = (meta.len() as usize).saturating_sub(44) / 2;
        let spb_ms = (n as f64 / SR as f64 / l.beats as f64 * 1000.0).round() as i64;
        groups.entry((l.genre.clone(), spb_ms)).or_default()[slot].push(l);
    }
    let mut band = Tally::default();
    let mut k = 0u64;
    for g in groups.values() {
        if g.iter().any(|v| v.is_empty()) {
            continue;
        }
        let n = g.iter().map(|v| v.len()).max().unwrap_or(0).min(12);
        for i in 0..n {
            let mut mix: Vec<f32> = Vec::new();
            let mut spb = 0.0;
            for (slot, gain) in [(0usize, 0.5f32), (1, 0.5), (2, 0.4)] {
                let l = g[slot][i % g[slot].len()];
                let Some(x) = read_wav(&format!("{dir}/{}", l.file)) else { continue };
                let (y, s, _) = tiled(&x, l.beats, 32);
                spb = s;
                let y = &y[..(32.0 * s * SR as f64) as usize];
                if mix.len() < y.len() {
                    mix.resize(y.len(), 0.0);
                }
                for (a, b) in mix.iter_mut().zip(y) {
                    *a += gain * b;
                }
            }
            k += 1;
            let rot = Rng::new(k).below(4);
            let beats: Vec<f64> = (rot..32).map(|b| b as f64 * spb).collect();
            let f = bars::features(&mix, SR, &beats, spb);
            let b = bars::detect(&f, &METERS);
            band.add(&b, 4, ((4 - rot % 4) % 4, bars::phase(&f, 4)));
        }
    }
    println!("{}", band.row("Bands: drums, bass and keys or guitar summed"));
    println!("\nWrong beats per bar, by what was found: {}.\n", wrong.iter().map(|(m, n)| format!("{m}: {n}")).collect::<Vec<_>>().join(", "));
}

/// A band from the loops: drums, bass and keys of one genre at one tempo, each 8 or 16 beats,
/// tiled to 4 bars.
fn real_band(dir: &str) -> Option<(String, [Vec<f32>; 3], f64)> {
    let loops = read_index(dir);
    let want = std::env::var("METERREMAP_BAND").ok();
    for genre in ["09 Disco Funk", "11 Blues Garage", "08 Indie Disco", "10 Vintage Breaks"] {
        let mut picks: Vec<(Vec<f32>, f64, String)> = Vec::new();
        let mut tempo = None;
        for cat in ["Drums", "Bass", "Keyboards"] {
            let found = loops.iter().filter(|l| l.genre == genre && l.category == cat && (l.beats == 8 || l.beats == 16)).find_map(|l| {
                if let Some(w) = &want {
                    if !w.split(',').any(|n| l.file.contains(n.trim())) {
                        return None;
                    }
                }
                let x = read_wav(&format!("{dir}/{}", l.file))?;
                let spb = x.len() as f64 / SR as f64 / l.beats as f64;
                if tempo.is_some_and(|t: f64| (spb / t - 1.0).abs() > 0.003) {
                    return None;
                }
                Some((x, spb, l.file.clone(), l.beats))
            });
            let (x, spb, file, beats) = found?;
            tempo.get_or_insert(spb);
            let (y, _, _) = tiled(&x, beats, 16);
            picks.push((y[..(16.0 * spb * SR as f64) as usize].to_vec(), spb, file));
        }
        if picks.len() == 3 {
            let names = picks.iter().map(|p| p.2.trim_end_matches(".wav").to_string()).collect::<Vec<_>>().join(", ");
            let spb = picks[0].1;
            let mut it = picks.into_iter().map(|p| p.0);
            let (d, b, k) = (it.next()?, it.next()?, it.next()?);
            return Some((format!("{genre}: {names}"), [d, b, k], spb));
        }
    }
    None
}

fn real_examples(dir: &str, wav: &Option<String>) {
    let Some((names, stems, spb)) = real_band(dir) else {
        println!("## Remapping real loops\n\nNo band of drums, bass and keys at one tempo found.\n");
        return;
    };
    println!("## Remapping real loops\n");
    println!("A band of three real loops at one tempo, 4 bars of 4/4: {names} ({:.0} bpm). No ideal exists for real audio, so the join measure here compares the high band in the 8 ms before each join with the same before every beat of the source: 0 dB means a join is no spikier than the take's own beat starts. Downbeats are at the start of the source.\n", 60.0 / spb);
    println!("| Pattern | Render | Joins | Spike before joins, mean / worst (dB) | Detected beats per bar | Downbeat found at |");
    println!("| --- | --- | --- | --- | --- | --- |");
    let lead = (0.3 * SR as f64) as usize;
    let len = stems[0].len() + 2 * lead;
    let pad = |x: &[f32]| -> Vec<f32> {
        let mut v = vec![0.0f32; lead];
        v.extend_from_slice(x);
        v.resize(len, 0.0);
        v
    };
    let gains = [0.5f32, 0.5, 0.4];
    let stems: Vec<Vec<f32>> = stems.iter().zip(gains).map(|(s, g)| pad(s).iter().map(|v| v * g).collect()).collect();
    let mut mix = vec![0.0f32; len];
    for s in &stems {
        add(&mut mix, s);
    }
    let g = Groove { name: "real".into(), bpm: 60.0 / spb, meter: 4, bars: 4, lead: 0.3, events: Vec::new() };
    let beats: Vec<i64> = (0..16).map(|b| (g.at(b as f64) * SR as f64).round() as i64).collect();
    let mut gain = 0.9 / peak(&mix);
    let mut files: Vec<(String, Vec<f32>)> = vec![("real-0-source".into(), mix.clone())];
    for (name, pat) in [("drop4", vec![1, 2, 3]), ("shuffle", vec![1, 3, 4, 2]), ("repeat4", vec![1, 2, 3, 4, 4])] {
        let src = groove::source_beats(4, 4, &pat);
        let out_len = (g.at(src.len() as f64) * SR as f64) as usize + lead;
        let mix_runs = runs_for(&mix, &g, &src, true);
        let one = remap::engine_hits(&mix, &mix_runs, out_len);
        let mut st = vec![0.0f32; out_len];
        let mut sl = vec![0.0f32; out_len];
        for s in &stems {
            let r = runs_for(s, &g, &src, true);
            add(&mut st, &remap::engine_hits(s, &r, out_len));
            add(&mut sl, &remap::slicer(s, &r, out_len, (0.005 * SR) as i64, (0.030 * SR) as i64));
        }
        let grid = remap::engine_hits(&mix, &runs_for(&mix, &g, &src, false), out_len);
        let out_beats: Vec<f64> = (0..src.len()).map(|j| g.at(j as f64)).collect();
        gain = gain.min(0.9 / [&one, &st, &sl, &grid].iter().map(|v| peak(v)).fold(0.0, f32::max));
        for (rname, out, runs) in [("engine, one take", &one, &mix_runs), ("engine, stems", &st, &mix_runs), ("slicer, stems", &sl, &mix_runs)] {
            let joins = remap::joins(runs);
            let (m, w) = join_spike(out, &joins, &mix, &beats);
            let f = bars::features(out, SR, &out_beats, spb);
            let b = bars::detect(&f, &METERS);
            println!("| {} | {rname} | {} | {m:+.1} / {w:+.1} | {} | beat {} |", pattern_str(&pat), joins.len(), b.meter, b.phase + 1);
        }
        let grid_joins: Vec<i64> = src.iter().enumerate().skip(1).filter(|(j, k)| **k != src[j - 1] + 1).map(|(j, _)| (g.at(j as f64) * SR as f64) as i64).collect();
        let (m, w) = join_spike(&grid, &grid_joins, &mix, &beats);
        println!("| {} | grid cut | {} | {m:+.1} / {w:+.1} | | |", pattern_str(&pat), grid_joins.len());
        files.push((format!("real-{name}-1-engine-one-take"), one));
        files.push((format!("real-{name}-2-engine-stems"), st));
        files.push((format!("real-{name}-3-grid-cut"), grid));
        files.push((format!("real-{name}-4-slicer-stems"), sl));
    }
    for (name, x) in &files {
        write_wav(wav, name, x, gain);
    }
    println!();
}

fn determinism() {
    println!("## Determinism\n");
    let g = groove::five(4);
    let (perf, _) = groove::performed(&g);
    let src = groove::source_beats(5, 4, &[1, 3, 2, 5]);
    let runs = runs_for(&perf.mix, &g, &src, true);
    let len = (g.at(src.len() as f64) * SR as f64) as usize;
    let pts = remap::points(&runs);
    for (mode, name) in [(MODE_HITS, "hits"), (MODE_SMOOTH, "smooth")] {
        let render = |sizes: &mut dyn FnMut() -> usize, start: f64| -> Vec<f32> {
            let mut st = Stretcher::new(SR);
            st.load(mode, len as f64 + 1.0, pts.iter().copied());
            st.seek(start);
            let mut out = vec![0.0f32; len - start as usize];
            let mut i = 0;
            while i < out.len() {
                let n = sizes().min(out.len() - i);
                st.process(&perf.mix, &mut out[i..i + n], |_| 1.0);
                i += n;
            }
            out
        };
        let a = render(&mut || 128, 0.0);
        let b = render(&mut || 128, 0.0);
        let mut r = Rng::new(9);
        let c = render(&mut || 1 + r.below(3000), 0.0);
        let same = |x: &[f32], y: &[f32]| x.iter().zip(y).all(|(p, q)| p.to_bits() == q.to_bits());
        // Play from eight points: equal to the whole render from the next run or hard point on.
        let mut worst_ms = 0.0f64;
        let mut all_match = true;
        for _ in 0..8 {
            let u = (r.below(len - 1000) as f64).floor();
            let s = render(&mut || 128, u);
            let next = pts.iter().map(|p| p.u - 0.005 * SR as f64).filter(|&x| x > u).fold(f64::INFINITY, f64::min);
            let last_diff = s.iter().zip(&a[u as usize..]).rposition(|(p, q)| p.to_bits() != q.to_bits());
            if let Some(ld) = last_diff {
                if (u + ld as f64) >= next {
                    all_match = false;
                }
                worst_ms = worst_ms.max(ld as f64 * 1000.0 / SR as f64);
            }
        }
        println!(
            "- {name}: two renders {}; random call sizes 1 to 3,000 {}; play from eight points: {} (differences last at most {:.0} ms).",
            if same(&a, &b) { "bit-identical" } else { "DIFFER" },
            if same(&a, &c) { "bit-identical" } else { "DIFFER" },
            if all_match { "each equals the whole render from the next slice start on" } else { "SOME DIFFER past the next slice" },
            worst_ms
        );
    }
    println!();
}

fn cpu() -> (f64, f64) {
    println!("## Cost\n");
    let g = groove::five(8);
    let (perf, _) = groove::performed(&g);
    let src = groove::source_beats(5, 8, &[1, 3, 2, 5]);
    let runs = runs_for(&perf.mix, &g, &src, true);
    let len = (g.at(src.len() as f64) * SR as f64) as usize;
    let lead = (g.lead * SR as f64) as i64;
    let ident = vec![Run { a: 0, b: src.len(), k: 0, u0: lead, u1: len as i64, s0: lead, s1: len as i64, hard: true }];
    let time = |mode: u32, r: &[Run]| -> f64 {
        let mut best = f64::INFINITY;
        for _ in 0..5 {
            let t0 = Instant::now();
            let o = remap::engine(&perf.mix, r, len, mode);
            std::hint::black_box(&o);
            best = best.min(t0.elapsed().as_secs_f64());
        }
        best * 1e9 / len as f64
    };
    let (h, hi, s, si) = (time(MODE_HITS, &runs), time(MODE_HITS, &ident), time(MODE_SMOOTH, &runs), time(MODE_SMOOTH, &ident));
    let t0 = Instant::now();
    for _ in 0..1000 {
        std::hint::black_box(runs_for_cached(&src));
    }
    let map_us = t0.elapsed().as_secs_f64() * 1e6 / 1000.0;
    println!("The engine's stretcher playing a remapped 8-bar 5/4 take ({} runs) against the same take as one run, ns per output sample, best of 5, 128-sample calls:\n", runs.len());
    println!("| Mode | Remapped | One run |");
    println!("| --- | --- | --- |");
    println!("| Hits | {h:.1} | {hi:.1} |");
    println!("| Smooth | {s:.1} | {si:.1} |");
    println!("\nBuilding the runs from beats and anchors (anchors already found): {map_us:.1} us for 8 bars.\n");
    (h, s)
}

/// The run building alone, with anchors already found.
fn runs_for_cached(src: &[usize]) -> Vec<Run> {
    let an: Vec<(f64, f32)> = (0..200).map(|i| (0.3 + i as f64 * 0.1875, 1.0)).collect();
    remap::runs(src, 14400, 18000, |t| match remap::refine(&an, t as f64 / SR as f64, 0.06) {
        Some(a) => ((a * SR as f64).round() as i64, true),
        None => (t, false),
    })
}

/// Downbeat accuracy (meter typed) for a few weightings of the cues, on the random grooves, the
/// three named grooves and the real loops. Printed so the choice of `bars::CUES` is on record.
fn tune(loops: &Option<String>) {
    let configs: Vec<(&str, bars::Cues)> = vec![
        ("low 1, beat change 1, full 0.5 (first try)", bars::Cues { low: 1.0, beat_change: 1.0, full: 0.5, bar_change: 0.0, mid: 0.0, high: 0.0 }),
        ("low 1, full 0.5", bars::Cues { low: 1.0, beat_change: 0.0, full: 0.5, bar_change: 0.0, mid: 0.0, high: 0.0 }),
        ("low 1", bars::Cues { low: 1.0, beat_change: 0.0, full: 0.0, bar_change: 0.0, mid: 0.0, high: 0.0 }),
        ("bar change 1", bars::Cues { low: 0.0, beat_change: 0.0, full: 0.0, bar_change: 1.0, mid: 0.0, high: 0.0 }),
        ("low 1, full 0.5, bar change 1", bars::Cues { low: 1.0, beat_change: 0.0, full: 0.5, bar_change: 1.0, mid: 0.0, high: 0.0 }),
        ("low 1, full 0.5, bar change 2", bars::Cues { low: 1.0, beat_change: 0.0, full: 0.5, bar_change: 2.0, mid: 0.0, high: 0.0 }),
        ("low 1, bar change 1", bars::Cues { low: 1.0, beat_change: 0.0, full: 0.0, bar_change: 1.0, mid: 0.0, high: 0.0 }),
        ("low 1, beat change 0.5, full 0.5, bar change 1", bars::Cues { low: 1.0, beat_change: 0.5, full: 0.5, bar_change: 1.0, mid: 0.0, high: 0.0 }),
        ("low 1, mid -0.5", bars::Cues { low: 1.0, beat_change: 0.0, full: 0.0, bar_change: 0.0, mid: -0.5, high: 0.0 }),
        ("low 1, mid -0.5, bar change 1", bars::Cues { low: 1.0, beat_change: 0.0, full: 0.0, bar_change: 1.0, mid: -0.5, high: 0.0 }),
        ("low 1, mid -1, bar change 1", bars::Cues { low: 1.0, beat_change: 0.0, full: 0.0, bar_change: 1.0, mid: -1.0, high: 0.0 }),
        ("low 1, high 0.5, mid -0.5, bar change 1", bars::Cues { low: 1.0, beat_change: 0.0, full: 0.0, bar_change: 1.0, mid: -0.5, high: 0.5 }),
        ("low 1, beat change 0.5, mid -0.5, bar change 1", bars::Cues { low: 1.0, beat_change: 0.5, full: 0.0, bar_change: 1.0, mid: -0.5, high: 0.0 }),
    ];
    // (features, meter, true downbeat, group)
    let mut takes: Vec<(bars::Features, usize, usize, &str)> = Vec::new();
    for band in [Band::Full, Band::DrumsOnly, Band::NoDrums, Band::KeysOnly] {
        for meter in [3, 4, 5, 6, 7] {
            for seed in 0..10u64 {
                let g = groove::random(seed * 1000 + meter as u64 * 10 + band as u64, meter, 8, band);
                let (perf, _) = groove::performed(&g);
                let rot = Rng::new(seed ^ 0x77 ^ meter as u64).below(meter);
                let beats: Vec<f64> = (rot..g.beats()).map(|b| g.at(b as f64)).collect();
                takes.push((bars::features(&perf.mix, SR, &beats, g.spb()), meter, (meter - rot % meter) % meter, "random"));
            }
        }
    }
    for g in [groove::five(8), groove::four(8), groove::three(8)] {
        let (perf, _) = groove::performed(&g);
        for rot in 0..g.meter {
            let beats: Vec<f64> = (rot..g.beats()).map(|b| g.at(b as f64)).collect();
            takes.push((bars::features(&perf.mix, SR, &beats, g.spb()), g.meter, (g.meter - rot) % g.meter, "named"));
        }
    }
    if let Some(dir) = loops {
        for (i, l) in read_index(dir).iter().enumerate() {
            if l.beats % 4 != 0 {
                continue;
            }
            let Some(x) = read_wav(&format!("{dir}/{}", l.file)) else { continue };
            let (y, spb, nb) = tiled(&x, l.beats, 16);
            let rot = Rng::new(i as u64).below(4);
            let beats: Vec<f64> = (rot..nb).map(|b| b as f64 * spb).collect();
            let group: &'static str = Box::leak(format!("real {} {}", l.category, l.beats).into_boxed_str());
            takes.push((bars::features(&y, SR, &beats, spb), 4, (4 - rot % 4) % 4, group));
        }
    }
    println!("## Choosing the downbeat cues\n");
    println!("Downbeat right with the beats per bar typed, for each weighting tried. The detector uses the row in bold.\n");
    println!("| Cues | Random grooves | Named grooves, every start | Real loops |");
    println!("| --- | --- | --- | --- |");
    for (name, c) in &configs {
        let mut row = Vec::new();
        for group in ["random", "named", "real"] {
            let t: Vec<_> = takes.iter().filter(|t| t.3 == group || (group == "real" && t.3.starts_with("real"))).collect();
            let ok = t.iter().filter(|t| bars::phase_with(&t.0, t.1, c) == t.2).count();
            row.push(pct(ok, t.len()));
        }
        let chosen = format!("{c:?}") == format!("{:?}", bars::CUES);
        let name = if chosen { format!("**{name}**") } else { name.to_string() };
        println!("| {name} | {} | {} | {} |", row[0], row[1], row[2]);
    }
    println!();
    // Where the chosen cues go wrong on real loops: how far off, by kind and length.
    let mut hist: std::collections::BTreeMap<&str, [usize; 4]> = Default::default();
    for t in takes.iter().filter(|t| t.3.starts_with("real")) {
        let off = (bars::phase(&t.0, 4) + 4 - t.2) % 4;
        hist.entry(t.3).or_default()[off] += 1;
    }
    println!("| Loops (kind, beats) | Right | 1 beat late | 2 beats off | 1 beat early |");
    println!("| --- | --- | --- | --- | --- |");
    for (k, h) in hist {
        if h.iter().sum::<usize>() >= 10 {
            println!("| {} | {} | {} | {} | {} |", k.trim_start_matches("real "), h[0], h[1], h[2], h[3]);
        }
    }
    println!();
}

fn main() {
    let wav = std::env::var("METERREMAP_WAV").ok();
    for d in [&wav, &std::env::var("METERREMAP_EXAMPLES").ok()].into_iter().flatten() {
        std::fs::create_dir_all(d).unwrap();
    }
    let loops = std::env::var("METERREMAP_LOOPS").ok();
    println!("# Meter remap prototype: results\n");
    println!("Output of `cargo run --release --manifest-path experiments/meterremap/Cargo.toml`{}. See docs/meter.md for what it means.\n", if loops.is_some() { " with METERREMAP_LOOPS set" } else { "" });
    remap_cases(&wav);
    detection_synthetic();
    if let Some(dir) = &loops {
        detection_real(dir);
    }
    tune(&loops);
    if let Some(dir) = &loops {
        real_examples(dir, &wav);
    }
    determinism();
    cpu();
}
