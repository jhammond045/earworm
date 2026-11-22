//! Stage P0: the same detectors on real, labelled recordings.
//!
//!   cargo run --release -- real <dir> [--profile guitar|piano|generic]
//!
//! `<dir>` holds 48 kHz WAV files, each with a note list beside it
//! (`<name>.notes.tsv`: onset s, offset s, MIDI pitch, velocity or -1),
//! as tools/prep.py writes them. A `<name>.bp.tsv` beside them (Basic Pitch's
//! notes, from tools/run_basic_pitch.py) is scored as one more offline row.
//!
//! POLY_THREADS sets how many runs go at once (default 6; CPU figures are
//! only clean at 1). POLY_SECONDS keeps only the first so many seconds of
//! each file. POLY_FILES keeps only names containing one of its
//! comma-separated parts.

use crate::eval::{Metrics, percentile};
use crate::guitar::{Note, SR};
use crate::spectrum::midi_hz;
use crate::track::Est;
use crate::{CONFIGS, GENERIC, GUITAR, PIANO, Profile, run, tables};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Excerpt {
    name: String,
    sig: Vec<f32>,
    truth: Vec<Note>,
    group: &'static str,
    len: f64,
    /// Basic Pitch's notes and its inference time, if it was run.
    bp: Option<(Vec<Est>, f64)>,
}

/// Mono samples from a 16-bit PCM or 32-bit float WAV, channels averaged.
fn load_wav(path: &str) -> Vec<f32> {
    let b = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    assert!(&b[0..4] == b"RIFF" && &b[8..12] == b"WAVE", "{path}: not a WAV file");
    let (mut fmt, mut ch, mut bits, mut sr) = (0u16, 1usize, 16u16, 0u32);
    let mut i = 12;
    while i + 8 <= b.len() {
        let id = &b[i..i + 4];
        let size = u32::from_le_bytes(b[i + 4..i + 8].try_into().unwrap()) as usize;
        let body = &b[i + 8..(i + 8 + size).min(b.len())];
        if id == b"fmt " {
            fmt = u16::from_le_bytes([body[0], body[1]]);
            ch = u16::from_le_bytes([body[2], body[3]]) as usize;
            sr = u32::from_le_bytes(body[4..8].try_into().unwrap());
            bits = u16::from_le_bytes([body[14], body[15]]);
            if fmt == 0xFFFE {
                fmt = u16::from_le_bytes([body[24], body[25]]);
            }
        } else if id == b"data" {
            assert_eq!(sr as f64, SR, "{path}: {sr} Hz; tools/prep.py resamples to 48 kHz");
            let frame: Vec<f32> = match (fmt, bits) {
                (1, 16) => body.chunks_exact(2).map(|x| i16::from_le_bytes([x[0], x[1]]) as f32 / 32768.0).collect(),
                (3, 32) => body.chunks_exact(4).map(|x| f32::from_le_bytes(x.try_into().unwrap())).collect(),
                _ => panic!("{path}: format {fmt}, {bits} bits; only 16-bit PCM and 32-bit float"),
            };
            return frame.chunks_exact(ch).map(|c| c.iter().sum::<f32>() / ch as f32).collect();
        }
        i += 8 + size + (size & 1);
    }
    panic!("{path}: no data chunk");
}

/// Lines of tab-separated numbers.
fn load_tsv(path: &str) -> Vec<Vec<f64>> {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{path}: {e}"))
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| l.split('\t').map(|x| x.trim().parse().unwrap_or(f64::NAN)).collect())
        .collect()
}

fn load(dir: &str, limit: f64) -> Vec<Excerpt> {
    let filter = std::env::var("POLY_FILES").ok();
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{dir}: {e}"))
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".wav").map(String::from))
        .filter(|n| filter.as_ref().is_none_or(|f| f.split(',').any(|p| n.contains(p))))
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|name| {
            let mut sig = load_wav(&format!("{dir}/{name}.wav"));
            sig.truncate((limit * SR) as usize);
            // The live tier's 60 Hz high-pass, which the design puts in front
            // of the detector. The synthetic guitar has no rumble to remove.
            let mut hp = dsp::filters::HighPass::new(SR as f32, 60.0);
            for x in sig.iter_mut() {
                *x = hp.process(*x);
            }
            let len = sig.len() as f64 / SR;
            let mut truth: Vec<Note> = load_tsv(&format!("{dir}/{name}.notes.tsv"))
                .iter()
                .filter(|r| r[0] < len)
                .map(|r| Note {
                    midi: r[2].round() as u8,
                    hz: midi_hz(r[2] as f32) as f64,
                    on: r[0],
                    off: r[1].min(len),
                    vel_db: if r.len() > 3 && r[3] > 0.0 { 40.0 * (r[3] / 127.0).log10() } else { f64::NAN },
                })
                .collect();
            truth.sort_by(|a, b| a.on.total_cmp(&b.on));
            let bp_path = format!("{dir}/{name}.bp.tsv");
            let bp = std::path::Path::new(&bp_path).exists().then(|| {
                let rows = load_tsv(&bp_path);
                let secs: f64 = std::fs::read_to_string(format!("{dir}/{name}.bp.secs"))
                    .ok()
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(f64::NAN);
                let est = rows
                    .iter()
                    .filter(|r| r[0] < len)
                    .map(|r| Est {
                        midi: r[2].round() as u8,
                        on: r[0],
                        first: r[0],
                        off: r[1],
                        last: r[1],
                        level: r[3] as f32,
                        cents: f32::NAN,
                    })
                    .collect();
                (est, secs)
            });
            let group = if name.contains("_comp") {
                "comp"
            } else if name.contains("_solo") {
                "solo"
            } else {
                "all"
            };
            Excerpt { name, sig, truth, group, len, bp }
        })
        .collect()
}

/// Per played note: how many notes were struck with it (within 50 ms of the
/// group's first, as `eval` counts chord size), and whether the same pitch
/// was still sounding just before it (a re-strike).
fn classes(truth: &[Note]) -> Vec<(usize, bool)> {
    let mut size = vec![1usize; truth.len()];
    let mut g = 0;
    for i in 1..=truth.len() {
        if i == truth.len() || truth[i].on - truth[g].on >= 0.05 {
            for s in size[g..i].iter_mut() {
                *s = i - g;
            }
            g = i;
        }
    }
    truth
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let restruck = truth.iter().any(|u| u.midi == t.midi && u.on < t.on - 0.02 && u.off > t.on - 0.1);
            (size[i], restruck)
        })
        .collect()
}

/// Distinct pitches sounding every 10 ms, over frames where anything sounds.
fn polyphony(ex: &[Excerpt]) -> Vec<usize> {
    let mut v = vec![];
    for e in ex {
        for s in 0..(e.len / 0.01) as usize {
            let t = s as f64 * 0.01;
            let mask = e.truth.iter().filter(|u| u.on <= t && t < u.off).fold(0u128, |m, u| m | 1u128 << u.midi);
            if mask != 0 {
                v.push(mask.count_ones() as usize);
            }
        }
    }
    v
}

pub fn main(args: &[String]) {
    let dir = args.first().expect("real <dir> [--profile guitar|piano|generic]");
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let prof: Profile = match opt("--profile").as_deref() {
        None | Some("guitar") => GUITAR,
        Some("piano") => PIANO,
        Some("generic") => GENERIC,
        Some(p) => panic!("unknown profile {p}"),
    };
    let limit = std::env::var("POLY_SECONDS").ok().and_then(|v| v.parse().ok()).unwrap_or(f64::INFINITY);
    let threads = std::env::var("POLY_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or(6usize);
    let ex = load(dir, limit);
    let secs: f64 = ex.iter().map(|e| e.len).sum();
    let notes: usize = ex.iter().map(|e| e.truth.len()).sum();
    println!(
        "## {dir}\n\n{} files, {:.1} min, {notes} notes; profile {} (MIDI {} to {}, up to {} Hz, {} notes at once)",
        ex.len(),
        secs / 60.0,
        prof.name,
        prof.lo,
        prof.hi,
        prof.f_max,
        crate::knob("MAX_NOTES", prof.max_notes as f32)
    );
    let mut poly = polyphony(&ex);
    poly.sort();
    let share = |k: usize| poly.iter().filter(|&&n| n > k).count() as f64 / poly.len().max(1) as f64;
    let pct = |q: f64| poly.get(((poly.len().max(1) - 1) as f64 * q) as usize).copied().unwrap_or(0);
    println!(
        "Distinct pitches sounding (labels, per 10 ms with any): median {}, p90 {}, p99 {}, max {}; more than 6 in {:.1}% of frames, more than 8 in {:.1}%, more than 10 in {:.1}%",
        pct(0.5),
        pct(0.9),
        pct(0.99),
        poly.last().copied().unwrap_or(0),
        100.0 * share(6),
        100.0 * share(8),
        100.0 * share(10)
    );

    // Every (file, detector) pair is one job.
    let only: Option<Vec<usize>> = std::env::var("POLY_ONLY").ok().map(|o| o.split(',').filter_map(|x| x.parse().ok()).collect());
    let cfgs: Vec<usize> = (0..CONFIGS.len()).filter(|c| only.as_ref().is_none_or(|o| o.contains(c))).collect();
    let jobs: Vec<(usize, usize)> = (0..ex.len()).flat_map(|f| cfgs.iter().map(move |&c| (f, c))).collect();
    let next = AtomicUsize::new(0);
    let out: Mutex<Vec<Option<(Metrics, f64)>>> = Mutex::new((0..jobs.len()).map(|_| None).collect());
    std::thread::scope(|s| {
        for _ in 0..threads.max(1) {
            s.spawn(|| {
                loop {
                    let j = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&(f, c)) = jobs.get(j) else { break };
                    let e = &ex[f];
                    let r = run(&CONFIGS[c], &prof, &e.sig, &e.truth, &[(0.0, e.group)], e.len);
                    out.lock().unwrap()[j] = Some((r.m, r.us_per_block));
                }
            });
        }
    });
    let out: Vec<(Metrics, f64)> = out.into_inner().unwrap().into_iter().map(|r| r.unwrap()).collect();

    let mut names: Vec<String> = cfgs.iter().map(|&c| CONFIGS[c].name.to_string()).collect();
    let mut live: Vec<bool> = cfgs.iter().map(|&c| CONFIGS[c].live).collect();
    let mut totals: Vec<Metrics> = cfgs.iter().map(|_| Metrics::default()).collect();
    let mut cpu = vec![0.0; cfgs.len()];
    // Per detector: (played, found) for fresh chord notes, re-struck notes, and notes struck alone or in pairs.
    let mut by_class = vec![[(0usize, 0usize); 3]; cfgs.len() + 1];
    let mut tally = |row: usize, e: &Excerpt, hit: &[bool]| {
        for ((size, restruck), &h) in classes(&e.truth).into_iter().zip(hit) {
            let k = if restruck {
                1
            } else if size >= 3 {
                0
            } else {
                2
            };
            by_class[row][k].0 += 1;
            by_class[row][k].1 += h as usize;
        }
    };
    for (j, &(f, c)) in jobs.iter().enumerate() {
        let ci = cfgs.iter().position(|&x| x == c).unwrap();
        let (m, us) = &out[j];
        tally(ci, &ex[f], &m.hit);
        totals[ci].add(m);
        cpu[ci] += us * ex[f].len / secs;
    }
    if ex.iter().all(|e| e.bp.is_some()) {
        let mut m_all = Metrics::default();
        let mut bp_secs = 0.0;
        for e in &ex {
            let (est, t) = e.bp.as_ref().unwrap();
            let m = crate::eval::score(&e.truth, est, false, (0.05, 0.05), &[(0.0, e.group)], e.len);
            tally(cfgs.len(), e, &m.hit);
            m_all.add(&m);
            bp_secs += t;
        }
        names.push("Basic Pitch, offline".into());
        live.push(false);
        totals.push(m_all);
        cpu.push(bp_secs * 1e6 / (secs * SR / 128.0));
    } else {
        by_class.pop();
    }
    let name_refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
    let bands = [(21, 40, "A0 to D#2"), (40, 52, "E2 to D#3"), (52, 64, "E3 to D#4"), (64, 76, "E4 to D#5"), (76, 128, "E5 and up")];
    tables(prof.name, &name_refs, &live, &totals, &cpu, &bands);

    println!("\n### Recall by kind of note ({})\n", prof.name);
    println!("Fresh chord notes: struck with two or more others, pitch not already sounding (the nearest thing to the synthetic single strums). Re-struck: the same pitch was sounding until just before.\n");
    println!("| Detector | Fresh chord notes | Re-struck notes | Alone or in pairs |");
    println!("| --- | --- | --- | --- |");
    for (ci, row) in by_class.iter().enumerate() {
        let cells: Vec<String> =
            row.iter().map(|&(n, f)| if n == 0 { "-".into() } else { format!("{:.2} ({f}/{n})", f as f64 / n as f64) }).collect();
        println!("| {} | {} |", names[ci], cells.join(" | "));
    }

    println!("\n### Where the false notes are ({})\n", prof.name);
    println!("Precision of tracked notes by register, and the share of false notes shorter than 100 ms.\n");
    println!("| Detector | {} | False notes under 100 ms |", bands.iter().map(|b| b.2).collect::<Vec<_>>().join(" | "));
    println!("| --- |{} --- |", " --- |".repeat(bands.len()));
    for (ci, m) in totals.iter().enumerate() {
        let cells: Vec<String> = bands
            .iter()
            .map(|&(lo, hi, _)| {
                let v: Vec<&(u8, bool, f64)> = m.tracked.iter().filter(|x| x.0 >= lo && x.0 < hi).collect();
                let found = v.iter().filter(|x| x.1).count();
                if v.is_empty() { "-".into() } else { format!("{:.2} ({found}/{})", found as f64 / v.len() as f64, v.len()) }
            })
            .collect();
        let fp: Vec<&(u8, bool, f64)> = m.tracked.iter().filter(|x| !x.1).collect();
        let short = fp.iter().filter(|x| x.2 < 0.1).count() as f64 / fp.len().max(1) as f64;
        println!("| {} | {} | {:.2} |", names[ci], cells.join(" | "), short);
    }

    println!("\n### F1 per file, NMF live ({})\n", prof.name);
    if let Some(ci) = cfgs.iter().position(|&c| c == 2) {
        let mut rows: Vec<String> = vec![];
        for (j, &(f, c)) in jobs.iter().enumerate() {
            if cfgs[ci] == c {
                let m = &out[j].0;
                rows.push(format!("{} {:.2} (P {:.2} R {:.2}, n {})", ex[f].name, m.f1(), m.precision(), m.recall(), m.n_ref));
            }
        }
        let f1s: Vec<f64> =
            jobs.iter().enumerate().filter(|(_, (_, c))| *c == cfgs[ci]).map(|(j, _)| out[j].0.f1()).collect();
        println!(
            "median {:.3}, p10 {:.3}, p90 {:.3} over {} files",
            percentile(&f1s, 0.5),
            percentile(&f1s, 0.1),
            percentile(&f1s, 0.9),
            f1s.len()
        );
        if ex.len() <= 20 {
            for r in rows {
                println!("- {r}");
            }
        }
    }
}
