//! Bars from audio when the beats are known: how many beats a bar has (the meter) and which beat
//! is the first (the downbeat). The beats come from the section grid (a take placed on it) or a
//! loop's own tempo; this only groups them.
//!
//! Per beat it measures onset strength in three bands at four points through the beat, and the
//! pitch classes sounding through it. The meter is the lag, in beats, at which beats look most
//! alike. The downbeat is the beat position that most often has a low-band onset, a change of
//! harmony and a strong onset overall.

use dsp::fft::{Fft, hann};

const N: usize = 2048;
const HOP: usize = 256;

pub struct Features {
    /// Per beat: onset strength, 3 bands x 4 points through the beat.
    pub profile: Vec<[f32; 12]>,
    /// Per beat: pitch-class profile of the beat, unit length.
    pub chroma: Vec<[f32; 12]>,
    /// Per beat: low-band and full-band onset strength at the beat.
    pub low: Vec<f32>,
    pub full: Vec<f32>,
}

/// Per-frame band fluxes and chroma of `x`.
fn frames(x: &[f32], sr: f32) -> (Vec<[f32; 3]>, Vec<[f32; 12]>) {
    let fft = Fft::new(N);
    let mut win = vec![0.0f32; N];
    hann(&mut win);
    let nf = x.len() / HOP + 1;
    let bin_hz = sr / N as f32;
    let band_of = |k: usize| -> Option<usize> {
        let f = k as f32 * bin_hz;
        if f < 30.0 {
            None
        } else if f < 160.0 {
            Some(0)
        } else if f < 2000.0 {
            Some(1)
        } else if f < 12000.0 {
            Some(2)
        } else {
            None
        }
    };
    let pc_of: Vec<Option<usize>> = (0..N / 2)
        .map(|k| {
            let f = k as f32 * bin_hz;
            if !(60.0..=2000.0).contains(&f) {
                return None;
            }
            let m = 69.0 + 12.0 * (f / 440.0).log2();
            Some((m.round() as i64).rem_euclid(12) as usize)
        })
        .collect();
    let mut prev = vec![0.0f32; N / 2];
    let (mut flux, mut chroma) = (Vec::with_capacity(nf), Vec::with_capacity(nf));
    let (mut re, mut im) = (vec![0.0f32; N], vec![0.0f32; N]);
    for f in 0..nf {
        let c = f * HOP;
        for i in 0..N {
            let k = c as i64 + i as i64 - (N / 2) as i64;
            re[i] = if k >= 0 && (k as usize) < x.len() { x[k as usize] * win[i] } else { 0.0 };
            im[i] = 0.0;
        }
        fft.forward(&mut re, &mut im);
        let mut fl = [0.0f32; 3];
        let mut ch = [0.0f32; 12];
        for k in 1..N / 2 {
            let m = (re[k] * re[k] + im[k] * im[k]).sqrt();
            let l = (1.0 + 100.0 * m).ln();
            if let Some(b) = band_of(k) {
                fl[b] += (l - prev[k]).max(0.0);
            }
            if let Some(pc) = pc_of[k] {
                ch[pc] += l;
            }
            prev[k] = l;
        }
        flux.push(fl);
        chroma.push(ch);
    }
    (flux, chroma)
}

fn unit(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 1e-9 {
        for x in v.iter_mut() {
            *x /= n;
        }
    }
}

fn cos(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Features of each beat starting at `beats` (seconds), `spb` seconds apart.
pub fn features(x: &[f32], sr: f32, beats: &[f64], spb: f64) -> Features {
    let (flux, chroma) = frames(x, sr);
    let fr = |t: f64| ((t * sr as f64) / HOP as f64).round() as i64;
    let max_in = |band: usize, t0: f64, t1: f64| -> f32 {
        let (a, b) = (fr(t0).max(0), fr(t1).min(flux.len() as i64 - 1));
        (a..=b).map(|i| flux[i as usize][band]).fold(0.0, f32::max)
    };
    let w = spb / 8.0;
    let mut out = Features { profile: Vec::new(), chroma: Vec::new(), low: Vec::new(), full: Vec::new() };
    for &t in beats {
        let mut p = [0.0f32; 12];
        for band in 0..3 {
            for s in 0..4 {
                let c = t + s as f64 * spb / 4.0;
                p[band * 4 + s] = max_in(band, c - w, c + w);
            }
        }
        let mut ch = [0.0f32; 12];
        let (a, b) = (fr(t).max(0), fr(t + spb).min(chroma.len() as i64 - 1));
        for i in a..=b {
            for k in 0..12 {
                ch[k] += chroma[i as usize][k];
            }
        }
        let mean = ch.iter().sum::<f32>() / 12.0;
        for v in ch.iter_mut() {
            *v = (*v - mean).max(0.0);
        }
        unit(&mut ch);
        out.low.push(p[0]);
        out.full.push(p[0] + p[4] + p[8]);
        out.profile.push(p);
        out.chroma.push(ch);
    }
    // Scale the profiles per band over the take, so a loud low band does not decide alone.
    for band in 0..3 {
        let m = out.profile.iter().flat_map(|p| p[band * 4..band * 4 + 4].iter().copied()).fold(0.0f32, f32::max).max(1e-9);
        for p in out.profile.iter_mut() {
            for v in &mut p[band * 4..band * 4 + 4] {
                *v /= m;
            }
        }
    }
    out
}

pub struct Bars {
    pub meter: usize,
    /// Index of the first beat that is a downbeat (0..meter).
    pub phase: usize,
}

fn zscore(v: &[f32]) -> Vec<f32> {
    let n = v.len().max(1) as f32;
    let m = v.iter().sum::<f32>() / n;
    let sd = (v.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / n).sqrt().max(1e-9);
    v.iter().map(|x| (x - m) / sd).collect()
}

/// Weights of the downbeat cues: low-band onset on the beat, harmony change from the beat
/// before, overall onset on the beat, and harmony change between bar-long windows.
#[derive(Clone, Copy, Debug)]
pub struct Cues {
    pub low: f32,
    pub beat_change: f32,
    pub full: f32,
    pub bar_change: f32,
    /// Mid- and high-band onset on the beat (a snare on the backbeat counts against it).
    pub mid: f32,
    pub high: f32,
}

pub const CUES: Cues = Cues { low: 1.0, beat_change: 0.5, full: 0.5, bar_change: 1.0, mid: 0.0, high: 0.0 };

/// How much each beat looks like a downbeat from the per-beat cues.
pub fn downbeatness(f: &Features, c: &Cues) -> Vec<f32> {
    let nov: Vec<f32> = (0..f.chroma.len()).map(|b| if b == 0 { 0.0 } else { 1.0 - cos(&f.chroma[b], &f.chroma[b - 1]) }).collect();
    let (zl, zn, zf) = (zscore(&f.low), zscore(&nov), zscore(&f.full));
    let zm = zscore(&f.profile.iter().map(|p| p[4]).collect::<Vec<_>>());
    let zh = zscore(&f.profile.iter().map(|p| p[8]).collect::<Vec<_>>());
    (0..f.low.len()).map(|b| c.low * zl[b] + c.beat_change * zn[b] + c.full * zf[b] + c.mid * zm[b] + c.high * zh[b]).collect()
}

/// The downbeat, given the meter: the beat position whose beats score highest, plus how much the
/// harmony changes between bar-long windows starting there (chords tend to change on the bar).
pub fn phase_with(f: &Features, meter: usize, c: &Cues) -> usize {
    let d = downbeatness(f, c);
    let n = f.chroma.len();
    let bar_chroma = |start: usize| -> [f32; 12] {
        let mut v = [0.0f32; 12];
        for b in start..(start + meter).min(n) {
            for k in 0..12 {
                v[k] += f.chroma[b][k];
            }
        }
        unit(&mut v);
        v
    };
    let mut change = vec![0.0f32; meter];
    for (p, ch) in change.iter_mut().enumerate() {
        let starts: Vec<usize> = (p..n).step_by(meter).filter(|&s| s + meter <= n).collect();
        let v: Vec<f32> = starts.windows(2).map(|w| 1.0 - cos(&bar_chroma(w[0]), &bar_chroma(w[1]))).collect();
        *ch = v.iter().sum::<f32>() / v.len().max(1) as f32;
    }
    // Scaled by how much it varies over beat positions, with a floor so a take that never
    // changes chord does not have its noise blown up.
    let cm = change.iter().sum::<f32>() / meter as f32;
    let csd = (change.iter().map(|x| (x - cm) * (x - cm)).sum::<f32>() / meter as f32).sqrt().max(0.02);
    // Beat 0 has no harmony before it to change from, so its beat novelty is left out.
    (0..meter)
        .map(|p| {
            let v: Vec<f32> = (p..d.len()).step_by(meter).filter(|&b| b > 0).map(|b| d[b]).collect();
            let s = v.iter().sum::<f32>() / v.len().max(1) as f32 + c.bar_change * (change[p] - cm) / csd;
            (p, s)
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|x| x.0)
        .unwrap_or(0)
}

pub fn phase(f: &Features, meter: usize) -> usize {
    phase_with(f, meter, &CUES)
}

/// Meter and downbeat. `meters` lists the meters allowed (2 to 7).
pub fn detect(f: &Features, meters: &[usize]) -> Bars {
    let n = f.profile.len();
    let mut sim = [0.0f32; 6];
    for lag in 2..=7usize {
        let (mut s, mut c) = (0.0f32, 0usize);
        for b in 0..n.saturating_sub(lag) {
            let mut a = f.profile[b].to_vec();
            let mut z = f.profile[b + lag].to_vec();
            unit(&mut a);
            unit(&mut z);
            s += cos(&a, &z) + 0.5 * cos(&f.chroma[b], &f.chroma[b + lag]);
            c += 1;
        }
        sim[lag - 2] = if c > 0 { s / c as f32 } else { 0.0 };
    }
    // The best lag, with two folds: a pattern that repeats every 2 beats is read as 4 (rock
    // drums), and one that repeats every 3 beats is not read as 6 unless 6 is clearly better.
    let score = |m: usize| sim[m - 2] + if m == 4 { 0.01 } else { 0.0 };
    let mut meter = *meters.iter().max_by(|a, b| score(**a).total_cmp(&score(**b))).unwrap_or(&4);
    if meter == 2 && meters.contains(&4) && sim[2] >= sim[0] - 0.03 {
        meter = 4;
    }
    if meter == 6 && meters.contains(&3) && sim[1] >= sim[4] - 0.02 {
        meter = 3;
    }
    Bars { meter, phase: phase(f, meter) }
}
