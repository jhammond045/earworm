//! M4 engine tests: the grain cloud (LOAD_GRAIN) and states with the morph
//! lane (LOAD_STATES, track blob v2).

use klont::Engine;
use klont::protocol::*;
use klont::seq::{FEEL_GRID, MORPH_LINE, MORPH_STEP, PPQ, TRACK_MAGIC, TRACK_MAGIC2};
use params as P;

const SR: f32 = 48_000.0;
/// C0 + 5700 cents is A4 = 440 Hz.
const A4: f32 = 5700.0;

fn bytes(w: &[u32]) -> Vec<u8> {
    w.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// One grid section of `beats` at 120 bpm (one beat = 0.5 s) with one note
/// clip on layer 0: (t beats, dur beats, cents). With `morph` markers
/// (t ticks, state, len ms, shape) the blob is v2, else v1.
fn track(beats: f32, notes: &[(f32, f32, f32)], morph: &[(f32, u32, f32, u32)]) -> Vec<u8> {
    let v2 = !morph.is_empty();
    let mut w: Vec<u32> =
        vec![if v2 { TRACK_MAGIC2 } else { TRACK_MAGIC }, 1, 1, notes.len() as u32, 0, 1, morph.len() as u32, 0];
    w.extend([FEEL_GRID, (beats * PPQ as f32).to_bits(), 120f32.to_bits(), 1, 0, 0, 0, 0]);
    w.extend([0, 0, (beats * PPQ as f32).to_bits(), 0, notes.len() as u32, (-1i32) as u32, 0, 0]);
    if v2 {
        w.extend([0, morph.len() as u32, 0, 0]);
    }
    for &(t, d, c) in notes {
        let mut r = [0u32; 16];
        r[0] = (t * PPQ as f32).to_bits();
        r[1] = (d * PPQ as f32).to_bits();
        r[2] = c.to_bits();
        r[5] = 1f32.to_bits();
        for x in r.iter_mut().skip(6).take(8) {
            *x = f32::NAN.to_bits();
        }
        w.extend(r);
    }
    for &(t, st, ms, sh) in morph {
        w.extend([t.to_bits(), st, ms.to_bits(), sh]);
    }
    bytes(&w)
}

fn grain_blob(rate: f32, ref_cents: f32, data: &[f32]) -> Vec<u8> {
    let mut w = vec![GRAIN_MAGIC, rate.to_bits(), ref_cents.to_bits(), data.len() as u32];
    w.extend(data.iter().map(|x| x.to_bits()));
    bytes(&w)
}

fn sine(freq: f32, secs: f32, amp: f32) -> Vec<f32> {
    (0..(secs * SR) as usize).map(|i| amp * (std::f32::consts::TAU * freq * i as f32 / SR).sin()).collect()
}

fn set(e: &mut Engine, p: usize, real: f32) {
    e.push(MSG_SET_PARAM, 0, SET_BASE, p as u16, P::to_norm(p, real), 0);
}

/// A layer that makes sound only through its grain cloud.
fn grain_engine(max_block: usize) -> Box<Engine> {
    grain_engine_mix(max_block, 1.0)
}

fn grain_engine_mix(max_block: usize, mix: f32) -> Box<Engine> {
    let mut e = Engine::new(SR, max_block);
    set(&mut e, P::SRC_HARM_MIX, 0.0);
    set(&mut e, P::SRC_NOISE_MIX, 0.0);
    set(&mut e, P::SRC_GRAIN_MIX, mix);
    set(&mut e, P::GRAIN_POS, 0.5);
    // Parameters glide (15 ms each): let the partial bank fade out before any note.
    render(&mut e, 48_000, 512);
    e
}

fn play_note(e: &mut Engine, cents: f32, beats: f32) {
    assert_eq!(e.load_bytes(0, LOAD_TRACK, &track(beats + 1.0, &[(0.0, beats, cents)], &[])), LOAD_OK);
    e.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
}

fn render(e: &mut Engine, n: usize, chunk: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let k = e.render(chunk.min(n - out.len()));
        out.extend_from_slice(&e.output().0[..k]);
    }
    out
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

/// Frequency of the strongest component between 100 and 2000 Hz (Goertzel scan, 2 Hz steps).
fn peak_hz(x: &[f32]) -> f32 {
    let mut best = (0.0f64, 0.0f32);
    let mut f = 100.0f32;
    while f <= 2000.0 {
        let w = std::f64::consts::TAU * f as f64 / SR as f64;
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, &v) in x.iter().enumerate() {
            re += v as f64 * (w * i as f64).cos();
            im += v as f64 * (w * i as f64).sin();
        }
        let p = re * re + im * im;
        if p > best.0 {
            best = (p, f);
        }
        f += 2.0;
    }
    best.1
}

// ---------------------------------------------------------------- grain cloud

#[test]
fn no_source_or_no_mix_is_silent_and_a_source_with_mix_is_not() {
    // No source loaded.
    let mut e = grain_engine(512);
    play_note(&mut e, A4, 2.0);
    assert!(render(&mut e, 24_000, 512).iter().all(|&v| v == 0.0));

    // A source but a mix of 0.
    let mut e = grain_engine_mix(512, 0.0);
    assert_eq!(e.load_bytes(0, LOAD_GRAIN, &grain_blob(SR, A4, &sine(440.0, 1.0, 0.5))), LOAD_OK);
    play_note(&mut e, A4, 2.0);
    assert!(render(&mut e, 24_000, 512).iter().all(|&v| v == 0.0));

    // Source and mix.
    let mut e = grain_engine(512);
    assert_eq!(e.load_bytes(0, LOAD_GRAIN, &grain_blob(SR, A4, &sine(440.0, 1.0, 0.5))), LOAD_OK);
    play_note(&mut e, A4, 2.0);
    let out = render(&mut e, 24_000, 512);
    assert!(rms(&out[4800..]) > 0.05, "rms {}", rms(&out[4800..]));
    assert!(out.iter().all(|v| v.is_finite() && v.abs() <= 1.0));

    // Clearing the source (n = 0) fades the cloud out.
    assert_eq!(e.load_bytes(0, LOAD_GRAIN, &grain_blob(SR, A4, &[])), LOAD_OK);
    let tail = render(&mut e, 24_000, 512);
    assert!(rms(&tail[12_000..]) == 0.0, "still sounding after a clear");
}

#[test]
fn grain_pitch_follows_the_note() {
    let src = grain_blob(SR, A4, &sine(440.0, 1.0, 0.5));
    for (cents, want) in [(A4, 440.0), (A4 + 1200.0, 880.0), (A4 - 1200.0, 220.0)] {
        let mut e = grain_engine(512);
        assert_eq!(e.load_bytes(0, LOAD_GRAIN, &src), LOAD_OK);
        play_note(&mut e, cents, 3.0);
        let out = render(&mut e, 48_000, 512);
        let hz = peak_hz(&out[9600..9600 + 24_000]);
        assert!((hz - want).abs() < want * 0.02, "note {cents}: peak {hz} Hz, wanted about {want}");
    }
}

#[test]
fn an_unpitched_source_plays_at_its_own_speed_around_c4() {
    let mut e = grain_engine(512);
    assert_eq!(e.load_bytes(0, LOAD_GRAIN, &grain_blob(SR, f32::NAN, &sine(300.0, 1.0, 0.5))), LOAD_OK);
    play_note(&mut e, 4800.0, 3.0);
    let out = render(&mut e, 48_000, 512);
    let hz = peak_hz(&out[9600..9600 + 24_000]);
    assert!((hz - 300.0).abs() < 6.0, "{hz}");
}

#[test]
fn a_source_at_another_rate_keeps_its_pitch() {
    let mut e = grain_engine(512);
    let src: Vec<f32> = (0..24_000).map(|i| 0.5 * (std::f32::consts::TAU * 440.0 * i as f32 / 24_000.0).sin()).collect();
    assert_eq!(e.load_bytes(0, LOAD_GRAIN, &grain_blob(24_000.0, A4, &src)), LOAD_OK);
    play_note(&mut e, A4, 3.0);
    let out = render(&mut e, 48_000, 512);
    let hz = peak_hz(&out[9600..9600 + 24_000]);
    assert!((hz - 440.0).abs() < 9.0, "{hz}");
}

fn max_step(x: &[f32]) -> f32 {
    x.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max)
}

#[test]
fn changing_the_source_makes_no_click() {
    let mut e = grain_engine(128);
    let a = grain_blob(SR, A4, &sine(440.0, 1.0, 0.5));
    let b = grain_blob(SR, A4, &sine(660.0, 1.0, 0.5));
    assert_eq!(e.load_bytes(0, LOAD_GRAIN, &a), LOAD_OK);
    play_note(&mut e, A4, 4.0);
    let before = render(&mut e, 48_000, 128);
    let steady = max_step(&before[12_000..]);
    // Load mid-block (not on a tick boundary) and keep rendering.
    let _ = render(&mut e, 77, 77);
    assert_eq!(e.load_bytes(0, LOAD_GRAIN, &b), LOAD_OK);
    let after = render(&mut e, 24_000, 128);
    let worst = max_step(&after);
    assert!(steady > 0.01, "a 440 Hz sine should step by about 0.02: {steady}");
    // A click would step by the source amplitude (about 0.5), 20x the steady step.
    assert!(worst < steady * 1.6, "step {worst} after the change, {steady} before");
    // And the new source is what plays afterwards.
    let hz = peak_hz(&after[12_000..]);
    assert!((hz - 660.0).abs() < 14.0, "{hz}");
    // A second change while one is fading is accepted.
    assert_eq!(e.load_bytes(0, LOAD_GRAIN, &a), LOAD_OK);
    render(&mut e, 100, 100);
    assert_eq!(e.load_bytes(0, LOAD_GRAIN, &b), LOAD_OK);
    assert!(render(&mut e, 4800, 128).iter().all(|v| v.is_finite()));
}

fn busy_cloud(max_block: usize) -> Box<Engine> {
    let mut e = grain_engine(max_block);
    set(&mut e, P::GRAIN_SPRAY, 0.6);
    set(&mut e, P::GRAIN_SCATTER, 0.7);
    set(&mut e, P::GRAIN_DENSITY, 60.0);
    set(&mut e, P::LFO2_TO_GRAIN, 0.5);
    set(&mut e, P::LFO2_RATE, 3.0);
    let noise: Vec<f32> = (0..48_000).map(|i| ((i * 7919) % 2000) as f32 / 1000.0 - 1.0).map(|x| x * 0.3).collect();
    assert_eq!(e.load_bytes(0, LOAD_GRAIN, &grain_blob(SR, f32::NAN, &noise)), LOAD_OK);
    play_note(&mut e, 5400.0, 3.0);
    e
}

#[test]
fn offline_renders_are_bit_identical() {
    let a = render(&mut busy_cloud(512), 60_000, 512);
    let b = render(&mut busy_cloud(512), 60_000, 512);
    assert!(rms(&a) > 0.02);
    assert!(a == b);
    // Not even the call length matters.
    let c = render(&mut busy_cloud(512), 60_000, 33);
    assert!(a == c);
}

#[test]
fn grain_loads_are_checked() {
    let mut e = Engine::new(SR, 128);
    let ok = grain_blob(SR, A4, &[0.0; 16]);
    assert_eq!(e.load_bytes(8, LOAD_GRAIN, &ok), LOAD_BAD_SLOT);
    let mut bad = ok.clone();
    bad[0] ^= 1;
    assert_eq!(e.load_bytes(0, LOAD_GRAIN, &bad), LOAD_BAD_MAGIC);
    assert_eq!(e.load_bytes(0, LOAD_GRAIN, &ok[..ok.len() - 4]), LOAD_MALFORMED);
    assert_eq!(e.load_bytes(0, LOAD_GRAIN, &ok[..8]), LOAD_MALFORMED);
    assert_eq!(e.load_bytes(0, LOAD_GRAIN, &grain_blob(0.0, A4, &[0.0; 16])), LOAD_MALFORMED);
    let mut big = ok.clone();
    big[12..16].copy_from_slice(&(GRAIN_MAX_SAMPLES as u32 + 1).to_le_bytes());
    assert_eq!(e.load_bytes(0, LOAD_GRAIN, &big), LOAD_TOO_BIG);
    // The maximum fits.
    assert_eq!(e.load_bytes(0, LOAD_GRAIN, &grain_blob(SR, A4, &vec![0.1; GRAIN_MAX_SAMPLES])), LOAD_OK);
    // A mono source of any other length too, down to one sample.
    assert_eq!(e.load_bytes(0, LOAD_GRAIN, &grain_blob(SR, A4, &[0.5])), LOAD_OK);
}

// ---------------------------------------------------------------- states and morph

fn states_blob(states: &[(f32, f32)]) -> Vec<u8> {
    // Each state: a filter cutoff (normalized) and an Influence amount; the rest neutral.
    let neutral = P::neutral_set();
    let mut w = vec![STATES_MAGIC, states.len() as u32];
    for &(cut, amount) in states {
        let mut s = [0f32; STATE_WORDS];
        s[..P::COUNT].copy_from_slice(&neutral);
        s[P::FILT_CUTOFF] = cut;
        for m in 0..N_MACROS {
            s[P::COUNT + m] = 0.5;
        }
        s[P::COUNT + N_MACROS] = amount;
        w.extend(s.iter().map(|x| x.to_bits()));
    }
    bytes(&w)
}

/// Meter value of the selected layer's stored cutoff after each 480-sample
/// call (10 ms), for `secs` seconds.
fn cutoff_trace(e: &mut Engine, secs: f32) -> Vec<f32> {
    (0..(secs * 100.0) as usize)
        .map(|_| {
            e.render(480);
            e.meter()[M_STORED + P::FILT_CUTOFF]
        })
        .collect()
}

/// A track of 4 beats (2 s) at 120 bpm: marker 0 at t=0 on state 0, marker 1
/// at the half (1 s) on state 1.
fn morph_engine(shape: u32, len_ms: f32) -> Box<Engine> {
    let mut e = Engine::new(SR, 480);
    assert_eq!(e.load_bytes(0, LOAD_STATES, &states_blob(&[(0.2, 0.0), (0.8, 0.0)])), LOAD_OK);
    let markers = [(0.0, 0, 0.0, MORPH_LINE), (2.0 * PPQ as f32, 1, len_ms, shape)];
    assert_eq!(e.load_bytes(0, LOAD_TRACK, &track(4.0, &[(0.0, 0.25, 4800.0)], &markers)), LOAD_OK);
    e.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
    e
}

#[test]
fn a_line_morph_moves_between_states_over_its_length() {
    let mut e = morph_engine(MORPH_LINE, 500.0);
    let tr = cutoff_trace(&mut e, 1.9);
    // The track is 2 s at 120 bpm: the marker sits at 1 s (index 100).
    // (The first 300 ms glide from the engine's neutral cutoff to state 0.)
    for (i, &v) in tr.iter().enumerate().take(99).skip(30) {
        assert!((v - 0.2).abs() < 1e-5, "before the marker, at {} ms: {v}", i * 10);
    }
    // Moving over about 500 ms (the 15 ms smoothing adds a short lag).
    let mid = tr[125];
    assert!(mid > 0.4 && mid < 0.6, "mid-morph {mid}");
    assert!(tr[105] > 0.2 && tr[105] < tr[125] && tr[125] < tr[145], "{} {} {}", tr[105], tr[125], tr[145]);
    for w in tr[99..150].windows(2) {
        assert!(w[1] >= w[0] - 1e-6, "monotonic");
    }
    // Sits at the endpoint exactly.
    for &v in &tr[185..] {
        assert!((v - 0.8).abs() < 1e-5, "{v}");
    }
}

#[test]
fn a_step_morph_switches_at_once() {
    let mut e = morph_engine(MORPH_STEP, 500.0);
    let tr = cutoff_trace(&mut e, 1.5);
    assert!((tr[98] - 0.2).abs() < 1e-5);
    // Only the 15 ms parameter smoothing separates it from the new state.
    assert!(tr[103] > 0.7, "{}", tr[103]);
    assert!((tr[120] - 0.8).abs() < 1e-5);
}

#[test]
fn the_lane_loops_with_the_clip() {
    let mut e = Engine::new(SR, 480);
    assert_eq!(e.load_bytes(0, LOAD_STATES, &states_blob(&[(0.2, 0.0), (0.8, 0.0)])), LOAD_OK);
    let markers = [(0.0, 0, 0.0, MORPH_LINE), (1.0 * PPQ as f32, 1, 0.0, MORPH_LINE)];
    // Loop of 2 beats inside a 8-beat section: the lane repeats every second.
    let mut t = track(8.0, &[(0.0, 0.25, 4800.0)], &markers);
    t[(16 + 2) * 4..(16 + 3) * 4].copy_from_slice(&(2.0 * PPQ as f32).to_bits().to_le_bytes());
    assert_eq!(e.load_bytes(0, LOAD_TRACK, &t), LOAD_OK);
    e.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
    let tr = cutoff_trace(&mut e, 3.9);
    for (sec, want) in [(0.25, 0.2), (0.75, 0.8), (1.25, 0.2), (1.75, 0.8), (3.75, 0.8)] {
        let v = tr[(sec * 100.0) as usize];
        assert!((v - want).abs() < 1e-4, "at {sec} s: {v}, wanted {want}");
    }
}

#[test]
fn states_without_markers_or_a_clip_use_state_zero() {
    // No track at all: state 0 replaces what LOAD_LAYER-style messages set.
    let mut e = Engine::new(SR, 480);
    set(&mut e, P::FILT_CUTOFF, 5000.0);
    cutoff_trace(&mut e, 0.5);
    let before = e.meter()[M_STORED + P::FILT_CUTOFF];
    assert!((before - P::to_norm(P::FILT_CUTOFF, 5000.0)).abs() < 1e-3);
    assert_eq!(e.load_bytes(0, LOAD_STATES, &states_blob(&[(0.3, 0.0), (0.9, 0.0)])), LOAD_OK);
    let tr = cutoff_trace(&mut e, 0.5);
    assert!((tr[49] - 0.3).abs() < 1e-5, "{}", tr[49]);

    // A v1 track (no markers) on a layer with states: still state 0.
    assert_eq!(e.load_bytes(0, LOAD_TRACK, &track(4.0, &[(0.0, 0.25, 4800.0)], &[])), LOAD_OK);
    e.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
    let tr = cutoff_trace(&mut e, 0.5);
    assert!((tr[49] - 0.3).abs() < 1e-5);

    // A layer without a clip in the section (select layer 1, which has states but no clip).
    assert_eq!(e.load_bytes(1, LOAD_STATES, &states_blob(&[(0.6, 0.0)])), LOAD_OK);
    e.push(MSG_SELECT_LAYER, 1, 0, 0, 0.0, 0);
    let tr = cutoff_trace(&mut e, 0.3);
    assert!((tr[29] - 0.6).abs() < 1e-5);
}

#[test]
fn clearing_states_leaves_the_last_blend_in_place() {
    let mut e = morph_engine(MORPH_STEP, 0.0);
    cutoff_trace(&mut e, 1.5);
    assert_eq!(e.load_bytes(0, LOAD_STATES, &bytes(&[STATES_MAGIC, 0])), LOAD_OK);
    let tr = cutoff_trace(&mut e, 0.2);
    assert!((tr[19] - 0.8).abs() < 1e-5);
}

#[test]
fn the_influence_amount_morphs_too() {
    // Influence target far from the base, amount 0 in state 0 and 1 in state 1.
    let mut e = Engine::new(SR, 480);
    let target = P::to_norm(P::FILT_CUTOFF, 200.0);
    e.push(MSG_SET_PARAM, 0, SET_INFLUENCE, P::FILT_CUTOFF as u16, target, 0);
    e.push(MSG_SET_LAYER, 0, 0, LAYER_INFLUENCE_MASK, 0x1f as f32, 0);
    assert_eq!(e.load_bytes(0, LOAD_STATES, &states_blob(&[(0.8, 0.0), (0.8, 1.0)])), LOAD_OK);
    let markers = [(0.0, 0, 0.0, MORPH_LINE), (2.0 * PPQ as f32, 1, 0.0, MORPH_STEP)];
    assert_eq!(e.load_bytes(0, LOAD_TRACK, &track(4.0, &[(0.0, 0.25, 4800.0)], &markers)), LOAD_OK);
    e.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
    let tr = cutoff_trace(&mut e, 1.5);
    assert!((tr[50] - 0.8).abs() < 1e-5, "{}", tr[50]);
    assert!((tr[140] - target).abs() < 1e-3, "{} vs {target}", tr[140]);
}

#[test]
fn states_loads_are_checked_and_malformed_ones_change_nothing() {
    let mut e = Engine::new(SR, 480);
    let good = states_blob(&[(0.3, 0.0)]);
    assert_eq!(e.load_bytes(8, LOAD_STATES, &good), LOAD_BAD_SLOT);
    let mut bad = good.clone();
    bad[0] ^= 1;
    assert_eq!(e.load_bytes(0, LOAD_STATES, &bad), LOAD_BAD_MAGIC);
    assert_eq!(e.load_bytes(0, LOAD_STATES, &good[..good.len() - 4]), LOAD_MALFORMED);
    assert_eq!(e.load_bytes(0, LOAD_STATES, &good[..4]), LOAD_MALFORMED);
    let mut many = good.clone();
    many[4..8].copy_from_slice(&(MAX_STATES as u32 + 1).to_le_bytes());
    assert_eq!(e.load_bytes(0, LOAD_STATES, &many), LOAD_TOO_BIG);
    // None of those loaded anything: the layer still has its own cutoff.
    let tr = cutoff_trace(&mut e, 0.2);
    assert!((tr[19] - P::to_norm(P::FILT_CUTOFF, 20_000.0)).abs() < 1e-5);
    // Eight states fit.
    assert_eq!(e.load_bytes(0, LOAD_STATES, &states_blob(&[(0.5, 0.0); MAX_STATES])), LOAD_OK);
}

#[test]
fn a_malformed_v2_track_keeps_the_previous_one() {
    let mut e = morph_engine(MORPH_LINE, 500.0);
    let mut bad = track(4.0, &[(0.0, 0.25, 4800.0)], &[(0.0, 9, 0.0, 0)]);
    assert_eq!(e.load_bytes(0, LOAD_TRACK, &bad), LOAD_MALFORMED);
    bad.truncate(bad.len() - 4);
    assert_eq!(e.load_bytes(0, LOAD_TRACK, &bad), LOAD_MALFORMED);
    // The old lane still runs.
    let tr = cutoff_trace(&mut e, 1.9);
    assert!((tr[50] - 0.2).abs() < 1e-5 && (tr[180] - 0.8).abs() < 1e-5);
}

#[test]
fn v1_tracks_still_load_and_play() {
    let mut e = Engine::new(SR, 512);
    assert_eq!(e.load_bytes(0, LOAD_TRACK, &track(2.0, &[(0.0, 1.0, 5400.0)], &[])), LOAD_OK);
    e.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
    let out = render(&mut e, 12_000, 512);
    assert!(rms(&out) > 0.01);
}
