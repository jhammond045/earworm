//! End-to-end Live follow through the engine's process call: a synthetic hum
//! goes in as mic input, and the synth output must follow its pitch and level.
//! Also measures the engine's internal voice-to-output delay (no devices), the
//! deterministic part of the Live follow delay budget.

use dsp::frame;
use dsp::params;
use engine::Engine;
use engine::protocol::*;

const SR: f32 = 48_000.0;
const BLOCK: usize = 128;

/// Band-limited sawtooth at `hz` through two vowel-ish formant bells, as a
/// stand-in for a sung "ah".
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
}

fn run(engine: &mut Engine, input: &[f32]) -> Run {
    let mut out = Vec::with_capacity(input.len());
    let mut frames = Vec::new();
    for chunk in input.chunks(BLOCK) {
        engine.input_mut()[..chunk.len()].copy_from_slice(chunk);
        engine.process(chunk.len());
        out.extend_from_slice(&engine.output()[..chunk.len()]);
        frames.extend_from_slice(engine.frame_records());
    }
    Run { out, frames }
}

fn live_engine(n_bands: usize) -> Box<Engine> {
    let mut e = Engine::new(SR, BLOCK, n_bands);
    e.push_message(MSG_SET_PARAM, 0, params::DEPTH_MOUTH as u16, 0.0);
    e.push_message(MSG_SET_PARAM, 0, params::NOISE_MIX as u16, 0.0);
    e.push_message(MSG_SET_PARAM, 0, params::LEVEL as u16, 0.0);
    e.push_message(MSG_SET_LIVE, 0, 0, 1.0);
    e
}

fn onset(x: &[f32], threshold: f32) -> usize {
    x.iter().position(|v| v.abs() > threshold).unwrap_or(x.len())
}

#[test]
fn follows_pitch_and_measures_internal_delay() {
    let mut e = live_engine(16);
    let silence = (0.5 * SR) as usize;
    let note = (0.6 * SR) as usize;
    let mut input = vec![0.0; silence];
    input.extend(hum(220.0, note, 0.3));
    input.extend(vec![0.0; silence]);
    input.extend(hum(330.0, note, 0.3));
    input.extend(vec![0.0; silence]);
    let r = run(&mut e, &input);

    assert!(r.out.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
    // Silence before the first note is silent.
    assert!(r.out[..silence - 1000].iter().all(|x| x.abs() < 1e-3), "output before input");

    let mut level_ms = Vec::new();
    let mut pitch_ms = Vec::new();
    for (i, hz) in [(0usize, 220.0f32), (1, 330.0)] {
        let start = silence + i * (note + silence);
        let out_on = onset(&r.out[start - 1000..], 0.01) + start - 1000;
        level_ms.push((out_on as f32 - start as f32) / SR * 1000.0);
        // Pitch lock: first 20 ms window (2 ms hop) within 50 cents of the input.
        let win = (0.02 * SR) as usize;
        let hop = (0.002 * SR) as usize;
        let mut t = start;
        let lock = loop {
            let est = f0(&r.out[t..t + win]);
            if est > 0.0 && (1200.0 * (est / hz).log2()).abs() < 50.0 {
                break t;
            }
            t += hop;
            assert!(t < start + note / 2, "never locked to {hz} Hz");
        };
        pitch_ms.push((lock as f32 - start as f32) / SR * 1000.0);
        // Mid-note the output holds the input pitch.
        let mid = start + note / 2;
        let est = f0(&r.out[mid..mid + win]);
        assert!((1200.0 * (est / hz).log2()).abs() < 20.0, "mid-note f0 {est} vs {hz}");
    }
    // Requirement 9: silence after the notes is silent again.
    let tail = &r.out[r.out.len() - silence / 2..];
    assert!(tail.iter().all(|x| x.abs() < 1e-3), "gate did not close");

    eprintln!("engine internal delay: level {level_ms:?} ms, pitch lock {pitch_ms:?} ms");
    // TDD budget: analysis about 5 ms for level, about 20 ms for pitch, plus
    // one 2.7 ms quantum. Allow headroom for the gate attack.
    assert!(level_ms.iter().all(|&d| d < 15.0), "level delay {level_ms:?}");
    assert!(pitch_ms.iter().all(|&d| d < 45.0), "pitch delay {pitch_ms:?}");

    // Frames: 100 per second, the documented record stride.
    let records = r.frames.len() / RECORD_STRIDE;
    let expected = (input.len() as f32 / SR * frame::FRAME_RATE) as usize;
    assert!((records as i64 - expected as i64).abs() <= 1, "{records} frames vs {expected}");
}

#[test]
fn twenty_four_bands_run_the_same_path() {
    let mut e = live_engine(24);
    let mut input = vec![0.0; (0.3 * SR) as usize];
    input.extend(hum(196.0, (0.5 * SR) as usize, 0.3));
    let r = run(&mut e, &input);
    assert_eq!(e.meter()[M_N_BANDS], 24.0);
    let mid = input.len() - (0.1 * SR) as usize;
    let est = f0(&r.out[mid..mid + 960]);
    assert!((1200.0 * (est / 196.0).log2()).abs() < 20.0, "f0 {est}");
}

#[test]
fn snap_pulls_a_flat_note_onto_the_scale() {
    // 30 cents flat of A3 with full snap and no lag comes out on A3.
    let mut e = live_engine(16);
    e.push_message(MSG_SET_PARAM, 0, params::TUNE_SNAP as u16, 1.0);
    e.push_message(MSG_SET_PARAM, 0, params::TUNE_SPEED as u16, 0.0);
    let hz = 220.0 * (-30.0f32 / 1200.0).exp2();
    let mut input = vec![0.0; (0.3 * SR) as usize];
    input.extend(hum(hz, (0.6 * SR) as usize, 0.3));
    let r = run(&mut e, &input);
    let mid = input.len() - (0.15 * SR) as usize;
    let est = f0(&r.out[mid..mid + 960]);
    assert!((1200.0 * (est / 220.0).log2()).abs() < 10.0, "f0 {est}, wanted 220");
}

#[test]
fn offline_render_plays_a_note() {
    let mut e = Engine::new(SR, 4096, 16);
    e.push_message(MSG_NOTE_ON, 0, 0, dsp::hz_to_cents(440.0));
    let mut out = Vec::new();
    for _ in 0..6 {
        let n = e.render(4096);
        out.extend_from_slice(&e.output()[..n]);
    }
    let est = f0(&out[12_000..12_960]);
    assert!((1200.0 * (est / 440.0).log2()).abs() < 10.0, "f0 {est}");
}
