//! Measurements for the report: anchor accuracy, landing accuracy, spectral distance to the
//! ideal render, pitch, transients.

use dsp::fft::{Fft, hann};

pub struct Stats {
    pub n: usize,
    pub mean: f64,
    pub p95: f64,
    pub max: f64,
    pub bias: f64,
}

pub fn stats(errs_ms: &[f64]) -> Stats {
    if errs_ms.is_empty() {
        return Stats { n: 0, mean: f64::NAN, p95: f64::NAN, max: f64::NAN, bias: f64::NAN };
    }
    let mut a: Vec<f64> = errs_ms.iter().map(|e| e.abs()).collect();
    a.sort_by(|x, y| x.total_cmp(y));
    let n = a.len();
    Stats {
        n,
        mean: a.iter().sum::<f64>() / n as f64,
        p95: a[((n as f64 * 0.95).ceil() as usize).clamp(1, n) - 1],
        max: a[n - 1],
        bias: errs_ms.iter().sum::<f64>() / n as f64,
    }
}

/// One-to-one matching of found times to true times within `tol` seconds, nearest pairs first.
/// Returns (signed errors in ms, found - true, for each matched true time; missed; extra).
pub fn match_times(found: &[f64], truth: &[f64], tol: f64) -> (Vec<f64>, usize, usize) {
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (i, f) in found.iter().enumerate() {
        for (j, t) in truth.iter().enumerate() {
            let d = (f - t).abs();
            if d <= tol {
                pairs.push((d, i, j));
            }
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (mut uf, mut ut) = (vec![false; found.len()], vec![false; truth.len()]);
    let mut errs = Vec::new();
    for (_, i, j) in pairs {
        if !uf[i] && !ut[j] {
            uf[i] = true;
            ut[j] = true;
            errs.push((found[i] - truth[j]) * 1000.0);
        }
    }
    let matched = errs.len();
    (errs, truth.len() - matched, found.len() - matched)
}

/// Spectral distance in dB between two signals: the RMS difference of third-octave band levels
/// from 100 Hz to 8 kHz, over frames where `b` is within 50 dB of its loudest frame. Bands
/// rather than bins, so vibrato and noise detail count less than level, timing and smearing.
pub fn lsd(a: &[f32], b: &[f32], sr: f32) -> f64 {
    let n = 2048;
    let hop = 512;
    let fft = Fft::new(n);
    let mut win = vec![0.0f32; n];
    hann(&mut win);
    let edges: Vec<usize> = (0..=19).map(|i| (100.0 * 2f32.powf(i as f32 / 3.0) / sr * n as f32).round() as usize).collect();
    let len = a.len().min(b.len());
    let spec = |x: &[f32], start: usize, out: &mut Vec<f32>| {
        let (mut re, mut im) = (vec![0.0f32; n], vec![0.0f32; n]);
        for i in 0..n {
            re[i] = x.get(start + i).copied().unwrap_or(0.0) * win[i];
        }
        fft.forward(&mut re, &mut im);
        out.clear();
        for w in edges.windows(2) {
            let p: f32 = (w[0]..w[1].max(w[0] + 1)).map(|k| re[k] * re[k] + im[k] * im[k]).sum();
            out.push(10.0 * (p + 1e-10).log10());
        }
    };
    let energy = |x: &[f32], s: usize| -> f32 { x[s..(s + n).min(x.len())].iter().map(|v| v * v).sum() };
    let frames: Vec<usize> = (0..len.saturating_sub(n)).step_by(hop).collect();
    let peak = frames.iter().map(|&s| energy(b, s)).fold(0.0f32, f32::max);
    let (mut sa, mut sb) = (Vec::new(), Vec::new());
    let (mut sum, mut cnt) = (0.0f64, 0usize);
    for &s in &frames {
        if energy(b, s) < peak * 1e-5 {
            continue;
        }
        spec(a, s, &mut sa);
        spec(b, s, &mut sb);
        let d: f64 = sa.iter().zip(&sb).map(|(x, y)| ((x - y) as f64).powi(2)).sum::<f64>() / sa.len() as f64;
        sum += d.sqrt();
        cnt += 1;
    }
    sum / cnt.max(1) as f64
}

/// Median pitch in cents over [t0, t1) seconds, from YIN on 40 ms frames every 10 ms.
pub fn pitch_cents(x: &[f32], sr: f32, t0: f64, t1: f64) -> Option<f32> {
    let max_lag = (sr / 70.0).ceil() as usize;
    let win = (0.03 * sr) as usize;
    let len = win + max_lag;
    let mut scratch = vec![0.0f32; max_lag];
    let mut got = Vec::new();
    let mut t = t0;
    while t < t1 {
        let s = (t * sr as f64) as usize;
        if s + len < x.len() {
            let e = dsp::yin::yin(&x[s..s + len], sr, 70.0, 1000.0, 0.15, &mut scratch);
            if e.hz > 0.0 && e.confidence > 0.7 {
                got.push(dsp::hz_to_cents(e.hz));
            }
        }
        t += 0.01;
    }
    if got.is_empty() {
        return None;
    }
    got.sort_by(|a, b| a.total_cmp(b));
    Some(got[got.len() / 2])
}

/// Peak level in the 10 ms after `t` (attack), and RMS in the 15 ms to 2 ms before it
/// (anything there is a smeared or early copy of the attack), both in dB.
pub fn attack_db(x: &[f32], sr: f32, t: f64) -> (f32, f32) {
    let s = (t * sr as f64) as i64;
    let get = |i: i64| if i >= 0 && (i as usize) < x.len() { x[i as usize] } else { 0.0 };
    let ms = |m: f64| (m * sr as f64 / 1000.0) as i64;
    let peak = (s - ms(1.0)..s + ms(10.0)).map(|i| get(i).abs()).fold(0.0f32, f32::max);
    let pre: f32 = (s - ms(15.0)..s - ms(2.0)).map(|i| get(i) * get(i)).sum::<f32>() / ms(13.0) as f32;
    (dsp::lin_to_db(peak), 10.0 * (pre + 1e-12).log10())
}

/// For each hard warp point (output sample, take sample) other than the two ends: the lag in ms,
/// within 30 ms, at which the output best matches the take's audio from 2 ms before the anchor
/// to 20 ms after it, and the mean normalized correlation at those lags. A stretcher that copies
/// attacks verbatim gives lag 0 and correlation 1.
pub fn attack_offsets(input: &[f32], out: &[f32], map: &[(f64, f64, bool)], sr: f32) -> (Vec<f64>, f64) {
    let ms = |m: f64| (m * sr as f64 / 1000.0) as i64;
    let (pre, len, reach) = (ms(2.0), ms(22.0), ms(30.0));
    let get = |x: &[f32], i: i64| if i >= 0 && (i as usize) < x.len() { x[i as usize] } else { 0.0 };
    let mut lags = Vec::new();
    let mut corr_sum = 0.0f64;
    for &(u, s, hard) in &map[1..map.len() - 1] {
        if !hard {
            continue;
        }
        let (u, s) = (u.round() as i64, s.round() as i64);
        let xs: Vec<f32> = (0..len).map(|i| get(input, s - pre + i)).collect();
        let xx: f32 = xs.iter().map(|v| v * v).sum();
        let score = |lag: i64| -> f32 {
            let (mut xy, mut yy) = (0.0f32, 1e-12f32);
            for (i, &x) in xs.iter().enumerate() {
                let y = get(out, u - pre + lag + i as i64);
                xy += x * y;
                yy += y * y;
            }
            xy / (xx * yy).sqrt().max(1e-12)
        };
        let mut best = (f32::NEG_INFINITY, 0i64);
        let mut lag = -reach;
        while lag <= reach {
            let c = score(lag);
            if c > best.0 {
                best = (c, lag);
            }
            lag += 4;
        }
        let centre = best.1;
        for lag in centre - 4..=centre + 4 {
            let c = score(lag);
            if c > best.0 {
                best = (c, lag);
            }
        }
        lags.push(best.1 as f64 * 1000.0 / sr as f64);
        corr_sum += best.0 as f64;
    }
    let n = lags.len().max(1) as f64;
    (lags, corr_sum / n)
}
