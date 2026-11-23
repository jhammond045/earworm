//! How a remapped render compares with the ideal one (the events re-sequenced and rendered).

use crate::groove::SR;
use dsp::fft::{Fft, hann};

/// Spectral distance in dB (as experiments/flexquant): the RMS difference of third-octave band
/// levels from 100 Hz to 8 kHz in 2,048-sample frames every 512, over frames where `b` is within
/// 50 dB of its loudest frame and whose start passes `keep`.
pub fn lsd_where(a: &[f32], b: &[f32], keep: impl Fn(usize) -> bool) -> f64 {
    let n = 2048;
    let hop = 512;
    let fft = Fft::new(n);
    let mut win = vec![0.0f32; n];
    hann(&mut win);
    let edges: Vec<usize> = (0..=19).map(|i| (100.0 * 2f32.powf(i as f32 / 3.0) / SR * n as f32).round() as usize).collect();
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
        if !keep(s) || energy(b, s) < peak * 1e-5 {
            continue;
        }
        spec(a, s, &mut sa);
        spec(b, s, &mut sb);
        let d: f64 = sa.iter().zip(&sb).map(|(x, y)| ((x - y) as f64).powi(2)).sum::<f64>() / sa.len() as f64;
        sum += d.sqrt();
        cnt += 1;
    }
    if cnt == 0 { f64::NAN } else { sum / cnt as f64 }
}

fn db(x: f64) -> f64 {
    10.0 * x.max(1e-20).log10()
}

fn energy(x: &[f32], a: i64, b: i64) -> f64 {
    (a.max(0)..b.min(x.len() as i64)).map(|i| (x[i as usize] as f64).powi(2)).sum()
}

/// A crude high band: the second difference (+12 dB per octave), enough to show a click.
fn hf(x: &[f32]) -> Vec<f32> {
    (0..x.len()).map(|i| if i < 2 { 0.0 } else { x[i] - 2.0 * x[i - 1] + x[i - 2] }).collect()
}

pub struct JoinStats {
    /// Mean and worst high-band excess over the ideal in the 8 ms before each join, dB.
    pub click_mean: f64,
    pub click_max: f64,
    /// Residual (out - ideal) against the ideal over the quarter second after each join, dB,
    /// energy-averaged over joins.
    pub resid_join: f64,
    /// The same over the whole render up to `end`.
    pub resid_all: f64,
}

pub fn joins(out: &[f32], ideal: &[f32], joins: &[i64], end: usize) -> JoinStats {
    let (ho, hi) = (hf(out), hf(ideal));
    let pre = (0.008 * SR) as i64;
    let post = (0.25 * SR) as i64;
    let clicks: Vec<f64> = joins.iter().map(|&u| db(energy(&ho, u - pre, u + 1) / energy(&hi, u - pre, u + 1).max(1e-12))).collect();
    let diff: Vec<f32> = out.iter().zip(ideal).map(|(a, b)| a - b).collect();
    let (mut re, mut ie) = (0.0, 0.0);
    for &u in joins {
        re += energy(&diff, u, u + post);
        ie += energy(ideal, u, u + post);
    }
    let n = clicks.len().max(1) as f64;
    JoinStats {
        click_mean: clicks.iter().sum::<f64>() / n,
        click_max: clicks.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        resid_join: db(re / ie.max(1e-20)),
        resid_all: db(energy(&diff, 0, end as i64) / energy(ideal, 0, end as i64)),
    }
}

/// Near a join: a frame starting within 10 ms before to 150 ms after one.
pub fn near_join(joins: &[i64]) -> impl Fn(usize) -> bool + '_ {
    move |s: usize| {
        let c = s as i64 + 1024;
        joins.iter().any(|&u| c >= u - (0.010 * SR) as i64 && c <= u + (0.150 * SR) as i64)
    }
}
