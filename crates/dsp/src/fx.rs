//! Per-voice modules for the Klont (TDD "Klont DSP" table): fold, clang,
//! comb, click, and the motion sources (LFO, chaos). Per-layer effects are in
//! `layer_fx`.
//!
//! Convention for every type here: parameters are set at control rate
//! (`set*`, every 32 samples or so; cheap enough to call that often, but
//! may do some math), and `process` runs per sample and must be cheap, never
//! allocate, and return a finite value for any finite or non-finite input.
//! All buffers are allocated in `new`. Feedback paths add `crate::DENORMAL`.
//! Random sources take a seed so offline renders repeat exactly.

use crate::DENORMAL;
use crate::delay::DelayLine;
use crate::filters::Svf;
use crate::noise::Noise;
use std::f32::consts::{PI, TAU};

// ---- fast math (branch-free so loops over slices vectorize) ----

const FRAC_2_PI_F: f32 = 0.636_619_77;
// pi/2 split in three parts (Cody-Waite) so the reduction stays exact.
const PIO2_HI: f32 = 1.570_312_5;
const PIO2_MID: f32 = 4.837_512_97e-4;
const PIO2_LO: f32 = 7.549_79e-8;

/// Sine and cosine of `x` radians. Absolute error under 1e-6 for |x| up to
/// a few hundred; no libm, no branches, so it vectorizes.
#[inline(always)]
pub fn fast_sin_cos(x: f32) -> (f32, f32) {
    let n = (x * FRAC_2_PI_F + 0.5).floor();
    let r = ((x - n * PIO2_HI) - n * PIO2_MID) - n * PIO2_LO;
    let r2 = r * r;
    let s = r * (1.0 + r2 * (-1.0 / 6.0 + r2 * (1.0 / 120.0 + r2 * (-1.0 / 5040.0 + r2 * (1.0 / 362_880.0)))));
    let c = 1.0
        + r2 * (-0.5 + r2 * (1.0 / 24.0 + r2 * (-1.0 / 720.0 + r2 * (1.0 / 40_320.0 + r2 * (-1.0 / 3_628_800.0)))));
    let q = (n as i32) & 3;
    let swap = (q & 1) != 0;
    let (a, b) = if swap { (c, s) } else { (s, c) };
    let ss = 1.0 - 2.0 * (((q >> 1) & 1) as f32);
    let cs = 1.0 - 2.0 * ((((q + 1) >> 1) & 1) as f32);
    (a * ss, b * cs)
}

/// Sine only (same accuracy as `fast_sin_cos`).
#[inline(always)]
pub fn fast_sin(x: f32) -> f32 {
    fast_sin_cos(x).0
}

/// log2 for positive normal x (values at or below 1e-30 are clamped).
/// Relative error well under 1e-6 of the result's range.
#[inline(always)]
pub fn fast_log2(x: f32) -> f32 {
    let bits = x.max(1e-30).to_bits();
    let e = ((bits >> 23) & 255) as i32 - 127;
    let m = f32::from_bits((bits & 0x007f_ffff) | 0x3f80_0000);
    let hi = m > 1.414_213_6;
    let m = if hi { m * 0.5 } else { m };
    let e = (e + hi as i32) as f32;
    let s = (m - 1.0) / (m + 1.0);
    let s2 = s * s;
    let p = s * (1.0 + s2 * (1.0 / 3.0 + s2 * (1.0 / 5.0 + s2 * (1.0 / 7.0 + s2 * (1.0 / 9.0)))));
    e + p * 2.885_39
}

/// 2^x, clamped to about 2^-125 .. 2^125. Relative error about 1e-7.
#[inline(always)]
pub fn fast_exp2(x: f32) -> f32 {
    let x = x.clamp(-125.0, 125.0);
    let n = (x + 0.5).floor();
    let f = x - n;
    let t = f * core::f32::consts::LN_2;
    let p = 1.0
        + t * (1.0 + t * (0.5 + t * (1.0 / 6.0 + t * (1.0 / 24.0 + t * (1.0 / 120.0 + t * (1.0 / 720.0))))));
    p * f32::from_bits((((n as i32) + 127) as u32) << 23)
}

#[inline(always)]
fn finite_or_zero(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

// ---- Folder ----

const HB_N: usize = 23;
const HB_C: usize = 11;

/// Sine wavefolder `y = sin(g x + bias)`, oversampled 2x through a half-band
/// filter pair (upsample, fold at 2x, downsample).
/// `amt` 0..1 maps to drive g from 1 up to about 10; at amt 0 the folder is
/// bypassed exactly (`process` returns x unchanged, filters not run). `bias` -1..1 is a phase
/// offset in radians times pi/2. Output level should stay near the input level.
pub struct Folder {
    g: f32,
    bias: f32,
    dc: f32,
    makeup: f32,
    bypass: bool,
    /// Half-band taps at even history indices 0, 2, .. 10 (mirrored), centre tap.
    tap: [f32; 6],
    centre: f32,
    up: [f32; HB_N],
    dn: [f32; HB_N],
}

impl Folder {
    pub fn new(sample_rate: f32) -> Self {
        let _ = sample_rate;
        // Blackman-windowed half-band, odd offsets only, normalised to unity DC gain.
        let mut h = [0.0f32; HB_N];
        for (n, v) in h.iter_mut().enumerate() {
            let d = n as f32 - HB_C as f32;
            let w = 0.42 - 0.5 * (TAU * n as f32 / (HB_N - 1) as f32).cos()
                + 0.08 * (2.0 * TAU * n as f32 / (HB_N - 1) as f32).cos();
            *v = if n == HB_C {
                0.5
            } else if (n as i32 - HB_C as i32) % 2 != 0 {
                (PI * d / 2.0).sin() / (PI * d) * w
            } else {
                0.0
            };
        }
        let sum: f32 = h.iter().sum();
        for v in h.iter_mut() {
            *v /= sum;
        }
        let mut tap = [0.0; 6];
        for (j, t) in tap.iter_mut().enumerate() {
            *t = h[2 * j];
        }
        Self {
            g: 1.0,
            bias: 0.0,
            dc: 0.0,
            makeup: 1.0,
            bypass: true,
            tap,
            centre: h[HB_C],
            up: [0.0; HB_N],
            dn: [0.0; HB_N],
        }
    }
    pub fn set(&mut self, amt: f32, bias: f32) {
        let amt = if amt.is_finite() { amt.clamp(0.0, 1.0) } else { 0.0 };
        let bias = if bias.is_finite() { bias.clamp(-1.0, 1.0) } else { 0.0 };
        if amt <= 0.0 {
            self.bypass = true;
            return;
        }
        if self.bypass {
            self.up = [0.0; HB_N];
            self.dn = [0.0; HB_N];
        }
        self.bypass = false;
        self.g = 1.0 + 9.0 * amt;
        self.bias = bias * (PI * 0.5);
        self.dc = fast_sin(self.bias);
        self.makeup = 1.0 / (1.0 + amt);
    }
    #[inline(always)]
    fn fir(&self, h: &[f32; HB_N]) -> f32 {
        let mut s = self.centre * h[HB_C];
        for j in 0..6 {
            s += self.tap[j] * (h[2 * j] + h[HB_N - 1 - 2 * j]);
        }
        s
    }
    #[inline(always)]
    fn fold(&self, v: f32) -> f32 {
        let a = (self.g * v + self.bias).clamp(-200.0, 200.0);
        (fast_sin(a) - self.dc) * self.makeup
    }
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let x = finite_or_zero(x);
        if self.bypass {
            return x;
        }
        // Upsample: zero-stuff, filter, gain 2.
        self.up.copy_within(0..HB_N - 1, 1);
        self.up[0] = x;
        let a = 2.0 * self.fir(&self.up);
        self.up.copy_within(0..HB_N - 1, 1);
        self.up[0] = 0.0;
        let b = 2.0 * self.fir(&self.up);
        let (fa, fb) = (self.fold(a), self.fold(b));
        // Downsample: filter, keep every other sample.
        self.dn.copy_within(0..HB_N - 1, 1);
        self.dn[0] = fa;
        self.dn.copy_within(0..HB_N - 1, 1);
        self.dn[0] = fb;
        self.fir(&self.dn)
    }
    pub fn reset(&mut self) {
        self.up = [0.0; HB_N];
        self.dn = [0.0; HB_N];
    }
}

// ---- Clang ----

/// Longest PM delay swing, seconds (peak to peak is twice this).
const CLANG_DEPTH_S: f32 = 0.0003;

/// Clang: metallic modulation of a voice by a sine at `ratio * f0`.
/// Phase modulation is done as a time warp: the input passes through a short
/// fractional delay whose length is modulated by the sine, which phase
/// modulates every partial in proportion to its frequency. That is crossfaded
/// with ring modulation by the same sine: amt 0..0.5 raises the PM index from
/// 0 to its maximum, 0.5..1 crossfades PM toward ring modulation.
/// At amt 0 `process` returns x unchanged.
pub struct Clang {
    sr: f32,
    buf: DelayLine,
    phase: f32,
    inc: f32,
    depth: f32,
    rm: f32,
    bypass: bool,
}

impl Clang {
    pub fn new(sample_rate: f32) -> Self {
        let depth_max = (CLANG_DEPTH_S * sample_rate).ceil() as usize;
        Self {
            sr: sample_rate,
            buf: DelayLine::new(2 * depth_max + 8),
            phase: 0.0,
            inc: 0.0,
            depth: 0.0,
            rm: 0.0,
            bypass: true,
        }
    }
    pub fn set(&mut self, f0_hz: f32, ratio: f32, amt: f32) {
        let amt = if amt.is_finite() { amt.clamp(0.0, 1.0) } else { 0.0 };
        if amt <= 0.0 {
            self.bypass = true;
            return;
        }
        if self.bypass {
            self.reset();
        }
        self.bypass = false;
        let f = finite_or_zero(f0_hz * ratio).clamp(0.0, 0.45 * self.sr);
        self.inc = TAU * f / self.sr;
        let pm = (amt * 2.0).min(1.0);
        self.depth = CLANG_DEPTH_S * self.sr * pm;
        self.rm = ((amt - 0.5) * 2.0).max(0.0);
    }
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let x = finite_or_zero(x);
        if self.bypass {
            return x;
        }
        self.buf.push(x);
        let s = fast_sin(self.phase);
        self.phase += self.inc;
        if self.phase > PI {
            self.phase -= TAU;
        }
        let ring = x * s;
        if self.rm >= 1.0 {
            return ring;
        }
        let pm = self.buf.read_cubic(2.0 + self.depth * (1.0 + s));
        pm + self.rm * (ring - pm)
    }
    pub fn reset(&mut self) {
        self.buf.clear();
        self.phase = 0.0;
    }
}

// ---- CombPair ----

const COMB_MAX_S: f32 = 0.05;
const COMB_DAMP_HZ: f32 = 6000.0;

#[derive(Clone, Copy, Default)]
struct CombLine {
    n: usize,
    c: f32,
    g: f32,
    ap_x: f32,
    ap_y: f32,
    lp: f32,
}

/// Two feedback combs in parallel, tuned to `f0 * tune` and an octave-ish
/// detuned partner (`f0 * tune * 1.5`), each with a one-pole damping low-pass
/// in the loop and a fractional delay (allpass interpolation, so the loop
/// stays in tune). Feedback is set so the loop decays 60 dB in `decay_s`.
/// `amt` 0..1 is a dry/wet crossfade; at 0 `process` returns x unchanged and
/// the combs are not run. The longest period is 50 ms (`f0 * tune` below
/// 20 Hz is clamped to 20 Hz).
pub struct CombPair {
    sr: f32,
    dl: [DelayLine; 2],
    ln: [CombLine; 2],
    lp_a: f32,
    amt: f32,
    bypass: bool,
}

impl CombPair {
    pub fn new(sample_rate: f32) -> Self {
        let max = (COMB_MAX_S * sample_rate).ceil() as usize + 8;
        let mut ln = [CombLine::default(); 2];
        for l in ln.iter_mut() {
            l.n = 1;
        }
        Self {
            sr: sample_rate,
            dl: [DelayLine::new(max), DelayLine::new(max)],
            ln,
            lp_a: 1.0 - (-TAU * COMB_DAMP_HZ / sample_rate).exp(),
            amt: 0.0,
            bypass: true,
        }
    }
    pub fn set(&mut self, f0_hz: f32, tune: f32, decay_s: f32, amt: f32) {
        let amt = if amt.is_finite() { amt.clamp(0.0, 1.0) } else { 0.0 };
        if amt <= 0.0 {
            self.bypass = true;
            self.amt = 0.0;
            return;
        }
        if self.bypass {
            self.reset();
        }
        self.bypass = false;
        self.amt = amt;
        let base = finite_or_zero(f0_hz * tune).max(20.0);
        let decay = if decay_s.is_finite() { decay_s.max(0.005) } else { 0.005 };
        let lp_delay = (1.0 - self.lp_a) / self.lp_a;
        let max_d = self.dl[0].max_delay() as f32 - 2.0;
        for (i, mul) in [1.0f32, 1.5].iter().enumerate() {
            let f = (base * mul).min(0.4 * self.sr);
            let d = (self.sr / f).min(COMB_MAX_S * self.sr);
            let dp = (d - lp_delay).clamp(1.5, max_d);
            let n = (dp - 0.5).floor();
            let frac = dp - n;
            let l = &mut self.ln[i];
            l.n = n as usize;
            l.c = (1.0 - frac) / (1.0 + frac);
            l.g = (-3.0 * core::f32::consts::LN_10 * (d / self.sr) / decay).exp().min(0.9999);
        }
    }
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let x = finite_or_zero(x);
        if self.bypass {
            return x;
        }
        let mut wet = 0.0;
        for i in 0..2 {
            let l = &mut self.ln[i];
            let yd = self.dl[i].tap(l.n - 1);
            let a = l.c * yd + l.ap_x - l.c * l.ap_y;
            l.ap_x = yd;
            l.ap_y = a;
            l.lp += self.lp_a * (a - l.lp);
            let y = (1.0 - l.g) * x + l.g * l.lp + DENORMAL;
            self.dl[i].push(y);
            wet += y;
        }
        x + self.amt * (0.6 * wet - x)
    }
    pub fn reset(&mut self) {
        for d in self.dl.iter_mut() {
            d.clear();
        }
        for l in self.ln.iter_mut() {
            l.ap_x = 0.0;
            l.ap_y = 0.0;
            l.lp = 0.0;
        }
    }
}

// ---- Click ----

const CLICK_S: f32 = 0.002;
const CLICK_Q: f32 = 2.0;

/// Strike click: a 2 ms raised-cosine-windowed noise burst through one
/// band-pass (SVF, Q about 2) centred at `color_hz`, normalised so
/// `trigger(1.0, ..)` peaks near 0.5. `trigger` may be called while a burst is
/// still running (restart). Between bursts `process` returns 0 and costs only
/// a branch. Used for both the start click and the end click.
pub struct Click {
    sr: f32,
    noise: Noise,
    svf: Svf,
    gain: f32,
    len: usize,
    n: usize,
    max_n: usize,
    on: bool,
}

impl Click {
    pub fn new(sample_rate: f32, seed: u32) -> Self {
        Self {
            sr: sample_rate,
            noise: Noise::new(seed),
            svf: Svf::default(),
            gain: 0.0,
            len: ((CLICK_S * sample_rate).round() as usize).max(2),
            n: 0,
            max_n: 0,
            on: false,
        }
    }
    pub fn trigger(&mut self, amount: f32, color_hz: f32) {
        let amount = if amount.is_finite() { amount.clamp(0.0, 1.0) } else { 0.0 };
        if amount <= 0.0 {
            return;
        }
        let f = if color_hz.is_finite() { color_hz.clamp(40.0, 0.45 * self.sr) } else { 2000.0 };
        self.svf = Svf::new(self.sr, f, CLICK_Q);
        // Band-limited rms of windowed uniform noise through the band-pass,
        // peak taken as 3 rms; the narrow-band estimate is capped for low f.
        let bw = PI * f / (2.0 * CLICK_Q);
        let frac = (bw / (0.5 * self.sr)).clamp(0.02, 1.0);
        let rms = (frac / 3.0).sqrt() * 0.612;
        self.gain = amount * 0.5 / (3.0 * rms);
        self.n = 0;
        self.max_n = self.len + (0.2 * self.sr) as usize;
        self.on = true;
    }
    #[inline]
    pub fn process(&mut self) -> f32 {
        if !self.on {
            return 0.0;
        }
        self.step()
    }
    fn step(&mut self) -> f32 {
        let x = if self.n < self.len {
            let w = 0.5 - 0.5 * fast_sin_cos(TAU * self.n as f32 / self.len as f32).1;
            self.noise.next() * w
        } else {
            0.0
        };
        let y = self.svf.bandpass_norm(x) * self.gain;
        self.n += 1;
        if self.n >= self.len && (y.abs() < 1e-6 || self.n >= self.max_n || !y.is_finite()) {
            self.on = false;
            self.svf.reset();
            return if y.is_finite() { y } else { 0.0 };
        }
        y
    }
    pub fn active(&self) -> bool {
        self.on
    }
    pub fn reset(&mut self) {
        self.on = false;
        self.svf.reset();
    }
}

// ---- Lfo, Chaos ----

/// Sine LFO evaluated at control rate. Output -1..1. `reset` puts the phase
/// at 0 (sin 0) so a note start and an export start are repeatable.
#[derive(Clone, Copy, Debug, Default)]
pub struct Lfo {
    phase: f32,
}

impl Lfo {
    pub fn new() -> Self {
        Self { phase: 0.0 }
    }
    /// Advance by `dt` seconds at `rate_hz` and return the new value.
    pub fn tick(&mut self, rate_hz: f32, dt: f32) -> f32 {
        let inc = rate_hz * dt;
        if inc.is_finite() && inc > 0.0 {
            self.phase += inc;
            self.phase -= self.phase.floor();
        }
        (TAU * self.phase).sin()
    }
    pub fn reset(&mut self) {
        self.phase = 0.0;
    }
}

/// Smoothed random source evaluated at control rate: a new uniform random
/// target in -1..1 at `rate_hz`, approached by cosine interpolation, so the
/// output is continuous with no steps. Output -1..1.
#[derive(Clone, Copy, Debug)]
pub struct Chaos {
    noise: Noise,
    prev: f32,
    next: f32,
    t: f32,
}

impl Chaos {
    pub fn new(seed: u32) -> Self {
        let mut c = Self { noise: Noise::new(seed), prev: 0.0, next: 0.0, t: 0.0 };
        c.reset(seed);
        c
    }
    pub fn tick(&mut self, rate_hz: f32, dt: f32) -> f32 {
        let inc = rate_hz * dt;
        if inc.is_finite() && inc > 0.0 {
            self.t += inc;
            // More than one target per tick only happens at absurd rates.
            let mut guard = 0;
            while self.t >= 1.0 && guard < 8 {
                self.t -= 1.0;
                self.prev = self.next;
                self.next = self.noise.next();
                guard += 1;
            }
            if self.t >= 1.0 {
                self.t = 0.0;
            }
        }
        let w = 0.5 - 0.5 * (PI * self.t).cos();
        (self.prev + (self.next - self.prev) * w).clamp(-1.0, 1.0)
    }
    /// Restart the sequence from 0 (so a note start is repeatable).
    pub fn reset(&mut self, seed: u32) {
        self.noise = Noise::new(seed);
        self.prev = 0.0;
        self.next = self.noise.next();
        self.t = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    /// Hann-windowed single-frequency magnitude.
    fn mag(x: &[f32], f: f32) -> f32 {
        let n = x.len();
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, &v) in x.iter().enumerate() {
            let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos();
            let ph = std::f64::consts::TAU * f as f64 * i as f64 / SR as f64;
            re += v as f64 * w * ph.cos();
            im += v as f64 * w * ph.sin();
        }
        ((re * re + im * im).sqrt() * 2.0 / n as f64) as f32
    }

    fn sine(f: f32, n: usize, a: f32) -> Vec<f32> {
        (0..n).map(|i| a * (TAU * f * i as f32 / SR).sin()).collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn fast_math_accuracy() {
        let mut worst = 0.0f64;
        let mut x = -7.0f32;
        while x < 7.0 {
            let (s, c) = fast_sin_cos(x);
            worst = worst.max((s as f64 - (x as f64).sin()).abs()).max((c as f64 - (x as f64).cos()).abs());
            x += 0.00137;
        }
        assert!(worst < 1e-6, "sincos err {worst}");
        let mut x = 1e-3f32;
        while x < 1e4 {
            let e = (fast_log2(x) as f64 - (x as f64).log2()).abs();
            assert!(e < 1e-5, "log2 {x} err {e}");
            x *= 1.013;
        }
        let mut x = -20.0f32;
        while x < 20.0 {
            let r = fast_exp2(x) as f64 / (x as f64).exp2();
            assert!((r - 1.0).abs() < 1e-5, "exp2 {x} rel {r}");
            x += 0.0173;
        }
    }

    #[test]
    fn bypass_is_exact_at_amount_zero() {
        let xs = sine(440.0, 256, 0.7);
        let mut f = Folder::new(SR);
        f.set(0.0, 0.3);
        let mut c = Clang::new(SR);
        c.set(200.0, 2.0, 0.0);
        let mut k = CombPair::new(SR);
        k.set(200.0, 1.0, 0.5, 0.0);
        for &x in &xs {
            assert_eq!(f.process(x), x);
            assert_eq!(c.process(x), x);
            assert_eq!(k.process(x), x);
        }
    }

    #[test]
    fn fold_adds_harmonics_and_keeps_level() {
        let n = 9600;
        let x = sine(1000.0, n, 0.4);
        let mut f = Folder::new(SR);
        f.set(0.7, 0.0);
        let y: Vec<f32> = x.iter().map(|&v| f.process(v)).collect();
        let y = &y[2000..];
        let x = &x[2000..];
        let h3 = mag(y, 3000.0);
        let h3_in = mag(x, 3000.0);
        assert!(h3 > 0.02 && h3_in < 1e-4, "h3 {h3}");
        assert!(rms(y) > 0.3 * rms(x) && rms(y) < 3.0 * rms(x), "level {} vs {}", rms(y), rms(x));
        // Nothing alias-sized between the harmonics.
        assert!(y.iter().all(|v| v.is_finite() && v.abs() < 1.5));
    }

    #[test]
    fn comb_resonates_and_decays_60_db() {
        let f0 = 200.0;
        let decay = 0.2;
        let mut k = CombPair::new(SR);
        k.set(f0, 1.0, decay, 1.0);
        // Steady state gain at the tuned frequency vs a detuned one.
        let run = |k: &mut CombPair, f: f32| -> f32 {
            k.reset();
            let x = sine(f, 24_000, 0.3);
            let y: Vec<f32> = x.iter().map(|&v| k.process(v)).collect();
            rms(&y[12_000..]) / rms(&x[12_000..])
        };
        let on = run(&mut k, f0);
        let off = run(&mut k, f0 * 1.1);
        assert!(on > 0.5 && on > 3.0 * off, "on {on} off {off}");
        // Decay: drive at f0, stop, compare two windows 0.1 s apart.
        k.reset();
        let mut y = Vec::new();
        for i in 0..48_000 {
            let x = if i < 24_000 { 0.3 * (TAU * f0 * i as f32 / SR).sin() } else { 0.0 };
            y.push(k.process(x));
        }
        let a = rms(&y[24_000 + 2400..24_000 + 4800]);
        let b = rms(&y[24_000 + 7200..24_000 + 9600]);
        let db = 20.0 * (a / b).log10();
        // 0.1 s apart at 60 dB per 0.2 s is 30 dB.
        assert!((db - 30.0).abs() < 6.0, "{db} dB per 100 ms");
    }

    #[test]
    fn comb_stays_tuned_for_odd_periods() {
        // 48000/337 is not an integer number of samples; the resonance must
        // still sit on 337 Hz, not on the nearest integer-delay frequency.
        let mut k = CombPair::new(SR);
        k.set(337.0, 1.0, 0.3, 1.0);
        let x = sine(337.0, 24_000, 0.3);
        let y: Vec<f32> = x.iter().map(|&v| k.process(v)).collect();
        let on = rms(&y[12_000..]);
        k.reset();
        let x2 = sine(337.0 * 1.03, 24_000, 0.3);
        let y2: Vec<f32> = x2.iter().map(|&v| k.process(v)).collect();
        assert!(on > 2.0 * rms(&y2[12_000..]), "{on} vs {}", rms(&y2[12_000..]));
    }

    #[test]
    fn clang_ring_makes_sidebands() {
        let (f, f0, ratio) = (1000.0, 200.0, 2.0);
        let mut c = Clang::new(SR);
        c.set(f0, ratio, 1.0);
        let n = 9600;
        let x = sine(f, n, 0.5);
        let y: Vec<f32> = x.iter().map(|&v| c.process(v)).collect();
        let y = &y[1000..];
        let lo = mag(y, f - ratio * f0);
        let hi = mag(y, f + ratio * f0);
        let mid = mag(y, f);
        assert!(lo > 0.1 && hi > 0.1, "{lo} {hi}");
        assert!(mid < 0.02, "carrier leak {mid}");
    }

    #[test]
    fn clang_pm_adds_sidebands_but_keeps_carrier() {
        let mut c = Clang::new(SR);
        c.set(200.0, 2.0, 0.5);
        let x = sine(1000.0, 9600, 0.5);
        let y: Vec<f32> = x.iter().map(|&v| c.process(v)).collect();
        let y = &y[1000..];
        assert!(mag(y, 1400.0) > 0.03 && mag(y, 600.0) > 0.03);
        assert!(y.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn click_is_a_short_burst_at_the_colour() {
        let mut c = Click::new(SR, 7);
        assert_eq!(c.process(), 0.0);
        c.trigger(1.0, 3000.0);
        assert!(c.active());
        let y: Vec<f32> = (0..2400).map(|_| c.process()).collect();
        let peak = y.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.25 && peak < 1.0, "peak {peak}");
        let total: f32 = y.iter().map(|v| v * v).sum();
        let early: f32 = y[..(0.003 * SR) as usize].iter().map(|v| v * v).sum();
        assert!(early > 0.95 * total, "energy outside 3 ms: {}", 1.0 - early / total);
        assert!(!c.active());
        // Centre frequency: more energy near 3 kHz than near 300 Hz or 15 kHz.
        let p = |f| mag(&y[..400], f);
        assert!(p(3000.0) > 2.0 * p(300.0) && p(3000.0) > 2.0 * p(15_000.0));
        // Retrigger mid-burst restarts and stays finite.
        c.trigger(1.0, 500.0);
        for _ in 0..50 {
            c.process();
        }
        c.trigger(0.5, 8000.0);
        for _ in 0..4800 {
            assert!(c.process().is_finite());
        }
        assert!(!c.active());
    }

    #[test]
    fn lfo_period_matches_rate() {
        let mut l = Lfo::new();
        let (rate, dt) = (5.0, 32.0 / SR);
        let mut prev = 0.0;
        let mut ups = Vec::new();
        for i in 1..(SR as usize * 2) / 32 {
            let v = l.tick(rate, dt);
            assert!((-1.0..=1.0).contains(&v));
            if prev < 0.0 && v >= 0.0 {
                ups.push(i as f32 * dt);
            }
            prev = v;
        }
        assert!(ups.len() >= 9);
        let period = (ups[ups.len() - 1] - ups[0]) / (ups.len() - 1) as f32;
        assert!((period - 0.2).abs() < 0.002, "period {period}");
        l.reset();
        assert_eq!(l.tick(0.0, dt), 0.0);
    }

    #[test]
    fn chaos_is_continuous_bounded_and_repeatable() {
        let mut c = Chaos::new(3);
        let dt = 32.0 / SR;
        let mut prev = 0.0;
        let mut seen = Vec::new();
        for _ in 0..20_000 {
            let v = c.tick(8.0, dt);
            assert!((-1.0..=1.0).contains(&v));
            // Fastest swing: 2 per half a cosine step of 8 Hz * dt.
            assert!((v - prev).abs() < 0.05, "step {}", (v - prev).abs());
            prev = v;
            seen.push(v);
        }
        assert!(seen.iter().any(|&v| v > 0.5) && seen.iter().any(|&v| v < -0.5));
        c.reset(3);
        let again: Vec<f32> = (0..100).map(|_| c.tick(8.0, dt)).collect();
        assert_eq!(&again[..], &seen[..100]);
    }

    #[test]
    fn nan_input_gives_finite_output() {
        let mut f = Folder::new(SR);
        f.set(0.5, 0.2);
        let mut c = Clang::new(SR);
        c.set(200.0, 1.5, 0.7);
        let mut k = CombPair::new(SR);
        k.set(200.0, 1.0, 0.3, 0.8);
        for i in 0..2000 {
            let x = match i % 5 {
                0 => f32::NAN,
                1 => f32::INFINITY,
                2 => f32::NEG_INFINITY,
                _ => 0.3,
            };
            assert!(f.process(x).is_finite());
            assert!(c.process(x).is_finite());
            assert!(k.process(x).is_finite());
        }
        // And the state recovers.
        for _ in 0..4000 {
            assert!(k.process(0.0).is_finite());
        }
        let mut l = Lfo::new();
        assert!(l.tick(f32::NAN, 0.001).is_finite());
        let mut ch = Chaos::new(1);
        assert!(ch.tick(f32::NAN, 0.001).is_finite());
        assert!(ch.tick(1e9, 1.0).is_finite());
    }
}
