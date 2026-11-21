//! The grain cloud of one layer (TDD "Klont DSP", Grain cloud row).
//!
//! A layer's patch names a sample of the pool (`LOAD_SAMPLE`, samples.rs) for
//! its grains. While that sample is loaded and a voice's grain level is above
//! 0, every sounding voice spawns short
//! Hann-windowed grains at `grain.density` per second. A grain reads the
//! source with 4-point cubic interpolation at
//! `source rate / engine rate * pitch ratio`, where the ratio is the voice's
//! pitch over the source's reference pitch (an unpitched source uses C4, so
//! the middle of the keyboard plays it at its own speed). Each grain is tagged
//! with the voice that spawned it and takes that voice's grain level (the
//! grains' level value, driven by an envelope, with the voice's velocity and
//! gesture routes: `Voice::grain_level`), so a released note fades its
//! grains with its own release.
//!
//! Where a grain starts: the position value (0..1 of the source, after the
//! voice's routes; the classic patch routes its second LFO there) plus a
//! random spread of the spray value (full spray spans the whole source). The
//! spawn interval is `1 / density`, jittered by scatter. Randomness comes
//! from a generator seeded per layer, so offline renders repeat.
//!
//! The source data lives in the sample pool; the cloud holds its region. A
//! new source crossfades from the old one over 50 ms (equal power, per
//! sample), so a change makes no click; the old region stays in the pool
//! until the next change. A second change inside the 50 ms ends the first
//! fade at once (the old grains are cut), which can click; the host does not
//! send two sources that close together.
//!
//! Cost: a layer without a source has nothing to do (`wants` is false and the
//! layer never calls in). With a source and a grain mix of 0, a tick is one
//! pass over the eight voice slots and no grain is rendered.

use crate::protocol::*;
use crate::samples::{SamplePool, SampleRef};
use crate::voice::TICK;
use dsp::noise::Noise;

/// Grains alive at once per layer.
pub const MAX_GRAINS: usize = 32;
const XFADE_S: f32 = 0.05;
const HANN_N: usize = 512;
/// Pitch reference of an unpitched source: C4, in cents above C0.
const UNPITCHED_REF: f32 = 4800.0;
/// Shortest spawn interval in samples (guards a density times scatter product).
const MIN_INTERVAL: f32 = 8.0;

/// What one voice tells the cloud each tick (real units).
#[derive(Clone, Copy, Debug, Default)]
pub struct VoiceGrain {
    /// The voice sounds.
    pub on: bool,
    /// The voice's grain level.
    pub amp: f32,
    pub size: f32,
    pub density: f32,
    pub pos: f32,
    pub spray: f32,
    pub scatter: f32,
    /// Current pitch of the voice in cents above C0.
    pub cents: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct Grain {
    on: bool,
    buf: u8,
    voice: u8,
    /// Samples still to wait before the grain starts (spawned mid-tick).
    delay: u32,
    /// Read position in source samples, and source samples per output sample.
    pos: f64,
    step: f64,
    age: u32,
    len: u32,
    inv_len: f32,
    amp: f32,
    amp_step: f32,
    /// Level scale that keeps dense clouds from piling up.
    norm: f32,
}

pub struct GrainCloud {
    sr: f32,
    /// The source of each grain buffer index: the active one and the one being left.
    src: [Option<SampleRef>; 2],
    active: usize,
    /// Crossfade progress of the active buffer, 1 = done.
    fade: f32,
    fade_step: f32,
    grains: [Grain; MAX_GRAINS],
    live: usize,
    /// Samples until each voice's next spawn, counted from the tick start.
    timer: [f32; crate::protocol::VOICES_PER_LAYER],
    rng: Noise,
    hann: [f32; HANN_N + 1],
}

impl GrainCloud {
    pub fn new(sr: f32, seed: u32) -> Self {
        let mut hann = [0.0; HANN_N + 1];
        for (i, h) in hann.iter_mut().enumerate() {
            *h = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / HANN_N as f32).cos();
        }
        GrainCloud {
            sr,
            src: [None; 2],
            active: 0,
            fade: 1.0,
            fade_step: 1.0 / (XFADE_S * sr),
            grains: [Grain::default(); MAX_GRAINS],
            live: 0,
            timer: [0.0; VOICES_PER_LAYER],
            rng: Noise::new(seed),
            hann,
        }
    }

    /// The sample the cloud plays now.
    pub fn source(&self) -> Option<SampleRef> {
        self.src[self.active]
    }

    /// Replace the source (None clears it), holding the new region in the pool
    /// and letting go of the one before the last.
    pub fn set_source(&mut self, s: Option<SampleRef>, pool: &mut SamplePool) {
        if s == self.src[self.active] {
            return;
        }
        if self.fade < 1.0 {
            self.end_fade();
        }
        let new = 1 - self.active;
        if let Some(old) = self.src[new].take() {
            pool.release(old.region);
        }
        if let Some(x) = s {
            pool.acquire(x.region);
        }
        self.src[new] = s;
        self.active = new;
        // Nothing sounding: no fade needed, the old buffer has no readers.
        self.fade = if self.live == 0 { 1.0 } else { 0.0 };
    }

    /// Finish a crossfade now: the old buffer's grains are cut.
    fn end_fade(&mut self) {
        let a = self.active as u8;
        for g in self.grains.iter_mut() {
            if g.on && g.buf != a {
                g.on = false;
                self.live -= 1;
            }
        }
        self.fade = 1.0;
    }

    pub fn has_source(&self) -> bool {
        self.src[self.active].is_some_and(|s| s.n > 0)
    }

    /// The layer needs to call `tick` (a source to play, or grains still sounding).
    pub fn wants(&self) -> bool {
        self.has_source() || self.live > 0
    }

    /// Grains are sounding, so the layer must render.
    pub fn active(&self) -> bool {
        self.live > 0
    }

    pub fn voice_start(&mut self, v: usize) {
        self.timer[v] = 0.0;
    }

    pub fn clear_grains(&mut self) {
        for g in self.grains.iter_mut() {
            g.on = false;
        }
        self.live = 0;
        self.fade = 1.0;
    }

    /// Once per control tick: retarget every grain's amplitude to its voice,
    /// then spawn new grains.
    pub fn tick(&mut self, vg: &[VoiceGrain; VOICES_PER_LAYER]) {
        let inv = 1.0 / TICK as f32;
        for g in self.grains.iter_mut() {
            if !g.on {
                continue;
            }
            let v = &vg[g.voice as usize];
            let target = if v.on { v.amp * g.norm } else { 0.0 };
            if target <= 1e-9 && g.amp <= 1e-9 && g.delay == 0 {
                g.on = false;
                self.live -= 1;
                continue;
            }
            g.amp_step = (target - g.amp) * inv;
        }
        let src = self.src[self.active].unwrap_or(SampleRef { region: 0, n: 0, rate: 0.0, ref_cents: f32::NAN });
        for (vi, v) in vg.iter().enumerate() {
            if !v.on || v.amp <= 1e-9 || src.n == 0 {
                self.timer[vi] = 0.0;
                continue;
            }
            let density = v.density.clamp(1.0, 100.0);
            let mut t = self.timer[vi];
            while t < TICK as f32 {
                self.spawn(vi, t.max(0.0) as u32, v, &src, density);
                let jitter = 1.0 + v.scatter.clamp(0.0, 1.0) * 0.9 * self.rng.next();
                t += (self.sr / density * jitter).max(MIN_INTERVAL);
            }
            self.timer[vi] = t - TICK as f32;
        }
    }

    fn spawn(&mut self, voice: usize, delay: u32, v: &VoiceGrain, src: &SampleRef, density: f32) {
        let Some(slot) = self.grains.iter().position(|g| !g.on) else { return };
        let n = src.n as f64;
        let size = v.size.clamp(0.01, 0.5);
        let len = ((size * self.sr) as u32).max(32);
        let refc = if src.ref_cents.is_finite() { src.ref_cents } else { UNPITCHED_REF };
        let cents = if v.cents.is_finite() { v.cents } else { UNPITCHED_REF };
        let ratio = ((cents - refc) / 1200.0).clamp(-6.0, 6.0).exp2();
        let step = (src.rate / self.sr * ratio) as f64;
        let centre = v.pos.clamp(0.0, 1.0) + v.spray.clamp(0.0, 1.0) * 0.5 * self.rng.next();
        let start = (centre as f64).rem_euclid(1.0) * n - 0.5 * len as f64 * step;
        let overlap = (density * size).max(1.0);
        self.grains[slot] = Grain {
            on: true,
            buf: self.active as u8,
            voice: voice as u8,
            delay,
            pos: start.rem_euclid(n),
            step,
            age: 0,
            len,
            inv_len: 1.0 / len as f32,
            amp: v.amp / overlap.sqrt(),
            amp_step: 0.0,
            norm: 1.0 / overlap.sqrt(),
        };
        self.live += 1;
    }

    /// Add `out.len()` (<= TICK) samples of the cloud into `out`.
    pub fn render(&mut self, out: &mut [f32], pool: &SamplePool) {
        if self.live == 0 {
            return;
        }
        let n = out.len();
        let mut g_new = [1.0f32; TICK];
        let mut g_old = [0.0f32; TICK];
        let fading = self.fade < 1.0;
        if fading {
            for i in 0..n {
                self.fade = (self.fade + self.fade_step).min(1.0);
                let a = self.fade * std::f32::consts::FRAC_PI_2;
                g_new[i] = a.sin();
                g_old[i] = a.cos();
            }
        }
        let active = self.active as u8;
        for g in self.grains.iter_mut() {
            if !g.on {
                continue;
            }
            let Some(src) = self.src[g.buf as usize] else {
                g.on = false;
                self.live -= 1;
                continue;
            };
            let ns = src.n;
            let data = &pool.data(src.region)[..ns];
            let fade_g = if g.buf == active { &g_new } else { &g_old };
            let mut ended = false;
            for i in 0..n {
                if g.delay > 0 {
                    g.delay -= 1;
                    continue;
                }
                if g.age >= g.len {
                    ended = true;
                    break;
                }
                let x = g.age as f32 * g.inv_len * HANN_N as f32;
                let k = (x as usize).min(HANN_N - 1);
                let w = self.hann[k] + (self.hann[k + 1] - self.hann[k]) * (x - k as f32);
                let s = cubic(data, g.pos);
                g.amp += g.amp_step;
                out[i] += s * w * g.amp * fade_g[i];
                g.age += 1;
                g.pos += g.step;
                while g.pos >= ns as f64 {
                    g.pos -= ns as f64;
                }
            }
            if ended || (g.age >= g.len && g.delay == 0) {
                g.on = false;
                self.live -= 1;
            }
        }
        if fading && self.fade >= 1.0 {
            self.end_fade();
        }
    }
}

/// 4-point cubic (Catmull-Rom) read of `d` at fractional index `pos` (0..d.len()),
/// wrapping at the ends.
#[inline]
fn cubic(d: &[f32], pos: f64) -> f32 {
    let n = d.len();
    if n == 0 {
        return 0.0;
    }
    let i = pos as usize;
    let f = (pos - i as f64) as f32;
    let wrap = |k: usize| if k >= n { k % n } else { k };
    let y0 = d[if i == 0 { n - 1 } else { i - 1 }];
    let y1 = d[i];
    let y2 = d[wrap(i + 1)];
    let y3 = d[wrap(i + 2)];
    let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
    let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c = -0.5 * y0 + 0.5 * y2;
    ((a * f + b) * f + c) * f + y1
}
