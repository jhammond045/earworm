//! Harmonic-sum multi-pitch estimation with iterative cancellation, after
//! Klapuri (2006), "Multiple fundamental frequency estimation by summing
//! harmonic amplitudes". Whiten the spectrum, score every candidate f0 by a
//! weighted sum of its partials' peaks, take the best, remove its partials
//! (smoothed across neighbouring partials, so a partial it shares with
//! another note is only partly removed), and repeat while the polyphony
//! estimate keeps growing.

use crate::spectrum::{Stft, midi_hz, refine_f0};

/// One pitch found in one frame.
#[derive(Clone, Copy, Debug)]
pub struct Det {
    pub midi: u8,
    /// Refined pitch, cents from the equal-tempered semitone.
    pub cents: f32,
    /// How sure the detector is, for ordering only.
    pub strength: f32,
    /// The note's own amplitude (its partials), for intensity and retriggers.
    pub level: f32,
}

pub struct HsParams {
    /// Polyphony estimate: stop when sum(s_i) / j^gamma stops growing.
    pub gamma: f32,
    /// Also stop when a candidate scores under this share of the first.
    pub rel_floor: f32,
    pub max_notes: usize,
    /// Whitening exponent (Klapuri's nu).
    pub nu: f32,
    /// Lowest and highest candidate, MIDI.
    pub lo: u8,
    pub hi: u8,
}
const STEPS: usize = 5;
const MAX_PARTIALS: usize = 20;
const F_MAX: f32 = 5000.0;
const B_MAX: f32 = 1.5e-4;

struct Cand {
    midi: f32,
    f0: f32,
    start: usize,
    end: usize,
}

pub struct Hs {
    p: HsParams,
    bin_hz: f32,
    cands: Vec<Cand>,
    /// Per candidate partial: lowest bin, highest bin, weight g(f0, m).
    tab: Vec<(u16, u16, f32)>,
    band_bins: Vec<f32>,
    /// Per bin: the band centre above it and how far it is from the one below.
    interp: Vec<(u8, f32)>,
    sigma: Vec<f32>,
    gam: Vec<f32>,
    y: Vec<f32>,
    yr: Vec<f32>,
    sal: Vec<f32>,
    amps: Vec<(usize, f32)>,
    k1: f32,
    k2: f32,
}

impl Hs {
    pub fn new(stft: &Stft, p: HsParams) -> Self {
        let bin_hz = stft.bin_hz;
        let nb = stft.mag.len();
        let alpha = if stft.n >= 4096 { 52.0 } else { 27.0 };
        let beta = 320.0;
        let mut cands = vec![];
        let mut tab = vec![];
        let n_c = (p.hi - p.lo) as usize * STEPS + STEPS;
        for i in 0..n_c {
            let midi = p.lo as f32 - 0.4 + i as f32 / STEPS as f32;
            let f0 = midi_hz(midi);
            let start = tab.len();
            let half = (0.5 / STEPS as f32 / 12.0).exp2();
            for m in 1..=MAX_PARTIALS {
                let fm = m as f32 * f0;
                if fm > F_MAX {
                    break;
                }
                let lo = (fm / half / bin_hz).round() as usize;
                let hi = (fm * half * (1.0 + B_MAX * (m * m) as f32).sqrt() / bin_hz).round() as usize;
                if hi + 3 >= nb {
                    break;
                }
                tab.push((lo as u16, hi.max(lo) as u16, (f0 + alpha) / (m as f32 * f0 + beta)));
            }
            cands.push(Cand { midi, f0, start, end: tab.len() });
        }
        let band_bins: Vec<f32> = (0..30).map(|b| 229.0 * (10f32.powf((b + 1) as f32 / 21.4) - 1.0) / bin_hz).collect();
        let interp = (0..nb)
            .map(|k| {
                let x = k as f32;
                match band_bins.iter().position(|&cb| cb > x) {
                    None => (29, 1.0),
                    Some(0) => (1, 0.0),
                    Some(b) => (b as u8, (x - band_bins[b - 1]) / (band_bins[b] - band_bins[b - 1])),
                }
            })
            .collect();
        let k0 = stft.kernel(0.0);
        Self {
            p,
            bin_hz,
            cands,
            tab,
            band_bins,
            interp,
            sigma: vec![0.0; 30],
            gam: vec![0.0; nb],
            y: vec![0.0; nb],
            yr: vec![0.0; nb],
            sal: vec![0.0; n_c],
            amps: Vec::with_capacity(MAX_PARTIALS),
            k1: stft.kernel(0.5) / k0,
            k2: stft.kernel(1.5) / k0,
        }
    }

    /// Spectral whitening: gain per bin from the power in 30 bands on the ERB
    /// scale, raised to nu - 1, so weak high partials count.
    fn whiten(&mut self, mag: &[f32]) {
        let nb = mag.len();
        let c = &self.band_bins;
        for b in 0..30 {
            let lo = if b == 0 { 0.0 } else { c[b - 1] };
            let hi = if b == 29 { c[29] * 1.2 } else { c[b + 1] };
            let (mut s, mut w) = (0.0f32, 0.0f32);
            let k_lo = lo.ceil() as usize;
            let k_hi = (hi.floor() as usize).min(nb - 1);
            for k in k_lo..=k_hi {
                let x = k as f32;
                let h = if x <= c[b] { (x - lo) / (c[b] - lo).max(1e-3) } else { (hi - x) / (hi - c[b]).max(1e-3) };
                s += h * mag[k] * mag[k];
                w += h;
            }
            self.sigma[b] = if w > 0.0 { (s / w).sqrt() } else { 0.0 };
        }
        let top = self.sigma.iter().cloned().fold(0.0f32, f32::max);
        let mut g = [0.0f32; 30];
        for (gb, &s) in g.iter_mut().zip(&self.sigma) {
            *gb = s.max(top * 1e-3).max(1e-9).powf(self.p.nu - 1.0);
        }
        for k in 0..nb {
            let (b, f) = self.interp[k];
            let b = b as usize;
            self.gam[k] = g[b - 1] * (1.0 - f) + g[b] * f;
            self.y[k] = self.gam[k] * mag[k];
        }
    }

    pub fn detect(&mut self, stft: &Stft, out: &mut Vec<Det>) {
        out.clear();
        self.whiten(&stft.mag);
        self.yr.copy_from_slice(&self.y);
        let mut taken = [false; 128];
        let (mut sum, mut prev_s_j, mut first) = (0.0f32, 0.0f32, 0.0f32);
        for j in 1..=self.p.max_notes {
            for (ci, c) in self.cands.iter().enumerate() {
                let mut s = 0.0;
                for &(lo, hi, g) in &self.tab[c.start..c.end] {
                    let mut mx = 0.0f32;
                    for k in lo as usize..=hi as usize {
                        mx = mx.max(self.yr[k]);
                    }
                    s += g * mx;
                }
                self.sal[ci] = s;
            }
            let mut best = None;
            for (ci, c) in self.cands.iter().enumerate() {
                let semi = c.midi.round() as usize;
                if taken[semi] {
                    continue;
                }
                if best.is_none_or(|b: usize| self.sal[ci] > self.sal[b]) {
                    best = Some(ci);
                }
            }
            let Some(bi) = best else { break };
            let s = self.sal[bi];
            let s_j = (sum + s) / (j as f32).powf(self.p.gamma);
            if j == 1 {
                first = s;
            } else if s_j <= prev_s_j || s < self.p.rel_floor * first {
                break;
            }
            sum += s;
            prev_s_j = s_j;
            let level = self.cancel(bi);
            let c = &self.cands[bi];
            let semi = c.midi.round();
            taken[semi as usize] = true;
            let f = refine_f0(&stft.mag, self.bin_hz, c.f0);
            let cents = 1200.0 * (f / midi_hz(semi)).log2();
            out.push(Det { midi: semi as u8, cents, strength: s / first.max(1e-9), level });
        }
    }

    /// Remove candidate `ci`'s partials from the residual; return the note's
    /// amplitude in the unwhitened spectrum.
    fn cancel(&mut self, ci: usize) -> f32 {
        let c = &self.cands[ci];
        self.amps.clear();
        for &(lo, hi, _) in &self.tab[c.start..c.end] {
            let mut k = lo as usize;
            for kk in lo as usize..=hi as usize {
                if self.yr[kk] > self.yr[k] {
                    k = kk;
                }
            }
            self.amps.push((k, self.yr[k]));
        }
        let n = self.amps.len();
        let mut level = 0.0f32;
        for i in 0..n {
            let (k, a) = self.amps[i];
            let lo = i.saturating_sub(1);
            let hi = (i + 1).min(n - 1);
            let mean = self.amps[lo..=hi].iter().map(|x| x.1).sum::<f32>() / (hi - lo + 1) as f32;
            let a = a.min(mean);
            for (d, kern) in [(0isize, 1.0), (1, self.k1), (2, self.k2)] {
                for kk in [k as isize - d, k as isize + d] {
                    if kk >= 0 && (kk as usize) < self.yr.len() {
                        let v = &mut self.yr[kk as usize];
                        *v = (*v - a * kern).max(0.0);
                    }
                    if d == 0 {
                        break;
                    }
                }
            }
            let x = a / self.gam[k];
            level += x * x;
        }
        level.sqrt()
    }
}
