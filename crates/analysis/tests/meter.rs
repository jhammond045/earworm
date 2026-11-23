//! Meter remap, stage R1 (docs/meter.md "R1"), through the engine. The meter prototype's six
//! cases (experiments/meterremap): its grooves are rendered as stems, each stem's anchors are
//! found by the app's own detector, the app's compiler turns them into the stretch maps of three
//! layers locked to one meter, and the engine's stretcher plays those maps.
//!
//! The two languages meet in two fixture files. This test writes the anchors
//! (`fixtures/meter_anchors.txt`, with `METER_WRITE=1`); `web/src/project/meter-fixture.test.ts`
//! reads them, compiles each case as the app does and writes `fixtures/meter_maps.txt`; this test
//! reads the maps back. Each side checks that the file it writes is still what it produces.
//!
//! - spectral distance to the ideal within 0.1 dB of the prototype's, no click more than 3 dB
//!   over the ideal;
//! - two exports bit-identical; 128- and 333-sample calls bit-identical; play from eight points
//!   repeats and matches a whole render from the next slice on; live playback from a stop
//!   against an export;
//! - no allocation with eight remapped layers.
//!
//! Run with `--release -- --nocapture` for the figures. `R1_WAV=<dir>` writes the renders.

mod common;
use common::grooves::{self as gv, Groove, SR, STEMS};
use klont::Engine;
use klont::protocol::*;
use klont::seq::{FEEL_GRID, TRACK_MAGIC5};
use klont::stretch::{MODE_HITS, POINT_HARD, POINT_RUN, Point, Stretcher};
use params as P;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
}
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if ARMED.try_with(|a| a.get()).unwrap_or(false) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        if ARMED.try_with(|a| a.get()).unwrap_or(false) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static GLOBAL: Counting = Counting;

/// Seconds of the performed take kept before beat 1: the app's take starts here, so a hit played
/// a little early on beat 1 is in the take, at the end of the loop.
const PRE: f64 = 0.03;
/// The engine hears the takes at this gain, so the master's soft clip stays out of the way.
const IN_GAIN: f32 = 0.25;
const FIRST_SAMPLE: u32 = 8;

/// The prototype's "engine, stems" figures (experiments/meterremap/results.md): spectral distance
/// over the whole render and near the joins, and the worst click, dB.
const PROTO: [(&str, f64, f64, f64); 6] = [
    ("five-drop5", 0.64, 2.19, 0.0),
    ("five-drop4", 3.62, 9.50, 2.9),
    ("four-drop4", 0.39, 1.27, -0.1),
    ("three-repeat3", 0.22, 0.46, 0.0),
    ("three-repeat1", 0.57, 1.58, -0.1),
    ("four-shuffle", 2.48, 3.49, 1.7),
];

fn dir() -> String {
    format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"))
}

fn groove(name: &str) -> Groove {
    match name {
        "five" => gv::five(4),
        "four" => gv::four(4),
        "three" => gv::three(4),
        _ => panic!("no groove {name}"),
    }
}

/// The take the app gets for one stem: the performed stem from PRE before beat 1.
fn take_of(g: &Groove, x: &[f32]) -> Vec<f32> {
    x[((g.lead - PRE) * SR as f64).round() as usize..].to_vec()
}

/// Hard anchors of a take, as `crates/analysis/src/anchors.rs` finds them for beat takes (the
/// prototype's `remap::anchors`): (seconds, weight).
fn anchors(x: &[f32]) -> Vec<(f64, f32)> {
    let n = x.len() * 100 / SR as usize;
    let lv = analysis::signal::Levels::new(x, SR, n);
    let full = analysis::signal::detect_onsets(&lv, &analysis::signal::flux(x, SR, n));
    analysis::anchors::onsets_with_high_band(x, SR, n, &full).into_iter().map(|(t, w)| (t as f64, w)).collect()
}

fn anchors_text() -> String {
    let mut s = String::from("# Written by crates/analysis/tests/meter.rs (METER_WRITE=1): each groove stem's take and its hard anchors.\n");
    for name in ["five", "four", "three"] {
        let g = groove(name);
        let (perf, _) = gv::performed(&g);
        let take_len = take_of(&g, &perf.stems[0]).len() as f64 / SR as f64;
        s += &format!("groove {name} bpm {} meter {} bars {} pre {PRE} take {take_len:.6}\n", g.bpm, g.meter, g.bars);
        for (k, stem) in perf.stems.iter().enumerate() {
            let a = anchors(&take_of(&g, stem));
            s += &format!("stem {k} {}\n", a.len());
            for (t, w) in a {
                s += &format!("{t:.6} {w:.5}\n");
            }
        }
    }
    s
}

#[test]
fn the_anchors_fixture_is_what_the_detector_finds() {
    let path = format!("{}/meter_anchors.txt", dir());
    let got = anchors_text();
    if std::env::var("METER_WRITE").is_ok() {
        std::fs::create_dir_all(dir()).unwrap();
        std::fs::write(&path, &got).unwrap();
    }
    let want = std::fs::read_to_string(&path).expect("run with METER_WRITE=1 to write the anchors fixture");
    // Last-digit differences between machines are allowed; anything more means the detector moved.
    let (a, b): (Vec<&str>, Vec<&str>) = (got.lines().collect(), want.lines().collect());
    assert_eq!(a.len(), b.len(), "anchor count changed: write the fixture again and rerun the web test");
    for (x, y) in a.iter().zip(&b) {
        if x == y {
            continue;
        }
        let (p, q): (Vec<f64>, Vec<f64>) = (x.split(' ').filter_map(|v| v.parse().ok()).collect(), y.split(' ').filter_map(|v| v.parse().ok()).collect());
        assert!(p.len() == q.len() && p.iter().zip(&q).all(|(u, v)| (u - v).abs() < 1e-4), "{x} against {y}");
    }
}

// ---------------------------------------------------------------- the app's maps

struct Case {
    name: String,
    groove: String,
    pattern: Vec<usize>,
    /// Section units per second, the clip's loop and the section as they play, in ticks.
    ups: f64,
    loop_len: f64,
    section: f64,
    /// Per stem: the stretch map (clip ticks, take seconds, flags).
    maps: Vec<Vec<(f64, f64, u32)>>,
    /// Output ticks where a run starts.
    joins: Vec<f64>,
}

fn cases() -> Vec<Case> {
    let text = std::fs::read_to_string(format!("{}/meter_maps.txt", dir())).expect("meter_maps.txt: run the web test meter-fixture.test.ts");
    let mut out: Vec<Case> = Vec::new();
    let mut lines = text.lines().filter(|l| !l.starts_with('#'));
    while let Some(l) = lines.next() {
        let w: Vec<&str> = l.split(' ').collect();
        let val = |k: &str| w[w.iter().position(|x| *x == k).unwrap() + 1];
        match w[0] {
            "case" => {
                let p0 = w.iter().position(|x| *x == "pattern").unwrap() + 1;
                let p1 = w.iter().position(|x| *x == "ups").unwrap();
                out.push(Case {
                    name: w[1].into(),
                    groove: val("groove").into(),
                    pattern: w[p0..p1].iter().map(|x| x.parse().unwrap()).collect(),
                    ups: val("ups").parse().unwrap(),
                    loop_len: val("loop").parse().unwrap(),
                    section: val("section").parse().unwrap(),
                    maps: Vec::new(),
                    joins: Vec::new(),
                });
            }
            "stem" => {
                let n: usize = w[2].parse().unwrap();
                let map = (0..n)
                    .map(|_| {
                        let v: Vec<f64> = lines.next().unwrap().split(' ').map(|x| x.parse().unwrap()).collect();
                        (v[0], v[1], v[2] as u32)
                    })
                    .collect();
                out.last_mut().unwrap().maps.push(map);
            }
            "joins" => out.last_mut().unwrap().joins = w[2..].iter().map(|x| x.parse().unwrap()).collect(),
            _ => panic!("meter_maps.txt: {l}"),
        }
    }
    out
}

/// The map in samples, from the f32 values the track blob carries (as the engine reads them).
fn points(c: &Case, map: &[(f64, f64, u32)]) -> Vec<Point> {
    let spu = SR as f64 / c.ups;
    map.iter().map(|&(u, s, flags)| Point { u: (u as f32) as f64 * spu, s: (s as f32) as f64 * SR as f64, flags }).collect()
}

/// One stem played by the engine's stretcher, in hits mode, through its map, `n` samples from
/// output sample `start` of the looping clip, in 128-sample calls.
fn stretch(c: &Case, take: &[f32], map: &[(f64, f64, u32)], start: usize, n: usize) -> Vec<f32> {
    let mut st = Stretcher::new(SR);
    assert!(st.load(MODE_HITS, c.loop_len * SR as f64 / c.ups, points(c, map).into_iter()));
    st.seek(start as f64);
    let mut out = vec![0.0f32; n];
    for ch in out.chunks_mut(128) {
        st.process(take, ch, |_| 1.0);
    }
    out
}

struct Played {
    takes: Vec<Vec<f32>>,
    /// The stems summed, two passes and a bit of the clip's loop.
    out: Vec<f32>,
}

fn play(c: &Case, g: &Groove) -> Played {
    let (perf, _) = gv::performed(g);
    let takes: Vec<Vec<f32>> = perf.stems.iter().map(|s| take_of(g, s)).collect();
    let pass = (c.loop_len * SR as f64 / c.ups).round() as usize;
    let mut out = vec![0.0f32; 3 * pass];
    for s in 0..STEMS {
        for (o, v) in out.iter_mut().zip(stretch(c, &takes[s], &c.maps[s], 0, 3 * pass)) {
            *o += v;
        }
    }
    Played { takes, out }
}

#[test]
fn six_cases_through_the_engine_against_the_prototype() {
    let wav = std::env::var("R1_WAV").ok();
    println!("| Case | Pattern | Distance, all / near joins (dB) | Prototype | Clicks, mean / worst (dB over ideal) | Prototype worst |");
    println!("| --- | --- | --- | --- | --- | --- |");
    let cs = cases();
    assert_eq!(cs.len(), 6);
    for c in &cs {
        let g = groove(&c.groove);
        let (ide, _) = gv::ideal(&g, &c.pattern);
        let src = gv::source_beats(g.meter, g.bars, &c.pattern);
        let end = (g.at(src.len() as f64) * SR as f64) as usize;
        let p = play(c, &g);
        // The second pass, lined up with the ideal: its beat 1 at the groove's lead.
        let pass = (c.loop_len * SR as f64 / c.ups).round() as usize;
        let lead = (g.lead * SR as f64).round() as usize;
        let len = ide.mix.len();
        let a: Vec<f32> = (0..len).map(|i| p.out.get(pass + i - lead).copied().unwrap_or(0.0)).collect();
        // Joins inside the pass (the loop start is where the prototype's render began).
        let tol = 0.07 * c.ups;
        let joins: Vec<i64> = c
            .joins
            .iter()
            .filter(|&&o| o > tol && o < c.loop_len - tol)
            .map(|&o| (o / c.ups * SR as f64).round() as i64 + lead as i64)
            .collect();
        let all = gv::lsd_where(&a[..end], &ide.mix[..end], |_| true);
        let near = gv::lsd_where(&a[..end], &ide.mix[..end], gv::near_join(&joins));
        let j = gv::joins(&a, &ide.mix, &joins, end);
        let proto = PROTO.iter().find(|x| x.0 == c.name).unwrap();
        println!(
            "| {} | {} | {all:.2} / {near:.2} | {:.2} / {:.2} | {:+.1} / {:+.1} | {:+.1} |",
            c.name,
            c.pattern.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(" "),
            proto.1,
            proto.2,
            j.click_mean,
            j.click_max,
            proto.3
        );
        if let Some(d) = &wav {
            write_wav(d, &format!("{}-app", c.name), &a[..(end + (0.4 * SR) as usize).min(len)]);
            write_wav(d, &format!("{}-ideal", c.name), &ide.mix[..(end + (0.4 * SR) as usize).min(len)]);
        }
        assert!((all - proto.1).abs() <= 0.1, "{}: distance {all:.2} dB, the prototype's {:.2}", c.name, proto.1);
        assert!(j.click_max <= 3.0, "{}: a click {:.1} dB over the ideal", c.name, j.click_max);
        assert_eq!(p.takes.len(), STEMS);
    }
}

fn write_wav(dir: &str, name: &str, x: &[f32]) {
    let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-9);
    let mut b: Vec<u8> = Vec::new();
    let n = x.len() as u32;
    for (tag, v) in [(b"RIFF", 36 + n * 2), (b"WAVE", 0), (b"fmt ", 16)] {
        b.extend_from_slice(tag);
        if tag != b"WAVE" {
            b.extend_from_slice(&v.to_le_bytes());
        }
    }
    for v in [1u16, 1] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(&(SR as u32).to_le_bytes());
    b.extend_from_slice(&(SR as u32 * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(n * 2).to_le_bytes());
    for v in x {
        b.extend_from_slice(&(((v / peak) * 0.7 * 32767.0) as i16).to_le_bytes());
    }
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(format!("{dir}/{name}.wav"), b).unwrap();
}

// ---------------------------------------------------------------- the whole engine

/// A v5 track: one looping grid section as long as the case plays, one clip per layer in
/// `layers`, each with stem `stem_of(i)`'s take audio stretched through its map in hits mode.
fn track(c: &Case, layers: &[u32], stem_of: impl Fn(usize) -> usize) -> Vec<u8> {
    let bpm = c.ups * 60.0 / 960.0;
    let maps: Vec<&Vec<(f64, f64, u32)>> = (0..layers.len()).map(|i| &c.maps[stem_of(i)]).collect();
    let total: usize = maps.iter().map(|m| m.len()).sum();
    let mut w: Vec<u32> = vec![TRACK_MAGIC5, 1, layers.len() as u32, 0, total as u32, 1, 0, 0];
    w.extend([FEEL_GRID, (c.section as f32).to_bits(), (bpm as f32).to_bits(), 1, 0, 1, 0, 0]);
    let mut first = 0;
    for (i, &l) in layers.iter().enumerate() {
        let n = maps[i].len() as u32;
        w.extend([0, l, (c.loop_len as f32).to_bits(), 0, 0, u32::MAX, 0, 0, 0, 0, FIRST_SAMPLE + stem_of(i) as u32, 0, 0, first, n, MODE_HITS]);
        first += n;
    }
    for m in &maps {
        for &(u, s, f) in m.iter() {
            w.extend([(s as f32).to_bits(), (u as f32).to_bits(), f]);
        }
    }
    w.push(0);
    w.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn engine(takes: &[Vec<f32>], layers: &[u32], blob: &[u8], block: usize) -> Box<Engine> {
    let mut e = Engine::new(SR, block.max(128));
    let mut v = klont::build::simple_voice(&[P::T_OSC]);
    v.set.push((1, P::amp::GAIN, 0.0));
    let patch = klont::build::patch_with(&[v], &[(P::T_INPUT, false)], &[(0, P::input::MIX, 1.0)], &[]);
    for &l in layers {
        assert_eq!(e.load_bytes(l, LOAD_PATCH, &patch), LOAD_OK);
    }
    for (s, t) in takes.iter().enumerate() {
        let quiet: Vec<f32> = t.iter().map(|x| x * IN_GAIN).collect();
        assert_eq!(e.load_bytes(FIRST_SAMPLE + s as u32, LOAD_SAMPLE, &klont::classic::sample_blob(SR, f32::NAN, &quiet)), LOAD_OK);
    }
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

fn export(takes: &[Vec<f32>], blob: &[u8], n: usize, block: usize) -> Vec<f32> {
    let layers = [0, 1, 2];
    let mut e = engine(takes, &layers, blob, block);
    e.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
    render(&mut e, n, block)
}

fn shuffle() -> (Case, Groove) {
    let c = cases().into_iter().find(|c| c.name == "four-shuffle").unwrap();
    let g = groove(&c.groove);
    (c, g)
}

#[test]
fn the_engine_plays_what_the_stretcher_renders() {
    // Each stem on its own layer: the layer's raw input at mix 1 is the stretcher's output times
    // the layer's gain (0 dB, centre, equal power), then the master's DC blocker and soft clip.
    let (c, g) = shuffle();
    let (perf, _) = gv::performed(&g);
    let takes: Vec<Vec<f32>> = perf.stems.iter().map(|s| take_of(&g, s)).collect();
    let n = (2.2 * c.loop_len / c.ups * SR as f64) as usize;
    for s in 0..STEMS {
        let out = export(&takes, &track(&c, &[0], |_| s), n, 4096);
        let gain = std::f64::consts::FRAC_1_SQRT_2 as f32 * IN_GAIN;
        let soft = |x: f32| {
            let x = x.clamp(-3.0, 3.0);
            (x * (27.0 + x * x) / (27.0 + 9.0 * x * x)).clamp(-1.0, 1.0)
        };
        let mut dc = dsp::filters::DcBlocker::new(SR);
        let want: Vec<f32> = stretch(&c, &takes[s], &c.maps[s], 0, n).iter().map(|&v| soft(dc.process(v * gain))).collect();
        // Play from a stop starts the first slice at full level, where a stretcher seeked to 0 fades
        // it in over its pre-roll (docs/Q2.md), so the first 20 ms are left out.
        let skip = (0.02 * SR) as usize;
        let (e, p) = out[skip..].iter().zip(&want[skip..]).fold((0.0f64, 0.0f64), |(e, p), (a, b)| (e + ((a - b) as f64).powi(2), p + (*b as f64).powi(2)));
        let db = 10.0 * (e / p).log10();
        println!("four-shuffle stem {s}: the engine against the stretcher through the layer gain and the master: {db:.1} dB");
        assert!(db < -80.0, "stem {s}: {db:.1} dB");
    }
}

#[test]
fn renders_do_not_depend_on_calls_or_where_play_started() {
    let (c, g) = shuffle();
    let (perf, _) = gv::performed(&g);
    let takes: Vec<Vec<f32>> = perf.stems.iter().map(|s| take_of(&g, s)).collect();
    let blob = track(&c, &[0, 1, 2], |i| i);
    let n = (3.0 * c.loop_len / c.ups * SR as f64) as usize;
    let whole = export(&takes, &blob, n, 4096);
    assert_eq!(whole, export(&takes, &blob, n, 4096), "two exports");
    assert_eq!(whole, export(&takes, &blob, n, 128), "128-sample calls");
    assert_eq!(whole, export(&takes, &blob, n, 333), "333-sample calls");
    // Play from eight points, on each layer's stretcher (the master's DC blocker starts again at
    // Play, so the engine's output differs for a few ms after any start, remapped or not): from
    // the next slice on, the same samples as the whole render.
    let pass = (c.loop_len * SR as f64 / c.ups).round() as usize;
    for s in 0..STEMS {
        let all = stretch(&c, &takes[s], &c.maps[s], 0, 2 * pass);
        let hard: Vec<usize> = points(&c, &c.maps[s]).iter().filter(|p| p.flags & POINT_HARD != 0).map(|p| p.u.round() as usize).collect();
        for k in 1..9 {
            let start = pass * k / 9 + 777;
            let a = stretch(&c, &takes[s], &c.maps[s], start, pass);
            assert_eq!(a, stretch(&c, &takes[s], &c.maps[s], start, pass), "stem {s}: play from {start} repeats");
            let next = hard.iter().copied().find(|&u| u > start).unwrap_or(pass);
            if let Some(d) = a.iter().zip(&all[start..]).rposition(|(x, y)| x.to_bits() != y.to_bits()) {
                assert!(start + d < next, "stem {s}: play from {start} still differs {d} samples later, past the next slice at {next}");
            }
        }
    }
    println!("four-shuffle, three remapped layers: exports and call sizes bit-identical; plays from eight points match from the next slice on");
}

#[test]
fn live_playback_from_a_stop_against_an_export() {
    let (c, g) = shuffle();
    let (perf, _) = gv::performed(&g);
    let takes: Vec<Vec<f32>> = perf.stems.iter().map(|s| take_of(&g, s)).collect();
    let blob = track(&c, &[0, 1, 2], |i| i);
    let n = (3.0 * SR) as usize;
    let mut live = engine(&takes, &[0, 1, 2], &blob, 128);
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
    let mut ex = engine(&takes, &[0, 1, 2], &blob, 4096);
    render(&mut ex, SR as usize, 4096);
    ex.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
    let el = render(&mut ex, n, 4096);
    let m = ll[..n].iter().zip(&el).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    println!("live playback after a stop against an export: bit-identical {}", ll[..n] == el[..]);
    assert!(m == 0.0, "worst difference {m}");
}

#[test]
fn eight_remapped_layers_play_and_reload_without_allocating() {
    let cs = cases();
    let (a, b) = (cs.iter().find(|c| c.name == "four-shuffle").unwrap(), cs.iter().find(|c| c.name == "four-drop4").unwrap());
    let g = groove("four");
    let (perf, _) = gv::performed(&g);
    let takes: Vec<Vec<f32>> = perf.stems.iter().map(|s| take_of(&g, s)).collect();
    let layers: Vec<u32> = (0..8).collect();
    let ta = track(a, &layers, |i| i % STEMS);
    let tb = track(b, &layers, |i| i % STEMS);
    let mut e = engine(&takes, &layers, &ta, 4096);
    e.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
    ALLOCS.store(0, Ordering::Relaxed);
    ARMED.with(|x| x.set(true));
    // Live calls through loop wraps, the other pattern loaded while playing, a stop and a play,
    // then an export's calls.
    for i in 0..(12.0 * SR as f64) as usize / 128 {
        if i == 1500 {
            e.load_bytes(0, LOAD_TRACK, &tb);
        }
        if i == 3000 {
            e.push(MSG_STOP, 0, 0, 0, 0.0, 0);
        }
        if i == 3100 {
            e.push(MSG_PLAY, 0, 0, 0, 0.0, 0);
        }
        e.process(128);
    }
    for _ in 0..24 {
        e.render(4096);
    }
    ARMED.with(|x| x.set(false));
    assert_eq!(ALLOCS.load(Ordering::Relaxed), 0, "allocations while playing remapped layers");
    assert!(e.output().0.iter().any(|v| v.abs() > 0.001), "the layers play");
    // Every map is folded into its loop and starts each run with a hard run point.
    for c in &cs {
        for m in &c.maps {
            assert!(m.first().is_some_and(|p| p.2 & POINT_RUN != 0) && m.iter().all(|p| p.0 >= 0.0 && p.0 <= c.loop_len + 1e-6), "{}", c.name);
        }
    }
}
