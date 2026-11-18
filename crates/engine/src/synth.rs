//! One-layer Klont for M0: partial bank, shaped noise, vowel-shape path,
//! envelopes and the voice-gesture mapping (resolution step 5). One voice.
//!
//! Control values are recomputed every `TICK` samples; audio is rendered in
//! chunks that never cross a tick boundary, and amplitudes are ramped
//! linearly across each tick. A sample counter carries the tick position
//! across `render` calls, so any block length gives the same output.

use dsp::bands::{band_centres, band_q, gain_at};
use dsp::capture::Gesture;
use dsp::filters::{DcBlocker, Svf};
use dsp::frame::MAX_BANDS;
use dsp::noise::Noise;
use dsp::params as P;
use dsp::{cents_to_hz, db_to_lin};
use std::f32::consts::TAU;

/// What Live follow feeds the synth each block.
pub struct LiveDrive<'a> {
    /// Tuned carrier pitch in cents above C0 (octave transpose applied), NaN
    /// until a pitch is known.
    pub tuned_cents: f32,
    pub gesture: &'a Gesture,
}

/// Values published to the meter block.
#[derive(Clone, Debug, Default)]
pub struct SynthMeters {
    pub f0_hz: f32,
    pub voiced_mix: f32,
    pub active_partials: u32,
    pub tone_eff_db: [f32; MAX_BANDS],
}

const TICK: usize = 32;
const MAX_PARTIALS: usize = 64;
/// Partial power after normalisation: sum of a_k^2 / 2 (rms about 0.35).
const PARTIAL_POWER: f32 = 0.12;
/// Target rms of the shaped noise with flat bands, before level.
const NOISE_RMS: f32 = 0.35;
/// Width of a formant bell in octaves (sigma of a Gaussian in log frequency).
const FORMANT_SIGMA_OCT: f32 = 0.15;
const DB_FLOOR: f32 = -72.0;
const VOICE_XFADE_S: f32 = 0.010;
/// Extra pitch smoothing in live mode (the tuner already smooths).
const PITCH_TAU_S: f32 = 0.002;
const GLIDE_S: f32 = 0.010;
const GLIDE_TAU_S: f32 = 0.0025;
const BAND_SMOOTH_S: f32 = 0.015;
const CLICK_TAU_S: f32 = 0.0015;

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
                // Mode density grows with frequency, so spacing shrinks:
                // count of modes below ratio R is about 0.7 R^2.
                (k as f32 / 0.697).sqrt()
            }
        }
        _ => {
            if k <= BAR_RATIOS.len() {
                BAR_RATIOS[k - 1]
            } else {
                // Free-bar series, matched to the last tabulated value.
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
fn soft_clip(x: f32) -> f32 {
    // Pade approximation of tanh, exactly 1 at |x| = 3.
    let x = x.clamp(-3.0, 3.0);
    (x * (27.0 + x * x) / (27.0 + 9.0 * x * x)).clamp(-1.0, 1.0)
}

#[inline]
fn sane(x: f32, lo: f32, hi: f32) -> f32 {
    if x.is_finite() { x.clamp(lo, hi) } else { lo.max(0.0).min(hi) }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Stage {
    Idle,
    Attack,
    Hold,
    Decay,
    Release,
}

pub struct Synth {
    sr: f32,
    n_bands: usize,
    centres: [f32; MAX_BANDS],

    // Parameters: stored target, smoothed value, smoothing coefficient per tick.
    target: [f32; P::COUNT],
    cur: [f32; P::COUNT],
    coef: [f32; P::COUNT],
    tone_t: [f32; MAX_BANDS],
    tone_c: [f32; MAX_BANDS],
    noise_t: [f32; MAX_BANDS],
    noise_c: [f32; MAX_BANDS],
    band_a: f32,
    // Effective (step 5) values for this tick.
    tone_eff: [f32; MAX_BANDS],
    noise_eff: [f32; MAX_BANDS],
    tilt_eff: f32,
    f1_eff: f32,

    tick_pos: usize,
    force_tick: bool,
    live: bool,

    // Partial bank (rotators).
    re: [f32; MAX_PARTIALS],
    im: [f32; MAX_PARTIALS],
    cr: [f32; MAX_PARTIALS],
    ci: [f32; MAX_PARTIALS],
    amp: [f32; MAX_PARTIALS],
    step: [f32; MAX_PARTIALS],
    tgt: [f32; MAX_PARTIALS],
    ring: [f32; MAX_PARTIALS],
    // Cached ratio data for the current modal/stretch.
    ratio_lr: [f32; MAX_PARTIALS],
    freq_mul: [f32; MAX_PARTIALS],
    freq_lg: [f32; MAX_PARTIALS],
    ratio_key: (u32, u32),
    rot_key: (u32, u32, u32, usize),
    n_proc: usize,

    // Shaped noise.
    noise: Noise,
    bp: [Svf; MAX_BANDS],
    ng: [f32; MAX_BANDS],
    ng_step: [f32; MAX_BANDS],
    ng_tgt: [f32; MAX_BANDS],
    noise_norm: f32,
    noise_on: bool,
    click: f32,
    click_dec: f32,
    dc: DcBlocker,

    // Note voice.
    stage: Stage,
    env: f32,
    hold_left: f32,
    noise_env: f32,
    note_t: f32,
    note_cents: f32,
    vel: f32,

    // Pitch and live state.
    pitch_cur: f32,
    last_cents: f32,
    have_pitch: bool,
    pitch_fresh: bool,
    glide_left: f32,
    gate_prev: bool,
    env_prev: f32,
    vmix: f32,
    vmix_target: f32,
    level_lin: f32,

    meters: SynthMeters,
}

impl Synth {
    pub fn new(sample_rate: f32, n_bands: usize, _max_block: usize) -> Self {
        let sr = sample_rate.max(8000.0);
        let n_bands = n_bands.clamp(2, MAX_BANDS);
        let dt = TICK as f32 / sr;
        let mut target = [0.0; P::COUNT];
        let mut coef = [1.0; P::COUNT];
        for (i, d) in P::PARAMS.iter().enumerate() {
            target[i] = d.default;
            if d.smooth_ms > 0.0 {
                coef[i] = 1.0 - (-dt * 1000.0 / d.smooth_ms).exp();
            }
        }
        let mut centres = [0.0; MAX_BANDS];
        band_centres(n_bands, &mut centres);
        let q = band_q(n_bands);
        let mut bp = [Svf::default(); MAX_BANDS];
        let mut bw = 0.0;
        for i in 0..n_bands {
            bp[i] = Svf::new(sr, centres[i], q);
            bw += centres[i] / q;
        }
        // Power of the summed band-passes relative to white noise.
        let frac = (bw / (sr * 0.5)).clamp(0.05, 1.0);
        let noise_norm = NOISE_RMS / (0.577 * frac.sqrt());
        let mut s = Synth {
            sr,
            n_bands,
            centres,
            target,
            cur: target,
            coef,
            tone_t: [0.0; MAX_BANDS],
            tone_c: [0.0; MAX_BANDS],
            noise_t: [0.0; MAX_BANDS],
            noise_c: [0.0; MAX_BANDS],
            band_a: 1.0 - (-dt / BAND_SMOOTH_S).exp(),
            tone_eff: [0.0; MAX_BANDS],
            noise_eff: [0.0; MAX_BANDS],
            tilt_eff: 0.0,
            f1_eff: 600.0,
            tick_pos: 0,
            force_tick: false,
            live: false,
            re: [1.0; MAX_PARTIALS],
            im: [0.0; MAX_PARTIALS],
            cr: [1.0; MAX_PARTIALS],
            ci: [0.0; MAX_PARTIALS],
            amp: [0.0; MAX_PARTIALS],
            step: [0.0; MAX_PARTIALS],
            tgt: [0.0; MAX_PARTIALS],
            ring: [1.0; MAX_PARTIALS],
            ratio_lr: [0.0; MAX_PARTIALS],
            freq_mul: [1.0; MAX_PARTIALS],
            freq_lg: [0.0; MAX_PARTIALS],
            ratio_key: (u32::MAX, u32::MAX),
            rot_key: (0, 0, 0, 0),
            n_proc: 0,
            noise: Noise::new(0x1234_5678),
            bp,
            ng: [0.0; MAX_BANDS],
            ng_step: [0.0; MAX_BANDS],
            ng_tgt: [0.0; MAX_BANDS],
            noise_norm,
            noise_on: false,
            click: 0.0,
            click_dec: (-1.0 / (CLICK_TAU_S * sr)).exp(),
            dc: DcBlocker::new(sr),
            stage: Stage::Idle,
            env: 0.0,
            hold_left: 0.0,
            noise_env: 0.0,
            note_t: 0.0,
            note_cents: 5700.0,
            vel: 1.0,
            pitch_cur: 5700.0,
            last_cents: 5700.0,
            have_pitch: false,
            pitch_fresh: false,
            glide_left: 0.0,
            gate_prev: false,
            env_prev: 0.0,
            vmix: 0.0,
            vmix_target: 0.0,
            level_lin: db_to_lin(P::PARAMS[P::LEVEL].default),
            meters: SynthMeters::default(),
        };
        s.tilt_eff = s.cur[P::TILT];
        s.f1_eff = s.cur[P::F1_FREQ];
        s
    }

    /// Real units (see dsp::params). Tuning params are ignored here.
    pub fn set_param(&mut self, id: usize, value: f32) {
        if id >= P::COUNT || !value.is_finite() {
            return;
        }
        self.target[id] = P::clamp(id, value);
    }

    pub fn set_tone_band(&mut self, i: usize, db: f32) {
        if i < self.n_bands && db.is_finite() {
            self.tone_t[i] = db.clamp(-60.0, 60.0);
        }
    }

    pub fn set_noise_band(&mut self, i: usize, db: f32) {
        if i < self.n_bands && db.is_finite() {
            self.noise_t[i] = db.clamp(-60.0, 60.0);
        }
    }

    /// Note mode (Influence audition, offline render). Ignored while live.
    pub fn note_on(&mut self, cents: f32, velocity: f32) {
        if self.live || !cents.is_finite() {
            return;
        }
        if self.env < 1e-4 && self.noise_env < 1e-4 {
            // Silent: start the sines from phase zero so the attack is clean.
            self.re = [1.0; MAX_PARTIALS];
            self.im = [0.0; MAX_PARTIALS];
        }
        self.note_cents = cents;
        self.vel = sane(velocity, 0.0, 1.0);
        self.stage = Stage::Attack;
        self.hold_left = 0.0;
        self.noise_env = 1.0;
        self.note_t = 0.0;
        self.ring = [1.0; MAX_PARTIALS];
        self.pitch_cur = cents + 100.0 * self.cur[P::ENV_PITCH_AMT];
        self.force_tick = true;
    }

    pub fn note_off(&mut self) {
        if !self.live && self.stage != Stage::Idle {
            self.stage = Stage::Release;
        }
    }

    /// Silence everything immediately.
    pub fn panic(&mut self) {
        self.reset_state();
        self.stage = Stage::Idle;
        self.env = 0.0;
        self.noise_env = 0.0;
    }

    fn reset_state(&mut self) {
        self.re = [1.0; MAX_PARTIALS];
        self.im = [0.0; MAX_PARTIALS];
        self.amp = [0.0; MAX_PARTIALS];
        self.step = [0.0; MAX_PARTIALS];
        self.tgt = [0.0; MAX_PARTIALS];
        self.ring = [1.0; MAX_PARTIALS];
        self.n_proc = 0;
        self.rot_key = (0, 0, 0, 0);
        self.ng = [0.0; MAX_BANDS];
        self.ng_step = [0.0; MAX_BANDS];
        self.ng_tgt = [0.0; MAX_BANDS];
        self.noise_on = false;
        for f in self.bp.iter_mut() {
            f.reset();
        }
        self.click = 0.0;
        self.dc = DcBlocker::new(self.sr);
        self.stage = Stage::Idle;
        self.env = 0.0;
        self.noise_env = 0.0;
        self.vmix = 0.0;
        self.meters.active_partials = 0;
        if !self.pitch_cur.is_finite() {
            self.pitch_cur = 5700.0;
        }
    }

    /// Render `out.len()` samples, overwriting `out`. `drive` is Some while
    /// Live follow is on.
    pub fn render(&mut self, out: &mut [f32], drive: Option<&LiveDrive>) {
        let live = drive.is_some();
        if live != self.live {
            self.live = live;
            if live {
                self.stage = Stage::Idle;
                self.env = 0.0;
                self.noise_env = 0.0;
            }
        }
        if let Some(d) = drive {
            // An onset fires in one block only; start the click right away.
            let g = d.gesture;
            if g.onset > 0.0 {
                let amt = sane(g.hardness, 0.0, 1.0) * self.cur[P::DEPTH_HARD];
                self.click = self.click.max(amt * self.level_lin);
            }
        }
        if self.force_tick {
            self.force_tick = false;
            self.tick_pos = 0;
        }
        let mut done = 0;
        while done < out.len() {
            if self.tick_pos == 0 {
                self.control_tick(drive);
            }
            let n = (TICK - self.tick_pos).min(out.len() - done);
            self.render_chunk(&mut out[done..done + n]);
            self.tick_pos = (self.tick_pos + n) % TICK;
            done += n;
        }
    }

    pub fn meters(&self) -> &SynthMeters {
        &self.meters
    }

    // ---- control rate ----

    fn control_tick(&mut self, drive: Option<&LiveDrive>) {
        let dt = TICK as f32 / self.sr;
        let nb = self.n_bands;
        for i in 0..P::COUNT {
            self.cur[i] += self.coef[i] * (self.target[i] - self.cur[i]);
        }
        for i in 0..nb {
            self.tone_c[i] += self.band_a * (self.tone_t[i] - self.tone_c[i]);
            self.noise_c[i] += self.band_a * (self.noise_t[i] - self.noise_c[i]);
        }
        let lvl = db_to_lin(self.cur[P::LEVEL]);
        self.level_lin = lvl;
        let hmix = self.cur[P::HARM_MIX];
        let nmix = self.cur[P::NOISE_MIX];

        let (gp, gn);
        let mut pitch_a = 1.0;
        let pitch_target;
        self.tilt_eff = self.cur[P::TILT];
        self.f1_eff = self.cur[P::F1_FREQ];
        self.tone_eff[..nb].copy_from_slice(&self.tone_c[..nb]);
        self.noise_eff[..nb].copy_from_slice(&self.noise_c[..nb]);

        match drive {
            None => {
                self.env_tick(dt);
                self.note_t += dt;
                let drop = self.cur[P::ENV_PITCH_AMT] * (-self.note_t / self.cur[P::ENV_PITCH_TIME]).exp();
                pitch_target = self.note_cents + 100.0 * drop;
                gp = hmix * self.env * self.vel * lvl;
                gn = nmix * self.noise_env * self.vel * lvl;
                self.vmix = 1.0;
            }
            Some(d) => {
                let g = d.gesture;
                let gate_env = sane(g.gate_env, 0.0, 1.0);
                let loud = sane(g.loudness, 0.0, 1.0);
                let open = sane(g.openness, 0.0, 1.0) * self.cur[P::DEPTH_OPEN];
                let breath = sane(g.breathiness, 0.0, 1.0) * self.cur[P::DEPTH_BREATH];
                let dl = self.cur[P::DEPTH_LOUD];
                let dm = self.cur[P::DEPTH_MOUTH];

                // Pitch (requirement 7).
                let t = d.tuned_cents;
                let silent_prev = !self.gate_prev && self.env_prev < 0.02;
                if g.gate && !self.gate_prev {
                    self.pitch_fresh = t.is_finite();
                }
                if t.is_finite() {
                    if !self.have_pitch || silent_prev {
                        self.pitch_cur = t;
                    } else if !self.pitch_fresh && g.gate {
                        self.glide_left = GLIDE_S;
                    }
                    if g.gate {
                        self.pitch_fresh = true;
                    }
                    self.last_cents = t;
                    self.have_pitch = true;
                }
                pitch_target = self.last_cents;
                pitch_a = if self.glide_left > 0.0 {
                    self.glide_left -= dt;
                    1.0 - (-dt / GLIDE_TAU_S).exp()
                } else {
                    1.0 - (-dt / PITCH_TAU_S).exp()
                };
                self.gate_prev = g.gate;
                self.env_prev = gate_env;

                // Voicing crossfade over 10 ms.
                if g.voicing < 0.5 {
                    self.vmix_target = if self.have_pitch { 1.0 } else { 0.0 };
                } else if g.voicing < 1.5 {
                    self.vmix_target = 0.0;
                }
                let dv = dt / VOICE_XFADE_S;
                self.vmix += (self.vmix_target - self.vmix).clamp(-dv, dv);

                self.tilt_eff = (self.cur[P::TILT] + 12.0 * open).min(18.0);
                self.f1_eff = self.cur[P::F1_FREQ] * (1.0 + 0.5 * open);
                for i in 0..nb {
                    let gt = sane(g.bands_db[i], -48.0, 48.0);
                    self.tone_eff[i] = (1.0 - dm) * self.tone_c[i] + dm * gt;
                    self.noise_eff[i] = (1.0 - dm) * self.noise_c[i] + dm * gt;
                }

                let amp = gate_env * ((1.0 - dl) + dl * loud) * lvl;
                let nm = (nmix + breath).min(1.0);
                let unv = hmix.max(nm);
                gp = hmix * self.vmix * amp;
                gn = amp * (nm * self.vmix + unv * (1.0 - self.vmix));
            }
        }

        self.pitch_cur += pitch_a * (pitch_target - self.pitch_cur);
        if !self.pitch_cur.is_finite() {
            self.pitch_cur = 5700.0;
        }

        self.update_partials(gp, dt);
        self.update_noise(gn);

        self.meters.f0_hz = cents_to_hz(self.pitch_cur);
        self.meters.voiced_mix = if self.live { self.vmix } else { 1.0 };
        self.meters.tone_eff_db = [0.0; MAX_BANDS];
        self.meters.tone_eff_db[..nb].copy_from_slice(&self.tone_eff[..nb]);
    }

    fn env_tick(&mut self, dt: f32) {
        let p = &self.cur;
        match self.stage {
            Stage::Idle => {}
            Stage::Attack => {
                self.env += dt / p[P::ENV_ATTACK].max(1e-3);
                if self.env >= 1.0 {
                    self.env = 1.0;
                    if p[P::ENV_HOLD] > 0.0 {
                        self.hold_left = p[P::ENV_HOLD];
                        self.stage = Stage::Hold;
                    } else {
                        self.stage = Stage::Decay;
                    }
                }
            }
            Stage::Hold => {
                self.hold_left -= dt;
                if self.hold_left <= 0.0 {
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                let s = p[P::ENV_SUSTAIN];
                self.env = s + (self.env - s) * (-dt / p[P::ENV_DECAY]).exp();
                if s < 1e-4 && self.env < 1e-4 {
                    self.env = 0.0;
                    self.stage = Stage::Idle;
                }
            }
            Stage::Release => {
                self.env *= (-dt / p[P::ENV_RELEASE]).exp();
                if self.env < 1e-4 {
                    self.env = 0.0;
                    self.stage = Stage::Idle;
                }
            }
        }
        self.noise_env *= (-dt / p[P::ENV_NOISE_DECAY]).exp();
        if self.stage == Stage::Release {
            self.noise_env *= (-dt / p[P::ENV_RELEASE]).exp();
        }
        if self.noise_env < 1e-5 {
            self.noise_env = 0.0;
        }
    }

    fn update_partials(&mut self, gp: f32, dt: f32) {
        let np = (self.cur[P::PARTIALS].round() as usize).clamp(1, MAX_PARTIALS);
        let modal = self.cur[P::MODAL];
        let stretch = self.cur[P::STRETCH];
        let nyq = self.sr * 0.45;
        let f0 = cents_to_hz(self.pitch_cur).clamp(8.0, nyq);

        // Previous targets become the ramp starts, which also removes any
        // rounding drift from stepping.
        self.amp = self.tgt;

        let rk = (modal.to_bits(), stretch.to_bits());
        if rk != self.ratio_key {
            self.ratio_key = rk;
            for k in 0..MAX_PARTIALS {
                let r = modal_ratio(modal, k + 1);
                let kk = (k + 1) as f32;
                let fm = r * (1.0 + stretch * kk * kk).sqrt();
                self.ratio_lr[k] = r.log2();
                self.freq_mul[k] = fm;
                self.freq_lg[k] = fm.log2();
            }
        }
        let rot_key = (f0.to_bits(), modal.to_bits(), stretch.to_bits(), np);
        if rot_key != self.rot_key {
            self.rot_key = rot_key;
            let w0 = TAU * f0 / self.sr;
            for k in 0..np {
                let (s, c) = (w0 * self.freq_mul[k]).sin_cos();
                self.cr[k] = c;
                self.ci[k] = s;
            }
        }

        let tilt = self.tilt_eff;
        let oe = self.cur[P::ODDEVEN] * 6.0;
        let (f1g, f2g) = (self.cur[P::F1_GAIN], self.cur[P::F2_GAIN]);
        let l1 = self.f1_eff.max(20.0).log2();
        let l2 = self.cur[P::F2_FREQ].max(20.0).log2();
        let l2f0 = f0.log2();
        let nb = self.n_bands;
        let mut db = [f32::NEG_INFINITY; MAX_PARTIALS];
        let mut maxdb = f32::NEG_INFINITY;
        for k in 0..np {
            let f = f0 * self.freq_mul[k];
            if f >= nyq {
                continue;
            }
            let lf = l2f0 + self.freq_lg[k];
            let mut d = tilt * self.ratio_lr[k] + if k % 2 == 0 { oe } else { -oe };
            if f1g > 0.01 {
                let x = (lf - l1) / FORMANT_SIGMA_OCT;
                d += f1g * (-0.5 * x * x).exp();
            }
            if f2g > 0.01 {
                let x = (lf - l2) / FORMANT_SIGMA_OCT;
                d += f2g * (-0.5 * x * x).exp();
            }
            d += gain_at(f, &self.centres[..nb], &self.tone_eff[..nb]);
            if d.is_finite() {
                db[k] = d;
                maxdb = maxdb.max(d);
            }
        }

        // Ring: per-partial decay clock in note mode.
        let damp = self.cur[P::DAMP];
        if !self.live && damp > 0.0 {
            let x = dt / self.cur[P::ENV_DECAY];
            for k in 0..np {
                // tau_k = decay * ratio^(-2 damp)
                let rate = x * (2.0 * damp * self.ratio_lr[k]).exp2();
                self.ring[k] *= (-rate).exp();
            }
        }

        let mut lin = [0.0f32; MAX_PARTIALS];
        let mut power = 0.0;
        let mut active = 0u32;
        if maxdb.is_finite() {
            for k in 0..np {
                if db[k] > maxdb + DB_FLOOR {
                    let l = ((db[k] - maxdb) * 0.166_096_4).exp2();
                    lin[k] = l;
                    power += l * l;
                    active += 1;
                }
            }
        }
        let norm = if power > 0.0 { (PARTIAL_POWER * 2.0 / power).sqrt() * gp } else { 0.0 };
        let mut hi = 0;
        for k in 0..MAX_PARTIALS {
            let t = if k < np { lin[k] * norm * self.ring[k] } else { 0.0 };
            self.tgt[k] = if t.is_finite() { t } else { 0.0 };
            self.step[k] = (self.tgt[k] - self.amp[k]) * (1.0 / TICK as f32);
            if self.tgt[k] > 0.0 || self.amp[k] > 0.0 {
                hi = k + 1;
            }
        }
        self.n_proc = hi.div_ceil(4) * 4;
        self.meters.active_partials = if gp > 0.0 { active } else { 0 };

        // Renormalise rotators against slow drift.
        for k in 0..self.n_proc {
            let s = 1.5 - 0.5 * (self.re[k] * self.re[k] + self.im[k] * self.im[k]);
            self.re[k] *= s;
            self.im[k] *= s;
        }
    }

    fn update_noise(&mut self, gn: f32) {
        let nb = self.n_bands;
        let mut any = false;
        for i in 0..nb {
            self.ng[i] = self.ng_tgt[i];
            let t = db_to_lin(self.noise_eff[i]) * gn * self.noise_norm;
            let t = if t.is_finite() { t } else { 0.0 };
            self.ng_step[i] = (t - self.ng[i]) * (1.0 / TICK as f32);
            self.ng_tgt[i] = t;
            any |= t > 0.0 || self.ng[i] > 0.0;
        }
        self.noise_on = any;
    }

    // ---- audio rate ----

    fn render_chunk(&mut self, out: &mut [f32]) {
        let n = out.len();
        let mut buf = [0.0f32; TICK];

        // Partial bank, four at a time so the inner loop becomes SIMD.
        for c in 0..self.n_proc / 4 {
            let b = c * 4;
            let mut re = [0.0f32; 4];
            let mut im = [0.0f32; 4];
            let mut cr = [0.0f32; 4];
            let mut ci = [0.0f32; 4];
            let mut amp = [0.0f32; 4];
            let mut st = [0.0f32; 4];
            re.copy_from_slice(&self.re[b..b + 4]);
            im.copy_from_slice(&self.im[b..b + 4]);
            cr.copy_from_slice(&self.cr[b..b + 4]);
            ci.copy_from_slice(&self.ci[b..b + 4]);
            amp.copy_from_slice(&self.amp[b..b + 4]);
            st.copy_from_slice(&self.step[b..b + 4]);
            for y in buf.iter_mut().take(n) {
                let mut s = [0.0f32; 4];
                for j in 0..4 {
                    let nr = re[j] * cr[j] - im[j] * ci[j];
                    let ni = re[j] * ci[j] + im[j] * cr[j];
                    re[j] = nr;
                    im[j] = ni;
                    amp[j] += st[j];
                    s[j] = amp[j] * ni;
                }
                *y += (s[0] + s[1]) + (s[2] + s[3]);
            }
            self.re[b..b + 4].copy_from_slice(&re);
            self.im[b..b + 4].copy_from_slice(&im);
            self.amp[b..b + 4].copy_from_slice(&amp);
        }

        // Shaped noise.
        if self.noise_on {
            let nb = self.n_bands;
            for y in buf.iter_mut().take(n) {
                let x = self.noise.next();
                let mut s = 0.0;
                for b in 0..nb {
                    self.ng[b] += self.ng_step[b];
                    s += self.ng[b] * self.bp[b].bandpass_norm(x);
                }
                *y += s;
            }
        }

        // Output: click, DC blocker, soft clip.
        let mut ok = true;
        for i in 0..n {
            let mut x = buf[i];
            if self.click > 1e-6 {
                x += self.click * self.noise.next();
                self.click *= self.click_dec;
            }
            let y = soft_clip(self.dc.process(x));
            ok &= y.is_finite();
            out[i] = y;
        }
        if !ok {
            self.reset_state();
            out.fill(0.0);
        }
    }

    #[cfg(test)]
    fn snap_params(&mut self) {
        self.cur = self.target;
        self.tone_c = self.tone_t;
        self.noise_c = self.noise_t;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsp::frame::{SILENT, UNVOICED, VOICED};
    use dsp::hz_to_cents;

    const SR: f32 = 48_000.0;
    const A4: f32 = 5700.0;

    fn synth() -> Synth {
        Synth::new(SR, 16, 128)
    }

    fn render(s: &mut Synth, n: usize) -> Vec<f32> {
        let mut v = vec![0.0; n];
        for c in v.chunks_mut(128) {
            s.render(c, None);
        }
        v
    }

    fn render_live(s: &mut Synth, n: usize, tuned: f32, g: &Gesture) -> Vec<f32> {
        let mut v = vec![0.0; n];
        let d = LiveDrive { tuned_cents: tuned, gesture: g };
        for c in v.chunks_mut(128) {
            s.render(c, Some(&d));
        }
        v
    }

    fn gesture() -> Gesture {
        Gesture {
            pitch_cents: f32::NAN,
            pitch_conf: 1.0,
            pitch_updates: [(0.0, 0.0); dsp::capture::MAX_PITCH_UPDATES],
            n_pitch_updates: 0,
            loudness: 1.0,
            openness: 0.0,
            breathiness: 0.0,
            onset: 0.0,
            hardness: 0.0,
            voicing: VOICED,
            gate: true,
            gate_env: 1.0,
            bands_db: [0.0; MAX_BANDS],
            input_db: -20.0,
            floor_db: -80.0,
        }
    }

    /// Hann-windowed power spectrum by direct DFT (test-only, O(n^2)).
    fn spectrum(x: &[f32]) -> Vec<f32> {
        let n = x.len();
        let tw: Vec<(f32, f32)> =
            (0..n).map(|i| ((TAU * i as f32 / n as f32).cos(), (TAU * i as f32 / n as f32).sin())).collect();
        let w: Vec<f32> = (0..n).map(|i| 0.5 - 0.5 * tw[i].0).collect();
        (0..n / 2)
            .map(|k| {
                let (mut re, mut im) = (0.0f64, 0.0f64);
                for i in 0..n {
                    let (c, s) = tw[(k * i) % n];
                    let v = (x[i] * w[i]) as f64;
                    re += v * c as f64;
                    im -= v * s as f64;
                }
                (re * re + im * im) as f32
            })
            .collect()
    }

    fn bin(f: f32, n: usize) -> usize {
        (f * n as f32 / SR).round() as usize
    }

    fn band_pow(sp: &[f32], lo: f32, hi: f32) -> f32 {
        let n = sp.len() * 2;
        sp[bin(lo, n)..=bin(hi, n)].iter().sum()
    }

    fn peak_hz(sp: &[f32], lo: f32, hi: f32) -> f32 {
        let n = sp.len() * 2;
        let (a, b) = (bin(lo, n), bin(hi, n));
        let k = (a..=b).max_by(|&i, &j| sp[i].total_cmp(&sp[j])).unwrap();
        k as f32 * SR / n as f32
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    fn centroid(sp: &[f32], hi_hz: f32) -> f32 {
        let n = sp.len() * 2;
        let (mut num, mut den) = (0.0, 0.0);
        for k in 1..bin(hi_hz, n) {
            num += k as f32 * sp[k];
            den += sp[k];
        }
        num / den * SR / n as f32
    }

    fn note_tail(s: &mut Synth, cents: f32) -> Vec<f32> {
        s.note_on(cents, 1.0);
        render(s, 24_000);
        render(s, 4096)
    }

    #[test]
    fn harmonic_note_has_dominant_fundamental() {
        let mut s = synth();
        s.set_param(P::NOISE_MIX, 0.0);
        let x = note_tail(&mut s, A4);
        let sp = spectrum(&x);
        let p = peak_hz(&sp, 100.0, 5000.0);
        assert!((p - 440.0).abs() < 12.0, "peak at {p}");
        assert!((s.meters().f0_hz - 440.0).abs() < 1.0);
    }

    #[test]
    fn bar_mode_puts_a_peak_at_2_76() {
        let mut s = synth();
        s.set_param(P::NOISE_MIX, 0.0);
        s.set_param(P::MODAL, 2.0);
        s.set_param(P::TILT, 0.0);
        s.set_param(P::PARTIALS, 5.0);
        let f0 = 200.0;
        let x = note_tail(&mut s, hz_to_cents(f0));
        let sp = spectrum(&x);
        let at_bar = band_pow(&sp, 2.76 * f0 * 0.98, 2.76 * f0 * 1.02);
        let at_harm = band_pow(&sp, 3.0 * f0 * 0.98, 3.0 * f0 * 1.02);
        assert!(at_bar > 50.0 * at_harm, "{at_bar} vs {at_harm}");
        let p = peak_hz(&sp, 400.0, 700.0);
        assert!((p / f0 - 2.76).abs() < 0.07, "peak ratio {}", p / f0);
    }

    #[test]
    fn lower_tilt_lowers_centroid() {
        let mut cs = Vec::new();
        for tilt in [-3.0, -18.0] {
            let mut s = synth();
            s.set_param(P::NOISE_MIX, 0.0);
            s.set_param(P::TILT, tilt);
            let x = note_tail(&mut s, hz_to_cents(220.0));
            cs.push(centroid(&spectrum(&x), 8000.0));
        }
        assert!(cs[1] < cs[0] * 0.7, "{cs:?}");
    }

    #[test]
    fn tone_band_boost_raises_energy_near_2k() {
        let mut ratios = Vec::new();
        for boost in [0.0, 18.0] {
            let mut s = synth();
            s.set_param(P::NOISE_MIX, 0.0);
            let mut c = [0.0; MAX_BANDS];
            band_centres(16, &mut c);
            for i in 0..16 {
                if c[i] > 1500.0 && c[i] < 2800.0 {
                    s.set_tone_band(i, boost);
                }
            }
            let x = note_tail(&mut s, hz_to_cents(200.0));
            let sp = spectrum(&x);
            ratios.push(band_pow(&sp, 1700.0, 2300.0) / band_pow(&sp, 150.0, 1000.0));
        }
        assert!(ratios[1] > ratios[0] * 4.0, "{ratios:?}");
    }

    #[test]
    fn noise_only_is_broadband_and_shaped() {
        let mut s = synth();
        s.set_param(P::HARM_MIX, 0.0);
        s.set_param(P::NOISE_MIX, 1.0);
        s.set_param(P::ENV_NOISE_DECAY, 4.0);
        s.set_param(P::ENV_RELEASE, 4.0);
        s.snap_params();
        s.note_on(A4, 1.0);
        render(&mut s, 12_000);
        let flat = spectrum(&render(&mut s, 4096));
        let r_flat = band_pow(&flat, 5000.0, 8000.0) / band_pow(&flat, 300.0, 600.0);
        assert!(rms(&render(&mut s, 2048)) > 0.01);
        // Silence the high bands.
        let mut c = [0.0; MAX_BANDS];
        band_centres(16, &mut c);
        for i in 0..16 {
            if c[i] > 1200.0 {
                s.set_noise_band(i, -40.0);
            }
        }
        render(&mut s, 12_000);
        let shaped = spectrum(&render(&mut s, 4096));
        let r_shaped = band_pow(&shaped, 5000.0, 8000.0) / band_pow(&shaped, 300.0, 600.0);
        assert!(r_flat > 0.3, "flat noise should reach the highs: {r_flat}");
        assert!(r_shaped < r_flat * 0.05, "{r_shaped} vs {r_flat}");
    }

    #[test]
    fn attack_peaks_near_attack_time_and_release_decays() {
        let mut s = synth();
        s.set_param(P::NOISE_MIX, 0.0);
        s.set_param(P::ENV_ATTACK, 0.2);
        s.set_param(P::ENV_RELEASE, 0.05);
        s.set_param(P::ENV_DECAY, 1.0);
        s.snap_params();
        s.note_on(A4, 1.0);
        let x = render(&mut s, 48_000);
        let win = 480;
        let env: Vec<f32> = x.chunks(win).map(rms).collect();
        let peak = (0..env.len()).max_by(|&a, &b| env[a].total_cmp(&env[b])).unwrap();
        let t = (peak as f32 + 0.5) * win as f32 / SR;
        assert!((t - 0.2).abs() < 0.05, "peak at {t}");
        assert!(env[0] < 0.1 * env[peak]);
        s.note_off();
        render(&mut s, 19_200);
        let tail = rms(&render(&mut s, 2400));
        assert!(tail < 0.01 * env[peak], "{tail}");
    }

    #[test]
    fn damp_makes_high_partials_die_first() {
        let mut s = synth();
        s.set_param(P::NOISE_MIX, 0.0);
        s.set_param(P::TILT, 0.0);
        s.set_param(P::DAMP, 1.0);
        s.set_param(P::ENV_DECAY, 0.3);
        s.set_param(P::ENV_SUSTAIN, 1.0);
        s.set_param(P::ENV_ATTACK, 0.001);
        s.set_param(P::PARTIALS, 16.0);
        let f0 = 200.0;
        s.note_on(hz_to_cents(f0), 1.0);
        let early = spectrum(&render(&mut s, 4096)[..4096]);
        render(&mut s, 9000);
        let late = spectrum(&render(&mut s, 4096));
        let h = |sp: &[f32], k: f32| band_pow(sp, k * f0 * 0.97, k * f0 * 1.03);
        let hi_ratio = h(&late, 8.0) / h(&early, 8.0);
        let lo_ratio = h(&late, 1.0) / h(&early, 1.0);
        assert!(hi_ratio < lo_ratio * 0.01, "hi {hi_ratio} lo {lo_ratio}");
    }

    #[test]
    fn pitch_drop_starts_high() {
        let mut s = synth();
        s.set_param(P::NOISE_MIX, 0.0);
        s.set_param(P::ENV_PITCH_AMT, 12.0);
        s.set_param(P::ENV_PITCH_TIME, 0.05);
        s.note_on(A4, 1.0);
        render(&mut s, 64);
        assert!(s.meters().f0_hz > 800.0, "{}", s.meters().f0_hz);
        render(&mut s, 24_000);
        assert!((s.meters().f0_hz - 440.0).abs() < 2.0);
    }

    #[test]
    fn live_voiced_plays_tuned_pitch() {
        let mut s = synth();
        s.set_param(P::NOISE_MIX, 0.0);
        let g = gesture();
        render_live(&mut s, 12_000, A4, &g);
        let x = render_live(&mut s, 4096, A4, &g);
        let sp = spectrum(&x);
        let p = peak_hz(&sp, 100.0, 5000.0);
        assert!((p - 440.0).abs() < 12.0, "peak at {p}");
        assert!(rms(&x) > 0.01);
        assert!(s.meters().voiced_mix > 0.99);
    }

    #[test]
    fn live_loudness_depth_scales_level() {
        let mut quiet = gesture();
        quiet.loudness = 0.25;
        let mut s = synth();
        s.set_param(P::DEPTH_LOUD, 1.0);
        render_live(&mut s, 12_000, A4, &quiet);
        let a = rms(&render_live(&mut s, 4096, A4, &quiet));
        let mut s2 = synth();
        s2.set_param(P::DEPTH_LOUD, 0.0);
        render_live(&mut s2, 12_000, A4, &quiet);
        let b = rms(&render_live(&mut s2, 4096, A4, &quiet));
        assert!(a < b * 0.4, "{a} vs {b}");
    }

    #[test]
    fn unvoiced_switches_to_noise_within_15ms() {
        let mut s = synth();
        let mut g = gesture();
        render_live(&mut s, 12_000, A4, &g);
        assert!(s.meters().voiced_mix > 0.99);
        g.voicing = UNVOICED;
        render_live(&mut s, 720, A4, &g);
        assert!(s.meters().voiced_mix < 0.01);
        let x = render_live(&mut s, 8192, A4, &g);
        assert!(rms(&x) > 0.02, "noise should sound: {}", rms(&x));
        let sp = spectrum(&x[..4096]);
        // No sharp peak at the carrier: energy near 440 is close to its neighbours.
        let at = band_pow(&sp, 420.0, 460.0);
        assert!(at < 30.0 * band_pow(&sp, 520.0, 560.0), "tonal peak remains");
    }

    #[test]
    fn gate_closed_goes_silent() {
        let mut s = synth();
        let mut g = gesture();
        render_live(&mut s, 12_000, A4, &g);
        g.gate = false;
        g.gate_env = 0.0;
        g.voicing = SILENT;
        render_live(&mut s, 9600, A4, &g);
        let x = render_live(&mut s, 2048, A4, &g);
        assert!(x.iter().all(|v| v.abs() < 1e-4), "max {}", x.iter().fold(0.0f32, |a, v| a.max(v.abs())));
    }

    #[test]
    fn no_pitch_at_gate_open_plays_noise_then_glides_in() {
        let mut s = synth();
        let g = gesture();
        let x = render_live(&mut s, 4800, f32::NAN, &g);
        assert!(rms(&x) > 0.01, "noise only");
        assert!(s.meters().voiced_mix < 0.01);
        render_live(&mut s, 4800, A4, &g);
        assert!(s.meters().voiced_mix > 0.99);
        assert!((s.meters().f0_hz - 440.0).abs() < 2.0);
    }

    #[test]
    fn last_pitch_used_while_estimate_missing() {
        let mut s = synth();
        let mut g = gesture();
        s.set_param(P::NOISE_MIX, 0.0);
        render_live(&mut s, 12_000, A4, &g);
        g.gate = false;
        g.gate_env = 0.0;
        g.voicing = SILENT;
        render_live(&mut s, 4800, f32::NAN, &g);
        g.gate = true;
        g.gate_env = 1.0;
        g.voicing = VOICED;
        render_live(&mut s, 1200, f32::NAN, &g);
        assert!((s.meters().f0_hz - 440.0).abs() < 1.0, "{}", s.meters().f0_hz);
        assert!(s.meters().voiced_mix > 0.99);
        // First estimate a fourth up: reached within about 20 ms.
        render_live(&mut s, 960, A4 + 500.0, &g);
        let target = cents_to_hz(A4 + 500.0);
        assert!((s.meters().f0_hz - target).abs() < 0.03 * target, "{}", s.meters().f0_hz);
    }

    #[test]
    fn mouth_depth_blends_tone_bands() {
        let mut g = gesture();
        for i in 0..MAX_BANDS {
            g.bands_db[i] = i as f32 - 8.0;
        }
        let mut s = synth();
        for i in 0..16 {
            s.set_tone_band(i, 3.0);
        }
        s.set_param(P::DEPTH_MOUTH, 1.0);
        render_live(&mut s, 24_000, A4, &g);
        for i in 0..16 {
            assert!((s.meters().tone_eff_db[i] - g.bands_db[i]).abs() < 0.05);
        }
        s.set_param(P::DEPTH_MOUTH, 0.0);
        render_live(&mut s, 24_000, A4, &g);
        for i in 0..16 {
            assert!((s.meters().tone_eff_db[i] - 3.0).abs() < 0.05);
        }
        s.set_param(P::DEPTH_MOUTH, 0.5);
        render_live(&mut s, 24_000, A4, &g);
        assert!((s.meters().tone_eff_db[10] - 0.5 * (3.0 + 2.0)).abs() < 0.05);
        assert_eq!(s.meters().tone_eff_db[20], 0.0);
    }

    #[test]
    fn output_is_always_finite_and_bounded() {
        let mut s = synth();
        s.set_param(P::LEVEL, 6.0);
        s.set_param(P::NOISE_MIX, 1.0);
        s.set_param(P::TILT, 6.0);
        s.set_param(P::F1_GAIN, 24.0);
        s.set_param(P::F2_GAIN, 24.0);
        s.set_param(P::PARTIALS, 64.0);
        s.set_param(P::STRETCH, 0.01);
        s.set_param(P::MODAL, 1.5);
        s.set_param(P::ENV_PITCH_AMT, 48.0);
        for i in 0..16 {
            s.set_tone_band(i, 40.0);
            s.set_noise_band(i, 40.0);
        }
        s.note_on(hz_to_cents(8000.0), 1.0);
        let x = render(&mut s, 20_000);
        assert!(x.iter().all(|v| v.is_finite() && v.abs() <= 1.0));
        let mut g = gesture();
        g.bands_db = [f32::NAN; MAX_BANDS];
        g.loudness = f32::INFINITY;
        g.onset = 1.0;
        g.hardness = 1.0;
        let x = render_live(&mut s, 20_000, f32::INFINITY, &g);
        assert!(x.iter().all(|v| v.is_finite() && v.abs() <= 1.0));
        s.panic();
        let x = render(&mut s, 256);
        assert!(x.iter().all(|v| v.abs() < 1e-3));
    }

    #[test]
    fn block_length_does_not_change_output() {
        let setup = || {
            let mut s = synth();
            s.set_param(P::ENV_PITCH_AMT, 5.0);
            s.set_param(P::DAMP, 0.5);
            s.set_param(P::MODAL, 1.0);
            s.note_on(A4, 0.8);
            s
        };
        let mut a = setup();
        let mut whole = vec![0.0; 4096];
        a.render(&mut whole, None);
        for &len in &[1usize, 7, 128, 4096] {
            let mut b = setup();
            let mut x = vec![0.0; 4096];
            for c in x.chunks_mut(len) {
                b.render(c, None);
            }
            let err = whole.iter().zip(&x).fold(0.0f32, |m, (p, q)| m.max((p - q).abs()));
            assert!(err < 1e-6, "block {len}: {err}");
        }
        assert!(rms(&whole) > 0.01);
    }

    #[test]
    fn live_block_length_does_not_change_output() {
        let g = gesture();
        let mut a = synth();
        let mut whole = vec![0.0; 4096];
        a.render(&mut whole, Some(&LiveDrive { tuned_cents: A4, gesture: &g }));
        for &len in &[1usize, 7, 128] {
            let mut b = synth();
            let mut x = vec![0.0; 4096];
            for c in x.chunks_mut(len) {
                b.render(c, Some(&LiveDrive { tuned_cents: A4, gesture: &g }));
            }
            let err = whole.iter().zip(&x).fold(0.0f32, |m, (p, q)| m.max((p - q).abs()));
            assert!(err < 1e-6, "block {len}: {err}");
        }
    }

    #[test]
    #[ignore]
    fn bench_64_partials() {
        let mut s = synth();
        s.set_param(P::PARTIALS, 64.0);
        s.set_param(P::TILT, -2.0);
        s.snap_params();
        s.note_on(hz_to_cents(110.0), 1.0);
        let mut buf = [0.0f32; 128];
        for _ in 0..100 {
            s.render(&mut buf, None);
        }
        let t = std::time::Instant::now();
        let blocks = 4000;
        for _ in 0..blocks {
            s.render(&mut buf, None);
        }
        let ns = t.elapsed().as_nanos() as f64 / (blocks * 128) as f64;
        println!("64 partials + noise: {ns:.1} ns/sample, active {}", s.meters().active_partials);
    }
}
