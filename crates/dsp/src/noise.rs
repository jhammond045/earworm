//! Seeded white noise (xorshift32). Seeded so offline renders are repeatable.

#[derive(Clone, Copy, Debug)]
pub struct Noise {
    state: u32,
}

impl Noise {
    /// Seed 0 is replaced by a fixed non-zero seed.
    pub fn new(seed: u32) -> Self {
        Self { state: if seed == 0 { 0x9E37_79B9 } else { seed } }
    }
    /// Uniform in [-1, 1).
    #[inline]
    pub fn next(&mut self) -> f32 {
        let mut s = self.state;
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        self.state = s;
        // Top 24 bits fit an f32 mantissa exactly: [0, 2^24) * 2^-23 - 1.
        (s >> 8) as f32 * (1.0 / 8_388_608.0) - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_and_mean() {
        let mut n = Noise::new(1);
        let (mut sum, mut sq) = (0.0f64, 0.0f64);
        let count = 200_000;
        for _ in 0..count {
            let x = n.next();
            assert!((-1.0..1.0).contains(&x));
            sum += x as f64;
            sq += (x * x) as f64;
        }
        assert!((sum / count as f64).abs() < 0.01);
        // Uniform on [-1,1) has variance 1/3.
        assert!((sq / count as f64 - 1.0 / 3.0).abs() < 0.01);
    }

    #[test]
    fn seed_zero_works_and_is_repeatable() {
        let mut a = Noise::new(0);
        let mut b = Noise::new(0);
        let va: Vec<f32> = (0..10).map(|_| a.next()).collect();
        let vb: Vec<f32> = (0..10).map(|_| b.next()).collect();
        assert_eq!(va, vb);
        assert!(va.iter().any(|&x| x != 0.0));
    }
}
