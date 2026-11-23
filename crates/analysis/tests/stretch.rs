//! Stretching a flexed take's audio (docs/quant.md "Q2"), through the engine as the app plays
//! it: the flex prototype's drum loop A and sung line, their anchors from the second pass,
//! quantized at full strength (drums to straight 16ths, the sung line to straight 8ths), sent as
//! a v5 track and rendered on a layer whose patch plays only its raw input.
//!
//! - every attack at a hard anchor is found at the sample the map gives (correlation above 0.99);
//! - sung pitch within 5 cents; legato joins within 0.5 dB of the input's level;
//! - spectral distance to the ideal render below the untouched input's;
//! - two exports bit-identical; 128- and 333-sample calls bit-identical; play from eight points
//!   repeats exactly and matches a whole render from the next hard anchor on; live playback
//!   from a stop against an export.
//!
//! Run with `--release -- --nocapture` for the figures. `Q2_WAV=<dir>` also writes the listening
//! examples (see docs/Q2.md).

mod common;
use analysis::second::Kind;
use common::flex_map::*;
use common::flex_takes::{Feel, Take, drums, sung};
use common::*;
use dsp::fft::{Fft, hann};
use klont::Engine;
use klont::protocol::*;
use klont::seq::{FEEL_GRID, TRACK_MAGIC5};
use klont::stretch::{MODE_HITS, MODE_READ, MODE_SMOOTH, POINT_HARD, POINT_RUN};
use params as P;

const BPM: f64 = 100.0;
const TAKE_SAMPLE: u32 = 8;
/// Section units per second at BPM.
const UPS: f64 = BPM * 960.0 / 60.0;
/// The take goes into the engine at this gain, so the master's soft clip stays out of the way.
const IN_GAIN: f32 = 0.25;

// ---------------------------------------------------------------- quantizing

// ---------------------------------------------------------------- the engine

struct W(Vec<u8>);
impl W {
    fn u(&mut self, x: u32) {
        self.0.extend_from_slice(&x.to_le_bytes());
    }
    fn f(&mut self, x: f32) {
        self.u(x.to_bits());
    }
}

/// A v5 track: one looping grid section `len` seconds long at BPM, one clip on layer 0 as long,
/// with the take's audio stretched through `map` in `mode` (MODE_READ with an empty map: the
/// take as it was performed).
fn track(len: f64, map: &[(f64, f64, bool)], mode: u32) -> Vec<u8> {
    let mut w = W(Vec::new());
    let ticks = (len * UPS) as f32;
    for x in [TRACK_MAGIC5, 1, 1, 0, map.len() as u32, 1, 0, 0] {
        w.u(x);
    }
    w.u(FEEL_GRID);
    w.f(ticks);
    w.f(BPM as f32);
    w.u(1);
    w.f(0.0);
    w.u(1);
    w.u(0);
    w.u(0);
    for x in [0, 0, ticks.to_bits(), 0, 0, u32::MAX, 0, 0, 0, 0, TAKE_SAMPLE, 0, 0, 0, map.len() as u32, mode] {
        w.u(x);
    }
    for (i, &(u, s, hard)) in map.iter().enumerate() {
        w.f(s as f32);
        w.f((u * UPS) as f32);
        w.u(if hard { POINT_HARD } else { 0 } | if i == 0 { POINT_RUN } else { 0 });
    }
    w.u(0);
    w.0
}

/// An engine with the take on layer 0, whose patch plays only the raw input, and the track.
fn engine(take: &[f32], blob: &[u8], block: usize) -> Box<Engine> {
    let mut e = Engine::new(SR, block);
    let mut v = klont::build::simple_voice(&[P::T_OSC]);
    v.set.push((1, P::amp::GAIN, 0.0));
    let patch = klont::build::patch_with(&[v], &[(P::T_INPUT, false)], &[(0, P::input::MIX, 1.0)], &[]);
    assert_eq!(e.load_bytes(0, LOAD_PATCH, &patch), LOAD_OK);
    let quiet: Vec<f32> = take.iter().map(|x| x * IN_GAIN).collect();
    assert_eq!(e.load_bytes(TAKE_SAMPLE, LOAD_SAMPLE, &klont::classic::sample_blob(SR, f32::NAN, &quiet)), LOAD_OK);
    assert_eq!(e.load_bytes(0, LOAD_TRACK, blob), LOAD_OK);
    e
}

fn render(e: &mut Engine, n: usize, block: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let k = block.min(n - out.len());
        e.render(k);
        out.extend_from_slice(&e.output().0[..k]);
    }
    out
}

/// Play from the start and render `n` samples in calls of `block`.
fn export(take: &[f32], blob: &[u8], n: usize, block: usize) -> Vec<f32> {
    let mut e = engine(take, blob, block.max(128));
    e.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
    render(&mut e, n, block)
}

// ---------------------------------------------------------------- measures

fn stats(errs: &[f64]) -> (f64, f64) {
    let a: Vec<f64> = errs.iter().map(|e| e.abs()).collect();
    (a.iter().sum::<f64>() / a.len().max(1) as f64, a.iter().copied().fold(0.0, f64::max))
}

/// Spectral distance in dB: the RMS difference of third-octave band levels from 100 Hz to 8 kHz,
/// over frames where `b` is within 50 dB of its loudest (the prototype's measure).
fn lsd(a: &[f32], b: &[f32]) -> f64 {
    let (n, hop) = (2048, 512);
    let fft = Fft::new(n);
    let mut win = vec![0.0f32; n];
    hann(&mut win);
    let edges: Vec<usize> = (0..=19).map(|i| (100.0 * 2f32.powf(i as f32 / 3.0) / SR * n as f32).round() as usize).collect();
    let len = a.len().min(b.len());
    let spec = |x: &[f32], start: usize, out: &mut Vec<f32>| {
        let (mut re, mut im) = (vec![0.0f32; n], vec![0.0f32; n]);
        for i in 0..n {
            re[i] = x.get(start + i).copied().unwrap_or(0.0) * win[i];
        }
        fft.forward(&mut re, &mut im);
        out.clear();
        for w in edges.windows(2) {
            let p: f32 = (w[0]..w[1].max(w[0] + 1)).map(|k| re[k] * re[k] + im[k] * im[k]).sum();
            out.push(10.0 * (p + 1e-10).log10());
        }
    };
    let energy = |x: &[f32], s: usize| -> f32 { x[s..(s + n).min(x.len())].iter().map(|v| v * v).sum() };
    let frames: Vec<usize> = (0..len.saturating_sub(n)).step_by(hop).collect();
    let peak = frames.iter().map(|&s| energy(b, s)).fold(0.0f32, f32::max);
    let (mut sa, mut sb) = (Vec::new(), Vec::new());
    let (mut sum, mut cnt) = (0.0f64, 0usize);
    for &s in &frames {
        if energy(b, s) < peak * 1e-5 {
            continue;
        }
        spec(a, s, &mut sa);
        spec(b, s, &mut sb);
        let d: f64 = sa.iter().zip(&sb).map(|(x, y)| ((x - y) as f64).powi(2)).sum::<f64>() / sa.len() as f64;
        sum += d.sqrt();
        cnt += 1;
    }
    sum / cnt.max(1) as f64
}

/// Median YIN pitch in cents over [t0, t1) seconds.
fn pitch_cents(x: &[f32], t0: f64, t1: f64) -> Option<f32> {
    let max_lag = (SR / 70.0).ceil() as usize;
    let len = (0.03 * SR) as usize + max_lag;
    let mut scratch = vec![0.0f32; max_lag];
    let mut got = Vec::new();
    let mut t = t0;
    while t < t1 {
        let s = (t * SR as f64) as usize;
        if s + len < x.len() {
            let e = dsp::yin::yin(&x[s..s + len], SR, 70.0, 1000.0, 0.15, &mut scratch);
            if e.hz > 0.0 && e.confidence > 0.7 {
                got.push(dsp::hz_to_cents(e.hz));
            }
        }
        t += 0.01;
    }
    got.sort_by(|a, b| a.total_cmp(b));
    got.get(got.len() / 2).copied()
}

/// For each hard point of the map other than the ends: the lag in samples, within 30 ms, at
/// which the output best matches the take's audio from `pre_ms` before the point to 20 ms
/// after, and the normalized correlation there.
fn attacks(input: &[f32], out: &[f32], map: &[(f64, f64, bool)], pre_ms: f64) -> Vec<(i64, f64)> {
    let ms = |m: f64| (m * SR as f64 / 1000.0) as i64;
    let (pre, len, reach) = (ms(pre_ms), ms(pre_ms + 20.0), ms(30.0));
    let get = |x: &[f32], i: i64| if i >= 0 && (i as usize) < x.len() { x[i as usize] } else { 0.0 };
    map[1..map.len() - 1]
        .iter()
        .filter(|p| p.2)
        .map(|&(u, s, _)| {
            let (u, s) = ((u * SR as f64).round() as i64, (s * SR as f64).round() as i64);
            let xs: Vec<f32> = (0..len).map(|i| get(input, s - pre + i)).collect();
            let xx: f64 = xs.iter().map(|v| (v * v) as f64).sum();
            let score = |lag: i64| -> f64 {
                let (mut xy, mut yy) = (0.0f64, 1e-24f64);
                for (i, &x) in xs.iter().enumerate() {
                    let y = get(out, u - pre + lag + i as i64) as f64;
                    xy += x as f64 * y;
                    yy += y * y;
                }
                xy / (xx * yy).sqrt().max(1e-24)
            };
            (-reach..=reach).map(|l| (l, score(l))).fold((0, f64::NEG_INFINITY), |b, c| if c.1 > b.1 { c } else { b })
        })
        .collect()
}

/// The level dip at `t` seconds: the quietest 5 ms within 30 ms against the level 40 to 60 ms
/// either side, in dB.
fn dip(x: &[f32], t: f64) -> f32 {
    let rms = |a: f64, b: f64| {
        let (i, j) = ((a * SR as f64) as usize, (b * SR as f64) as usize);
        (x[i..j].iter().map(|v| v * v).sum::<f32>() / (j - i) as f32).sqrt()
    };
    let around = 0.5 * (rms(t - 0.06, t - 0.04) + rms(t + 0.04, t + 0.06));
    let mut lo = f32::INFINITY;
    let mut u = t - 0.03;
    while u < t + 0.03 {
        lo = lo.min(rms(u, u + 0.005));
        u += 0.001;
    }
    dsp::lin_to_db(lo / around)
}


// ---------------------------------------------------------------- the takes

struct Case {
    take: Take,
    moved: Vec<Moved>,
    map: Vec<(f64, f64, bool)>,
    len: f64,
}

fn case(take: Take, kind: Kind, step_beats: f64, strength: f64) -> Case {
    let anchors = found(&take.audio, kind);
    let step = step_beats * 60.0 / BPM;
    let len = take.audio.len() as f64 / SR as f64;
    let moved = quantize(&anchors, take.lead, step, strength);
    let map = stretch_map(&moved, len);
    Case { take, moved, map, len }
}

fn drums_a() -> Take {
    drums(11, BPM, 4, &Feel { swing: 0.58, snare_ms: 12.0, jitter_ms: 6.0, drift_ms: 10.0 })
}

fn sung_line() -> Take {
    sung(5, BPM, 15.0, 20.0)
}

/// The engine's output for the case in `mode` (MODE_READ: the take as performed). Every
/// measure compares engine outputs with engine outputs, since the master's DC blocker (a 38 Hz
/// high-pass) and the layer's gain shape them all alike.
fn play(c: &Case, mode: u32) -> Vec<f32> {
    let blob = if mode == MODE_READ { track(c.len, &[], MODE_READ) } else { track(c.len, &c.map, mode) };
    export(&c.take.audio, &blob, c.take.audio.len(), 4096)
}

/// The stretcher alone, from output sample `start` on (a seek, as Play from a stop does).
fn stretch_from(c: &Case, mode: u32, start: usize) -> Vec<f32> {
    stretch_alone(&c.take.audio, &c.map, mode, start)
}

/// The ideal render (every event on its slot) through the engine, as `play` gives the take.
fn play_ideal(c: &Case) -> Vec<f32> {
    export(&c.take.ideal, &track(c.len, &[], MODE_READ), c.take.ideal.len(), 4096)
}

// ---------------------------------------------------------------- checks

#[test]
fn attacks_land_on_the_sample_the_map_gives() {
    for (name, c) in [("drums A", case(drums_a(), Kind::Beat, 0.25, 1.0)), ("sung", case(sung_line(), Kind::Melody, 0.5, 1.0))] {
        for mode in [MODE_HITS, MODE_SMOOTH] {
            let (out, read) = (stretch_from(&c, mode, 0), c.take.audio.clone());
            // The prototype's measure: 2 ms before the point to 20 ms after.
            let a = attacks(&read, &out, &c.map, 2.0);
            let mean = a.iter().map(|x| x.1).sum::<f64>() / a.len() as f64;
            let worst = a.iter().map(|x| x.1).fold(1.0, f64::min);
            // From the point on: the attack itself.
            let b = attacks(&read, &out, &c.map, 0.0);
            let worst_on = b.iter().map(|x| x.1).fold(1.0, f64::min);
            let off: Vec<i64> = a.iter().chain(&b).map(|x| x.0).filter(|&l| l != 0).collect();
            println!("{name}, mode {mode}: {} hard anchors, correlation from 2 ms before: mean {mean:.4}, worst {worst:.4}; from the point on: worst {worst_on:.4}; off the sample: {off:?}", a.len());
            assert!(a.len() > 10);
            assert!(off.is_empty(), "{name} mode {mode}: attacks off by {off:?} samples");
            assert!(mean > 0.99, "{name} mode {mode}: mean correlation {mean:.4}");
            assert!(worst_on > 0.99, "{name} mode {mode}: worst correlation from the point on {worst_on:.4}");
        }
    }
}

#[test]
fn the_engine_plays_what_the_stretcher_renders() {
    // The layer's raw input at mix 1 is the stretcher's output times the layer's gain, then the
    // master's DC blocker and soft clip.
    for (name, c) in [("drums A", case(drums_a(), Kind::Beat, 0.25, 1.0)), ("sung", case(sung_line(), Kind::Melody, 0.5, 1.0))] {
        for mode in [MODE_HITS, MODE_SMOOTH] {
            let out = play(&c, mode);
            let st: Vec<f32> = stretch_from(&c, mode, 0).iter().map(|v| v * IN_GAIN).collect();
            // A layer at 0 dB, panned to the centre (equal power).
            let g = std::f64::consts::FRAC_1_SQRT_2;
            let soft = |x: f32| {
                let x = x.clamp(-3.0, 3.0);
                (x * (27.0 + x * x) / (27.0 + 9.0 * x * x)).clamp(-1.0, 1.0)
            };
            let mut dc = dsp::filters::DcBlocker::new(SR);
            let want: Vec<f32> = st.iter().map(|&v| soft(dc.process(v * g as f32))).collect();
            let (e, p) = out.iter().zip(&want).fold((0.0f64, 0.0f64), |(e, p), (a, b)| (e + ((a - b) as f64).powi(2), p + (*b as f64).powi(2)));
            let db = 10.0 * (e / p).log10();
            println!("{name}, mode {mode}: the engine against the stretcher through the layer gain and the master: {db:.1} dB");
            assert!(db < -80.0, "{name} mode {mode}: {db:.1} dB");
        }
    }
}

#[test]
fn sung_pitch_and_legato_joins() {
    let c = case(sung_line(), Kind::Melody, 0.5, 1.0);
    let take = &c.take;
    let want: Vec<f64> = take.events.iter().map(|e| e.t + (take.lead + e.beat * 60.0 / BPM - e.t)).collect();
    let out = play(&c, MODE_SMOOTH);
    let read = play(&c, MODE_READ);
    let pitch = |x: &[f32], at: &dyn Fn(usize) -> f64| -> Vec<f64> {
        let mut errs = Vec::new();
        for (i, e) in take.events.iter().enumerate() {
            let end = if i + 1 < take.events.len() { at(i + 1) } else { at(i) + 0.6 };
            let t0 = at(i) + 0.06;
            if let Some(cents) = pitch_cents(x, t0, (t0 + 0.15).min(end - 0.03)) {
                errs.push((cents - e.cents) as f64);
            }
        }
        errs
    };
    let (pm, px) = stats(&pitch(&out, &|i| forward(&c.moved, take.events[i].t)));
    let (rm, rx) = stats(&pitch(&read, &|i| take.events[i].t));
    println!("sung pitch, smooth: mean {pm:.2} cents, max {px:.2} (the take as performed: {rm:.2} / {rx:.2}); notes on 8ths {}", want.len());
    assert!(px <= 5.0, "pitch off by up to {px:.1} cents");
    let mut worst = 0.0f32;
    for e in take.events.iter().filter(|e| e.class == 2) {
        let (a, b) = (dip(&read, e.t), dip(&out, forward(&c.moved, e.t)));
        println!("legato join at beat {}: input {a:.2} dB, smooth {b:.2} dB", e.beat);
        worst = worst.max((a - b).abs());
    }
    assert!(worst <= 0.5, "a legato join is {worst:.2} dB off the input's level");
}

#[test]
fn closer_to_the_ideal_render_than_the_untouched_take() {
    for (name, c, modes) in [
        ("drums A", case(drums_a(), Kind::Beat, 0.25, 1.0), &[MODE_HITS, MODE_SMOOTH][..]),
        ("sung", case(sung_line(), Kind::Melody, 0.5, 1.0), &[MODE_SMOOTH][..]),
    ] {
        let ideal = play_ideal(&c);
        let input = lsd(&play(&c, MODE_READ), &ideal);
        for &mode in modes {
            let d = lsd(&play(&c, mode), &ideal);
            println!("{name}: spectral distance to the ideal render, untouched {input:.2} dB, mode {mode} {d:.2} dB");
            assert!(d < input, "{name} mode {mode}: {d:.2} dB, untouched {input:.2} dB");
        }
    }
}

#[test]
fn renders_do_not_depend_on_calls_or_where_play_started() {
    for (name, c) in [("drums A", case(drums_a(), Kind::Beat, 0.25, 1.0)), ("sung", case(sung_line(), Kind::Melody, 0.5, 1.0))] {
        let n = c.take.audio.len();
        for mode in [MODE_HITS, MODE_SMOOTH] {
            let blob = track(c.len, &c.map, mode);
            let whole = export(&c.take.audio, &blob, n, 4096);
            assert_eq!(whole, export(&c.take.audio, &blob, n, 4096), "{name} {mode}: two exports");
            assert_eq!(whole, export(&c.take.audio, &blob, n, 128), "{name} {mode}: 128-sample calls");
            assert_eq!(whole, export(&c.take.audio, &blob, n, 333), "{name} {mode}: 333-sample calls");
            // Play from eight points, on the layer's stretcher itself: the engine's master DC
            // blocker starts again at Play, so its output differs from a whole render for a few
            // ms after any start point, stretching or not.
            let from = |start: usize| stretch_from(&c, mode, start);
            let whole = from(0);
            let mut longest = 0.0f64;
            for k in 1..9 {
                let start = n * k / 9 + 1234;
                let a = from(start);
                assert_eq!(a, from(start), "{name} {mode}: play from {start} repeats");
                // From the next hard point on, the play from `start` equals the whole render.
                let next = c.map.iter().filter(|p| p.2).map(|p| (p.0 * SR as f64).round() as usize).find(|&u| u > start).unwrap_or(n);
                if let Some(d) = a.iter().zip(&whole[start..]).rposition(|(x, y)| x.to_bits() != y.to_bits()) {
                    longest = longest.max(d as f64 / SR as f64 * 1000.0);
                    assert!(start + d < next, "{name} {mode}: play from {start} still differs {d} samples later, past the next hard point at {next}");
                }
            }
            println!("{name}, mode {mode}: exports and call sizes bit-identical; plays from eight points repeat and differ for at most {longest:.0} ms");
        }
    }
}

#[test]
fn live_playback_from_a_stop_against_an_export() {
    // As S5's check runs it: live, play a while, stop, let the tails die, play from the start;
    // the export settles one second stopped, then plays. MODE_READ is the same check without
    // stretching, what `main` plays.
    let c = case(drums_a(), Kind::Beat, 0.25, 1.0);
    let n = (3.0 * SR) as usize;
    for mode in [MODE_READ, MODE_HITS, MODE_SMOOTH] {
        let blob = if mode == MODE_READ { track(c.len, &[], MODE_READ) } else { track(c.len, &c.map, mode) };
        let mut live = engine(&c.take.audio, &blob, 128);
        live.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
        for _ in 0..(1.5 * SR) as usize / 128 {
            live.process(128);
        }
        live.push(MSG_STOP, 0, 0, 0, 0.0, 0);
        for _ in 0..(2.0 * SR) as usize / 128 {
            live.process(128);
        }
        live.seek(0, 0.0);
        live.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
        let mut ll = Vec::new();
        while ll.len() < n {
            live.process(128);
            ll.extend_from_slice(&live.output().0[..128]);
        }
        let mut ex = engine(&c.take.audio, &blob, 4096);
        render(&mut ex, SR as usize, 4096);
        ex.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
        let el = render(&mut ex, n, 4096);
        let m = ll[..n].iter().zip(&el).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        let db = 20.0 * (m.max(1e-30) as f64).log10();
        println!("mode {mode}: live playback after a stop against an export: worst difference {db:.1} dBFS, bit-identical {}", ll[..n] == el[..]);
        assert!(db <= -120.0, "mode {mode}: {db:.1} dB");
    }
}

#[test]
fn listening_examples() {
    // Short before/after renders for the owner to judge by ear (docs/Q2.md); writes nothing
    // unless Q2_WAV names a directory. Each: the take as performed, then quantized at 0.5 and at
    // 1, through the engine.
    if std::env::var("Q2_WAV").is_err() {
        return;
    }
    // 6 s each; each take's three renders share one gain that brings the loudest to -1 dBFS.
    let cut = |x: Vec<f32>| x[..(6.0 * SR) as usize].to_vec();
    let takes: [(&str, fn() -> Take, Kind, f64, u32); 3] = [
        ("drums", drums_a, Kind::Beat, 0.25, MODE_HITS),
        ("drums-smooth", drums_a, Kind::Beat, 0.25, MODE_SMOOTH),
        ("sung", sung_line, Kind::Melody, 0.5, MODE_SMOOTH),
    ];
    for (name, take, kind, step, mode) in takes {
        let full = case(take(), kind, step, 1.0);
        let renders = [
            ("0-as-played", cut(play(&full, MODE_READ))),
            ("1-strength-50", cut(play(&case(take(), kind, step, 0.5), mode))),
            ("2-strength-100", cut(play(&full, mode))),
        ];
        let peak = renders.iter().flat_map(|r| r.1.iter()).fold(0.0f32, |m, v| m.max(v.abs()));
        let g = 0.89 / peak;
        for (what, x) in &renders {
            // The smooth drums share the hits drums' as-played file, and are there at full
            // strength only, to compare the two modes.
            if name == "drums-smooth" && *what != "2-strength-100" {
                continue;
            }
            write_wav(&format!("{name}-{what}"), &x.iter().map(|v| v * g).collect::<Vec<f32>>());
        }
    }
}
