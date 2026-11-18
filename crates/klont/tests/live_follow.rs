//! Live follow through the klont engine's `process`: a synthetic voice goes
//! in as mic input and must drive the armed layer (level within a few ms,
//! pitch lock inside the budget), beats must land on kit slots, frames must
//! match the stored-take path exactly, and a disarmed engine stays silent.

use dsp::capture::frames_from_signal;
use klont::Engine;
use klont::curve::{CURVE_STRIDE, TUNED};
use klont::protocol::*;

const SR: f32 = 48_000.0;
const BLOCK: usize = 128;

/// Harmonic voice with two formant bells at `hz`, as a stand-in for a sung "ah".
fn hum(hz: f32, n: usize, amp: f32) -> Vec<f32> {
    let mut out = vec![0.0; n];
    let kmax = ((SR * 0.45) / hz) as usize;
    for k in 1..=kmax {
        let f = hz * k as f32;
        let formant = |fc: f32, bw: f32| (-(f - fc).powi(2) / (2.0 * bw * bw)).exp();
        let a = (1.0 / k as f32) * (0.2 + formant(700.0, 150.0) + 0.6 * formant(1100.0, 200.0));
        let w = std::f32::consts::TAU * f / SR;
        for (i, x) in out.iter_mut().enumerate() {
            *x += a * (w * i as f32).sin();
        }
    }
    let peak = out.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
    out.iter_mut().for_each(|x| *x *= amp / peak);
    out
}

/// Autocorrelation f0 over a window, 70..1000 Hz. Returns 0 for silence.
fn f0(x: &[f32]) -> f32 {
    let energy: f32 = x.iter().map(|v| v * v).sum();
    if energy < 1e-6 {
        return 0.0;
    }
    let (min_lag, max_lag) = ((SR / 1000.0) as usize, (SR / 70.0) as usize);
    let mut best = (0usize, 0.0f32);
    for lag in min_lag..max_lag.min(x.len() / 2) {
        let c: f32 = (0..x.len() - lag).map(|i| x[i] * x[i + lag]).sum::<f32>() / energy;
        if c > best.1 {
            best = (lag, c);
        }
    }
    if best.1 < 0.5 { 0.0 } else { SR / best.0 as f32 }
}

struct Run {
    out: Vec<f32>,
    frames: Vec<f32>,
    /// Per block: hits counter and last slot after the call.
    hits: Vec<(f32, f32)>,
}

fn run(e: &mut Engine, input: &[f32]) -> Run {
    let mut r = Run { out: Vec::with_capacity(input.len()), frames: Vec::new(), hits: Vec::new() };
    for chunk in input.chunks(BLOCK) {
        e.input_mut()[..chunk.len()].copy_from_slice(chunk);
        e.process(chunk.len());
        r.out.extend_from_slice(&e.output().0[..chunk.len()]);
        r.frames.extend_from_slice(e.frame_records());
        r.hits.push((e.meter()[M_LIVE_HITS], e.meter()[M_LIVE_SLOT]));
    }
    r
}

/// A melody layer armed, plain sound: partials only, so pitch is measurable.
fn armed_engine(layer: u8) -> Box<Engine> {
    let mut e = Engine::new(SR, BLOCK);
    e.push(MSG_ARM, layer, 0, 0, 1.0, 0);
    e
}

fn onset(x: &[f32], threshold: f32) -> usize {
    x.iter().position(|v| v.abs() > threshold).unwrap_or(x.len())
}

fn silence(sec: f32) -> Vec<f32> {
    vec![0.0; (sec * SR) as usize]
}

/// Level delay and pitch lock in ms of the note starting at `start`.
fn measure(out: &[f32], start: usize, note: usize, hz: f32) -> (f32, f32) {
    let out_on = onset(&out[start - 1000..], 0.01) + start - 1000;
    let level = (out_on as f32 - start as f32) / SR * 1000.0;
    let win = (0.02 * SR) as usize;
    let hop = (0.002 * SR) as usize;
    let mut t = start;
    let lock = loop {
        let est = f0(&out[t..t + win]);
        if est > 0.0 && (1200.0 * (est / hz).log2()).abs() < 50.0 {
            // Three windows in a row.
            let ok = (1..3).all(|k| {
                let e = f0(&out[t + k * win..t + (k + 1) * win]);
                e > 0.0 && (1200.0 * (e / hz).log2()).abs() < 50.0
            });
            if ok {
                break t;
            }
        }
        t += hop;
        assert!(t + 4 * win < start + note, "never locked to {hz} Hz");
    };
    (level, (lock as f32 - start as f32) / SR * 1000.0)
}

#[test]
fn hum_drives_the_armed_melody_layer() {
    let mut e = armed_engine(0);
    let (sil, note) = (silence(0.5), (0.6 * SR) as usize);
    let mut input = sil.clone();
    input.extend(hum(220.0, note, 0.3));
    input.extend(sil.clone());
    input.extend(hum(330.0, note, 0.3));
    input.extend(sil.clone());
    let r = run(&mut e, &input);
    assert!(r.out.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
    assert!(r.out[..sil.len() - 1000].iter().all(|x| x.abs() < 1e-3), "output before input");

    let mut level = Vec::new();
    let mut pitch = Vec::new();
    for (i, hz) in [(0usize, 220.0f32), (1, 330.0)] {
        let start = sil.len() + i * (note + sil.len());
        let (l, p) = measure(&r.out, start, note, hz);
        level.push(l);
        pitch.push(p);
        let win = (0.02 * SR) as usize;
        let mid = start + note / 2;
        let est = f0(&r.out[mid..mid + win]);
        assert!((1200.0 * (est / hz).log2()).abs() < 20.0, "mid-note f0 {est} vs {hz}");
    }
    let tail = &r.out[r.out.len() - sil.len() / 2..];
    assert!(tail.iter().all(|x| x.abs() < 1e-3), "gate did not close");
    println!("native live follow: level delay {level:?} ms, pitch lock {pitch:?} ms");
    assert!(level.iter().all(|&d| d <= 3.0), "level delay {level:?}");
    assert!(pitch.iter().all(|&d| d <= 40.0), "pitch lock {pitch:?}");
}

#[test]
fn octave_moves_the_carrier() {
    let mut e = armed_engine(0);
    e.push(MSG_SET_TUNE, 0, 0, TUNE_OCTAVE, 1.0, 0);
    let mut input = silence(0.3);
    input.extend(hum(220.0, (0.6 * SR) as usize, 0.3));
    let r = run(&mut e, &input);
    let mid = input.len() - (0.2 * SR) as usize;
    let est = f0(&r.out[mid..mid + 960]);
    assert!((1200.0 * (est / 440.0).log2()).abs() < 25.0, "f0 {est}");
    let tuned = e.meter()[M_TUNED];
    let raw = e.meter()[M_PITCH];
    assert!((tuned - raw - 1200.0).abs() < 60.0, "tuned {tuned} raw {raw}");
}

/// A dark thump: low-passed noise burst with a fast decay.
fn thump(n: usize) -> Vec<f32> {
    let mut rng = 0x1234_5678u32;
    let mut lp = 0.0f32;
    (0..n)
        .map(|i| {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            let w = (rng >> 8) as f32 / (1u32 << 23) as f32 - 1.0;
            lp += 0.03 * (w - lp);
            let t = i as f32 / SR;
            lp * 25.0 * (-t / 0.04).exp() * 0.6
        })
        .collect()
}

/// A bright noise burst: high-passed noise, fast decay.
fn burst(n: usize) -> Vec<f32> {
    let mut rng = 0x9e37_79b9u32;
    let mut prev = 0.0f32;
    (0..n)
        .map(|i| {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            let w = (rng >> 8) as f32 / (1u32 << 23) as f32 - 1.0;
            let h = w - prev;
            prev = w;
            let t = i as f32 / SR;
            h * 0.3 * (-t / 0.03).exp()
        })
        .collect()
}

fn beat_input() -> (Vec<f32>, [usize; 2]) {
    let mut input = silence(0.6);
    let a = input.len();
    input.extend(thump((0.25 * SR) as usize));
    input.extend(silence(0.5));
    let b = input.len();
    input.extend(burst((0.25 * SR) as usize));
    input.extend(silence(0.5));
    (input, [a, b])
}

/// First block (as an index) at which the hit counter went up after `from`.
fn hit_block(hits: &[(f32, f32)], from: usize) -> Option<usize> {
    (from / BLOCK..hits.len()).find(|&i| hits[i].0 > if i == 0 { 0.0 } else { hits[i - 1].0 })
}

fn beat_engine() -> Box<Engine> {
    let mut e = Engine::new(SR, BLOCK);
    e.push(MSG_SET_LAYER, 0, 0, LAYER_KIND, KIND_BEAT as f32, 0);
    e.push(MSG_ARM, 0, 0, 0, 1.0, 0);
    e
}

#[test]
fn beat_hits_pick_slots_by_brightness() {
    let mut e = beat_engine();
    let (input, [a, b]) = beat_input();
    let r = run(&mut e, &input);
    let ia = hit_block(&r.hits, a - 2 * BLOCK).expect("no hit for the thump");
    let ib = hit_block(&r.hits, b - 2 * BLOCK).expect("no hit for the burst");
    let (sa, sb) = (r.hits[ia].1, r.hits[ib].1);
    println!("thump: hit at block {ia} (input at {}), slot {sa}; burst: block {ib} (input at {}), slot {sb}", a / BLOCK, b / BLOCK);
    assert_ne!(sa, sb, "dark and bright hits share slot {sa}");
    assert!(sa < sb, "dark {sa} should sit below bright {sb}");
    // Each hit sounds within one quantum of the input that caused it (the
    // tier's own analysis delay is the level delay, tested separately).
    for (i, s) in [(ia, a), (ib, b)] {
        assert!(i <= s / BLOCK + 1, "hit {i} too late for input at {s}");
        let seg = &r.out[i * BLOCK..(i + 1) * BLOCK];
        assert!(seg.iter().any(|x| x.abs() > 1e-4), "hit {i} silent in its own quantum");
    }
    assert_eq!(r.hits.last().unwrap().0, 2.0);
}

#[test]
fn taught_kit_routes_to_the_taught_slot() {
    // Teach slot 3 to the thump's own features and slot 1 far away: the
    // bright burst is then nearer slot 1 only if it is taught nearer.
    let (input, _) = beat_input();
    let mut e = beat_engine();
    e.push(MSG_SET_LIVE_KIT, 0, 3, LIVE_KIT_TAUGHT, 1.0, 0);
    e.push(MSG_SET_LIVE_KIT, 0, 3, LIVE_KIT_BRIGHT, 0.9, 0);
    e.push(MSG_SET_LIVE_KIT, 0, 3, LIVE_KIT_LOW, 0.1, 0);
    e.push(MSG_SET_LIVE_KIT, 0, 3, LIVE_KIT_NOISE, 0.9, 0);
    e.push(MSG_SET_LIVE_KIT, 0, 2, LIVE_KIT_TAUGHT, 1.0, 0);
    e.push(MSG_SET_LIVE_KIT, 0, 2, LIVE_KIT_BRIGHT, 0.1, 0);
    e.push(MSG_SET_LIVE_KIT, 0, 2, LIVE_KIT_LOW, 0.9, 0);
    e.push(MSG_SET_LIVE_KIT, 0, 2, LIVE_KIT_NOISE, 0.0, 0);
    let r = run(&mut e, &input);
    let (input2, [a2, b2]) = beat_input();
    assert_eq!(input.len(), input2.len());
    let ia = hit_block(&r.hits, a2 - 2 * BLOCK).expect("no thump hit");
    let ib = hit_block(&r.hits, b2 - 2 * BLOCK).expect("no burst hit");
    assert_eq!(r.hits[ia].1, 2.0, "dark thump goes to the dark taught slot");
    assert_eq!(r.hits[ib].1, 3.0, "bright burst goes to the bright taught slot");
}

#[test]
fn frames_are_counted_laid_out_and_match_the_stored_path() {
    let mut e = armed_engine(0);
    let mut input = silence(0.3);
    input.extend(hum(220.0, (0.5 * SR) as usize, 0.3));
    input.extend(hum(262.0, (0.3 * SR) as usize, 0.3));
    input.extend(silence(0.3));
    let cap = e.table().frames_cap as usize;
    assert_eq!(cap, (BLOCK as f32 / (SR / 100.0)).ceil() as usize + 2);
    let mut per_call = Vec::new();
    let mut frames = Vec::new();
    for chunk in input.chunks(BLOCK) {
        e.input_mut()[..chunk.len()].copy_from_slice(chunk);
        e.process(chunk.len());
        let n = e.frame_records().len() / CURVE_STRIDE;
        assert!(n <= cap && n <= 1, "{n} frames in one call");
        per_call.push(n);
        frames.extend_from_slice(e.frame_records());
    }
    let total: usize = per_call.iter().sum();
    assert_eq!(total, input.len() / 480, "one frame per 10 ms");

    let reference = frames_from_signal(&input, SR, 16);
    assert_eq!(reference.len(), total);
    for (i, f) in reference.iter().enumerate() {
        let rec = &frames[i * CURVE_STRIDE..(i + 1) * CURVE_STRIDE];
        for k in 0..24 {
            assert!(rec[k].to_bits() == f[k].to_bits(), "frame {i} field {k}: {} vs {}", rec[k], f[k]);
        }
    }
    // Tuned pitch: NaN when unvoiced, near the raw pitch when voiced and no tuning.
    let mut voiced = 0;
    for i in 0..total {
        let rec = &frames[i * CURVE_STRIDE..(i + 1) * CURVE_STRIDE];
        if rec[7] == 0.0 {
            voiced += 1;
            assert!(rec[TUNED].is_nan() || (rec[TUNED] - rec[0]).abs() < 200.0, "frame {i}: {} vs {}", rec[TUNED], rec[0]);
        } else {
            assert!(rec[TUNED].is_nan(), "frame {i} unvoiced but tuned {}", rec[TUNED]);
        }
    }
    assert!(voiced > 50, "{voiced} voiced frames");
    // Offline render makes none.
    e.render(BLOCK);
    assert_eq!(e.frame_records().len(), 0);
}

#[test]
fn disarmed_is_silent_but_the_tier_keeps_running() {
    let mut e = Engine::new(SR, BLOCK);
    let mut input = silence(0.3);
    input.extend(hum(220.0, (0.6 * SR) as usize, 0.3));
    let r = run(&mut e, &input);
    assert!(r.out.iter().all(|x| x.abs() < 1e-6), "disarmed engine made sound");
    assert!(r.frames.len() / CURVE_STRIDE > 50);
    let m = e.meter();
    assert_eq!(m[M_ARMED], -1.0);
    assert!(m[M_IN_DB] > -40.0, "in {}", m[M_IN_DB]);
    assert_eq!(m[M_GATE], 1.0);
    assert!(m[M_PITCH].is_finite());
    // Arming one layer disarms the other.
    e.push(MSG_ARM, 1, 0, 0, 1.0, 0);
    e.push(MSG_ARM, 2, 0, 0, 1.0, 0);
    e.process(BLOCK);
    assert_eq!(e.meter()[M_ARMED], 2.0);
    e.push(MSG_ARM, 2, 0, 0, 0.0, 0);
    e.process(BLOCK);
    assert_eq!(e.meter()[M_ARMED], -1.0);
}
