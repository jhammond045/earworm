//! One layer: its stored sound (base, macros, Influence target, kit slots),
//! parameter resolution, up to eight voices, the shaped noise and the
//! per-layer effects.
//!
//! Resolution (TDD "Resolution order"), as built in M1:
//! 1. Morph: not yet (states and the morph lane arrive in M4).
//! 2. Influence: `params::influence_blend` on the layer's base set.
//! 3. Macros: `params::apply_macros` on every set.
//!    Steps 2 and 3 run when a stored value changes, then every set is
//!    smoothed per control tick with each parameter's `smooth_ms`, in the
//!    normalized domain, and mapped to real units.
//! 4. Voice start: a voice reads the layer set (melody, texture) or its kit
//!    slot's set (beat).
//! 5. Gesture: a voice playing a clip with a curve reads the take's frame
//!    each tick and offsets a few parameters through the fixed linear maps in
//!    `gesture_offsets`; Mouth blends the band shapes toward the frame.
//! 6. Hit shape: beat voices offset their slot's values by the hit's shape,
//!    scaled by Follow (`hit_start`).
//! 7. Modulators: inside the voice (LFOs, chaos).
//! 8. Smooth and map: smoothing is on the layer sets (step 3's output), not
//!    per voice; per-voice offsets come from 10 ms frames read with linear
//!    interpolation or are fixed for the note, so they need none.
//!
//! The space, crush and tape modules are per layer and always read the
//! layer's base set, also on beat layers.

use crate::curve::{self, CurvePool, CURVE_STRIDE, TUNED};
use crate::protocol::*;
use crate::seq::{Clip, Event, Track};
use crate::voice::{NoteStart, Voice, VoiceControls, N_BANDS, TICK};
use dsp::bands::{band_centres, band_q};
use dsp::filters::Svf;
use dsp::frame;
use dsp::layer_fx::{width, Crush, Reverb, StereoDelay, Tape};
use dsp::noise::Noise;
use params as P;
use params::ParamSet;

/// Sets per layer: the layer's own (index 0) and the four kit slots.
pub const SETS: usize = 1 + KIT_SLOTS;
/// Shaped-noise rms with flat bands before level, as in M0.
const NOISE_RMS: f32 = 0.35;
/// Voicing crossfade (TDD "Vowel shape: the vocoder path").
const VOICE_XFADE_S: f32 = 0.010;

/// What drives a voice besides its layer's set.
#[derive(Clone, Copy, Debug)]
struct Slot {
    set: usize,
    /// Pitch of the note in cents (hits: resolved slot or hit pitch).
    cents: f32,
    /// Live triggers: the cents value of the NOTE_ON, for NOTE_OFF matching.
    live_key: f32,
    /// Samples until note off; u32::MAX holds until a NOTE_OFF.
    left: u32,
    seq: bool,
    age: u64,
    velocity: f32,
    /// Curve slot and the clip whose warp map reads it (-1: none).
    curve: i32,
    clip: u32,
    /// Clip time of the voice now, and its end (reading holds after it).
    clip_t: f64,
    clip_end: f64,
    /// Seconds of take time per clip unit with no anchors (free sections: 1).
    default_slope: f64,
    /// Hit-shape and start offsets in the normalized domain, fixed for the note.
    off: Offsets,
    /// Voicing crossfade, 1 = partials, 0 = noise.
    vmix: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct Offsets {
    tilt: f32,
    f1: f32,
    harm: Option<f32>,
    noise: Option<f32>,
}

impl Default for Slot {
    fn default() -> Self {
        Slot {
            set: 0,
            cents: 4800.0,
            live_key: f32::NAN,
            left: 0,
            seq: false,
            age: 0,
            velocity: 1.0,
            curve: -1,
            clip: 0,
            clip_t: 0.0,
            clip_end: 0.0,
            default_slope: 1.0,
            off: Offsets::default(),
            vmix: 1.0,
        }
    }
}

/// Sixteen band-passed noise bands at the capture band centres.
struct ShapedNoise {
    bp: [Svf; N_BANDS],
    gain: [f32; N_BANDS],
    step: [f32; N_BANDS],
    target: [f32; N_BANDS],
    norm: f32,
    on: bool,
}

impl ShapedNoise {
    fn new(sr: f32) -> Self {
        let mut c = [0.0; frame::MAX_BANDS];
        band_centres(N_BANDS, &mut c);
        let q = band_q(N_BANDS);
        let mut bp = [Svf::default(); N_BANDS];
        let mut bw = 0.0;
        for i in 0..N_BANDS {
            bp[i] = Svf::new(sr, c[i], q);
            bw += c[i] / q;
        }
        let frac = (bw / (sr * 0.5)).clamp(0.05, 1.0);
        ShapedNoise {
            bp,
            gain: [0.0; N_BANDS],
            step: [0.0; N_BANDS],
            target: [0.0; N_BANDS],
            norm: NOISE_RMS / (0.577 * frac.sqrt()),
            on: false,
        }
    }

    /// Per tick: `level` is the summed noise level of the voices on this set,
    /// `bands_db` the set's noise bands in dB.
    fn control(&mut self, level: f32, bands_db: &[f32; N_BANDS], len: usize) {
        let mut any = false;
        let inv = 1.0 / len.max(1) as f32;
        for i in 0..N_BANDS {
            self.gain[i] = self.target[i];
            let t = dsp::db_to_lin(bands_db[i]) * level * self.norm;
            let t = if t.is_finite() { t } else { 0.0 };
            self.step[i] = (t - self.gain[i]) * inv;
            self.target[i] = t;
            any |= t > 1e-7 || self.gain[i] > 1e-7;
        }
        if !any && self.on {
            for f in self.bp.iter_mut() {
                f.reset();
            }
            self.gain = [0.0; N_BANDS];
        }
        self.on = any;
    }

    fn render(&mut self, noise: &mut Noise, out: &mut [f32]) {
        if !self.on {
            return;
        }
        for y in out.iter_mut() {
            let x = noise.next();
            let mut s = 0.0;
            for b in 0..N_BANDS {
                self.gain[b] += self.step[b];
                s += self.gain[b] * self.bp[b].bandpass_norm(x);
            }
            *y += s;
        }
    }
}

pub struct Layer {
    pub sr: f32,
    pub kind: u32,
    pub level_db: f32,
    pub pan: f32,
    pub mute: bool,
    pub depth: [f32; 5],
    pub follow: f32,
    pub infl_amount: f32,
    pub infl_mask: u32,
    pub kit_cents: [f32; KIT_SLOTS],
    pub base: ParamSet,
    pub macros: [f32; N_MACROS],
    pub influence: ParamSet,
    pub kit: [ParamSet; KIT_SLOTS],

    /// Steps 2-3 output per set, smoothed copy, and real units of the smoothed copy.
    target: [ParamSet; SETS],
    norm: [ParamSet; SETS],
    real: [ParamSet; SETS],
    settled: [bool; SETS],
    dirty: bool,
    smooth: ParamSet,

    pub voices: [Voice; VOICES_PER_LAYER],
    slots: [Slot; VOICES_PER_LAYER],
    noise_banks: [ShapedNoise; SETS],
    noise: Noise,
    crush: Crush,
    tape: Tape,
    delay: StereoDelay,
    reverb: Reverb,
    bus: [f32; TICK],
    gl: f32,
    gr: f32,
    gl_t: f32,
    gr_t: f32,
    grit_on: bool,
    pub peak: f32,
    pub steals: u32,
    /// Partial budget share per voice, set by the engine before each tick.
    pub max_partials: [usize; VOICES_PER_LAYER],
    age: u64,
    /// Normalized values after step 5 for the loudest voice (meters).
    pub live: ParamSet,
}

impl Layer {
    pub fn new(sr: f32, index: usize) -> Self {
        let seed = 0x9E37_79B9u32.wrapping_mul(index as u32 + 1);
        let neutral = P::neutral_set();
        let dt = TICK as f32 / sr;
        let mut smooth = [1.0; P::COUNT];
        for (i, d) in P::DESCS.iter().enumerate() {
            if d.smooth_ms > 0.0 {
                smooth[i] = 1.0 - (-dt * 1000.0 / d.smooth_ms).exp();
            }
        }
        let mut l = Layer {
            sr,
            kind: KIND_MELODY,
            level_db: 0.0,
            pan: 0.0,
            mute: false,
            depth: [1.0, 0.5, 0.5, 0.5, 1.0],
            follow: 1.0,
            infl_amount: 0.0,
            infl_mask: 0x1f,
            kit_cents: [2400.0, 4800.0, 6000.0, 7200.0],
            base: neutral,
            macros: [0.5; N_MACROS],
            influence: neutral,
            kit: [neutral; KIT_SLOTS],
            target: [neutral; SETS],
            norm: [neutral; SETS],
            real: [[0.0; P::COUNT]; SETS],
            settled: [false; SETS],
            dirty: true,
            smooth,
            voices: std::array::from_fn(|v| Voice::new(sr, seed ^ (v as u32 + 1).wrapping_mul(0x85EB_CA6B))),
            slots: [Slot::default(); VOICES_PER_LAYER],
            noise_banks: std::array::from_fn(|_| ShapedNoise::new(sr)),
            noise: Noise::new(seed ^ 0x5bd1_e995),
            crush: Crush::new(sr, seed ^ 0x27d4_eb2f),
            tape: Tape::new(sr, seed ^ 0x1656_67b1),
            delay: StereoDelay::new(sr),
            reverb: Reverb::new(sr, seed ^ 0xc2b2_ae35),
            bus: [0.0; TICK],
            gl: 0.0,
            gr: 0.0,
            gl_t: 0.0,
            gr_t: 0.0,
            grit_on: false,
            peak: 0.0,
            steals: 0,
            max_partials: [crate::voice::MAX_PARTIALS; VOICES_PER_LAYER],
            age: 0,
            live: neutral,
        };
        l.resolve();
        for s in 0..SETS {
            l.norm[s] = l.target[s];
            l.map_set(s);
        }
        l
    }

    // ------------------------------------------------------------ stored sound

    pub fn set_param(&mut self, set: u8, i: usize, v: f32) {
        if i >= P::COUNT || !v.is_finite() {
            return;
        }
        let v = v.clamp(0.0, 1.0);
        match set {
            SET_BASE => self.base[i] = v,
            SET_INFLUENCE => self.influence[i] = v,
            s if (SET_KIT..SET_KIT + KIT_SLOTS as u8).contains(&s) => self.kit[(s - SET_KIT) as usize][i] = v,
            _ => return,
        }
        self.dirty = true;
    }

    pub fn set_macro(&mut self, k: usize, v: f32) {
        if k < N_MACROS && v.is_finite() {
            self.macros[k] = v.clamp(0.0, 1.0);
            self.dirty = true;
        }
    }

    pub fn set_field(&mut self, field: u16, v: f32) {
        if !v.is_finite() {
            return;
        }
        match field {
            LAYER_LEVEL => self.level_db = v.clamp(-60.0, 12.0),
            LAYER_PAN => self.pan = v.clamp(-1.0, 1.0),
            LAYER_MUTE => {
                self.mute = v > 0.5;
                if self.mute {
                    self.release_all();
                }
            }
            LAYER_KIND => self.kind = (v.round() as u32).min(KIND_TEXTURE),
            LAYER_DEPTH_LOUD..=LAYER_DEPTH_MOUTH => self.depth[(field - LAYER_DEPTH_LOUD) as usize] = v.clamp(0.0, 1.0),
            LAYER_FOLLOW => self.follow = v.clamp(0.0, 1.0),
            LAYER_INFLUENCE_AMOUNT => {
                self.infl_amount = v.clamp(0.0, 1.0);
                self.dirty = true;
            }
            LAYER_INFLUENCE_MASK => {
                self.infl_mask = (v.max(0.0) as u32) & 0x1f;
                self.dirty = true;
            }
            f if (LAYER_KIT_CENTS..LAYER_KIT_CENTS + KIT_SLOTS as u16).contains(&f) => {
                self.kit_cents[(f - LAYER_KIT_CENTS) as usize] = v.clamp(0.0, 13_000.0)
            }
            _ => {}
        }
    }

    /// Load a layer blob (protocol LAYER_* layout). Words are already
    /// bounds-checked by the caller to be LAYER_WORDS long.
    pub fn load(&mut self, w: &[f32]) {
        for f in 0..LAYER_FIELDS {
            self.set_field(f as u16, w[LAYER_OFF_FIELDS + f]);
        }
        for i in 0..P::COUNT {
            self.set_param(SET_BASE, i, w[LAYER_OFF_BASE + i]);
            self.set_param(SET_INFLUENCE, i, w[LAYER_OFF_INFLUENCE + i]);
            for k in 0..KIT_SLOTS {
                self.set_param(SET_KIT + k as u8, i, w[LAYER_OFF_KIT + k * P::COUNT + i]);
            }
        }
        for k in 0..N_MACROS {
            self.set_macro(k, w[LAYER_OFF_MACROS + k]);
        }
        self.dirty = true;
    }

    fn resolve(&mut self) {
        let mut b = self.base;
        P::influence_blend(&self.base, &self.influence, self.infl_amount, self.infl_mask, &mut b);
        P::apply_macros(&mut b, &self.macros);
        self.target[0] = b;
        for k in 0..KIT_SLOTS {
            let mut s = self.kit[k];
            P::apply_macros(&mut s, &self.macros);
            self.target[1 + k] = s;
        }
        self.settled = [false; SETS];
        self.dirty = false;
    }

    fn map_set(&mut self, s: usize) {
        for i in 0..P::COUNT {
            self.real[s][i] = P::to_real(i, self.norm[s][i]);
        }
    }

    /// Normalized values after step 3 for the layer set (meters).
    pub fn stored(&self) -> &ParamSet {
        &self.norm[0]
    }

    // ------------------------------------------------------------ notes

    fn alloc_voice(&mut self) -> usize {
        if let Some(v) = (0..VOICES_PER_LAYER).find(|&v| !self.voices[v].active()) {
            return v;
        }
        self.steals += 1;
        let released = (0..VOICES_PER_LAYER).filter(|&v| self.voices[v].released()).min_by_key(|&v| self.slots[v].age);
        if let Some(v) = released {
            return v;
        }
        (0..VOICES_PER_LAYER)
            .min_by(|&a, &b| self.voices[a].loudness().total_cmp(&self.voices[b].loudness()))
            .unwrap_or(0)
    }

    /// Start a sequenced note or hit. `len` samples remain in the current tick.
    pub fn note_from_event(&mut self, track: &Track, curves: &CurvePool, clip_i: u32, e: &Event, dur_samples: u32, len: usize) {
        if self.mute {
            return;
        }
        let clip = track.clip(clip_i as usize);
        let section = track.section(clip.section as usize);
        let free = section.feel == crate::seq::FEEL_FREE;
        let default_slope = if free { 1.0 } else { 60.0 / (section.bpm.max(1.0) * crate::seq::PPQ) };
        let mut slot = Slot {
            left: dur_samples.max(1),
            seq: true,
            velocity: if e.strength.is_finite() && e.strength > 0.0 { e.strength.min(1.0) } else { 1.0 },
            curve: clip.curve,
            clip: clip_i,
            clip_t: e.t,
            clip_end: e.t + e.dur,
            default_slope,
            ..Slot::default()
        };
        if self.kind == KIND_BEAT {
            let k = (e.slot as usize).min(KIT_SLOTS - 1);
            slot.set = 1 + k;
            let own = self.kit_cents[k];
            slot.cents = if e.cents.is_finite() { own + self.follow * (e.cents - own) } else { own };
        } else {
            slot.set = 0;
            slot.cents = if e.cents.is_finite() { e.cents } else { 4800.0 };
        }
        let start = self.hit_start(&mut slot, e);
        self.start_voice(slot, &start, track, curves, len);
    }

    /// Live trigger (MSG_NOTE_ON).
    pub fn note_on_live(&mut self, kit_slot: usize, cents: f32, velocity: f32, track: &Track, curves: &CurvePool, len: usize) {
        if self.mute {
            return;
        }
        let mut slot = Slot { left: u32::MAX, live_key: cents, velocity: velocity.clamp(0.0, 1.0), ..Slot::default() };
        if self.kind == KIND_BEAT {
            let k = kit_slot.min(KIT_SLOTS - 1);
            slot.set = 1 + k;
            slot.cents = if cents.is_finite() { cents } else { self.kit_cents[k] };
        } else {
            if !cents.is_finite() {
                return;
            }
            slot.cents = cents;
        }
        let e = Event { hard: 0.0, ..Event::default() };
        let start = self.hit_start(&mut slot, &e);
        self.start_voice(slot, &start, track, curves, len);
    }

    pub fn note_off_live(&mut self, cents: f32) {
        for v in 0..VOICES_PER_LAYER {
            let s = &mut self.slots[v];
            if !s.seq && s.left == u32::MAX && (s.live_key == cents || (s.live_key.is_nan() && cents.is_nan())) {
                s.left = 0;
                self.voices[v].release();
            }
        }
    }

    pub fn release_all(&mut self) {
        for v in 0..VOICES_PER_LAYER {
            if self.voices[v].active() && !self.voices[v].released() {
                self.voices[v].release();
            }
            self.slots[v].left = 0;
        }
    }

    pub fn release_sequenced(&mut self) {
        for v in 0..VOICES_PER_LAYER {
            if self.slots[v].seq && self.voices[v].active() && !self.voices[v].released() {
                self.voices[v].release();
                self.slots[v].left = 0;
            }
        }
    }

    pub fn panic(&mut self) {
        for v in self.voices.iter_mut() {
            v.kill();
        }
        self.slots = [Slot::default(); VOICES_PER_LAYER];
        self.delay.reset();
        self.reverb.reset();
        self.noise_banks = std::array::from_fn(|_| ShapedNoise::new(self.sr));
    }

    /// Step 6 (and the note's fixed start values). Fills the slot's offsets
    /// and returns the voice's NoteStart.
    fn hit_start(&mut self, slot: &mut Slot, e: &Event) -> NoteStart {
        let r = &self.real[slot.set];
        let mut start = NoteStart {
            velocity: slot.velocity,
            attack: r[P::ENV_ATTACK],
            hold: if r[P::ENV_HOLD] <= 0.0006 { 0.0 } else { r[P::ENV_HOLD] },
            decay: r[P::ENV_DECAY],
            sustain: r[P::ENV_SUSTAIN],
            release: r[P::ENV_RELEASE],
            noise_decay: r[P::ENV_NOISE_DECAY],
            pitch_amt: r[P::ENV_PITCH_AMT],
            pitch_time: r[P::ENV_PITCH_TIME],
            click: r[P::STRIKE_CLICK],
            end_click: r[P::STRIKE_END_CLICK],
            end_stop: false,
        };
        // Note hardness (melody notes and live hits) sharpens the start.
        if e.hard.is_finite() && e.hard > 0.0 {
            let h = e.hard.min(1.0) * self.depth[2];
            start.click = (start.click + 0.5 * h).min(1.0);
            start.attack = P::to_real(P::ENV_ATTACK, P::to_norm(P::ENV_ATTACK, start.attack) - 0.3 * h);
        }
        if self.kind == KIND_BEAT {
            let f = self.follow;
            let ok = |x: f32| x.is_finite();
            if ok(e.attack) && e.attack > 0.0 {
                start.attack += f * (e.attack.clamp(0.001, 1.0) - start.attack);
            }
            if ok(e.burst) {
                start.click = (start.click + f * 0.5 * e.burst.clamp(0.0, 1.0)).min(1.0);
            }
            if ok(e.bend) {
                start.pitch_amt = (start.pitch_amt - f * e.bend).clamp(-24.0, 48.0);
            }
            if ok(e.hold) && e.hold > 0.0 {
                let k = (e.hold / 0.2).clamp(0.25, 4.0).powf(f);
                start.hold *= k;
                start.decay *= k;
                start.noise_decay *= k;
            }
            if ok(e.vowel) {
                slot.off.tilt = f * (e.vowel.clamp(0.0, 1.0) - 0.5) * 0.4;
                slot.off.f1 = f * (e.vowel.clamp(0.0, 1.0) - 0.5) * 0.5;
            }
            if ok(e.tone) {
                let t = e.tone.clamp(0.0, 1.0);
                let n = &self.norm[slot.set];
                slot.off.harm = Some(n[P::SRC_HARM_MIX] + f * (t - n[P::SRC_HARM_MIX]));
                slot.off.noise = Some(n[P::SRC_NOISE_MIX] + f * ((1.0 - t) - n[P::SRC_NOISE_MIX]));
            }
            if ok(e.end_stop) && e.end_stop > 0.5 && f > 0.5 {
                start.end_stop = true;
            }
            if ok(e.end_click) {
                start.end_click = (start.end_click + f * e.end_click.clamp(0.0, 1.0)).min(1.0);
            }
        }
        start
    }

    fn start_voice(&mut self, slot: Slot, start: &NoteStart, track: &Track, curves: &CurvePool, len: usize) {
        let v = self.alloc_voice();
        self.age += 1;
        self.slots[v] = Slot { age: self.age, vmix: 1.0, ..slot };
        let mut nf = [0.0f32; CURVE_STRIDE];
        let frame = self.read_frame(v, track, curves, &mut nf);
        let mut live = self.norm[self.slots[v].set];
        let mut c = self.controls(v, frame.then_some(&nf), &mut live);
        c.max_partials = self.max_partials[v];
        self.voices[v].start(start, &c, len);
    }

    // ------------------------------------------------------------ control rate

    /// Per tick, before the voices: steps 2-3 when dirty, then smoothing.
    pub fn control_tick(&mut self) {
        if self.dirty {
            self.resolve();
        }
        for s in 0..SETS {
            if self.settled[s] {
                continue;
            }
            let mut moving = false;
            for i in 0..P::COUNT {
                let d = self.target[s][i] - self.norm[s][i];
                if d.abs() > 1e-6 {
                    self.norm[s][i] += self.smooth[i] * d;
                    moving = true;
                } else {
                    self.norm[s][i] = self.target[s][i];
                }
            }
            self.map_set(s);
            self.settled[s] = !moving;
        }
        let r = &self.real[0];
        self.crush.set(r[P::CRUSH_BITS], r[P::CRUSH_RATE]);
        self.tape.set(r[P::TAPE_AMT]);
        self.grit_on = r[P::CRUSH_BITS] < 15.99 || r[P::CRUSH_RATE] < self.sr * 0.999 || r[P::TAPE_AMT] > 0.0;
        self.delay.set(r[P::DELAY_TIME], r[P::DELAY_FB], r[P::DELAY_SHIFT], r[P::DELAY_MIX]);
        self.reverb.set(r[P::VERB_SIZE], r[P::VERB_DAMP], r[P::VERB_MIX], r[P::VERB_FREEZE]);
        let lvl = if self.mute { 0.0 } else { dsp::db_to_lin(self.level_db) };
        let a = (self.pan + 1.0) * std::f32::consts::FRAC_PI_4;
        self.gl_t = lvl * a.cos();
        self.gr_t = lvl * a.sin();
    }

    /// Read voice `v`'s curve frame at its clip time (held after the note's
    /// end). False when it has none.
    fn read_frame(&self, v: usize, track: &Track, curves: &CurvePool, out: &mut [f32; CURVE_STRIDE]) -> bool {
        let s = &self.slots[v];
        if s.curve < 0 || !s.seq {
            return false;
        }
        let clip: &Clip = track.clip(s.clip as usize);
        let t = s.clip_t.min(s.clip_end);
        let sec = curve::warp_to_take(track.warp(clip), t, s.default_slope);
        curves.read(s.curve as usize, sec, out)
    }

    /// Per tick, after `control_tick`: each sounding voice gets its controls.
    /// `units_per_sample` advances the voices' clip time for curve reads.
    pub fn voices_tick(&mut self, track: &Track, curves: &CurvePool, units_per_sample: f64) {
        let mut noise_lvl = [0.0f32; SETS];
        let mut loudest = (0.0f32, usize::MAX);
        let mut nf = [0.0f32; CURVE_STRIDE];
        let mut live = self.norm[0];
        for v in 0..VOICES_PER_LAYER {
            if !self.voices[v].active() {
                continue;
            }
            let frame = self.read_frame(v, track, curves, &mut nf);
            let mut lv = self.norm[self.slots[v].set];
            let mut c = self.controls(v, frame.then_some(&nf), &mut lv);
            c.max_partials = self.max_partials[v];
            self.voices[v].control(&c, TICK);
            self.slots[v].clip_t += units_per_sample * TICK as f64;
            noise_lvl[self.slots[v].set] += self.voices[v].noise_level();
            let l = self.voices[v].loudness();
            if l > loudest.0 {
                loudest = (l, v);
                live = lv;
            }
        }
        self.live = if loudest.1 == usize::MAX { self.norm[0] } else { live };
        for s in 0..SETS {
            let mut nb = [0.0f32; N_BANDS];
            for b in 0..N_BANDS {
                nb[b] = self.real[s][P::NOISE_BANDS + b];
            }
            self.noise_banks[s].control(noise_lvl[s], &nb, TICK);
        }
    }

    /// Samples until the next note off among this layer's voices (u32::MAX: none).
    pub fn next_off(&self) -> u32 {
        let mut m = u32::MAX;
        for v in 0..VOICES_PER_LAYER {
            let l = self.slots[v].left;
            if l > 0 && l != u32::MAX && self.voices[v].active() {
                m = m.min(l);
            }
        }
        m
    }

    /// Count `n` samples off every pending note; release those that reach 0.
    pub fn count_offs(&mut self, n: u32) {
        for v in 0..VOICES_PER_LAYER {
            let s = &mut self.slots[v];
            if s.left == 0 || s.left == u32::MAX {
                continue;
            }
            s.left = s.left.saturating_sub(n);
            if s.left == 0 && self.voices[v].active() {
                self.voices[v].release();
            }
        }
    }

    /// Render `n` samples (n <= TICK) of this layer, adding into `l` and `r`.
    pub fn render(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = l.len();
        let bus = &mut self.bus[..n];
        bus.fill(0.0);
        let mut any = false;
        for v in self.voices.iter_mut() {
            if v.active() {
                v.render(bus);
                any = true;
            }
        }
        for nb in self.noise_banks.iter_mut() {
            any |= nb.on;
            nb.render(&mut self.noise, bus);
        }
        let space = self.delay.active() || self.reverb.active();
        let gain_moving = (self.gl - self.gl_t).abs() > 1e-6 || (self.gr - self.gr_t).abs() > 1e-6;
        if !any && !space && !gain_moving {
            self.peak = 0.0;
            return;
        }
        let k = 1.0 / TICK as f32;
        let (dl, dr) = ((self.gl_t - self.gl) * k, (self.gr_t - self.gr) * k);
        let w = self.real[0][P::WIDTH];
        let mut peak = 0.0f32;
        for i in 0..n {
            let mut x = bus[i];
            if self.grit_on {
                x = self.tape.process(self.crush.process(x));
            }
            self.gl += dl;
            self.gr += dr;
            if (self.gl - self.gl_t) * dl > 0.0 {
                self.gl = self.gl_t;
            }
            if (self.gr - self.gr_t) * dr > 0.0 {
                self.gr = self.gr_t;
            }
            let (mut a, mut b) = (x * self.gl, x * self.gr);
            if space {
                (a, b) = self.delay.process(a, b);
                (a, b) = self.reverb.process(a, b);
            }
            (a, b) = width(a, b, w);
            peak = peak.max(a.abs()).max(b.abs());
            l[i] += a;
            r[i] += b;
        }
        self.peak = self.peak.max(peak);
    }

    pub fn sounding(&self) -> usize {
        self.voices.iter().filter(|v| v.active()).count()
    }

    /// Steps 4-6 for voice `v` into real-unit controls; `live` receives the
    /// normalized values after step 5.
    fn controls(&mut self, v: usize, frame: Option<&[f32; CURVE_STRIDE]>, live: &mut ParamSet) -> VoiceControls {
        let slot = &mut self.slots[v];
        let set = slot.set;
        let n = &self.norm[set];
        let r = &self.real[set];
        *live = *n;
        let mut amp = 1.0;
        let mut cents = slot.cents;
        live[P::SRC_TILT] = (n[P::SRC_TILT] + slot.off.tilt).clamp(0.0, 1.0);
        live[P::SRC_F1_FREQ] = (n[P::SRC_F1_FREQ] + slot.off.f1).clamp(0.0, 1.0);
        if let Some(h) = slot.off.harm {
            live[P::SRC_HARM_MIX] = h.clamp(0.0, 1.0);
        }
        if let Some(x) = slot.off.noise {
            live[P::SRC_NOISE_MIX] = x.clamp(0.0, 1.0);
        }
        if let Some(f) = frame {
            amp = gesture_offsets(f, &self.depth, live, &mut slot.vmix, TICK as f32 / self.sr);
            if f[TUNED].is_finite() {
                cents = f[TUNED];
            } else if f[frame::PITCH].is_finite() {
                cents = f[frame::PITCH];
            }
        }
        let m = |i: usize| P::to_real(i, live[i]);
        let mut tone = [0.0; N_BANDS];
        for b in 0..N_BANDS {
            tone[b] = P::to_real(P::TONE_BANDS + b, live[P::TONE_BANDS + b]);
        }
        VoiceControls {
            cents,
            amp,
            harm_mix: m(P::SRC_HARM_MIX),
            noise_mix: m(P::SRC_NOISE_MIX),
            tilt: m(P::SRC_TILT),
            oddeven: r[P::SRC_ODDEVEN],
            stretch: r[P::SRC_STRETCH],
            jitter: r[P::SRC_JITTER],
            modal: r[P::SRC_MODAL],
            damp: r[P::SRC_DAMP],
            f1_freq: m(P::SRC_F1_FREQ),
            f1_gain: r[P::SRC_F1_GAIN],
            f2_freq: r[P::SRC_F2_FREQ],
            f2_gain: r[P::SRC_F2_GAIN],
            tone_bands: tone,
            fold_amt: r[P::FOLD_AMT],
            fold_bias: r[P::FOLD_BIAS],
            clang_amt: r[P::CLANG_AMT],
            clang_ratio: r[P::CLANG_RATIO],
            comb_amt: r[P::COMB_AMT],
            comb_tune: r[P::COMB_TUNE],
            comb_decay: r[P::COMB_DECAY],
            filt_cutoff: m(P::FILT_CUTOFF),
            filt_res: r[P::FILT_RES],
            filt_morph: r[P::FILT_MORPH],
            filt_track: r[P::FILT_TRACK],
            strike_color: r[P::STRIKE_COLOR],
            lfo1_rate: r[P::LFO1_RATE],
            lfo1_pitch: r[P::LFO1_TO_PITCH],
            lfo1_filter: r[P::LFO1_TO_FILTER],
            lfo1_level: r[P::LFO1_TO_LEVEL],
            lfo2_rate: r[P::LFO2_RATE],
            lfo2_pitch: r[P::LFO2_TO_PITCH],
            lfo2_filter: r[P::LFO2_TO_FILTER],
            chaos_rate: r[P::CHAOS_RATE],
            chaos_amt: r[P::CHAOS_AMT],
            max_partials: crate::voice::MAX_PARTIALS,
        }
    }
}

/// Step 5 for stored playback: fixed linear maps from a frame to normalized
/// parameter offsets (TDD Live follow requirement 12), and the voicing
/// crossfade. Returns the amplitude factor from loudness.
/// Maps, each scaled by its depth (loud, open, hard, breath, mouth):
/// - loudness l: amplitude (1 - d) + d l
/// - openness o: src.tilt +0.4 o, src.f1_freq +0.25 o, filt.cutoff +0.2 o
/// - breathiness b: src.noise_mix +0.5 b
/// - mouth: tone and noise bands move toward the frame's vowel shape (dB)
/// - unvoiced frames crossfade partials to noise over 10 ms
pub fn gesture_offsets(f: &[f32; CURVE_STRIDE], depth: &[f32; 5], p: &mut ParamSet, vmix: &mut f32, dt: f32) -> f32 {
    let fin = |x: f32| if x.is_finite() { x.clamp(0.0, 1.0) } else { 0.0 };
    let loud = fin(f[frame::LOUDNESS]);
    let open = fin(f[frame::OPENNESS]) * depth[1];
    let breath = fin(f[frame::BREATHINESS]) * depth[3];
    let add = |p: &mut ParamSet, i: usize, d: f32| p[i] = (p[i] + d).clamp(0.0, 1.0);
    add(p, P::SRC_TILT, 0.4 * open);
    add(p, P::SRC_F1_FREQ, 0.25 * open);
    add(p, P::FILT_CUTOFF, 0.2 * open);
    add(p, P::SRC_NOISE_MIX, 0.5 * breath);
    let dm = depth[4];
    if dm > 0.0 {
        for b in 0..N_BANDS {
            let v = f[frame::BANDS + b];
            if v.is_finite() {
                let t = P::to_norm(P::TONE_BANDS + b, v.clamp(-30.0, 30.0));
                p[P::TONE_BANDS + b] += dm * (t - p[P::TONE_BANDS + b]);
                p[P::NOISE_BANDS + b] += dm * (t - p[P::NOISE_BANDS + b]);
            }
        }
    }
    let voicing = f[frame::VOICING];
    let target = if voicing < 0.5 { 1.0 } else { 0.0 };
    let dv = dt / VOICE_XFADE_S;
    *vmix += (target - *vmix).clamp(-dv, dv);
    let h = p[P::SRC_HARM_MIX];
    let n = p[P::SRC_NOISE_MIX];
    p[P::SRC_HARM_MIX] = h * *vmix;
    p[P::SRC_NOISE_MIX] = n * *vmix + h.max(n) * (1.0 - *vmix);
    let silent = voicing > 1.5;
    let amp = (1.0 - depth[0]) + depth[0] * loud;
    if silent { amp * 0.0 } else { amp }
}
