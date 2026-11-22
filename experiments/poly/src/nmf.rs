//! Template decomposition: non-negative matrix factorization with a fixed
//! dictionary. Each frame's (compressed) magnitude spectrum V is explained as
//! W h, where W holds two harmonic templates per semitone (a bright and a dark
//! partial slope, each partial drawn with the window's own peak shape) and h
//! is solved by KL-divergence multiplicative updates with an L1 penalty,
//! warm-started from the last frame. A pitch is present when its activation
//! carries enough of the frame. The templates are generic: nothing is learned
//! from the test guitar.

use crate::hs::{Det, HIGH_MIDI, LOW_MIDI};
use crate::spectrum::{Stft, midi_hz, refine_f0};

pub struct NmfParams {
    /// Magnitude compression: V = |X|^pw.
    pub pw: f32,
    /// L1 penalty on activations (relative; the templates sum to 1).
    pub lambda: f32,
    pub iters: usize,
    /// A pitch is present when its activation is at least this share of sum(V).
    pub share: f32,
    pub max_notes: usize,
}

const F_MAX: f32 = 5000.0;
const SLOPES: [f32; 2] = [0.6, 1.4];
const B_NOMINAL: f32 = 5e-5;

pub struct Nmf {
    p: NmfParams,
    bin_hz: f32,
    kmax: usize,
    /// Sparse templates: (bin, weight) pairs, each template summing to 1.
    w: Vec<Vec<(u16, f32)>>,
    midi: Vec<u8>,
    h: Vec<f32>,
    v: Vec<f32>,
    wh: Vec<f32>,
    r: Vec<f32>,
    act: Vec<(u8, f32)>,
}

impl Nmf {
    pub fn new(stft: &Stft, p: NmfParams) -> Self {
        let bin_hz = stft.bin_hz;
        let kmax = ((F_MAX / bin_hz) as usize).min(stft.mag.len() - 1);
        let mut w = vec![];
        let mut midi = vec![];
        for m in LOW_MIDI as u8..=HIGH_MIDI as u8 {
            let f0 = midi_hz(m as f32);
            for &slope in &SLOPES {
                let mut t = vec![0.0f32; kmax];
                for h in 1..200 {
                    let fh = h as f32 * f0 * (1.0 + B_NOMINAL * (h * h) as f32).sqrt();
                    if fh > F_MAX {
                        break;
                    }
                    let a = (h as f32).powf(-slope);
                    let c = fh / bin_hz;
                    let lo = (c - 4.0).ceil().max(1.0) as usize;
                    let hi = ((c + 4.0).floor() as usize).min(kmax - 1);
                    for k in lo..=hi {
                        t[k] += a * stft.kernel(k as f32 - c);
                    }
                }
                let peak = t.iter().cloned().fold(0.0f32, f32::max);
                let mut col: Vec<(u16, f32)> =
                    t.iter().enumerate().filter(|&(_, &x)| x > peak * 1e-3).map(|(k, &x)| (k as u16, x.powf(p.pw))).collect();
                let sum: f32 = col.iter().map(|x| x.1).sum();
                for x in col.iter_mut() {
                    x.1 /= sum;
                }
                w.push(col);
                midi.push(m);
            }
        }
        let nt = w.len();
        Self {
            p,
            bin_hz,
            kmax,
            w,
            midi,
            h: vec![0.0; nt],
            v: vec![0.0; kmax],
            wh: vec![0.0; kmax],
            r: vec![0.0; kmax],
            act: Vec::with_capacity(64),
        }
    }

    pub fn detect(&mut self, stft: &Stft, out: &mut Vec<Det>) {
        out.clear();
        let mut total = 0.0f32;
        for k in 1..self.kmax {
            self.v[k] = stft.mag[k].powf(self.p.pw);
            total += self.v[k];
        }
        self.v[0] = 0.0;
        let floor = total * 1e-4;
        for h in self.h.iter_mut() {
            *h = h.max(floor);
        }
        for _ in 0..self.p.iters {
            self.wh.fill(1e-12);
            for (col, &h) in self.w.iter().zip(&self.h) {
                for &(k, x) in col {
                    self.wh[k as usize] += x * h;
                }
            }
            for k in 0..self.kmax {
                self.r[k] = self.v[k] / self.wh[k];
            }
            for (col, h) in self.w.iter().zip(self.h.iter_mut()) {
                let mut num = 0.0f32;
                for &(k, x) in col {
                    num += x * self.r[k as usize];
                }
                *h *= num / (1.0 + self.p.lambda);
            }
        }
        // Activation per pitch: the sum of its templates.
        self.act.clear();
        let mut i = 0;
        while i < self.h.len() {
            let m = self.midi[i];
            let mut a = 0.0;
            while i < self.h.len() && self.midi[i] == m {
                a += self.h[i];
                i += 1;
            }
            if a >= self.p.share * total {
                self.act.push((m, a));
            }
        }
        self.act.sort_by(|a, b| b.1.total_cmp(&a.1));
        for &(m, a) in self.act.iter().take(self.p.max_notes) {
            let f0 = midi_hz(m as f32);
            let f = refine_f0(&stft.mag, self.bin_hz, f0);
            out.push(Det {
                midi: m,
                cents: 1200.0 * (f / f0).log2(),
                strength: a / total.max(1e-12),
                level: a.powf(1.0 / self.p.pw),
            });
        }
    }

    /// Forget activations (a closed gate).
    pub fn reset(&mut self) {
        self.h.fill(0.0);
    }
}
