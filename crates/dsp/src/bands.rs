//! The 16 (or 24) log-spaced bands shared by the capture front end, the
//! partial bank's `tone_bands` and the shaped noise's `noise_bands`.

use crate::filters::{OnePole, Svf};
use crate::frame::MAX_BANDS;

pub const BAND_LO_HZ: f32 = 80.0;
pub const BAND_HI_HZ: f32 = 12_000.0;

/// Ratio between neighbouring centres.
fn ratio(n: usize) -> f64 {
    (BAND_HI_HZ as f64 / BAND_LO_HZ as f64).powf(1.0 / (n.max(2) - 1) as f64)
}

/// Log-spaced centres from 80 Hz to 12 kHz inclusive. Writes `out[..n]`.
pub fn band_centres(n: usize, out: &mut [f32]) {
    if n == 0 {
        return;
    }
    if n == 1 {
        out[0] = BAND_LO_HZ;
        return;
    }
    let r = ratio(n);
    for (i, o) in out[..n].iter_mut().enumerate() {
        *o = (BAND_LO_HZ as f64 * r.powi(i as i32)) as f32;
    }
    out[0] = BAND_LO_HZ;
    out[n - 1] = BAND_HI_HZ;
}

/// Q for a band-pass at these centres so neighbours cross near -3 dB:
/// q = sqrt(r) / (r - 1) where r is the centre ratio between neighbours.
pub fn band_q(n: usize) -> f32 {
    let r = ratio(n);
    (r.sqrt() / (r - 1.0)) as f32
}

/// Gain in dB at `freq`, by linear interpolation in log frequency between
/// band centres. Below the first centre or above the last, hold the end value.
pub fn gain_at(freq: f32, centres: &[f32], gains_db: &[f32]) -> f32 {
    let n = centres.len().min(gains_db.len());
    if n == 0 {
        return 0.0;
    }
    if !(freq > centres[0]) {
        return gains_db[0];
    }
    if freq >= centres[n - 1] {
        return gains_db[n - 1];
    }
    // At most 24 centres, so a linear scan is fine.
    let mut i = 0;
    while i + 2 < n && freq >= centres[i + 1] {
        i += 1;
    }
    let (lo, hi) = (centres[i], centres[i + 1]);
    let t = (freq / lo).ln() / (hi / lo).ln();
    gains_db[i] + t * (gains_db[i + 1] - gains_db[i])
}

/// Analysis band bank: `n` band-pass SVFs at the band centres, squared,
/// smoothed with separate attack/release one-poles (5 ms / 15 ms per TDD).
pub struct BandBank {
    n: usize,
    filters: [Svf; MAX_BANDS],
    power: [f32; MAX_BANDS],
    att: f32,
    rel: f32,
    centres: [f32; MAX_BANDS],
}

impl BandBank {
    pub fn new(sample_rate: f32, n: usize) -> Self {
        let n = n.clamp(1, MAX_BANDS);
        let mut centres = [0.0; MAX_BANDS];
        band_centres(n, &mut centres);
        let q = band_q(n);
        let mut filters = [Svf::default(); MAX_BANDS];
        for i in 0..n {
            filters[i] = Svf::new(sample_rate, centres[i], q);
        }
        Self {
            n,
            filters,
            power: [0.0; MAX_BANDS],
            att: OnePole::coef_tau(0.005, sample_rate),
            rel: OnePole::coef_tau(0.015, sample_rate),
            centres,
        }
    }
    pub fn n(&self) -> usize {
        self.n
    }
    pub fn centres(&self) -> &[f32] {
        &self.centres[..self.n]
    }
    /// Feed one sample.
    #[inline]
    pub fn process(&mut self, x: f32) {
        let (att, rel) = (self.att, self.rel);
        for (f, p) in self.filters[..self.n].iter_mut().zip(self.power[..self.n].iter_mut()) {
            let y = f.bandpass_norm(x);
            let e = y * y;
            let a = if e > *p { att } else { rel };
            *p += a * (e - *p);
        }
    }
    /// Smoothed mean-square power per band (linear).
    pub fn power(&self) -> &[f32] {
        &self.power[..self.n]
    }
    /// Smoothed level per band in dB (10 log10 power), floor -120 dB.
    pub fn levels_db(&self, out: &mut [f32]) {
        for (o, &p) in out.iter_mut().zip(&self.power[..self.n]) {
            *o = (10.0 * p.max(1e-12).log10()).max(-120.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centres_endpoints_and_monotonic() {
        for &n in &[16usize, 24, 2] {
            let mut c = [0.0f32; MAX_BANDS];
            band_centres(n, &mut c);
            assert_eq!(c[0], 80.0);
            assert_eq!(c[n - 1], 12_000.0);
            for i in 1..n {
                assert!(c[i] > c[i - 1]);
            }
        }
        let mut c = [0.0f32; MAX_BANDS];
        band_centres(16, &mut c);
        let r0 = c[1] / c[0];
        for i in 1..15 {
            assert!((c[i + 1] / c[i] - r0).abs() < 1e-3);
        }
    }

    #[test]
    fn q_values() {
        let q = band_q(16);
        let r = (150.0f32).powf(1.0 / 15.0);
        assert!((q - r.sqrt() / (r - 1.0)).abs() < 1e-4);
        assert!(band_q(24) > q);
    }

    #[test]
    fn gain_at_interpolates() {
        let c = [100.0, 400.0, 1600.0];
        let g = [0.0, -6.0, 6.0];
        assert_eq!(gain_at(100.0, &c, &g), 0.0);
        assert!((gain_at(400.0, &c, &g) + 6.0).abs() < 1e-5);
        assert!((gain_at(1600.0, &c, &g) - 6.0).abs() < 1e-5);
        // 200 Hz is the log midpoint of 100..400.
        assert!((gain_at(200.0, &c, &g) + 3.0).abs() < 1e-4);
        assert!((gain_at(800.0, &c, &g) - 0.0).abs() < 1e-4);
        assert_eq!(gain_at(20.0, &c, &g), 0.0);
        assert_eq!(gain_at(9000.0, &c, &g), 6.0);
    }

    #[test]
    fn bank_peaks_at_driven_band() {
        let sr = 48_000.0;
        for n in [16usize, 24] {
            let mut c = [0.0f32; MAX_BANDS];
            band_centres(n, &mut c);
            for &k in &[0usize, 5, 9, n - 1] {
                let mut bank = BandBank::new(sr, n);
                for i in 0..24_000 {
                    let x = (2.0 * std::f32::consts::PI * c[k] * i as f32 / sr).sin();
                    bank.process(x);
                }
                let p = bank.power();
                let best = (0..n).max_by(|&a, &b| p[a].total_cmp(&p[b])).unwrap();
                assert_eq!(best, k, "n {n}");
                // Unit sine through a unity-gain band: mean square 0.5. The fast
                // attack and slow release bias the smoothed value upward (it
                // rides the ripple of the squared signal), so allow 0.4..0.8.
                // Skip the lowest band: its ripple is slow enough to swing wildly.
                if c[k] > 200.0 {
                    assert!(p[k] > 0.4 && p[k] < 0.8, "band {k} power {}", p[k]);
                    let mut db = [0.0f32; MAX_BANDS];
                    bank.levels_db(&mut db);
                    assert!(db[k] > -4.0 && db[k] < -1.0);
                }
            }
        }
    }

    #[test]
    fn bank_silence_floors_and_release_is_slower() {
        let sr = 48_000.0;
        let mut bank = BandBank::new(sr, 16);
        let mut db = [0.0f32; 16];
        bank.levels_db(&mut db);
        assert!(db.iter().all(|&d| d <= -119.0));
        let f = bank.centres()[8];
        for i in 0..9600 {
            bank.process((2.0 * std::f32::consts::PI * f * i as f32 / sr).sin());
        }
        let peak = bank.power()[8];
        for _ in 0..480 {
            bank.process(0.0);
        }
        assert!(bank.power()[8] < peak && bank.power()[8] > 0.0);
    }
}
