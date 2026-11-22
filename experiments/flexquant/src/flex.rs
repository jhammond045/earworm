//! The flex model from docs/quant.md: anchors found in a take, a target (grid resolution plus
//! a feel), a strength, and the warp map that results. Pure functions of their inputs, so the
//! same document always gives the same map.

use analysis::second::{Kind, Options, STRIDE, second_pass};
use analysis::signal::{FPS, Levels, detect_onsets, flux, refine_onset};
use dsp::capture::frames_from_signal;
use dsp::frame::{DEFAULT_BANDS, ONSET, frame_len};

/// A point in a take that should land on the beat.
#[derive(Clone, Copy, Debug)]
pub struct Anchor {
    /// Take seconds.
    pub t: f64,
    /// How sure the detector is, or how strong the event; collisions keep the heavier one.
    pub w: f32,
    /// A transient: the audio around it is copied untouched. A soft anchor (a legato pitch
    /// change, a feature peak) only bends the time map; stretching runs straight through it.
    pub hard: bool,
}

// ---------------------------------------------------------------- anchor detectors

/// The second pass's onset detector on the audio (spectral flux, refined on a 1 ms envelope).
pub fn onsets(audio: &[f32], sr: f32) -> Vec<Anchor> {
    let n = (audio.len() as f32 / sr * FPS) as usize;
    let lv = Levels::new(audio, sr, n);
    detect_onsets(&lv, &flux(audio, sr, n)).iter().map(|o| Anchor { t: o.t as f64, w: o.flux, hard: true }).collect()
}

/// Fourth-order high-pass at `fc` (two Butterworth biquads).
fn highpass(x: &[f32], sr: f32, fc: f32) -> Vec<f32> {
    let w0 = std::f32::consts::TAU * fc / sr;
    let (sn, cs) = w0.sin_cos();
    let alpha = sn / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
    let a0 = 1.0 + alpha;
    let (b0, b1, b2) = ((1.0 + cs) / 2.0 / a0, -(1.0 + cs) / a0, (1.0 + cs) / 2.0 / a0);
    let (a1, a2) = (-2.0 * cs / a0, (1.0 - alpha) / a0);
    let mut y = x.to_vec();
    for _ in 0..2 {
        let (mut x1, mut x2, mut y1, mut y2) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for v in y.iter_mut() {
            let x0 = *v;
            let out = b0 * x0 + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
            x2 = x1;
            x1 = x0;
            y2 = y1;
            y1 = out;
            *v = out;
        }
    }
    y
}

/// Onsets on the full signal plus onsets on a 4 kHz high-passed copy, which catches quiet bright
/// hits (hats) under louder low ones. Within 20 ms of a full-band onset the full-band one wins.
pub fn onsets_sensitive(audio: &[f32], sr: f32) -> Vec<Anchor> {
    let mut all = onsets(audio, sr);
    let hf = onsets(&highpass(audio, sr, 4000.0), sr);
    for h in hf {
        if !all.iter().any(|a| (a.t - h.t).abs() < 0.02) {
            all.push(Anchor { t: h.t, w: h.w * 0.5, hard: true });
        }
    }
    all.sort_by(|a, b| a.t.total_cmp(&b.t));
    all
}

/// The 25-float records the second pass reads: live-tier frames plus a NaN tuned slot.
pub fn records(audio: &[f32], sr: f32) -> Vec<f32> {
    let fr = frames_from_signal(audio, sr, DEFAULT_BANDS);
    let mut out = Vec::with_capacity(fr.len() * STRIDE);
    for f in &fr {
        out.extend_from_slice(&f[..frame_len(DEFAULT_BANDS)]);
        out.push(f32::NAN);
    }
    out
}

/// What the app already stores: the second pass's note starts (melody) or hits (beat).
pub fn second_pass_anchors(audio: &[f32], sr: f32, beat: bool) -> Vec<Anchor> {
    let rec = records(audio, sr);
    let opts = Options { kind: Some(if beat { Kind::Beat } else { Kind::Melody }), ..Options::default() };
    let r = second_pass(audio, sr, &rec, &opts);
    if beat {
        r.hits.iter().map(|h| Anchor { t: h.t as f64, w: h.strength, hard: true }).collect()
    } else {
        r.notes.iter().map(|n| Anchor { t: n.t as f64, w: 0.5 + n.hard, hard: true }).collect()
    }
}

/// Second pass note starts, with each start that has no audio onset near it (a legato join,
/// found from the pitch) moved back to where the pitch is halfway from the last note to this one.
pub fn note_anchors_refined(audio: &[f32], sr: f32) -> Vec<Anchor> {
    let rec = records(audio, sr);
    let opts = Options { kind: Some(Kind::Melody), ..Options::default() };
    let r = second_pass(audio, sr, &rec, &opts);
    let ons = onsets(audio, sr);
    // pYIN on the audio itself, frame-centred; the second pass's pitch is mostly the live
    // tier's, which trails the audio.
    let pitch = analysis::pyin::pyin(audio, sr);
    let mut out = Vec::new();
    for (i, n) in r.notes.iter().enumerate() {
        let mut t = n.t as f64;
        let hard = i == 0 || ons.iter().any(|o| (o.t - t).abs() < 0.02);
        if !hard {
            let (prev, cur) = (r.notes[i - 1].cents, n.cents);
            let k0 = ((t - 0.12) * FPS as f64).floor().max(1.0) as usize;
            let k1 = ((t * FPS as f64).ceil() as usize + 2).min(pitch.len());
            let frac = |k: usize| (pitch[k] - prev) / (cur - prev);
            for k in k0..k1 {
                let (f0, f1) = (frac(k - 1), frac(k));
                if f0.is_finite() && f1.is_finite() && f0 < 0.5 && f1 >= 0.5 {
                    t = (k as f64 - 1.0 + ((0.5 - f0) / (f1 - f0)) as f64) / FPS as f64;
                    break;
                }
            }
        }
        out.push(Anchor { t, w: 0.5 + n.hard, hard });
    }
    out
}

/// Anchors from a captured signal alone: peaks of the live tier's onset slot, at frame
/// resolution (10 ms). With the take's audio at hand, each is refined on the 1 ms envelope.
pub fn frame_anchors(audio: &[f32], sr: f32, refine: bool) -> Vec<Anchor> {
    let fr = frames_from_signal(audio, sr, DEFAULT_BANDS);
    let n = fr.len();
    let lv = refine.then(|| Levels::new(audio, sr, n));
    let mut out = Vec::new();
    for k in 0..n {
        let v = fr[k][ONSET];
        if v < 0.5 || (k > 0 && fr[k - 1][ONSET] >= v) || (k + 1 < n && fr[k + 1][ONSET] > v) {
            continue;
        }
        let t = k as f32 / FPS;
        let t = match &lv {
            Some(lv) => refine_onset(lv, t),
            None => t,
        };
        out.push(Anchor { t: t as f64, w: v, hard: true });
    }
    out
}

// ---------------------------------------------------------------- targets

/// Where beats fall in a section: beat 0 at `origin` seconds, `bpm` beats a minute.
#[derive(Clone, Copy)]
pub struct Grid {
    pub bpm: f64,
    pub origin: f64,
}

impl Grid {
    pub fn time(&self, beat: f64) -> f64 {
        self.origin + beat * 60.0 / self.bpm
    }
    pub fn beat(&self, t: f64) -> f64 {
        (t - self.origin) * self.bpm / 60.0
    }
}

/// How the grid is bent before anchors are pulled to it.
#[derive(Clone)]
pub enum Feel {
    Straight,
    /// Every second slot of `unit` beats falls at `amount` of the pair (0.5 straight, 0.66 triplet).
    Swing { amount: f64, unit: f64 },
    /// Points taken from another layer: (beat, seconds after beat 0), sorted by beat. Between
    /// points the time is interpolated; past the ends it runs at the section tempo. Times count
    /// from beat 0 so a layer whose take starts elsewhere in the section can use them.
    Map(Vec<(f64, f64)>),
}

pub fn feel_time(g: &Grid, feel: &Feel, beat: f64) -> f64 {
    match feel {
        Feel::Straight => g.time(beat),
        Feel::Swing { amount, unit } => {
            let slot = |k: f64| {
                let odd = (k as i64).rem_euclid(2) == 1;
                g.time(k * unit + if odd { (amount - 0.5) * 2.0 * unit } else { 0.0 })
            };
            let k = (beat / unit).floor();
            let f = beat / unit - k;
            slot(k) + (slot(k + 1.0) - slot(k)) * f
        }
        Feel::Map(p) => {
            if p.is_empty() {
                return g.time(beat);
            }
            let i = p.partition_point(|q| q.0 <= beat);
            let rel = if i == 0 {
                p[0].1 + (beat - p[0].0) * 60.0 / g.bpm
            } else if i == p.len() {
                let l = p[p.len() - 1];
                l.1 + (beat - l.0) * 60.0 / g.bpm
            } else {
                let (a, b) = (p[i - 1], p[i]);
                a.1 + (b.1 - a.1) * (beat - a.0) / (b.0 - a.0)
            };
            g.origin + rel
        }
    }
}

pub struct Target<'a> {
    /// Grid resolution in beats (0.25 is 16ths).
    pub res: f64,
    pub feel: Feel,
    /// 0..1 at a beat; a lane in the app, sampled once per anchor.
    pub strength: &'a dyn Fn(f64) -> f64,
    /// Anchors further than this from their slot stay where they are (seconds).
    pub window: f64,
    /// Moved anchors closer than this to the previous one are dropped (seconds).
    pub min_gap: f64,
}

/// One anchor after quantizing: its take time, where it now plays, and its slot.
#[derive(Clone, Copy, Debug)]
pub struct Moved {
    pub src: f64,
    pub dst: f64,
    /// The slot it was pulled to, in beats; NaN when it rides along (no slot, or lost its slot).
    pub slot: f64,
    pub hard: bool,
}

/// Pull each anchor toward the nearest slot of the bent grid by the strength at that slot.
/// Two anchors on one slot: the heavier moves, the other rides along with the time map.
pub fn quantize(anchors: &[Anchor], g: &Grid, tg: &Target) -> Vec<Moved> {
    let mut picks: Vec<(Anchor, f64, f64)> = Vec::new(); // anchor, slot beat, slot time
    for a in anchors {
        let k0 = (g.beat(a.t) / tg.res).round();
        let mut best = (f64::INFINITY, f64::NAN, f64::NAN);
        for k in [k0 - 1.0, k0, k0 + 1.0] {
            let beat = k * tg.res;
            let t = feel_time(g, &tg.feel, beat);
            if (t - a.t).abs() < best.0 {
                best = ((t - a.t).abs(), beat, t);
            }
        }
        if best.0 <= tg.window {
            picks.push((*a, best.1, best.2));
        } else {
            picks.push((*a, f64::NAN, f64::NAN));
        }
    }
    picks.sort_by(|x, y| x.0.t.total_cmp(&y.0.t));
    // One anchor per slot.
    let mut keep = vec![true; picks.len()];
    for i in 0..picks.len() {
        for j in i + 1..picks.len() {
            if picks[i].1.is_nan() || picks[j].1 != picks[i].1 {
                continue;
            }
            if picks[j].0.w > picks[i].0.w {
                keep[i] = false;
            } else {
                keep[j] = false;
            }
        }
    }
    let mut out: Vec<Moved> = Vec::new();
    let mut riders: Vec<Anchor> = Vec::new();
    for (p, k) in picks.iter().zip(keep) {
        let (a, slot, t) = *p;
        if !k || slot.is_nan() {
            riders.push(a);
            continue;
        }
        let s = (tg.strength)(slot).clamp(0.0, 1.0);
        let dst = a.t + s * (t - a.t);
        if let Some(prev) = out.last() {
            if dst < prev.dst + tg.min_gap || a.t < prev.src + tg.min_gap {
                riders.push(a);
                continue;
            }
        }
        out.push(Moved { src: a.t, dst, slot, hard: a.hard });
    }
    // Anchors that were not pulled to a slot still mark a transient: they ride along with the
    // map (so it does not change) and stay segment boundaries, so no stretch runs through them.
    let line = out.clone();
    for a in riders {
        let t = a.t;
        let dst = interp(&line, t);
        let i = out.partition_point(|m| m.src < t);
        let ok_prev = i == 0 || (t - out[i - 1].src >= tg.min_gap && dst - out[i - 1].dst >= tg.min_gap);
        let ok_next = i == out.len() || (out[i].src - t >= tg.min_gap && out[i].dst - dst >= tg.min_gap);
        if ok_prev && ok_next {
            out.insert(i, Moved { src: t, dst, slot: f64::NAN, hard: a.hard });
        }
    }
    out
}

/// Where take time `t` plays, by the moved anchors alone (offset of the nearest one past the ends).
fn interp(m: &[Moved], t: f64) -> f64 {
    if m.is_empty() {
        return t;
    }
    let i = m.partition_point(|x| x.src <= t);
    if i == 0 {
        return t + m[0].dst - m[0].src;
    }
    if i == m.len() {
        return t + m[i - 1].dst - m[i - 1].src;
    }
    let (a, b) = (m[i - 1], m[i]);
    a.dst + (b.dst - a.dst) * (t - a.src) / (b.src - a.src)
}

/// The feel a layer gives others: where each of its slots is heard (beat, seconds after beat 0).
pub fn groove_points(moved: &[Moved], g: &Grid) -> Vec<(f64, f64)> {
    moved.iter().filter(|m| !m.slot.is_nan()).map(|m| (m.slot, m.dst - g.origin)).collect()
}

/// The same feel averaged into a repeating template: the mean offset of each slot within
/// `period` beats from the straight grid, laid out again over `beats` beats.
pub fn groove_template(moved: &[Moved], g: &Grid, res: f64, period: f64, beats: f64) -> Vec<(f64, f64)> {
    let slots = (period / res).round() as usize;
    let mut sum = vec![(0.0f64, 0usize); slots];
    for m in moved.iter().filter(|m| !m.slot.is_nan()) {
        let i = ((m.slot / res).round() as i64).rem_euclid(slots as i64) as usize;
        sum[i].0 += m.dst - g.time(m.slot);
        sum[i].1 += 1;
    }
    let off: Vec<Option<f64>> = sum.iter().map(|&(s, c)| (c > 0).then(|| s / c as f64)).collect();
    let n = (beats / res).round() as usize;
    (0..=n)
        .filter_map(|k| off[k % slots].map(|o| (k as f64 * res, g.time(k as f64 * res) - g.origin + o)))
        .collect()
}

/// Map a take time to where it now plays (piecewise linear through the moved anchors, pinned
/// at both ends of the take). Notes, hits and recorded lane points move through this.
pub fn forward(moved: &[Moved], len: f64, t: f64) -> f64 {
    let pts = pins(moved, len);
    let i = pts.partition_point(|p| p.0 <= t).clamp(1, pts.len() - 1);
    let (a, b) = (pts[i - 1], pts[i]);
    a.1 + (b.1 - a.1) * (t - a.0) / (b.0 - a.0)
}

/// (src, dst, hard) points with the take's ends pinned, seconds.
fn pins(moved: &[Moved], len: f64) -> Vec<(f64, f64, bool)> {
    let mut pts = vec![(0.0, 0.0, true)];
    pts.extend(moved.iter().filter(|m| m.src > 0.0 && m.src < len && m.dst > 0.0 && m.dst < len).map(|m| (m.src, m.dst, m.hard)));
    pts.push((len, len, true));
    pts
}

/// The warp map the engine would get: (output sample, take sample, hard) points, both
/// increasing, starting at (0, 0) and ending at the take's length.
pub fn warp_map(moved: &[Moved], len_samples: usize, sr: f32) -> Vec<(f64, f64, bool)> {
    let len = len_samples as f64 / sr as f64;
    pins(moved, len).iter().map(|&(s, d, h)| (d * sr as f64, s * sr as f64, h)).collect()
}
