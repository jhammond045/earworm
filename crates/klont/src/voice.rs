//! One Klont voice: the per-voice modules of the TDD "Klont DSP" table and
//! the drum model. The layer resolves parameters (steps 1 to 6) and hands the
//! voice real-unit controls once per control tick; the voice runs its own
//! envelopes and modulators (step 7) and renders audio.
//!
//! Signal path per sample:
//! (partial bank + start/end click) -> clang -> fold -> comb -> filter -> out.
//! Shaped noise is per layer (TDD: equivalent because the path is linear);
//! the voice only reports how loud its noise should be (`noise_level`).
//!
//! Control rate: the engine calls `control` at every global tick boundary
//! (every TICK samples) and `start` at any sample. Between calls `render` is
//! called on consecutive sub-spans whose lengths add up to the `len` given
//! to the last `control`/`start`. Amplitudes ramp linearly across that `len`,
//! so a note can start mid-tick and still ramp to the next boundary.
//! The output must not depend on how a span is split into `render` calls.

/// Samples per control tick.
pub const TICK: usize = 32;
pub const MAX_PARTIALS: usize = 64;
pub const N_BANDS: usize = 16;

/// Per-tick controls in real units, after resolution steps 1 to 6.
#[derive(Clone, Copy, Debug)]
pub struct VoiceControls {
    /// Carrier pitch in cents above C0, before the pitch envelope and LFOs.
    pub cents: f32,
    /// Linear amplitude (layer level is applied by the layer, not here):
    /// velocity and gesture loudness. 1 is nominal.
    pub amp: f32,
    pub harm_mix: f32,
    pub noise_mix: f32,
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
    /// dB at the 16 band centres (dsp::bands::band_centres(16, ..)).
    pub tone_bands: [f32; N_BANDS],
    pub fold_amt: f32,
    pub fold_bias: f32,
    pub clang_amt: f32,
    pub clang_ratio: f32,
    pub comb_amt: f32,
    pub comb_tune: f32,
    pub comb_decay: f32,
    pub filt_cutoff: f32,
    pub filt_res: f32,
    pub filt_morph: f32,
    pub filt_track: f32,
    pub strike_color: f32,
    pub lfo1_rate: f32,
    pub lfo1_pitch: f32,
    pub lfo1_filter: f32,
    pub lfo1_level: f32,
    pub lfo2_rate: f32,
    pub lfo2_pitch: f32,
    pub lfo2_filter: f32,
    pub chaos_rate: f32,
    pub chaos_amt: f32,
    /// Most partials this voice may sound this tick (the global budget's share).
    pub max_partials: usize,
}

/// Fixed for the life of a note (hit shape already applied by the layer).
#[derive(Clone, Copy, Debug)]
pub struct NoteStart {
    pub velocity: f32,
    pub attack: f32,
    pub hold: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    pub noise_decay: f32,
    /// Semitones at note start, decaying with `pitch_time`.
    pub pitch_amt: f32,
    pub pitch_time: f32,
    /// Start click amount 0..1.
    pub click: f32,
    /// End click amount 0..1, fired on release (or at a choke from end_stop).
    pub end_click: f32,
    /// Hit shape endStop: choke (3 ms) at the end of hold instead of decaying.
    pub end_stop: bool,
}

use dsp::bands::band_centres;
use dsp::filters::Svf;
use dsp::fx::{Chaos, Clang, Click, CombPair, Folder, Lfo, fast_exp2, fast_log2, fast_sin_cos};
use dsp::noise::Noise;
use dsp::{C0_HZ, DENORMAL};
use std::f32::consts::{LOG2_E, TAU};

/// Summed partial power after normalisation: sum of a_k^2 / 2.
const PARTIAL_POWER: f32 = 0.03;
/// Width of a formant bell in octaves (sigma of a Gaussian in log frequency).
const FORMANT_SIGMA_OCT: f32 = 0.15;
const DB_FLOOR: f32 = 72.0;
const CHOKE_S: f32 = 0.003;
/// Output peak below this (-100 dB) counts as silent.
const SILENT_PEAK: f32 = 1e-5;
const OUT_LIMIT: f32 = 32.0;

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
fn cl(v: f32, lo: f32, hi: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, hi) } else { 0.0f32.clamp(lo, hi) }
}

fn clean_controls(c: &VoiceControls) -> VoiceControls {
    let mut o = *c;
    o.cents = cl(c.cents, -4800.0, 16800.0);
    o.amp = cl(c.amp, 0.0, 8.0);
    o.harm_mix = cl(c.harm_mix, 0.0, 2.0);
    o.noise_mix = cl(c.noise_mix, 0.0, 2.0);
    o.tilt = cl(c.tilt, -48.0, 48.0);
    o.oddeven = cl(c.oddeven, -1.0, 1.0);
    // sqrt(1 + stretch k^2) must stay real up to k = 64.
    o.stretch = cl(c.stretch, -2.0e-4, 0.05);
    o.jitter = cl(c.jitter, 0.0, 200.0);
    o.modal = cl(c.modal, 0.0, 2.0);
    o.damp = cl(c.damp, 0.0, 4.0);
    o.f1_freq = cl(c.f1_freq, 20.0, 20_000.0);
    o.f1_gain = cl(c.f1_gain, -60.0, 60.0);
    o.f2_freq = cl(c.f2_freq, 20.0, 20_000.0);
    o.f2_gain = cl(c.f2_gain, -60.0, 60.0);
    for (d, s) in o.tone_bands.iter_mut().zip(c.tone_bands.iter()) {
        *d = cl(*s, -60.0, 60.0);
    }
    o.fold_amt = cl(c.fold_amt, 0.0, 1.0);
    o.fold_bias = cl(c.fold_bias, -1.0, 1.0);
    o.clang_amt = cl(c.clang_amt, 0.0, 1.0);
    o.clang_ratio = cl(c.clang_ratio, 0.1, 32.0);
    o.comb_amt = cl(c.comb_amt, 0.0, 1.0);
    o.comb_tune = cl(c.comb_tune, 0.1, 16.0);
    o.comb_decay = cl(c.comb_decay, 0.005, 20.0);
    o.filt_cutoff = cl(c.filt_cutoff, 20.0, 20_000.0);
    o.filt_res = cl(c.filt_res, 0.0, 1.0);
    o.filt_morph = cl(c.filt_morph, 0.0, 1.0);
    o.filt_track = cl(c.filt_track, -2.0, 2.0);
    o.strike_color = cl(c.strike_color, 40.0, 20_000.0);
    o.lfo1_rate = cl(c.lfo1_rate, 0.0, 100.0);
    o.lfo1_pitch = cl(c.lfo1_pitch, -4800.0, 4800.0);
    o.lfo1_filter = cl(c.lfo1_filter, -8.0, 8.0);
    o.lfo1_level = cl(c.lfo1_level, 0.0, 1.0);
    o.lfo2_rate = cl(c.lfo2_rate, 0.0, 100.0);
    o.lfo2_pitch = cl(c.lfo2_pitch, -4800.0, 4800.0);
    o.lfo2_filter = cl(c.lfo2_filter, -8.0, 8.0);
    o.chaos_rate = cl(c.chaos_rate, 0.0, 100.0);
    o.chaos_amt = cl(c.chaos_amt, 0.0, 4.0);
    o.max_partials = c.max_partials.min(MAX_PARTIALS);
    o
}

fn clean_start(s: &NoteStart) -> NoteStart {
    NoteStart {
        velocity: cl(s.velocity, 0.0, 2.0),
        attack: cl(s.attack, 0.0, 30.0),
        hold: cl(s.hold, 0.0, 60.0),
        decay: cl(s.decay, 0.001, 120.0),
        sustain: cl(s.sustain, 0.0, 1.0),
        release: cl(s.release, 0.001, 120.0),
        noise_decay: cl(s.noise_decay, 0.001, 120.0),
        pitch_amt: cl(s.pitch_amt, -96.0, 96.0),
        pitch_time: cl(s.pitch_time, 0.001, 30.0),
        click: cl(s.click, 0.0, 1.0),
        end_click: cl(s.end_click, 0.0, 1.0),
        end_stop: s.end_stop,
    }
}

fn blank_controls() -> VoiceControls {
    VoiceControls {
        cents: 5700.0,
        amp: 1.0,
        harm_mix: 1.0,
        noise_mix: 0.0,
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
        fold_amt: 0.0,
        fold_bias: 0.0,
        clang_amt: 0.0,
        clang_ratio: 2.0,
        comb_amt: 0.0,
        comb_tune: 1.0,
        comb_decay: 0.5,
        filt_cutoff: 20_000.0,
        filt_res: 0.0,
        filt_morph: 0.0,
        filt_track: 0.0,
        strike_color: 3000.0,
        lfo1_rate: 0.0,
        lfo1_pitch: 0.0,
        lfo1_filter: 0.0,
        lfo1_level: 0.0,
        lfo2_rate: 0.0,
        lfo2_pitch: 0.0,
        lfo2_filter: 0.0,
        chaos_rate: 0.0,
        chaos_amt: 0.0,
        max_partials: MAX_PARTIALS,
    }
}

fn blank_start() -> NoteStart {
    NoteStart {
        velocity: 1.0,
        attack: 0.002,
        hold: 0.0,
        decay: 1.0,
        sustain: 1.0,
        release: 0.1,
        noise_decay: 0.1,
        pitch_amt: 0.0,
        pitch_time: 0.05,
        click: 0.0,
        end_click: 0.0,
        end_stop: false,
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Stage {
    Idle,
    Attack,
    Hold,
    Decay,
    Release,
    Choke,
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

pub struct Voice {
    sr: f32,
    seed: u32,
    noise: Noise,
    c: VoiceControls,
    ns: NoteStart,

    // Envelope and modulators.
    stage: Stage,
    env: f32,
    hold_left: f32,
    noise_env: f32,
    note_t: f32,
    lfo1: Lfo,
    lfo2: Lfo,
    chaos: Chaos,
    l1: f32,
    l2: f32,
    ch: f32,
    cents_now: f32,
    f0: f32,

    // Partial bank. Index k is partial k + 1.
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
    rat: [f32; MAX_PARTIALS],
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
    left: usize,
    scratch: [[f32; 4]; TICK],

    // Per-voice modules and their flags.
    click_s: Click,
    click_e: Click,
    end_post: bool,
    keep_click: bool,
    clang: Clang,
    fold: Folder,
    comb: CombPair,
    svf: Svf,
    svf_key: (u32, u32),
    clang_on: bool,
    fold_on: bool,
    comb_on: bool,
    filt_on: bool,
    chain_dirty: bool,
    fw: [f32; 3],

    // Choke gain.
    cg: f32,
    cg_t: f32,
    cg_step: f32,
    choke_done: bool,

    peak_acc: f32,
    peak_last: f32,
}

impl Voice {
    pub fn new(sample_rate: f32, seed: u32) -> Self {
        let sr = sample_rate.max(8000.0);
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
        let s = seed ^ 0x5bd1_e995;
        Voice {
            sr,
            seed,
            noise: Noise::new(seed),
            c: blank_controls(),
            ns: blank_start(),
            stage: Stage::Idle,
            env: 0.0,
            hold_left: 0.0,
            noise_env: 0.0,
            note_t: 0.0,
            lfo1: Lfo::new(),
            lfo2: Lfo::new(),
            chaos: Chaos::new(s),
            l1: 0.0,
            l2: 0.0,
            ch: 0.0,
            cents_now: 5700.0,
            f0: 440.0,
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
            rat: [1.0; MAX_PARTIALS],
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
            left: 0,
            scratch: [[0.0; 4]; TICK],
            click_s: Click::new(sr, seed ^ 0x1234_5679),
            click_e: Click::new(sr, seed ^ 0x9e37_79b1),
            end_post: false,
            keep_click: false,
            clang: Clang::new(sr),
            fold: Folder::new(sr),
            comb: CombPair::new(sr),
            svf: Svf::default(),
            svf_key: (0, 0),
            clang_on: false,
            fold_on: false,
            comb_on: false,
            filt_on: false,
            chain_dirty: false,
            fw: [0.0; 3],
            cg: 1.0,
            cg_t: 1.0,
            cg_step: 1.0 / (CHOKE_S * sr),
            choke_done: false,
            peak_acc: 0.0,
            peak_last: 0.0,
        }
    }

    /// Start a note. `len` samples remain until the next tick boundary.
    pub fn start(&mut self, s: &NoteStart, c: &VoiceControls, len: usize) {
        let was_active = self.active();
        if !was_active {
            self.reset_all();
        }
        self.ns = clean_start(s);
        self.c = clean_controls(c);
        // A restart keeps phases and amplitudes; only a choke in progress is cancelled.
        self.cg_t = 1.0;
        self.choke_done = false;
        self.end_post = false;
        self.click_e.reset();
        self.stage = Stage::Attack;
        self.hold_left = self.ns.hold;
        self.noise_env = 1.0;
        self.note_t = 0.0;
        self.ring = [1.0; MAX_PARTIALS];
        self.lfo1.reset();
        self.lfo2.reset();
        self.chaos.reset(self.seed ^ 0x5bd1_e995);
        self.l1 = 0.0;
        self.l2 = 0.0;
        self.ch = 0.0;
        for r in self.jit_r.iter_mut() {
            *r = self.noise.next();
        }
        self.fm_dirty = true;
        self.spec_key = None;
        self.peak_acc = 0.0;
        self.peak_last = 0.0;
        if self.ns.click > 0.0 {
            self.click_s.trigger(self.ns.click, self.c.strike_color);
        }
        self.tick(len.max(1), true);
    }

    /// Once per tick while active. `len` is normally TICK.
    pub fn control(&mut self, c: &VoiceControls, len: usize) {
        if !self.active() {
            return;
        }
        self.c = clean_controls(c);
        self.peak_last = self.peak_acc;
        self.peak_acc = 0.0;
        self.tick(len.max(1), false);
    }

    /// Note off: enter release and fire the end click.
    pub fn release(&mut self) {
        match self.stage {
            Stage::Attack | Stage::Hold | Stage::Decay => {
                self.stage = Stage::Release;
                if self.ns.end_click > 0.0 {
                    self.click_e.trigger(self.ns.end_click, self.c.strike_color);
                }
            }
            _ => {}
        }
    }

    /// Fade to silence in 3 ms, then go idle.
    pub fn choke(&mut self) {
        if self.stage == Stage::Idle || self.stage == Stage::Choke {
            return;
        }
        self.stage = Stage::Choke;
        self.cg_t = 0.0;
    }

    /// Silence at once and clear all state.
    pub fn kill(&mut self) {
        self.reset_all();
    }

    /// Add this voice into `out` (mono).
    pub fn render(&mut self, out: &mut [f32]) {
        if !self.active() {
            return;
        }
        let mut pos = 0;
        while pos < out.len() {
            let mut m = (out.len() - pos).min(TICK);
            if self.left > 0 {
                m = m.min(self.left);
            }
            self.render_piece(&mut out[pos..pos + m]);
            pos += m;
        }
    }

    /// Sounding or ringing (an idle voice costs nothing and may be reused).
    pub fn active(&self) -> bool {
        self.stage != Stage::Idle
            || self.noise_level() > 1e-4
            || self.click_s.active()
            || self.click_e.active()
            || self.peak_last.max(self.peak_acc) > SILENT_PEAK
    }

    pub fn released(&self) -> bool {
        matches!(self.stage, Stage::Release | Stage::Choke)
    }

    /// Envelope times amp, for budget sharing and voice stealing.
    pub fn loudness(&self) -> f32 {
        self.env * self.c.amp * self.ns.velocity * self.cg
    }

    /// Value of the second LFO (-1..1) at the last control tick; the grain
    /// cloud reads it for `lfo2.to_grain`.
    pub fn lfo2(&self) -> f32 {
        self.l2
    }

    /// Shaped-noise amplitude this voice asks of its layer for the current
    /// tick: noise_mix * noise envelope * amp * velocity.
    pub fn noise_level(&self) -> f32 {
        self.c.noise_mix * self.noise_env * self.c.amp * self.ns.velocity * self.cg
    }

    /// Audible partials at the last control tick before the budget cut.
    pub fn wanted_partials(&self) -> usize {
        self.n_wanted
    }

    /// Partials actually sounding at the last control tick.
    pub fn active_partials(&self) -> usize {
        self.n_kept
    }

    /// Current pitch in cents including pitch envelope and modulators.
    pub fn pitch_cents(&self) -> f32 {
        self.cents_now
    }

    // ---- state ----

    fn reset_all(&mut self) {
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
        self.left = 0;
        self.spec_key = None;
        self.stage = Stage::Idle;
        self.env = 0.0;
        self.noise_env = 0.0;
        self.click_s.reset();
        if !self.keep_click {
            self.click_e.reset();
            self.end_post = false;
        }
        self.clang_on = false;
        self.fold_on = false;
        self.comb_on = false;
        self.filt_on = false;
        if self.chain_dirty {
            self.clang.reset();
            self.fold.reset();
            self.comb.reset();
            self.chain_dirty = false;
        }
        self.svf.reset();
        self.svf_key = (0, 0);
        self.cg = 1.0;
        self.cg_t = 1.0;
        self.choke_done = false;
        self.peak_acc = 0.0;
        self.peak_last = 0.0;
        if !self.cents_now.is_finite() {
            self.cents_now = 5700.0;
        }
    }

    // ---- control rate ----

    fn env_tick(&mut self, dt: f32) {
        let ns = self.ns;
        match self.stage {
            Stage::Attack => {
                self.env += dt / ns.attack.max(1e-3);
                if self.env >= 1.0 {
                    self.env = 1.0;
                    if ns.hold > 0.0 {
                        self.hold_left = ns.hold;
                        self.stage = Stage::Hold;
                    } else if ns.end_stop {
                        self.end_stop_choke();
                    } else {
                        self.stage = Stage::Decay;
                    }
                }
            }
            Stage::Hold => {
                self.hold_left -= dt;
                if self.hold_left <= 0.0 {
                    if ns.end_stop {
                        self.end_stop_choke();
                    } else {
                        self.stage = Stage::Decay;
                    }
                }
            }
            Stage::Decay => {
                let s = ns.sustain;
                self.env = s + (self.env - s) * fast_exp2(-dt / ns.decay * LOG2_E);
                if s < 1e-4 && self.env < 1e-4 {
                    self.env = 0.0;
                    self.stage = Stage::Idle;
                }
            }
            Stage::Release => {
                self.env *= fast_exp2(-dt / ns.release * LOG2_E);
                if self.env < 1e-4 {
                    self.env = 0.0;
                    self.stage = Stage::Idle;
                }
            }
            Stage::Idle | Stage::Choke => {}
        }
        self.noise_env *= fast_exp2(-dt / ns.noise_decay * LOG2_E);
        if self.stage == Stage::Release {
            self.noise_env *= fast_exp2(-dt / ns.release * LOG2_E);
        }
        if self.noise_env < 1e-5 {
            self.noise_env = 0.0;
        }
    }

    fn end_stop_choke(&mut self) {
        self.stage = Stage::Choke;
        self.cg_t = 0.0;
        if self.ns.end_click > 0.0 {
            // The click is added after the choke gain so the fade does not eat it.
            self.click_e.trigger(self.ns.end_click, self.c.strike_color);
            self.end_post = true;
        }
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
            self.rat[k] = r;
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

    fn update_ring_rates(&mut self, dt: f32) {
        let key = [self.c.damp, self.ns.decay, self.c.modal];
        if self.ring_dirty || key != self.ring_key {
            self.ring_key = key;
            self.ring_dirty = false;
            self.ring_dt = -1.0;
            let (damp, decay) = (self.c.damp, self.ns.decay);
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

    /// Control-rate update. `first` is true when called from `start`: the
    /// modulators and pitch envelope sit at their note-start values.
    fn tick(&mut self, len: usize, first: bool) {
        let dt = len as f32 / self.sr;
        if !first {
            self.note_t += dt;
            self.l1 = self.lfo1.tick(self.c.lfo1_rate, dt);
            self.l2 = self.lfo2.tick(self.c.lfo2_rate, dt);
            self.ch = self.chaos.tick(self.c.chaos_rate, dt);
        }
        self.env_tick(dt);

        let c = self.c;
        let ns = self.ns;
        let nyq = 0.45 * self.sr;

        // Pitch.
        let drop = ns.pitch_amt * 100.0 * fast_exp2(-self.note_t / ns.pitch_time * LOG2_E);
        let cents = c.cents + drop + c.lfo1_pitch * self.l1 + c.lfo2_pitch * self.l2 + c.chaos_amt * 50.0 * self.ch;
        let f0 = (C0_HZ * fast_exp2(cents * (1.0 / 1200.0))).clamp(8.0, nyq);
        if !f0.is_finite() || !cents.is_finite() {
            self.reset_all();
            return;
        }
        self.cents_now = cents;
        self.f0 = f0;

        // Spectrum cache.
        self.refresh_freqs();
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
            self.update_ring_rates(dt);
            if self.stage != Stage::Attack {
                for k in 0..MAX_PARTIALS {
                    self.ring[k] *= self.ring_fac[k];
                }
            }
        }

        // Targets.
        let lfo_amp = 1.0 - c.lfo1_level * (0.5 - 0.5 * self.l1);
        let mut g = c.harm_mix * c.amp * ns.velocity * self.env * lfo_amp;
        if !g.is_finite() {
            g = 0.0;
        }
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
        self.left = len;
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

        self.set_chain(&c, f0);
    }

    fn set_chain(&mut self, c: &VoiceControls, f0: f32) {
        self.clang_on = c.clang_amt > 0.0;
        if self.clang_on {
            self.clang.set(f0, c.clang_ratio, c.clang_amt);
        }
        self.fold_on = c.fold_amt > 0.0;
        if self.fold_on {
            self.fold.set(c.fold_amt, c.fold_bias);
        }
        self.comb_on = c.comb_amt > 0.0;
        if self.comb_on {
            self.comb.set(f0, c.comb_tune, c.comb_decay, c.comb_amt);
        }
        self.chain_dirty |= self.clang_on || self.fold_on || self.comb_on;

        let oct = c.lfo1_filter * self.l1 + c.lfo2_filter * self.l2 + c.chaos_amt * self.ch;
        let track = fast_exp2(c.filt_track * fast_log2(f0 * (1.0 / 261.63)));
        let cut = (c.filt_cutoff * track * fast_exp2(oct)).clamp(20.0, 0.45 * self.sr);
        let on = !(c.filt_morph <= 0.0 && cut >= 18_000.0);
        if on {
            let q = 0.5 + 19.5 * c.filt_res * c.filt_res;
            let key = (cut.to_bits(), q.to_bits());
            if !self.filt_on {
                self.svf.reset();
                self.svf_key = (0, 0);
            }
            if key != self.svf_key {
                self.svf_key = key;
                self.svf.set(self.sr, cut, q);
            }
            let m = c.filt_morph;
            self.fw = if m < 0.5 {
                let w = m * 2.0;
                [1.0 - w, w / q, 0.0]
            } else {
                let w = m * 2.0 - 1.0;
                [0.0, (1.0 - w) / q, w]
            };
        }
        self.filt_on = on;
    }

    // ---- audio rate ----

    fn finish_choke(&mut self) {
        // Everything is silent: clear the bank and chain. A running end click
        // (end_stop) keeps playing; it is added after the choke gain.
        self.keep_click = self.end_post;
        self.reset_all();
        self.keep_click = false;
    }

    fn render_piece(&mut self, out: &mut [f32]) {
        let m = out.len();
        let nch = self.n_chunks;
        if nch > 0 {
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
        }

        let mut pk = 0.0f32;
        let mut ok = true;
        for i in 0..m {
            let mut x = if self.n_chunks > 0 {
                let a = &self.scratch[i];
                (a[0] + a[1]) + (a[2] + a[3])
            } else {
                0.0
            };
            x += self.click_s.process();
            if !self.end_post {
                x += self.click_e.process();
            }
            if self.clang_on {
                x = self.clang.process(x);
            }
            if self.fold_on {
                x = self.fold.process(x);
            }
            if self.comb_on {
                x = self.comb.process(x);
            }
            if self.filt_on {
                let o = self.svf.process(x);
                x = o.lp * self.fw[0] + o.bp * self.fw[1] + o.hp * self.fw[2] + DENORMAL;
            }
            if self.cg != self.cg_t {
                if self.cg_t < self.cg {
                    self.cg = (self.cg - self.cg_step).max(self.cg_t);
                } else {
                    self.cg = (self.cg + self.cg_step).min(self.cg_t);
                }
            }
            let mut y = x * self.cg;
            if self.end_post {
                y += self.click_e.process();
            }
            if !y.is_finite() {
                ok = false;
                break;
            }
            y = y.clamp(-OUT_LIMIT, OUT_LIMIT);
            pk = pk.max(y.abs());
            out[i] += y;
            if self.cg == 0.0 && self.cg_t == 0.0 {
                // Choke complete: go idle at exactly this sample.
                self.finish_choke();
                for a in self.scratch[i + 1..m].iter_mut() {
                    *a = [0.0; 4];
                }
            }
        }
        if !ok {
            self.reset_all();
            return;
        }
        self.peak_acc = self.peak_acc.max(pk);

        if self.left > 0 {
            self.left -= m.min(self.left);
            if self.left == 0 {
                let nb = self.n_chunks * 4;
                for k in 0..nb {
                    self.amp[k] = self.tgt[k];
                    self.step[k] = 0.0;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsp::hz_to_cents;

    const SR: f32 = 48_000.0;

    fn ctl(f0: f32) -> VoiceControls {
        VoiceControls { cents: hz_to_cents(f0), ..blank_controls() }
    }

    fn note() -> NoteStart {
        blank_start()
    }

    /// Render `n` samples (a multiple of TICK) in tick-sized steps. `first`
    /// says the first tick was already set up by `start`.
    fn run(v: &mut Voice, c: &VoiceControls, n: usize, span: usize, first: bool) -> Vec<f32> {
        let mut out = vec![0.0; n];
        for t in 0..n / TICK {
            if t > 0 || !first {
                v.control(c, TICK);
            }
            for ch in out[t * TICK..(t + 1) * TICK].chunks_mut(span) {
                v.render(ch);
            }
        }
        out
    }

    fn play(c: &VoiceControls, s: &NoteStart, n: usize) -> (Voice, Vec<f32>) {
        let mut v = Voice::new(SR, 1);
        v.start(s, c, TICK);
        let out = run(&mut v, c, n, TICK, true);
        (v, out)
    }

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

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0, |m, v| m.max(v.abs()))
    }

    fn peak_hz(x: &[f32], lo: f32, hi: f32) -> f32 {
        let mut best = (0.0, lo);
        let mut f = lo;
        while f <= hi {
            let m = mag(x, f);
            if m > best.0 {
                best = (m, f);
            }
            f += 2.0;
        }
        best.1
    }

    #[test]
    fn harmonic_note_peaks_at_f0() {
        let (_, out) = play(&ctl(200.0), &note(), 9600 + 4800);
        let x = &out[4800..];
        let p = peak_hz(x, 60.0, 1200.0);
        assert!((p - 200.0).abs() < 6.0, "peak at {p}");
        assert!(mag(x, 400.0) > 0.0 && mag(x, 400.0) < mag(x, 200.0));
        let p = peak(&out);
        assert!(p > 0.15 && p < 0.9, "peak level {p}");
    }

    #[test]
    fn bar_mode_peaks_at_2_76() {
        let mut c = ctl(200.0);
        c.modal = 2.0;
        c.tilt = 0.0;
        let (_, out) = play(&c, &note(), 9600 + 4800);
        let x = &out[4800..];
        let bar = mag(x, 2.76 * 200.0);
        assert!(bar > 10.0 * mag(x, 400.0), "{bar} vs {}", mag(x, 400.0));
        assert!(bar > 10.0 * mag(x, 600.0));
        let p = peak_hz(x, 450.0, 700.0);
        assert!((p / 200.0 - 2.76).abs() < 0.03, "ratio {}", p / 200.0);
    }

    #[test]
    fn budget_cut_removes_highs_not_the_fundamental() {
        let mut full = ctl(200.0);
        full.tilt = 0.0;
        let mut cut = full;
        cut.max_partials = 4;
        let (v1, a) = play(&full, &note(), 9600 + 4800);
        let (v2, b) = play(&cut, &note(), 9600 + 4800);
        let (a, b) = (&a[4800..], &b[4800..]);
        let (f1, f2) = (mag(a, 200.0), mag(b, 200.0));
        assert!((f1 - f2).abs() < 0.02 * f1, "{f1} vs {f2}");
        let hi = |x: &[f32]| (5..30).map(|k| mag(x, 200.0 * k as f32).powi(2)).sum::<f32>();
        assert!(hi(b) < 0.01 * hi(a), "{} vs {}", hi(b), hi(a));
        assert_eq!(v1.wanted_partials(), v2.wanted_partials());
        assert_eq!(v2.active_partials(), 4);
        assert!(v1.active_partials() > 60);
    }

    #[test]
    fn damp_kills_high_partials_first() {
        let f0 = 480.0;
        let mut c = ctl(f0);
        c.damp = 1.0;
        c.tilt = 0.0;
        let mut s = note();
        s.decay = 0.5;
        s.sustain = 0.0;
        s.attack = 0.001;
        let (_, out) = play(&c, &s, 4800 * 4);
        let w1 = &out[480..480 + 2400];
        let w2 = &out[480 + 2400..480 + 4800];
        let r = |f: f32| mag(w2, f) / mag(w1, f);
        // Fundamental: time constant decay. 50 ms apart: exp(-0.05/0.5).
        let want1 = (-0.05f32 / 0.5).exp();
        assert!((r(f0) - want1).abs() < 0.04, "fundamental {} want {want1}", r(f0));
        // Partial 4: time constant decay / 16.
        let want4 = (-0.05f32 * 16.0 / 0.5).exp();
        assert!((r(4.0 * f0) - want4).abs() < 0.05, "partial 4 {} want {want4}", r(4.0 * f0));
        // Partial 2: decay / 4.
        let want2 = (-0.05f32 * 4.0 / 0.5).exp();
        assert!((r(2.0 * f0) - want2).abs() < 0.05, "partial 2 {} want {want2}", r(2.0 * f0));
    }

    #[test]
    fn pitch_envelope_starts_high_and_falls() {
        let mut c = ctl(200.0);
        c.max_partials = 1;
        let mut s = note();
        s.pitch_amt = 12.0;
        s.pitch_time = 0.05;
        let (v, out) = play(&c, &s, 4800 * 3);
        assert!((v.pitch_cents() - c.cents).abs() < 5.0, "settled {}", v.pitch_cents() - c.cents);
        let mut v = Voice::new(SR, 1);
        v.start(&s, &c, TICK);
        assert!(v.pitch_cents() - c.cents > 1190.0);
        // Rising zero crossings per 10 ms: 2 at 200 Hz, about 4 an octave up.
        let zc = |x: &[f32]| x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        let early = zc(&out[96..576]);
        let late = zc(&out[out.len() - 480..]);
        assert!(early >= 3 && late <= 2 && early > late, "{early} vs {late}");
    }

    #[test]
    fn release_decays_and_goes_idle() {
        let c = ctl(200.0);
        let mut s = note();
        s.release = 0.1;
        let mut v = Voice::new(SR, 1);
        v.start(&s, &c, TICK);
        let before = run(&mut v, &c, 9600, TICK, true);
        assert!(!v.released());
        v.release();
        assert!(v.released());
        let after = run(&mut v, &c, 48_000 * 2, TICK, false);
        assert!(rms(&after[19_200..19_200 + 4800]) < 0.06 * rms(&before[4800..]));
        assert!(!v.active());
        let mut x = [0.0f32; 64];
        v.render(&mut x);
        assert!(x.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn choke_silences_within_3_ms() {
        let mut c = ctl(200.0);
        c.comb_amt = 0.5;
        c.comb_decay = 2.0;
        c.filt_cutoff = 3000.0;
        let mut v = Voice::new(SR, 1);
        v.start(&note(), &c, TICK);
        let a = run(&mut v, &c, 9600, TICK, true);
        assert!(rms(&a[4800..]) > 0.05);
        v.choke();
        let b = run(&mut v, &c, 960, TICK, false);
        let n3 = (0.003 * SR) as usize;
        assert!(peak(&b[..n3 / 2]) > 0.01);
        assert!(peak(&b[n3 + 2..]) < 1e-6, "tail {}", peak(&b[n3 + 2..]));
        assert!(!v.active());
    }

    #[test]
    fn end_stop_chokes_after_hold_and_fires_the_end_click() {
        let c = ctl(200.0);
        let mut s = note();
        s.attack = 0.001;
        s.hold = 0.05;
        s.end_stop = true;
        s.end_click = 1.0;
        let mut v = Voice::new(SR, 1);
        v.start(&s, &c, TICK);
        let a = run(&mut v, &c, 1920, TICK, true);
        assert!(rms(&a[500..]) > 0.05);
        // Hold ends at about sample 2450; ten milliseconds later all is quiet.
        let b = run(&mut v, &c, 4800, TICK, false);
        assert!(!v.active());
        assert!(peak(&b[1000..]) < 1e-6);
        // The end click shows up as extra signal compared with no click.
        let mut s0 = s;
        s0.end_click = 0.0;
        let mut v0 = Voice::new(SR, 1);
        v0.start(&s0, &c, TICK);
        run(&mut v0, &c, 1920, TICK, true);
        let b0 = run(&mut v0, &c, 4800, TICK, false);
        let diff: f32 = b.iter().zip(&b0).map(|(x, y)| (x - y).abs()).sum();
        assert!(diff > 0.5, "click energy {diff}");
    }

    #[test]
    fn restart_while_sounding_has_no_jump() {
        let mut c = ctl(200.0);
        c.damp = 0.5;
        let mut s = note();
        s.decay = 0.3;
        s.sustain = 0.2;
        let mut v = Voice::new(SR, 1);
        v.start(&s, &c, TICK);
        let a = run(&mut v, &c, 9600, TICK, true);
        let step = |x: &[f32]| x.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        let normal = step(&a[..2400]);
        v.start(&s, &c, TICK);
        let b = run(&mut v, &c, 4800, TICK, true);
        let joined: Vec<f32> = a[a.len() - 8..].iter().chain(b.iter()).copied().collect();
        let worst = step(&joined);
        assert!(worst <= 1.25 * normal, "restart step {worst} vs normal {normal}");
        // The note does restart: level comes back up.
        assert!(rms(&b[1000..2000]) > 1.3 * rms(&a[a.len() - 1000..]));
    }

    fn busy_controls() -> VoiceControls {
        let mut c = ctl(150.0);
        c.noise_mix = 0.3;
        c.tilt = -3.0;
        c.oddeven = 0.4;
        c.stretch = 0.0002;
        c.jitter = 8.0;
        c.modal = 0.7;
        c.damp = 0.6;
        c.f1_gain = 9.0;
        c.f2_gain = -6.0;
        c.tone_bands = [0.0, 3.0, -2.0, 4.0, 0.0, -6.0, 2.0, 0.0, 1.0, -3.0, 2.0, 0.0, -1.0, 0.0, 3.0, 0.0];
        c.fold_amt = 0.3;
        c.fold_bias = 0.2;
        c.clang_amt = 0.4;
        c.comb_amt = 0.35;
        c.comb_decay = 0.3;
        c.filt_cutoff = 2500.0;
        c.filt_res = 0.5;
        c.filt_morph = 0.3;
        c.filt_track = 0.5;
        c.lfo1_rate = 5.0;
        c.lfo1_pitch = 30.0;
        c.lfo1_filter = 0.5;
        c.lfo1_level = 0.3;
        c.lfo2_rate = 0.7;
        c.lfo2_pitch = 20.0;
        c.chaos_rate = 3.0;
        c.chaos_amt = 0.4;
        c.max_partials = 40;
        c
    }

    fn drive(span: usize) -> Vec<f32> {
        let c = busy_controls();
        let mut s = note();
        s.click = 0.5;
        s.end_click = 0.4;
        s.pitch_amt = 5.0;
        s.decay = 0.4;
        s.sustain = 0.3;
        let n = 4096;
        let mut v = Voice::new(SR, 77);
        let mut out = vec![0.0; n];
        let mut pos = 0;
        while pos < n {
            if pos == 0 {
                v.start(&s, &c, TICK);
            } else if pos == 500 {
                v.start(&s, &c, 12);
            } else if pos % TICK == 0 {
                v.control(&c, TICK);
            }
            if pos == 1500 {
                v.release();
            }
            let mut end = (pos / TICK + 1) * TICK;
            for e in [500, 1500] {
                if pos < e && e < end {
                    end = e;
                }
            }
            end = end.min(n);
            while pos < end {
                let m = span.min(end - pos);
                v.render(&mut out[pos..pos + m]);
                pos += m;
            }
        }
        out
    }

    #[test]
    fn output_does_not_depend_on_how_spans_are_split() {
        let a = drive(32);
        assert!(rms(&a) > 0.02 && a.iter().all(|v| v.is_finite()));
        for span in [1, 7, 32, 100] {
            let b = drive(span);
            assert!(a.iter().zip(&b).all(|(x, y)| x.to_bits() == y.to_bits()), "span {span} differs");
        }
        // Same seed, same calls: identical; another seed differs.
        assert_eq!(a, drive(32));
    }

    #[test]
    fn extreme_controls_stay_finite_and_bounded() {
        let mut extremes = Vec::new();
        let mut hi = ctl(200.0);
        hi.cents = 1e9;
        hi.amp = 1e9;
        hi.harm_mix = 1e9;
        hi.tilt = 1e9;
        hi.stretch = 1e9;
        hi.jitter = 1e9;
        hi.damp = 1e9;
        hi.fold_amt = 1e9;
        hi.clang_amt = 0.9;
        hi.clang_ratio = 1e9;
        hi.comb_amt = 1.0;
        hi.comb_tune = 1e9;
        hi.comb_decay = 1e9;
        hi.filt_cutoff = 1e9;
        hi.filt_res = 1e9;
        hi.filt_morph = 0.5;
        hi.lfo1_rate = 1e9;
        hi.lfo1_pitch = 1e9;
        hi.lfo1_filter = 1e9;
        hi.chaos_rate = 1e9;
        hi.chaos_amt = 1e9;
        hi.max_partials = usize::MAX;
        extremes.push(hi);
        let mut lo = ctl(200.0);
        lo.cents = -1e9;
        lo.stretch = -1e9;
        lo.tilt = -1e9;
        lo.comb_decay = -5.0;
        lo.comb_amt = 1.0;
        lo.filt_cutoff = -1e9;
        lo.f1_freq = -1.0;
        lo.f1_gain = 1e9;
        lo.f2_gain = -1e9;
        lo.tone_bands = [1e9; N_BANDS];
        lo.clang_amt = 1.0;
        lo.max_partials = 0;
        extremes.push(lo);
        let mut nan = ctl(200.0);
        nan.cents = f32::NAN;
        nan.amp = f32::INFINITY;
        nan.tilt = f32::NAN;
        nan.stretch = f32::NAN;
        nan.comb_amt = f32::NAN;
        nan.filt_res = f32::NEG_INFINITY;
        nan.tone_bands = [f32::NAN; N_BANDS];
        nan.fold_amt = f32::NAN;
        extremes.push(nan);
        let mut dense = busy_controls();
        dense.fold_amt = 1.0;
        dense.clang_amt = 1.0;
        dense.comb_amt = 1.0;
        dense.comb_decay = 20.0;
        dense.filt_res = 1.0;
        dense.filt_cutoff = 200.0;
        extremes.push(dense);
        for (i, c) in extremes.iter().enumerate() {
            let mut s = note();
            s.velocity = 1e9;
            s.attack = 0.0;
            s.decay = 0.0;
            s.sustain = 1e9;
            s.pitch_amt = 1e9;
            s.pitch_time = 0.0;
            s.click = 1e9;
            let mut v = Voice::new(SR, 5);
            v.start(&s, c, TICK);
            let out = run(&mut v, c, 24_000, TICK, true);
            assert!(out.iter().all(|x| x.is_finite() && x.abs() <= 32.0), "case {i}");
            v.release();
            let out = run(&mut v, c, 4800, TICK, false);
            assert!(out.iter().all(|x| x.is_finite() && x.abs() <= 32.0), "case {i}");
        }
    }

    #[test]
    fn partials_above_0_45_sr_are_skipped() {
        let (v, out) = play(&ctl(10_000.0), &note(), 4800);
        // 10 kHz and 20 kHz are under 21.6 kHz; 30 kHz is not.
        assert_eq!(v.wanted_partials(), 2);
        assert!(out.iter().all(|x| x.is_finite()));
        let (v, _) = play(&ctl(5000.0), &note(), 320);
        assert_eq!(v.wanted_partials(), 4);
        let (v, _) = play(&ctl(100.0), &note(), 320);
        assert!(v.wanted_partials() <= 64 && v.wanted_partials() > 40);
    }

    #[test]
    fn idle_voice_is_inactive_and_free() {
        let mut v = Voice::new(SR, 3);
        assert!(!v.active());
        let c = ctl(200.0);
        v.control(&c, TICK);
        let mut x = [0.0f32; 32];
        v.render(&mut x);
        assert!(x.iter().all(|&s| s == 0.0));
        assert!(!v.active());
        let mut s = note();
        s.decay = 0.05;
        s.sustain = 0.0;
        v.start(&s, &c, TICK);
        assert!(v.active());
        run(&mut v, &c, 48_000, TICK, true);
        assert!(!v.active());
        assert_eq!(v.noise_level(), 0.0);
        v.start(&note(), &c, TICK);
        assert!(v.active());
        v.kill();
        assert!(!v.active());
    }

    #[test]
    fn comb_tail_keeps_the_voice_active_until_it_is_quiet() {
        let mut c = ctl(200.0);
        c.comb_amt = 1.0;
        c.comb_decay = 1.0;
        let mut s = note();
        s.sustain = 0.0;
        s.decay = 0.02;
        let mut v = Voice::new(SR, 1);
        v.start(&s, &c, TICK);
        run(&mut v, &c, 4800, TICK, true);
        // Envelope is long gone but the comb still rings.
        assert!(v.active());
        let tail = run(&mut v, &c, 9600, TICK, false);
        assert!(rms(&tail) > 1e-4);
        run(&mut v, &c, 48_000 * 2, TICK, false);
        assert!(!v.active());
    }

    #[test]
    fn modulators_and_chain_do_something() {
        // Vibrato moves the pitch; filter cutoff lowers highs; fold adds highs.
        let mut c = ctl(200.0);
        c.lfo1_rate = 5.0;
        c.lfo1_pitch = 100.0;
        let mut v = Voice::new(SR, 1);
        v.start(&note(), &c, TICK);
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for _ in 0..300 {
            v.control(&c, TICK);
            lo = lo.min(v.pitch_cents() - c.cents);
            hi = hi.max(v.pitch_cents() - c.cents);
        }
        assert!(hi > 95.0 && lo < -95.0, "{lo} {hi}");

        let base = ctl(200.0);
        let mut dark = base;
        dark.filt_cutoff = 500.0;
        let (_, a) = play(&base, &note(), 9600);
        let (_, b) = play(&dark, &note(), 9600);
        let hf = |x: &[f32]| (6..20).map(|k| mag(&x[2400..], 200.0 * k as f32).powi(2)).sum::<f32>();
        assert!(hf(&b) < 0.05 * hf(&a));

        let mut sine = ctl(200.0);
        sine.max_partials = 1;
        let mut folded = sine;
        folded.fold_amt = 0.8;
        let (_, a) = play(&sine, &note(), 9600);
        let (_, b) = play(&folded, &note(), 9600);
        assert!(mag(&b[2400..], 600.0) > 20.0 * mag(&a[2400..], 600.0));

        // LFO 1 level modulates the amplitude.
        let mut trem = ctl(200.0);
        trem.lfo1_rate = 10.0;
        trem.lfo1_level = 1.0;
        let (_, t) = play(&trem, &note(), 24_000);
        let w = |i: usize| rms(&t[i * 480..(i + 1) * 480]);
        let (mn, mx) = (10..50).fold((f32::MAX, 0.0f32), |(a, b), i| (a.min(w(i)), b.max(w(i))));
        assert!(mn < 0.3 * mx, "{mn} {mx}");
    }

    #[test]
    fn start_click_and_noise_level() {
        let mut c = ctl(200.0);
        c.noise_mix = 0.5;
        let mut s = note();
        s.click = 1.0;
        s.noise_decay = 0.1;
        let mut v = Voice::new(SR, 1);
        v.start(&s, &c, TICK);
        let out = run(&mut v, &c, 480, TICK, true);
        let early = peak(&out[..200]);
        assert!(early > 0.3, "click peak {early}");
        assert!((v.noise_level() - 0.5).abs() < 0.05, "{}", v.noise_level());
        run(&mut v, &c, 9600, TICK, false);
        assert!(v.noise_level() < 0.5 * (-2.0f32).exp() * 1.5);
    }

    #[test]
    #[ignore]
    fn bench_ns_per_voice_sample() {
        let mut c = ctl(110.0);
        c.max_partials = 64;
        c.tilt = -3.0;
        let mut vib = c;
        vib.lfo1_rate = 5.0;
        vib.lfo1_pitch = 40.0;
        for (name, cc) in [("static pitch", c), ("vibrato", vib)] {
            let n_voices = 16;
            let mut voices: Vec<Voice> = (0..n_voices).map(|i| Voice::new(SR, i as u32 + 1)).collect();
            let s = note();
            for v in voices.iter_mut() {
                v.start(&s, &cc, TICK);
            }
            let ticks = 3000;
            let mut out = [0.0f32; TICK];
            let t0 = std::time::Instant::now();
            let mut sink = 0.0;
            for t in 0..ticks {
                for v in voices.iter_mut() {
                    if t > 0 {
                        v.control(&cc, TICK);
                    }
                    v.render(&mut out);
                }
                sink += out[0];
            }
            let ns = t0.elapsed().as_nanos() as f64 / (ticks * TICK * n_voices) as f64;
            println!("{name}: {ns:.1} ns per voice-sample (64 partials), sink {sink}");
            println!(
                "  8 layers x 8 voices real time load: {:.1}% of one core at 48 kHz",
                ns * 64.0 * 48_000.0 / 1e9 * 100.0
            );
        }
    }
}
