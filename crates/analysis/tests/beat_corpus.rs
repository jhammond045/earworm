//! Beat analysis on a deterministic synthetic corpus (TDD "Testing", beat analysis):
//! detection rate, false hits per 8 bars, grouping agreement and tempo error.
//!
//! 50 beatbox takes: 5 patterns (1, 2, 3, 4 and 3 sound classes) at 5 tempos, two
//! seeded timbre draws each, 4 bars of 4/4 on an eighth-note grid, with timing
//! jitter, velocity changes and room noise. Sounds: kick (sine with pitch drop), snare
//! (mid noise plus a short body), hat (short high noise), "pop" (short voiced burst
//! that stops dead), "boom" (long low thump) and "nnt" (nasal tone ending in a click).
//! A found hit matches a true hit within 50 ms (one to one).

mod common;
use analysis::second::{Kind, Options, second_pass};
use common::*;

const TAU: f32 = std::f32::consts::TAU;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Snd {
    Kick,
    Snare,
    Hat,
    Pop,
    Boom,
    Nnt,
}

struct Timbre {
    kick_hi: f32,
    kick_lo: f32,
    kick_tau: f32,
    snare_body: f32,
    snare_tau: f32,
    hat_tau: f32,
    pop_hz: f32,
    nnt_hz: f32,
}

fn timbre(r: &mut Rng) -> Timbre {
    Timbre {
        kick_hi: r.range(110.0, 190.0),
        kick_lo: r.range(45.0, 62.0),
        kick_tau: r.range(0.08, 0.14),
        snare_body: r.range(170.0, 230.0),
        snare_tau: r.range(0.045, 0.075),
        hat_tau: r.range(0.010, 0.018),
        pop_hz: r.range(170.0, 260.0),
        nnt_hz: r.range(120.0, 190.0),
    }
}

/// Harmonic tone with a rough formant, amplitude from `env`.
fn tone(n: usize, hz: f32, env: impl Fn(f32) -> f32, formant: f32) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let t = i as f32 / SR;
            let mut s = 0.0;
            for k in 1..=12 {
                let f = k as f32 * hz;
                let d = (f - formant) / (formant * 0.5);
                s += (TAU * f * t).sin() / (k as f32) * (0.3 + 1.0 / (1.0 + d * d));
            }
            0.35 * s * env(t)
        })
        .collect()
}

fn render(s: Snd, tb: &Timbre, r: &mut Rng) -> Vec<f32> {
    match s {
        Snd::Kick => {
            let n = (0.4 * SR) as usize;
            let mut ph = 0.0f32;
            (0..n)
                .map(|i| {
                    let t = i as f32 / SR;
                    let f = tb.kick_lo + (tb.kick_hi - tb.kick_lo) * (-t / 0.035).exp();
                    ph += f / SR;
                    let click = if t < 0.003 { 0.4 * r.noise() } else { 0.0 };
                    0.8 * (TAU * ph).sin() * (-t / tb.kick_tau).exp() + click
                })
                .collect()
        }
        Snd::Snare => {
            let n = (0.25 * SR) as usize;
            let (mut a, mut b) = (0.0f32, 0.0f32);
            (0..n)
                .map(|i| {
                    let t = i as f32 / SR;
                    let w = r.noise();
                    a += 0.25 * (w - a);
                    b += 0.03 * (a - b);
                    let band = a - b; // roughly 300 Hz to 3 kHz
                    let body = (TAU * tb.snare_body * t).sin() * (-t / 0.04).exp() * 0.3;
                    1.4 * band * (-t / tb.snare_tau).exp() + body
                })
                .collect()
        }
        Snd::Hat => {
            let n = (0.12 * SR) as usize;
            let (mut x1, mut x2) = (0.0f32, 0.0f32);
            (0..n)
                .map(|i| {
                    let t = i as f32 / SR;
                    let w = r.noise();
                    let y = w - 2.0 * x1 + x2; // second difference: strong highs
                    x2 = x1;
                    x1 = w;
                    0.35 * y * (-t / tb.hat_tau).exp()
                })
                .collect()
        }
        Snd::Pop => {
            let n = (0.06 * SR) as usize;
            let mut v = tone(n, tb.pop_hz, |t| (t / 0.004).min(1.0) * if t < 0.045 { 1.0 } else { (1.0 - (t - 0.045) / 0.003).max(0.0) }, 700.0);
            for (i, x) in v.iter_mut().enumerate().take((0.004 * SR) as usize) {
                *x += 0.3 * r.noise() * (1.0 - i as f32 / (0.004 * SR));
            }
            v
        }
        Snd::Boom => {
            let n = (0.6 * SR) as usize;
            let mut ph = 0.0f32;
            (0..n)
                .map(|i| {
                    let t = i as f32 / SR;
                    ph += (46.0 + 30.0 * (-t / 0.05).exp()) / SR;
                    0.9 * (TAU * ph).sin() * (-t / 0.22).exp()
                })
                .collect()
        }
        Snd::Nnt => {
            let dur = 0.13;
            let n = ((dur + 0.012) * SR) as usize;
            let mut v = tone(n, tb.nnt_hz, |t| (t / 0.02).min(1.0) * if t < dur { 1.0 } else { 0.0 }, 300.0);
            // Closing "t": a short high noise burst right at the end.
            let (mut x1, mut x2) = (0.0f32, 0.0f32);
            for (i, x) in v.iter_mut().enumerate() {
                let t = i as f32 / SR;
                if t >= dur - 0.002 {
                    let w = r.noise();
                    *x += 0.4 * (w - 2.0 * x1 + x2) * (-(t - dur + 0.002) / 0.005).exp();
                    x2 = x1;
                    x1 = w;
                }
            }
            v
        }
    }
}

type Pattern = &'static [(usize, Snd)];

/// (name, eighth-note step within a 4/4 bar, sound) per pattern.
fn patterns() -> Vec<(&'static str, Pattern, usize)> {
    use Snd::*;
    vec![
        ("kicks", &[(0, Kick), (2, Kick), (4, Kick), (6, Kick)], 1),
        ("backbeat", &[(0, Kick), (2, Snare), (4, Kick), (5, Kick), (6, Snare)], 2),
        ("hats", &[(0, Kick), (1, Hat), (2, Snare), (3, Hat), (4, Kick), (5, Hat), (6, Snare), (7, Hat)], 3),
        ("four", &[(0, Kick), (1, Hat), (2, Snare), (3, Pop), (4, Kick), (5, Hat), (6, Snare), (7, Hat)], 4),
        ("voiced", &[(0, Boom), (2, Pop), (3, Pop), (4, Boom), (6, Nnt)], 3),
    ]
}

struct Take {
    audio: Vec<f32>,
    hits: Vec<(f32, usize)>, // time, class index
    bpm: f32,
    bars: usize,
    classes: usize,
    name: &'static str,
}

fn make_take(pi: usize, bpm: f32, seed: u64) -> Take {
    let (name, pat, classes) = patterns()[pi].clone();
    let mut r = Rng::new(seed);
    let tb = timbre(&mut r);
    let bars = 4;
    let beat = 60.0 / bpm;
    let lead = 0.3;
    let total = lead + bars as f32 * 4.0 * beat + 0.8;
    let mut audio = vec![0.0f32; (total * SR) as usize];
    let mut kinds_seen: Vec<Snd> = Vec::new();
    let mut hits = Vec::new();
    for b in 0..bars {
        for &(step, snd) in pat {
            let jitter = r.range(-0.010, 0.010);
            let t = lead + (b as f32 * 8.0 + step as f32) * beat * 0.5 + jitter;
            let vel = r.range(0.55, 1.0);
            let w = render(snd, &tb, &mut r);
            let a = (t * SR) as usize;
            for (i, v) in w.iter().enumerate() {
                if a + i < audio.len() {
                    audio[a + i] += 0.5 * vel * v;
                }
            }
            if !kinds_seen.contains(&snd) {
                kinds_seen.push(snd);
            }
            hits.push((t, kinds_seen.iter().position(|&k| k == snd).unwrap()));
        }
    }
    let floor = 10f32.powf(r.range(-68.0, -58.0) / 20.0);
    let rumble = 10f32.powf(r.range(-58.0, -48.0) / 20.0);
    let mut lp = 0.0f32;
    for a in audio.iter_mut() {
        lp += 0.02 * (r.normal() - lp);
        *a += floor * r.normal() + rumble * lp * 4.0;
    }
    Take { audio, hits, bpm, bars, classes, name }
}

/// Best agreement over injective maps of true classes to slots.
fn best_agreement(pairs: &[(usize, u8)], classes: usize) -> usize {
    fn go(pairs: &[(usize, u8)], class: usize, classes: usize, used: &mut [bool; 4], map: &mut Vec<usize>, best: &mut usize) {
        if class == classes {
            let n = pairs.iter().filter(|(c, s)| map[*c] == *s as usize).count();
            *best = (*best).max(n);
            return;
        }
        for s in 0..4 {
            if !used[s] {
                used[s] = true;
                map.push(s);
                go(pairs, class + 1, classes, used, map, best);
                map.pop();
                used[s] = false;
            }
        }
    }
    let mut best = 0;
    go(pairs, 0, classes.min(4), &mut [false; 4], &mut Vec::new(), &mut best);
    best
}

#[test]
fn beats_corpus() {
    let tempos = [72.0f32, 90.0, 110.0, 128.0, 146.0];
    let (mut truth, mut tp, mut found, mut false_hits) = (0usize, 0usize, 0usize, 0usize);
    let (mut agree, mut matched, mut bars_total) = (0usize, 0usize, 0usize);
    let (mut beat_kind, mut takes, mut kinds_ok, mut tempo_found, mut tempo_ok, mut tempo_exact) = (0, 0, 0, 0, 0, 0);
    let mut worst_t = 0.0f32;
    let mut tempo_errs: Vec<f32> = Vec::new();
    for pi in 0..patterns().len() {
        for (ti, &bpm) in tempos.iter().enumerate() {
            for s in 0..2u64 {
                let take = make_take(pi, bpm, 100 * (pi as u64 + 1) + 10 * ti as u64 + s);
                let rec = records(&take.audio, SR);
                let r = second_pass(&take.audio, SR, &rec, &Options::default());
                takes += 1;
                beat_kind += (r.kind == Kind::Beat) as usize;
                // One-to-one matching by time.
                let mut used = vec![false; r.hits.len()];
                let mut pairs: Vec<(usize, u8)> = Vec::new();
                let mut feats: Vec<(usize, [f32; 6])> = Vec::new();
                let mut tp_take = 0;
                for &(t, class) in &take.hits {
                    let best = r.hits.iter().enumerate().filter(|(i, h)| !used[*i] && (h.t - t).abs() <= 0.05).min_by(|a, b| (a.1.t - t).abs().total_cmp(&(b.1.t - t).abs()));
                    if let Some((i, h)) = best {
                        used[i] = true;
                        tp_take += 1;
                        pairs.push((class, h.slot));
                        feats.push((class, h.features));
                        worst_t = worst_t.max((h.t - t).abs());
                    }
                }
                if std::env::var("BEAT_STATS").is_ok() {
                    for c in 0..take.classes {
                        let v: Vec<&[f32; 6]> = feats.iter().filter(|f| f.0 == c).map(|f| &f.1).collect();
                        if v.len() < 3 { continue; }
                        let mut line = format!("STAT {} bpm {} class {c} n {}:", take.name, take.bpm, v.len());
                        for d in 0..6 {
                            let m = v.iter().map(|f| f[d]).sum::<f32>() / v.len() as f32;
                            let sd = (v.iter().map(|f| (f[d] - m).powi(2)).sum::<f32>() / v.len() as f32).sqrt();
                            line += &format!(" {:.2}+-{:.2}", m, sd);
                        }
                        println!("{line}");
                    }
                }
                let fa = r.hits.len() - tp_take;
                truth += take.hits.len();
                tp += tp_take;
                found += r.hits.len();
                false_hits += fa;
                bars_total += take.bars;
                let ag = best_agreement(&pairs, take.classes);
                agree += ag;
                matched += pairs.len();
                kinds_ok += (r.kinds == take.classes) as usize;
                let mut terr = String::from("none");
                if let Some(b) = r.bpm {
                    tempo_found += 1;
                    let e = (b - take.bpm).abs() / take.bpm;
                    let oct = [1.0f32, 2.0, 0.5].iter().map(|k| (b / take.bpm / k - 1.0).abs()).fold(f32::INFINITY, f32::min);
                    tempo_ok += (oct < 0.03) as usize;
                    tempo_exact += (e < 0.03) as usize;
                    tempo_errs.push(e);
                    terr = format!("{:.1}bpm ({:+.1}%)", b, 100.0 * (b - take.bpm) / take.bpm);
                }
                println!(
                    "{:9} {:5.0}bpm seed {:3}: hits {:2}/{:2} found {:2} false {} group {:2}/{:2} kinds {} (true {}) tempo {} kind {:?} [vf {:.2} ons/s {:.1} ev {:.2} stab {:.2} perc {:.2}]",
                    take.name, take.bpm, 100 * (pi as u64 + 1) + 10 * ti as u64 + s, tp_take, take.hits.len(), r.hits.len(), fa, ag, pairs.len(), r.kinds, take.classes, terr, r.kind, r.kind_features[0], r.kind_features[1], r.kind_features[2], r.kind_features[3], r.kind_features[4]
                );
            }
        }
    }
    let det = tp as f32 / truth as f32;
    let fa8 = false_hits as f32 / (bars_total as f32 / 8.0);
    let grp = agree as f32 / matched.max(1) as f32;
    println!(
        "\ncorpus: {takes} takes, {truth} true hits, {found} found. detection {:.3} ({tp} of {truth}), false hits {false_hits} ({:.2} per 8 bars), worst onset error {:.0} ms",
        det, fa8, worst_t * 1000.0
    );
    println!(
        "grouping agreement {:.3} ({agree} of {matched}), kinds right {kinds_ok} of {takes}, kind=beat {beat_kind} of {takes}",
        grp
    );
    println!(
        "tempo: found {tempo_found} of {takes}; within 3% {tempo_exact}, within 3% allowing half or double {tempo_ok}; median error {:.1}%",
        100.0 * { tempo_errs.sort_by(|a, b| a.total_cmp(b)); tempo_errs.get(tempo_errs.len() / 2).copied().unwrap_or(f32::NAN) }
    );
    assert!(takes >= 50);
    assert!(det >= 0.9, "detection {det}");
    assert!(fa8 <= 1.0, "false hits per 8 bars {fa8}");
    assert!(grp >= 0.85, "grouping {grp}");
    assert!(beat_kind as f32 >= 0.85 * takes as f32, "kind=beat {beat_kind} of {takes}");
    assert!(tempo_ok as f32 >= 0.85 * takes as f32, "tempo within 3% (or half, double) {tempo_ok} of {takes}");
}
