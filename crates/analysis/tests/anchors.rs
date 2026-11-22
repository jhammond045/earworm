//! Flex anchor accuracy on the flex prototype's takes (docs/quant.md "Q1", the anchor check):
//! drum loops A and B and the sung line, through the whole second pass as the app runs it.
//! At least 85% of onsets found within 50 ms with a mean error of 2 ms or less, and legato
//! joins 12 ms or less late or early on average. `beat_corpus` and `hum_corpus` score the same
//! on their own takes. Run with `--release -- --nocapture` for the figures.

mod common;
use analysis::anchors::{KIND_HIT, KIND_ONSET};
use analysis::second::{Kind, Options, second_pass};
use common::flex_takes::{Feel, drums, sung};
use common::*;
use std::time::Instant;

fn found(audio: &[f32], kind: Kind) -> (Vec<analysis::anchors::Anchor>, f64) {
    let rec = records(audio, SR);
    let t0 = Instant::now();
    let r = second_pass(audio, SR, &rec, &Options { kind: Some(kind), ..Options::default() });
    let secs = t0.elapsed().as_secs_f64();
    assert_eq!(r.kind, kind);
    (r.anchors, secs * 1000.0 / (audio.len() as f64 / SR as f64))
}

#[test]
fn anchors_on_the_prototype_drum_loops() {
    let a = drums(11, 100.0, 4, &Feel { swing: 0.58, snare_ms: 12.0, jitter_ms: 6.0, drift_ms: 10.0 });
    let b = drums(23, 100.0, 4, &Feel { swing: 0.5, snare_ms: -8.0, jitter_ms: 12.0, drift_ms: 0.0 });
    let mut all = AnchorScore::default();
    for (name, take) in [("A", &a), ("B", &b)] {
        let (anchors, ms_per_s) = found(&take.audio, Kind::Beat);
        assert!(anchors.windows(2).all(|w| w[0].t <= w[1].t), "sorted");
        assert!(anchors.iter().all(|x| x.hard() && matches!(x.kind(), KIND_HIT | KIND_ONSET)));
        let times: Vec<f64> = anchors.iter().map(|x| x.t as f64).collect();
        let truth: Vec<f64> = take.events.iter().map(|e| e.t).collect();
        let mut s = AnchorScore::default();
        s.add(&times, &truth);
        all.add(&times, &truth);
        let hits = anchors.iter().filter(|x| x.kind() == KIND_HIT).count();
        println!("drums {name}: {} anchors ({hits} carry a kit slot); {}; second pass with anchors {ms_per_s:.1} ms per second of audio", anchors.len(), s.line());
    }
    println!("drums A and B: {}", all.line());
    assert!(all.found_share() >= 0.85, "{}", all.line());
    assert!(all.mean_ms() <= 2.0, "{}", all.line());
}

#[test]
fn anchors_on_the_prototype_sung_line() {
    let take = sung(5, 100.0, 15.0, 20.0);
    let (anchors, ms_per_s) = found(&take.audio, Kind::Melody);
    let times: Vec<f64> = anchors.iter().map(|x| x.t as f64).collect();
    let pick = |legato: bool| -> Vec<f64> { take.events.iter().filter(|e| (e.class == 2) == legato).map(|e| e.t).collect() };
    let (mut hard, mut legato) = (AnchorScore::default(), AnchorScore::default());
    hard.add(&times, &pick(false));
    legato.add(&times, &pick(true));
    let soft = anchors.iter().filter(|x| !x.hard()).count();
    println!("sung: {} anchors, {soft} soft; detached and \"da\" starts: {}; legato joins: {}; second pass with anchors {ms_per_s:.1} ms per second of audio", anchors.len(), hard.line(), legato.line());
    assert!(hard.found_share() >= 0.85, "{}", hard.line());
    assert!(hard.mean_ms() <= 2.0, "{}", hard.line());
    assert_eq!(legato.found_share(), 1.0, "{}", legato.line());
    assert!(legato.mean_ms() <= 12.0, "{}", legato.line());
}

#[test]
fn the_high_band_pass_costs_little() {
    // The extra work anchors add to the second pass: one onset pass on a high-passed copy.
    let take = drums(11, 100.0, 4, &Feel { swing: 0.58, snare_ms: 12.0, jitter_ms: 6.0, drift_ms: 10.0 });
    let n = take.audio.len() * 100 / SR as usize;
    let lv = analysis::signal::Levels::new(&take.audio, SR, n);
    let full = analysis::signal::detect_onsets(&lv, &analysis::signal::flux(&take.audio, SR, n));
    let mut best = f64::INFINITY;
    for _ in 0..5 {
        let t0 = Instant::now();
        let all = analysis::anchors::onsets_with_high_band(&take.audio, SR, n, &full);
        best = best.min(t0.elapsed().as_secs_f64());
        assert!(all.len() > full.len());
    }
    let per_s = best * 1000.0 / (take.audio.len() as f64 / SR as f64);
    println!("high-band onset pass: {per_s:.2} ms per second of audio (best of 5)");
}
