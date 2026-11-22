//! Stage S5 checks that need only the engine (docs/synth.md "S5: recording and automation" and
//! "The raw input beside the synth"):
//! - automation lanes play back (track blob v3);
//! - live playback after Play from a stop matches an export (the Play-time reset);
//! - the raw-input mix at 0 and 1 nulls against the synth alone and the raw input alone;
//! - a take with an automated mix sweep renders bit-identically twice;
//! - the offset between the raw input and the synth it drives, live and on playback.
//!
//! The recorded-pass check (record, undo, journal replay) is in web/src/record/record.test.ts.

use klont::build::{self, FxSpec, LayerRouteSpec, VoiceSpec};
use klont::curve::{CURVE_MAGIC, CURVE_STRIDE};
use klont::patch::*;
use klont::protocol::*;
use klont::seq::{FEEL_FREE, LANE_LINE, LANE_VALUE, TRACK_MAGIC3};
use klont::Engine;
use params as P;

const SR: f32 = 48_000.0;
const QUANTUM: usize = 128;
/// Sample id of the take audio (past the eight grain samples, as the app does).
const TAKE_SAMPLE: u32 = 8;

// ---------------------------------------------------------------- blobs

struct W(Vec<u8>);
impl W {
    fn u(&mut self, x: u32) {
        self.0.extend_from_slice(&x.to_le_bytes());
    }
    fn f(&mut self, x: f32) {
        self.u(x.to_bits());
    }
}

struct Lane {
    kind: u32,
    index: u32,
    points: Vec<(f32, f32)>,
}

/// A v3 track: one free section `len` seconds long, one clip on layer 0 with these notes
/// (t, dur, cents), its take curve in `curve`, its take audio in `audio`, and lanes.
fn track(len: f32, notes: &[(f32, f32, f32)], curve: i32, audio: i32, lanes: &[Lane]) -> Vec<u8> {
    let mut w = W(Vec::new());
    let n_points: usize = lanes.iter().map(|l| l.points.len()).sum();
    for x in [TRACK_MAGIC3, 1, 1, notes.len() as u32, 0, 0, 0, lanes.len() as u32] {
        w.u(x);
    }
    w.u(FEEL_FREE);
    w.f(len);
    w.f(0.0);
    w.u(1);
    w.f(0.0);
    w.u(1);
    w.u(0);
    w.u(0);
    for x in [0, 0, len.to_bits(), 0, notes.len() as u32, curve as u32, 0, 0, 0, 0, audio as u32, 0, lanes.len() as u32, 0, 0, 0] {
        w.u(x);
    }
    for &(t, dur, cents) in notes {
        w.f(t);
        w.f(dur);
        w.f(cents);
        w.f(0.0);
        w.u(0);
        w.f(1.0);
        (6..16).for_each(|_| w.f(0.0));
    }
    w.u(n_points as u32);
    let mut first = 0;
    for l in lanes {
        w.u(l.kind);
        w.u(l.index);
        w.u(first);
        w.u(l.points.len() as u32);
        first += l.points.len() as u32;
    }
    for l in lanes {
        for &(t, v) in &l.points {
            w.f(t);
            w.f(v);
            w.u(LANE_LINE);
        }
    }
    w.0
}

fn curve_blob(frames: &[f32]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend(CURVE_MAGIC.to_le_bytes());
    b.extend(((frames.len() / CURVE_STRIDE) as u32).to_le_bytes());
    frames.iter().for_each(|x| b.extend(x.to_le_bytes()));
    b
}

/// A sine osc into an amp the first envelope opens (sustain 1). The amp's gain starts at
/// `gain`.
fn sine_voice(gain: f32) -> VoiceSpec {
    let mut v = build::simple_voice(&[P::T_OSC]);
    v.set.push((0, P::osc::WAVE, 0.0));
    v.set.push((1, P::amp::GAIN, gain));
    v
}

/// The sung level opens the amp: layer.volume adds up to gain 1 (no smoothing in the route).
fn volume_route() -> LayerRouteSpec {
    LayerRouteSpec { src: SRC_LAYER_VOLUME, arg: 0, vp: Some(0), module: 1, param: P::amp::GAIN, mode: MODE_ADD, polarity: POL_UP, amount: 0.5, rise_ms: 0.0, fall_ms: 0.0 }
}

/// Value index of `input1.mix` in a patch from `build::patch` with one sine voice and this
/// channel: the voice's values come first.
fn mix_index(voice: &VoiceSpec, channel: &[FxSpec], input_at: usize) -> usize {
    let mut n = 0;
    for &c in voice.modules.iter().chain(&voice.mods) {
        n += P::module_type(c).unwrap().n_values;
    }
    n += 1 + voice.routes.len();
    for &(c, _) in &channel[..input_at] {
        n += P::module_type(c & !CH_RAW).unwrap().n_values;
    }
    n + P::input::MIX
}

// ---------------------------------------------------------------- signals

/// A sung-like tone: 220 Hz with five harmonics and a 5 Hz vibrato, silent until `onset`, then
/// a 2 ms attack to about -12 dBFS.
fn sung(n: usize, onset: usize) -> Vec<f32> {
    let mut ph = 0.0f64;
    (0..n)
        .map(|i| {
            if i < onset {
                return 0.0;
            }
            let t = (i - onset) as f64 / SR as f64;
            let f = 220.0 * (1.0 + 0.01 * (std::f64::consts::TAU * 5.0 * t).sin());
            ph += std::f64::consts::TAU * f / SR as f64;
            let a = (t / 0.002).min(1.0) * 0.25;
            (a * (1..=5).map(|k| (k as f64 * ph).sin() / k as f64).sum::<f64>()) as f32
        })
        .collect()
}

/// Run the live tier over `sig` as a take is captured: the high-passed audio the engine leaves
/// in its input buffer, and the frame records it makes (what the app stores).
fn capture(sig: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let mut e = Engine::new(SR, QUANTUM);
    let (mut audio, mut frames) = (Vec::new(), Vec::new());
    for c in sig.chunks(QUANTUM) {
        e.input_mut()[..c.len()].copy_from_slice(c);
        e.process(c.len());
        audio.extend_from_slice(&e.input()[..c.len()]);
        frames.extend_from_slice(e.frame_records());
    }
    (audio, frames)
}

fn render(e: &mut Engine, n: usize, block: usize) -> (Vec<f32>, Vec<f32>) {
    let (mut l, mut r) = (Vec::with_capacity(n), Vec::with_capacity(n));
    let mut done = 0;
    while done < n {
        let k = block.min(n - done);
        e.render(k);
        let (a, b) = e.output();
        l.extend_from_slice(&a[..k]);
        r.extend_from_slice(&b[..k]);
        done += k;
    }
    (l, r)
}

fn max_diff_db(a: &[f32], b: &[f32]) -> f64 {
    let m = a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
    20.0 * (m.max(1e-30) as f64).log10()
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

/// The master bus after the layers: the engine's DC blocker and soft clip.
fn master(x: &[f32]) -> Vec<f32> {
    let mut dc = dsp::filters::DcBlocker::new(SR);
    x.iter()
        .map(|&v| {
            let v = dc.process(v).clamp(-3.0, 3.0);
            (v * (27.0 + v * v) / (27.0 + 9.0 * v * v)).clamp(-1.0, 1.0)
        })
        .collect()
}

// ---------------------------------------------------------------- a take on layer 0

struct Take {
    audio: Vec<f32>,
    frames: Vec<f32>,
}

fn take(secs: f32, onset: usize) -> Take {
    let (audio, frames) = capture(&sung((secs * SR) as usize, onset));
    Take { audio, frames }
}

/// A fresh engine playing `patch` on layer 0 with the take's curve and audio, a note held
/// from 0 to `secs` - 0.5 at A3, and `lanes`.
fn take_engine(t: &Take, patch: &[u8], secs: f32, lanes: &[Lane]) -> Box<Engine> {
    let mut e = Engine::new(SR, 4096);
    assert_eq!(e.load_bytes(0, LOAD_PATCH, patch), LOAD_OK);
    assert_eq!(e.load_bytes(0, LOAD_CURVE, &curve_blob(&t.frames)), LOAD_OK);
    assert_eq!(e.load_bytes(TAKE_SAMPLE, LOAD_SAMPLE, &klont::classic::sample_blob(SR, f32::NAN, &t.audio)), LOAD_OK);
    let blob = track(secs, &[(0.0, secs - 0.5, 4500.0)], 0, TAKE_SAMPLE as i32, lanes);
    assert_eq!(e.load_bytes(0, LOAD_TRACK, &blob), LOAD_OK);
    e.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
    e
}

// ---------------------------------------------------------------- checks

#[test]
fn a_lane_moves_its_value_while_the_transport_plays() {
    let voice = sine_voice(1.0);
    let ch: Vec<FxSpec> = vec![(P::T_INPUT, false)];
    let mix = mix_index(&voice, &ch, 0);
    let patch = build::patch(&[voice], &ch, &[]);
    let t = take(2.0, 4800);
    // mix 0 until 0.5 s, then a ramp to 1 at 1.5 s.
    let lanes = [Lane { kind: LANE_VALUE, index: mix as u32, points: vec![(0.5, 0.0), (1.5, 1.0)] }];
    let mut e = take_engine(&t, &patch, 2.0, &lanes);
    let (l, _) = render(&mut e, (2.0 * SR) as usize, 4096);
    let mut quiet = take_engine(&t, &patch, 2.0, &[]);
    let (q, _) = render(&mut quiet, (2.0 * SR) as usize, 4096);
    let s = |a: f32, b: f32| ((a * SR) as usize, (b * SR) as usize);
    let (a, b) = s(0.1, 0.45);
    assert_eq!(&l[a..b], &q[a..b], "before the ramp the lane holds its first value");
    let (a, b) = s(1.6, 1.9);
    assert!(max_diff_db(&l[a..b], &q[a..b]) > -20.0, "after the ramp the layer plays the raw input");
}

#[test]
fn live_playback_after_play_from_a_stop_matches_an_export() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/public/m1/demo/");
    let read = |f: &str| std::fs::read(format!("{root}{f}")).unwrap();
    let load = |e: &mut Engine| {
        for i in 0..8u32 {
            assert_eq!(e.load_bytes(i, LOAD_PATCH, &read(&format!("patch{i}.bin"))), LOAD_OK);
            assert_eq!(e.load_bytes(i, LOAD_LAYER, &read(&format!("layer{i}.bin"))), LOAD_OK);
        }
        assert_eq!(e.load_bytes(0, LOAD_TRACK, &read("track.bin")), LOAD_OK);
    };
    let n = (6.0 * SR) as usize;
    // Live: play a while, stop, let the tails die, then play again from the start.
    let mut live = Engine::new(SR, QUANTUM);
    load(&mut live);
    live.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
    for _ in 0..(3.0 * SR) as usize / QUANTUM {
        live.process(QUANTUM);
    }
    live.push(MSG_STOP, 0, 0, 0, 0.0, 0);
    for _ in 0..(8.0 * SR) as usize / QUANTUM {
        live.process(QUANTUM);
    }
    live.seek(0, 0.0);
    live.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
    let (mut ll, mut lr) = (Vec::new(), Vec::new());
    while ll.len() < n {
        live.process(QUANTUM);
        let (a, b) = live.output();
        ll.extend_from_slice(&a[..QUANTUM]);
        lr.extend_from_slice(&b[..QUANTUM]);
    }
    // The export, as the app makes it (render-core `settle`): one second with the transport
    // stopped, then Play.
    let mut ex = Engine::new(SR, 4096);
    load(&mut ex);
    render(&mut ex, SR as usize, 4096);
    ex.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
    let (el, er) = render(&mut ex, n, 4096);
    let d = max_diff_db(&ll[..n], &el).max(max_diff_db(&lr[..n], &er));
    println!("demo song, live playback after a stop against an export: worst difference {d:.1} dB (peak {:.2})", peak(&el));
    assert!(d <= -90.0, "live playback differs from the export by {d:.1} dB");
    // Shorter: it is exact.
    assert_eq!(ll[..n], el[..], "live playback is not bit-identical to the export");
}

#[test]
fn mix_at_0_and_1_nulls_against_the_synth_alone_and_the_raw_input_alone() {
    let secs = 3.0;
    let n = (secs * SR) as usize;
    let t = take(secs, 9600);
    let voice = sine_voice(0.0);
    let route = [volume_route()];
    // A shared delay after the input element: at 0 the synth goes through it exactly as it
    // would with no element.
    let with_input: Vec<FxSpec> = vec![(P::T_INPUT, false), (P::T_DELAY, false)];
    let without: Vec<FxSpec> = vec![(P::T_DELAY, false)];
    let delay_mix = [(1, P::delay::MIX, 0.3), (1, P::delay::TIME, 0.12)];
    let at0 = build::patch_with(&[voice.clone()], &with_input, &[(0, P::input::MIX, 0.0), delay_mix[0], delay_mix[1]], &route);
    let synth = build::patch_with(&[voice.clone()], &without, &[(0, P::delay::MIX, 0.3), (0, P::delay::TIME, 0.12)], &route);
    let (a, _) = render(&mut take_engine(&t, &at0, secs, &[]), n, 4096);
    let (b, _) = render(&mut take_engine(&t, &synth, secs, &[]), n, 4096);
    let d0 = max_diff_db(&a, &b);
    println!("mix 0 against the synth alone: worst difference {d0:.1} dB (synth peak {:.2})", peak(&b));
    assert!(peak(&b) > 0.05, "the synth sounds");
    assert!(d0 <= -90.0);

    // At 1, with only the element in the channel: the take, read as much earlier as its frames
    // lag it, through level and pan (0 dB, centre) and the master bus.
    let only: Vec<FxSpec> = vec![(P::T_INPUT, false)];
    let at1 = build::patch_with(&[voice], &only, &[(0, P::input::MIX, 1.0)], &route);
    let mut e = take_engine(&t, &at1, secs, &[]);
    let delay = (e.take_lag_s() * SR as f64).round() as usize;
    let (l, r) = render(&mut e, n, 4096);
    let g = std::f32::consts::FRAC_PI_4.cos();
    let raw: Vec<f32> = (0..n).map(|i| i.checked_sub(delay).map_or(0.0, |k| t.audio[k]) * g).collect();
    let want = master(&raw);
    let d1 = max_diff_db(&l, &want).max(max_diff_db(&r, &want));
    println!("mix 1 against the raw input alone (read {delay} samples late): worst difference {d1:.1} dB (raw peak {:.2})", peak(&want));
    assert!(peak(&want) > 0.05);
    assert!(d1 <= -90.0);
}

#[test]
fn a_take_with_an_automated_mix_sweep_renders_bit_identically_twice() {
    let secs = 4.0;
    let n = (secs * SR) as usize;
    let t = take(secs, 4800);
    let voice = sine_voice(0.0);
    // A raw-path filter and a synth-path crush before the element, a reverb after it.
    let ch: Vec<FxSpec> = vec![(P::T_CRUSH, false), (P::T_CFILTER | CH_RAW, false), (P::T_INPUT, false), (P::T_REVERB, false)];
    let set = [(0, P::crush::BITS, 8.0), (1, P::cfilter::CUTOFF, 1200.0), (3, P::reverb::MIX, 0.3)];
    let patch = build::patch_with(&[voice.clone()], &ch, &set, &[volume_route()]);
    let mix = mix_index(&voice, &ch, 2);
    let lanes = [Lane { kind: LANE_VALUE, index: mix as u32, points: vec![(0.2, 0.0), (1.8, 1.0), (2.4, 1.0), (3.6, 0.0)] }];
    let (a, ar) = render(&mut take_engine(&t, &patch, secs, &lanes), n, 4096);
    let (b, br) = render(&mut take_engine(&t, &patch, secs, &lanes), n, 333);
    assert!(a == b && ar == br, "two renders of the sweep differ");
    let (s, sr_) = render(&mut take_engine(&t, &patch, secs, &[]), n, 4096);
    let mid = (2.1 * SR) as usize..(2.3 * SR) as usize;
    assert!(max_diff_db(&a[mid.clone()], &s[mid]) > -30.0, "the sweep moves the sound");
    println!("mix sweep 0 -> 1 -> 0 over {secs} s: two renders (4096 and 333-sample blocks) are bit-identical; peak {:.2} {:.2}", peak(&a), peak(&sr_));
}

/// Where a signal's envelope first reaches half its level over `[from, to)` (samples).
fn half_rise(x: &[f32], from: usize, to: usize) -> usize {
    let a = (-1.0 / (0.001 * SR)).exp();
    let mut env = Vec::with_capacity(x.len());
    let mut y = 0.0f32;
    for &v in x {
        y = a * y + (1.0 - a) * v.abs();
        env.push(y);
    }
    let level = env[from..to].iter().sum::<f32>() / (to - from) as f32;
    env.iter().position(|&e| e >= 0.5 * level).unwrap()
}

fn offsets(what: &str, synth: &[f32], raw: &[f32], onset: usize, delay: f64) -> f64 {
    let steady = (onset + (0.5 * SR) as usize, onset + (1.0 * SR) as usize);
    let (s, r) = (half_rise(synth, steady.0, steady.1), half_rise(raw, steady.0, steady.1));
    let ms = |x: f64| x * 1000.0 / SR as f64;
    let off = s as f64 - r as f64;
    println!(
        "{what}: raw input reaches half level {:.2} ms after the sung onset, the synth {:.2} ms; synth - raw = {:+.2} ms ({} samples); without the {:.2} ms the raw input is held back it would be {:+.2} ms",
        ms(r as f64 - onset as f64),
        ms(s as f64 - onset as f64),
        ms(off),
        off,
        delay * 1000.0,
        ms(off) + delay * 1000.0
    );
    off
}

#[test]
fn offset_between_the_raw_input_and_the_synth_it_drives() {
    let onset = (0.6 * SR) as usize;
    let secs = 2.0;
    let n = (secs * SR) as usize;
    let voice = sine_voice(0.0);
    let only: Vec<FxSpec> = vec![(P::T_INPUT, false)];
    let patch = |m: f32| build::patch_with(&[voice.clone()], &only, &[(0, P::input::MIX, m)], &[volume_route()]);

    // Live: the layer listens, a note is held from before the onset, the mic sings.
    let sig = sung(n, onset);
    let live = |m: f32| {
        let mut e = Engine::new(SR, QUANTUM);
        assert_eq!(e.load_bytes(0, LOAD_PATCH, &patch(m)), LOAD_OK);
        e.push(MSG_LISTEN, 0, 0, 0, 1.0, 0);
        e.push(MSG_NOTE_ON, 0, 0, 0, 4500.0, 0);
        let mut out = Vec::new();
        for c in sig.chunks(QUANTUM) {
            e.input_mut()[..c.len()].copy_from_slice(c);
            e.process(c.len());
            out.extend_from_slice(&e.output().0[..c.len()]);
        }
        (out, e.mic_delay())
    };
    let ((ls, delay), (lr, _)) = (live(0.0), live(1.0));
    let off_live = offsets("live (listening layer, mic)", &ls, &lr, onset, delay as f64 / SR as f64);

    // Playback: the same singing as a take.
    let t = take(secs, onset);
    let mut e = take_engine(&t, &patch(0.0), secs, &[]);
    let lag = e.take_lag_s();
    let (ps, _) = render(&mut e, n, 4096);
    let (pr, _) = render(&mut take_engine(&t, &patch(1.0), secs, &[]), n, 4096);
    let off_play = offsets("playback (the take)", &ps, &pr, onset, lag);
    // The take was captured live, so playback lines up as live does, to within a frame.
    assert!((off_live - off_play).abs() <= 480.0, "live and playback differ by {} samples", off_live - off_play);
    // With the delay the two land within 5 ms of each other.
    assert!(off_live.abs() <= 240.0 && off_play.abs() <= 240.0, "offsets {off_live} and {off_play} samples");
}

