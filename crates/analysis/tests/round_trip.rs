//! Calibration of `match_full` against the Klont itself: render known
//! parameter sets through the engine, measure them again, and check the
//! parameters come back within the tolerances below. These tolerances are what
//! the mapping comments in src/full.rs rest on.
//!
//! | row | tolerance |
//! | --- | --- |
//! | pitch | 5 cents |
//! | tilt | 0.5 dB/oct (tilt -3, -6, -12) |
//! | oddeven | 0.1 (at +-0.5) |
//! | noise_mix / harm_mix | within 25% (partials louder than the noise) |
//! | noise band shape | 3 dB rms over bands 1 to 14 |
//! | env attack / decay / sustain / release | 25% / 30% / 0.08 / 35% |
//! | modal, damp, decay (bar strike) | 0.3, 0.15, 50% |
//! | env.pitch_amt / pitch_time | 2 semitones / 20 ms |
//! | lfo1.rate, to_pitch (vibrato) | 0.5 Hz, 25% |
//! | lfo1.to_level (tremolo) | 0.1 |
//! | chaos | found as chaos, amount 0.4 to 1 at true 1 |
//! | strike.click | 0.1 to 0.15, and 0 when there is none |
//! | fold.amt | weak: 0 at none, over 0.15 at 1 |
//! | verb.size / verb.mix | 0.07 / 0.2 (mix up to 0.5) |
//!
//! Known limits, kept as they are: a source whose noise is as loud as its
//! partials (noise_mix / harm_mix near 1 or above) reads the partials and the
//! tilt less well; the Klont's own crush is not read back (it leaves images,
//! not a brick wall, and the match only takes a brick wall as a low-rate
//! recording); fold reads about a quarter of its true amount.

mod klont_util;
use analysis::{FullMatch, FullOpts, match_full};
use klont_util::*;
use params as P;
use std::time::Instant;

const C4: f32 = 4800.0;

fn measure_opts(over: &[(usize, f32)], bands: &[(usize, [f32; 16])], cents: f32, hold: f32, total: f32, opts: FullOpts) -> FullMatch {
    let mut s = real_set(over);
    for &(off, sh) in bands {
        for b in 0..16 {
            s[off + b] = P::to_norm(off + b, sh[b]);
        }
    }
    let x = render_note(&LayerSpec::base(s), None, cents, hold, total);
    match_full(&x, SR, opts)
}

fn measure(over: &[(usize, f32)], cents: f32, hold: f32, total: f32) -> FullMatch {
    measure_opts(over, &[], cents, hold, total, FullOpts::default())
}

fn near(what: &str, got: f32, want: f32, tol: f32) {
    assert!((got - want).abs() <= tol, "{what}: got {got}, want {want} +- {tol}");
}

fn within_pct(what: &str, got: f32, want: f32, pct: f32) {
    assert!(((got - want) / want).abs() <= pct, "{what}: got {got}, want {want} within {}%", pct * 100.0);
}

#[test]
fn tone_rows_come_back() {
    use P::*;
    for tilt in [-3.0f32, -6.0, -12.0] {
        let m = measure(&[(SRC_NOISE_MIX, 0.0), (SRC_TILT, tilt)], C4, 1.2, 1.5);
        assert!(m.pitched);
        near("pitch", m.pitch_cents, C4, 5.0);
        near("tilt", m.real(SRC_TILT), tilt, 0.5);
        near("oddeven", m.real(SRC_ODDEVEN), 0.0, 0.1);
        assert!(m.real(SRC_HARM_MIX) > 0.99 && m.real(SRC_NOISE_MIX) < 0.06, "clean tone has no noise");
    }
    for oe in [-0.5f32, 0.5] {
        let m = measure(&[(SRC_NOISE_MIX, 0.0), (SRC_ODDEVEN, oe)], C4, 1.2, 1.5);
        near("oddeven", m.real(SRC_ODDEVEN), oe, 0.1);
    }
}

#[test]
fn harm_and_noise_levels_follow_the_engine() {
    use P::*;
    // noise_mix over harm_mix, with the partials louder than the noise.
    for (h, n) in [(1.0f32, 0.2f32), (1.0, 0.5), (1.0, 0.9), (0.5, 0.3)] {
        let m = measure(&[(SRC_HARM_MIX, h), (SRC_NOISE_MIX, n)], C4, 1.2, 1.5);
        let ratio = m.real(SRC_NOISE_MIX) / m.real(SRC_HARM_MIX);
        within_pct(&format!("noise/harm at h{h} n{n}"), ratio, n / h, 0.25);
    }
    // A shaped noise: the band shape is read, and the level relation holds
    // with the shape's own power taken into account (the engine's band-passes
    // are constant-Q, so a band's power follows its centre frequency).
    let mut shape = [0.0f32; 16];
    for (b, v) in shape.iter_mut().enumerate() {
        *v = 10.0 - 20.0 * b as f32 / 15.0;
    }
    let m = measure_opts(&[(SRC_HARM_MIX, 0.0), (SRC_NOISE_MIX, 1.0), (ENV_NOISE_DECAY, 4.0)], &[(NOISE_BANDS, shape)], C4, 1.2, 1.5, FullOpts { decaying: false, use_sound: Some(false) });
    assert!(!m.pitched, "noise only is unpitched");
    assert!(m.real(SRC_HARM_MIX) == 0.0);
    let mean = shape.iter().sum::<f32>() / 16.0;
    let mut err = 0.0f32;
    for b in 1..15 {
        let got = m.real(NOISE_BANDS + b);
        err += (got - (shape[b] - mean) - (m.real(NOISE_BANDS + 8) - (shape[8] - mean))).powi(2);
    }
    let rms = (err / 14.0).sqrt();
    assert!(rms < 3.0, "noise band shape rms error {rms} dB");
    // Level relation with a shaped noise and partials.
    let m = measure_opts(&[(SRC_HARM_MIX, 1.0), (SRC_NOISE_MIX, 0.5)], &[(NOISE_BANDS, shape)], C4, 1.2, 1.5, FullOpts::default());
    let mut c = [0.0f32; 16];
    dsp::bands::band_centres(16, &mut c);
    let (mut w, mut p) = (0.0f32, 0.0f32);
    for b in 0..16 {
        w += c[b];
        p += c[b] * 10f32.powf(0.1 * shape[b]);
    }
    let eff = 0.5 * (p / w).sqrt();
    let ratio = m.real(SRC_NOISE_MIX) / m.real(SRC_HARM_MIX);
    within_pct("shaped noise level", ratio, eff, 0.4);
}

#[test]
fn envelope_rows_come_back() {
    use P::*;
    let m = measure(&[(ENV_ATTACK, 0.1), (ENV_DECAY, 0.25), (ENV_SUSTAIN, 0.4), (ENV_RELEASE, 0.3), (SRC_NOISE_MIX, 0.0)], C4, 1.2, 1.8);
    within_pct("attack", m.real(ENV_ATTACK), 0.1, 0.25);
    within_pct("decay", m.real(ENV_DECAY), 0.25, 0.3);
    near("sustain", m.real(ENV_SUSTAIN), 0.4, 0.08);
    within_pct("release", m.real(ENV_RELEASE), 0.3, 0.35);
}

#[test]
fn bar_strike_reads_as_bar_with_damping() {
    use P::*;
    let m = measure(&[(ENV_ATTACK, 0.002), (ENV_DECAY, 0.4), (ENV_SUSTAIN, 0.0), (SRC_NOISE_MIX, 0.0), (SRC_MODAL, 2.0), (SRC_DAMP, 0.5)], C4, 0.1, 1.5);
    near("modal", m.real(SRC_MODAL), 2.0, 0.3);
    near("damp", m.real(SRC_DAMP), 0.5, 0.15);
    // env.decay is the fundamental's own time constant when partials are
    // damped; the 85 ms analysis window smears fast partial decays, so it
    // reads low.
    within_pct("decay", m.real(ENV_DECAY), 0.4, 0.5);
}

#[test]
fn pitch_drop_comes_back() {
    use P::*;
    let m = measure(&[(ENV_PITCH_AMT, 12.0), (ENV_PITCH_TIME, 0.05), (ENV_DECAY, 0.4), (ENV_SUSTAIN, 0.0), (SRC_NOISE_MIX, 0.0)], C4, 0.1, 1.0);
    near("pitch_amt", m.real(ENV_PITCH_AMT), 12.0, 2.0);
    near("pitch_time", m.real(ENV_PITCH_TIME), 0.05, 0.02);
    near("settled pitch", m.pitch_cents, C4, 10.0);
}

#[test]
fn motion_rows_come_back() {
    use P::*;
    let m = measure(&[(LFO1_RATE, 5.0), (LFO1_TO_PITCH, 40.0), (SRC_NOISE_MIX, 0.0)], C4, 2.0, 2.2);
    near("vibrato rate", m.real(LFO1_RATE), 5.0, 0.5);
    within_pct("vibrato depth", m.real(LFO1_TO_PITCH), 40.0, 0.25);
    assert!(m.real(LFO1_TO_LEVEL) < 0.05);
    assert!(!m.provisional, "2 s of 5 Hz is ten cycles");

    let m = measure(&[(LFO1_RATE, 4.0), (LFO1_TO_LEVEL, 0.6), (SRC_NOISE_MIX, 0.0)], C4, 2.0, 2.2);
    near("tremolo rate", m.real(LFO1_RATE), 4.0, 0.5);
    near("tremolo depth", m.real(LFO1_TO_LEVEL), 0.6, 0.1);
    assert!(m.real(LFO1_TO_PITCH) < 1.0);

    let m = measure(&[(CHAOS_AMT, 1.0), (CHAOS_RATE, 3.0), (SRC_NOISE_MIX, 0.0)], C4, 3.0, 3.2);
    assert!(m.real(LFO1_TO_PITCH) == 0.0, "a wandering pitch is not a vibrato");
    let c = m.real(CHAOS_AMT);
    assert!((0.4..=1.0).contains(&c), "chaos amount {c}");

    // A steady note has none of it.
    let m = measure(&[(SRC_NOISE_MIX, 0.0)], C4, 2.0, 2.2);
    assert!(m.real(LFO1_TO_PITCH) == 0.0 && m.real(LFO1_TO_LEVEL) == 0.0 && m.real(CHAOS_AMT) == 0.0);
}

#[test]
fn short_sounds_flag_motion_as_provisional() {
    use P::*;
    // Half a second of 5 Hz vibrato is 2.5 cycles but under 2 s of steady sound.
    let m = measure(&[(LFO1_RATE, 5.0), (LFO1_TO_PITCH, 40.0), (SRC_NOISE_MIX, 0.0)], C4, 0.6, 0.7);
    assert!(m.provisional);
}

#[test]
fn click_comes_back() {
    use P::*;
    let base = |c: f32| measure(&[(STRIKE_CLICK, c), (ENV_DECAY, 0.3), (ENV_SUSTAIN, 0.0), (SRC_NOISE_MIX, 0.0), (SRC_TILT, -12.0)], C4, 0.1, 0.8);
    assert_eq!(base(0.0).real(STRIKE_CLICK), 0.0);
    near("click 0.3", base(0.3).real(STRIKE_CLICK), 0.3, 0.1);
    near("click 0.6", base(0.6).real(STRIKE_CLICK), 0.6, 0.15);
    assert!(base(1.0).real(STRIKE_CLICK) >= 0.85);
}

#[test]
fn fold_reads_weakly_and_never_from_a_clean_tone() {
    use P::*;
    let f = |a: f32| measure(&[(FOLD_AMT, a), (SRC_NOISE_MIX, 0.0), (SRC_TILT, -5.0)], C4, 1.2, 1.5).real(FOLD_AMT);
    assert_eq!(f(0.0), 0.0);
    assert!(f(1.0) > 0.15, "fold 1 reads {}", f(1.0));
}

#[test]
fn reverb_tail_comes_back() {
    use P::*;
    let v = |size: f32, mix: f32| {
        let m = measure(&[(VERB_SIZE, size), (VERB_MIX, mix), (ENV_ATTACK, 0.005), (ENV_RELEASE, 0.01), (SRC_NOISE_MIX, 0.0)], C4, 0.5, 4.0);
        (m.real(VERB_SIZE), m.real(VERB_MIX))
    };
    for size in [0.4f32, 0.6, 0.8] {
        let (s, mix) = v(size, 0.3);
        near(&format!("verb.size at {size}"), s, size, 0.07);
        near(&format!("verb.mix at size {size}"), mix, 0.3, 0.2);
    }
    let (_, mix) = v(0.4, 0.5);
    near("verb.mix at 0.5", mix, 0.5, 0.2);
    // No tail, no reverb: a note with a plain release.
    let m = measure(&[(ENV_ATTACK, 0.005), (ENV_RELEASE, 0.2), (SRC_NOISE_MIX, 0.0)], C4, 0.5, 2.0);
    assert_eq!(m.real(VERB_MIX), 0.0);
}

#[test]
fn unpitched_sources_skip_the_harmonic_rows_and_use_the_sound() {
    use P::*;
    let noise = measure_opts(&[(SRC_HARM_MIX, 0.0), (SRC_NOISE_MIX, 1.0), (ENV_NOISE_DECAY, 4.0)], &[], C4, 1.2, 1.5, FullOpts::default());
    assert!(!noise.pitched && noise.voiced_fraction < 0.3);
    assert!(noise.use_sound, "use_sound is on by default for unpitched sources");
    near("grain_mix", noise.real(SRC_GRAIN_MIX), analysis::full::GRAIN_MIX_ON, 0.01);
    assert!(noise.real(SRC_HARM_MIX) == 0.0);
    assert!(noise.grain.unwrap().ref_cents.is_none(), "no reference pitch for noise");

    let tone = measure(&[(SRC_NOISE_MIX, 0.0)], C4, 1.2, 1.5);
    assert!(tone.pitched && !tone.use_sound);
    assert_eq!(tone.real(SRC_GRAIN_MIX), 0.0);
    let g = tone.grain.unwrap();
    near("grain reference", g.ref_cents.unwrap(), C4, 5.0);
    assert!(g.end > g.start && (g.end - g.start) as f32 >= 0.1 * SR && (g.end - g.start) as f32 <= 10.0 * SR);

    // use_sound forced on for a pitched sound: grain_mix up, partials down in proportion.
    let on = measure_opts(&[(SRC_NOISE_MIX, 0.0)], &[], C4, 1.2, 1.5, FullOpts { decaying: false, use_sound: Some(true) });
    assert!(on.use_sound);
    near("harm_mix", on.real(SRC_HARM_MIX), 1.0 - analysis::full::GRAIN_MIX_ON, 0.01);
    let off = measure_opts(&[(SRC_NOISE_MIX, 0.0)], &[], C4, 1.2, 1.5, FullOpts { decaying: false, use_sound: Some(false) });
    assert_eq!(off.real(SRC_GRAIN_MIX), 0.0);
}

#[test]
fn steady_region_is_the_longest_stretch_near_the_median() {
    // 0.3 s ramp up, 1.0 s steady, 0.7 s ramp down.
    let n = (2.0 * SR) as usize;
    let x: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / SR;
            let env = if t < 0.3 { t / 0.3 } else if t < 1.3 { 1.0 } else { ((2.0 - t) / 0.7).max(0.0) };
            0.5 * env * (std::f32::consts::TAU * 220.0 * t).sin()
        })
        .collect();
    let m = match_full(&x, SR, FullOpts::default());
    let g = m.grain.unwrap();
    let (a, b) = (g.start as f32 / SR, g.end as f32 / SR);
    // Within 6 dB of the median: the ramps count from -6 dB up and down.
    assert!(a >= 0.1 && a <= 0.2 && b >= 1.6 && b <= 1.7, "steady region {a}..{b} s");
}

#[test]
fn degenerate_input_gives_a_neutral_finite_target() {
    for x in [vec![], vec![0.0f32; 48_000], vec![f32::NAN; 48_000], vec![0.3f32; 100]] {
        let m = match_full(&x, SR, FullOpts::default());
        assert!(m.target.iter().all(|v| (0.0..=1.0).contains(v)));
        assert!(m.pitch_cents.is_finite() && m.duration_s.is_finite());
    }
    let m = match_full(&[0.1f32; 48_000], f32::NAN, FullOpts::default());
    assert_eq!(m.target, P::neutral_set());
}

#[test]
fn flat_layout_matches_the_struct() {
    let x = render_note(&LayerSpec::base(real_set(&[(P::SRC_NOISE_MIX, 0.0)])), None, C4, 1.0, 1.2);
    let m = match_full(&x, SR, FullOpts::default());
    let mut out = vec![0.0f32; analysis::FULL_MATCH_LEN];
    m.to_floats(&mut out);
    assert_eq!(&out[..P::COUNT], &m.target[..]);
    assert_eq!(analysis::FULL_MATCH_LEN, P::COUNT + 10);
    assert_eq!(out[P::COUNT], 1.0);
    assert_eq!(out[P::COUNT + 1], m.pitch_cents);
    assert_eq!(out[P::COUNT + 9], 1.0);
    assert!(out[P::COUNT + 8].is_finite());
}

#[test]
fn ten_seconds_matches_quickly() {
    let mut x = render_note(&LayerSpec::base(real_set(&[(P::LFO1_TO_PITCH, 30.0)])), None, C4, 4.0, 4.5);
    let again = x.clone();
    for _ in 0..2 {
        x.extend_from_slice(&again);
    }
    assert!(x.len() as f32 / SR >= 10.0);
    let t = Instant::now();
    let m = match_full(&x, SR, FullOpts::default());
    let ms = t.elapsed().as_secs_f32() * 1000.0;
    println!("match_full over {:.1} s: {ms:.0} ms native", x.len() as f32 / SR);
    assert!(m.pitched);
    assert!(ms < 1000.0, "{ms} ms");
}
