//! The "hums kept" gate (TDD "Testing", melody analysis): note-level precision and
//! recall on a deterministic synthetic corpus. There is no recorded corpus yet;
//! `note_eval` scores real recordings the same way.
//!
//! 11 voices from low male to child, 5 seeded hums each (55 hums), 3 to 12 notes and
//! 2 to 8 s: vibrato, scoops, legato and detached phrasing, repeated pitches split by
//! dips or "da" bursts, slides, breath noise, jitter and shimmer. A found note matches
//! a true note when its onset is within 50 ms and its pitch within 50 cents (one to
//! one). A hum is kept when F1 >= 0.9. Run with `--release -- --nocapture`.

mod common;
use analysis::second::{Kind, Options, second_pass};
use common::*;

const ONSET_TOL: f32 = 0.05;
const CENTS_TOL: f32 = 50.0;
const KEPT_F1: f32 = 0.9;
const HUMS_PER_VOICE: u64 = 5;

struct Row {
    voice: &'static str,
    desc: String,
    seed: u64,
    vi: usize,
    score: NoteScore,
    kind: Kind,
    feat: [f32; 5],
    missed: Vec<String>,
    extra: Vec<String>,
}

fn run_hum(vi: usize, voice: &Voice, seed: u64, opts: &Options) -> Row {
    let hum = make_hum(voice, seed);
    let rec = records(&hum.audio, SR);
    let r = second_pass(&hum.audio, SR, &rec, opts);
    let found: Vec<(f32, f32)> = r.notes.iter().map(|n| (n.t, n.cents)).collect();
    let truth: Vec<(f32, f32)> = hum.notes.iter().map(|n| (n.onset, n.cents)).collect();
    let score = match_notes(&found, &truth, ONSET_TOL, CENTS_TOL).0;
    // Which are unmatched, for the failure report.
    let mut used = vec![false; found.len()];
    let mut missed = Vec::new();
    for (j, t) in hum.notes.iter().enumerate() {
        let hit = found.iter().enumerate().position(|(i, f)| !used[i] && (f.0 - t.onset).abs() <= ONSET_TOL && (f.1 - t.cents).abs() <= CENTS_TOL);
        match hit {
            Some(i) => used[i] = true,
            None => missed.push(format!("true #{j} {:.2}s {:.0}c {:?} scoop {:.0}", t.onset, t.cents, t.link, t.scoop)),
        }
    }
    let extra = found.iter().enumerate().filter(|(i, _)| !used[*i]).map(|(_, f)| format!("found {:.2}s {:.0}c", f.0, f.1)).collect();
    Row { voice: voice.name, desc: hum.desc, seed, vi, score, kind: r.kind, feat: r.kind_features, missed, extra }
}

#[test]
fn hums_kept() {
    let voices = voices();
    let opts = Options::default();
    let mut rows = Vec::new();
    for (vi, v) in voices.iter().enumerate() {
        for s in 0..HUMS_PER_VOICE {
            rows.push(run_hum(vi, v, 1000 * (vi as u64 + 1) + s, &opts));
        }
    }
    let (mut tp, mut found, mut truth, mut kept, mut melody) = (0, 0, 0, 0, 0);
    for r in &rows {
        let k = r.score.f1() >= KEPT_F1;
        println!(
            "{} {:18} P {:.2} R {:.2} F1 {:.2} ({} of {} true, {} found) kind {:?} [vf {:.2} ons/s {:.1} ev {:.2} stab {:.2} perc {:.2}] | voice {} seed {} | {}",
            if k { "kept  " } else { "FAILED" },
            r.voice,
            r.score.precision(),
            r.score.recall(),
            r.score.f1(),
            r.score.tp,
            r.score.truth,
            r.score.found,
            r.kind,
            r.feat[0],
            r.feat[1],
            r.feat[2],
            r.feat[3],
            r.feat[4],
            r.vi,
            r.seed,
            r.desc
        );
        if !k {
            for m in &r.missed {
                println!("        missed: {m}");
            }
            for e in &r.extra {
                println!("        extra:  {e}");
            }
        }
        tp += r.score.tp;
        found += r.score.found;
        truth += r.score.truth;
        kept += k as usize;
        melody += (r.kind == Kind::Melody) as usize;
    }
    let total = NoteScore { tp, found, truth };
    let share = kept as f32 / rows.len() as f32;
    println!(
        "\ncorpus: {} hums, {} true notes, {} found. precision {:.3} recall {:.3} F1 {:.3}. kept {} of {} = {:.1}% (target 80%). kind=melody {} of {}",
        rows.len(),
        truth,
        found,
        total.precision(),
        total.recall(),
        total.f1(),
        kept,
        rows.len(),
        100.0 * share,
        melody,
        rows.len()
    );
    assert!(rows.len() >= 40);
    assert!(share >= 0.8, "kept share {:.1}% is under the 80% target", 100.0 * share);
    assert!(melody as f32 >= 0.95 * rows.len() as f32, "kind detection called {} of {} hums melody", melody, rows.len());
}
