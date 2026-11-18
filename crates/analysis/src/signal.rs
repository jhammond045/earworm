//! Level curves, a fine envelope and the onset detector the second pass builds
//! on. Everything here reads the take's audio directly, with frame k centred
//! on k / 100 s as in `pyin`.

use dsp::fft::{Fft, hann};

/// Frames at 100 per second, as everywhere in the capture pipeline.
pub const FPS: f32 = 100.0;

/// Level curves of one take.
pub struct Levels {
    pub n: usize,
    /// Power level per frame in dB (20 ms window centred on the frame).
    pub frame_db: Vec<f32>,
    /// Fine envelope, one RMS value per millisecond-ish block (window of two blocks).
    pub fine: Vec<f32>,
    /// Samples per fine block.
    pub block: usize,
    pub sr: f32,
    /// A quiet-level estimate: 10th percentile of `frame_db`.
    pub floor_db: f32,
    /// Loudest frame level.
    pub peak_db: f32,
}

pub fn power_db(p: f32) -> f32 {
    10.0 * p.max(1e-12).log10()
}

impl Levels {
    pub fn new(audio: &[f32], sr: f32, n: usize) -> Self {
        let mut ps = Vec::with_capacity(audio.len() + 1);
        let mut acc = 0.0f64;
        ps.push(0.0f64);
        for &v in audio {
            let v = if v.is_finite() { v as f64 } else { 0.0 };
            acc += v * v;
            ps.push(acc);
        }
        let l = sr as f64 / FPS as f64;
        let mean_sq = |a: f64, b: f64| -> f32 {
            let a = (a.round().max(0.0) as usize).min(audio.len());
            let b = (b.round().max(0.0) as usize).min(audio.len());
            if b <= a { 0.0 } else { ((ps[b] - ps[a]) / (b - a) as f64) as f32 }
        };
        let frame_db: Vec<f32> = (0..n).map(|k| power_db(mean_sq((k as f64 - 1.0) * l, (k as f64 + 1.0) * l))).collect();
        let block = ((sr / 1000.0).round() as usize).max(1);
        let nb = audio.len() / block;
        let fine: Vec<f32> = (0..nb)
            .map(|j| {
                let a = j * block;
                let b = ((j + 2) * block).min(audio.len());
                ((ps[b] - ps[a]) / (b - a).max(1) as f64).sqrt() as f32
            })
            .collect();
        let mut sorted = frame_db.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let floor_db = sorted.get(sorted.len() / 10).copied().unwrap_or(-100.0);
        let peak_db = sorted.last().copied().unwrap_or(-100.0);
        Self { n, frame_db, fine, block, sr, floor_db, peak_db }
    }

    /// Seconds at the centre of fine block `j`.
    pub fn fine_time(&self, j: usize) -> f32 {
        ((j + 1) * self.block) as f32 / self.sr
    }

    /// Fine block holding time `t` (seconds), clamped.
    pub fn fine_index(&self, t: f32) -> usize {
        let j = (t * self.sr / self.block as f32 - 1.0).round();
        (j.max(0.0) as usize).min(self.fine.len().saturating_sub(1))
    }

    /// Fine envelope in dB (amplitude).
    pub fn fine_db(&self, j: usize) -> f32 {
        20.0 * self.fine[j].max(1e-6).log10()
    }
}

/// Log-band spectral flux per frame, with the max filter over neighbouring
/// bands and a two-frame lag that keeps vibrato and steady tones from
/// registering ("SuperFlux"). Output is the mean positive rise in dB.
pub fn flux(audio: &[f32], sr: f32, n: usize) -> Vec<f32> {
    let mut size = 256usize;
    while (size as f32) < 0.02 * sr {
        size *= 2;
    }
    let fft = Fft::new(size);
    let mut win = vec![0.0f32; size];
    hann(&mut win);
    let nb = 32usize;
    let lo = 60.0f32;
    let hi = (0.45 * sr).min(14_000.0);
    let edge = |b: usize| -> usize { ((lo * (hi / lo).powf(b as f32 / nb as f32)) * size as f32 / sr).round() as usize };
    let mut bins: Vec<(usize, usize)> = Vec::with_capacity(nb);
    let mut prev = edge(0).max(1);
    for b in 0..nb {
        let e = edge(b + 1).max(prev + 1).min(size / 2);
        bins.push((prev.min(size / 2 - 1), e.max(prev.min(size / 2 - 1) + 1)));
        prev = e;
    }
    let l = sr / FPS;
    let mut re = vec![0.0f32; size];
    let mut im = vec![0.0f32; size];
    let mut db = vec![[0.0f32; 32]; n];
    for k in 0..n {
        let centre = (k as f32 * l) as isize;
        let start = centre - (size / 2) as isize;
        for j in 0..size {
            let idx = start + j as isize;
            let v = if idx >= 0 && (idx as usize) < audio.len() { audio[idx as usize] } else { 0.0 };
            re[j] = if v.is_finite() { v * win[j] } else { 0.0 };
            im[j] = 0.0;
        }
        fft.forward(&mut re, &mut im);
        for (b, &(a, e)) in bins.iter().enumerate() {
            let mut p = 0.0f32;
            for i in a..e {
                p += re[i] * re[i] + im[i] * im[i];
            }
            // Normalise by the window energy so levels are comparable across sizes.
            db[k][b] = power_db(p / (size as f32 * size as f32) * 4.0).max(-110.0);
        }
    }
    let mut out = vec![0.0f32; n];
    for k in 2..n {
        let mut s = 0.0f32;
        for b in 0..nb {
            let mut r = db[k - 2][b];
            if b > 0 {
                r = r.max(db[k - 2][b - 1]);
            }
            if b + 1 < nb {
                r = r.max(db[k - 2][b + 1]);
            }
            s += (db[k][b] - r).max(0.0);
        }
        out[k] = s / nb as f32;
    }
    out
}

/// One detected onset.
#[derive(Clone, Copy, Debug)]
pub struct Onset {
    /// Frame index nearest the attack start.
    pub frame: usize,
    /// Refined attack start in seconds.
    pub t: f32,
    /// Flux peak, dB.
    pub flux: f32,
}

/// Flux needed for an onset, dB of mean rise across the bands.
pub const ONSET_FLUX: f32 = 2.5;
/// Onsets closer than this many frames keep only the stronger one.
pub const ONSET_GAP: usize = 5;
/// An onset must sit this far above the take's quiet level.
pub const ONSET_ABOVE_FLOOR_DB: f32 = 10.0;

/// Peaks of `flux` that clear `ONSET_FLUX`, a level check and the minimum gap,
/// each refined to the attack start on the fine envelope.
pub fn detect_onsets(lv: &Levels, flux: &[f32]) -> Vec<Onset> {
    let n = flux.len().min(lv.n);
    let mut cand: Vec<(usize, f32)> = Vec::new();
    for k in 2..n {
        let f = flux[k];
        if f < ONSET_FLUX {
            continue;
        }
        let lo = k.saturating_sub(2);
        let hi = (k + 2).min(n - 1);
        if (lo..=hi).any(|j| flux[j] > f || (flux[j] == f && j < k)) {
            continue;
        }
        // The level must come up out of the quiet and actually rise.
        let after = lv.frame_db[(k + 3).min(n - 1)].max(lv.frame_db[(k + 2).min(n - 1)]);
        let before = (k.saturating_sub(6)..k).map(|j| lv.frame_db[j]).fold(f32::INFINITY, f32::min);
        if after < lv.floor_db + ONSET_ABOVE_FLOOR_DB || after - before < 1.5 {
            continue;
        }
        cand.push((k, f));
    }
    // Minimum gap, strongest first.
    let mut order: Vec<usize> = (0..cand.len()).collect();
    order.sort_by(|&a, &b| cand[b].1.total_cmp(&cand[a].1));
    let mut keep = vec![false; cand.len()];
    let mut taken: Vec<usize> = Vec::new();
    for i in order {
        if taken.iter().all(|&k| k.abs_diff(cand[i].0) >= ONSET_GAP) {
            keep[i] = true;
            taken.push(cand[i].0);
        }
    }
    cand.iter()
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(&(k, f), _)| {
            let t = refine_onset(lv, k as f32 / FPS);
            Onset { frame: ((t * FPS).round().max(0.0) as usize).min(n - 1), t, flux: f }
        })
        .collect()
}

/// Attack start near `t`: the first point on the fine envelope that has risen
/// 20% of the way (in dB) from the quiet before it to the peak after it.
pub fn refine_onset(lv: &Levels, t: f32) -> f32 {
    if lv.fine.is_empty() {
        return t;
    }
    let a = lv.fine_index((t - 0.040).max(0.0));
    let b = lv.fine_index(t + 0.030);
    if b <= a + 2 {
        return t;
    }
    let mid = lv.fine_index(t);
    let lo_idx = (a..=mid).min_by(|&x, &y| lv.fine[x].total_cmp(&lv.fine[y])).unwrap_or(a);
    let lo = lv.fine_db(lo_idx);
    let hi = (lo_idx..=b).map(|j| lv.fine_db(j)).fold(f32::NEG_INFINITY, f32::max);
    if hi - lo < 3.0 {
        return t;
    }
    let level = lo + 0.2 * (hi - lo);
    for j in lo_idx..=b {
        if lv.fine_db(j) >= level {
            return lv.fine_time(j);
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn burst_train(sr: f32, times: &[f32], len: f32) -> Vec<f32> {
        let n = (len * sr) as usize;
        let mut s = 7u32;
        let mut x = vec![0.0f32; n];
        for v in x.iter_mut() {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            *v = ((s >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.0004;
        }
        for &t in times {
            let a = (t * sr) as usize;
            for i in 0..(0.08 * sr) as usize {
                if a + i < n {
                    s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                    let w = (s >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
                    x[a + i] += w * (-(i as f32) / (0.02 * sr)).exp();
                }
            }
        }
        x
    }

    #[test]
    fn finds_noise_bursts_at_their_times() {
        let sr = 48_000.0;
        let times = [0.3f32, 0.8, 1.4, 1.65];
        let x = burst_train(sr, &times, 2.2);
        let n = (x.len() as f32 / sr * FPS) as usize;
        let lv = Levels::new(&x, sr, n);
        let f = flux(&x, sr, n);
        let on = detect_onsets(&lv, &f);
        assert_eq!(on.len(), times.len(), "{:?}", on.iter().map(|o| o.t).collect::<Vec<_>>());
        for (o, &t) in on.iter().zip(&times) {
            assert!((o.t - t).abs() < 0.02, "{} vs {t}", o.t);
        }
    }

    #[test]
    fn steady_tone_with_vibrato_has_one_onset() {
        let sr = 48_000.0;
        let n_s = (2.0 * sr) as usize;
        let mut ph = 0.0f32;
        let x: Vec<f32> = (0..n_s)
            .map(|i| {
                let t = i as f32 / sr;
                ph += (220.0 * (1.0 + 0.02 * (std::f32::consts::TAU * 5.5 * t).sin())) / sr;
                let env = ((t - 0.3) / 0.02).clamp(0.0, 1.0);
                0.2 * env * (std::f32::consts::TAU * ph).sin()
            })
            .collect();
        let n = (x.len() as f32 / sr * FPS) as usize;
        let lv = Levels::new(&x, sr, n);
        let on = detect_onsets(&lv, &flux(&x, sr, n));
        assert_eq!(on.len(), 1, "{on:?}");
        assert!((on[0].t - 0.3).abs() < 0.03);
    }

    #[test]
    fn levels_are_sane() {
        let sr = 48_000.0;
        let x: Vec<f32> = (0..48_000).map(|i| if i > 24_000 { 0.1 * (i as f32 * 0.05).sin() } else { 0.0 }).collect();
        let lv = Levels::new(&x, sr, 100);
        assert!(lv.frame_db[10] < -100.0 + 1.0);
        assert!((lv.frame_db[80] - power_db(0.005)).abs() < 1.0, "{}", lv.frame_db[80]);
        assert!(lv.fine.len() > 900);
    }
}
