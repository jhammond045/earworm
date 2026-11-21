//! One layer: its patch (`LOAD_PATCH`), its value vector (stored values,
//! macros, Influence target), resolution steps 1 to 4, up to eight voices,
//! its layer sources (shaped noise per voice patch, one grain cloud) and its
//! channel.
//!
//! Resolution (docs/synth.md "Order of evaluation"):
//! 1. Morph: with states loaded (`load_states`), the engine tells the layer
//!    each control tick which two states its clip's morph lane blends and how
//!    far (`set_morph`); the blend of their values, macros and Influence
//!    amount replaces the layer's own. A steady blend marks nothing dirty.
//!    A scene (MSG_MORPH_TO, `morph_to`) blends from where the layer was to
//!    one state over a time, and is used instead when the clip has no marker
//!    or the transport is stopped. Without a marker (or clip) and without a
//!    scene the engine asks for state 0. A layer that
//!    never got states keeps what `LOAD_LAYER` and live messages set. A new
//!    patch starts every value at the patch's starting value.
//! 2. Automation: arrives with recording (stage S5).
//! 3. Influence: each value whose group bit is in the mask moves toward the
//!    target by the amount.
//! 4. Layer routes: sources that are one per layer (macros, the mod wheel,
//!    bend and pressure, the layer LFOs, the layer's sung features) move any
//!    value of the layer, voice values included; then clamp. Step 3 runs when
//!    a stored value changes; step 4 also runs every tick while a route's
//!    source can move by itself (anything but a macro). Every value is then
//!    smoothed per control tick with its descriptor's `smooth_ms`, in the
//!    normalized domain, and mapped to real units. Channel values are
//!    finished here.
//! 5 to 7 run per voice (`voice.rs`).
//!
//! The layer's sung features come from the mic while the layer listens
//! (`listen`), else from its clip's take at the transport position while the
//! transport plays; the engine hands them to `control_tick`.
//!
//! The channel is the patch's effect chain: the effects before the first
//! stereo one run on the mono sum of the voices, level and pan then spread it
//! to stereo, the stereo effects follow, and width is last (today's order).
//! A filter runs mono or stereo depending on where it sits. A bypassed effect
//! is skipped; bypass is not structure, so switching it keeps notes sounding.
//!
//! Structure changes (docs/synth.md "Changing the structure"): a new patch
//! that is more than a route-shaper or bypass edit, arriving while the layer
//! sounds, does not cut it. The layer keeps playing the old patch and fades
//! its output to silence over 5 ms, then takes the new patch at the next
//! control tick. Values, states and layer blobs sent meanwhile are for the
//! new patch and wait with it; notes that start meanwhile start when it is
//! taken. The patch's cost (patch.rs) caps how many voices it may sound.

use crate::curve::{self, CurvePool, CURVE_STRIDE};
use crate::modules::LfoSrc;
use crate::grain::{GrainCloud, VoiceGrain};
use crate::patch::*;
use crate::protocol::*;
use crate::samples::SamplePool;
use crate::seq::{Clip, Event, Track};
use crate::voice::{features, via_value, LayerView, Note, Voice, N_BANDS, TICK};
use dsp::bands::{band_centres, band_q};
use dsp::filters::Svf;
use dsp::frame;
use dsp::layer_fx::{width, Crush, Reverb, StereoDelay, Tape};
use dsp::DENORMAL;
use dsp::noise::Noise;
use params as P;

/// Shaped-noise rms with flat bands before level, as in M0.
const NOISE_RMS: f32 = 0.35;

/// What drives a voice besides its layer's values.
#[derive(Clone, Copy, Debug)]
struct Slot {
    /// Voice patch (kit slot on a beat layer).
    vp: usize,
    /// Live triggers: the cents value of the NOTE_ON, for NOTE_OFF matching.
    live_key: f32,
    /// Started by Live follow (the mic): melody and texture voices read the
    /// live frame every tick, and only the engine releases them.
    follow: bool,
    /// Samples until note off; u32::MAX holds until a NOTE_OFF.
    left: u32,
    seq: bool,
    age: u64,
    /// Curve slot and the clip whose warp map reads it (-1: none).
    curve: i32,
    clip: u32,
    /// Clip time of the voice now, and its end (reading holds after it).
    clip_t: f64,
    clip_end: f64,
    /// Seconds of take time per clip unit with no anchors (free sections: 1).
    default_slope: f64,
}

impl Default for Slot {
    fn default() -> Self {
        Slot {
            vp: 0,
            live_key: f32::NAN,
            follow: false,
            left: 0,
            seq: false,
            age: 0,
            curve: -1,
            clip: 0,
            clip_t: 0.0,
            clip_end: 0.0,
            default_slope: 1.0,
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
    /// Each bank has its own generator: a shared one would hand out numbers
    /// in an order that depends on how a tick is split into calls.
    rng: Noise,
}

impl ShapedNoise {
    fn new(sr: f32, seed: u32) -> Self {
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
            rng: Noise::new(seed),
        }
    }

    /// Per tick: `level` is the summed noise level of the voices on this
    /// bank, `bands_db` its noise bands in dB.
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

    /// Move toward a new level from where the ramp is now, over the `len` samples left in
    /// the tick (a voice that started inside it).
    fn retarget(&mut self, level: f32, bands_db: &[f32; N_BANDS], len: usize) {
        let inv = 1.0 / len.max(1) as f32;
        let mut any = false;
        for i in 0..N_BANDS {
            let t = dsp::db_to_lin(bands_db[i]) * level * self.norm;
            let t = if t.is_finite() { t } else { 0.0 };
            let now = self.target[i] - self.step[i] * len as f32;
            self.step[i] = (t - now) * inv;
            self.target[i] = t;
            any |= t > 1e-7 || now > 1e-7;
        }
        self.on |= any;
    }

    fn render(&mut self, out: &mut [f32]) {
        if !self.on {
            return;
        }
        for y in out.iter_mut() {
            let x = self.rng.next();
            let mut s = 0.0;
            for b in 0..N_BANDS {
                self.gain[b] += self.step[b];
                s += self.gain[b] * self.bp[b].bandpass_norm(x);
            }
            *y += s;
        }
    }
}

/// A channel filter: two state-variable filters (left and right; the left alone in the mono part).
struct ChanFilter {
    svf: [Svf; 2],
    key: (u32, u32),
    on: bool,
    w: [f32; 3],
}

impl ChanFilter {
    fn new() -> Self {
        ChanFilter { svf: [Svf::default(); 2], key: (0, 0), on: false, w: [0.0; 3] }
    }

    /// Per tick, the voice filter's mapping: off when fully open as a low-pass.
    fn set(&mut self, sr: f32, cutoff: f32, res: f32, morph: f32) {
        let cut = modules_cl(cutoff, 20.0, 0.45 * sr);
        let (res, morph) = (modules_cl(res, 0.0, 1.0), modules_cl(morph, 0.0, 1.0));
        let on = !(morph <= 0.0 && cut >= 18_000.0);
        if on {
            let q = 0.5 + 19.5 * res * res;
            let key = (cut.to_bits(), q.to_bits());
            if !self.on {
                self.svf.iter_mut().for_each(|f| f.reset());
                self.key = (0, 0);
            }
            if key != self.key {
                self.key = key;
                self.svf.iter_mut().for_each(|f| f.set(sr, cut, q));
            }
            self.w = if morph < 0.5 {
                let w = morph * 2.0;
                [1.0 - w, w / q, 0.0]
            } else {
                let w = morph * 2.0 - 1.0;
                [0.0, (1.0 - w) / q, w]
            };
        }
        self.on = on;
    }

    #[inline]
    fn process(&mut self, ch: usize, x: f32) -> f32 {
        let o = self.svf[ch].process(x);
        o.lp * self.w[0] + o.bp * self.w[1] + o.hp * self.w[2] + DENORMAL
    }
}

use crate::modules::cl as modules_cl;

/// The channel's effect instances, allocated at start-up: as many of each as a chain may hold.
struct Channel {
    crush: [Crush; MAX_CHANNEL],
    tape: [Tape; MAX_CHANNEL],
    filter: [ChanFilter; MAX_CHANNEL],
    delay: [StereoDelay; MAX_DELAYS],
    reverb: [Reverb; MAX_REVERBS],
    /// The instance each effect of the patch's chain uses, and where the stereo part starts.
    inst: [u8; MAX_CHANNEL],
    split: usize,
    /// A filter in the stereo part is on this tick.
    stereo_filter: bool,
}

impl Channel {
    fn reset_tails(&mut self) {
        self.delay.iter_mut().for_each(|d| d.reset());
        self.reverb.iter_mut().for_each(|r| r.reset());
    }

    /// Instances for the chain of `p`: the k-th effect of a type uses that type's k-th instance.
    fn index(&mut self, p: &Patch) {
        let mut n = [0u8; 256];
        self.split = p.n_channel;
        for (c, fx) in p.channel().iter().enumerate() {
            let t = (fx.code & 255) as usize;
            self.inst[c] = n[t];
            n[t] += 1;
            if matches!(fx.code, P::T_DELAY | P::T_REVERB) && self.split == p.n_channel {
                self.split = c;
            }
        }
    }

    /// Something in the stereo part sounds.
    fn stereo_active(&self, p: &Patch) -> bool {
        self.stereo_filter
            || p.channel().iter().zip(&self.inst).any(|(fx, &j)| match fx.code {
                P::T_DELAY => self.delay[j as usize].active(),
                P::T_REVERB => self.reverb[j as usize].active(),
                _ => false,
            })
    }
}

/// The structure-change fade (docs/synth.md "Changing the structure").
const FADE_S: f32 = 0.005;

/// Per voice patch: the values its voice routes and voicing swap write.
struct Touched {
    list: [u16; MAX_VALUES],
    n: usize,
    is: [bool; MAX_VALUES],
    amount: [bool; MAX_VALUES],
    /// Moved only by routes latched at note start: the voice starts it from its real value.
    from_real: [bool; MAX_VALUES],
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
    pub macros: [f32; N_MACROS],

    patch: Box<Patch>,
    /// A new structure waiting for the fade to end (`pending`), else spare.
    next: Box<Patch>,
    pending: bool,
    /// Samples of the fade left, and its gain and step per sample.
    fade_left: u32,
    fade_len: u32,
    fade_g: f32,
    /// Notes that started during the fade; they start when the new patch is taken.
    queued: [(Slot, Note); VOICES_PER_LAYER],
    n_queued: usize,
    /// What the layer may spend (ns per sample, patch.rs `layer_budget`) and the voices the
    /// current patch's cost leaves room for.
    budget: f32,
    pub max_voices: usize,
    touched: Box<[Touched; KIT_SLOTS]>,
    /// Stored values (after the morph), Influence target and group bits.
    base: Box<[f32; MAX_VALUES]>,
    influence: Box<[f32; MAX_VALUES]>,
    groups: Box<[u32; MAX_VALUES]>,
    /// Steps 3-4 output, the smoothed copy and its real units.
    target: Box<[f32; MAX_VALUES]>,
    norm: Box<[f32; MAX_VALUES]>,
    real: Box<[f32; MAX_VALUES]>,
    smooth: Box<[f32; MAX_VALUES]>,
    settled: bool,
    dirty: bool,

    pub voices: [Voice; VOICES_PER_LAYER],
    slots: [Slot; VOICES_PER_LAYER],
    noise_banks: [ShapedNoise; KIT_SLOTS],
    seed: u32,
    chan: Channel,
    bus: [f32; TICK],
    gl: f32,
    gr: f32,
    gl_t: f32,
    gr_t: f32,
    gl_step: f32,
    gr_step: f32,
    mono_on: bool,
    /// What renders during the current tick, decided at the tick (or when a
    /// voice starts) so that splitting a tick into smaller calls cannot change
    /// which samples a voice or effect is advanced over.
    voice_on: [bool; VOICES_PER_LAYER],
    layer_on: bool,
    stereo_on: bool,
    pub peak: f32,
    pub steals: u32,
    /// Morph states (resolution step 1) and the blend last applied.
    states: Box<[f32; MAX_STATES * MAX_VALUES]>,
    state_macros: [[f32; N_MACROS]; MAX_STATES],
    state_amount: [f32; MAX_STATES],
    n_states: usize,
    morph_applied: Option<(u32, u32, f32)>,
    /// Scene (MSG_MORPH_TO): the blend the layer had when it was asked, and the move to a state.
    scene: Option<Scene>,
    scene_from: Box<[f32; MAX_VALUES]>,
    scene_from_macros: [f32; N_MACROS],
    scene_from_amount: f32,
    pub grain: GrainCloud,
    /// Partial budget share per voice, set by the engine before each tick.
    pub max_partials: [usize; VOICES_PER_LAYER],
    age: u64,
    /// Normalized values after step 6 for the loudest voice (meters).
    pub live: Box<[f32; MAX_VALUES]>,
    /// The meter block shows this layer, so `live` is kept up to date.
    pub metered: bool,
    /// Live follow: the frame record the live voice reads each tick, and the
    /// gate envelope that scales its amplitude.
    follow_rec: [f32; CURVE_STRIDE],
    follow_gain: f32,

    /// Listening (MSG_LISTEN): the layer's features come from the mic.
    pub listen: bool,
    /// Mod wheel (0..1), pitch bend (-1..1), channel pressure (0..1): MSG_SET_PERF.
    pub perf: [f32; 3],
    lfos: [LfoSrc; MAX_LAYER_LFOS],
    /// The layer's source values this tick (macros, perf, layer LFOs, features).
    lsrc: [f32; N_LAYER_SOURCES],
    /// This tick's feature frame, for routes from the sung vowel.
    feat: [f32; CURVE_STRIDE],
    has_feat: bool,
    /// The sung pitch holds through unvoiced frames.
    held_pitch: f32,
    /// Steps 1 to 3 (before the layer routes).
    pre: Box<[f32; MAX_VALUES]>,
    /// Some layer route's source moves by itself (or is smoothed), so step 4 runs every tick.
    lr_dynamic: bool,
    /// Some layer route reads the layer's features.
    lr_features: bool,
    /// Values a layer route writes.
    lr_list: Box<[u16; MAX_VALUES]>,
    lr_n: usize,
    lr_smooth: [f32; MAX_LAYER_ROUTES],
    lr_fresh: bool,
    /// Each layer route's shaped output this tick, NaN without a value (meters).
    lr_out: [f32; MAX_LAYER_ROUTES],
    /// The voice the meters show (the loudest), or usize::MAX.
    meter_voice: usize,
}

/// A scene move: to state `to` over `len` samples, `done` so far; `applied` is the blend
/// last written (NaN: write it again, because a lane or new states wrote over it).
#[derive(Clone, Copy)]
struct Scene {
    to: u32,
    len: u32,
    done: u32,
    applied: f32,
}

/// Salt of the layer LFOs' random sequences.
const LAYER_LFO_SALT: [u32; MAX_LAYER_LFOS] = [0x6c07_8965, 0x9d2c_5680];

fn bank_seed(layer_seed: u32, k: usize) -> u32 {
    layer_seed ^ 0x5bd1_e995u32.wrapping_mul(k as u32 + 1)
}

/// Group depths in route order: loud, open, hard, breath, mouth, follow.
fn depths(d: &[f32; 5], follow: f32) -> [f32; 6] {
    [d[0], d[1], d[2], d[3], d[4], follow]
}

impl Layer {
    pub fn new(sr: f32, index: usize) -> Self {
        let seed = 0x9E37_79B9u32.wrapping_mul(index as u32 + 1);
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
            macros: [0.5; N_MACROS],
            patch: Patch::new_boxed(),
            next: Patch::new_boxed(),
            pending: false,
            fade_left: 0,
            fade_len: ((FADE_S * sr).round() as u32).max(1),
            fade_g: 1.0,
            queued: [(Slot::default(), Note::default()); VOICES_PER_LAYER],
            n_queued: 0,
            budget: 0.0,
            max_voices: VOICES_PER_LAYER,
            touched: Box::new(std::array::from_fn(|_| Touched { list: [0; MAX_VALUES], n: 0, is: [false; MAX_VALUES], amount: [false; MAX_VALUES], from_real: [false; MAX_VALUES] })),
            base: Box::new([0.0; MAX_VALUES]),
            influence: Box::new([0.0; MAX_VALUES]),
            groups: Box::new([0; MAX_VALUES]),
            target: Box::new([0.0; MAX_VALUES]),
            norm: Box::new([0.0; MAX_VALUES]),
            real: Box::new([0.0; MAX_VALUES]),
            smooth: Box::new([1.0; MAX_VALUES]),
            settled: false,
            dirty: true,
            voices: std::array::from_fn(|v| Voice::new(sr, seed ^ (v as u32 + 1).wrapping_mul(0x85EB_CA6B))),
            slots: [Slot::default(); VOICES_PER_LAYER],
            noise_banks: std::array::from_fn(|k| ShapedNoise::new(sr, bank_seed(seed, k + 1))),
            seed,
            chan: Channel {
                crush: std::array::from_fn(|j| Crush::new(sr, seed ^ 0x27d4_eb2f ^ (j as u32).wrapping_mul(0x9e37_79b9))),
                tape: std::array::from_fn(|j| Tape::new(sr, seed ^ 0x1656_67b1 ^ (j as u32).wrapping_mul(0x9e37_79b9))),
                filter: std::array::from_fn(|_| ChanFilter::new()),
                delay: std::array::from_fn(|_| StereoDelay::new(sr)),
                reverb: std::array::from_fn(|j| Reverb::new(sr, seed ^ 0xc2b2_ae35 ^ (j as u32).wrapping_mul(0x9e37_79b9))),
                inst: [0; MAX_CHANNEL],
                split: 0,
                stereo_filter: false,
            },
            bus: [0.0; TICK],
            gl: 0.0,
            gr: 0.0,
            gl_t: 0.0,
            gr_t: 0.0,
            gl_step: 0.0,
            gr_step: 0.0,
            mono_on: false,
            voice_on: [false; VOICES_PER_LAYER],
            layer_on: false,
            stereo_on: false,
            peak: 0.0,
            steals: 0,
            states: Box::new([0.0; MAX_STATES * MAX_VALUES]),
            state_macros: [[0.5; N_MACROS]; MAX_STATES],
            state_amount: [0.0; MAX_STATES],
            n_states: 0,
            morph_applied: None,
            scene: None,
            scene_from: Box::new([0.0; MAX_VALUES]),
            scene_from_macros: [0.5; N_MACROS],
            scene_from_amount: 0.0,
            grain: GrainCloud::new(sr, seed ^ 0x7f4a_7c15),
            max_partials: [crate::voice::MAX_PARTIALS; VOICES_PER_LAYER],
            age: 0,
            live: Box::new([0.0; MAX_VALUES]),
            metered: index == 0,
            follow_rec: [0.0; CURVE_STRIDE],
            follow_gain: 0.0,
            listen: false,
            perf: [0.0; 3],
            lfos: std::array::from_fn(|k| LfoSrc::new(seed ^ LAYER_LFO_SALT[k])),
            lsrc: [f32::NAN; N_LAYER_SOURCES],
            feat: [f32::NAN; CURVE_STRIDE],
            has_feat: false,
            held_pitch: f32::NAN,
            pre: Box::new([0.0; MAX_VALUES]),
            lr_dynamic: false,
            lr_features: false,
            lr_list: Box::new([0; MAX_VALUES]),
            lr_n: 0,
            lr_smooth: [f32::NAN; MAX_LAYER_ROUTES],
            lr_fresh: true,
            lr_out: [f32::NAN; MAX_LAYER_ROUTES],
            meter_voice: usize::MAX,
        };
        let classic = crate::classic::patch_blob(KIND_MELODY, index as u32);
        let mut p = Patch::new_boxed();
        p.parse(&classic).expect("the built-in classic patch is valid");
        let mut beat = Patch::new_boxed();
        beat.parse(&crate::classic::patch_blob(KIND_BEAT, index as u32)).expect("the built-in classic kit is valid");
        l.budget = layer_budget(&[&p, &beat]);
        l.set_patch(&p);
        l
    }

    // ------------------------------------------------------------ patch

    pub fn patch(&self) -> &Patch {
        &self.patch
    }

    /// Values of the patch that layer and states blobs are for (the incoming one during a fade).
    pub fn n_values(&self) -> usize {
        self.incoming().n_values
    }

    /// Replace the patch (LOAD_PATCH). An identical patch changes nothing; one that differs only
    /// in its routes' shapers or the channel's bypass switches keeps the voices and the values.
    /// Otherwise the values return to the patch's starting values, and the layer takes the patch
    /// now when it is silent, or after a 5 ms fade on the old patch when it sounds.
    pub fn set_patch(&mut self, p: &Patch) {
        if self.pending {
            if *self.next == *p {
                return;
            }
            let same = self.next.same_structure(p);
            (*self.next).clone_from(p);
            if !same {
                self.start_values();
            }
            return;
        }
        if *self.patch == *p {
            return;
        }
        if self.patch.same_structure(p) {
            (*self.patch).clone_from(p);
            self.index_routes();
            self.dirty = true;
            return;
        }
        (*self.next).clone_from(p);
        self.start_values();
        if self.layer_on {
            self.pending = true;
            self.fade_left = self.fade_len;
            self.fade_g = 1.0;
        } else {
            self.swap();
        }
    }

    /// The incoming patch's values at its starting values, Influence moving each by its
    /// descriptor's group until a layer blob says otherwise; no states until LOAD_STATES.
    /// While the old patch fades out nothing reads these (its values are frozen).
    fn start_values(&mut self) {
        let p = &*self.next;
        for i in 0..p.n_values {
            self.base[i] = p.init[i];
            self.influence[i] = p.init[i];
            self.groups[i] = 1 << (P::desc(p.descs[i] as usize).group as u32);
        }
        self.n_states = 0;
        self.morph_applied = None;
        self.scene = None;
    }

    /// The patch that values, states and layer blobs are for: the one waiting, if any.
    fn incoming(&self) -> &Patch {
        if self.pending { &self.next } else { &self.patch }
    }

    /// Take the incoming patch: every voice is silenced, the channel's tails are cleared, and
    /// the values (already set for it) are resolved afresh.
    fn swap(&mut self) {
        std::mem::swap(&mut self.patch, &mut self.next);
        self.pending = false;
        self.fade_left = 0;
        self.fade_g = 1.0;
        for v in 0..VOICES_PER_LAYER {
            self.voices[v].kill();
        }
        self.slots = [Slot::default(); VOICES_PER_LAYER];
        let n = self.patch.n_values;
        let dt = TICK as f32 / self.sr;
        for i in 0..n {
            let ms = P::desc(self.patch.descs[i] as usize).smooth_ms;
            self.smooth[i] = if ms > 0.0 { 1.0 - (-dt * 1000.0 / ms).exp() } else { 1.0 };
        }
        self.index_routes();
        self.chan.index(&self.patch);
        for l in self.lfos.iter_mut() {
            l.start();
        }
        self.held_pitch = f32::NAN;
        for v in 0..VOICES_PER_LAYER {
            self.voices[v].bind(&self.patch.voices[0], 0);
            self.voices[v].kill();
        }
        self.max_voices = self.patch.voice_count(self.budget);
        self.reset_noise();
        self.grain.clear_grains();
        self.chan.reset_tails();
        self.resolve();
        self.norm[..n].copy_from_slice(&self.target[..n]);
        self.map_all();
        self.settled = true;
    }

    /// Voices the patch may sound at once, and its cost per voice and per layer (ns per sample)
    /// against the layer's budget.
    pub fn cost(&self) -> (usize, f32, f32, f32) {
        let p = self.incoming();
        (p.voice_count(self.budget), p.max_voice_cost(), p.layer_cost(), self.budget)
    }

    /// What the patch's routes write: per voice patch the values its voice routes and voicing
    /// swap touch, and the values the layer routes write.
    fn index_routes(&mut self) {
        let p = &*self.patch;
        for (k, vp) in p.voices().iter().enumerate() {
            let t = &mut self.touched[k];
            t.is = [false; MAX_VALUES];
            t.amount = [false; MAX_VALUES];
            for r in vp.routes() {
                t.amount[r.amount as usize] = true;
            }
            t.n = 0;
            let add = |t: &mut Touched, i: usize| {
                if !t.is[i] {
                    t.is[i] = true;
                    t.list[t.n] = i as u16;
                    t.n += 1;
                }
            };
            for r in vp.routes() {
                match r.target_kind {
                    TGT_BANDS => (0..N_BANDS).for_each(|b| add(t, r.targets[0] as usize + b)),
                    TGT_VALUE | TGT_OCTAVES => r.targets[..r.n_targets as usize].iter().for_each(|&x| add(t, x as usize)),
                    _ => {}
                }
            }
            t.from_real = t.is;
            for r in vp.routes() {
                if !r.latch && (r.target_kind == TGT_VALUE || r.target_kind == TGT_OCTAVES) {
                    r.targets[..r.n_targets as usize].iter().for_each(|&x| t.from_real[x as usize] = false);
                }
                if !r.latch && r.target_kind == TGT_BANDS {
                    (0..N_BANDS).for_each(|b| t.from_real[r.targets[0] as usize + b] = false);
                }
            }
            if let (Some(m), Some((nb, _))) = (vp.find(P::T_PARTIALS), vp.noise) {
                add(t, vp.modules[m].base as usize + P::partials::LEVEL);
                add(t, nb as usize + P::shaped_noise::LEVEL);
                t.from_real[vp.modules[m].base as usize + P::partials::LEVEL] = false;
                t.from_real[nb as usize + P::shaped_noise::LEVEL] = false;
            }
        }
        let mut seen = [false; MAX_VALUES];
        self.lr_n = 0;
        self.lr_dynamic = false;
        self.lr_features = false;
        for r in p.layer_routes() {
            let n = if r.target_kind == TGT_BANDS { N_BANDS } else { 1 };
            for &t in &r.targets[..r.n_targets as usize] {
                for b in 0..n {
                    let i = t as usize + b;
                    if !seen[i] {
                        seen[i] = true;
                        self.lr_list[self.lr_n] = i as u16;
                        self.lr_n += 1;
                    }
                }
            }
            let moves = |s: u16| s != SRC_NONE && s != SRC_MACRO;
            // A smoothed route keeps moving after its source steps.
            self.lr_dynamic |= moves(r.source) || moves(r.via) || r.smoothed();
            self.lr_features |= is_feature_source(r.source) || is_feature_source(r.via);
        }
        self.lr_smooth = [f32::NAN; MAX_LAYER_ROUTES];
        self.lr_fresh = true;
    }

    /// Grain sample id of the patch (its first voice patch with grains), if any.
    pub fn grain_sample(&self) -> Option<usize> {
        self.incoming().voices().iter().find_map(|vp| vp.grains.map(|g| g.2 as usize))
    }

    // ------------------------------------------------------------ stored sound

    pub fn set_param(&mut self, set: u8, i: usize, v: f32) {
        if i >= self.n_values() || !v.is_finite() {
            return;
        }
        let v = v.clamp(0.0, 1.0);
        match set {
            SET_BASE => self.base[i] = v,
            SET_INFLUENCE => self.influence[i] = v,
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

    /// Load a layer blob v2 (protocol "Layer blob"): the caller checked the
    /// magic and that it holds `n_values` values for this layer's patch.
    pub fn load(&mut self, w: &[f32]) {
        let n = self.n_values();
        for f in 0..LAYER_FIELDS {
            self.set_field(f as u16, w[LAYER_OFF_FIELDS + f]);
        }
        let vals = &w[LAYER_HEADER_WORDS..];
        for i in 0..n {
            self.set_param(SET_BASE, i, vals[i]);
            self.set_param(SET_INFLUENCE, i, vals[n + N_MACROS + i]);
            self.groups[i] = vals[2 * n + N_MACROS + i].to_bits() & 0x1f;
        }
        for k in 0..N_MACROS {
            self.set_macro(k, vals[n + k]);
        }
        // With states loaded they stay the source of values, macros and amount.
        self.morph_applied = None;
        self.dirty = true;
    }

    /// Load states (protocol "States blob" v2): `w` holds `n` states of
    /// `n_values + N_MACROS + 1` floats, already checked. n = 0 clears them
    /// (the layer then keeps the values it has now).
    pub fn load_states(&mut self, w: &[f32], n: usize) {
        let nv = self.n_values();
        let stride = nv + N_MACROS + 1;
        let clean = |x: f32, d: f32| if x.is_finite() { x.clamp(0.0, 1.0) } else { d };
        for (k, st) in w.chunks_exact(stride).take(n.min(MAX_STATES)).enumerate() {
            for i in 0..nv {
                self.states[k * MAX_VALUES + i] = clean(st[i], self.incoming().init[i]);
            }
            for m in 0..N_MACROS {
                self.state_macros[k][m] = clean(st[nv + m], 0.5);
            }
            self.state_amount[k] = clean(st[nv + N_MACROS], 0.0);
        }
        self.n_states = n.min(MAX_STATES);
        self.morph_applied = None;
        if let Some(sc) = self.scene.as_mut() {
            sc.applied = f32::NAN;
        }
    }

    pub fn has_states(&self) -> bool {
        self.n_states > 0
    }

    /// Step 1: blend state `from` into state `to` by `x` (0..1). Does nothing
    /// without states, and nothing (no dirty mark) when the blend is the one
    /// already applied. Indices past the loaded states use the last one.
    pub fn set_morph(&mut self, from: u32, to: u32, x: f32) {
        if self.n_states == 0 {
            return;
        }
        let last = (self.n_states - 1) as u32;
        let (a, b) = (from.min(last), to.min(last));
        let x = if a == b || !x.is_finite() { 0.0 } else { x.clamp(0.0, 1.0) };
        if self.morph_applied == Some((a, b, x)) {
            return;
        }
        self.morph_applied = Some((a, b, x));
        if let Some(sc) = self.scene.as_mut() {
            sc.applied = f32::NAN;
        }
        let (a, b) = (a as usize, b as usize);
        // Exact at the ends, so a settled morph sits on the state's own values.
        let mix = |p: f32, q: f32| if x <= 0.0 { p } else if x >= 1.0 { q } else { p + (q - p) * x };
        for i in 0..self.n_values() {
            self.base[i] = mix(self.states[a * MAX_VALUES + i], self.states[b * MAX_VALUES + i]);
        }
        for m in 0..N_MACROS {
            self.macros[m] = mix(self.state_macros[a][m], self.state_macros[b][m]);
        }
        self.infl_amount = mix(self.state_amount[a], self.state_amount[b]);
        self.dirty = true;
    }

    /// MSG_MORPH_TO: move from the blend the layer has now to state `to` over `ms`. Does
    /// nothing without states.
    pub fn morph_to(&mut self, to: u32, ms: f32) {
        if self.n_states == 0 {
            return;
        }
        let n = self.n_values();
        self.scene_from[..n].copy_from_slice(&self.base[..n]);
        self.scene_from_macros = self.macros;
        self.scene_from_amount = self.infl_amount;
        let ms = if ms.is_finite() { ms.clamp(0.0, 60_000.0) } else { 0.0 };
        let len = (ms * 0.001 * self.sr).round() as u32;
        self.scene = Some(Scene { to: to.min(self.n_states as u32 - 1), len, done: 0, applied: f32::NAN });
    }

    /// Whether a scene has been asked for since the states were last replaced by a new patch.
    pub fn has_scene(&self) -> bool {
        self.scene.is_some()
    }

    /// Step 1 from the scene: advance it one control tick and write its blend. Marks nothing
    /// dirty once it has arrived and nothing wrote over it.
    pub fn scene_tick(&mut self) {
        let Some(mut sc) = self.scene else { return };
        sc.done = sc.done.saturating_add(TICK as u32).min(sc.len);
        let x = if sc.len == 0 { 1.0 } else { sc.done as f32 / sc.len as f32 };
        if sc.applied == x {
            self.scene = Some(sc);
            return;
        }
        sc.applied = x;
        self.scene = Some(sc);
        self.morph_applied = None;
        let b = sc.to as usize;
        let mix = |p: f32, q: f32| if x >= 1.0 { q } else { p + (q - p) * x };
        for i in 0..self.n_values() {
            self.base[i] = mix(self.scene_from[i], self.states[b * MAX_VALUES + i]);
        }
        for m in 0..N_MACROS {
            self.macros[m] = mix(self.scene_from_macros[m], self.state_macros[b][m]);
        }
        self.infl_amount = mix(self.scene_from_amount, self.state_amount[b]);
        self.dirty = true;
    }

    /// Steps 3 and 4 for every value.
    fn resolve(&mut self) {
        let n = self.patch.n_values;
        let a = if self.infl_amount.is_nan() { 0.0 } else { self.infl_amount.clamp(0.0, 1.0) };
        let fz = |x: f32| if x.is_finite() { x } else { 0.0 };
        for i in 0..n {
            let b = fz(self.base[i]);
            self.pre[i] = if self.groups[i] & self.infl_mask != 0 {
                ((1.0 - a) * b + a * fz(self.influence[i])).clamp(0.0, 1.0)
            } else {
                b.clamp(0.0, 1.0)
            };
        }
        self.target[..n].copy_from_slice(&self.pre[..n]);
        self.layer_routes(true);
        self.settled = false;
        self.dirty = false;
    }

    /// The layer's sources for this tick: macros, perf, the layer LFOs (advanced one tick with
    /// their values as they stand), and the sung features of `frame`.
    fn update_sources(&mut self, frame: Option<&[f32; CURVE_STRIDE]>) {
        for k in 0..N_MACROS {
            self.lsrc[LS_MACRO + k] = if self.macros[k].is_nan() { 0.5 } else { self.macros[k] };
        }
        self.lsrc[LS_PERF..LS_PERF + 3].copy_from_slice(&self.perf);
        let dt = TICK as f32 / self.sr;
        let p = &*self.patch;
        for k in 0..MAX_LAYER_LFOS {
            self.lsrc[LS_LFO + k] = match p.layer_lfos().get(k) {
                Some(m) => {
                    let b = m.base as usize;
                    let r = &self.real;
                    let x = self.lfos[k].tick(r[b + P::lfo::RATE], r[b + P::lfo::WAVE], dt);
                    let level = r[b + P::lfo::LEVEL];
                    if level == 1.0 { x } else { x * level }
                }
                None => f32::NAN,
            };
        }
        match frame {
            Some(f) if self.lr_features || self.metered => {
                let mut x = features(f);
                if x[0].is_finite() {
                    self.held_pitch = x[0];
                } else {
                    x[0] = self.held_pitch;
                }
                self.lsrc[LS_FEATURE..LS_FEATURE + N_FEATURES].copy_from_slice(&x);
                self.feat = *f;
                self.has_feat = true;
            }
            _ => {
                self.lsrc[LS_FEATURE..LS_FEATURE + N_FEATURES].fill(f32::NAN);
                self.has_feat = false;
            }
        }
    }

    /// Step 4: the layer routes onto `target`. `full`: every value was just set from `pre`;
    /// otherwise only the values the routes write are put back first.
    fn layer_routes(&mut self, full: bool) {
        let n = self.patch.n_values;
        if !full {
            for &t in &self.lr_list[..self.lr_n] {
                self.target[t as usize] = self.pre[t as usize];
            }
        }
        let dt = TICK as f32 / self.sr;
        let fresh = std::mem::replace(&mut self.lr_fresh, false);
        let dep = depths(&self.depth, self.follow);
        let p = &*self.patch;
        for (j, r) in p.layer_routes().iter().enumerate() {
            self.lr_out[j] = f32::NAN;
            let src = |s: u16, a: u16| layer_source_index(s, a).map_or(f32::NAN, |i| self.lsrc[i]);
            let a = r.amount as usize;
            // An amount another layer route moves is read as it stands now (those routes come first).
            let amt = P::to_real(p.descs[a] as usize, self.target[a].clamp(0.0, 1.0));
            let via = if r.via == SRC_NONE { 1.0 } else { via_value(r.via_bipolar, src(r.via, r.via_arg)) };
            let depth = if r.group == GROUP_NONE { 1.0 } else { dep[r.group as usize - 1] };
            let k = amt * r.unit * via * depth;
            if !k.is_finite() {
                continue;
            }
            if r.source == SRC_LAYER_VOWEL {
                // Each band moves toward the sung vowel shape.
                if !self.has_feat {
                    continue;
                }
                self.lr_out[j] = 1.0;
                let t0 = r.targets[0] as usize;
                for b in 0..N_BANDS {
                    let v = self.feat[frame::BANDS + b];
                    if v.is_finite() {
                        let y = P::to_norm(p.descs[t0 + b] as usize, v.clamp(-30.0, 30.0));
                        self.target[t0 + b] += k * (y - self.target[t0 + b]);
                    }
                }
                continue;
            }
            let raw = src(r.source, r.arg);
            let x = if r.bipolar(&[]) { (raw + 1.0) * 0.5 } else { raw };
            if !x.is_finite() {
                continue;
            }
            let mut y = r.shape(x);
            if r.smoothed() {
                let s = &mut self.lr_smooth[j];
                if fresh || !s.is_finite() {
                    *s = y;
                } else {
                    let ms = if y > *s { r.rise_ms } else { r.fall_ms };
                    *s += if ms > 0.0 { 1.0 - (-dt * 1000.0 / ms).exp() } else { 1.0 } * (y - *s);
                }
                y = *s;
            }
            self.lr_out[j] = y;
            let c = r.polar(y);
            // A pull moves its target in its own units toward the source value.
            let toward = if r.bend == 0.0 && r.n_points < 2 { raw.clamp(r.in_lo.min(r.in_hi), r.in_lo.max(r.in_hi)) } else { r.in_lo + y * (r.in_hi - r.in_lo) };
            let width = if r.target_kind == TGT_BANDS { N_BANDS } else { 1 };
            for &t in &r.targets[..r.n_targets as usize] {
                for b in 0..width {
                    let t = t as usize + b;
                    match r.mode {
                        MODE_SCALE => self.target[t] *= 1.0 + k * c,
                        MODE_PULL => {
                            let d = p.descs[t] as usize;
                            let cur = P::to_real(d, self.target[t].clamp(0.0, 1.0));
                            let next = (cur + k * (toward - cur)).clamp(P::desc(d).min, P::desc(d).max);
                            self.target[t] = P::to_norm(d, next);
                        }
                        _ => self.target[t] += k * c,
                    }
                }
            }
        }
        if full {
            for v in self.target[..n].iter_mut() {
                *v = v.clamp(0.0, 1.0);
            }
        } else {
            for &t in &self.lr_list[..self.lr_n] {
                let t = t as usize;
                self.target[t] = self.target[t].clamp(0.0, 1.0);
            }
        }
    }

    fn map_all(&mut self) {
        for i in 0..self.patch.n_values {
            self.real[i] = P::to_real(self.patch.descs[i] as usize, self.norm[i]);
        }
    }

    /// Normalized values after step 4 (meters).
    pub fn stored(&self) -> &[f32] {
        &self.norm[..self.patch.n_values]
    }

    /// Real value `i` after step 4.
    pub fn real(&self, i: usize) -> f32 {
        self.real[i]
    }

    // ------------------------------------------------------------ notes

    /// A voice for a new note, among the first `max_voices`: an idle one, else the oldest
    /// released one, else the quietest.
    fn alloc_voice(&mut self) -> usize {
        let n = self.max_voices.clamp(1, VOICES_PER_LAYER);
        if let Some(v) = (0..n).find(|&v| !self.voices[v].active()) {
            return v;
        }
        self.steals += 1;
        let released = (0..n).filter(|&v| self.voices[v].released()).min_by_key(|&v| self.slots[v].age);
        if let Some(v) = released {
            return v;
        }
        (0..n)
            .min_by(|&a, &b| self.voices[a].loudness().total_cmp(&self.voices[b].loudness()))
            .unwrap_or(0)
    }

    /// The voice patch a kit slot plays (the only one on melody and texture layers).
    fn vp_of(&self, kit_slot: usize) -> usize {
        if self.kind == KIND_BEAT { kit_slot.min(self.patch.n_voices - 1) } else { 0 }
    }

    /// The hit-shape part of a note: beat layers only, gated by Follow where
    /// a field needs it; NaN elsewhere so its routes do nothing.
    fn shape_note(&self, note: &mut Note, e: &Event) {
        if self.kind != KIND_BEAT {
            return;
        }
        note.attack = if e.attack.is_finite() && e.attack > 0.0 { e.attack.clamp(0.001, 1.0) } else { f32::NAN };
        note.burst = e.burst;
        note.bend = e.bend;
        note.hold = if e.hold.is_finite() && e.hold > 0.0 { e.hold.clamp(0.05, 0.8) } else { f32::NAN };
        note.vowel = e.vowel;
        note.tone = e.tone;
        note.end_click = e.end_click;
        note.end_stop = e.end_stop.is_finite() && e.end_stop > 0.5 && self.follow > 0.5;
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
            curve: clip.curve,
            clip: clip_i,
            clip_t: e.t,
            clip_end: e.t + e.dur,
            default_slope,
            ..Slot::default()
        };
        let mut note = Note {
            velocity: if e.strength.is_finite() && e.strength > 0.0 { e.strength.min(1.0) } else { 1.0 },
            hard: e.hard,
            ..Note::default()
        };
        if self.kind == KIND_BEAT {
            let k = (e.slot as usize).min(KIT_SLOTS - 1);
            slot.vp = self.vp_of(k);
            let own = self.kit_cents[k];
            note.cents = if e.cents.is_finite() { own + self.follow * (e.cents - own) } else { own };
        } else {
            note.cents = if e.cents.is_finite() { e.cents } else { 4800.0 };
        }
        self.shape_note(&mut note, e);
        self.start_voice(slot, &note, track, curves, len);
    }

    /// Live trigger (MSG_NOTE_ON).
    pub fn note_on_live(&mut self, kit_slot: usize, cents: f32, velocity: f32, track: &Track, curves: &CurvePool, len: usize) {
        if self.mute {
            return;
        }
        let slot = Slot { left: u32::MAX, live_key: cents, vp: self.vp_of(kit_slot), ..Slot::default() };
        let mut note = Note { velocity: velocity.clamp(0.0, 1.0), hard: 0.0, ..Note::default() };
        if self.kind == KIND_BEAT {
            let k = kit_slot.min(KIT_SLOTS - 1);
            note.cents = if cents.is_finite() { cents } else { self.kit_cents[k] };
        } else {
            if !cents.is_finite() {
                return;
            }
            note.cents = cents;
        }
        let e = Event { hard: 0.0, ..Event::default() };
        self.shape_note(&mut note, &e);
        self.start_voice(slot, &note, track, curves, len);
        self.noise_now(slot.vp.min(self.patch.n_voices - 1), len);
    }

    /// A played note's shaped noise starts with the note, not at the next tick: the noise bank
    /// of voice patch `k` moves to its voices' level over the `len` samples left in this tick.
    /// Only for notes played live, so that song playback stays as it was.
    fn noise_now(&mut self, k: usize, len: usize) {
        if self.pending || len == 0 || len > TICK {
            return;
        }
        let Some((nb, _)) = self.patch.voices[k].noise else { return };
        let mut lvl = 0.0;
        for v in 0..VOICES_PER_LAYER {
            if self.voice_on[v] && self.slots[v].vp == k {
                lvl += self.voices[v].noise_level();
            }
        }
        let mut b = [0.0f32; N_BANDS];
        for (j, x) in b.iter_mut().enumerate() {
            *x = self.real[nb as usize + P::shaped_noise::BANDS + j];
        }
        self.noise_banks[k].retarget(lvl, &b, len);
    }

    /// Live follow: the mic starts a voice. Beat layers play a hit on
    /// `kit_slot`; melody and texture layers start the live voice that reads
    /// the frame given to `follow_set` every tick. Releases the previous
    /// follow voice. `hard` is the onset hardness.
    pub fn follow_start(&mut self, kit_slot: usize, velocity: f32, hard: f32, track: &Track, curves: &CurvePool, len: usize) {
        self.follow_release();
        if self.mute {
            return;
        }
        let slot = Slot { left: u32::MAX, follow: true, vp: self.vp_of(kit_slot), ..Slot::default() };
        let mut note = Note { velocity: velocity.clamp(0.0, 1.0), hard, follow: true, ..Note::default() };
        if self.kind == KIND_BEAT {
            note.cents = self.kit_cents[kit_slot.min(KIT_SLOTS - 1)];
        }
        let e = Event { hard, ..Event::default() };
        self.shape_note(&mut note, &e);
        self.start_voice(slot, &note, track, curves, len);
    }

    /// Release the voice Live follow started, if it still sounds.
    pub fn follow_release(&mut self) {
        self.unqueue(|s| s.follow);
        for v in 0..VOICES_PER_LAYER {
            if self.slots[v].follow && self.slots[v].left == u32::MAX {
                self.slots[v].left = 0;
                if self.voices[v].active() {
                    let vp = &self.patch.voices[self.slots[v].vp];
                    self.voices[v].release(vp);
                }
            }
        }
    }

    /// Live follow: the record and amplitude scale the live voice reads from
    /// the next tick on.
    pub fn follow_set(&mut self, rec: &[f32; CURVE_STRIDE], gain: f32) {
        self.follow_rec = *rec;
        self.follow_gain = gain;
    }

    pub fn note_off_live(&mut self, cents: f32) {
        self.unqueue(|s| !s.seq && !s.follow && s.left == u32::MAX && (s.live_key == cents || (s.live_key.is_nan() && cents.is_nan())));
        for v in 0..VOICES_PER_LAYER {
            let s = &mut self.slots[v];
            if !s.seq && !s.follow && s.left == u32::MAX && (s.live_key == cents || (s.live_key.is_nan() && cents.is_nan())) {
                s.left = 0;
                self.voices[v].release(&self.patch.voices[s.vp]);
            }
        }
    }

    pub fn release_all(&mut self) {
        self.n_queued = 0;
        for v in 0..VOICES_PER_LAYER {
            if self.voices[v].active() && !self.voices[v].released() {
                self.voices[v].release(&self.patch.voices[self.slots[v].vp]);
            }
            self.slots[v].left = 0;
        }
    }

    pub fn release_sequenced(&mut self) {
        self.unqueue(|s| s.seq);
        for v in 0..VOICES_PER_LAYER {
            if self.slots[v].seq && self.voices[v].active() && !self.voices[v].released() {
                self.voices[v].release(&self.patch.voices[self.slots[v].vp]);
                self.slots[v].left = 0;
            }
        }
    }

    pub fn panic(&mut self) {
        self.n_queued = 0;
        if self.pending {
            self.swap();
        }
        for v in self.voices.iter_mut() {
            v.kill();
        }
        self.slots = [Slot::default(); VOICES_PER_LAYER];
        self.grain.clear_grains();
        self.chan.reset_tails();
        self.reset_noise();
    }

    /// Drop the queued notes that `f` picks.
    fn unqueue(&mut self, f: impl Fn(&Slot) -> bool) {
        let mut k = 0;
        for i in 0..self.n_queued {
            if !f(&self.queued[i].0) {
                self.queued[k] = self.queued[i];
                k += 1;
            }
        }
        self.n_queued = k;
    }

    /// Fresh noise banks. A single voice patch uses the old layer bank's seed,
    /// kit slots the old kit banks' seeds.
    fn reset_noise(&mut self) {
        let (sr, seed) = (self.sr, self.seed);
        let single = self.patch.n_voices <= 1;
        self.noise_banks = std::array::from_fn(|k| ShapedNoise::new(sr, bank_seed(seed, if single { k } else { k + 1 })));
    }

    #[allow(clippy::too_many_arguments)]
    fn view<'a>(norm: &'a [f32], real: &'a [f32], descs: &'a [u16], t: &'a Touched, depth: [f32; 6], max_partials: usize, follow_gain: f32, lsrc: &'a [f32; N_LAYER_SOURCES]) -> LayerView<'a> {
        LayerView { norm, real, descs, touched: &t.list[..t.n], is_touched: &t.is, is_amount: &t.amount, from_real: &t.from_real, depth, max_partials, follow_gain, lsrc }
    }

    fn start_voice(&mut self, slot: Slot, note: &Note, track: &Track, curves: &CurvePool, len: usize) {
        if self.pending {
            // The new structure is on its way: start the note when it is taken.
            if self.n_queued < VOICES_PER_LAYER {
                self.queued[self.n_queued] = (slot, *note);
                self.n_queued += 1;
            }
            return;
        }
        let slot = Slot { vp: slot.vp.min(self.patch.n_voices - 1), ..slot };
        let v = self.alloc_voice();
        self.age += 1;
        self.slots[v] = Slot { age: self.age, ..slot };
        let mut nf = [0.0f32; CURVE_STRIDE];
        let frame = self.read_frame(v, track, curves, &mut nf);
        let k = slot.vp;
        if self.voices[v].vp != k {
            self.voices[v].bind(&self.patch.voices[k], k);
        }
        let lv = Self::view(&self.norm[..], &self.real[..], &self.patch.descs, &self.touched[k], depths(&self.depth, self.follow), self.max_partials[v], self.follow_gain, &self.lsrc);
        self.voices[v].start(&self.patch.voices[k], &lv, note, frame.then_some(&nf), len);
        self.grain.voice_start(v);
        self.voice_on[v] = true;
        self.layer_on = true;
    }

    // ------------------------------------------------------------ control rate

    /// Per tick, before the voices: the layer's sources, steps 3-4 when dirty (step 4 every
    /// tick while a layer route's source moves by itself), then smoothing and the channel.
    /// `frame`: the layer's sung features this tick, if it has any.
    pub fn control_tick(&mut self, frame: Option<&[f32; CURVE_STRIDE]>) {
        if self.pending && self.fade_left == 0 {
            self.swap();
        }
        self.update_sources(frame);
        // During a fade the old patch plays on its values as they stand; the new values wait.
        if !self.pending {
            self.advance_values();
        }
        let p = &*self.patch;
        let r = &self.real;
        let (mut mono, mut stereo_filter) = (false, false);
        let ch = &mut self.chan;
        for (c, fx) in p.channel().iter().enumerate() {
            if fx.bypass {
                continue;
            }
            let (b, j) = (fx.base as usize, ch.inst[c] as usize);
            match fx.code {
                P::T_CRUSH => {
                    let (bits, rate) = (r[b + P::crush::BITS], r[b + P::crush::RATE]);
                    ch.crush[j].set(bits, rate);
                    mono |= bits < 15.99 || rate < self.sr * 0.999;
                }
                P::T_TAPE => {
                    let amt = r[b + P::tape::AMOUNT];
                    ch.tape[j].set(amt);
                    mono |= amt > 0.0;
                }
                P::T_DELAY => ch.delay[j].set(r[b + P::delay::TIME], r[b + P::delay::FEEDBACK], r[b + P::delay::SHIFT], r[b + P::delay::MIX]),
                P::T_REVERB => ch.reverb[j].set(r[b + P::reverb::SIZE], r[b + P::reverb::DAMP], r[b + P::reverb::MIX], r[b + P::reverb::FREEZE]),
                P::T_CFILTER => {
                    let f = &mut ch.filter[j];
                    f.set(self.sr, r[b + P::cfilter::CUTOFF], r[b + P::cfilter::RES], r[b + P::cfilter::MORPH]);
                    if f.on {
                        if c < ch.split {
                            mono = true;
                        } else {
                            stereo_filter = true;
                        }
                    }
                }
                _ => {}
            }
        }
        ch.stereo_filter = stereo_filter;
        self.mono_on = mono;
        let lvl = if self.mute { 0.0 } else { dsp::db_to_lin(self.level_db) };
        let a = (self.pan + 1.0) * std::f32::consts::FRAC_PI_4;
        self.gl_t = lvl * a.cos();
        self.gr_t = lvl * a.sin();
        // Fixed per tick so the ramp does not depend on how the tick is split.
        self.gl_step = (self.gl_t - self.gl) / TICK as f32;
        self.gr_step = (self.gr_t - self.gr) / TICK as f32;
    }

    /// Steps 3 and 4 when needed, then the per-tick smoothing and mapping of the values.
    fn advance_values(&mut self) {
        if self.dirty {
            self.resolve();
        } else if self.lr_dynamic {
            self.layer_routes(false);
        }
        let n = self.patch.n_values;
        if !self.settled {
            let mut moving = false;
            for i in 0..n {
                let d = self.target[i] - self.norm[i];
                if d.abs() > 1e-6 {
                    self.norm[i] += self.smooth[i] * d;
                    moving = true;
                } else {
                    self.norm[i] = self.target[i];
                }
            }
            self.map_all();
            self.settled = !moving;
        } else if self.lr_dynamic {
            // Settled apart from what the layer routes move: smooth and map only those.
            for x in 0..self.lr_n {
                let i = self.lr_list[x] as usize;
                let d = self.target[i] - self.norm[i];
                if d.abs() > 1e-6 {
                    self.norm[i] += self.smooth[i] * d;
                } else {
                    self.norm[i] = self.target[i];
                }
                self.real[i] = P::to_real(self.patch.descs[i] as usize, self.norm[i]);
            }
        }
    }

    /// Read voice `v`'s curve frame at its clip time (held after the note's
    /// end). False when it has none.
    fn read_frame(&self, v: usize, track: &Track, curves: &CurvePool, out: &mut [f32; CURVE_STRIDE]) -> bool {
        let s = &self.slots[v];
        if s.follow {
            if self.kind == KIND_BEAT {
                return false;
            }
            *out = self.follow_rec;
            return true;
        }
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
        let mut noise_lvl = [0.0f32; KIT_SLOTS];
        let mut loudest = (0.0f32, usize::MAX);
        let mut nf = [0.0f32; CURVE_STRIDE];
        let dep = depths(&self.depth, self.follow);
        for v in 0..VOICES_PER_LAYER {
            self.voice_on[v] = self.voices[v].active();
            if !self.voice_on[v] {
                continue;
            }
            let frame = self.read_frame(v, track, curves, &mut nf);
            let k = self.slots[v].vp;
            let lv = Self::view(&self.norm[..], &self.real[..], &self.patch.descs, &self.touched[k], dep, self.max_partials[v], self.follow_gain, &self.lsrc);
            self.voices[v].control(&self.patch.voices[k], &lv, frame.then_some(&nf), TICK);
            self.slots[v].clip_t += units_per_sample * TICK as f64;
            noise_lvl[k] += self.voices[v].noise_level();
            let l = self.voices[v].loudness();
            if l > loudest.0 {
                loudest = (l, v);
            }
        }
        // Meters (the selected layer only): the loudest voice's values over the layer's.
        self.meter_voice = loudest.1;
        let n = self.patch.n_values;
        if self.metered {
            self.live[..n].copy_from_slice(&self.norm[..n]);
        }
        if self.metered && loudest.1 != usize::MAX {
            let v = loudest.1;
            let t = &self.touched[self.slots[v].vp];
            for &i in &t.list[..t.n] {
                if let Some(x) = self.voices[v].norm(i as usize) {
                    self.live[i as usize] = x;
                }
            }
        }
        for (k, vp) in self.patch.voices().iter().enumerate() {
            let Some((nb, _)) = vp.noise else { continue };
            let mut b = [0.0f32; N_BANDS];
            for (j, x) in b.iter_mut().enumerate() {
                *x = self.real[nb as usize + P::shaped_noise::BANDS + j];
            }
            self.noise_banks[k].control(noise_lvl[k], &b, TICK);
        }
        if self.grain.wants() {
            let mut vg = [VoiceGrain::default(); VOICES_PER_LAYER];
            for v in 0..VOICES_PER_LAYER {
                if !self.voice_on[v] {
                    continue;
                }
                let k = self.slots[v].vp;
                let Some((gb, _, _)) = self.patch.voices[k].grains else { continue };
                let val = |i: usize| self.voices[v].real(i).unwrap_or(self.real[i]);
                let gb = gb as usize;
                vg[v] = VoiceGrain {
                    on: true,
                    amp: self.voices[v].grain_level(),
                    size: val(gb + P::grains::SIZE),
                    density: val(gb + P::grains::DENSITY),
                    pos: val(gb + P::grains::POS),
                    spray: val(gb + P::grains::SPRAY),
                    scatter: val(gb + P::grains::SCATTER),
                    cents: self.voices[v].pitch_cents(),
                };
            }
            self.grain.tick(&vg);
        }
        self.stereo_on = self.chan.stereo_active(&self.patch);
        let gain_moving = (self.gl - self.gl_t).abs() > 1e-6 || (self.gr - self.gr_t).abs() > 1e-6;
        self.layer_on = self.voice_on.iter().any(|&a| a)
            || self.noise_banks.iter().any(|n| n.on)
            || self.grain.active()
            || self.stereo_on
            || gain_moving;
        // Notes that waited for a new structure start now, as if at the start of this tick.
        if !self.pending && self.n_queued > 0 {
            let n = std::mem::replace(&mut self.n_queued, 0);
            for i in 0..n {
                let (slot, note) = self.queued[i];
                self.start_voice(slot, &note, track, curves, TICK);
            }
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
                self.voices[v].release(&self.patch.voices[s.vp]);
            }
        }
    }

    /// Render `n` samples (n <= TICK) of this layer, adding into `l` and `r`.
    pub fn render(&mut self, l: &mut [f32], r: &mut [f32], pool: &SamplePool) {
        let n = l.len();
        if !self.layer_on {
            return;
        }
        let bus = &mut self.bus[..n];
        bus.fill(0.0);
        for (v, on) in self.voices.iter_mut().zip(self.voice_on) {
            if on {
                v.render(&self.patch.voices[v.vp], bus, pool);
            }
        }
        for nb in self.noise_banks.iter_mut() {
            nb.render(bus);
        }
        self.grain.render(bus, pool);
        let p = &*self.patch;
        let (dl, dr) = (self.gl_step, self.gr_step);
        let w = self.real[p.width as usize];
        let (mono, stereo) = (self.mono_on, self.stereo_on);
        let ch = &mut self.chan;
        let split = ch.split;
        let mut peak = 0.0f32;
        for i in 0..n {
            let mut x = bus[i];
            if mono {
                for (c, fx) in p.channel()[..split].iter().enumerate() {
                    if fx.bypass {
                        continue;
                    }
                    let j = ch.inst[c] as usize;
                    match fx.code {
                        P::T_CRUSH => x = ch.crush[j].process(x),
                        P::T_TAPE => x = ch.tape[j].process(x),
                        P::T_CFILTER if ch.filter[j].on => x = ch.filter[j].process(0, x),
                        _ => {}
                    }
                }
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
            if stereo {
                for (c, fx) in p.channel().iter().enumerate().skip(split) {
                    if fx.bypass {
                        continue;
                    }
                    let j = ch.inst[c] as usize;
                    match fx.code {
                        P::T_DELAY => (a, b) = ch.delay[j].process(a, b),
                        P::T_REVERB => (a, b) = ch.reverb[j].process(a, b),
                        P::T_CFILTER if ch.filter[j].on => {
                            let f = &mut ch.filter[j];
                            (a, b) = (f.process(0, a), f.process(1, b));
                        }
                        _ => {}
                    }
                }
            }
            (a, b) = width(a, b, w);
            if self.pending {
                // The structure-change fade: linear to silence, then silence until the swap.
                if self.fade_left > 0 {
                    self.fade_left -= 1;
                    self.fade_g = self.fade_left as f32 / self.fade_len as f32;
                }
                a *= self.fade_g;
                b *= self.fade_g;
            }
            peak = peak.max(a.abs()).max(b.abs());
            l[i] += a;
            r[i] += b;
        }
        self.peak = self.peak.max(peak);
    }

    /// The layer's source values this tick (macros, perf, layer LFOs, features; patch.rs LS_*).
    pub fn layer_sources(&self) -> &[f32; N_LAYER_SOURCES] {
        &self.lsrc
    }

    /// Wants its sung features each tick: a layer route reads them, or the meters show them.
    pub fn wants_features(&self) -> bool {
        self.lr_features || self.metered
    }

    /// Route outputs for the meters: voice patch k's route i at k * MAX_ROUTES + i (from the
    /// loudest sounding voice), then the layer routes. NaN where a route had no value.
    pub fn route_meters(&self, out: &mut [f32]) {
        out.fill(f32::NAN);
        let v = self.meter_voice;
        if v < VOICES_PER_LAYER && self.voices[v].active() {
            let k = self.slots[v].vp;
            for i in 0..self.patch.voices[k].n_routes {
                out[k * MAX_ROUTES + i] = self.voices[v].route_out(i);
            }
        }
        let n = self.patch.n_layer_routes;
        out[KIT_SLOTS * MAX_ROUTES..KIT_SLOTS * MAX_ROUTES + n].copy_from_slice(&self.lr_out[..n]);
    }

    /// MSG_SET_PERF: 0 mod wheel (0..1), 1 pitch bend (-1..1), 2 channel pressure (0..1).
    pub fn set_perf(&mut self, k: usize, v: f32) {
        if k < 3 && v.is_finite() {
            self.perf[k] = if k == 1 { v.clamp(-1.0, 1.0) } else { v.clamp(0.0, 1.0) };
        }
    }

    pub fn sounding(&self) -> usize {
        self.voices.iter().filter(|v| v.active()).count()
    }

    /// Macro positions with the voices' movement projected back onto each
    /// macro through its layer routes (TDD "What the sliders show").
    pub fn macros_live(&self, out: &mut [f32; N_MACROS]) {
        let p = &*self.patch;
        let mut dot = [0.0f32; N_MACROS];
        let mut n2 = [0.0f32; N_MACROS];
        let fz = |x: f32| if x.is_finite() { x } else { 0.0 };
        for r in p.layer_routes() {
            let k = r.arg as usize;
            let w = P::to_real(p.descs[r.amount as usize] as usize, self.base[r.amount as usize]) * r.unit;
            for &t in &r.targets[..r.n_targets as usize] {
                let t = t as usize;
                dot[k] += w * (fz(self.live[t]) - fz(self.norm[t]));
                n2[k] += w * w;
            }
        }
        for k in 0..N_MACROS {
            let u = if self.macros[k].is_nan() { 0.5 } else { self.macros[k].clamp(0.0, 1.0) };
            out[k] = if n2[k] > 0.0 { (u + dot[k] / (2.0 * n2[k])).clamp(0.0, 1.0) } else { u };
        }
    }
}
