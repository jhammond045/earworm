//! Voice modules and modulation sources (docs/synth.md "Module catalog").
//!
//! The partial bank, the envelopes and the LFOs are the code that used to
//! live inside the one fixed Klont voice, moved here unchanged so the classic
//! patch sounds as it did. The smaller processors (click, filter, fold,
//! clang, comb, amp) are thin wrappers in `voice.rs` around `dsp::fx`.
//! The oscillator, noise and sampler sources are new in stage S3.

use crate::voice::{MAX_PARTIALS, N_BANDS, TICK};
use dsp::bands::band_centres;
use dsp::fx::{Chaos, Lfo, fast_exp2, fast_log2, fast_sin, fast_sin_cos};
use dsp::noise::Noise;
use std::f32::consts::{LOG2_E, TAU};

/// Summed partial power after normalisation: sum of a_k^2 / 2.
const PARTIAL_POWER: f32 = 0.03;
/// Width of a formant bell in octaves (sigma of a Gaussian in log frequency).
const FORMANT_SIGMA_OCT: f32 = 0.15;
const DB_FLOOR: f32 = 72.0;

const DRUM_RATIOS: [f32; 12] = [1.0, 1.59, 2.14, 2.30, 2.65, 2.92, 3.16, 3.50, 3.60, 3.65, 4.06, 4.15];
const BAR_RATIOS: [f32; 5] = [1.0, 2.76, 5.40, 8.93, 13.34];

/// Mode ratio of partial `k` (1-based) for the three tables.
fn table_ratio(table: usize, k: usize) -> f32 {
    match table {
        0 => k as f32,
        1 => {
            if k <= DRUM_RATIOS.len() {
                DRUM_RATIOS[k - 1]
            } else {
                // Mode density grows with frequency, so spacing shrinks.
                (k as f32 / 0.697).sqrt()
            }
        }
        _ => {
            if k <= BAR_RATIOS.len() {
                BAR_RATIOS[k - 1]
            } else {
                let m = (2 * k + 1) as f32;
                m * m / 9.0 * 0.9926
            }
        }
    }
}

/// Ratio of partial `k` for `modal` in 0..=2, interpolating adjacent tables.
fn modal_ratio(modal: f32, k: usize) -> f32 {
    let m = modal.clamp(0.0, 2.0);
    let t = (m.floor() as usize).min(1);
    let f = m - t as f32;
    let a = table_ratio(t, k);
    let b = table_ratio(t + 1, k);
    a + (b - a) * f
}

#[inline]
pub fn cl(v: f32, lo: f32, hi: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, hi) } else { 0.0f32.clamp(lo, hi) }
}

// ------------------------------------------------------------------ partials

/// Per-tick controls of the partial bank, real units, already clamped.
#[derive(Clone, Copy, Debug)]
pub struct PartialsCtl {
    pub tilt: f32,
    pub oddeven: f32,
    pub stretch: f32,
    pub jitter: f32,
    pub modal: f32,
    pub damp: f32,
    pub f1_freq: f32,
    pub f1_gain: f32,
    pub f2_freq: f32,
    pub f2_gain: f32,
    pub tone_bands: [f32; N_BANDS],
    /// Most partials this voice may sound this tick (the global budget's share).
    pub max_partials: usize,
}

impl PartialsCtl {
    pub fn clean(mut self) -> Self {
        self.tilt = cl(self.tilt, -48.0, 48.0);
        self.oddeven = cl(self.oddeven, -1.0, 1.0);
        // sqrt(1 + stretch k^2) must stay real up to k = 64.
        self.stretch = cl(self.stretch, -2.0e-4, 0.05);
        self.jitter = cl(self.jitter, 0.0, 200.0);
        self.modal = cl(self.modal, 0.0, 2.0);
        self.damp = cl(self.damp, 0.0, 4.0);
        self.f1_freq = cl(self.f1_freq, 20.0, 20_000.0);
        self.f1_gain = cl(self.f1_gain, -60.0, 60.0);
        self.f2_freq = cl(self.f2_freq, 20.0, 20_000.0);
        self.f2_gain = cl(self.f2_gain, -60.0, 60.0);
        for d in self.tone_bands.iter_mut() {
            *d = cl(*d, -60.0, 60.0);
        }
        self.max_partials = self.max_partials.min(MAX_PARTIALS);
        self
    }
}

/// Inputs that change the cached partial levels.
#[derive(Clone, Copy, PartialEq)]
struct SpecKey {
    cents: f32,
    tilt: f32,
    oddeven: f32,
    f1f: f32,
    f1g: f32,
    f2f: f32,
    f2g: f32,
    tone: [f32; N_BANDS],
    max_partials: usize,
}

/// The partial bank: up to 64 sine partials from rotators, grouped four at a
/// time. Index k is partial k + 1.
pub struct Partials {
    sr: f32,
    noise: Noise,
    c: PartialsCtl,
    re: [f32; MAX_PARTIALS],
    im: [f32; MAX_PARTIALS],
    cr: [f32; MAX_PARTIALS],
    ci: [f32; MAX_PARTIALS],
    amp: [f32; MAX_PARTIALS],
    step: [f32; MAX_PARTIALS],
    tgt: [f32; MAX_PARTIALS],
    /// Normalised level per partial (before envelope, ring, amp).
    lv: [f32; MAX_PARTIALS],
    ring: [f32; MAX_PARTIALS],
    ring_rate: [f32; MAX_PARTIALS],
    ring_fac: [f32; MAX_PARTIALS],
    jit_r: [f32; MAX_PARTIALS],
    /// log2 of the modal ratio.
    rlr: [f32; MAX_PARTIALS],
    /// Frequency multiplier on f0 (ratio, stretch, jitter).
    fm: [f32; MAX_PARTIALS],
    /// log2(fm).
    flg: [f32; MAX_PARTIALS],
    /// Radians per sample per Hz of f0.
    wm: [f32; MAX_PARTIALS],
    fm_key: [f32; 3],
    fm_dirty: bool,
    ring_key: [f32; 3],
    ring_dt: f32,
    ring_dirty: bool,
    spec_key: Option<SpecKey>,
    centres_log2: [f32; N_BANDS],
    centres_inv: [f32; N_BANDS],
    n_chunks: usize,
    n_wanted: usize,
    n_kept: usize,
    scratch: [[f32; 4]; TICK],
}

impl Partials {
    pub fn new(sr: f32, seed: u32) -> Self {
        let mut centres = [0.0f32; N_BANDS];
        band_centres(N_BANDS, &mut centres);
        let mut centres_log2 = [0.0; N_BANDS];
        let mut centres_inv = [0.0; N_BANDS];
        for i in 0..N_BANDS {
            centres_log2[i] = centres[i].log2();
        }
        for i in 0..N_BANDS - 1 {
            centres_inv[i] = 1.0 / (centres_log2[i + 1] - centres_log2[i]);
        }
        Partials {
            sr,
            noise: Noise::new(seed),
            c: PartialsCtl {
                tilt: -6.0,
                oddeven: 0.0,
                stretch: 0.0,
                jitter: 0.0,
                modal: 0.0,
                damp: 0.0,
                f1_freq: 800.0,
                f1_gain: 0.0,
                f2_freq: 2400.0,
                f2_gain: 0.0,
                tone_bands: [0.0; N_BANDS],
                max_partials: MAX_PARTIALS,
            },
            re: [1.0; MAX_PARTIALS],
            im: [0.0; MAX_PARTIALS],
            cr: [1.0; MAX_PARTIALS],
            ci: [0.0; MAX_PARTIALS],
            amp: [0.0; MAX_PARTIALS],
            step: [0.0; MAX_PARTIALS],
            tgt: [0.0; MAX_PARTIALS],
            lv: [0.0; MAX_PARTIALS],
            ring: [1.0; MAX_PARTIALS],
            ring_rate: [0.0; MAX_PARTIALS],
            ring_fac: [1.0; MAX_PARTIALS],
            jit_r: [0.0; MAX_PARTIALS],
            rlr: [0.0; MAX_PARTIALS],
            fm: [1.0; MAX_PARTIALS],
            flg: [0.0; MAX_PARTIALS],
            wm: [0.0; MAX_PARTIALS],
            fm_key: [f32::NAN; 3],
            fm_dirty: true,
            ring_key: [f32::NAN; 3],
            ring_dt: 0.0,
            ring_dirty: true,
            spec_key: None,
            centres_log2,
            centres_inv,
            n_chunks: 0,
            n_wanted: 0,
            n_kept: 0,
            scratch: [[0.0; 4]; TICK],
        }
    }

    pub fn reset(&mut self) {
        self.re = [1.0; MAX_PARTIALS];
        self.im = [0.0; MAX_PARTIALS];
        self.cr = [1.0; MAX_PARTIALS];
        self.ci = [0.0; MAX_PARTIALS];
        self.amp = [0.0; MAX_PARTIALS];
        self.step = [0.0; MAX_PARTIALS];
        self.tgt = [0.0; MAX_PARTIALS];
        self.lv = [0.0; MAX_PARTIALS];
        self.ring = [1.0; MAX_PARTIALS];
        self.n_chunks = 0;
        self.n_wanted = 0;
        self.n_kept = 0;
        self.spec_key = None;
    }

    /// Note start: a restart keeps phases and amplitudes; ring and jitter start over.
    pub fn note_start(&mut self) {
        self.ring = [1.0; MAX_PARTIALS];
        for r in self.jit_r.iter_mut() {
            *r = self.noise.next();
        }
        self.fm_dirty = true;
        self.spec_key = None;
    }

    pub fn wanted(&self) -> usize {
        self.n_wanted
    }
    pub fn kept(&self) -> usize {
        self.n_kept
    }

    fn refresh_freqs(&mut self) {
        let (modal, stretch, jitter) = (self.c.modal, self.c.stretch, self.c.jitter);
        let key = [modal, stretch, jitter];
        if !self.fm_dirty && key == self.fm_key {
            return;
        }
        self.fm_key = key;
        self.fm_dirty = false;
        let w = TAU / self.sr;
        for k in 0..MAX_PARTIALS {
            let kk = (k + 1) as f32;
            let r = modal_ratio(modal, k + 1);
            let jit = fast_exp2(jitter * self.jit_r[k] * (1.0 / 1200.0));
            let fm = r * (1.0 + stretch * kk * kk).max(1e-6).sqrt() * jit;
            self.rlr[k] = fast_log2(r);
            self.fm[k] = fm;
            self.flg[k] = fast_log2(fm);
            self.wm[k] = w * fm;
        }
        self.ring_dirty = true;
    }

    /// Partial levels for the current spectral controls at pitch `f0`.
    fn recompute_levels(&mut self, f0: f32) {
        let c = &self.c;
        let nyq = 0.45 * self.sr;
        let lf0 = fast_log2(f0);
        let l1 = fast_log2(c.f1_freq);
        let l2 = fast_log2(c.f2_freq);
        let oe = c.oddeven * 6.0;
        let (tilt, f1g, f2g) = (c.tilt, c.f1_gain, c.f2_gain);
        let inv_s = 1.0 / FORMANT_SIGMA_OCT;
        let mut db = [f32::NEG_INFINITY; MAX_PARTIALS];
        let mut maxd = f32::NEG_INFINITY;
        let mut bi = 0usize;
        for k in 0..MAX_PARTIALS {
            let f = f0 * self.fm[k];
            if !(f < nyq) {
                continue;
            }
            let lf = lf0 + self.flg[k];
            // 1-based odd partials (k even here) get +oddeven.
            let mut d = tilt * self.rlr[k] + if k % 2 == 0 { oe } else { -oe };
            if f1g != 0.0 {
                let x = (lf - l1) * inv_s;
                d += f1g * fast_exp2(-0.5 * LOG2_E * x * x);
            }
            if f2g != 0.0 {
                let x = (lf - l2) * inv_s;
                d += f2g * fast_exp2(-0.5 * LOG2_E * x * x);
            }
            // Tone bands: partials and bands are both sorted, so the band
            // index only walks forward (and back a step for odd orderings).
            let lc = &self.centres_log2;
            d += if lf <= lc[0] {
                c.tone_bands[0]
            } else if lf >= lc[N_BANDS - 1] {
                c.tone_bands[N_BANDS - 1]
            } else {
                while bi + 2 < N_BANDS && lf >= lc[bi + 1] {
                    bi += 1;
                }
                while bi > 0 && lf < lc[bi] {
                    bi -= 1;
                }
                let t = (lf - lc[bi]) * self.centres_inv[bi];
                c.tone_bands[bi] + t * (c.tone_bands[bi + 1] - c.tone_bands[bi])
            };
            if d.is_finite() {
                db[k] = d;
                if d > maxd {
                    maxd = d;
                }
            }
        }
        let mut lin = [0.0f32; MAX_PARTIALS];
        let mut power = 0.0;
        let mut wanted = 0;
        if maxd.is_finite() {
            for k in 0..MAX_PARTIALS {
                if db[k] > maxd - DB_FLOOR {
                    let l = fast_exp2((db[k] - maxd) * 0.166_096_4);
                    lin[k] = l;
                    power += l * l;
                    wanted += 1;
                }
            }
        }
        // Normalise over everything audible, then cut to the budget so a
        // cut removes the highs without making the rest louder.
        let norm = if power > 0.0 { (PARTIAL_POWER * 2.0 / power).sqrt() } else { 0.0 };
        let mut kept = 0;
        for k in 0..MAX_PARTIALS {
            if lin[k] > 0.0 && kept < c.max_partials {
                self.lv[k] = lin[k] * norm;
                kept += 1;
            } else {
                self.lv[k] = 0.0;
            }
        }
        self.n_wanted = wanted;
        self.n_kept = kept;
    }

    fn update_ring_rates(&mut self, dt: f32, decay: f32) {
        let key = [self.c.damp, decay, self.c.modal];
        if self.ring_dirty || key != self.ring_key {
            self.ring_key = key;
            self.ring_dirty = false;
            self.ring_dt = -1.0;
            let damp = self.c.damp;
            for k in 0..MAX_PARTIALS {
                // tau_k = decay * ratio^(-2 damp); the main envelope already
                // supplies 1 / decay, so the ring adds the difference.
                let p = fast_exp2(2.0 * damp * self.rlr[k]);
                self.ring_rate[k] = (p - 1.0) / decay;
            }
        }
        if dt != self.ring_dt {
            self.ring_dt = dt;
            let s = -dt * LOG2_E;
            for k in 0..MAX_PARTIALS {
                self.ring_fac[k] = fast_exp2(self.ring_rate[k] * s);
            }
        }
    }

    /// Control tick. `cents`/`f0`: the bank's pitch. `g`: overall amplitude
    /// (level, the fused amp's gain). `decay` and `attack` come from the
    /// voice's first envelope: the drum ring is relative to its decay and
    /// holds during its attack. Amplitudes ramp over `len` samples.
    #[allow(clippy::too_many_arguments)]
    pub fn control(&mut self, c: &PartialsCtl, cents: f32, f0: f32, g: f32, decay: f32, attack: bool, len: usize) {
        self.c = *c;
        let dt = len as f32 / self.sr;
        self.refresh_freqs();
        let c = self.c;
        let key = SpecKey {
            cents,
            tilt: c.tilt,
            oddeven: c.oddeven,
            f1f: c.f1_freq,
            f1g: c.f1_gain,
            f2f: c.f2_freq,
            f2g: c.f2_gain,
            tone: c.tone_bands,
            max_partials: c.max_partials,
        };
        let redo = match &self.spec_key {
            None => true,
            Some(old) => (old.cents - cents).abs() > 3.0 || old.tilt != key.tilt
                || old.oddeven != key.oddeven
                || old.f1f != key.f1f
                || old.f1g != key.f1g
                || old.f2f != key.f2f
                || old.f2g != key.f2g
                || old.tone != key.tone
                || old.max_partials != key.max_partials,
        };
        if redo {
            self.spec_key = Some(key);
            self.recompute_levels(f0);
        }

        // Drum ring: after the attack every partial also decays on its own clock.
        if c.damp > 0.0 {
            self.update_ring_rates(dt, decay);
            if !attack {
                for k in 0..MAX_PARTIALS {
                    self.ring[k] *= self.ring_fac[k];
                }
            }
        }

        let g = if g.is_finite() { g } else { 0.0 };
        let mut hi = 0;
        for k in 0..MAX_PARTIALS {
            let t = self.lv[k] * g * self.ring[k];
            self.tgt[k] = if t > 1e-9 { t } else { 0.0 };
            if self.tgt[k] > 0.0 || self.amp[k] > 0.0 {
                hi = k + 1;
            }
        }
        self.n_chunks = hi.div_ceil(4);
        let inv = 1.0 / len as f32;
        for k in 0..MAX_PARTIALS {
            self.step[k] = (self.tgt[k] - self.amp[k]) * inv;
        }
        if g <= 0.0 {
            self.n_kept = 0;
        }

        // Rotator coefficients (the pitch moved) and renormalisation.
        let nb = self.n_chunks * 4;
        for k in 0..nb {
            let (s, co) = fast_sin_cos(f0 * self.wm[k]);
            self.ci[k] = s;
            self.cr[k] = co;
        }
        for k in 0..nb {
            let s = 1.5 - 0.5 * (self.re[k] * self.re[k] + self.im[k] * self.im[k]);
            self.re[k] *= s;
            self.im[k] *= s;
        }
    }

    /// Write the next `out.len()` samples (within one control span).
    pub fn render(&mut self, out: &mut [f32]) {
        let m = out.len();
        let nch = self.n_chunks;
        if nch == 0 {
            out.fill(0.0);
            return;
        }
        for a in self.scratch[..m].iter_mut() {
            *a = [0.0; 4];
        }
        let scratch = &mut self.scratch;
        for ch in 0..nch {
            let b = ch * 4;
            let mut re: [f32; 4] = self.re[b..b + 4].try_into().unwrap();
            let mut im: [f32; 4] = self.im[b..b + 4].try_into().unwrap();
            let cr: [f32; 4] = self.cr[b..b + 4].try_into().unwrap();
            let ci: [f32; 4] = self.ci[b..b + 4].try_into().unwrap();
            let mut amp: [f32; 4] = self.amp[b..b + 4].try_into().unwrap();
            let st: [f32; 4] = self.step[b..b + 4].try_into().unwrap();
            for acc in scratch[..m].iter_mut() {
                for j in 0..4 {
                    let nr = re[j] * cr[j] - im[j] * ci[j];
                    let ni = re[j] * ci[j] + im[j] * cr[j];
                    re[j] = nr;
                    im[j] = ni;
                    amp[j] += st[j];
                    acc[j] += amp[j] * ni;
                }
            }
            self.re[b..b + 4].copy_from_slice(&re);
            self.im[b..b + 4].copy_from_slice(&im);
            self.amp[b..b + 4].copy_from_slice(&amp);
        }
        for (o, a) in out.iter_mut().zip(self.scratch[..m].iter()) {
            *o = (a[0] + a[1]) + (a[2] + a[3]);
        }
    }

    /// The control span ended: amplitudes sit on their targets.
    pub fn span_end(&mut self) {
        let nb = self.n_chunks * 4;
        for k in 0..nb {
            self.amp[k] = self.tgt[k];
            self.step[k] = 0.0;
        }
    }
}

// ------------------------------------------------------------------ oscillator

/// Two-sample polyBLEP: the residual of a unit step at phase 0, for phase `t` moving `dt` a sample.
#[inline]
fn blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let x = t / dt;
        x + x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + x + x + 1.0
    } else {
        0.0
    }
}

/// Two-sample polyBLAMP: the integrated residual, for a change of slope at phase 0.
#[inline]
fn blamp(t: f32, dt: f32) -> f32 {
    if t < dt {
        let x = t / dt - 1.0;
        -1.0 / 3.0 * x * x * x
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt + 1.0;
        1.0 / 3.0 * x * x * x
    } else {
        0.0
    }
}

#[inline]
fn wrap01(x: f32) -> f32 {
    x - x.floor()
}

/// Wave `w` (0 sine, 1 triangle, 2 saw, 3 pulse of width `pw`) at phase `t`, moving `dt` cycles a
/// sample. Every wave starts at 0 and rises, except the pulse; the pulse has no DC.
#[inline]
fn osc_wave(w: u8, t: f32, dt: f32, pw: f32) -> f32 {
    match w {
        0 => fast_sin(TAU * t),
        1 => {
            let mut y = 4.0 * t;
            if y >= 3.0 {
                y -= 4.0;
            } else if y > 1.0 {
                y = 2.0 - y;
            }
            y + 4.0 * dt * (blamp(wrap01(t + 0.25), dt) - blamp(wrap01(t + 0.75), dt))
        }
        2 => {
            let s = wrap01(t + 0.5);
            2.0 * s - 1.0 - blep(s, dt)
        }
        _ => {
            let y = if t < pw { 2.0 - 2.0 * pw } else { -2.0 * pw };
            y + blep(t, dt) - blep(wrap01(t + 1.0 - pw), dt)
        }
    }
}

/// A band-limited oscillator (polyBLEP and polyBLAMP). The wave is a morph: sine at 0, triangle
/// at 1/3, saw at 2/3, pulse at 1; between two waves it crossfades. A phase input (the `pm` port)
/// moves the phase sample by sample, so one oscillator can frequency-modulate another.
pub struct Osc {
    phase: f32,
    /// The phase actually played at the last sample (with the phase input), for its step size.
    last: f32,
}

impl Default for Osc {
    fn default() -> Self {
        Self::new()
    }
}

impl Osc {
    pub fn new() -> Self {
        Osc { phase: 0.0, last: 0.0 }
    }

    pub fn reset(&mut self) {
        self.phase = 0.0;
        self.last = 0.0;
    }

    /// Advance `m` samples without output.
    pub fn skip(&mut self, inc: f32, m: usize) {
        self.phase = wrap01(self.phase + inc * m as f32);
        self.last = self.phase;
    }

    /// Write `out.len()` samples at `inc` cycles a sample, times a gain ramping by `step` a sample.
    /// `pm`: the phase input and its depth in radians per unit.
    #[allow(clippy::too_many_arguments)]
    pub fn render(&mut self, out: &mut [f32], inc: f32, wave: f32, pw: f32, pm: Option<(&[f32], f32)>, gain: &mut f32, step: f32) {
        let s = wave.clamp(0.0, 1.0) * 3.0;
        let seg = (s.floor() as u8).min(2);
        let f = s - seg as f32;
        let dt0 = inc.clamp(1e-7, 0.5);
        for (i, y) in out.iter_mut().enumerate() {
            let (t, dt) = match pm {
                Some((x, d)) => {
                    let t = wrap01(self.phase + x[i] * d * (1.0 / TAU));
                    let moved = (wrap01(t - self.last + 0.5) - 0.5).abs();
                    (t, moved.clamp(1e-7, 0.5))
                }
                None => (self.phase, dt0),
            };
            let a = osc_wave(seg, t, dt, pw);
            let v = if f > 0.0 { a + (osc_wave(seg + 1, t, dt, pw) - a) * f } else { a };
            *gain += step;
            *y = v * *gain;
            self.last = t;
            self.phase += inc;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            }
        }
    }
}

// ------------------------------------------------------------------ noise

/// Pink and brown noise gains that bring each to white noise's level (rms 0.577).
const PINK_GAIN: f32 = 0.335;
const BROWN_GAIN: f32 = 10.0;

/// Noise whose colour is a morph: white at 0, pink at 0.5 (Paul Kellet's filter), brown at 1 (a
/// leaky integrator). All three run all the time, so moving the colour does not click.
pub struct NoiseSrc {
    rng: Noise,
    b: [f32; 3],
    brown: f32,
}

impl NoiseSrc {
    pub fn new(seed: u32) -> Self {
        NoiseSrc { rng: Noise::new(seed), b: [0.0; 3], brown: 0.0 }
    }

    pub fn render(&mut self, out: &mut [f32], colour: f32, gain: &mut f32, step: f32) {
        let c = colour.clamp(0.0, 1.0);
        for y in out.iter_mut() {
            let w = self.rng.next();
            self.b[0] = 0.99765 * self.b[0] + w * 0.099_046;
            self.b[1] = 0.963 * self.b[1] + w * 0.296_516_4;
            self.b[2] = 0.57 * self.b[2] + w * 1.052_691_3;
            let pink = (self.b[0] + self.b[1] + self.b[2] + w * 0.1848) * PINK_GAIN;
            self.brown = (self.brown + 0.02 * w) * (1.0 / 1.02);
            let brown = self.brown * BROWN_GAIN;
            let v = if c <= 0.5 {
                let f = 2.0 * c;
                w + (pink - w) * f
            } else {
                let f = 2.0 * c - 1.0;
                pink + (brown - pink) * f
            };
            *gain += step;
            *y = v * *gain;
        }
    }
}

// ------------------------------------------------------------------ sampler

/// Plays a sample from the pool with a 4-point cubic read: from a start point to the end, or
/// round a loop. The speed is set per span by the voice.
pub struct SamplerSrc {
    pos: f64,
    start: f32,
    fresh: bool,
    on: bool,
}

impl Default for SamplerSrc {
    fn default() -> Self {
        Self::new()
    }
}

impl SamplerSrc {
    pub fn new() -> Self {
        SamplerSrc { pos: 0.0, start: 0.0, fresh: false, on: false }
    }

    /// Note start: play from `start` (a part of the sample's length) at the next render.
    pub fn start(&mut self, start: f32) {
        self.start = start;
        self.fresh = true;
        self.on = true;
    }

    pub fn stop(&mut self) {
        self.on = false;
    }

    pub fn playing(&self) -> bool {
        self.on
    }

    /// `inc`: sample frames per output sample. `lp`: loop start and length as parts of the length.
    pub fn render(&mut self, out: &mut [f32], data: &[f32], inc: f32, lp: Option<(f32, f32)>, gain: &mut f32, step: f32) {
        let n = data.len();
        let last = (n - 1) as f64;
        if self.fresh {
            self.pos = self.start as f64 * last;
            self.fresh = false;
        }
        let lp = lp.and_then(|(s, l)| {
            let a = (s as f64 * last).floor();
            let b = (a + (l as f64 * n as f64).max(2.0)).min(last);
            (b - a >= 2.0).then_some((a, b))
        });
        let inc = if inc.is_finite() { inc.clamp(0.0, 64.0) as f64 } else { 0.0 };
        for (i, y) in out.iter_mut().enumerate() {
            match lp {
                Some((a, b)) => {
                    if self.pos >= b {
                        self.pos = a + (self.pos - a) % (b - a);
                    }
                }
                None if self.pos >= last => {
                    self.on = false;
                    out[i..].fill(0.0);
                    return;
                }
                None => {}
            }
            *gain += step;
            *y = cubic_read(data, self.pos) * *gain;
            self.pos += inc;
        }
    }
}

/// 4-point cubic (Catmull-Rom) read at `pos`, holding the end samples past either end.
#[inline]
fn cubic_read(d: &[f32], pos: f64) -> f32 {
    let n = d.len();
    let i = pos as usize;
    let f = (pos - i as f64) as f32;
    let at = |k: isize| d[k.clamp(0, n as isize - 1) as usize];
    let i = i as isize;
    let (y0, y1, y2, y3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
    let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
    let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c = -0.5 * y0 + 0.5 * y2;
    ((a * f + b) * f + c) * f + y1
}

#[cfg(test)]
mod source_tests {
    use super::*;

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn noise_colours_have_about_the_same_level() {
        for c in [0.0, 0.5, 1.0] {
            let mut n = NoiseSrc::new(5);
            let mut out = vec![0.0; 1 << 18];
            let mut g = 1.0;
            n.render(&mut out, c, &mut g, 0.0);
            let r = rms(&out[4096..]);
            assert!((r / 0.577 - 1.0).abs() < 0.2, "colour {c}: rms {r}");
        }
    }

    #[test]
    fn every_wave_stays_in_range_and_the_morph_is_continuous() {
        for w in [0.0, 0.2, 1.0 / 3.0, 0.5, 2.0 / 3.0, 0.9, 1.0] {
            let mut o = Osc::new();
            let mut out = vec![0.0; 4800];
            let mut g = 1.0;
            o.render(&mut out, 220.0 / 48000.0, w, 0.3, None, &mut g, 0.0);
            let peak = out.iter().fold(0.0f32, |a, x| a.max(x.abs()));
            assert!(peak > 0.5 && peak < 1.8, "wave {w}: peak {peak}");
        }
        // The waves at their morph points start where the next segment starts.
        let a = osc_wave(0, 0.3, 0.01, 0.5);
        assert!((a - (TAU * 0.3).sin()).abs() < 1e-3);
    }

    #[test]
    fn sampler_plays_to_the_end_or_loops() {
        let data: Vec<f32> = (0..1000).map(|i| i as f32).collect();
        let mut s = SamplerSrc::new();
        s.start(0.5);
        let mut out = vec![0.0; 600];
        let mut g = 1.0;
        s.render(&mut out, &data, 1.0, None, &mut g, 0.0);
        assert!((out[0] - 499.5).abs() < 1.0, "{}", out[0]);
        assert!(!s.playing() && out[599] == 0.0);
        s.start(0.0);
        s.render(&mut out, &data, 2.0, Some((0.1, 0.2)), &mut g, 0.0);
        assert!(s.playing());
        assert!(out[300..].iter().all(|&x| (99.0..=301.0).contains(&x)), "the loop holds");
    }
}

// ------------------------------------------------------------------ envelope

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Stage {
    Idle,
    Attack,
    Hold,
    Decay,
    Release,
    Choke,
}

/// Envelope modes (`env.mode`).
pub const ENV_ADSR: u8 = 0;
/// Starts at full, falls with decay; while the voice's first envelope is in
/// its release the release time is added (today's shaped-noise envelope).
pub const ENV_DECAY: u8 = 1;
/// Starts at full, falls with decay from the note start, ignores note-off
/// (today's pitch envelope).
pub const ENV_ONESHOT: u8 = 2;

/// Envelope times, latched at note start.
#[derive(Clone, Copy, Debug)]
pub struct EnvTimes {
    pub mode: u8,
    pub attack: f32,
    pub hold: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
}

impl EnvTimes {
    pub fn clean(self) -> Self {
        EnvTimes {
            mode: self.mode.min(ENV_ONESHOT),
            attack: cl(self.attack, 0.0, 30.0),
            hold: cl(self.hold, 0.0, 60.0),
            decay: cl(self.decay, 0.001, 120.0),
            sustain: cl(self.sustain, 0.0, 1.0),
            release: cl(self.release, 0.001, 120.0),
        }
    }
}

/// What a tick of the first envelope asks of the voice.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum EnvEvent {
    None,
    /// The hold ended on a hit with endStop: choke now.
    EndStop,
}

pub struct Env {
    pub t: EnvTimes,
    pub stage: Stage,
    pub level: f32,
    hold_left: f32,
}

impl Env {
    pub fn new() -> Self {
        Env {
            t: EnvTimes { mode: ENV_ADSR, attack: 0.002, hold: 0.0, decay: 1.0, sustain: 1.0, release: 0.1 },
            stage: Stage::Idle,
            level: 0.0,
            hold_left: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.stage = Stage::Idle;
        self.level = 0.0;
    }

    /// Note start. ADSR restarts its attack from the current level.
    pub fn start(&mut self, t: EnvTimes) {
        self.t = t.clean();
        self.stage = Stage::Attack;
        self.hold_left = self.t.hold;
        if self.t.mode != ENV_ADSR {
            self.level = 1.0;
        }
    }

    /// Note-off. Only ADSR has a release stage of its own.
    pub fn release(&mut self) -> bool {
        if self.t.mode != ENV_ADSR {
            return false;
        }
        match self.stage {
            Stage::Attack | Stage::Hold | Stage::Decay => {
                self.stage = Stage::Release;
                true
            }
            _ => false,
        }
    }

    /// Advance by `dt` seconds. `end_stop`: an ADSR going from attack or hold
    /// to decay chokes instead. `first_releasing`: the voice's first envelope
    /// is in its release (ENV_DECAY adds the release then). `note_t`: seconds
    /// since note start (ENV_ONESHOT).
    pub fn tick(&mut self, dt: f32, end_stop: bool, first_releasing: bool, note_t: f32) -> EnvEvent {
        let t = self.t;
        match t.mode {
            ENV_DECAY => {
                self.level *= fast_exp2(-dt / t.decay * LOG2_E);
                if first_releasing {
                    self.level *= fast_exp2(-dt / t.release * LOG2_E);
                }
                if self.level < 1e-5 {
                    self.level = 0.0;
                }
                EnvEvent::None
            }
            ENV_ONESHOT => {
                self.level = fast_exp2(-note_t / t.decay * LOG2_E);
                EnvEvent::None
            }
            _ => self.tick_adsr(dt, end_stop),
        }
    }

    fn tick_adsr(&mut self, dt: f32, end_stop: bool) -> EnvEvent {
        let t = self.t;
        match self.stage {
            Stage::Attack => {
                self.level += dt / t.attack.max(1e-3);
                if self.level >= 1.0 {
                    self.level = 1.0;
                    if t.hold > 0.0 {
                        self.hold_left = t.hold;
                        self.stage = Stage::Hold;
                    } else if end_stop {
                        self.stage = Stage::Choke;
                        return EnvEvent::EndStop;
                    } else {
                        self.stage = Stage::Decay;
                    }
                }
            }
            Stage::Hold => {
                self.hold_left -= dt;
                if self.hold_left <= 0.0 {
                    if end_stop {
                        self.stage = Stage::Choke;
                        return EnvEvent::EndStop;
                    }
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                let s = t.sustain;
                self.level = s + (self.level - s) * fast_exp2(-dt / t.decay * LOG2_E);
                if s < 1e-4 && self.level < 1e-4 {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
            }
            Stage::Release => {
                self.level *= fast_exp2(-dt / t.release * LOG2_E);
                if self.level < 1e-4 {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
            }
            Stage::Idle | Stage::Choke => {}
        }
        EnvEvent::None
    }
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}

// ------------------------------------------------------------------ LFO

/// An LFO, bipolar, times its level. Wave (stepped, 0..1 in steps of 0.2):
/// sine, triangle, saw, square, random steps, drift (smoothed random, the
/// old chaos). Sine and drift are the old LFOs' code, unchanged.
pub struct LfoSrc {
    sine: Lfo,
    drift: Chaos,
    /// Phase of the triangle, saw, square and random-step waves.
    phase: f32,
    step: f32,
    rng: Noise,
    seed: u32,
    pub value: f32,
}

pub const LFO_WAVES: f32 = 5.0;

impl LfoSrc {
    pub fn new(seed: u32) -> Self {
        let mut l = LfoSrc { sine: Lfo::new(), drift: Chaos::new(seed), phase: 0.0, step: 0.0, rng: Noise::new(seed ^ 0x2545_f491), seed, value: 0.0 };
        l.start();
        l
    }

    /// Note start: phase 0, the random sequences from the start, output 0.
    pub fn start(&mut self) {
        self.sine.reset();
        self.drift.reset(self.seed);
        self.phase = 0.0;
        self.rng = Noise::new(self.seed ^ 0x2545_f491);
        self.step = self.rng.next().clamp(-1.0, 1.0);
        self.value = 0.0;
    }

    /// Advance and return the raw (unit-level) value; the caller applies the level.
    pub fn tick(&mut self, rate: f32, wave: f32, dt: f32) -> f32 {
        let w = (wave * LFO_WAVES).round() as i32;
        self.value = match w {
            i32::MIN..=0 => self.sine.tick(rate, dt),
            5.. => self.drift.tick(rate, dt),
            _ => {
                let inc = rate * dt;
                if inc.is_finite() && inc > 0.0 {
                    self.phase += inc;
                    if self.phase >= 1.0 {
                        self.phase -= self.phase.floor();
                        self.step = self.rng.next().clamp(-1.0, 1.0);
                    }
                }
                let p = self.phase;
                match w {
                    // Triangle and saw start at 0 and rise, like the sine.
                    1 => 1.0 - 4.0 * ((p + 0.25).fract() - 0.5).abs(),
                    2 => 2.0 * (p + 0.5).fract() - 1.0,
                    3 => {
                        if p < 0.5 {
                            1.0
                        } else {
                            -1.0
                        }
                    }
                    _ => self.step,
                }
            }
        };
        self.value
    }
}

#[cfg(test)]
mod lfo_tests {
    use super::*;

    #[test]
    fn waves_start_at_zero_or_a_step_and_stay_bipolar() {
        for w in 0..=5 {
            let mut l = LfoSrc::new(7);
            let mut seen = (f32::MAX, f32::MIN);
            for _ in 0..4000 {
                let x = l.tick(3.0, w as f32 / LFO_WAVES, 1.0 / 1500.0);
                assert!((-1.0..=1.0).contains(&x), "wave {w}: {x}");
                seen = (seen.0.min(x), seen.1.max(x));
            }
            assert!(seen.1 - seen.0 > 0.5, "wave {w} moves: {seen:?}");
        }
        // Triangle and saw rise from 0 at the start.
        for w in [1, 2] {
            let mut l = LfoSrc::new(1);
            let x = l.tick(1.0, w as f32 / LFO_WAVES, 0.01);
            assert!(x > 0.0 && x < 0.1, "wave {w}: {x}");
        }
    }

    #[test]
    fn random_steps_hold_within_a_cycle_and_repeat_after_start() {
        let run = |l: &mut LfoSrc| (0..300).map(|_| l.tick(10.0, 0.8, 1.0 / 1500.0)).collect::<Vec<_>>();
        let mut l = LfoSrc::new(3);
        let a = run(&mut l);
        l.start();
        let b = run(&mut l);
        assert_eq!(a, b);
        // 10 Hz at 1500 ticks a second: 150 ticks a step, so at most 3 distinct values in 300 ticks.
        let mut d = a.clone();
        d.dedup();
        assert!(d.len() <= 3 && d.len() >= 2, "{d:?}");
    }
}
