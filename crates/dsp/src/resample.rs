//! Integer-factor decimator for the pitch path (context rate -> ~16 kHz).

/// Factor used to bring `sample_rate` near 16 kHz: max(1, round(sr / 16000)).
/// 48 kHz -> 3 (16 kHz), 44.1 kHz -> 3 (14.7 kHz), 16 kHz -> 1.
pub fn decimation_factor(sample_rate: f32) -> usize {
    todo!()
}

/// Anti-aliased decimator: FIR low-pass (cutoff ~0.4 of the output rate,
/// windowed sinc, odd length around 31 taps) then keep every `factor`-th sample.
pub struct Decimator {
    factor: usize,
    // implementation-defined: taps, history ring, phase
}

impl Decimator {
    pub fn new(factor: usize) -> Self {
        todo!()
    }
    pub fn factor(&self) -> usize {
        self.factor
    }
    /// Feed one input sample; returns an output sample every `factor` inputs.
    #[inline]
    pub fn process(&mut self, x: f32) -> Option<f32> {
        todo!()
    }
    /// Group delay in input samples ((taps - 1) / 2).
    pub fn delay_samples(&self) -> usize {
        todo!()
    }
}
