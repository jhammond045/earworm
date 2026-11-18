//! The C ABI the analysis worker calls, round-tripped through raw pointers.

mod common;
use analysis::second::{Options, STRIDE, second_pass};
use analysis::{TUNE_SETTINGS_LEN, an_alloc, an_free, an_result_len, an_second, an_tune, tune_take};
use common::*;

fn put(v: &[f32]) -> *mut f32 {
    let p = an_alloc(v.len().max(1) as u32);
    unsafe { std::slice::from_raw_parts_mut(p, v.len()).copy_from_slice(v) };
    p
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|x| x.to_bits()).collect()
}

#[test]
fn second_pass_through_the_c_abi_matches_the_direct_call() {
    let vs = voices();
    let hum = make_hum(&vs[3], 42);
    let rec = records(&hum.audio, SR);
    let direct = {
        let r = second_pass(&hum.audio, SR, &rec, &Options::default());
        let mut f = Vec::new();
        r.to_floats(&mut f);
        f
    };
    let (a, f, o) = (put(&hum.audio), put(&rec), put(&[0.0]));
    let out = unsafe { an_second(a, hum.audio.len() as u32, SR, f, rec.len() as u32, o, 0) };
    let n = an_result_len() as usize;
    let via = unsafe { std::slice::from_raw_parts(out, n) }.to_vec();
    assert_eq!(bits(&via), bits(&direct));
    assert_eq!(via.len(), 16 + (rec.len() / STRIDE) + 4 * via[12] as usize + 22 * via[13] as usize);
    assert!(via[12] >= 3.0, "a hum with notes gives notes");
    // Options: force a beat read of the same take.
    let mut opts = vec![0.0f32; analysis::second::OPTIONS_LEN];
    opts[0] = 2.0;
    let op = put(&opts);
    let out = unsafe { an_second(a, hum.audio.len() as u32, SR, f, rec.len() as u32, op, opts.len() as u32) };
    let n = an_result_len() as usize;
    let forced = unsafe { std::slice::from_raw_parts(out, n) };
    assert_eq!(forced[0], 1.0);
    assert_eq!(forced[12], 0.0);
    unsafe {
        an_free(a, hum.audio.len() as u32);
        an_free(f, rec.len() as u32);
        an_free(o, 1);
        an_free(op, opts.len() as u32);
    }
}

#[test]
fn tune_through_the_c_abi_matches_tune_curve_and_level_out() {
    let vs = voices();
    let hum = make_hum(&vs[2], 7);
    let rec = records(&hum.audio, SR);
    let n = rec.len() / STRIDE;
    let r = second_pass(&hum.audio, SR, &rec, &Options::default());
    // snap 1, speed 0, glide, vibrato flattened, root 0, major, reference 0, level out 0.5
    let s = [1.0f32, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.5];
    assert_eq!(s.len(), TUNE_SETTINGS_LEN);
    let (f, p, sp) = (put(&rec), put(&r.pitch), put(&s));
    let out = unsafe { an_tune(f, rec.len() as u32, p, sp) };
    assert_eq!(an_result_len() as usize, 2 * n);
    let got = unsafe { std::slice::from_raw_parts(out, 2 * n) }.to_vec();
    assert_eq!(bits(&got), bits(&tune_take(&rec, &r.pitch, &s)));
    let (tuned, loud) = got.split_at(n);
    // Snapped: most voiced frames sit on a whole semitone of C major (the rest are glides).
    let c_major = [0, 2, 4, 5, 7, 9, 11];
    let voiced: Vec<f32> = tuned.iter().copied().filter(|v| v.is_finite()).collect();
    assert!(voiced.len() > n / 4);
    let on = voiced
        .iter()
        .filter(|&&v| {
            let semis = (v / 100.0).round();
            (v - semis * 100.0).abs() < 1.0 && c_major.contains(&(semis as i32).rem_euclid(12))
        })
        .count();
    assert!(on as f32 > 0.7 * voiced.len() as f32, "{on} of {}", voiced.len());
    // Level out 0.5 pulls loudness toward its median: lower spread than the stored curve.
    let stored: Vec<f32> = (0..n).map(|k| rec[k * STRIDE + 2]).collect();
    let spread = |v: &[f32]| {
        let on: Vec<f32> = v.iter().copied().filter(|x| *x > 0.0).collect();
        let m = on.iter().sum::<f32>() / on.len() as f32;
        (on.iter().map(|x| (x - m).powi(2)).sum::<f32>() / on.len() as f32).sqrt()
    };
    assert!(spread(loud) < spread(&stored) + 1e-6);
    assert!(loud.iter().zip(&stored).all(|(l, s)| (*s == 0.0) == (*l == 0.0)));
    unsafe {
        an_free(f, rec.len() as u32);
        an_free(p, n as u32);
        an_free(sp, s.len() as u32);
    }
}

#[test]
fn alloc_and_match_still_work() {
    let p = an_alloc(4800);
    unsafe { std::slice::from_raw_parts_mut(p, 4800).iter_mut().enumerate().for_each(|(i, v)| *v = 0.1 * (i as f32 * 0.05).sin()) };
    let out = unsafe { analysis::an_match(p, 4800, 48_000.0, 16) };
    let m = unsafe { std::slice::from_raw_parts(out, analysis::MATCH_LEN) };
    assert!(m.iter().any(|v| *v != 0.0));
    unsafe { an_free(p, 4800) };
}

/// Second-pass time for 10 s takes (TDD budget: 2 s). Run with
/// `cargo test --release -p analysis --test abi -- --ignored --nocapture`.
/// With `EARWORM_DUMP=<dir>` it also writes each take (`<name>.audio.f32`, `<name>.frames.f32`)
/// so the same takes can be timed in the wasm module.
#[test]
#[ignore]
fn second_pass_time_for_ten_seconds() {
    let vs = voices();
    let mut a = make_hum(&vs[4], 1).audio;
    let b = make_hum(&vs[4], 2).audio;
    a.extend_from_slice(&b);
    a.truncate((10.0 * SR) as usize);
    let mut takes = vec![("hum", a)];
    // A dense beat take: noise bursts every 120 ms for 10 s.
    let mut r = Rng::new(3);
    let mut beat = vec![0.0f32; (10.0 * SR) as usize];
    for i in 0..beat.len() {
        beat[i] = 0.0005 * r.noise();
    }
    for k in 0..80 {
        let t0 = (0.2 + k as f32 * 0.12 * 1.0) * SR;
        for i in 0..(0.05 * SR) as usize {
            let idx = t0 as usize + i;
            if idx < beat.len() {
                beat[idx] += 0.3 * r.noise() * (-(i as f32) / (0.012 * SR)).exp();
            }
        }
    }
    takes.push(("beat", beat));
    for (name, audio) in takes {
        let rec = records(&audio, SR);
        let t = std::time::Instant::now();
        let res = second_pass(&audio, SR, &rec, &Options::default());
        println!("{name}: {:.0} ms for {:.1} s ({:?}, {} notes, {} hits)", t.elapsed().as_secs_f32() * 1000.0, audio.len() as f32 / SR, res.kind, res.notes.len(), res.hits.len());
        if let Ok(dir) = std::env::var("EARWORM_DUMP") {
            let bytes = |v: &[f32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
            std::fs::write(format!("{dir}/{name}.audio.f32"), bytes(&audio)).unwrap();
            std::fs::write(format!("{dir}/{name}.frames.f32"), bytes(&rec)).unwrap();
        }
    }
}
