//! Stage Q2 (docs/quant.md "Q2: stretching the take's audio"), engine side: a stretching layer
//! renders and reloads its map without allocating, a map change while playing crossfades over
//! the 5 ms fade instead of stepping, the loop wraps, and a take at another rate keeps today's
//! read. The quality checks on the prototype's takes are crates/analysis/tests/stretch.rs.

use klont::Engine;
use klont::protocol::*;
use klont::seq::{FEEL_GRID, TRACK_MAGIC5};
use klont::stretch::{MODE_HITS, MODE_SMOOTH, POINT_HARD, POINT_RUN};
use params as P;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);
thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
}

fn armed() -> bool {
    ARMED.try_with(|a| a.get()).unwrap_or(false)
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if armed() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(l.size(), Ordering::Relaxed);
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
const BPM: f64 = 120.0;
const UPS: f64 = BPM * 960.0 / 60.0;
const TAKE_SAMPLE: u32 = 8;
/// The take and the loop: 2 s.
const LEN: f64 = 2.0;

/// A 220 Hz tone with a soft attack every 0.25 s: smooth enough that a step shows.
fn take(rate: f32) -> Vec<f32> {
    let n = (LEN * rate as f64) as usize;
    (0..n)
        .map(|i| {
            let t = i as f64 / rate as f64;
            let env = 0.6 + 0.4 * (-(t % 0.25) / 0.05).exp();
            (0.2 * env * (std::f64::consts::TAU * 220.0 * t).sin()) as f32
        })
        .collect()
}

/// The map: hard points every 0.25 s of the take, each played `shift(k)` seconds off.
fn map(shift: impl Fn(usize) -> f64) -> Vec<(f64, f64, u32)> {
    (0..=8)
        .map(|k| {
            let s = k as f64 * 0.25;
            let u = if k == 0 || k == 8 { s } else { s + shift(k) };
            (u, s, POINT_HARD | if k == 0 { POINT_RUN } else { 0 })
        })
        .collect()
}

/// A v5 track: a looping grid section of LEN seconds, one clip on `layer` with the take
/// stretched through `map` in `mode`.
fn track(layers: &[u32], map: &[(f64, f64, u32)], mode: u32) -> Vec<u8> {
    let mut w: Vec<u32> = Vec::new();
    let ticks = (LEN * UPS) as f32;
    let n = map.len() as u32;
    w.extend([TRACK_MAGIC5, 1, layers.len() as u32, 0, n * layers.len() as u32, 1, 0, 0]);
    w.extend([FEEL_GRID, ticks.to_bits(), (BPM as f32).to_bits(), 1, 0, 1, 0, 0]);
    for (i, &l) in layers.iter().enumerate() {
        w.extend([0, l, ticks.to_bits(), 0, 0, u32::MAX, 0, 0, 0, 0, TAKE_SAMPLE, 0, 0, i as u32 * n, n, mode]);
    }
    for _ in layers {
        for &(u, s, f) in map {
            w.extend([(s as f32).to_bits(), ((u * UPS) as f32).to_bits(), f]);
        }
    }
    w.push(0);
    w.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn raw_only_patch() -> Vec<u8> {
    let mut v = klont::build::simple_voice(&[P::T_OSC]);
    v.set.push((1, P::amp::GAIN, 0.0));
    klont::build::patch_with(&[v], &[(P::T_INPUT, false)], &[(0, P::input::MIX, 1.0)], &[])
}

fn engine(layers: &[u32], blob: &[u8], audio: &[f32], rate: f32) -> Box<Engine> {
    let mut e = Engine::new(SR, 4096);
    let patch = raw_only_patch();
    for &l in layers {
        assert_eq!(e.load_bytes(l, LOAD_PATCH, &patch), LOAD_OK);
    }
    assert_eq!(e.load_bytes(TAKE_SAMPLE, LOAD_SAMPLE, &klont::classic::sample_blob(rate, f32::NAN, audio)), LOAD_OK);
    assert_eq!(e.load_bytes(0, LOAD_TRACK, blob), LOAD_OK);
    e.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
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

/// Largest sample-to-sample step in `x[a..b]`.
fn step(x: &[f32], a: usize, b: usize) -> f32 {
    x[a.max(1)..b].iter().zip(&x[a.max(1) - 1..b - 1]).map(|(p, q)| (p - q).abs()).fold(0.0, f32::max)
}

#[test]
fn eight_stretching_layers_render_and_reload_without_allocating() {
    let layers: Vec<u32> = (0..8).collect();
    let audio = take(SR);
    let a = track(&layers, &map(|k| if k % 2 == 1 { 0.02 } else { -0.015 }), MODE_SMOOTH);
    let b = track(&layers, &map(|k| if k % 2 == 1 { -0.01 } else { 0.01 }), MODE_HITS);
    let mut e = engine(&layers, &a, &audio, SR);
    // Live calls of 128, through a loop wrap, a reload of the map and a stop and play.
    ALLOCS.store(0, Ordering::Relaxed);
    ARMED.with(|x| x.set(true));
    for i in 0..(5.0 * SR as f64) as usize / 128 {
        if i == 400 {
            e.load_bytes(0, LOAD_TRACK, &b);
        }
        if i == 1200 {
            e.push(MSG_STOP, 0, 0, 0, 0.0, 0);
        }
        if i == 1300 {
            e.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
        }
        e.process(128);
    }
    for _ in 0..12 {
        e.render(4096);
    }
    ARMED.with(|x| x.set(false));
    assert_eq!(ALLOCS.load(Ordering::Relaxed), 0, "allocations while stretching");
    assert!(e.output().0.iter().any(|v| v.abs() > 0.01), "the layers play");
}

#[test]
fn a_layer_stretcher_takes_under_400_kb() {
    // Everything a layer's stretching needs, allocated once when the engine starts.
    ALLOCS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    ARMED.with(|x| x.set(true));
    let s = klont::stretch::LayerStretch::new(SR);
    ARMED.with(|x| x.set(false));
    let kb = BYTES.load(Ordering::Relaxed) as f64 / 1024.0;
    println!("one layer's stretcher: {kb:.0} KB in {} allocations", ALLOCS.load(Ordering::Relaxed));
    assert!(kb < 400.0, "{kb:.0} KB");
    drop(s);
}

#[test]
fn a_map_change_while_playing_crossfades() {
    // The tone's own steps are about 0.2 * 2 pi 220 / 48000 = 0.006. Swapping maps without a
    // fade would jump by up to the tone's full swing.
    let audio = take(SR);
    let a = track(&[0], &map(|_| 0.03), MODE_SMOOTH);
    let b = track(&[0], &map(|_| -0.03), MODE_SMOOTH);
    for at in [20_000usize, 33_333, 70_001] {
        let mut e = engine(&[0], &a, &audio, SR);
        let mut out = render(&mut e, at, 128);
        assert_eq!(e.load_bytes(0, LOAD_TRACK, &b), LOAD_OK);
        out.extend(render(&mut e, 24_000, 128));
        let around = step(&out, at - 2400, at + 2400);
        let before = step(&out, at - 9600, at - 2400);
        let alone = render(&mut engine(&[0], &b, &audio, SR), at + 24_000, 128);
        let after = step(&alone, at - 2400, at + 2400).max(step(&out, at + 2400, at + 9600));
        println!("map change at {at}: largest step around it {around:.4}, elsewhere {before:.4} and {after:.4}");
        assert!(around <= 1.5 * before.max(after), "a step of {around:.4} at the map change");
        // From the next hard point on it plays the new map, as a render with the new map does.
        let old = render(&mut engine(&[0], &a, &audio, SR), at + 24_000, 128);
        let (x, y) = (at + 14_400, at + 24_000);
        let diff = |p: &[f32], q: &[f32]| p[x..y].iter().zip(&q[x..y]).map(|(u, v)| (u - v).abs()).fold(0.0f32, f32::max);
        assert!(diff(&out, &alone) < 1e-3, "plays the new map: {}", diff(&out, &alone));
        assert!(diff(&out, &old) > 0.05, "no longer the old map: {}", diff(&out, &old));
    }
    // A reload that leaves the map as it was changes nothing at all.
    let mut e = engine(&[0], &a, &audio, SR);
    let mut out = render(&mut e, 30_000, 128);
    e.load_bytes(0, LOAD_TRACK, &a);
    out.extend(render(&mut e, 30_000, 128));
    assert_eq!(out, render(&mut engine(&[0], &a, &audio, SR), 60_000, 128));
}

#[test]
fn the_loop_wraps_at_its_start() {
    // A map that pulls the first hard point after the loop start earlier still plays each pass
    // like the first, from the loop's first hard point on.
    let audio = take(SR);
    let blob = track(&[0], &map(|k| if k == 1 { -0.02 } else { 0.0 }), MODE_SMOOTH);
    let out = render(&mut engine(&[0], &blob, &audio, SR), (3.0 * LEN * SR as f64) as usize, 4096);
    let n = (LEN * SR as f64) as usize;
    // Skip the DC blocker's start and the crossfade into each pass.
    for k in 1..3 {
        let d = out[2400..n - 2400].iter().zip(&out[k * n + 2400..(k + 1) * n - 2400]).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(d < 1e-4, "pass {k} differs from the first by {d}");
    }
    assert!(step(&out, n - 2400, n + 2400) < 0.02, "no click at the loop start");
}

#[test]
fn a_take_at_another_rate_keeps_todays_read() {
    // Stretching needs the take at the engine's rate; at 44.1 kHz the clip reads it as before.
    let audio = take(44_100.0);
    let blob = track(&[0], &map(|_| 0.0), MODE_SMOOTH);
    let out = render(&mut engine(&[0], &blob, &audio, 44_100.0), 48_000, 4096);
    assert!(out.iter().any(|v| v.abs() > 0.01));
}

#[test]
fn a_tempo_ramp_into_the_section_plays_on_and_catches_up_with_a_crossfade() {
    // Section 0 at 90 bpm, then section 1 at 120 bpm with a 300 ms ramp and the stretching clip.
    // During the ramp the stretcher plays at the section's tempo; when the ramp ends it moves to
    // where the transport is, over the 5 ms crossfade.
    let audio = take(SR);
    let one = track(&[0], &map(|_| 0.02), MODE_SMOOTH);
    let mut w: Vec<u32> = one.chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    // Insert section 0 before the clip's section and point the clip at section 1.
    w[1] = 2;
    w[5] = 0;
    let s1: Vec<u32> = vec![FEEL_GRID, (90.0f32 * 16.0).to_bits(), 90.0f32.to_bits(), 1, 0, 2, 0, 0];
    w[8 + 4] = 300.0f32.to_bits();
    w.splice(8..8, s1);
    w[16 + 8] = 1;
    let blob: Vec<u8> = w.iter().flat_map(|x| x.to_le_bytes()).collect();
    let out = render(&mut engine(&[0], &blob, &audio, SR), 3 * 48_000, 128);
    let mut read = w.clone();
    let mode_at = 8 + 16 + 15;
    read[mode_at] = klont::stretch::MODE_READ;
    let read: Vec<u8> = read.iter().flat_map(|x| x.to_le_bytes()).collect();
    let plain = render(&mut engine(&[0], &read, &audio, SR), 3 * 48_000, 128);
    println!("section entry: largest step stretched {:.4}, read without stretching {:.4}", step(&out, 47_900, 48_600), step(&plain, 47_900, 48_600));
    assert!(out.iter().all(|v| v.is_finite()));
    let ramp_end = 48_000 + 300 * 48;
    println!("ramp: largest step in the ramp {:.4}, around its end {:.4}, after {:.4}", step(&out, 48_000, ramp_end - 600), step(&out, ramp_end - 600, ramp_end + 1200), step(&out, ramp_end + 1200, 2 * 48_000));
    assert!(out[48_000..ramp_end].iter().any(|v| v.abs() > 0.01), "the take plays during the ramp");
    assert!(out[ramp_end + 4800..].iter().any(|v| v.abs() > 0.01), "and after it");
    assert!(step(&out, 48_000 + 600, 2 * 48_000) < 0.02, "no click in or after the ramp");
}
