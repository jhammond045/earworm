//! Beat slices played in a new order. A pattern names, for each beat of the new bar, which beat
//! of the source bar plays there. Output beats whose source beats follow on unbroken make one
//! run; each run is the source moved by a whole number of beats, so every hit keeps its played
//! offset from the grid. A run starts at the first hard anchor near its first beat, so a hit
//! played a little early stays with its beat.
//!
//! Two renderers: the engine's own stretcher (`klont::stretch::Stretcher`, hits mode) fed the
//! runs as a map with run points, unchanged; and a plain slicer that can use a longer crossfade
//! where a run starts with no hard anchor to cut at.

use crate::groove::SR;
use klont::stretch::{MODE_HITS, Point, POINT_HARD, POINT_RUN, Stretcher};

/// Hard anchors of a take: the app's own onset detector plus its high-band pass, as
/// `crates/analysis/src/anchors.rs` finds them for beat takes. (seconds, weight)
pub fn anchors(x: &[f32]) -> Vec<(f64, f32)> {
    let n = x.len() * 100 / SR as usize;
    let lv = analysis::signal::Levels::new(x, SR, n);
    let full = analysis::signal::detect_onsets(&lv, &analysis::signal::flux(x, SR, n));
    analysis::anchors::onsets_with_high_band(x, SR, n, &full).into_iter().map(|(t, w)| (t as f64, w)).collect()
}

/// Where a slice that starts at beat time `t` is cut: the earliest strong anchor within `w` of
/// it (at least 0.3 of the heaviest there), so an early kick is not split from its beat. None:
/// no anchor, cut at `t`.
pub fn refine(anchors: &[(f64, f32)], t: f64, w: f64) -> Option<f64> {
    let near: Vec<&(f64, f32)> = anchors.iter().filter(|a| (a.0 - t).abs() <= w).collect();
    let top = near.iter().map(|a| a.1).fold(0.0f32, f32::max);
    near.iter().filter(|a| a.1 >= 0.3 * top).map(|a| a.0).min_by(|a, b| a.total_cmp(b))
}

#[derive(Clone, Copy, Debug)]
pub struct Run {
    /// Output beats [a, b) play source beats [k, k + b - a).
    pub a: usize,
    pub b: usize,
    pub k: usize,
    /// Output start and end, source start and end, in whole samples.
    pub u0: i64,
    pub u1: i64,
    pub s0: i64,
    pub s1: i64,
    /// The start was cut at a hard anchor.
    pub hard: bool,
}

impl Run {
    pub fn shift(&self) -> i64 {
        self.u0 - self.s0
    }
}

/// Runs for output beats `src` (the global source beat of each), on a grid of `spb_samples` per
/// beat starting at `lead` samples. `cut(beat time) -> (cut time, hard)` picks each cut.
pub fn runs(src: &[usize], lead: i64, spb_samples: i64, cut: impl Fn(i64) -> (i64, bool)) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    let mut a = 0;
    while a < src.len() {
        let mut b = a + 1;
        while b < src.len() && src[b] == src[b - 1] + 1 {
            b += 1;
        }
        let k = src[a];
        let shift = (a as i64 - k as i64) * spb_samples;
        let (s0, hard) = cut(lead + k as i64 * spb_samples);
        let (s1, _) = cut(lead + (k + b - a) as i64 * spb_samples);
        let (u0, u1) = (s0 + shift, s1 + shift);
        if let Some(p) = out.last_mut() {
            // Two runs never overlap: the earlier one ends where the later one starts.
            if p.u1 > u0 {
                p.s1 -= p.u1 - u0;
                p.u1 = u0;
            }
        }
        out.push(Run { a, b, k, u0, u1, s0, s1, hard });
        a = b;
    }
    out
}

/// The runs as the engine's stretch map: a hard run point where each run starts, a hard point
/// where it ends. Points in samples, sorted by output position.
pub fn points(runs: &[Run]) -> Vec<Point> {
    let mut p = Vec::new();
    for r in runs {
        p.push(Point { u: r.u0 as f64, s: r.s0 as f64, flags: POINT_RUN | POINT_HARD });
        p.push(Point { u: r.u1 as f64, s: r.s1 as f64, flags: POINT_HARD });
    }
    p
}

/// Play the runs through the engine's stretcher in 128-sample calls, `len` samples, one pass.
pub fn engine(src: &[f32], runs: &[Run], len: usize, mode: u32) -> Vec<f32> {
    let mut st = Stretcher::new(SR);
    let mut out = vec![0.0f32; len];
    if !st.load(mode, len as f64 + 1.0, points(runs).into_iter()) {
        return out;
    }
    st.seek(0.0);
    for chunk in out.chunks_mut(128) {
        st.process(src, chunk, |_| 1.0);
    }
    out
}

pub fn engine_hits(src: &[f32], runs: &[Run], len: usize) -> Vec<f32> {
    engine(src, runs, len, MODE_HITS)
}

/// A plain slicer: each run read at the source's own speed, fading in over `fade` samples before
/// its start and out over the next run's fade before its end, equal power (the two sides are
/// different audio). `fade_hard` where the run starts at an anchor, else `fade_soft`.
pub fn slicer(src: &[f32], runs: &[Run], len: usize, fade_hard: i64, fade_soft: i64) -> Vec<f32> {
    let mut out = vec![0.0f32; len];
    let fade_of = |r: &Run| if r.hard { fade_hard } else { fade_soft };
    for (i, r) in runs.iter().enumerate() {
        let fi = fade_of(r);
        let fo = runs.get(i + 1).map(fade_of).unwrap_or(fade_hard);
        let shift = r.shift();
        for u in (r.u0 - fi).max(0)..r.u1.min(len as i64) {
            let s = u - shift;
            if s < 0 || s as usize >= src.len() {
                continue;
            }
            let mut g = 1.0f32;
            if u < r.u0 {
                g *= (std::f32::consts::FRAC_PI_2 * ((u - (r.u0 - fi)) as f32 + 0.5) / fi as f32).sin();
            }
            if u >= r.u1 - fo {
                g *= (std::f32::consts::FRAC_PI_2 * ((u - (r.u1 - fo)) as f32 + 0.5) / fo as f32).cos();
            }
            out[u as usize] += g * src[s as usize];
        }
    }
    out
}

/// Joins: output samples where a run starts that does not follow on from the one before.
pub fn joins(runs: &[Run]) -> Vec<i64> {
    runs.iter().skip(1).map(|r| r.u0).collect()
}
