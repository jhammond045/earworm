//! Live follow (M2): the live tier, tuner and the rules that turn a sung
//! voice into a held voice (melody, texture) or a hit (beat) on the armed
//! layer. The engine owns one `LiveCapture`; it runs inside `process` only.
//!
//! Pitch follows requirement 7: a voice that opens before a pitch is known
//! sounds as noise (or at the last pitch), and glides 10 ms to the first
//! estimate. The frame record the live voice reads every control tick is
//! built here and goes through the same `controls()` and `gesture_offsets`
//! path as a stored take (TDD decision 4).

use crate::curve::{CURVE_STRIDE, TUNED};
use crate::protocol::*;
use crate::voice::{N_BANDS, TICK};
use dsp::capture::{FrameSink, LiveTier};
use dsp::frame::{self, silent_frame, MAX_FRAME_LEN};
use dsp::tune::{Slides, TuneSettings, Tuner, SCALES};

/// Tuner step: one per YIN hop, 5 ms.
const TUNER_DT: f32 = 0.005;
/// Pitch smoothing in live mode (the tuner already smooths), seconds.
const PITCH_TAU_S: f32 = 0.002;
const GLIDE_S: f32 = 0.010;
const GLIDE_TAU_S: f32 = 0.0025;
/// Recent live hits the adaptive brightness range looks at.
const BRIGHT_WINDOW: usize = 12;
/// Onsets that arrive just before the gate opens wait this many calls for it.
const PENDING_CALLS: u32 = 4;

/// What the engine should do with the armed layer this call.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    None,
    /// Start the live voice or a hit (a start releases the previous one).
    Start { slot: u8, velocity: f32, hard: f32 },
    Release,
}

pub struct LiveCapture {
    sr: f32,
    tier: LiveTier,
    sink: FrameSink,
    tuner: Tuner,
    /// Frame records of the last call, CURVE_STRIDE floats each.
    pub frames: Vec<f32>,
    pub frames_count: u32,
    pub frames_cap: usize,

    tune: [[f32; TUNE_FIELDS]; MAX_LAYERS],
    taught: [[bool; KIT_SLOTS]; MAX_LAYERS],
    centre: [[[f32; 3]; KIT_SLOTS]; MAX_LAYERS],
    /// Armed layer, -1 none.
    pub armed: i32,

    /// Last tuner output (no octave), NaN until a pitch is known.
    tuned: f32,
    gate_prev: bool,
    env_prev: f32,
    env_now: f32,
    pending: Option<(f32, f32, u32)>,
    have_pitch: bool,
    last_cents: f32,
    pitch_cur: f32,
    pitch_fresh: bool,
    glide_left: f32,
    /// Voicing the live voice is told about (a voiced frame with no pitch
    /// yet reads as unvoiced, so the voice is noise until the pitch lands).
    voicing_target: f32,
    /// Latest block values the record is made from.
    rec: [f32; CURVE_STRIDE],

    bright: [f32; BRIGHT_WINDOW],
    n_bright: usize,
    pub hits: u32,
    pub last_slot: i32,
}

fn tune_defaults() -> [f32; TUNE_FIELDS] {
    let mut t = [0.0; TUNE_FIELDS];
    t[TUNE_SPEED as usize] = 60.0;
    t[TUNE_VIBRATO as usize] = 1.0;
    t
}

fn settings(t: &[f32; TUNE_FIELDS]) -> TuneSettings {
    let scale = (t[TUNE_SCALE as usize].round().max(0.0) as usize).min(SCALES.len() - 1);
    TuneSettings {
        snap: t[TUNE_SNAP as usize],
        speed_ms: t[TUNE_SPEED as usize],
        slides: Slides::from_index(t[TUNE_SLIDES as usize]),
        vibrato: t[TUNE_VIBRATO as usize],
        root: (t[TUNE_ROOT as usize].round() as i32).rem_euclid(12) as u8,
        mask: SCALES[scale].1,
        ref_cents: t[TUNE_REF_CENTS as usize],
    }
}

fn sane(x: f32, lo: f32, hi: f32) -> f32 {
    if x.is_finite() { x.clamp(lo, hi) } else { 0.0f32.clamp(lo, hi) }
}

impl LiveCapture {
    /// Frames one call can make at most (the tier emits one per sr/100 samples).
    pub fn frames_cap_for(sr: f32, max_block: usize) -> usize {
        (max_block as f32 / (sr / 100.0)).ceil() as usize + 2
    }

    pub fn new(sr: f32, max_block: usize) -> Self {
        let cap = Self::frames_cap_for(sr, max_block);
        let mut rec = [0.0; CURVE_STRIDE];
        rec[frame::PITCH] = f32::NAN;
        rec[frame::VOICING] = frame::UNVOICED;
        rec[TUNED] = f32::NAN;
        let mut c = LiveCapture {
            sr,
            tier: LiveTier::new(sr, N_BANDS),
            sink: FrameSink::with_capacity(cap + 2),
            tuner: Tuner::new(TUNER_DT),
            frames: vec![0.0; cap * CURVE_STRIDE],
            frames_count: 0,
            frames_cap: cap,
            tune: [tune_defaults(); MAX_LAYERS],
            taught: [[false; KIT_SLOTS]; MAX_LAYERS],
            centre: [[[0.5, 0.5, 0.0]; KIT_SLOTS]; MAX_LAYERS],
            armed: -1,
            tuned: f32::NAN,
            gate_prev: false,
            env_prev: 0.0,
            env_now: 0.0,
            pending: None,
            have_pitch: false,
            last_cents: f32::NAN,
            pitch_cur: 5700.0,
            pitch_fresh: false,
            glide_left: 0.0,
            voicing_target: frame::UNVOICED,
            rec,
            bright: [0.5; BRIGHT_WINDOW],
            n_bright: 0,
            hits: 0,
            last_slot: -1,
        };
        c.apply_tune();
        c
    }

    pub fn tier(&self) -> &LiveTier {
        &self.tier
    }

    fn layer_for_tune(&self) -> usize {
        if self.armed >= 0 { self.armed as usize } else { 0 }
    }

    fn apply_tune(&mut self) {
        let s = settings(&self.tune[self.layer_for_tune()]);
        self.tuner.set(s);
    }

    fn octave(&self) -> f32 {
        let t = &self.tune[self.layer_for_tune()];
        t[TUNE_OCTAVE as usize].round().clamp(-2.0, 2.0) * 1200.0
    }

    pub fn set_tune(&mut self, layer: usize, field: u16, v: f32) {
        if layer >= MAX_LAYERS || (field as usize) >= TUNE_FIELDS || !v.is_finite() {
            return;
        }
        self.tune[layer][field as usize] = v;
        if layer == self.layer_for_tune() {
            self.apply_tune();
        }
    }

    pub fn set_kit(&mut self, layer: usize, slot: usize, field: u16, v: f32) {
        if layer >= MAX_LAYERS || slot >= KIT_SLOTS || !v.is_finite() {
            return;
        }
        match field {
            LIVE_KIT_TAUGHT => self.taught[layer][slot] = v > 0.5,
            LIVE_KIT_BRIGHT..=LIVE_KIT_NOISE => self.centre[layer][slot][(field - LIVE_KIT_BRIGHT) as usize] = v,
            _ => {}
        }
    }

    /// Arm `layer` (or -1). Forgets the follow state so a gate that is
    /// already open starts the new layer's voice at once.
    pub fn arm(&mut self, layer: i32) {
        self.armed = layer;
        self.gate_prev = false;
        self.pending = None;
        self.tuner.reset();
        self.tuned = f32::NAN;
        self.have_pitch = false;
        self.last_cents = f32::NAN;
        self.glide_left = 0.0;
        self.pitch_fresh = false;
        self.apply_tune();
    }

    pub fn reset_singer(&mut self) {
        self.tier.reset_singer();
    }

    /// Run the tier over `input` (high-passed in place), run the tuner, write
    /// the frame records, and decide what the armed layer does.
    pub fn process(&mut self, input: &mut [f32], beat: bool) -> Action {
        self.sink.clear();
        self.tier.process(input, &mut self.sink);
        let g = self.tier.gesture();
        for &(cents, conf) in &g.pitch_updates[..g.n_pitch_updates] {
            self.tuned = self.tuner.process(cents, conf);
        }

        let n = self.sink.frames().len().min(self.frames_cap);
        for (i, f) in self.sink.frames()[..n].iter().enumerate() {
            let rec = &mut self.frames[i * CURVE_STRIDE..(i + 1) * CURVE_STRIDE];
            rec[..frame::HEADER + N_BANDS].copy_from_slice(&f[..frame::HEADER + N_BANDS]);
            rec[TUNED] = if f[frame::VOICING] == frame::VOICED { self.tuned } else { f32::NAN };
        }
        self.frames_count = n as u32;

        let g = self.tier.gesture().clone();
        self.env_prev = self.env_now;
        self.env_now = sane(g.gate_env, 0.0, 1.0);

        let action = if self.armed < 0 { Action::None } else { self.follow(&g, beat) };
        self.gate_prev = g.gate && self.armed >= 0;

        // Block-level part of the record.
        let r = &mut self.rec;
        r[frame::PITCH] = g.pitch_cents;
        r[frame::CONFIDENCE] = g.pitch_conf;
        r[frame::LOUDNESS] = sane(g.loudness, 0.0, 1.0);
        r[frame::OPENNESS] = sane(g.openness, 0.0, 1.0);
        r[frame::BREATHINESS] = sane(g.breathiness, 0.0, 1.0);
        r[frame::ONSET] = sane(g.onset, 0.0, 1.0);
        r[frame::HARDNESS] = sane(g.hardness, 0.0, 1.0);
        r[frame::BANDS..frame::BANDS + N_BANDS].copy_from_slice(&g.bands_db[..N_BANDS]);
        action
    }

    /// Pitch bookkeeping every call; start and release decisions.
    fn follow(&mut self, g: &dsp::capture::Gesture, beat: bool) -> Action {
        let octave = self.octave();
        let t = if self.tuned.is_finite() { self.tuned + octave } else { f32::NAN };
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
        if g.voicing < 0.5 {
            self.voicing_target = if self.have_pitch { frame::VOICED } else { frame::UNVOICED };
        } else if g.voicing < 1.5 {
            self.voicing_target = frame::UNVOICED;
        }

        let rising = g.gate && !self.gate_prev;
        let hard = sane(g.hardness, 0.0, 1.0);
        if beat {
            let mut fire = None;
            if g.onset > 0.0 {
                if g.gate {
                    fire = Some((g.onset, hard));
                } else {
                    self.pending = Some((g.onset, hard, PENDING_CALLS));
                }
            } else if let Some((s, h, ttl)) = self.pending {
                if g.gate {
                    fire = Some((s, h));
                    self.pending = None;
                } else if ttl <= 1 {
                    self.pending = None;
                } else {
                    self.pending = Some((s, h, ttl - 1));
                }
            }
            if let Some((strength, h)) = fire {
                self.pending = None;
                let slot = self.choose_slot(g);
                let loud = sane(g.loudness, 0.0, 1.0);
                let velocity = (0.25 + 0.75 * (0.6 * strength + 0.4 * loud)).clamp(0.2, 1.0);
                self.hits = self.hits.wrapping_add(1);
                self.last_slot = slot as i32;
                return Action::Start { slot: slot as u8, velocity, hard: h };
            }
            if !g.gate && self.gate_prev {
                return Action::Release;
            }
            return Action::None;
        }
        if rising || (g.gate && g.onset > 0.0) {
            return Action::Start { slot: 0, velocity: 1.0, hard };
        }
        if !g.gate && self.gate_prev {
            return Action::Release;
        }
        Action::None
    }

    /// Kit slot of a live hit: the nearest taught centre in the three live
    /// hit features, else brightness thirds over the recent range.
    fn choose_slot(&mut self, g: &dsp::capture::Gesture) -> usize {
        let mut f = silent_frame();
        f[frame::BANDS..frame::BANDS + N_BANDS].copy_from_slice(&g.bands_db[..N_BANDS]);
        f[frame::BREATHINESS] = g.breathiness;
        let feat = frame::live_hit_features(&f[..MAX_FRAME_LEN], N_BANDS);
        let layer = self.armed.max(0) as usize;

        let mut best: Option<(f32, usize)> = None;
        for k in 0..KIT_SLOTS {
            if !self.taught[layer][k] {
                continue;
            }
            let c = &self.centre[layer][k];
            let d: f32 = (0..3).map(|i| (feat[i] - c[i]) * (feat[i] - c[i])).sum();
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, k));
            }
        }
        // The window takes every hit so the range keeps following the singer.
        let b = feat[0];
        let slot = if let Some((_, k)) = best {
            k
        } else {
            let (lo, hi) = self.bright_range();
            (((b - lo) / (hi - lo) * 3.0).floor() as i32).clamp(0, 2) as usize
        };
        self.bright[self.n_bright % BRIGHT_WINDOW] = b;
        self.n_bright += 1;
        slot
    }

    /// Brightness range for the thirds: starts at 0.3..0.7 and moves to the
    /// range of the recent hits as they come in.
    fn bright_range(&self) -> (f32, f32) {
        let n = self.n_bright.min(BRIGHT_WINDOW);
        let w = n.min(8) as f32 / 8.0;
        let (mut mn, mut mx) = (1.0f32, 0.0f32);
        for &b in &self.bright[..n] {
            mn = mn.min(b);
            mx = mx.max(b);
        }
        let (mut lo, mut hi) = if n == 0 { (0.3, 0.7) } else { (0.3 + w * (mn - 0.3), 0.7 + w * (mx - 0.7)) };
        if hi - lo < 0.15 {
            let mid = 0.5 * (lo + hi);
            lo = mid - 0.075;
            hi = mid + 0.075;
        }
        (lo, hi)
    }

    /// One control tick passed (pitch smoothing for the live voice).
    pub fn advance_tick(&mut self) {
        if self.armed < 0 {
            return;
        }
        let dt = TICK as f32 / self.sr;
        let target = if self.last_cents.is_finite() { self.last_cents } else { return };
        let a = if self.glide_left > 0.0 {
            self.glide_left -= dt;
            1.0 - (-dt / GLIDE_TAU_S).exp()
        } else {
            1.0 - (-dt / PITCH_TAU_S).exp()
        };
        self.pitch_cur += a * (target - self.pitch_cur);
        if !self.pitch_cur.is_finite() {
            self.pitch_cur = 5700.0;
        }
    }

    /// The record the live voice reads now, and its amplitude scale. `at` is
    /// the position in the call in `0..=1`, for the gate envelope.
    pub fn record(&self, at: f32) -> ([f32; CURVE_STRIDE], f32) {
        let mut r = self.rec;
        r[frame::VOICING] = self.voicing_target;
        r[TUNED] = if self.have_pitch { self.pitch_cur } else { f32::NAN };
        let gain = self.env_prev + (self.env_now - self.env_prev) * at.clamp(0.0, 1.0);
        (r, gain)
    }

    /// The record a listening layer reads: the latest block's features with the
    /// tier's own voicing, and the tuned pitch (octave applied) while voiced.
    pub fn listen_record(&self) -> [f32; CURVE_STRIDE] {
        let mut r = self.rec;
        let g = self.tier.gesture();
        r[frame::VOICING] = g.voicing;
        r[TUNED] = if g.voicing < 0.5 { self.carrier() } else { f32::NAN };
        r
    }

    /// Tuned carrier pitch with the octave applied (meters), NaN unknown.
    pub fn carrier(&self) -> f32 {
        if self.tuned.is_finite() { self.tuned + self.octave() } else { f32::NAN }
    }

    pub fn write_meters(&self, m: &mut [f32]) {
        let g = self.tier.gesture();
        m[M_IN_DB] = g.input_db;
        m[M_FLOOR_DB] = g.floor_db;
        m[M_GATE] = g.gate as u8 as f32;
        m[M_PITCH] = g.pitch_cents;
        m[M_CONF] = g.pitch_conf;
        m[M_TUNED] = self.carrier();
        m[M_LOUD] = g.loudness;
        m[M_OPEN] = g.openness;
        m[M_BREATH] = g.breathiness;
        m[M_HARD] = g.hardness;
        m[M_VOICING] = g.voicing;
        m[M_ONSET] = g.onset;
        m[M_ARMED] = self.armed as f32;
        let d = self.tier.delays();
        m[M_DELAY_LEVEL] = d.level_s;
        m[M_DELAY_PITCH] = d.pitch_s;
        m[M_LIVE_SLOT] = self.last_slot as f32;
        m[M_LIVE_HITS] = self.hits as f32;
        m[M_VOWEL..M_VOWEL + N_BANDS].copy_from_slice(&g.bands_db[..N_BANDS]);
        m[M_INTENSITY] = if g.gate { crate::voice::intensity(&g.bands_db[..N_BANDS]) } else { f32::NAN };
    }
}
