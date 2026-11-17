//! Integer-factor decimator for the pitch path (context rate -> ~16 kHz).

use std::f64::consts::PI;

const TAPS: usize = 31;

/// Factor used to bring `sample_rate` near 16 kHz: max(1, round(sr / 16000)).
/// 48 kHz -> 3 (16 kHz), 44.1 kHz -> 3 (14.7 kHz), 16 kHz -> 1.
pub fn decimation_factor(sample_rate: f32) -> usize {
    ((sample_rate / 16_000.0).round() as usize).max(1)
}

/// Anti-aliased decimator: FIR low-pass (cutoff ~0.4 of the output rate,
/// windowed sinc, odd length around 31 taps) then keep every `factor`-th sample.
pub struct Decimator {
    factor: usize,
    taps: [f32; TAPS],
    // History is written twice (at pos and pos + TAPS) so the newest TAPS
    // samples are always one contiguous slice and the dot product never wraps.
    hist: [f32; 2 * TAPS],
    pos: usize,
    phase: usize,
}

impl Decimator {
    pub fn new(factor: usize) -> Self {
        let factor = factor.max(1);
        let mut taps = [0.0f32; TAPS];
        // Cutoff at 0.4 of the output rate, as cycles per input sample.
        let fc = 0.4 / factor as f64;
        let mid = (TAPS - 1) as f64 / 2.0;
        let mut sum = 0.0f64;
        let mut t64 = [0.0f64; TAPS];
        for (i, t) in t64.iter_mut().enumerate() {
            let x = i as f64 - mid;
            let sinc = if x == 0.0 { 2.0 * fc } else { (2.0 * PI * fc * x).sin() / (PI * x) };
            let w = 0.54 - 0.46 * (2.0 * PI * i as f64 / (TAPS - 1) as f64).cos();
            *t = sinc * w;
            sum += *t;
        }
        for (o, t) in taps.iter_mut().zip(t64) {
            *o = (t / sum) as f32; // unity gain at DC
        }
        Self { factor, taps, hist: [0.0; 2 * TAPS], pos: 0, phase: 0 }
    }
    pub fn factor(&self) -> usize {
        self.factor
    }
    /// Feed one input sample; returns an output sample every `factor` inputs.
    #[inline]
    pub fn process(&mut self, x: f32) -> Option<f32> {
        let x = if x.is_finite() { x } else { 0.0 };
        self.hist[self.pos] = x;
        self.hist[self.pos + TAPS] = x;
        self.pos += 1;
        if self.pos == TAPS {
            self.pos = 0;
        }
        self.phase += 1;
        if self.phase < self.factor {
            return None;
        }
        self.phase = 0;
        // Oldest sample is at pos; taps[0] pairs with the newest sample.
        let w = &self.hist[self.pos..self.pos + TAPS];
        let mut acc = 0.0;
        for i in 0..TAPS {
            acc += self.taps[i] * w[TAPS - 1 - i];
        }
        Some(acc)
    }
    /// Group delay in input samples ((taps - 1) / 2).
    pub fn delay_samples(&self) -> usize {
        (TAPS - 1) / 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factors() {
        assert_eq!(decimation_factor(48_000.0), 3);
        assert_eq!(decimation_factor(44_100.0), 3);
        assert_eq!(decimation_factor(16_000.0), 1);
        assert_eq!(decimation_factor(8_000.0), 1);
    }

    fn out_rms(hz: f32) -> f32 {
        let sr = 48_000.0;
        let mut d = Decimator::new(3);
        let (mut s, mut n) = (0.0f64, 0);
        for i in 0..48_000 {
            let x = (2.0 * std::f32::consts::PI * hz * i as f32 / sr).sin();
            if let Some(y) = d.process(x) {
                if i > 4000 {
                    s += (y * y) as f64;
                    n += 1;
                }
            }
        }
        (s / n as f64).sqrt() as f32 * std::f32::consts::SQRT_2
    }

    #[test]
    fn passes_1khz_and_attenuates_high() {
        let g1 = out_rms(1000.0);
        assert!((g1 - 1.0).abs() < 0.02, "{g1}");
        // 7.5 kHz is above the 6.4 kHz cutoff; 9 kHz would alias to 7 kHz.
        let g75 = out_rms(7500.0);
        assert!(g75 < 0.3, "7.5k {g75}");
        let g9 = out_rms(9000.0);
        assert!(g9 < 0.05, "9k {g9}");
    }

    #[test]
    fn dc_gain_and_rate() {
        let mut d = Decimator::new(3);
        let outs: Vec<f32> = (0..300).filter_map(|_| d.process(1.0)).collect();
        assert_eq!(outs.len(), 100);
        assert!((outs[99] - 1.0).abs() < 1e-4);
        assert_eq!(d.delay_samples(), 15);
    }

    #[test]
    fn factor_one_is_delayed_passthrough_shape() {
        let mut d = Decimator::new(1);
        assert!(d.process(1.0).is_some());
    }
}
