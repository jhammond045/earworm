//! Causal short-time spectra: the last `n` samples, windowed, every hop.

use crate::hs::Det;
use dsp::fft::Fft;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Window {
    /// Symmetric Hann. Its weight is centred n/2 samples back.
    Hann,
    /// Long Hann rise over 7n/8, short Hann fall over the newest n/8, so the
    /// newest samples count more and a new note shows sooner.
    Asym,
}

const KERNEL_RES: usize = 16;
const KERNEL_BINS: usize = 4;

pub struct Stft {
    pub n: usize,
    pub bin_hz: f32,
    win: Vec<f32>,
    norm: f32,
    fft: Fft,
    re: Vec<f32>,
    im: Vec<f32>,
    /// Magnitude, bins 0..n/2, scaled so a sinusoid of amplitude A peaks near A.
    pub mag: Vec<f32>,
    /// |W| of the window at fractional bin offsets, normalized to 1 at 0.
    kernel: Vec<f32>,
    /// Centre of mass of the window, seconds before the frame's last sample.
    pub centre_s: f32,
}

impl Stft {
    pub fn new(n: usize, sr: f32, shape: Window) -> Self {
        let mut win = vec![0.0f32; n];
        match shape {
            Window::Hann => dsp::fft::hann(&mut win),
            Window::Asym => {
                let r = n / 8;
                let rise = n - r;
                for (i, w) in win.iter_mut().enumerate() {
                    *w = if i < rise {
                        0.5 - 0.5 * (std::f32::consts::PI * i as f32 / rise as f32).cos()
                    } else {
                        0.5 + 0.5 * (std::f32::consts::PI * (i - rise) as f32 / r as f32).cos()
                    };
                }
            }
        }
        let sum: f32 = win.iter().sum();
        let com: f32 = win.iter().enumerate().map(|(i, w)| i as f32 * w).sum::<f32>() / sum;
        let kernel = (0..=KERNEL_BINS * KERNEL_RES)
            .map(|d| {
                let delta = d as f64 / KERNEL_RES as f64;
                let (mut a, mut b) = (0.0f64, 0.0f64);
                for (i, &w) in win.iter().enumerate() {
                    let ph = -2.0 * std::f64::consts::PI * delta * i as f64 / n as f64;
                    a += w as f64 * ph.cos();
                    b += w as f64 * ph.sin();
                }
                (a.hypot(b) / sum as f64) as f32
            })
            .collect();
        Self {
            n,
            bin_hz: sr / n as f32,
            win,
            norm: 2.0 / sum,
            fft: Fft::new(n),
            re: vec![0.0; n],
            im: vec![0.0; n],
            mag: vec![0.0; n / 2],
            kernel,
            centre_s: (n as f32 - com) / sr,
        }
    }

    /// Spectrum of the `n` samples ending just before `end` (zeros before the start).
    pub fn frame(&mut self, x: &[f32], end: usize) {
        let n = self.n;
        for i in 0..n {
            let idx = end as isize - n as isize + i as isize;
            self.re[i] = if idx >= 0 { x[idx as usize] * self.win[i] } else { 0.0 };
            self.im[i] = 0.0;
        }
        self.fft.forward(&mut self.re, &mut self.im);
        for k in 0..n / 2 {
            self.mag[k] = self.re[k].hypot(self.im[k]) * self.norm;
        }
    }

    /// Window magnitude `delta` bins from a sinusoid's peak (0 past 4 bins).
    pub fn kernel(&self, delta: f32) -> f32 {
        let x = delta.abs() * KERNEL_RES as f32;
        let i = x as usize;
        if i >= KERNEL_BINS * KERNEL_RES {
            return 0.0;
        }
        let f = x - i as f32;
        self.kernel[i] * (1.0 - f) + self.kernel[i + 1] * f
    }
}

pub fn midi_hz(m: f32) -> f32 {
    440.0 * ((m - 69.0) / 12.0).exp2()
}

/// Refine an f0 guess from the peaks of the first six partials (parabolic
/// interpolation on log magnitude), weighted by their level.
pub fn refine_f0(mag: &[f32], bin_hz: f32, f0: f32) -> f32 {
    let (mut sw, mut sf) = (0.0f32, 0.0f32);
    for m in 1..=6 {
        let target = m as f32 * f0 / bin_hz;
        let k0 = target.round() as usize;
        if k0 < 2 || k0 + 2 >= mag.len() {
            continue;
        }
        let k = (k0 - 1..=k0 + 1).max_by(|&a, &b| mag[a].total_cmp(&mag[b])).unwrap();
        let (a, b, c) = (mag[k - 1].max(1e-9).ln(), mag[k].max(1e-9).ln(), mag[k + 1].max(1e-9).ln());
        let den = a - 2.0 * b + c;
        let p = if den < 0.0 { (0.5 * (a - c) / den).clamp(-0.5, 0.5) } else { 0.0 };
        let f = (k as f32 + p) * bin_hz / m as f32;
        if (f / f0 - 1.0).abs() < 0.03 {
            sw += mag[k];
            sf += mag[k] * f;
        }
    }
    if sw > 0.0 { sf / sw } else { f0 }
}

/// A note's level in the raw spectrum: partials `from..=to`, each the
/// largest bin within one of where it should be.
pub fn note_level(mag: &[f32], bin_hz: f32, f0: f32, from: usize, to: usize) -> f32 {
    let mut e = 0.0f32;
    for m in from..=to {
        let k = (m as f32 * f0 / bin_hz).round() as usize;
        if k < 1 || k + 1 >= mag.len() {
            break;
        }
        let a = mag[k - 1].max(mag[k]).max(mag[k + 1]);
        e += a * a;
    }
    e.sqrt()
}

/// Broadband onsets: summed rise of log magnitude (bins up to 5 kHz) over the
/// frame two hops back, against a running mean plus four deviations.
pub struct Flux {
    prev: Vec<Vec<f32>>,
    head: usize,
    mean: f32,
    dev: f32,
    hold: u32,
}

impl Flux {
    pub fn new(bins: usize) -> Self {
        Self { prev: vec![vec![-120.0; bins]; 2], head: 0, mean: 0.0, dev: 0.0, hold: 0 }
    }
    pub fn step(&mut self, mag: &[f32]) -> bool {
        let n = self.prev[0].len();
        let old = &self.prev[self.head];
        let mut flux = 0.0f32;
        for k in 1..n {
            let db = 20.0 * mag[k].max(1e-6).log10();
            flux += (db - old[k].max(-80.0)).max(0.0) * if db > -80.0 { 1.0 } else { 0.0 };
        }
        flux /= n as f32;
        for k in 0..n {
            self.prev[self.head][k] = 20.0 * mag[k].max(1e-6).log10();
        }
        self.head = 1 - self.head;
        let thr = self.mean + 4.0 * self.dev + 0.5;
        let hit = self.hold == 0 && flux > thr;
        if hit {
            self.hold = 4;
        } else if self.hold > 0 {
            self.hold -= 1;
        }
        let a = 0.05;
        self.mean += a * (flux - self.mean);
        self.dev += a * ((flux - self.mean).abs() - self.dev);
        hit
    }
}

/// Peak magnitude of partial `m` of `f0` (the largest bin within one).
fn partial(mag: &[f32], bin_hz: f32, f0: f32, m: usize) -> f32 {
    let k = (m as f32 * f0 / bin_hz).round() as usize;
    if k < 1 || k + 1 >= mag.len() { 0.0 } else { mag[k - 1].max(mag[k]).max(mag[k + 1]) }
}

/// Octave checks by spectral smoothness, on one frame's pitches. `e(m)` is
/// how far pitch m's even partials (2, 4, 6) stand above the mean of their
/// odd neighbours.
/// - Both m and m + 12 found: keep m + 12 only if e(m) >= `keep`; otherwise
///   m's own even partials explain it.
/// - m found without m + 12, and e(m) >= `lift`: m's odd partials are too
///   weak for m to be sounding, so it is really m + 12.
/// Zero turns a rule off.
pub fn octave_check(mag: &[f32], bin_hz: f32, dets: &mut Vec<Det>, keep: f32, lift: f32) {
    let e = |m: u8| {
        let f0 = midi_hz(m as f32);
        let (mut ev, mut od) = (0.0f32, 0.0f32);
        for h in [2usize, 4, 6] {
            ev += partial(mag, bin_hz, f0, h);
            od += 0.5 * (partial(mag, bin_hz, f0, h - 1) + partial(mag, bin_hz, f0, h + 1));
        }
        ev / od.max(1e-9)
    };
    let has = |dets: &Vec<Det>, m: u8| dets.iter().any(|d| d.midi == m);
    if keep > 0.0 {
        let drop: Vec<u8> = dets.iter().filter(|d| has(dets, d.midi + 12) && e(d.midi) < keep).map(|d| d.midi + 12).collect();
        dets.retain(|d| !drop.contains(&d.midi));
    }
    if lift > 0.0 {
        for i in 0..dets.len() {
            let m = dets[i].midi;
            if m + 12 <= 100 && !has(dets, m + 12) && e(m) >= lift {
                dets[i].midi = m + 12;
            }
        }
    }
}
