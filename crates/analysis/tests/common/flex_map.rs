//! Flex for the stretching checks and listening examples (docs/quant.md Q2): anchors from the
//! second pass, quantized as the app does, and the stretch map the compiler sends for a clip
//! that starts with its take and is as long as it.

use super::{SR, records};
use analysis::second::{Kind, Options, second_pass};
use klont::stretch::{POINT_HARD, POINT_RUN, Point, Stretcher};

#[derive(Clone, Copy, Debug)]
pub struct Anchor {
    pub t: f64,
    pub w: f32,
    pub hard: bool,
}

pub fn found(audio: &[f32], kind: Kind) -> Vec<Anchor> {
    let rec = records(audio, SR);
    let r = second_pass(audio, SR, &rec, &Options { kind: Some(kind), ..Options::default() });
    r.anchors.iter().map(|a| Anchor { t: a.t as f64, w: a.w, hard: a.hard() }).collect()
}

/// (performed, played, hard) take seconds.
pub type Moved = (f64, f64, bool);

/// Quantize as the app does (web/src/project/flex.ts `quantize`), on a straight grid of `step`
/// seconds from `lead`, at `strength`, catching anchors half a slot away, one anchor per slot (the
/// heaviest), 12 ms apart at least, riders kept on the line when they fit.
pub fn quantize(anchors: &[Anchor], lead: f64, step: f64, strength: f64) -> Vec<Moved> {
    let gap = 0.012;
    let slot = |t: f64| ((t - lead) / step).round() as i64;
    let mut picks: Vec<(Anchor, Option<i64>)> = anchors.iter().map(|&a| (a, Some(slot(a.t)).filter(|&k| ((lead + k as f64 * step) - a.t).abs() <= step / 2.0))).collect();
    picks.sort_by(|x, y| x.0.t.total_cmp(&y.0.t));
    let owner = |k: i64| {
        let mut best: Option<usize> = None;
        for (i, p) in picks.iter().enumerate() {
            if p.1 == Some(k) && best.is_none_or(|b| p.0.w > picks[b].0.w) {
                best = Some(i);
            }
        }
        best
    };
    let mut out: Vec<Moved> = Vec::new();
    let mut riders = Vec::new();
    for (i, (a, k)) in picks.iter().enumerate() {
        match k {
            Some(k) if owner(*k) == Some(i) => {
                let dst = a.t + strength * (lead + *k as f64 * step - a.t);
                if out.last().is_some_and(|p| dst < p.1 + gap || a.t < p.0 + gap) {
                    riders.push(*a);
                } else {
                    out.push((a.t, dst, a.hard));
                }
            }
            _ => riders.push(*a),
        }
    }
    let line = out.clone();
    let at = |t: f64| {
        let i = line.partition_point(|p| p.0 <= t);
        match (i.checked_sub(1).map(|j| line[j]), line.get(i)) {
            (Some(a), Some(b)) => a.1 + (b.1 - a.1) * (t - a.0) / (b.0 - a.0),
            (Some(a), None) => a.1 + t - a.0,
            (None, Some(b)) => b.1 + t - b.0,
            (None, None) => t,
        }
    };
    for a in riders {
        let d = at(a.t);
        let i = out.partition_point(|p| p.0 < a.t);
        let ok_prev = i == 0 || (a.t - out[i - 1].0 >= gap && d - out[i - 1].1 >= gap);
        let ok_next = i == out.len() || (out[i].0 - a.t >= gap && out[i].1 - d >= gap);
        if ok_prev && ok_next {
            out.insert(i, (a.t, d, a.hard));
        }
    }
    out
}

/// Where performed take second `t` plays.
pub fn forward(moved: &[Moved], t: f64) -> f64 {
    let i = moved.partition_point(|p| p.0 <= t);
    match (i.checked_sub(1).map(|j| moved[j]), moved.get(i)) {
        (Some(a), Some(b)) => a.1 + (b.1 - a.1) * (t - a.0) / (b.0 - a.0),
        (Some(a), None) => a.1 + t - a.0,
        (None, Some(b)) => b.1 + t - b.0,
        (None, None) => t,
    }
}

/// The stretch map as the compiler sends it for a clip that starts with the take and is as long
/// as it: one run, pinned at the take's ends. (played seconds, take seconds, hard).
pub fn stretch_map(moved: &[Moved], len: f64) -> Vec<(f64, f64, bool)> {
    let mut m = vec![(0.0, 0.0, true)];
    m.extend(moved.iter().filter(|p| p.1 > 0.012 && p.1 < len - 0.012).map(|p| (p.1, p.0, p.2)));
    m.push((len, len, true));
    m
}

/// The map as the layer's stretcher takes it, in samples.
pub fn points(map: &[(f64, f64, bool)]) -> Vec<Point> {
    map.iter().enumerate().map(|(i, p)| Point { u: p.0 * SR as f64, s: p.1 * SR as f64, flags: if p.2 { POINT_HARD } else { 0 } | if i == 0 { POINT_RUN } else { 0 } }).collect()
}

/// The layer's stretcher alone over `audio` through `map` (seconds), from output sample `start`.
pub fn stretch_alone(audio: &[f32], map: &[(f64, f64, bool)], mode: u32, start: usize) -> Vec<f32> {
    let mut st = Stretcher::new(SR);
    assert!(st.load(mode, audio.len() as f64, points(map).into_iter()));
    st.seek(start as f64);
    let mut out = vec![0.0f32; audio.len() - start];
    st.process(audio, &mut out, |_| 1.0);
    out
}

/// Write `x` as 16-bit mono to `$Q2_WAV/name.wav`; nothing when Q2_WAV is unset.
pub fn write_wav(name: &str, x: &[f32]) {
    let Ok(dir) = std::env::var("Q2_WAV") else { return };
    let spec = hound::WavSpec { channels: 1, sample_rate: SR as u32, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(format!("{dir}/{name}.wav"), spec).unwrap();
    for &v in x {
        w.write_sample((v.clamp(-1.0, 1.0) * 32767.0).round() as i16).unwrap();
    }
    w.finalize().unwrap();
}
