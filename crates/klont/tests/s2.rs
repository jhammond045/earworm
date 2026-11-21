//! Stage S2 checks that need the live engine (docs/synth.md "S2: the modulation
//! matrix"): a listening layer's routes follow the mic. A synthetic voice goes
//! in through `process` in 32-sample calls; a layer route moves the voice
//! filter's cutoff; we time the cutoff the filter reads (the meter block shows
//! it after every call) and the output level it lets through.

use klont::classic::{self, Extra, ExtraTarget};
use klont::patch::*;
use klont::protocol::*;
use klont::Engine;
use params as P;

const SR: f32 = 48_000.0;
const BLOCK: usize = 32;

/// Harmonic "ah" at `hz` (as in live_follow.rs).
fn hum(hz: f32, n: usize, amp: f32, phase0: usize) -> Vec<f32> {
    let mut out = vec![0.0; n];
    let kmax = ((SR * 0.45) / hz) as usize;
    for k in 1..=kmax {
        let f = hz * k as f32;
        let formant = |fc: f32, bw: f32| (-(f - fc).powi(2) / (2.0 * bw * bw)).exp();
        let a = (1.0 / k as f32) * (0.2 + formant(700.0, 150.0) + 0.6 * formant(1100.0, 200.0));
        let w = std::f32::consts::TAU * f / SR;
        for (i, x) in out.iter_mut().enumerate() {
            *x += a * (w * (i + phase0) as f32).sin();
        }
    }
    let peak = out.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
    out.iter_mut().for_each(|x| *x *= amp / peak);
    out
}

struct Trace {
    /// Cutoff (normalized) after each call, the output rms of each call, and the live
    /// tier's raw pitch after each call (NaN until it knows one).
    cutoff: Vec<f32>,
    rms: Vec<f32>,
    pitch: Vec<f32>,
}

/// A melody layer that listens, holds a note, and has one layer route into the
/// filter cutoff from `route`; the cutoff starts at `base` (normalized).
fn run(route: Extra, base: f32, input: &[f32]) -> Trace {
    let mut e = Engine::new(SR, BLOCK);
    let blob = classic::patch_blob_ext(KIND_MELODY, 0, &[], &[route], 0, true);
    assert_eq!(e.load_bytes(0, LOAD_PATCH, &blob), LOAD_OK);
    let cut = classic::index(KIND_MELODY, P::FILT_CUTOFF)[0];
    e.push(MSG_SET_PARAM, 0, SET_BASE, cut as u16, base, 0);
    e.push(MSG_LISTEN, 0, 0, 0, 1.0, 0);
    e.push(MSG_NOTE_ON, 0, 0, 0, 5700.0, 0);
    let mut t = Trace { cutoff: Vec::new(), rms: Vec::new(), pitch: Vec::new() };
    for chunk in input.chunks(BLOCK) {
        e.input_mut()[..chunk.len()].copy_from_slice(chunk);
        e.process(chunk.len());
        t.cutoff.push(e.meter()[M_STORED + cut]);
        t.pitch.push(e.meter()[M_PITCH]);
        let o = &e.output().0[..chunk.len()];
        t.rms.push((o.iter().map(|x| x * x).sum::<f32>() / o.len() as f32).sqrt());
    }
    t
}

/// Milliseconds from sample `at` until `xs` (one value per call) has covered
/// `frac` of its move from the value before `at` to its value `settle` calls later.
fn reach(xs: &[f32], at: usize, settle: usize, frac: f32) -> f32 {
    let i0 = at / BLOCK;
    let (a, b) = (xs[i0 - 1], xs[i0 + settle]);
    assert!((b - a).abs() > 1e-3, "nothing moved: {a} -> {b}");
    let k = (i0..).find(|&i| (xs[i] - a) / (b - a) >= frac).unwrap();
    // The value after call k is there at the end of the call.
    ((k + 1) * BLOCK - at) as f32 / SR * 1000.0
}

#[test]
fn a_step_in_input_level_moves_the_cutoff_within_6_ms() {
    let quiet = (1.0 * SR) as usize;
    let mut input = vec![0.0; quiet];
    input.extend(hum(220.0, (0.5 * SR) as usize, 0.3, 0));
    let route = Extra::new(SRC_LAYER_VOLUME, 0, ExtraTarget::V("filter1", P::filter::CUTOFF), 0.6);
    let t = run(route, 0.2, &input);
    let settle = (0.2 * SR) as usize / BLOCK;
    let first = reach(&t.cutoff, quiet, settle, 0.05);
    let half = reach(&t.cutoff, quiet, settle, 0.5);
    let out_first = reach(&t.rms, quiet, settle, 0.05);
    let out_half = reach(&t.rms, quiet, settle, 0.5);
    println!("level step -> cutoff: moves after {first:.2} ms, half way {half:.2} ms; output level: {out_first:.2} ms, half way {out_half:.2} ms");
    assert!(first <= 6.0, "the cutoff moved after {first} ms");
    assert!(out_first <= 6.0, "the output moved after {out_first} ms");
}

/// Pitch to the cutoff through a layer route, C3 to C5 across `amount` of its range.
fn pitch_route() -> Extra {
    Extra { in_lo: 3600.0, in_hi: 6000.0, ..Extra::new(SRC_LAYER_PITCH, 0, ExtraTarget::V("filter1", P::filter::CUTOFF), 0.6) }
}

/// Milliseconds from sample `at` until the live tier first reports a pitch.
fn pitch_known(t: &Trace, at: usize) -> f32 {
    let k = (at / BLOCK..).find(|&i| t.pitch[i].is_finite()).unwrap();
    ((k + 1) * BLOCK - at) as f32 / SR * 1000.0
}

#[test]
fn a_pitch_route_follows_the_sung_pitch_one_tick_after_the_analysis() {
    let settle = (0.2 * SR) as usize / BLOCK;
    // From silence, as M2 measured Live follow's pitch: the route moves nothing until the pitch is known.
    let quiet = (1.0 * SR) as usize;
    let mut input = vec![0.0; quiet];
    input.extend(hum(330.0, (0.5 * SR) as usize, 0.3, 0));
    let t = run(pitch_route(), 0.2, &input);
    let (known, onset) = (pitch_known(&t, quiet), reach(&t.cutoff, quiet, settle, 0.05));
    // A step a minor third up in a held note, and a fifth (702 cents), which the capture tier's
    // continuity guard lets through only after it has lasted 30 ms.
    let a = (1.0 * SR) as usize;
    let step = |hz: f32| {
        let mut input = hum(220.0, a, 0.3, 0);
        input.extend(hum(hz, (0.5 * SR) as usize, 0.3, a));
        let t = run(pitch_route(), 0.2, &input);
        (reach(&t.cutoff, a, settle, 0.05), reach(&t.cutoff, a, settle, 0.5))
    };
    let (third, fifth) = (step(261.6), step(330.0));
    println!(
        "pitch -> cutoff: from silence {onset:.2} ms (the live tier knows the pitch after {known:.2} ms); a minor third up {:.2} ms (half way {:.2}); a fifth up {:.2} ms (half way {:.2})",
        third.0, third.1, fifth.0, fifth.1
    );
    // The route adds at most one control tick (0.67 ms) to the analysis.
    assert!(onset - known <= 32.0 / SR * 1000.0 + 1e-3, "the route took {} ms after the pitch was known", onset - known);
}

#[test]
fn the_benchmark_routes_all_run_every_tick() {
    let mut e = Engine::new(SR, 128);
    let blob = classic::patch_blob_ext(KIND_MELODY, 0, &classic::bench_routes(false), &[], 0, false);
    assert_eq!(e.load_bytes(0, LOAD_PATCH, &blob), LOAD_OK);
    e.push(MSG_NOTE_ON, 0, 0, 0, 4800.0, 0);
    let mut seen = [false; 48];
    for _ in 0..200 {
        e.render(128);
        for (i, s) in seen.iter_mut().enumerate() {
            *s |= e.meter()[M_ROUTES + i].is_finite();
        }
    }
    assert!(seen.iter().all(|&s| s), "{seen:?}");
    assert!(e.meter()[M_PEAK_L] > 0.01);
}

#[test]
fn a_shaper_edit_keeps_the_notes_sounding() {
    let mut e = Engine::new(SR, 128);
    let a = Extra { pol: POL_BOTH, ..Extra::new(SRC_MOD, 3, ExtraTarget::V("filter1", P::filter::RES), 0.3) };
    let bent = Extra { bend: 0.5, rise_ms: 20.0, ..a };
    let load = |e: &mut Engine, x: &[Extra]| assert_eq!(e.load_bytes(0, LOAD_PATCH, &classic::patch_blob_ext(KIND_MELODY, 0, x, &[], 0, true)), LOAD_OK);
    load(&mut e, &[a]);
    e.push(MSG_NOTE_ON, 0, 0, 0, 5700.0, 0);
    for _ in 0..40 {
        e.render(128);
    }
    assert_eq!(e.meter()[M_LAYER_VOICES], 1.0);
    // Only the shaper changed: the note keeps sounding.
    load(&mut e, &[bent]);
    e.render(128);
    assert_eq!(e.meter()[M_LAYER_VOICES], 1.0, "a shaper edit silenced the layer");
    assert!(e.meter()[M_PEAK_L] > 1e-3);
    // Another route is a structure change: the layer fades out over 5 ms (S3), then starts over.
    load(&mut e, &[bent, a]);
    e.render(128);
    assert_eq!(e.meter()[M_LAYER_VOICES], 1.0, "the old patch plays through the fade");
    for _ in 0..3 {
        e.render(128);
    }
    assert_eq!(e.meter()[M_LAYER_VOICES], 0.0);
}
