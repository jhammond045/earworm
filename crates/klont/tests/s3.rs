//! Stage S3 checks on the engine (docs/synth.md "S3: editing patches"):
//! 1. a cycle, a cap overrun, an unknown type and a truncated blob are each refused, and the
//!    engine keeps playing the old patch;
//! 2. a saw oscillator near 2 kHz keeps its aliasing at least 40 dB below the fundamental
//!    (`cargo test -p klont --release --test s3 -- --nocapture` prints the figures from 100 Hz
//!    to 8 kHz);
//! 3. a structure edit while 8 voices play makes no step larger than the 5 ms fade.
//! Also: the voice count follows the patch's cost, notes that start during the fade wait for
//! the new patch, and a bypass switch keeps notes sounding.

use klont::build::{self, FxSpec, RouteSpec, VoiceSpec};
use klont::modules::Osc;
use klont::patch::*;
use klont::protocol::*;
use klont::Engine;
use params as P;

const SR: f32 = 48_000.0;

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

fn render(e: &mut Engine, n: usize, block: usize) -> (Vec<f32>, Vec<f32>) {
    let (mut l, mut r) = (Vec::with_capacity(n), Vec::with_capacity(n));
    let mut done = 0;
    while done < n {
        let k = block.min(n - done);
        e.render(k);
        l.extend_from_slice(&e.output().0[..k]);
        r.extend_from_slice(&e.output().1[..k]);
        done += k;
    }
    (l, r)
}

/// Two oscillators (a saw, and a sine frequency-modulating it), a noise and a filter, into an
/// amp the first envelope opens.
fn osc_voice() -> VoiceSpec {
    let (o1, o2, n, f, a) = (0, 1, 2, 3, 4);
    VoiceSpec {
        modules: vec![P::T_OSC, P::T_OSC, P::T_NOISE, P::T_FILTER, P::T_AMP],
        cables: vec![(o1, o2, PORT_PM), (o2, f, PORT_IN), (n, f, PORT_IN), (f, a, PORT_IN)],
        out: vec![a],
        mods: vec![P::T_ENV],
        routes: vec![RouteSpec { mode: MODE_SCALE, polarity: POL_DOWN, ..RouteSpec::new(SRC_MOD, 0, a, P::amp::GAIN, 1.0) }],
        set: vec![
            (o1, P::osc::WAVE, 0.0),
            (o1, P::osc::OCTAVE, 1.0),
            (o2, P::osc::PM, 1.0),
            (n, P::noise::LEVEL, 0.1),
            (f, P::filter::CUTOFF, 3000.0),
            (5, P::env::SUSTAIN, 1.0),
        ],
        ..VoiceSpec::default()
    }
}

fn osc_patch() -> Vec<u8> {
    build::patch(&[osc_voice()], &[(P::T_CRUSH, false), (P::T_CFILTER, false), (P::T_DELAY, false), (P::T_REVERB, false)], &[])
}

fn word(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]])
}
fn put(b: &mut [u8], i: usize, x: u32) {
    b[i * 4..i * 4 + 4].copy_from_slice(&x.to_le_bytes());
}

#[test]
fn check1_bad_patches_are_refused_and_the_old_one_keeps_playing() {
    let mut e = Engine::new(SR, 128);
    let good = osc_patch();
    assert_eq!(e.load_bytes(0, LOAD_PATCH, &good), LOAD_OK);
    e.push(MSG_NOTE_ON, 0, 0, 0, 5700.0, 0);
    let (before, _) = render(&mut e, 9600, 128);
    assert!(rms(&before[4800..]) > 0.01, "the oscillator patch sounds");

    let n = word(&good, 1) as usize;
    let vp = 7 + 2 * n;
    let modules = vp + 14;
    let cables = modules + 2 * word(&good, vp) as usize;
    let mut cases: Vec<(&str, Vec<u8>)> = Vec::new();
    cases.push(("truncated", good[..good.len() - 4].to_vec()));
    cases.push(("truncated in the header", good[..20].to_vec()));
    let mut x = good.clone();
    put(&mut x, modules + 2 * 2, 99);
    cases.push(("unknown module type", x));
    let mut x = good.clone();
    // The pm cable osc1 -> osc2 turned round: osc2 runs before osc1 reads it, a cycle in run order.
    put(&mut x, cables, 1);
    put(&mut x, cables + 1, 0);
    cases.push(("cycle", x));
    let mut v = osc_voice();
    v.modules = vec![P::T_OSC; 5];
    v.cables.clear();
    v.out = vec![4];
    v.routes.clear();
    v.set.clear();
    cases.push(("five oscillators (cap 4)", build::patch(&[v], &[], &[])));
    let mut v = osc_voice();
    v.modules[3] = P::T_NOISE;
    v.modules[4] = P::T_NOISE;
    v.cables.clear();
    v.routes.clear();
    v.set.clear();
    cases.push(("three noise modules (cap 2)", build::patch(&[v], &[], &[])));
    let mut v = osc_voice();
    v.cables[1] = (1, 3, PORT_PM);
    cases.push(("a pm cable into a filter", build::patch(&[v], &[], &[])));
    let mut v = osc_voice();
    v.cables.push((3, 0, PORT_IN));
    cases.push(("an audio cable into an oscillator", build::patch(&[v], &[], &[])));
    cases.push(("three delays (cap 2)", build::patch(&[osc_voice()], &[(P::T_DELAY, false); 3], &[])));
    cases.push(("two reverbs (cap 1)", build::patch(&[osc_voice()], &[(P::T_REVERB, false); 2], &[])));
    cases.push(("nine channel effects (cap 8)", build::patch(&[osc_voice()], &[(P::T_CFILTER, false); 9], &[])));
    cases.push(("crush after the delay", build::patch(&[osc_voice()], &[(P::T_DELAY, false), (P::T_CRUSH, false)], &[])));
    let mut v = osc_voice();
    v.modules.push(P::T_SAMPLER);
    cases.push(("a sampler without a sample", build::patch(&[v], &[], &[])));
    for (what, blob) in &cases {
        assert_eq!(e.load_bytes(0, LOAD_PATCH, blob), LOAD_MALFORMED, "{what}");
    }
    let (after, _) = render(&mut e, 4800, 128);
    assert!(rms(&after) > 0.5 * rms(&before[4800..]), "the note on the old patch keeps sounding");
    // The old patch is still the one loaded: a layer blob for it is taken.
    assert_eq!(e.meter()[M_N_VALUES] as usize, n);
}

/// Magnitude of the windowed DFT of `x` at `f` Hz (4-term Blackman-Harris).
fn dft(x: &[f32], f: f64) -> f64 {
    let n = x.len();
    let w = std::f64::consts::TAU * f / SR as f64;
    let (mut re, mut im) = (0.0f64, 0.0f64);
    let (c, s) = (w.cos(), w.sin());
    let (mut cr, mut ci) = (1.0f64, 0.0f64);
    for (i, &v) in x.iter().enumerate() {
        let t = std::f64::consts::TAU * i as f64 / n as f64;
        let win = 0.35875 - 0.48829 * t.cos() + 0.14128 * (2.0 * t).cos() - 0.01168 * (3.0 * t).cos();
        let y = v as f64 * win;
        re += y * cr;
        im -= y * ci;
        // Rotate by w; renormalise now and then.
        let nr = cr * c - ci * s;
        ci = cr * s + ci * c;
        cr = nr;
        if i % 1024 == 0 {
            let m = (cr * cr + ci * ci).sqrt();
            cr /= m;
            ci /= m;
        }
    }
    (re * re + im * im).sqrt()
}

/// The strongest alias of a saw at `f0`, in dB against the fundamental: every harmonic above
/// Nyquist folded back, skipping those that land within 6 Hz of a true harmonic or of DC.
fn saw_alias_db(f0: f32) -> (f64, f64) {
    let mut o = Osc::new();
    let n = SR as usize;
    let mut x = vec![0.0f32; n + 4800];
    let mut g = 1.0;
    o.render(&mut x, f0 / SR, 2.0 / 3.0, 0.5, None, &mut g, 0.0);
    let x = &x[4800..];
    let fund = dft(x, f0 as f64);
    let nyq = SR as f64 / 2.0;
    let f0 = f0 as f64;
    let top = ((10.0 * SR as f64) / f0) as usize;
    let mut worst = (f64::NEG_INFINITY, 0.0);
    for h in 1..=top {
        let fh = h as f64 * f0;
        if fh < nyq {
            continue;
        }
        let m = fh.rem_euclid(SR as f64);
        let fa = if m > nyq { SR as f64 - m } else { m };
        let near_harmonic = ((fa / f0).round() * f0 - fa).abs() < 6.0;
        if fa < 20.0 || near_harmonic {
            continue;
        }
        let db = 20.0 * (dft(x, fa) / fund).log10();
        if db > worst.0 {
            worst = (db, fa);
        }
    }
    worst
}

#[test]
fn check2_saw_aliasing_stays_40_db_down() {
    // Notes, so the folded harmonics fall between the true ones: G2, B3, B4, B5, B6 (the note
    // nearest 2 kHz), B7, B8.
    let notes = [("G2", 97.999), ("B3", 246.94), ("B4", 493.88), ("B5", 987.77), ("B6", 1975.53), ("B7", 3951.07), ("B8", 7902.13)];
    println!("saw oscillator aliasing, strongest folded harmonic against the fundamental:");
    for (name, f0) in notes {
        let (db, at) = saw_alias_db(f0);
        println!("  {name} {f0:8.2} Hz: {db:6.1} dB (at {at:.0} Hz)");
        if name == "B6" {
            assert!(db <= -40.0, "aliasing at {f0} Hz is {db:.1} dB");
        }
    }
}

/// Largest sample-to-sample change of the louder channel.
fn max_step(l: &[f32], r: &[f32]) -> f32 {
    l.windows(2).zip(r.windows(2)).map(|(a, b)| (a[1] - a[0]).abs().max((b[1] - b[0]).abs())).fold(0.0, f32::max)
}

#[test]
fn check3_a_structure_edit_while_8_voices_play_has_no_step_beyond_the_fade() {
    for (what, patch, edited) in [
        ("classic", klont::classic::patch_blob(KIND_MELODY, 0), osc_patch()),
        ("oscillators", osc_patch(), klont::classic::patch_blob(KIND_MELODY, 0)),
    ] {
        let mut e = Engine::new(SR, 128);
        assert_eq!(e.load_bytes(0, LOAD_PATCH, &patch), LOAD_OK);
        for v in 0..8 {
            e.push(MSG_NOTE_ON, 0, 0, 0, 3600.0 + 350.0 * v as f32, 0);
        }
        let (l, r) = render(&mut e, 24_000, 128);
        assert_eq!(e.meter()[M_LAYER_VOICES], 8.0, "{what}");
        let steady = max_step(&l[12_000..], &r[12_000..]);
        let peak = l[12_000..].iter().chain(&r[12_000..]).fold(0.0f32, |a, x| a.max(x.abs()));
        assert!(peak > 0.05, "{what}: {peak}");
        assert_eq!(e.load_bytes(0, LOAD_PATCH, &edited), LOAD_OK);
        let (l2, r2) = render(&mut e, 4800, 128);
        let edit = max_step(&l2, &r2);
        // What a 5 ms linear fade of the loudest sample adds per sample, on top of the signal's own.
        let fade = peak / (0.005 * SR);
        println!("{what}: steady max step {steady:.4}, during the edit {edit:.4}, peak {peak:.3}, fade allows {:.4}", steady + fade);
        assert!(edit <= steady + fade, "{what}: a step of {edit} during the edit (steady {steady}, peak {peak})");
        // Once the fade and the next tick are over the layer adds nothing: what is left is the
        // output DC blocker settling, which moves by less than 1e-4 a sample and dies away.
        let after = max_step(&l2[250..], &r2[250..]);
        let end = l2[4700..].iter().chain(&r2[4700..]).fold(0.0f32, |a, x| a.max(x.abs()));
        assert!(after < 1e-4 && end < 1e-3, "{what}: steps of {after} after the fade, {end} at the end");
        assert_eq!(e.meter()[M_LAYER_VOICES], 0.0);
    }
}

#[test]
fn a_note_that_starts_during_the_fade_plays_on_the_new_patch() {
    let mut e = Engine::new(SR, 32);
    e.push(MSG_NOTE_ON, 0, 0, 0, 4800.0, 0);
    render(&mut e, 4800, 32);
    assert_eq!(e.load_bytes(0, LOAD_PATCH, &osc_patch()), LOAD_OK);
    render(&mut e, 64, 32);
    e.push(MSG_NOTE_ON, 0, 0, 0, 5700.0, 0);
    render(&mut e, 64, 32);
    assert_eq!(e.meter()[M_LAYER_VOICES], 1.0, "the old note fades; the new one waits");
    let (l, _) = render(&mut e, 4800, 32);
    assert_eq!(e.meter()[M_LAYER_VOICES], 1.0, "the waiting note started");
    assert!(rms(&l[2400..]) > 0.01);
    // It started within the fade (5 ms) and a tick of the request.
    let first = l.iter().position(|x| x.abs() > 1e-5).unwrap();
    assert!(first < 240, "started {first} samples late");
}

#[test]
fn a_bypass_switch_keeps_notes_sounding() {
    let mut e = Engine::new(SR, 128);
    let chan: [FxSpec; 2] = [(P::T_CFILTER, false), (P::T_REVERB, false)];
    let on = build::patch(&[osc_voice()], &chan, &[(0, P::cfilter::CUTOFF, 500.0)]);
    let off = build::patch(&[osc_voice()], &[(P::T_CFILTER, true), (P::T_REVERB, false)], &[(0, P::cfilter::CUTOFF, 500.0)]);
    assert_eq!(e.load_bytes(0, LOAD_PATCH, &on), LOAD_OK);
    e.push(MSG_NOTE_ON, 0, 0, 0, 5700.0, 0);
    let (dark, _) = render(&mut e, 9600, 128);
    assert_eq!(e.load_bytes(0, LOAD_PATCH, &off), LOAD_OK);
    let (bright, _) = render(&mut e, 9600, 128);
    assert_eq!(e.meter()[M_LAYER_VOICES], 1.0, "bypass is not a structure change");
    // Without the 500 Hz filter more of the saw gets through.
    assert!(rms(&bright[4800..]) > 1.5 * rms(&dark[4800..]), "{} {}", rms(&bright[4800..]), rms(&dark[4800..]));
}

#[test]
fn the_voice_count_follows_the_patch_cost() {
    let mut e = Engine::new(SR, 128);
    e.render(128);
    assert_eq!(e.meter()[M_MAX_VOICES], 8.0, "the classic patch gets 8 voices");
    let mut kit = Engine::new(SR, 128);
    assert_eq!(kit.load_bytes(0, LOAD_PATCH, &klont::classic::patch_blob(KIND_BEAT, 0)), LOAD_OK);
    kit.render(128);
    assert_eq!(kit.meter()[M_MAX_VOICES], 8.0, "the classic kit gets 8 voices");
    // The heaviest patch the caps allow: fewer voices.
    let heavy = build::heaviest_patch();
    assert_eq!(e.load_bytes(0, LOAD_PATCH, &heavy), LOAD_OK);
    e.render(128);
    let n = e.meter()[M_MAX_VOICES] as usize;
    let mut p = Patch::new_boxed();
    p.parse(&heavy).unwrap();
    let classic = |k| {
        let mut c = Patch::new_boxed();
        c.parse(&klont::classic::patch_blob(k, 0)).unwrap();
        c
    };
    let (m, b) = (classic(KIND_MELODY), classic(KIND_BEAT));
    println!(
        "budget {:.1} ns a layer; classic melody {:.1} a voice + {:.1}, classic kit {:.1} + {:.1}; heaviest patch {:.1} a voice + {:.1}: {n} voices",
        layer_budget(&[&m, &b]), m.max_voice_cost(), m.layer_cost(), b.max_voice_cost(), b.layer_cost(), p.max_voice_cost(), p.layer_cost()
    );
    assert!(n < 8 && n >= 4, "a heavier voice gets fewer voices: {n}");
    for k in 0..8 {
        e.push(MSG_NOTE_ON, 0, 0, 0, 4800.0 + 100.0 * k as f32, 0);
    }
    e.render(128);
    assert_eq!(e.meter()[M_LAYER_VOICES] as usize, n, "no more voices than the cost allows");
}
