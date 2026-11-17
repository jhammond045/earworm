//! Radix-2 complex FFT plus helpers for magnitude spectra. Allocates only in `new`.

pub struct Fft {
    n: usize,
    // implementation-defined tables (twiddles, bit reversal)
}

impl Fft {
    /// `n` must be a power of two >= 2.
    pub fn new(n: usize) -> Self {
        todo!()
    }
    pub fn len(&self) -> usize {
        self.n
    }
    /// In-place forward transform, no scaling. `re.len() == im.len() == n`.
    pub fn forward(&self, re: &mut [f32], im: &mut [f32]) {
        todo!()
    }
    /// In-place inverse transform, scaled by 1/n.
    pub fn inverse(&self, re: &mut [f32], im: &mut [f32]) {
        todo!()
    }
}

/// Hann window of length n (periodic form, w[i] = 0.5 - 0.5 cos(2 pi i / n)).
pub fn hann(out: &mut [f32]) {
    todo!()
}
