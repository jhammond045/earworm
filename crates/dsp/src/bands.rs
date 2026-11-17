//! The 16 (or 24) log-spaced bands shared by the capture front end, the
//! partial bank's `tone_bands` and the shaped noise's `noise_bands`.

use crate::filters::Svf;
use crate::frame::MAX_BANDS;

pub const BAND_LO_HZ: f32 = 80.0;
pub const BAND_HI_HZ: f32 = 12_000.0;

/// Log-spaced centres from 80 Hz to 12 kHz inclusive. Writes `out[..n]`.
pub fn band_centres(n: usize, out: &mut [f32]) {
    todo!()
}

/// Q for a band-pass at these centres so neighbours cross near -3 dB:
/// q = sqrt(r) / (r - 1) where r is the centre ratio between neighbours.
pub fn band_q(n: usize) -> f32 {
    todo!()
}

/// Gain in dB at `freq`, by linear interpolation in log frequency between
/// band centres. Below the first centre or above the last, hold the end value.
pub fn gain_at(freq: f32, centres: &[f32], gains_db: &[f32]) -> f32 {
    todo!()
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
        todo!()
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
        todo!()
    }
    /// Smoothed mean-square power per band (linear).
    pub fn power(&self) -> &[f32] {
        &self.power[..self.n]
    }
    /// Smoothed level per band in dB (10 log10 power), floor -120 dB.
    pub fn levels_db(&self, out: &mut [f32]) {
        todo!()
    }
}
