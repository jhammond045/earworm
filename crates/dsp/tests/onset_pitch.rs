//! Regression: at a note start after silence, the first YIN windows are part
//! silence and can land on a harmonic. Such an estimate must not reach the
//! tuner, and must not lock the continuity guard onto the wrong pitch.

use dsp::capture::{FrameSink, LiveTier};

const SR: f32 = 48_000.0;

/// Sawtooth-like hum with a strong formant near the 5th-6th harmonic and a
/// 10 ms fade-in, like the delay page's test signal.
fn hum(f0: f32, n: usize) -> Vec<f32> {
    let kmax = (0.45 * SR / f0) as usize;
    let formant = |f: f32| 0.05 + 1.0 / (1.0 + ((f - 700.0) / 130.0).powi(2)) + 0.7 / (1.0 + ((f - 1220.0) / 90.0).powi(2));
    let mut x = vec![0.0f32; n];
    for k in 1..=kmax {
        let f = f0 * k as f32;
        let a = formant(f) / k as f32;
        let w = std::f32::consts::TAU * f / SR;
        for (i, v) in x.iter_mut().enumerate() {
            *v += a * (w * i as f32).sin();
        }
    }
    let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let fade = (0.01 * SR) as usize;
    for (i, v) in x.iter_mut().enumerate() {
        *v *= 0.3 / peak * (i as f32 / fade as f32).min(1.0);
    }
    x
}

#[test]
fn first_pitch_after_silence_is_right_and_prompt() {
    let mut signal = vec![0.0f32; (0.4 * SR) as usize];
    let onset = signal.len();
    signal.extend(hum(220.0, (0.3 * SR) as usize));
    let mut tier = LiveTier::new(SR, 16);
    let mut sink = FrameSink::with_capacity(8);
    let mut first_ok = None;
    let target = dsp::hz_to_cents(220.0);
    for (b, chunk) in signal.chunks(128).enumerate() {
        let mut buf = chunk.to_vec();
        sink.clear();
        tier.process(&mut buf, &mut sink);
        let g = tier.gesture();
        let t_ms = ((b + 1) * 128) as f32 / SR * 1000.0 - onset as f32 / SR * 1000.0;
        for &(cents, _) in &g.pitch_updates[..g.n_pitch_updates] {
            if cents.is_finite() {
                assert!((cents - target).abs() < 50.0, "wrong pitch {:.0} Hz at {t_ms:.1} ms", dsp::cents_to_hz(cents));
                first_ok.get_or_insert(t_ms);
            }
        }
    }
    let t = first_ok.expect("never tracked");
    assert!(t < 30.0, "first pitch at {t:.1} ms");
}
