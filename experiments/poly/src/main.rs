//! Polyphonic pitch capture: the feasibility experiment behind docs/poly.md.
//! Throwaway code, not wired into the app.
//!
//! Renders a synthetic guitar (acoustic and electric) playing single chords,
//! a strummed progression, arpeggios, a line with double stops and
//! palm-muted chugs; runs two multi-pitch detectors with one note tracker
//! over it; and prints the tables in docs/poly.md.
//!
//!   (cd experiments/poly && cargo run --release -- test)
//!   (cd experiments/poly && cargo run --release -- dev --wav <dir>)
//!
//! `dev` is the material the thresholds were tuned on; `test` is held out
//! (other chords, other lines, other random draws). `--wav <dir>` also
//! writes the rendered guitar; `--svg <dir>` draws piano rolls of the
//! strummed passage (poly-roll-hs.svg, poly-roll-nmf.svg).

mod eval;
mod guitar;
mod hs;
mod nmf;
mod real;
mod spectrum;
mod track;

use eval::{Metrics, pearson, percentile};
use guitar::{Kind, Note, SR, Set};
use hs::{Det, Hs, HsParams};
use nmf::{Nmf, NmfParams};
use spectrum::{Flux, Stft, Window, midi_hz, note_level, octave_check};
use std::time::Instant;
use track::{Est, TrackParams, Tracker};

#[derive(Clone, Copy, PartialEq)]
enum Method {
    Hs,
    Nmf,
}

#[derive(Clone, Copy)]
struct Config {
    name: &'static str,
    method: Method,
    n: usize,
    window: Window,
    live: bool,
    /// Strum mode: an onset restarts every sounding note whose level rose.
    strum: bool,
}

/// 5 ms, as the live tier's pitch hop.
const HOP: usize = 240;
/// Frames whose strongest bin is under -60 dBFS are not analysed.
const GATE: f32 = 1e-3;

const CONFIGS: [Config; 6] = [
    Config { name: "HS live, 4096 asym", method: Method::Hs, n: 4096, window: Window::Asym, live: true, strum: false },
    Config { name: "HS live, 2048 asym", method: Method::Hs, n: 2048, window: Window::Asym, live: true, strum: false },
    Config { name: "NMF live, 4096 asym", method: Method::Nmf, n: 4096, window: Window::Asym, live: true, strum: false },
    Config { name: "NMF live, 4096 asym, strum mode", method: Method::Nmf, n: 4096, window: Window::Asym, live: true, strum: true },
    Config { name: "HS offline, 8192 Hann", method: Method::Hs, n: 8192, window: Window::Hann, live: false, strum: false },
    Config { name: "NMF offline, 8192 Hann", method: Method::Nmf, n: 8192, window: Window::Hann, live: false, strum: false },
];

/// A tuning value, overridable from the environment (POLY_<NAME>) while tuning on dev.
fn knob(name: &str, default: f32) -> f32 {
    std::env::var(format!("POLY_{name}")).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// What the detectors assume about the instrument: the dictionary's range,
/// how far up its templates reach, their inharmonicity, how many notes the
/// tracker holds at once, how many broadband atoms NMF has beside the notes,
/// and the onset gate (a note may only start within `gate` times the
/// retrigger window of a broadband onset; 0 turns it off).
#[derive(Clone, Copy)]
pub struct Profile {
    pub name: &'static str,
    pub lo: u8,
    pub hi: u8,
    pub f_max: f32,
    pub b: fn(u8) -> f32,
    pub max_notes: usize,
    pub noise: usize,
    pub gate: f32,
}

/// E2 to E6, a steel string's inharmonicity, six strings. The noise atoms
/// and the onset gate were added after stage P0, tuned on GuitarSet's
/// player 00: real recordings put a body's thump, rumble and room sound on
/// the lowest notes without them. POLY_NOISE=0 POLY_OPEN_GATE=0 gives the
/// detector the synthetic figures in docs/poly.md were first measured with.
pub const GUITAR: Profile =
    Profile { name: "guitar", lo: 40, hi: 88, f_max: 5000.0, b: |_| 5e-5, max_notes: 6, noise: 8, gate: 2.0 };

/// A0 to C8, piano inharmonicity, up to 8 kHz so the top octave keeps a
/// second partial, ten notes. B follows the usual two-slope shape: lowest
/// around the bass break, rising steeply up the treble (about 4e-4 at C4,
/// 2e-2 at C8). No onset gate: in fast passages the onset detector, tuned
/// on guitar, misses too many key strikes. Tuned on two MAESTRO pieces.
pub const PIANO: Profile =
    Profile { name: "piano", lo: 21, hi: 108, f_max: 8000.0, b: piano_b, max_notes: 10, noise: 4, gate: 0.0 };

/// A1 to C7 with harmonic templates (B = 0), for bowed strings, winds,
/// voices and mixed ensembles when nothing is known about the instrument.
/// No onset gate: bowed and blown notes can start without one.
pub const GENERIC: Profile =
    Profile { name: "generic", lo: 33, hi: 96, f_max: 5000.0, b: |_| 0.0, max_notes: 10, noise: 4, gate: 0.0 };

fn piano_b(m: u8) -> f32 {
    let m = m as f32;
    4e-4 * (0.088 * (m - 60.0)).exp() + 1e-4 * (-0.06 * (m - 21.0)).exp()
}

fn track_params(cfg: &Config, prof: &Profile) -> TrackParams {
    TrackParams {
        min_on: knob("MIN_ON", 2.0) as u32,
        min_off: knob("MIN_OFF", 10.0) as u32,
        retrig: knob("RETRIG", 1.2),
        // About as long as the window takes to fill with a new pluck.
        retrig_window: knob("RETRIG_WINDOW", (cfg.n / 340) as f32) as u32,
        strum_rise: if cfg.strum { knob("STRUM", 1.05) } else { 0.0 },
        // NMF reports weak pitches too, so that a sounding note is not lost
        // between frames; it takes a stronger one to start a note.
        open_strength: if cfg.method == Method::Nmf { knob("OPEN", 0.08) } else { 0.0 },
        max_notes: knob("MAX_NOTES", prof.max_notes as f32) as usize,
        open_gate: (knob("OPEN_GATE", prof.gate) * (cfg.n / 340) as f32) as u32,
    }
}

fn hs_params(prof: &Profile) -> HsParams {
    let max_notes = knob("MAX_NOTES", prof.max_notes as f32) as usize;
    HsParams { gamma: knob("GAMMA", 0.7), rel_floor: knob("FLOOR", 0.15), max_notes, nu: knob("NU", 0.33), lo: prof.lo, hi: prof.hi }
}

fn nmf_params(prof: &Profile) -> NmfParams {
    NmfParams {
        pw: knob("PW", 0.6),
        lambda: knob("LAMBDA", 0.1),
        iters: knob("ITERS", 10.0) as usize,
        share: knob("SHARE", 0.04),
        max_notes: knob("MAX_NOTES", prof.max_notes as f32) as usize,
        lo: prof.lo,
        hi: prof.hi,
        f_max: prof.f_max,
        b: prof.b,
        noise: knob("NOISE", prof.noise as f32) as usize,
    }
}

enum Detector {
    Hs(Hs),
    Nmf(Nmf),
}

struct Run {
    m: Metrics,
    us_per_block: f64,
    est: Vec<Est>,
}

fn run(cfg: &Config, prof: &Profile, sig: &[f32], truth: &[Note], sections: &[(f64, &'static str)], len: f64) -> Run {
    let mut stft = Stft::new(cfg.n, SR as f32, cfg.window);
    let mut det = match cfg.method {
        Method::Hs => Detector::Hs(Hs::new(&stft, hs_params(prof))),
        Method::Nmf => Detector::Nmf(Nmf::new(&stft, nmf_params(prof))),
    };
    let mut tracker = Tracker::new(track_params(cfg, prof));
    // Broadband onsets come from a short window, which sees a pluck within
    // a few ms; the long window's spectrum takes tens of ms to show it.
    let mut short = Stft::new(1024, SR as f32, Window::Hann);
    let mut flux = Flux::new((5000.0 / short.bin_hz) as usize);
    let mut onsets: Vec<f64> = vec![];
    let mut dets: Vec<Det> = Vec::with_capacity(16);
    let (lv_from, lv_to) = (knob("LV_FROM", 2.0) as usize, knob("LV_TO", 10.0) as usize);
    let (oct_keep, oct_lift) = (knob("OCT_KEEP", 0.0), knob("OCT_LIFT", 0.0));
    let t0 = Instant::now();
    let mut end = HOP;
    while end <= sig.len() {
        stft.frame(sig, end);
        short.frame(sig, end);
        let peak = stft.mag.iter().cloned().fold(0.0f32, f32::max);
        if peak > GATE {
            match &mut det {
                Detector::Hs(h) => h.detect(&stft, &mut dets),
                Detector::Nmf(n) => n.detect(&stft, &mut dets),
            }
        } else {
            dets.clear();
            if let Detector::Nmf(n) = &mut det {
                n.reset();
            }
        }
        octave_check(&stft.mag, stft.bin_hz, &mut dets, oct_keep, oct_lift);
        for d in dets.iter_mut() {
            d.level = note_level(&stft.mag, stft.bin_hz, midi_hz(d.midi as f32), lv_from, lv_to);
        }
        let onset = flux.step(&short.mag);
        let t = end as f64 / SR;
        if onset {
            onsets.push(t - short.centre_s as f64);
        }
        tracker.step(t, &dets, onset);
        end += HOP;
    }
    tracker.finish(end as f64 / SR);
    let secs = t0.elapsed().as_secs_f64();
    let us_per_block = secs * 1e6 / (sig.len() as f64 / 128.0);
    let mut est = tracker.notes;
    if !cfg.live {
        // Offline: each note starts at the latest broadband onset in the
        // 250 ms before it was first seen (the second pass refines onsets the
        // same way); ends where it was last seen, less the window's centre.
        let c = stft.centre_s as f64;
        for e in est.iter_mut() {
            let snap = onsets.iter().rev().find(|&&o| o <= e.first && o >= e.first - 0.25);
            e.first = snap.copied().unwrap_or(e.first - c);
            e.last -= c;
        }
    }
    if let Ok(range) = std::env::var("POLY_DEBUG") {
        let v: Vec<f64> = range.split(',').filter_map(|x| x.parse().ok()).collect();
        let (a, b) = (v[0], v[1]);
        eprintln!("--- {} {:.2}..{:.2}", cfg.name, a, b);
        for t in truth.iter().filter(|t| t.on >= a && t.on < b) {
            eprintln!("  truth {:>3} on {:.3} off {:.3} vel {:.1}", t.midi, t.on, t.off, t.vel_db);
        }
        for e in est.iter().filter(|e| e.first >= a - 0.1 && e.first < b) {
            eprintln!("  est   {:>3} on {:.3} first {:.3} off {:.3} level {:.4}", e.midi, e.on, e.first, e.off, e.level);
        }
        for o in onsets.iter().filter(|&&o| o >= a && o < b) {
            eprintln!("  onset {:.3}", o);
        }
    }
    // Live: from 20 ms before to 100 ms after the onset, so the latency counts.
    // Offline: 50 ms either way.
    let window = if cfg.live { (0.02, 0.10) } else { (0.05, 0.05) };
    let m = eval::score(truth, &est, cfg.live, window, sections, len);
    Run { m, us_per_block, est }
}

fn write_wav(path: &str, sig: &[f32]) {
    let n = sig.len() as u32;
    let mut b: Vec<u8> = Vec::with_capacity(44 + 2 * sig.len());
    b.extend(b"RIFF");
    b.extend((36 + 2 * n).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend((SR as u32).to_le_bytes());
    b.extend((SR as u32 * 2).to_le_bytes());
    b.extend(2u16.to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(b"data");
    b.extend((2 * n).to_le_bytes());
    for &x in sig {
        b.extend(((x.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, b).expect("write wav");
}

fn ms(x: f64) -> String {
    if x.is_nan() { "-".into() } else { format!("{:.0}", x * 1000.0) }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "real") {
        return real::main(&args[2..]);
    }
    let set = if args.iter().any(|a| a == "dev") { Set::Dev } else { Set::Test };
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let wav_dir = opt("--wav");
    let svg = opt("--svg");
    let seed = if set == Set::Dev { 1 } else { 2 };

    let kinds: Vec<Kind> = match std::env::var("POLY_KIND").as_deref() {
        Ok("acoustic") => vec![Kind::Acoustic],
        Ok("electric") => vec![Kind::Electric],
        _ => vec![Kind::Acoustic, Kind::Electric],
    };
    let sc = guitar::score(set, seed);
    let mut totals: Vec<Metrics> = CONFIGS.iter().map(|_| Metrics::default()).collect();
    let mut cpu = vec![0.0; CONFIGS.len()];
    let mut per_kind = vec![];
    println!("## {:?} set\n", set);
    for (ki, &kind) in kinds.iter().enumerate() {
        let (sig, truth) = guitar::render(&sc, kind, seed * 10 + ki as u64);
        let sizes: Vec<usize> = (2..=6).map(|k| count_chords_of_size(&truth, k)).collect();
        println!(
            "{:?}: {:.1} s, {} notes; onsets within 50 ms of each other grouped: {:?} groups of 2..6",
            kind,
            sc.len,
            truth.len(),
            sizes
        );
        if let Some(dir) = &wav_dir {
            write_wav(&format!("{dir}/{:?}-{:?}.wav", set, kind).to_lowercase(), &sig);
        }
        for (ci, cfg) in CONFIGS.iter().enumerate() {
            if std::env::var("POLY_ONLY").is_ok_and(|o| !o.split(',').any(|x| x.parse() == Ok(ci))) {
                continue;
            }
            let r = run(cfg, &GUITAR, &sig, &truth, &sc.sections, sc.len);
            per_kind.push((kind, cfg.name, r.m.f1(), r.m.precision(), r.m.recall()));
            totals[ci].add(&r.m);
            cpu[ci] += r.us_per_block / kinds.len() as f64;
            if let (Some(dir), Kind::Acoustic, true) = (&svg, kind, ci == 0 || ci == 2) {
                let name = if ci == 0 { "hs" } else { "nmf" };
                piano_roll(&format!("{dir}/poly-roll-{name}.svg"), &truth, &r.est, cfg, &sc.sections);
            }
        }
    }

    let names: Vec<&str> = CONFIGS.iter().map(|c| c.name).collect();
    let live: Vec<bool> = CONFIGS.iter().map(|c| c.live).collect();
    tables("both instruments", &names, &live, &totals, &cpu, &[(40, 52, "E2 to D#3"), (52, 64, "E3 to D#4"), (64, 128, "E4 and up")]);

    println!("\n### F1 by instrument\n");
    println!("| Detector | Acoustic F1 (P / R) | Electric F1 (P / R) |");
    println!("| --- | --- | --- |");
    for cfg in &CONFIGS {
        let cell = |k: Kind| {
            match per_kind.iter().find(|x| x.0 == k && x.1 == cfg.name) {
                Some((_, _, f, p, r)) => format!("{f:.3} ({p:.3} / {r:.3})"),
                None => "-".into(),
            }
        };
        println!("| {} | {} | {} |", cfg.name, cell(Kind::Acoustic), cell(Kind::Electric));
    }
}

/// Chords in the truth: onsets within 50 ms of each other, counted by size.
fn count_chords_of_size(truth: &[Note], k: usize) -> usize {
    let mut groups = vec![];
    let mut cur = vec![truth[0].on];
    for n in &truth[1..] {
        if n.on - cur[0] < 0.05 {
            cur.push(n.on);
        } else {
            groups.push(cur.len());
            cur = vec![n.on];
        }
    }
    groups.push(cur.len());
    groups.iter().filter(|&&g| g == k).count()
}

/// Piano roll of the strummed section: played notes as grey bars, tracked
/// notes over them (green found, red false), missed notes outlined red.
fn piano_roll(path: &str, truth: &[Note], est: &[Est], cfg: &Config, sections: &[(f64, &'static str)]) {
    let t0 = sections.iter().find(|s| s.1 == "strummed").unwrap().0;
    let t1 = t0 + 4.8;
    let (lo, hi) = (38u8, 80u8);
    let (w, h) = (960.0, 12.0 * (hi - lo) as f64 + 40.0);
    let x = |t: f64| 40.0 + (t - t0) / (t1 - t0) * (w - 50.0);
    let y = |m: u8| 10.0 + (hi - m) as f64 * 12.0;
    let on = |e: &Est| if cfg.live { e.on } else { e.first };
    let off = |e: &Est| if cfg.live { e.off } else { e.last };
    let mut s = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {w} {h}\" font-family=\"sans-serif\" font-size=\"10\">\n<rect width=\"{w}\" height=\"{h}\" fill=\"#fff\"/>\n"
    );
    for m in lo..=hi {
        if [40, 52, 64, 76].contains(&m) {
            let name = ["E2", "E3", "E4", "E5"][((m - 40) / 12) as usize];
            s += &format!("<text x=\"4\" y=\"{}\" fill=\"#555\">{name}</text>\n", y(m) + 9.0);
            s += &format!("<line x1=\"40\" x2=\"{}\" y1=\"{}\" y2=\"{}\" stroke=\"#ddd\"/>\n", w - 10.0, y(m) + 12.0, y(m) + 12.0);
        }
    }
    let mut found = vec![false; truth.len()];
    let mut good = vec![false; est.len()];
    for (i, t) in truth.iter().enumerate() {
        for (j, e) in est.iter().enumerate() {
            let d = on(e) - t.on;
            let ok = if cfg.live { (-0.02..=0.1).contains(&d) } else { d.abs() <= 0.05 };
            if e.midi == t.midi && ok && !good[j] {
                found[i] = true;
                good[j] = true;
                break;
            }
        }
    }
    for (i, t) in truth.iter().enumerate() {
        if t.off < t0 || t.on > t1 {
            continue;
        }
        let (a, b) = (x(t.on.max(t0)), x(t.off.min(t1)));
        let stroke = if found[i] { "none" } else { "#d22" };
        s += &format!(
            "<rect x=\"{a:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"10\" fill=\"#ccc\" stroke=\"{stroke}\" stroke-width=\"1.5\"/>\n",
            y(t.midi),
            (b - a).max(1.0)
        );
    }
    for (j, e) in est.iter().enumerate() {
        if off(e) < t0 || on(e) > t1 {
            continue;
        }
        let (a, b) = (x(on(e).max(t0)), x(off(e).min(t1)));
        let fill = if good[j] { "#2a2" } else { "#d22" };
        s += &format!(
            "<rect x=\"{a:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"4\" fill=\"{fill}\"/>\n",
            y(e.midi) + 3.0,
            (b - a).max(1.0)
        );
    }
    s += &format!(
        "<text x=\"40\" y=\"{}\" fill=\"#333\">{}: grey = played, green = tracked and found, red bar = false note, red outline = missed. {:.1} s shown.</text>\n</svg>\n",
        h - 8.0,
        cfg.name,
        t1 - t0
    );
    std::fs::write(path, s).expect("write svg");
}

/// The tables docs/poly.md quotes, for any set of runs. `bands` splits live
/// latency by register: lowest MIDI note, one past the highest, label.
pub fn tables(what: &str, names: &[&str], live: &[bool], totals: &[Metrics], cpu: &[f64], bands: &[(u8, u8, &str)]) {
    println!("\n### Notes ({what})\n");
    println!("| Detector | Precision | Recall | F1 | Latency median / p90 ms | Frame F1 | Velocity r | Pitch error median / p90 cents | CPU us per 128-sample block |");
    println!("| --- | --- | --- | --- | --- | --- | --- | --- | --- |");
    for ci in 0..names.len() {
        let m = &totals[ci];
        let lat = format!(
            "{}{} / {}",
            if live[ci] { "" } else { "offline " },
            ms(percentile(&m.latency, 0.5)),
            ms(percentile(&m.latency, 0.9))
        );
        println!(
            "| {} | {:.3} | {:.3} | {:.3} | {} | {:.3} | {:.2} | {:.1} / {:.1} | {:.1} |",
            names[ci],
            m.precision(),
            m.recall(),
            m.f1(),
            lat,
            m.frame_f1(),
            pearson(&m.vel),
            percentile(&m.cents, 0.5),
            percentile(&m.cents, 0.9),
            cpu[ci]
        );
    }

    println!("\n### Errors ({what})\n");
    println!("| Detector | Notes played | Tracked | Found | False: octave | harmonic | semitone | repeat | other | Missed: octave-shadowed | other |");
    println!("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |");
    for ci in 0..names.len() {
        let m = &totals[ci];
        println!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            names[ci],
            m.n_ref,
            m.n_est,
            m.tp,
            m.fp_octave,
            m.fp_harmonic,
            m.fp_semitone,
            m.fp_repeat,
            m.fp_other,
            m.miss_shadowed,
            m.miss_other
        );
    }

    println!("\n### Recall by section ({what})\n");
    let secs: Vec<&str> = totals[0].sections.iter().map(|s| s.0).collect();
    println!("| Detector | {} |", secs.join(" | "));
    println!("| --- |{}", " --- |".repeat(secs.len()));
    for ci in 0..names.len() {
        let cells: Vec<String> =
            totals[ci].sections.iter().map(|s| format!("{:.2} ({}/{})", s.2 as f64 / s.1.max(1) as f64, s.2, s.1)).collect();
        println!("| {} | {} |", names[ci], cells.join(" | "));
    }

    println!("\n### Recall by chord size (notes struck within 50 ms; {what})\n");
    println!("| Detector | 1 | 2 | 3 | 4 | 5 | 6 |");
    println!("| --- | --- | --- | --- | --- | --- | --- |");
    for ci in 0..names.len() {
        let cells: Vec<String> = (1..=6)
            .map(|k| {
                let (n, f) = totals[ci].by_size[k];
                if n == 0 { "-".into() } else { format!("{:.2} ({f}/{n})", f as f64 / n as f64) }
            })
            .collect();
        println!("| {} | {} |", names[ci], cells.join(" | "));
    }

    println!("\n### Live latency by register, median / p90 ms ({what})\n");
    println!("| Detector | {} |", bands.iter().map(|b| b.2).collect::<Vec<_>>().join(" | "));
    println!("| --- |{}", " --- |".repeat(bands.len()));
    for ci in (0..names.len()).filter(|&ci| live[ci]) {
        let cells: Vec<String> = bands
            .iter()
            .map(|&(lo, hi, _)| {
                let v: Vec<f64> = totals[ci].latency_by_note.iter().filter(|x| x.0 >= lo && x.0 < hi).map(|x| x.1).collect();
                format!("{} / {} (n {})", ms(percentile(&v, 0.5)), ms(percentile(&v, 0.9)), v.len())
            })
            .collect();
        println!("| {} | {} |", names[ci], cells.join(" | "));
    }

}
