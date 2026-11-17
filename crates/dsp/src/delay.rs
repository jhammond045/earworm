//! Fixed-capacity delay line with integer, linear and cubic reads.
//! The buffer is allocated once in `new`; `push` and the reads never allocate.

#[derive(Clone, Debug)]
pub struct DelayLine {
    buf: Vec<f32>,
    mask: usize,
    w: usize,
}

impl DelayLine {
    /// Room for at least `max_delay` samples of delay (plus interpolation margin).
    pub fn new(max_delay: usize) -> Self {
        let n = (max_delay + 4).next_power_of_two();
        Self { buf: vec![0.0; n], mask: n - 1, w: 0 }
    }

    /// Longest delay a fractional read can use.
    pub fn max_delay(&self) -> usize {
        self.buf.len() - 4
    }

    /// Write the newest sample.
    #[inline]
    pub fn push(&mut self, x: f32) {
        self.w = (self.w + 1) & self.mask;
        self.buf[self.w] = if x.is_finite() { x } else { 0.0 };
    }

    /// The sample written `d` pushes ago: `tap(0)` is the newest.
    #[inline]
    pub fn tap(&self, d: usize) -> f32 {
        self.buf[self.w.wrapping_sub(d) & self.mask]
    }

    /// Fractional delay in samples, linear interpolation. Clamped to 0..max_delay.
    #[inline]
    pub fn read_linear(&self, d: f32) -> f32 {
        let d = d.clamp(0.0, self.max_delay() as f32);
        let i = d as usize;
        let f = d - i as f32;
        let a = self.tap(i);
        a + f * (self.tap(i + 1) - a)
    }

    /// Fractional delay in samples, 4-point Catmull-Rom (cubic Hermite).
    /// Clamped to 1..max_delay so all four points exist.
    #[inline]
    pub fn read_cubic(&self, d: f32) -> f32 {
        let d = d.clamp(1.0, self.max_delay() as f32 - 1.0);
        let i = d as usize;
        let t = d - i as f32;
        let (y0, y1, y2, y3) = (self.tap(i - 1), self.tap(i), self.tap(i + 1), self.tap(i + 2));
        let c1 = 0.5 * (y2 - y0);
        let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
        let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
        ((c3 * t + c2) * t + c1) * t + y1
    }

    pub fn clear(&mut self) {
        self.buf.fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_tap_returns_past_samples() {
        let mut d = DelayLine::new(10);
        for i in 0..20 {
            d.push(i as f32);
        }
        assert_eq!(d.tap(0), 19.0);
        assert_eq!(d.tap(5), 14.0);
        assert_eq!(d.read_linear(5.0), 14.0);
        assert_eq!(d.read_cubic(5.0), 14.0);
    }

    #[test]
    fn fractional_reads_on_a_ramp_are_exact() {
        let mut d = DelayLine::new(64);
        for i in 0..100 {
            d.push(i as f32);
        }
        assert!((d.read_linear(3.25) - 95.75).abs() < 1e-4);
        assert!((d.read_cubic(3.25) - 95.75).abs() < 1e-4);
    }

    #[test]
    fn cubic_delays_a_sine_accurately() {
        let mut d = DelayLine::new(256);
        let w = 0.05f32;
        let mut err = 0.0f32;
        for i in 0..2000 {
            d.push((w * i as f32).sin());
            if i > 300 {
                let want = (w * (i as f32 - 10.4)).sin();
                err = err.max((d.read_cubic(10.4) - want).abs());
            }
        }
        assert!(err < 1e-3, "{err}");
    }

    #[test]
    fn nan_input_is_dropped_and_reads_clamp() {
        let mut d = DelayLine::new(8);
        d.push(f32::NAN);
        assert_eq!(d.tap(0), 0.0);
        assert!(d.read_linear(1e9).is_finite());
        assert!(d.read_cubic(-5.0).is_finite());
    }
}
