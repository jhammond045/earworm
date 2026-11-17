//! Filters: one-pole smoother, TPT state-variable filter, DC blocker.

use std::f32::consts::PI;

/// One-pole low-pass / smoother. `y += a (x - y)`.
#[derive(Clone, Copy, Debug, Default)]
pub struct OnePole {
    pub a: f32,
    pub y: f32,
}

impl OnePole {
    /// Coefficient for a time constant in seconds at `rate` updates/second.
    /// `tau <= 0` gives a = 1 (no smoothing).
    pub fn coef_tau(tau_s: f32, rate: f32) -> f32 {
        if !(tau_s > 0.0) || !(rate > 0.0) {
            return 1.0;
        }
        1.0 - (-1.0 / (tau_s * rate)).exp()
    }
    /// Coefficient for a -3 dB cutoff in Hz at `rate` updates/second.
    pub fn coef_hz(hz: f32, rate: f32) -> f32 {
        if !(hz > 0.0) || !(rate > 0.0) {
            return 1.0;
        }
        1.0 - (-2.0 * PI * hz / rate).exp()
    }
    pub fn new(a: f32) -> Self {
        Self { a, y: 0.0 }
    }
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        // A NaN or inf input would stick in the state forever, so drop it.
        let x = if x.is_finite() { x } else { self.y };
        self.y += self.a * (x - self.y);
        self.y
    }
}

/// Topology-preserving (Zavalishin/Simper) state-variable filter.
#[derive(Clone, Copy, Debug, Default)]
pub struct Svf {
    g: f32,
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    ic1: f32,
    ic2: f32,
}

/// Simultaneous outputs of one SVF tick.
#[derive(Clone, Copy, Debug, Default)]
pub struct SvfOut {
    pub lp: f32,
    pub bp: f32,
    pub hp: f32,
}

impl Svf {
    pub fn new(sample_rate: f32, freq: f32, q: f32) -> Self {
        let mut s = Self::default();
        s.set(sample_rate, freq, q);
        s
    }
    /// Retune without clearing state.
    pub fn set(&mut self, sample_rate: f32, freq: f32, q: f32) {
        // tan blows up at Nyquist/2 of the filter's own rate, so stay below 0.49 sr.
        let f = freq.clamp(1.0, 0.49 * sample_rate);
        let q = q.max(0.05);
        self.g = (PI * f / sample_rate).tan();
        self.k = 1.0 / q;
        self.a1 = 1.0 / (1.0 + self.g * (self.g + self.k));
        self.a2 = self.g * self.a1;
        self.a3 = self.g * self.a2;
    }
    #[inline]
    fn tick(&mut self, x: f32) -> (f32, f32) {
        let x = if x.is_finite() { x } else { 0.0 };
        let v3 = x - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        (v1, v2)
    }
    #[inline]
    pub fn process(&mut self, x: f32) -> SvfOut {
        let x = if x.is_finite() { x } else { 0.0 };
        let (bp, lp) = self.tick(x);
        SvfOut { lp, bp, hp: x - self.k * bp - lp }
    }
    /// Band-pass output normalized to unity gain at the centre frequency.
    #[inline]
    pub fn bandpass_norm(&mut self, x: f32) -> f32 {
        // The raw band-pass peaks at 1/k = q.
        let (bp, _) = self.tick(x);
        self.k * bp
    }
    pub fn reset(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }
}

/// Second-order high-pass (SVF high output, Q = 0.707) for the 60 Hz input filter.
#[derive(Clone, Copy, Debug, Default)]
pub struct HighPass(pub Svf);

impl HighPass {
    pub fn new(sample_rate: f32, freq: f32) -> Self {
        Self(Svf::new(sample_rate, freq, std::f32::consts::FRAC_1_SQRT_2))
    }
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.0.process(x).hp
    }
}

/// DC blocker: y[n] = x[n] - x[n-1] + r y[n-1], r = 0.995 at 48 kHz scaled by rate.
#[derive(Clone, Copy, Debug, Default)]
pub struct DcBlocker {
    r: f32,
    x1: f32,
    y1: f32,
}

impl DcBlocker {
    pub fn new(sample_rate: f32) -> Self {
        // Keep the same time constant in seconds at other rates.
        let r = (0.995f32.ln() * 48_000.0 / sample_rate.max(1.0)).exp();
        Self { r, x1: 0.0, y1: 0.0 }
    }
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let x = if x.is_finite() { x } else { 0.0 };
        let y = x - self.x1 + self.r * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    /// Steady-state RMS gain of `f` for a sine at `hz`.
    fn gain(mut f: impl FnMut(f32) -> f32, hz: f32) -> f32 {
        let n = 48_000;
        let (mut si, mut so) = (0.0f64, 0.0f64);
        for i in 0..n {
            let x = (2.0 * std::f32::consts::PI * hz * i as f32 / SR).sin();
            let y = f(x);
            if i >= n / 2 {
                si += (x * x) as f64;
                so += (y * y) as f64;
            }
        }
        (so / si).sqrt() as f32
    }

    #[test]
    fn one_pole_step_reaches_one_minus_inv_e_after_tau() {
        let rate = 1000.0;
        let tau = 0.05;
        let mut p = OnePole::new(OnePole::coef_tau(tau, rate));
        let mut y = 0.0;
        for _ in 0..(tau * rate) as usize {
            y = p.process(1.0);
        }
        assert!((y - (1.0 - (-1.0f32).exp())).abs() < 1e-4, "{y}");
    }

    #[test]
    fn one_pole_edge_cases() {
        assert_eq!(OnePole::coef_tau(0.0, 100.0), 1.0);
        let mut p = OnePole::new(0.5);
        p.process(1.0);
        assert!(p.process(f32::NAN).is_finite());
    }

    #[test]
    fn one_pole_cutoff_is_minus_3db() {
        let a = OnePole::coef_hz(1000.0, SR);
        let mut p = OnePole::new(a);
        let g = gain(|x| p.process(x), 1000.0);
        // The discrete one-pole is only approximately -3 dB at the analog cutoff.
        assert!((g - 0.707).abs() < 0.04, "{g}");
    }

    #[test]
    fn svf_lowpass_dc_gain_is_one() {
        let mut f = Svf::new(SR, 1000.0, 0.707);
        let mut o = SvfOut::default();
        for _ in 0..20_000 {
            o = f.process(1.0);
        }
        assert!((o.lp - 1.0).abs() < 1e-3, "{}", o.lp);
        assert!(o.hp.abs() < 1e-3 && o.bp.abs() < 1e-3);
    }

    #[test]
    fn svf_bandpass_norm_unity_at_centre() {
        for &(fc, q) in &[(1000.0, 3.0), (200.0, 1.18), (5000.0, 8.0)] {
            let mut f = Svf::new(SR, fc, q);
            let g = gain(|x| f.bandpass_norm(x), fc);
            assert!((g - 1.0).abs() < 0.01, "fc {fc} q {q} gain {g}");
        }
    }

    #[test]
    fn svf_bandpass_neighbour_crossing_and_octave() {
        let q = crate::bands::band_q(16);
        let r = (12_000.0f32 / 80.0).powf(1.0 / 15.0);
        let fc = 1000.0;
        let mut f = Svf::new(SR, fc, q);
        let g = gain(|x| f.bandpass_norm(x), fc * r.sqrt());
        assert!((g - 0.707).abs() < 0.03, "neighbour crossing {g}");
        let mut f = Svf::new(SR, fc, q);
        let g = gain(|x| f.bandpass_norm(x), fc * 2.0);
        assert!(g < 0.3 && g > 0.1, "octave {g}");
    }

    #[test]
    fn svf_clamps_and_survives_nan() {
        let mut f = Svf::new(SR, 40_000.0, 1.0);
        for _ in 0..100 {
            assert!(f.process(1.0).lp.is_finite());
        }
        f.process(f32::NAN);
        assert!(f.process(0.0).lp.is_finite());
    }

    #[test]
    fn highpass_blocks_20hz_passes_1khz() {
        let mut h = HighPass::new(SR, 60.0);
        let g20 = gain(|x| h.process(x), 20.0);
        let mut h = HighPass::new(SR, 60.0);
        let g1k = gain(|x| h.process(x), 1000.0);
        assert!(g20 < 0.15, "{g20}");
        assert!((g1k - 1.0).abs() < 0.02, "{g1k}");
    }

    #[test]
    fn dc_blocker_removes_dc_passes_tone() {
        let mut d = DcBlocker::new(SR);
        let mut y = 1.0;
        for _ in 0..20_000 {
            y = d.process(1.0);
        }
        assert!(y.abs() < 1e-3, "{y}");
        let mut d = DcBlocker::new(SR);
        let g = gain(|x| d.process(x), 1000.0);
        assert!((g - 1.0).abs() < 0.01, "{g}");
    }
}
