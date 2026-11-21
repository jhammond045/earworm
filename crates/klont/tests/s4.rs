//! Stage S4 checks that need the live engine (docs/synth.md "S4: live play"):
//! a note written to the control ring sounds in the next render quantum, from
//! its first sample; a scene (MSG_MORPH_TO) morphs the layer to a state over
//! its time.

use klont::classic;
use klont::protocol::*;
use klont::Engine;
use params as P;

const SR: f32 = 48_000.0;
/// One Web Audio render quantum.
const QUANTUM: usize = 128;

/// Index of the first sample where `e` and `quiet` (the same engine without the note) differ.
fn first_sound(e: &Engine, quiet: &Engine, n: usize) -> Option<usize> {
    let (l, r) = e.output();
    let (ql, qr) = quiet.output();
    (0..n).find(|&i| l[i] != ql[i] || r[i] != qr[i])
}

/// The demo song's drum layer (layer 0 of web/public/m1/demo, `pnpm gen:demo`): a kit
/// whose slots sound the way the app's do.
fn demo_kit(e: &mut Engine) {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/public/m1/demo/");
    let read = |f: &str| std::fs::read(format!("{root}{f}")).unwrap();
    assert_eq!(e.load_bytes(0, LOAD_PATCH, &read("patch0.bin")), LOAD_OK);
    assert_eq!(e.load_bytes(0, LOAD_LAYER, &read("layer0.bin")), LOAD_OK);
}

/// Run `calls` silent quanta (no input), then write a note to the ring and run one more.
/// Returns where in that quantum the output first differs from the same engine without it.
fn note_after(setup: fn(&mut Engine), slot: u8, cents: f32, calls: usize) -> Option<usize> {
    let mut e = Engine::new(SR, QUANTUM);
    let mut quiet = Engine::new(SR, QUANTUM);
    setup(&mut e);
    setup(&mut quiet);
    for _ in 0..calls {
        e.process(QUANTUM);
        quiet.process(QUANTUM);
    }
    // What the main thread does on a MIDI note: one message, time 0 (the next block's start).
    e.push(MSG_NOTE_ON, 0, slot, 800, cents, 0);
    e.process(QUANTUM);
    quiet.process(QUANTUM);
    first_sound(&e, &quiet, QUANTUM)
}

#[test]
fn a_note_in_the_ring_sounds_from_the_first_sample_of_the_next_quantum() {
    let melody: fn(&mut Engine) = |_| {};
    for (what, setup, slot, cents) in [
        ("melody C4", melody, 0u8, 4800.0f32),
        ("melody A5", melody, 0, 6900.0),
        ("kit slot 1", demo_kit as fn(&mut Engine), 0, f32::NAN),
        ("kit slot 2", demo_kit, 1, f32::NAN),
        ("kit slot 3", demo_kit, 2, f32::NAN),
        ("kit slot 4", demo_kit, 3, f32::NAN),
    ] {
        for calls in [3, 17, 40] {
            let at = note_after(setup, slot, cents, calls);
            println!("{what}, after {calls} quanta: first output sample at index {at:?} of the quantum");
            let at = at.unwrap_or_else(|| panic!("{what}: no output in the quantum after the write"));
            // The engine reads the ring at the start of each quantum, so the wait for that read
            // (at most one quantum) plus `at` is the engine's whole delay. A wave that starts at
            // phase 0 with its envelope at 0 is exactly 0 at the first sample, so 1 is allowed.
            assert!(at <= 1, "{what}: the note starts {at} samples into the quantum");
        }
    }
}

/// A kit slot that sounds only through the layer's shaped noise (the classic kit at its
/// starting values) also starts at once: the noise bank, which runs per layer, follows a
/// played note inside the tick it starts in.
#[test]
fn a_noise_only_kit_slot_starts_at_once_too() {
    let bare: fn(&mut Engine) = |e| {
        assert_eq!(e.load_bytes(0, LOAD_PATCH, &classic::patch_blob(KIND_BEAT, 0)), LOAD_OK);
        e.push(MSG_SET_LAYER, 0, 0, LAYER_KIND, KIND_BEAT as f32, 0);
    };
    for slot in 0..4u8 {
        let at = note_after(bare, slot, f32::NAN, 3);
        println!("classic kit at its starting values (shaped noise only), slot {}: first output sample at {at:?}", slot + 1);
        assert!(at.is_some_and(|a| a <= 1), "slot {}: {at:?}", slot + 1);
    }
}

/// Two states of the classic melody patch that differ only in the filter cutoff.
fn states_blob(n: usize, init: &[f32], cut: usize, a: f32, b: f32) -> Vec<u8> {
    let mut w: Vec<u32> = vec![STATES_MAGIC, 2, n as u32];
    for c in [a, b] {
        let mut v = init[..n].to_vec();
        v[cut] = c;
        w.extend(v.iter().map(|x| x.to_bits()));
        w.extend([0.5f32; N_MACROS].iter().map(|x| x.to_bits()));
        w.push(0f32.to_bits());
    }
    w.iter().flat_map(|x| x.to_le_bytes()).collect()
}

#[test]
fn a_scene_morphs_to_its_state_over_its_time_and_holds() {
    let mut e = Engine::new(SR, QUANTUM);
    e.process(QUANTUM);
    let n = e.meter()[M_N_VALUES] as usize;
    let init: Vec<f32> = e.meter()[M_STORED..M_STORED + n].to_vec();
    let cut = classic::index(KIND_MELODY, P::FILT_CUTOFF)[0];
    assert_eq!(e.load_bytes(0, LOAD_STATES, &states_blob(n, &init, cut, 0.2, 0.8)), LOAD_OK);
    settle(&mut e);
    assert!((e.meter()[M_STORED + cut] - 0.2).abs() < 2e-3, "no scene: state 0, not {}", e.meter()[M_STORED + cut]);

    // 200 ms to state 1, read every quantum.
    e.push(MSG_MORPH_TO, 0, 1, 0, 200.0, 0);
    let mut trace = Vec::new();
    for _ in 0..(0.3 * SR) as usize / QUANTUM {
        e.process(QUANTUM);
        trace.push(e.meter()[M_STORED + cut]);
    }
    let at_ms = |k: usize| ((k + 1) * QUANTUM) as f32 / SR * 1000.0;
    let half = trace.iter().position(|&x| x >= 0.5).unwrap();
    let done = trace.iter().position(|&x| x >= 0.79).unwrap();
    println!("scene 0 -> 1 over 200 ms: half way at {:.1} ms, within 0.01 at {:.1} ms", at_ms(half), at_ms(done));
    // The meters show the value after its own smoothing, which lags the blend a little.
    assert!((at_ms(half) - 100.0).abs() < 20.0, "half way at {} ms", at_ms(half));
    assert!((at_ms(done) - 200.0).abs() < 40.0, "arrived at {} ms", at_ms(done));
    assert!(trace.windows(2).all(|w| w[1] >= w[0]), "the cutoff only rises");
    assert!((trace.last().unwrap() - 0.8).abs() < 2e-3, "settles on the state");

    // A second tap half way through starts from where the layer is, not from a state.
    e.push(MSG_MORPH_TO, 0, 0, 0, 200.0, 0);
    for _ in 0..(0.1 * SR) as usize / QUANTUM {
        e.process(QUANTUM);
    }
    let mid = e.meter()[M_STORED + cut];
    e.push(MSG_MORPH_TO, 0, 1, 0, 100.0, 0);
    e.process(QUANTUM);
    let next = e.meter()[M_STORED + cut];
    assert!((next - mid).abs() < 0.02, "a tap mid-morph jumped from {mid} to {next}");
    settle(&mut e);
    assert!((e.meter()[M_STORED + cut] - 0.8).abs() < 2e-3);

    // New states (an edit of the state the scene went to) are heard at once.
    assert_eq!(e.load_bytes(0, LOAD_STATES, &states_blob(n, &init, cut, 0.2, 0.6)), LOAD_OK);
    settle(&mut e);
    assert!((e.meter()[M_STORED + cut] - 0.6).abs() < 2e-3);

    // A zero time is a cut.
    e.push(MSG_MORPH_TO, 0, 0, 0, 0.0, 0);
    settle(&mut e);
    assert!((e.meter()[M_STORED + cut] - 0.2).abs() < 2e-3);
}

/// 300 ms: long enough for any value's own smoothing.
fn settle(e: &mut Engine) {
    for _ in 0..(0.3 * SR) as usize / QUANTUM {
        e.process(QUANTUM);
    }
}

#[test]
fn a_scene_without_states_does_nothing() {
    let mut e = Engine::new(SR, QUANTUM);
    e.process(QUANTUM);
    let cut = classic::index(KIND_MELODY, P::FILT_CUTOFF)[0];
    let before = e.meter()[M_STORED + cut];
    e.push(MSG_MORPH_TO, 0, 1, 0, 0.0, 0);
    e.process(QUANTUM);
    assert_eq!(e.meter()[M_STORED + cut], before);
}

