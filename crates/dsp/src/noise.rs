//! Seeded white noise (xorshift32). Seeded so offline renders are repeatable.

#[derive(Clone, Copy, Debug)]
pub struct Noise {
    state: u32,
}

impl Noise {
    /// Seed 0 is replaced by a fixed non-zero seed.
    pub fn new(seed: u32) -> Self {
        todo!()
    }
    /// Uniform in [-1, 1).
    #[inline]
    pub fn next(&mut self) -> f32 {
        todo!()
    }
}
