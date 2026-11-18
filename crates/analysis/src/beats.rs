//! Beats (TDD "Beats"): events, hit shapes, grouping into kit slots.

use crate::notes::{hardness_from_attack, rise_time};
use crate::signal::{FPS, Levels, Onset};
use dsp::fft::{Fft, hann};
use dsp::frame::{BANDS, DEFAULT_BANDS, live_hit_features};

/// Longest event, seconds.
pub const MAX_EVENT_S: f32 = 2.0;
/// An event ends when the level falls this far below its peak.
pub const EVENT_FALL_DB: f32 = 30.0;
/// Final fall faster than this, dB per ms, is a stop.
pub const END_STOP_DB_PER_MS: f32 = 1.0;
/// Flux (dB mean rise) that counts as a click in the last 40 ms.
pub const END_CLICK_FLUX: f32 = 1.5;

/// Spread floors per grouping feature, also the unit of the standardization:
/// ln attack (ms), burst, low-band share, centroid (octaves), tone, ln hold (ms).
pub const FEATURE_FLOOR: [f32; 6] = [1.0, 0.2, 0.15, 0.7, 0.4, 0.5];
/// Weight of each feature in distances: hold counts half.
pub const FEATURE_WEIGHT: [f32; 6] = [1.0, 1.0, 1.0, 1.0, 1.0, 0.5];
/// A hit this many (weighted, spread-normalized) units from every taught centre
/// goes to a spare slot instead.
pub const TAUGHT_MAX: f32 = 2.5;
/// Best silhouette must reach this for more than one kind.
pub const MIN_SILHOUETTE: f32 = 0.45;
/// And the closest two group centres must sit this far apart (standardized units).
pub const MIN_SEPARATION: f32 = 2.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HitShape {
    /// 10% to 90% rise time of the level, seconds.
    pub attack: f32,
    /// Share of energy above 4 kHz in the first 20 ms, 0..1.
    pub burst: f32,
    /// Median pitch of the voiced frames, cents above C0.
    pub cents: Option<f32>,
    /// Slope of a line fitted to the pitch, semitones per second.
    pub bend: f32,
    /// Event length, seconds.
    pub hold: f32,
    /// Mean openness over the body, 0..1.
    pub vowel: f32,
    /// Voiced fraction of the body, 0..1.
    pub tone: f32,
    /// 1 when the final fall is faster than `END_STOP_DB_PER_MS`, else 0.
    pub end_stop: f32,
    /// 1 when a flux peak sits in the last 40 ms, else 0.
    pub end_click: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Hit {
    pub t: f32,
    pub dur: f32,
    pub slot: u8,
    pub strength: f32,
    pub shape: HitShape,
    /// Grouping vector, unstandardized.
    pub features: [f32; 6],
    /// `dsp::frame::live_hit_features` of the frame at the onset.
    pub live: [f32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Taught {
    pub centre: [f32; 6],
    pub spread: [f32; 6],
}

/// Everything the hit measurements read.
pub struct HitInput<'a> {
    pub audio: &'a [f32],
    pub sr: f32,
    pub lv: &'a Levels,
    pub flux: &'a [f32],
    pub onsets: &'a [Onset],
    /// Repaired pitch per frame, cents, NaN unvoiced.
    pub pitch: &'a [f32],
    /// 25-float records.
    pub frames: &'a [f32],
    pub stride: usize,
}

impl HitInput<'_> {
    fn n(&self) -> usize {
        self.lv.n.min(self.pitch.len())
    }
    fn rec(&self, k: usize) -> &[f32] {
        &self.frames[k * self.stride..(k + 1) * self.stride]
    }
}

/// Power spectrum of `audio[start..start+len]` under a Hann window, zero padded
/// to a power of two. Returns (power per bin up to Nyquist, bin width in Hz).
fn spectrum(audio: &[f32], sr: f32, start: isize, len: usize) -> (Vec<f32>, f32) {
    let len = len.max(16);
    let mut size = 64usize;
    while size < len {
        size *= 2;
    }
    let fft = Fft::new(size);
    let mut win = vec![0.0f32; len];
    hann(&mut win);
    let mut re = vec![0.0f32; size];
    let mut im = vec![0.0f32; size];
    for i in 0..len {
        let idx = start + i as isize;
        let v = if idx >= 0 && (idx as usize) < audio.len() { audio[idx as usize] } else { 0.0 };
        re[i] = if v.is_finite() { v * win[i] } else { 0.0 };
    }
    fft.forward(&mut re, &mut im);
    let p = (0..size / 2).map(|i| re[i] * re[i] + im[i] * im[i]).collect();
    (p, sr / size as f32)
}

fn band_share(p: &[f32], hz: f32, from: f32, to: f32, lo_cut: f32) -> f32 {
    let (mut part, mut all) = (0.0f32, 0.0f32);
    for (i, &v) in p.iter().enumerate() {
        let f = i as f32 * hz;
        if f < lo_cut {
            continue;
        }
        all += v;
        if f >= from && f < to {
            part += v;
        }
    }
    if all > 0.0 { part / all } else { 0.0 }
}

fn median(v: &mut [f32]) -> f32 {
    let mid = v.len() / 2;
    *v.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1
}

/// Measure every onset as a hit. Slots are all 0 until `group` runs.
pub fn measure_hits(inp: &HitInput) -> Vec<Hit> {
    let n = inp.n();
    let lv = inp.lv;
    let mut hits = Vec::with_capacity(inp.onsets.len());
    let take_end = n as f32 / FPS;
    for (i, o) in inp.onsets.iter().enumerate() {
        let next = inp.onsets.get(i + 1);
        // Event end: level 30 dB under its peak, the quiet level, the next onset or the cap.
        let mut peak = f32::NEG_INFINITY;
        let mut end_t = (o.t + MAX_EVENT_S).min(take_end);
        let mut by_onset = false;
        let mut k = o.frame;
        while k < n {
            let tk = k as f32 / FPS;
            if let Some(nx) = next {
                if tk >= nx.t - 0.002 && k > o.frame {
                    end_t = nx.t;
                    by_onset = true;
                    break;
                }
            }
            if tk >= o.t + MAX_EVENT_S {
                break;
            }
            let l = lv.frame_db[k];
            peak = peak.max(l);
            if k > o.frame + 2 && (l < peak - EVENT_FALL_DB || l < lv.floor_db + 3.0) {
                end_t = tk;
                break;
            }
            k += 1;
        }
        let end_t = end_t.max(o.t + 0.01);
        let hold = end_t - o.t;
        let end_f = ((end_t * FPS).round() as usize).clamp(o.frame + 1, n);

        let attack = rise_time(lv, o.t, hold.min(0.15)).unwrap_or(0.005);

        // Spectral measurements straight from the audio.
        let s0 = (o.t * inp.sr) as isize;
        let (pb, hzb) = spectrum(inp.audio, inp.sr, s0 - (0.002 * inp.sr) as isize, (0.022 * inp.sr) as usize);
        let burst = band_share(&pb, hzb, 4000.0, f32::INFINITY, 60.0);
        let body_len = (hold.clamp(0.03, 0.06) * inp.sr) as usize;
        let (pk, hzk) = spectrum(inp.audio, inp.sr, s0, body_len);
        let low = band_share(&pk, hzk, 0.0, 250.0, 40.0);
        let (mut num, mut den) = (0.0f64, 0.0f64);
        for (i, &v) in pk.iter().enumerate() {
            let f = i as f32 * hzk;
            if f >= 40.0 {
                num += v as f64 * f as f64;
                den += v as f64;
            }
        }
        let centroid = if den > 0.0 { (num / den) as f32 } else { 100.0 };
        let centroid_oct = (centroid.max(50.0) / 1000.0).log2();

        // Pitch, openness and tone over the body: the frames after the attack.
        let b0 = (o.frame + 2).min(end_f.saturating_sub(1)).max(o.frame);
        let mut ps: Vec<f32> = Vec::new();
        let mut pts: Vec<(f32, f32)> = Vec::new();
        let mut open = 0.0f32;
        let mut voiced = 0usize;
        let body = (b0..end_f).count().max(1);
        for k in b0..end_f {
            open += inp.rec(k)[3];
            let p = inp.pitch[k];
            if p.is_finite() {
                voiced += 1;
                ps.push(p);
                pts.push((k as f32 / FPS, p / 100.0));
            }
        }
        let tone = voiced as f32 / body as f32;
        let cents = if ps.len() >= 3 { Some(median(&mut ps)) } else { None };
        let bend = if pts.len() >= 4 { slope(&pts) } else { 0.0 };

        // Ending: the steepest fall over the last stretch, and a flux bump there.
        let mut end_stop = 0.0;
        {
            let a = lv.fine_index((end_t - 0.060).max(o.t));
            let b = lv.fine_index(end_t + 0.005);
            let step = 5usize;
            let mut worst = 0.0f32;
            let mut j = a;
            while j + step <= b {
                let slope = (lv.fine_db(j + step) - lv.fine_db(j)) / (step as f32 * lv.block as f32 * 1000.0 / lv.sr);
                worst = worst.min(slope);
                j += 1;
            }
            // A fall that ends at the quiet level is a stop only if it was abrupt.
            if -worst > END_STOP_DB_PER_MS && !by_onset {
                end_stop = 1.0;
            }
        }
        let mut end_click = 0.0;
        {
            let lo = end_f.saturating_sub(4).max(o.frame + 4);
            for k in lo..end_f.min(inp.flux.len()) {
                let near_onset = inp.onsets.iter().any(|x| x.frame.abs_diff(k) <= 2);
                if !near_onset && inp.flux[k] >= END_CLICK_FLUX && inp.flux[k] >= inp.flux[k - 1] && inp.flux[k] >= inp.flux.get(k + 1).copied().unwrap_or(0.0) {
                    end_click = 1.0;
                }
            }
        }

        // Strength: the event's peak level within 30 dB of the loudest in the take.
        let ev_peak = (o.frame..(o.frame + 6).min(n)).map(|k| lv.frame_db[k]).fold(f32::NEG_INFINITY, f32::max);
        let strength = ((ev_peak - (lv.peak_db - 30.0)) / 30.0).clamp(0.0, 1.0);

        // The live frame for kit-slot choice: the one the live tier flagged, else a little after.
        let lf = (o.frame..(o.frame + 5).min(n)).find(|&k| inp.rec(k)[5] > 0.0).unwrap_or((o.frame + 2).min(n - 1));
        let live = live_hit_features(&inp.rec(lf)[..BANDS + DEFAULT_BANDS], DEFAULT_BANDS);

        let shape = HitShape { attack, burst, cents, bend, hold, vowel: open / body as f32, tone, end_stop, end_click };
        let features = [(attack * 1000.0).max(0.5).ln(), burst, low, centroid_oct, tone, (hold * 1000.0).max(20.0).ln()];
        hits.push(Hit { t: o.t, dur: hold, slot: 0, strength, shape, features, live });
    }
    hits
}

/// Least-squares slope of y against x.
fn slope(pts: &[(f32, f32)]) -> f32 {
    let n = pts.len() as f32;
    let mx = pts.iter().map(|p| p.0).sum::<f32>() / n;
    let my = pts.iter().map(|p| p.1).sum::<f32>() / n;
    let (mut sxy, mut sxx) = (0.0, 0.0);
    for &(x, y) in pts {
        sxy += (x - mx) * (y - my);
        sxx += (x - mx) * (x - mx);
    }
    if sxx > 0.0 { sxy / sxx } else { 0.0 }
}

/// Hardness reading of a hit's attack, for callers that want it.
pub fn hit_hardness(h: &Hit) -> f32 {
    hardness_from_attack(Some(h.shape.attack))
}

// ------------------------------------------------------------ grouping

/// Standardize `feats` (floors applied) and weight them. Returns the points.
pub fn standardize(feats: &[[f32; 6]]) -> Vec<[f32; 6]> {
    let n = feats.len().max(1) as f32;
    let mut mean = [0.0f32; 6];
    for f in feats {
        for d in 0..6 {
            mean[d] += f[d] / n;
        }
    }
    let mut sd = [0.0f32; 6];
    for f in feats {
        for d in 0..6 {
            sd[d] += (f[d] - mean[d]).powi(2) / n;
        }
    }
    for d in 0..6 {
        sd[d] = sd[d].sqrt().max(FEATURE_FLOOR[d]);
    }
    feats.iter().map(|f| std::array::from_fn(|d| (f[d] - mean[d]) / sd[d] * FEATURE_WEIGHT[d])).collect()
}

fn dist2(a: &[f32; 6], b: &[f32; 6]) -> f32 {
    (0..6).map(|d| (a[d] - b[d]).powi(2)).sum()
}

/// Ward clustering. Returns the cluster label per point at each requested
/// count (descending), as `(count, labels)`.
pub fn ward(points: &[[f32; 6]], counts: &[usize]) -> Vec<(usize, Vec<usize>)> {
    let n = points.len();
    let mut out = Vec::new();
    if n == 0 {
        return out;
    }
    // Squared distance matrix between live clusters, Lance-Williams updates.
    let mut d = vec![0.0f32; n * n];
    for i in 0..n {
        for j in 0..n {
            d[i * n + j] = dist2(&points[i], &points[j]);
        }
    }
    let mut size = vec![1usize; n];
    let mut alive = vec![true; n];
    let mut label: Vec<usize> = (0..n).collect();
    let mut left = n;
    let mut wanted: Vec<usize> = counts.to_vec();
    wanted.sort_unstable_by(|a, b| b.cmp(a));
    let snapshot = |label: &Vec<usize>, k: usize, out: &mut Vec<(usize, Vec<usize>)>| {
        // Compact the label values to 0..k.
        let mut map: Vec<(usize, usize)> = Vec::new();
        let l: Vec<usize> = label
            .iter()
            .map(|&x| {
                if let Some(&(_, m)) = map.iter().find(|(a, _)| *a == x) {
                    m
                } else {
                    map.push((x, map.len()));
                    map.len() - 1
                }
            })
            .collect();
        out.push((k, l));
    };
    for &w in &wanted {
        while left > w.max(1) {
            // Closest pair.
            let (mut bi, mut bj, mut bd) = (0, 0, f32::INFINITY);
            for i in 0..n {
                if !alive[i] {
                    continue;
                }
                for j in i + 1..n {
                    if alive[j] && d[i * n + j] < bd {
                        bd = d[i * n + j];
                        bi = i;
                        bj = j;
                    }
                }
            }
            let (na, nb) = (size[bi] as f32, size[bj] as f32);
            for c in 0..n {
                if !alive[c] || c == bi || c == bj {
                    continue;
                }
                let nc = size[c] as f32;
                let v = ((na + nc) * d[bi * n + c] + (nb + nc) * d[bj * n + c] - nc * bd) / (na + nb + nc);
                d[bi * n + c] = v;
                d[c * n + bi] = v;
            }
            alive[bj] = false;
            size[bi] += size[bj];
            for l in label.iter_mut() {
                if *l == bj {
                    *l = bi;
                }
            }
            left -= 1;
        }
        snapshot(&label, w.max(1), &mut out);
    }
    out
}

/// Mean silhouette of a labelling over `points`.
pub fn silhouette(points: &[[f32; 6]], labels: &[usize], k: usize) -> f32 {
    let n = points.len();
    if k < 2 || n < 2 {
        return 0.0;
    }
    let mut sizes = vec![0usize; k];
    for &l in labels {
        sizes[l] += 1;
    }
    let mut total = 0.0;
    for i in 0..n {
        if sizes[labels[i]] < 2 {
            continue; // singleton clusters score 0
        }
        let mut sums = vec![0.0f32; k];
        for j in 0..n {
            if i != j {
                sums[labels[j]] += dist2(&points[i], &points[j]).sqrt();
            }
        }
        let a = sums[labels[i]] / (sizes[labels[i]] - 1) as f32;
        let b = (0..k).filter(|&c| c != labels[i] && sizes[c] > 0).map(|c| sums[c] / sizes[c] as f32).fold(f32::INFINITY, f32::min);
        let m = a.max(b);
        if m > 0.0 && b.is_finite() {
            total += (b - a) / m;
        }
    }
    total / n as f32
}

fn centres(points: &[[f32; 6]], labels: &[usize], k: usize) -> Vec<[f32; 6]> {
    let mut c = vec![[0.0f32; 6]; k];
    let mut cnt = vec![0usize; k];
    for (p, &l) in points.iter().zip(labels) {
        cnt[l] += 1;
        for d in 0..6 {
            c[l][d] += p[d];
        }
    }
    for l in 0..k {
        for d in 0..6 {
            c[l][d] /= cnt[l].max(1) as f32;
        }
    }
    c
}

/// How many kinds of sound there are, from the best silhouette over 2..=4
/// groups (1 when nothing separates well enough).
pub fn choose_kinds(points: &[[f32; 6]]) -> usize {
    if points.len() < 4 {
        return 1;
    }
    let max_k = 4.min(points.len() / 2);
    if max_k < 2 {
        return 1;
    }
    let counts: Vec<usize> = (2..=max_k).collect();
    let mut valid: Vec<(usize, f32)> = Vec::new();
    for (k, labels) in ward(points, &counts) {
        let s = silhouette(points, &labels, k);
        let c = centres(points, &labels, k);
        let mut sep = f32::INFINITY;
        for a in 0..k {
            for b in a + 1..k {
                sep = sep.min(dist2(&c[a], &c[b]).sqrt());
            }
        }
        if sep >= MIN_SEPARATION && s >= MIN_SILHOUETTE {
            valid.push((k, s));
        }
    }
    // The best silhouette favours the coarsest split that is clean (kick against everything
    // else); take the most groups whose silhouette is close to it.
    let top = valid.iter().map(|v| v.1).fold(f32::NEG_INFINITY, f32::max);
    valid.iter().filter(|v| v.1 >= top - SILHOUETTE_SLACK).map(|v| v.0).max().unwrap_or(1)
}

/// How far below the best silhouette a finer grouping may fall and still be chosen.
pub const SILHOUETTE_SLACK: f32 = 0.12;

/// Distance from a hit's features to a taught slot: weighted, spread-normalized RMS.
pub fn taught_distance(f: &[f32; 6], t: &Taught) -> f32 {
    let (mut s, mut w) = (0.0f32, 0.0f32);
    for d in 0..6 {
        let sp = t.spread[d].max(FEATURE_FLOOR[d]);
        s += FEATURE_WEIGHT[d] * ((f[d] - t.centre[d]) / sp).powi(2);
        w += FEATURE_WEIGHT[d];
    }
    (s / w).sqrt()
}

/// Assign slots. With a taught kit each hit goes to the nearest taught slot, or
/// the first free slot when none is within `TAUGHT_MAX`. Otherwise Ward
/// clustering is cut at `kinds` (0 = best silhouette) and the groups are
/// ordered by spectral centroid into slots 0.. . Returns the number of kinds used.
pub fn group(hits: &mut [Hit], kinds: usize, taught: &[Option<Taught>; 4]) -> usize {
    if hits.is_empty() {
        return 0;
    }
    if taught.iter().any(|t| t.is_some()) {
        let spare = taught.iter().position(|t| t.is_none());
        let mut used = [false; 4];
        for h in hits.iter_mut() {
            let (mut bs, mut bd) = (0usize, f32::INFINITY);
            for (s, t) in taught.iter().enumerate() {
                if let Some(t) = t {
                    let d = taught_distance(&h.features, t);
                    if d < bd {
                        bd = d;
                        bs = s;
                    }
                }
            }
            if bd > TAUGHT_MAX {
                if let Some(s) = spare {
                    bs = s;
                }
            }
            h.slot = bs as u8;
            used[bs] = true;
        }
        return used.iter().filter(|&&u| u).count();
    }
    let feats: Vec<[f32; 6]> = hits.iter().map(|h| h.features).collect();
    let pts = standardize(&feats);
    let k = if kinds == 0 { choose_kinds(&pts) } else { kinds.clamp(1, 4).min(hits.len()) };
    if k <= 1 {
        hits.iter_mut().for_each(|h| h.slot = 0);
        return 1;
    }
    let labels = ward(&pts, &[k]).pop().map(|x| x.1).unwrap_or_else(|| vec![0; hits.len()]);
    // Order groups by mean spectral centroid.
    let mut mean_c = vec![0.0f32; k];
    let mut cnt = vec![0usize; k];
    for (h, &l) in hits.iter().zip(&labels) {
        mean_c[l] += h.features[3];
        cnt[l] += 1;
    }
    let mut order: Vec<usize> = (0..k).collect();
    order.sort_by(|&a, &b| (mean_c[a] / cnt[a].max(1) as f32).total_cmp(&(mean_c[b] / cnt[b].max(1) as f32)));
    let mut slot_of = vec![0u8; k];
    for (slot, &g) in order.iter().enumerate() {
        slot_of[g] = slot as u8;
    }
    for (h, &l) in hits.iter_mut().zip(&labels) {
        h.slot = slot_of[l];
    }
    k
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(centre: [f32; 6], n: usize, spread: f32, seed: &mut u32) -> Vec<[f32; 6]> {
        (0..n)
            .map(|_| {
                std::array::from_fn(|d| {
                    *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                    centre[d] + ((*seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * spread
                })
            })
            .collect()
    }

    fn hit(f: [f32; 6]) -> Hit {
        Hit { features: f, ..Hit::default() }
    }

    #[test]
    fn ward_separates_blobs() {
        let mut s = 1;
        let mut pts = blob([0.0; 6], 10, 0.2, &mut s);
        pts.extend(blob([5.0; 6], 10, 0.2, &mut s));
        pts.extend(blob([-4.0, 4.0, -4.0, 4.0, -4.0, 4.0], 10, 0.2, &mut s));
        let r = ward(&pts, &[3, 2]);
        assert_eq!(r[0].0, 3);
        let l = &r[0].1;
        assert!(l[..10].iter().all(|&x| x == l[0]) && l[10..20].iter().all(|&x| x == l[10]) && l[20..].iter().all(|&x| x == l[20]));
        assert!(l[0] != l[10] && l[10] != l[20] && l[0] != l[20]);
        // Two groups: some pair of blobs merged.
        let l2 = &r[1].1;
        assert_eq!(l2.iter().collect::<std::collections::HashSet<_>>().len(), 2);
    }

    #[test]
    fn silhouette_is_high_for_clean_clusters_and_low_for_a_blob() {
        let mut s = 5;
        let mut pts = blob([0.0; 6], 12, 0.3, &mut s);
        pts.extend(blob([4.0; 6], 12, 0.3, &mut s));
        let labels: Vec<usize> = (0..24).map(|i| i / 12).collect();
        assert!(silhouette(&pts, &labels, 2) > 0.8);
        let blobby = blob([0.0; 6], 24, 1.0, &mut s);
        let l = ward(&blobby, &[2]).pop().unwrap().1;
        assert!(silhouette(&blobby, &l, 2) < 0.5);
        assert_eq!(silhouette(&pts, &vec![0; 24], 1), 0.0);
    }

    #[test]
    fn kinds_count_one_through_four() {
        let mut s = 11;
        for k in 1..=4usize {
            let mut feats: Vec<[f32; 6]> = Vec::new();
            for g in 0..k {
                // Each kind stands out in a different feature, by several floors.
                let mut c = [1.0f32; 6];
                c[g] += 6.0 * FEATURE_FLOOR[g];
                feats.extend(blob(c, 8, 0.04, &mut s));
            }
            let pts = standardize(&feats);
            assert_eq!(choose_kinds(&pts), k, "k = {k}");
        }
    }

    #[test]
    fn groups_are_ordered_by_centroid_into_slots() {
        let mut s = 3;
        // Three groups with centroid (feature 3) deliberately out of order in the list.
        let mut hits: Vec<Hit> = Vec::new();
        for c in [2.0f32, -2.0, 0.0] {
            for f in blob([c, c * 0.3, 0.5 - c * 0.1, c, 0.3, 4.0 + c * 0.2], 6, 0.05, &mut s) {
                hits.push(hit(f));
            }
        }
        let k = group(&mut hits, 3, &[None; 4]);
        assert_eq!(k, 3);
        assert!(hits[6..12].iter().all(|h| h.slot == 0)); // lowest centroid -> slot 0
        assert!(hits[12..].iter().all(|h| h.slot == 1));
        assert!(hits[..6].iter().all(|h| h.slot == 2));
        // kinds = 1 puts everything in slot 0.
        let k = group(&mut hits, 1, &[None; 4]);
        assert_eq!(k, 1);
        assert!(hits.iter().all(|h| h.slot == 0));
        assert_eq!(group(&mut [], 0, &[None; 4]), 0);
    }

    #[test]
    fn taught_kit_picks_nearest_and_spare() {
        let t = |c: f32| Some(Taught { centre: [c; 6], spread: [0.3; 6] });
        let taught = [t(0.0), None, t(3.0), None];
        let mut hits = vec![hit([0.1; 6]), hit([2.9; 6]), hit([-0.1; 6]), hit([9.0; 6])];
        let k = group(&mut hits, 0, &taught);
        assert_eq!(hits.iter().map(|h| h.slot).collect::<Vec<_>>(), vec![0, 2, 0, 1]);
        assert_eq!(k, 3);
        // No spare slot: the far hit goes to the nearest anyway.
        let full = [t(0.0), t(3.0), t(6.0), t(8.0)];
        let mut hits = vec![hit([20.0; 6])];
        group(&mut hits, 0, &full);
        assert_eq!(hits[0].slot, 3);
    }

    #[test]
    fn standardize_floors_keep_a_tight_cluster_tight() {
        let mut s = 2;
        let feats = blob([1.0; 6], 20, 0.01, &mut s);
        let pts = standardize(&feats);
        assert!(pts.iter().all(|p| p.iter().all(|v| v.abs() < 0.2)));
    }
}
