//! Flex quantize feasibility prototype for docs/quant.md. Throwaway: not wired into the app.
//!
//!   cargo run --release --manifest-path experiments/flexquant/Cargo.toml
//!
//! Set FLEXQUANT_WAV=<dir> to also write every input and output as a WAV file to listen to, and
//! FLEXQUANT_DEBUG=1 to print each case's found and expected onsets to stderr.

mod flex;
mod metrics;
mod stretch;
mod synth;

use flex::{Anchor, Feel, Grid, Moved, Target, feel_time, forward, quantize, warp_map};
use metrics::{lsd, match_times, stats};
use std::time::Instant;
use stretch::{Method, Params, Stretcher, render};
use synth::{SR, Take};

const METHODS: [Method; 4] = [Method::Varispeed, Method::Slice, Method::Wsola, Method::Pv];
const BPM: f64 = 100.0;

fn ms(x: f64) -> String {
    if x.is_nan() { "-".into() } else { format!("{x:.1}") }
}

fn write_wav(dir: &Option<String>, name: &str, x: &[f32]) {
    let Some(dir) = dir else { return };
    let spec = hound::WavSpec { channels: 1, sample_rate: SR as u32, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let mut w = hound::WavWriter::create(format!("{dir}/{name}.wav"), spec).unwrap();
    for &v in x {
        w.write_sample(v).unwrap();
    }
    w.finalize().unwrap();
}

fn anchor_row(name: &str, found: &[Anchor], truth: &[f64]) {
    let (errs, missed, extra) = match_times(&found.iter().map(|a| a.t).collect::<Vec<_>>(), truth, 0.05);
    let s = stats(&errs);
    println!(
        "| {name} | {} | {} | {missed} | {extra} | {} | {} | {} | {} |",
        truth.len(),
        s.n,
        ms(s.mean),
        ms(s.p95),
        ms(s.max),
        ms(s.bias)
    );
}

/// Where each true event should land after quantizing.
fn intended(take: &Take, g: &Grid, feel: &Feel, strength: &dyn Fn(f64) -> f64) -> Vec<f64> {
    take.events.iter().map(|e| e.t + strength(e.beat) * (feel_time(g, feel, e.beat) - e.t)).collect()
}

struct Case<'a> {
    name: &'a str,
    file: &'a str,
    take: &'a Take,
    anchors: Vec<Anchor>,
    target: Target<'a>,
    /// The ideal render, when the target is the straight grid at full strength.
    ideal: bool,
    sung: bool,
}

struct Rendered {
    method: Method,
    out: Vec<f32>,
}

fn run_case(c: &Case, wav: &Option<String>, p: Params) -> (Vec<Moved>, Vec<(f64, f64, bool)>, Vec<Rendered>) {
    let take = c.take;
    let g = Grid { bpm: take.bpm, origin: take.lead };
    let moved = quantize(&c.anchors, &g, &c.target);
    let map = warp_map(&moved, take.audio.len(), SR);
    let want = intended(take, &g, &c.target.feel, c.target.strength);
    let truth: Vec<f64> = take.events.iter().map(|e| e.t).collect();
    let len = take.audio.len();
    let moved_n = moved.iter().filter(|m| !m.slot.is_nan()).count();

    println!("\n### {}\n", c.name);
    let tlen = len as f64 / SR as f64;
    // Which true onsets have an anchor (within 50 ms).
    let has: Vec<bool> = truth.iter().map(|t| c.anchors.iter().any(|a| (a.t - t).abs() <= 0.05)).collect();
    let off = |times: &dyn Fn(f64) -> f64, pick: Option<bool>| {
        let e: Vec<f64> = (0..truth.len())
            .filter(|&i| pick.is_none_or(|p| has[i] == p))
            .map(|i| (times(truth[i]) - want[i]) * 1000.0)
            .collect();
        let s = stats(&e);
        format!("mean {} ms, p95 {} ms, max {} ms ({})", ms(s.mean), ms(s.p95), ms(s.max), s.n)
    };
    let fwd = |t: f64| forward(&moved, tlen, t);
    let missed = has.iter().filter(|h| !**h).count();
    println!(
        "{} anchors, {} pulled to a slot, {} warp points. True onsets against where they should land, before: {}; after the warp map: {}{}.\n",
        c.anchors.len(),
        moved_n,
        map.len(),
        off(&|t| t, None),
        off(&fwd, None),
        if missed > 0 { format!("; of those, the onsets with an anchor: {}, the ones without: {}", off(&fwd, Some(true)), off(&fwd, Some(false))) } else { String::new() }
    );
    write_wav(wav, &format!("{}-0-input", c.file), &take.audio);
    if c.ideal {
        write_wav(wav, &format!("{}-0-ideal", c.file), &take.ideal);
    }
    let mut head = String::from("| Method | Onsets found in the output | Off where the map puts them: mean / p95 / max ms | Attacks matched at hard anchors: off mean / max ms, correlation |");
    let mut rule = String::from("| --- | --- | --- | --- |");
    if c.ideal {
        head += " Spectral distance to ideal, dB |";
        rule += " --- |";
    }
    if c.sung {
        head += " Pitch error: mean / max cents |";
        rule += " --- |";
    }
    if c.ideal && !c.sung {
        head += " Attack peak vs ideal, mean / worst dB | Before the attack vs ideal, mean dB |";
        rule += " --- | --- |";
    }
    head += " CPU ns per sample | x real time |";
    rule += " --- | --- |";
    println!("{head}\n{rule}");

    // The untouched take as a reference row.
    let mut rows: Vec<Rendered> = Vec::new();
    let detect = |x: &[f32]| -> Vec<f64> {
        let a = if c.sung { flex::note_anchors_refined(x, SR) } else { flex::onsets_sensitive(x, SR) };
        a.iter().map(|a| a.t).collect()
    };
    // Where the map sends the onsets the same detector finds in the input.
    let expect: Vec<f64> = detect(&take.audio).iter().map(|&t| forward(&moved, tlen, t)).collect();
    let report = |name: &str, out: &[f32], secs: f64, expect: &[f64]| {
        let found = detect(out);
        let (errs, _, _) = match_times(&found, expect, 0.05);
        if std::env::var("FLEXQUANT_DEBUG").is_ok() {
            eprintln!("{} {name}: found {:?}\n  expect {:?}\n  errs {:?}", c.file, found.iter().map(|t| (t * 1000.0).round()).collect::<Vec<_>>(), expect.iter().map(|t| (t * 1000.0).round()).collect::<Vec<_>>(), errs.iter().map(|e| e.round()).collect::<Vec<_>>());
        }
        let s = stats(&errs);
        let mut row = format!("| {name} | {}/{} | {} / {} / {} |", s.n, expect.len(), ms(s.mean), ms(s.p95), ms(s.max));
        if secs > 0.0 {
            let (lags, corr) = metrics::attack_offsets(&take.audio, out, &map, SR);
            let st = stats(&lags);
            row += &format!(" {} / {} ({:.3}) |", ms(st.mean), ms(st.max), corr);
        } else {
            row += " - |";
        }
        if c.ideal {
            row += &format!(" {:.2} |", lsd(out, &take.ideal, SR));
        }
        if c.sung {
            let mut errs = Vec::new();
            for (i, e) in take.events.iter().enumerate() {
                let end = take.events.get(i + 1).map(|_| want[i + 1]).unwrap_or(want[i] + 0.6);
                let t0 = want[i] + 0.06;
                let t1 = (t0 + 0.15).min(end - 0.03);
                if let Some(cents) = metrics::pitch_cents(out, SR, t0, t1) {
                    errs.push((cents - e.cents) as f64);
                }
            }
            let s = stats(&errs);
            row += &format!(" {} / {} |", ms(s.mean), ms(s.max));
        }
        if c.ideal && !c.sung {
            let (mut dp, mut worst, mut pre) = (0.0f32, 0.0f32, 0.0f32);
            let loud: Vec<usize> = (0..take.events.len()).filter(|&i| take.events[i].class & 3 != 0).collect();
            for &i in &loud {
                let (pk_o, pre_o) = metrics::attack_db(out, SR, want[i]);
                let (pk_i, pre_i) = metrics::attack_db(&take.ideal, SR, want[i]);
                dp += (pk_o - pk_i).abs();
                worst = worst.max((pk_o - pk_i).abs());
                pre += pre_o - pre_i;
            }
            let k = loud.len() as f32;
            row += &format!(" {:.2} / {:.2} | {:+.1} |", dp / k, worst, pre / k);
        }
        if secs > 0.0 {
            row += &format!(" {:.1} | {:.0} |", secs * 1e9 / len as f64, len as f64 / SR as f64 / secs);
        } else {
            row += " - | - |";
        }
        println!("{row}");
    };
    report("input, not quantized", &take.audio, 0.0, &expect);
    for m in METHODS {
        let mut best = f64::INFINITY;
        let mut out = Vec::new();
        for _ in 0..5 {
            let t = Instant::now();
            out = render(m, p, &take.audio, &map, len);
            best = best.min(t.elapsed().as_secs_f64());
        }
        report(m.name(), &out, best, &expect);
        write_wav(wav, &format!("{}-{}", c.file, m.name().split(' ').next().unwrap()), &out);
        rows.push(Rendered { method: m, out });
    }
    (moved, map, rows)
}

/// Level at each legato join (the deepest 5 ms in the 30 ms either side, against the level 40 to
/// 60 ms either side), in the input and in the output, with legato anchors soft (as designed)
/// and with every anchor hard.
fn join_dips(take: &Take, anchors: &[Anchor], moved: &[Moved], rows: &[Rendered], p: Params) {
    let dip = |x: &[f32], t: f64| -> f32 {
        let rms = |a: f64, b: f64| {
            let (i, j) = ((a * SR as f64) as usize, (b * SR as f64) as usize);
            (x[i..j].iter().map(|v| v * v).sum::<f32>() / (j - i) as f32).sqrt()
        };
        let around = 0.5 * (rms(t - 0.06, t - 0.04) + rms(t + 0.04, t + 0.06));
        let mut lo = f32::INFINITY;
        let mut u = t - 0.03;
        while u < t + 0.03 {
            lo = lo.min(rms(u, u + 0.005));
            u += 0.001;
        }
        dsp::lin_to_db(lo / around)
    };
    let g = Grid { bpm: take.bpm, origin: take.lead };
    let all_hard: Vec<Anchor> = anchors.iter().map(|a| Anchor { hard: true, ..*a }).collect();
    let tg = Target { res: 0.5, feel: Feel::Straight, strength: &|_| 1.0, window: 0.1, min_gap: 0.012 };
    let moved_h = quantize(&all_hard, &g, &tg);
    let map_h = warp_map(&moved_h, take.audio.len(), SR);
    let len = take.audio.len();
    let tlen = len as f64 / SR as f64;
    let out = |m: Method| rows.iter().find(|r| r.method == m).unwrap().out.clone();
    let (ws, pv) = (out(Method::Wsola), out(Method::Pv));
    let (wh, ph) = (render(Method::Wsola, p, &take.audio, &map_h, len), render(Method::Pv, p, &take.audio, &map_h, len));
    println!("\nLevel at the four legato joins of case 4 (dB below the level around them; the joins carry no level change, so closer to the input is better):\n");
    println!("| Join | Input | WSOLA, legato anchors soft | Phase vocoder, soft | WSOLA, every anchor hard | Phase vocoder, every anchor hard |");
    println!("| --- | --- | --- | --- | --- | --- |");
    for e in take.events.iter().filter(|e| e.class == 2) {
        let (t, th) = (forward(moved, tlen, e.t), forward(&moved_h, tlen, e.t));
        println!(
            "| beat {} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} |",
            e.beat,
            dip(&take.audio, e.t),
            dip(&ws, t),
            dip(&pv, t),
            dip(&wh, th),
            dip(&ph, th)
        );
    }
}

/// Render `len` samples in calls of random size.
fn render_chunked(m: Method, p: Params, src: &[f32], map: &[(f64, f64, bool)], len: usize, seed: u64) -> Vec<f32> {
    let mut s = Stretcher::new(m, p, src, map);
    let mut out = vec![0.0f32; len];
    let mut r = synth::Rng::new(seed);
    let mut i = 0;
    while i < len {
        let n = (1 + (r.f() * 3000.0) as usize).min(len - i);
        s.process(&mut out[i..i + n]);
        i += n;
    }
    out
}

fn determinism(name: &str, src: &[f32], map: &[(f64, f64, bool)], p: Params) {
    let len = src.len();
    for m in METHODS {
        let full = render(m, p, src, map, len);
        let again = render(m, p, src, map, len);
        let chunked = render_chunked(m, p, src, map, len, 99);
        let (mut converge_ms, mut repeat_ok, mut tail_ok) = (0.0f64, true, true);
        for k in 1..9 {
            let start = len * k / 9 + 1234;
            let from = |_: ()| {
                let mut s = Stretcher::new(m, p, src, map);
                s.seek(start as i64);
                let mut out = vec![0.0f32; len - start];
                s.process(&mut out);
                out
            };
            let a = from(());
            let b = from(());
            repeat_ok &= a == b;
            // From where on does a play from `start` equal the full render?
            let last_diff = a.iter().zip(&full[start..]).rposition(|(x, y)| x.to_bits() != y.to_bits());
            if let Some(d) = last_diff {
                converge_ms = converge_ms.max(d as f64 / SR as f64 * 1000.0);
                // It must agree from the next warp point after `start` on.
                let next = map.iter().filter(|q| q.2).map(|q| q.0.round() as usize).find(|&u| u > start).unwrap_or(len);
                tail_ok &= start + d < next;
            }
        }
        println!(
            "| {name} | {} | {} | {} | {} | {} | {} |",
            m.name(),
            if full == again { "yes" } else { "NO" },
            if full == chunked { "yes" } else { "NO" },
            if repeat_ok { "yes" } else { "NO" },
            if tail_ok { "yes" } else { "NO" },
            ms(converge_ms)
        );
    }
}

/// The synth reads the take's stored frames through the warp map; the raw input plays the
/// stretched audio. Compare the loudness the frames give with the loudness of the audio heard.
fn frames_follow(take: &Take, map: &[(f64, f64, bool)], out: &[f32]) -> (f64, f64) {
    use dsp::frame::LOUDNESS;
    let orig = dsp::capture::frames_from_signal(&take.audio, SR, dsp::frame::DEFAULT_BANDS);
    let heard = dsp::capture::frames_from_signal(out, SR, dsp::frame::DEFAULT_BANDS);
    let warp = |n: f64| -> f64 {
        let i = map.partition_point(|q| q.0 <= n).clamp(1, map.len() - 1);
        let (a, b) = (map[i - 1], map[i]);
        a.1 + (b.1 - a.1) * (n - a.0) / (b.0 - a.0)
    };
    let n = heard.len().min(orig.len());
    let read: Vec<f64> = (0..n)
        .map(|k| {
            let src_frame = warp(k as f64 / 100.0 * SR as f64) / SR as f64 * 100.0;
            let i = (src_frame.floor() as usize).min(orig.len() - 2);
            let f = (src_frame - i as f64).clamp(0.0, 1.0);
            orig[i][LOUDNESS] as f64 * (1.0 - f) + orig[i + 1][LOUDNESS] as f64 * f
        })
        .collect();
    let h: Vec<f64> = (0..n).map(|k| heard[k][LOUDNESS] as f64).collect();
    let corr = |lag: i64| -> f64 {
        let (mut xy, mut xx, mut yy) = (0.0, 0.0, 0.0);
        let mx = read.iter().sum::<f64>() / n as f64;
        let my = h.iter().sum::<f64>() / n as f64;
        for k in 20..n as i64 - 20 {
            let (x, y) = (read[k as usize] - mx, h[(k + lag) as usize] - my);
            xy += x * y;
            xx += x * x;
            yy += y * y;
        }
        xy / (xx * yy).sqrt()
    };
    let lags: Vec<(i64, f64)> = (-8..=8).map(|l| (l, corr(l))).collect();
    let (bl, bc) = lags.iter().copied().fold((0, f64::NEG_INFINITY), |a, b| if b.1 > a.1 { b } else { a });
    // Parabolic refinement between neighbouring lags, in ms.
    let at = |l: i64| lags.iter().find(|q| q.0 == l).map(|q| q.1).unwrap_or(bc);
    let (a, b, c) = (at(bl - 1), bc, at(bl + 1));
    let den = a - 2.0 * b + c;
    let frac = if den.abs() > 1e-12 { 0.5 * (a - c) / den } else { 0.0 };
    ((bl as f64 + frac) * 10.0, corr(0))
}

fn main() {
    let wav = std::env::var("FLEXQUANT_WAV").ok();
    let p = Params::default();
    let drum_a = synth::drums(11, BPM, 4, &synth::Feel { swing: 0.58, snare_ms: 12.0, jitter_ms: 6.0, drift_ms: 10.0 });
    let drum_b = synth::drums(23, BPM, 4, &synth::Feel { swing: 0.5, snare_ms: -8.0, jitter_ms: 12.0, drift_ms: 0.0 });
    let sung = synth::sung(5, BPM, 15.0, 20.0);

    println!("# Flex quantize prototype results\n");
    println!(
        "Takes (48 kHz, {BPM} bpm, synthetic, so every onset's true time and grid slot is known): drum loop A, 4 bars of 16ths, played with 58% swing, the snare 12 ms late, 6 ms random timing and a 10 ms push and drag over the loop ({:.1} s); drum loop B, the same pattern straight, the snare 8 ms early, 12 ms random timing ({:.1} s); a sung line of 18 notes (detached, \"da\" and legato joins), 15 ms late on average with 20 ms random timing ({:.1} s).",
        drum_a.audio.len() as f32 / SR,
        drum_b.audio.len() as f32 / SR,
        sung.audio.len() as f32 / SR
    );

    // ------------------------------------------------------------ anchors
    println!("\n## Anchor accuracy\n");
    println!("A found anchor matches a true onset within 50 ms, one to one. Errors are found minus true, in ms.\n");
    println!("| Take and detector | True | Matched | Missed | Extra | Mean error | p95 | Max | Bias |");
    println!("| --- | --- | --- | --- | --- | --- | --- | --- | --- |");
    let t_a: Vec<f64> = drum_a.events.iter().map(|e| e.t).collect();
    let t_s: Vec<f64> = sung.events.iter().map(|e| e.t).collect();
    let t0 = Instant::now();
    let a_onsets = flex::onsets(&drum_a.audio, SR);
    let onset_secs = t0.elapsed().as_secs_f64();
    anchor_row("Drums A, audio onsets (second pass detector)", &a_onsets, &t_a);
    let t0 = Instant::now();
    let a_hits = flex::second_pass_anchors(&drum_a.audio, SR, true);
    let sp_drum_secs = t0.elapsed().as_secs_f64();
    anchor_row("Drums A, second pass hits", &a_hits, &t_a);
    anchor_row("Drums A, live frames' onset slot (10 ms)", &flex::frame_anchors(&drum_a.audio, SR, false), &t_a);
    anchor_row("Drums A, live frames refined on the audio", &flex::frame_anchors(&drum_a.audio, SR, true), &t_a);
    let loud: Vec<f64> = drum_a.events.iter().filter(|e| e.class & 3 != 0).map(|e| e.t).collect();
    anchor_row("Drums A, kick and snare only, audio onsets", &a_onsets, &loud);
    let a_onsets = flex::onsets_sensitive(&drum_a.audio, SR);
    anchor_row("Drums A, audio onsets plus a 4 kHz high band pass", &a_onsets, &t_a);
    let t_b: Vec<f64> = drum_b.events.iter().map(|e| e.t).collect();
    anchor_row("Drums B, audio onsets", &flex::onsets(&drum_b.audio, SR), &t_b);
    let b_onsets = flex::onsets_sensitive(&drum_b.audio, SR);
    anchor_row("Drums B, audio onsets plus a 4 kHz high band pass", &b_onsets, &t_b);
    anchor_row("Sung, audio onsets", &flex::onsets(&sung.audio, SR), &t_s);
    let t0 = Instant::now();
    let s_notes = flex::second_pass_anchors(&sung.audio, SR, false);
    let sp_sung_secs = t0.elapsed().as_secs_f64();
    anchor_row("Sung, second pass note starts", &s_notes, &t_s);
    anchor_row("Sung, live frames' onset slot (10 ms)", &flex::frame_anchors(&sung.audio, SR, false), &t_s);
    anchor_row("Sung, live frames refined on the audio", &flex::frame_anchors(&sung.audio, SR, true), &t_s);
    let legato: Vec<f64> = sung.events.iter().filter(|e| e.class == 2).map(|e| e.t).collect();
    anchor_row("Sung, legato joins only, second pass", &s_notes, &legato);
    let s_notes = flex::note_anchors_refined(&sung.audio, SR);
    anchor_row("Sung, second pass with legato joins moved to the glide's midpoint", &s_notes, &t_s);
    anchor_row("Sung, legato joins only, moved to the glide's midpoint", &s_notes, &legato);
    println!(
        "\nDetection cost, single core: audio onsets {:.1} ms per second of audio; the whole second pass {:.1} ms (drums) and {:.1} ms (sung) per second of audio. It already runs in a worker after every take.",
        onset_secs * 1000.0 / (drum_a.audio.len() as f64 / SR as f64),
        sp_drum_secs * 1000.0 / (drum_a.audio.len() as f64 / SR as f64),
        sp_sung_secs * 1000.0 / (sung.audio.len() as f64 / SR as f64)
    );

    // ------------------------------------------------------------ quantizing
    println!("\n## Quantizing and stretching\n");
    println!(
        "Each case quantizes the anchors, builds the warp map and plays the take through it four ways. The line above each table says how close the warp map itself puts every true onset to where it should land: that is the anchors' accuracy, whatever plays the audio. The table then checks the audio. \"Attacks matched\" looks for the 22 ms of take audio around each hard anchor (2 ms before, 20 ms after) in the output, within 30 ms of where the map puts it, and gives the offset and the correlation where it matches best. The onset columns find the onsets in the output with the same detector that found the anchors and compares them with where the map puts the onsets that detector finds in the input (within 50 ms), so the detector's own bias cancels. Spectral distance compares third-octave band levels with the take rendered with every event exactly on the grid (lower is closer; the input row is what the timing alone costs). Pitch error is the median YIN pitch 60 to 210 ms into each note against the sung pitch. Attack rows compare the 10 ms after each kick and snare, and the 15 to 2 ms before it, with the ideal render: smeared or doubled attacks show as a lower peak and more level before the attack. CPU is the best of five whole-take renders on one core of the build machine."
    );
    let full = |_: f64| 1.0;
    let ramp = |beat: f64| (beat / 16.0).clamp(0.0, 1.0);
    let w = 0.075;
    let gap = 0.012;
    let g_a = Grid { bpm: BPM, origin: drum_a.lead };

    let c1 = Case {
        name: "1. Drums A to straight 16ths, full strength",
        file: "1-drums-straight",
        take: &drum_a,
        anchors: a_onsets.clone(),
        target: Target { res: 0.25, feel: Feel::Straight, strength: &full, window: w, min_gap: gap },
        ideal: true,
        sung: false,
    };
    let (_, map1, _) = run_case(&c1, &wav, p);

    let c2 = Case {
        name: "2. Drums A to 16ths with 58% swing (the feel it was played with)",
        file: "2-drums-swing",
        take: &drum_a,
        anchors: a_onsets.clone(),
        target: Target { res: 0.25, feel: Feel::Swing { amount: 0.58, unit: 0.25 }, strength: &full, window: w, min_gap: gap },
        ideal: false,
        sung: false,
    };
    run_case(&c2, &wav, p);

    // Strength automated by a lane: 0 at the start, 1 at the end of the loop.
    let c3 = Case {
        name: "3. Drums A to straight 16ths, strength automated from 0 to 1 over the loop",
        file: "3-drums-ramp",
        take: &drum_a,
        anchors: a_onsets.clone(),
        target: Target { res: 0.25, feel: Feel::Straight, strength: &ramp, window: w, min_gap: gap },
        ideal: false,
        sung: false,
    };
    let (m3, _, _) = run_case(&c3, &wav, p);
    print!("\nOffset from the straight grid after quantizing, by bar, as a share of the offset before (the lane is at 0.125, 0.375, 0.625, 0.875 mid-bar):");
    for bar in 0..4 {
        let (mut before, mut after) = (0.0, 0.0);
        for m in m3.iter().filter(|m| !m.slot.is_nan() && (m.slot / 4.0).floor() as usize == bar) {
            let gt = g_a.time(m.slot);
            before += (m.src - gt).abs();
            after += (m.dst - gt).abs();
        }
        print!(" bar {} {:.2};", bar + 1, after / before);
    }
    println!();

    let c4 = Case {
        name: "4. Sung line to straight 8ths, full strength",
        file: "4-sung-straight",
        take: &sung,
        anchors: s_notes.clone(),
        target: Target { res: 0.5, feel: Feel::Straight, strength: &full, window: 0.1, min_gap: gap },
        ideal: true,
        sung: true,
    };
    let (moved4, map4, rows4) = run_case(&c4, &wav, p);
    join_dips(&sung, &s_notes, &moved4, &rows4, p);

    // The drums as heard (not quantized) give the sung line its feel.
    let a_heard = quantize(&a_onsets, &g_a, &Target { res: 0.25, feel: Feel::Straight, strength: &|_| 0.0, window: w, min_gap: gap });
    let groove_a = flex::groove_points(&a_heard, &g_a);
    let c5 = Case {
        name: "5. Sung line to drum loop A's groove, hit by hit (8ths)",
        file: "5-sung-follows-drums",
        take: &sung,
        anchors: s_notes.clone(),
        target: Target { res: 0.5, feel: Feel::Map(groove_a.clone()), strength: &full, window: 0.1, min_gap: gap },
        ideal: false,
        sung: true,
    };
    run_case(&c5, &wav, p);

    let c6 = Case {
        name: "6. Drums B to drum loop A's groove, hit by hit (16ths)",
        file: "6-drumsB-follows-A",
        take: &drum_b,
        anchors: b_onsets.clone(),
        target: Target { res: 0.25, feel: Feel::Map(groove_a.clone()), strength: &full, window: w, min_gap: gap },
        ideal: false,
        sung: false,
    };
    run_case(&c6, &wav, p);

    let template = flex::groove_template(&a_heard, &g_a, 0.25, 4.0, 16.0);
    let c7 = Case {
        name: "7. Drums B to drum loop A's groove averaged into a one-bar template",
        file: "7-drumsB-template-A",
        take: &drum_b,
        anchors: b_onsets.clone(),
        target: Target { res: 0.25, feel: Feel::Map(template), strength: &full, window: w, min_gap: gap },
        ideal: false,
        sung: false,
    };
    run_case(&c7, &wav, p);

    // How far the followers end up from the hits they follow, slot by slot.
    println!("\nFollowing: distance from each true event's landing point to drum A's hit on the same slot.\n");
    println!("| Follower | Before, mean / max ms | After (anchors moved), mean / max ms |");
    println!("| --- | --- | --- |");
    let follow_row = |name: &str, take: &Take, anchors: &[Anchor], feel: Feel, res: f64, win: f64| {
        let g = Grid { bpm: BPM, origin: take.lead };
        let moved = quantize(anchors, &g, &Target { res, feel, strength: &full, window: win, min_gap: gap });
        let len = take.audio.len() as f64 / SR as f64;
        let (mut b, mut a) = (Vec::new(), Vec::new());
        // Both takes counted from their own beat 0, as clips in one section are.
        for e in &take.events {
            if let Some(h) = a_heard.iter().find(|m| (m.slot - e.beat).abs() < 1e-9) {
                let lead = h.dst - drum_a.lead;
                b.push((e.t - take.lead - lead) * 1000.0);
                a.push((forward(&moved, len, e.t) - take.lead - lead) * 1000.0);
            }
        }
        let (sb, sa) = (stats(&b), stats(&a));
        println!("| {name} | {} / {} | {} / {} |", ms(sb.mean), ms(sb.max), ms(sa.mean), ms(sa.max));
    };
    follow_row("Sung line, hit by hit", &sung, &s_notes, Feel::Map(groove_a.clone()), 0.5, 0.1);
    follow_row("Drums B, hit by hit", &drum_b, &b_onsets, Feel::Map(groove_a.clone()), 0.25, w);
    follow_row(
        "Drums B, one-bar template",
        &drum_b,
        &b_onsets,
        Feel::Map(flex::groove_template(&a_heard, &g_a, 0.25, 4.0, 16.0)),
        0.25,
        w,
    );

    // ------------------------------------------------------------ frames and determinism
    println!("\n## The synth stays on the stretched voice\n");
    let wsola = rows4.iter().find(|r| r.method == Method::Wsola).unwrap();
    let (lag, corr) = frames_follow(&sung, &map4, &wsola.out);
    println!(
        "Case 4 with WSOLA: the loudness the synth reads from the stored frames through the warp map, against the loudness of the stretched audio re-analysed: best lag {:.1} ms (positive: the audio comes later), correlation {:.3} at lag 0.",
        lag, corr
    );
    let vs = rows4.iter().find(|r| r.method == Method::Varispeed).unwrap();
    let (lag_v, corr_v) = frames_follow(&sung, &map4, &vs.out);
    println!("Same check for varispeed: lag {:.1} ms, correlation {:.3}.", lag_v, corr_v);

    println!("\n## Determinism\n");
    println!(
        "Two whole renders; a render in calls of random size (1 to 3,000 samples); eight plays started from points spread over the take (a seek, as Play from a stop does), each rendered twice; and, for those plays, how long after the start point their output still differs from the whole render. \"Equal from the next anchor\" checks that every difference ends before the next warp point.\n"
    );
    println!("| Case | Method | Two renders equal | Call size changes nothing | Play from a point repeats exactly | Equal from the next anchor | Longest difference after a start point, ms |");
    println!("| --- | --- | --- | --- | --- | --- | --- |");
    determinism("1 (drums)", &drum_a.audio, &map1, p);
    determinism("4 (sung)", &sung.audio, &map4, p);

    println!("\n## Cost in the worklet's call size\n");
    println!("| Take | Method | ns per sample, whole take in one call | ns per sample, 128-sample calls |");
    println!("| --- | --- | --- | --- |");
    for (name, take, map) in [("drums A", &drum_a, &map1), ("sung", &sung, &map4)] {
        for m in [Method::Slice, Method::Wsola, Method::Pv] {
            let len = take.audio.len();
            let time = |chunk: usize| {
                let mut best = f64::INFINITY;
                for _ in 0..5 {
                    let mut s = Stretcher::new(m, p, &take.audio, map);
                    let mut out = vec![0.0f32; len];
                    let t = Instant::now();
                    for c in out.chunks_mut(chunk) {
                        s.process(c);
                    }
                    best = best.min(t.elapsed().as_secs_f64());
                }
                best * 1e9 / len as f64
            };
            println!("| {name} | {} | {:.1} | {:.1} |", m.name(), time(len), time(128));
        }
    }

    let t0 = Instant::now();
    let mut n = 0;
    for _ in 0..100 {
        let m = quantize(&s_notes, &Grid { bpm: BPM, origin: sung.lead }, &Target { res: 0.5, feel: Feel::Map(groove_a.clone()), strength: &full, window: 0.1, min_gap: gap });
        n += warp_map(&m, sung.audio.len(), SR).len();
    }
    println!(
        "\nQuantize plus warp map for the sung line following the drums: {:.1} us per run ({n} points over 100 runs).",
        t0.elapsed().as_secs_f64() * 1e6 / 100.0
    );
}
