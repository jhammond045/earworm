//! Whole-engine tests: determinism, block-size invariance, sample-accurate
//! events, the partial budget, loads, and no allocation while rendering.

use klont::Engine;
use klont::curve::{CURVE_MAGIC, CURVE_STRIDE};
use klont::protocol::*;
use klont::seq::{FEEL_GRID, PPQ, TRACK_MAGIC};
use params as P;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

// ---- allocation counter: counts allocations made by this thread while armed ----

struct Counting;
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
thread_local! {
    // Tests run in parallel threads; only the rendering thread is armed.
    static ARMED: Cell<bool> = const { Cell::new(false) };
}

fn armed() -> bool {
    ARMED.try_with(|a| a.get()).unwrap_or(false)
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if armed() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        if armed() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

const SR: f32 = 48_000.0;

// ---- tiny blob builders (the real encoder is web/src/m1/track.ts) ----

#[derive(Clone, Copy)]
struct Ev {
    t: f32,
    dur: f32,
    cents: f32,
    slot: u32,
}

struct ClipDef {
    layer: u32,
    loop_len: f32,
    events: Vec<Ev>,
    curve: i32,
    warp: Vec<(f32, f32)>,
}

/// One grid section of `beats` at `bpm`, looping if `looping`.
fn track(beats: f32, bpm: f32, looping: bool, clips: &[ClipDef]) -> Vec<u8> {
    let mut w: Vec<u32> = Vec::new();
    let n_events: usize = clips.iter().map(|c| c.events.len()).sum();
    let n_warp: usize = clips.iter().map(|c| c.warp.len()).sum();
    w.extend([TRACK_MAGIC, 1, clips.len() as u32, n_events as u32, n_warp as u32, looping as u32, 0, 0]);
    w.extend([FEEL_GRID, (beats * PPQ as f32).to_bits(), bpm.to_bits(), 1, 0f32.to_bits(), 0, 0, 0]);
    let (mut fe, mut fw) = (0u32, 0u32);
    for c in clips {
        w.extend([0, c.layer, c.loop_len.to_bits(), fe, c.events.len() as u32, c.curve as u32, fw, c.warp.len() as u32]);
        fe += c.events.len() as u32;
        fw += c.warp.len() as u32;
    }
    for c in clips {
        for e in &c.events {
            let mut r = [0u32; 16];
            r[0] = e.t.to_bits();
            r[1] = e.dur.to_bits();
            r[2] = e.cents.to_bits();
            r[4] = e.slot;
            r[5] = 1f32.to_bits();
            for x in r.iter_mut().skip(6).take(8) {
                *x = f32::NAN.to_bits();
            }
            w.extend(r);
        }
    }
    for c in clips {
        for &(s, t) in &c.warp {
            w.extend([s.to_bits(), t.to_bits()]);
        }
    }
    w.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn note(t_beats: f32, dur_beats: f32, cents: f32) -> Ev {
    Ev { t: t_beats * PPQ as f32, dur: dur_beats * PPQ as f32, cents, slot: 0 }
}

fn render_all(e: &mut Engine, n: usize, chunk: usize) -> (Vec<f32>, Vec<f32>) {
    let (mut l, mut r) = (Vec::with_capacity(n), Vec::with_capacity(n));
    while l.len() < n {
        let k = e.render(chunk.min(n - l.len()));
        let (a, b) = e.output();
        l.extend_from_slice(&a[..k]);
        r.extend_from_slice(&b[..k]);
    }
    (l, r)
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

fn set(e: &mut Engine, layer: u8, p: usize, real: f32) {
    klont::classic::set_param(e, layer, KIND_MELODY, p, P::to_norm(p, real));
}

/// A small song: melody on layer 0, a beat layer on 1, a pad on 2.
fn song(max_block: usize) -> Box<Engine> {
    let mut e = Engine::new(SR, max_block);
    e.push(MSG_SET_LAYER, 1, 0, LAYER_KIND, KIND_BEAT as f32, 0);
    set(&mut e, 0, P::LFO1_TO_PITCH, 15.0);
    set(&mut e, 0, P::DELAY_MIX, 0.3);
    set(&mut e, 2, P::VERB_MIX, 0.4);
    set(&mut e, 2, P::SRC_NOISE_MIX, 0.4);
    e.render(1);
    let melody = ClipDef {
        layer: 0,
        loop_len: 4.0 * PPQ as f32,
        events: vec![note(0.0, 0.5, 5700.0), note(1.0, 0.5, 5900.0), note(2.5, 1.0, 6100.0)],
        curve: -1,
        warp: vec![],
    };
    let beat = ClipDef {
        layer: 1,
        loop_len: 2.0 * PPQ as f32,
        events: vec![
            Ev { slot: 0, ..note(0.0, 0.25, f32::NAN) },
            Ev { slot: 1, ..note(1.0, 0.25, f32::NAN) },
        ],
        curve: -1,
        warp: vec![],
    };
    let pad = ClipDef { layer: 2, loop_len: 4.0 * PPQ as f32, events: vec![note(0.0, 3.5, 4800.0)], curve: -1, warp: vec![] };
    assert_eq!(e.load_bytes(0, LOAD_TRACK, &track(4.0, 120.0, true, &[melody, beat, pad])), LOAD_OK);
    e.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
    e
}

#[test]
fn silent_and_finite_at_init() {
    let mut e = Engine::new(SR, 128);
    let (l, r) = render_all(&mut e, 4800, 128);
    assert!(l.iter().chain(&r).all(|v| *v == 0.0));
}

#[test]
fn song_sounds_and_is_bounded() {
    let mut e = song(4096);
    let (l, r) = render_all(&mut e, 48_000 * 3, 4096);
    assert!(rms(&l) > 0.01 && rms(&r) > 0.01, "{} {}", rms(&l), rms(&r));
    assert!(l.iter().chain(&r).all(|v| v.is_finite() && v.abs() <= 1.0));
    assert!(e.meter()[M_PLAYING] == 1.0);
}

#[test]
fn output_does_not_depend_on_call_length() {
    let n = 48_000;
    let mut a = song(4096);
    let (la, ra) = render_all(&mut a, n, 4096);
    for chunk in [1usize, 7, 128, 1000] {
        let mut b = song(4096);
        let (lb, rb) = render_all(&mut b, n, chunk);
        let first = la.iter().zip(&lb).position(|(x, y)| x != y);
        assert!(la == lb && ra == rb, "chunk {chunk} differs, first at {first:?}");
    }
}

#[test]
fn two_engines_render_bit_identical() {
    let mut a = song(512);
    let mut b = song(512);
    assert_eq!(render_all(&mut a, 30_000, 512), render_all(&mut b, 30_000, 512));
}

#[test]
fn hit_lands_on_its_sample() {
    // A hit at beat 1 of 120 bpm starts at sample 24000.
    let mut e = Engine::new(SR, 4096);
    e.push(MSG_SET_LAYER, 0, 0, LAYER_KIND, KIND_BEAT as f32, 0);
    // The kit slot plays the layer's (melody) classic patch: its only voice patch.
    klont::classic::set_param(&mut e, 0, KIND_MELODY, P::ENV_ATTACK, 0.0);
    e.render(1);
    let c = ClipDef { layer: 0, loop_len: 4.0 * PPQ as f32, events: vec![note(1.0, 0.25, f32::NAN)], curve: -1, warp: vec![] };
    assert_eq!(e.load_bytes(0, LOAD_TRACK, &track(4.0, 120.0, false, &[c])), LOAD_OK);
    e.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
    let (l, _) = render_all(&mut e, 30_000, 333);
    let first = l.iter().position(|v| v.abs() > 1e-6).expect("hit sounds");
    // The engine started one sample before PLAY (render(1) above).
    assert!((24_000..24_000 + 3).contains(&first), "first sample {first}");
}

#[test]
fn live_note_on_and_off() {
    let mut e = Engine::new(SR, 128);
    set(&mut e, 0, P::SRC_NOISE_MIX, 0.0);
    e.push(MSG_NOTE_ON, 0, 0, 0, 5700.0, 0);
    let (l, _) = render_all(&mut e, 24_000, 128);
    assert!(rms(&l[12_000..]) > 0.01);
    e.push(MSG_NOTE_OFF, 0, 0, 0, 5700.0, 0);
    // env.release is an exponential time constant (0.2 s neutral), so -60 dB
    // takes about 1.4 s; the voice goes idle soon after.
    let (l, _) = render_all(&mut e, 2 * 48_000, 128);
    assert!(rms(&l[4800..9600]) > 0.01, "release should still sound at first");
    assert!(rms(&l[90_000..]) < 1e-5, "{}", rms(&l[90_000..]));
    assert_eq!(e.meter()[M_ACTIVE_VOICES], 0.0);
}

#[test]
fn budget_caps_partials_and_favours_loud_voices() {
    let mut e = Engine::new(SR, 128);
    for layer in 0..MAX_LAYERS as u8 {
        set(&mut e, layer, P::SRC_TILT, -1.0);
        set(&mut e, layer, P::ENV_SUSTAIN, 1.0);
        e.push(MSG_SET_LAYER, layer, 0, LAYER_LEVEL, if layer == 0 { 0.0 } else { -30.0 }, 0);
    }
    e.push(MSG_SET_BUDGET, 0, 0, 0, 512.0, 0);
    e.render(128);
    for layer in 0..MAX_LAYERS as u8 {
        for v in 0..VOICES_PER_LAYER {
            e.push(MSG_NOTE_ON, layer, 0, 0, 3300.0 + 100.0 * v as f32, 0);
        }
    }
    let (l, r) = render_all(&mut e, 24_000, 128);
    assert!(l.iter().chain(&r).all(|v| v.is_finite()));
    let parts = e.meter()[M_ACTIVE_PARTIALS];
    assert!(parts <= 512.0 + 64.0 * 2.0, "active partials {parts}");
    assert!(parts >= 256.0, "budget unused: {parts}");
    assert_eq!(e.meter()[M_ACTIVE_VOICES], 64.0);
}

#[test]
fn eight_layers_render_without_allocating() {
    let mut e = song(128);
    for layer in 3..MAX_LAYERS as u8 {
        set(&mut e, layer, P::VERB_MIX, 0.3);
        set(&mut e, layer, P::DELAY_MIX, 0.3);
        set(&mut e, layer, P::FOLD_AMT, 0.3);
        set(&mut e, layer, P::COMB_AMT, 0.3);
        set(&mut e, layer, P::CLANG_AMT, 0.3);
        set(&mut e, layer, P::CRUSH_BITS, 8.0);
        set(&mut e, layer, P::TAPE_AMT, 0.5);
    }
    render_all(&mut e, 4800, 128);
    // Blobs are built before the watch starts; loading them must not allocate.
    let kind = KIND_BEAT;
    let fields = klont::classic::default_fields(kind);
    let kit = [P::neutral_set(); KIT_SLOTS];
    let patch = klont::classic::patch_blob(kind, 2);
    let layer = klont::classic::layer_blob(kind, &fields, &P::neutral_set(), &kit, &[0.5; N_MACROS], &P::neutral_set());
    let states = klont::classic::states_blob(kind, &[(P::neutral_set(), [0.5; N_MACROS], 0.0), (P::neutral_set(), [0.7; N_MACROS], 0.0)], &kit);
    let sample = klont::classic::sample_blob(SR, 4800.0, &vec![0.1; 48_000]);
    let melody = klont::classic::patch_blob(KIND_MELODY, 2);
    ALLOCS.store(0, Ordering::Relaxed);
    ARMED.with(|a| a.set(true));
    for i in 0..2000 {
        if i == 300 {
            assert_eq!(e.load_bytes(2, LOAD_SAMPLE, &sample), LOAD_OK);
            assert_eq!(e.load_bytes(2, LOAD_PATCH, &patch), LOAD_OK);
            assert_eq!(e.load_bytes(2, LOAD_LAYER, &layer), LOAD_OK);
            assert_eq!(e.load_bytes(2, LOAD_STATES, &states), LOAD_OK);
        }
        if i == 1200 {
            assert_eq!(e.load_bytes(2, LOAD_PATCH, &melody), LOAD_OK);
            assert_eq!(e.load_bytes(2, LOAD_SAMPLE, &sample), LOAD_OK);
        }
        if i % 50 == 0 {
            for layer in 3..MAX_LAYERS as u8 {
                e.push(MSG_NOTE_ON, layer, 0, 0, 4000.0 + 37.0 * i as f32 % 2400.0, (i % 128) as u32);
            }
        }
        if i % 50 == 25 {
            for layer in 3..MAX_LAYERS as u8 {
                e.push(MSG_NOTE_OFF, layer, 0, 0, 4000.0 + 37.0 * (i - 25) as f32 % 2400.0, 3);
            }
        }
        e.process(128);
    }
    ARMED.with(|a| a.set(false));
    assert_eq!(ALLOCS.load(Ordering::Relaxed), 0, "allocations while rendering or loading");
}

/// Mic in, tier, tuner, frames, an armed layer started and released over and
/// over (melody, then beat): nothing allocates.
#[test]
fn live_follow_process_does_not_allocate() {
    let mut e = Engine::new(SR, 128);
    e.push(MSG_SET_LAYER, 1, 0, LAYER_KIND, KIND_BEAT as f32, 0);
    e.push(MSG_SET_TUNE, 0, 0, TUNE_SNAP, 0.5, 0);
    e.push(MSG_SET_LIVE_KIT, 1, 2, LIVE_KIT_TAUGHT, 1.0, 0);
    e.push(MSG_ARM, 0, 0, 0, 1.0, 0);
    // 220 Hz bursts with gaps so the gate opens and closes, and clicks for onsets.
    let sample = |n: usize| -> f32 {
        let t = n as f32 / SR;
        let on = (t * 2.5).fract() < 0.6;
        if on { 0.3 * (std::f32::consts::TAU * 220.0 * t).sin() * (1.0 + 0.5 * (std::f32::consts::TAU * 880.0 * t).sin()) } else { 0.0 }
    };
    let mut n = 0usize;
    let mut feed = |e: &mut Engine| {
        for i in 0..128 {
            e.input_mut()[i] = sample(n + i);
        }
        n += 128;
        e.process(128);
    };
    for _ in 0..600 {
        feed(&mut e);
    }
    ALLOCS.store(0, Ordering::Relaxed);
    ARMED.with(|a| a.set(true));
    for i in 0..4000 {
        if i == 1500 {
            e.push(MSG_ARM, 1, 0, 0, 1.0, 0);
        }
        if i == 2500 {
            e.push(MSG_RESET_SINGER, 0, 0, 0, 0.0, 0);
            e.push(MSG_SET_TUNE, 1, 0, TUNE_OCTAVE, 1.0, 0);
        }
        feed(&mut e);
    }
    ARMED.with(|a| a.set(false));
    assert_eq!(ALLOCS.load(Ordering::Relaxed), 0, "allocations in live process");
    assert!(e.meter()[M_LIVE_HITS] > 0.0, "beat layer never fired");
}

#[test]
fn bad_loads_are_rejected_and_reported() {
    let mut e = Engine::new(SR, 128);
    assert_eq!(e.load_bytes(0, 99, &[0; 8]), LOAD_BAD_KIND);
    assert_eq!(e.load_bytes(0, LOAD_TRACK, &[0; 32]), LOAD_BAD_MAGIC);
    assert_eq!(e.load_bytes(9, LOAD_LAYER, &[0; 64]), LOAD_BAD_SLOT);
    assert_eq!(e.load_bytes(0, LOAD_LAYER, &[0; 16]), LOAD_MALFORMED);
    e.render(128);
    assert_eq!(e.meter()[M_LOAD_STATUS], LOAD_MALFORMED as f32);
}

#[test]
fn layer_blob_sets_the_stored_sound() {
    let mut e = Engine::new(SR, 128);
    let mut fields = klont::classic::default_fields(KIND_MELODY);
    fields[LAYER_LEVEL as usize] = -6.0;
    let mut base = P::neutral_set();
    base[P::SRC_TILT] = 0.9;
    let kit = [P::neutral_set(); KIT_SLOTS];
    assert_eq!(klont::classic::load(&mut e, 0, &fields, &base, &kit, &[0.5; N_MACROS], &P::neutral_set()), LOAD_OK);
    render_all(&mut e, 48_000, 128);
    let i = klont::classic::index(KIND_MELODY, P::SRC_TILT)[0];
    assert!((e.meter()[M_STORED + i] - 0.9).abs() < 1e-3);
    assert_eq!(e.meter()[M_N_VALUES] as usize, klont::classic::layout(KIND_MELODY).vals.len());
}

#[test]
fn bad_patches_are_refused_and_the_old_one_keeps_playing() {
    use klont::patch::PATCH_MAGIC;
    let mut e = Engine::new(SR, 128);
    e.push(MSG_NOTE_ON, 0, 0, 0, 5700.0, 0);
    render_all(&mut e, 4800, 128);
    let good = klont::classic::patch_blob(KIND_MELODY, 0);
    let w = |b: &[u8], i: usize| u32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]);
    let put = |b: &mut Vec<u8>, i: usize, x: u32| b[i * 4..i * 4 + 4].copy_from_slice(&x.to_le_bytes());
    let n = w(&good, 1) as usize;
    // Word offset of the first voice patch's module list.
    let vp = 7 + 2 * n;
    let modules = vp + 13;
    let cables = modules + 2 * w(&good, vp) as usize;
    let mut cases: Vec<(&str, Vec<u8>)> = Vec::new();
    cases.push(("truncated", good[..good.len() - 4].to_vec()));
    let mut x = good.clone();
    put(&mut x, modules, 77);
    cases.push(("unknown module type", x));
    let mut x = good.clone();
    put(&mut x, modules + 2 * 6, klont::patch::NONE);
    cases.push(("unknown module type at the end", x));
    let mut x = good.clone();
    // Cable 0 (partials -> amp) turned backwards: amp -> partials is a cycle in run order.
    put(&mut x, cables, 1);
    put(&mut x, cables + 1, 0);
    cases.push(("cycle", x));
    let mut x = good.clone();
    put(&mut x, modules + 2 * 6, params::T_PARTIALS);
    cases.push(("two partial banks (cap)", x));
    let mut x = good.clone();
    put(&mut x, 1, 5000);
    cases.push(("too many values", x));
    for (what, blob) in cases {
        assert_eq!(e.load_bytes(0, LOAD_PATCH, &blob), LOAD_MALFORMED, "{what}");
    }
    assert_eq!(e.load_bytes(0, LOAD_PATCH, &[0; 8]), LOAD_BAD_MAGIC);
    assert_eq!(w(&good, 0), PATCH_MAGIC);
    let (l, _) = render_all(&mut e, 4800, 128);
    assert!(rms(&l) > 0.01, "the note on the old patch keeps sounding");
    // A layer blob must carry one value per value of the patch.
    let mut b = klont::classic::layer_blob(KIND_MELODY, &klont::classic::default_fields(KIND_MELODY), &P::neutral_set(), &[P::neutral_set(); KIT_SLOTS], &[0.5; N_MACROS], &P::neutral_set());
    put(&mut b, LAYER_OFF_N, n as u32 - 1);
    assert_eq!(e.load_bytes(0, LOAD_LAYER, &b), LOAD_MALFORMED);
}

#[test]
fn curve_loudness_shapes_a_stored_note() {
    // A one-second note driven by a take whose loudness is 0 for 0.5 s, then 1.
    let mut e = Engine::new(SR, 4096);
    set(&mut e, 0, P::SRC_NOISE_MIX, 0.0);
    e.push(MSG_SET_LAYER, 0, 0, LAYER_DEPTH_LOUD, 1.0, 0);
    e.render(1);
    let n = 120usize;
    let mut words: Vec<u32> = vec![CURVE_MAGIC, n as u32];
    for i in 0..n {
        let mut fr = [0f32; CURVE_STRIDE];
        fr[0] = 5700.0;
        fr[1] = 1.0;
        fr[2] = if i < 50 { 0.0 } else { 1.0 };
        fr[CURVE_STRIDE - 1] = 5700.0;
        words.extend(fr.iter().map(|x| x.to_bits()));
    }
    let curve: Vec<u8> = words.iter().flat_map(|x| x.to_le_bytes()).collect();
    assert_eq!(e.load_bytes(0, LOAD_CURVE, &curve), LOAD_OK);
    // 120 bpm: one beat is 0.5 s; anchors map clip ticks to take seconds.
    let c = ClipDef { layer: 0, loop_len: 8.0 * PPQ as f32, events: vec![note(0.0, 2.0, 5700.0)], curve: 0, warp: vec![(0.0, 0.0), (1.0, 2.0 * PPQ as f32)] };
    assert_eq!(e.load_bytes(0, LOAD_TRACK, &track(8.0, 120.0, false, &[c])), LOAD_OK);
    e.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
    let (l, _) = render_all(&mut e, 48_000, 4096);
    let quiet = rms(&l[4800..21_600]);
    let loud = rms(&l[30_000..45_000]);
    assert!(loud > 0.01 && quiet < loud * 0.05, "quiet {quiet} loud {loud}");
}

#[test]
fn clock_counts_processed_samples_and_reports_tempo() {
    let mut e = song(256);
    let clock = |e: &Engine| e.meter()[M_CLOCK_HI] as u64 * (1 << 20) + e.meter()[M_CLOCK_LO] as u64;
    // song() ran render(1), and render counts toward the clock too.
    let mut total = 1u64;
    for &n in &[128usize, 100, 256, 7, 128] {
        e.input_mut()[..n].fill(0.0);
        e.process(n);
        total += n as u64;
        assert_eq!(clock(&e), total);
    }
    // The song has been playing for these samples: position agrees with the clock.
    let ups = 120.0 * PPQ as f64 / (60.0 * SR as f64);
    assert!((e.meter()[M_UNITS_PER_SAMPLE] as f64 - ups).abs() < 1e-6, "{}", e.meter()[M_UNITS_PER_SAMPLE]);
    assert!((e.meter()[M_SECTION_POS] as f64 - (total - 1) as f64 * ups).abs() < 0.5);
    // A long run crosses the 2^20 split: HI becomes 1 and the sum still matches.
    while total < (1 << 20) + 1000 {
        e.input_mut()[..256].fill(0.0);
        e.process(256);
        total += 256;
    }
    assert_eq!(e.meter()[M_CLOCK_HI], 1.0);
    assert_eq!(clock(&e), total);
    // Stopped: no motion reported.
    e.push(MSG_STOP, 0, 0, 0, 0.0, 0);
    e.process(128);
    assert_eq!(e.meter()[M_UNITS_PER_SAMPLE], 0.0);
}
