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
//!    Without a marker (or clip) the engine asks for state 0. A layer that
//!    never got states keeps what `LOAD_LAYER` and live messages set. A new
//!    patch starts every value at the patch's starting value.
//! 2. Automation: arrives with recording (stage S5).
//! 3. Influence: each value whose group bit is in the mask moves toward the
//!    target by the amount.
//! 4. Layer routes (the macros): added, then clamped. Steps 3 and 4 run when
//!    a stored value changes; every value is then smoothed per control tick
//!    with its descriptor's `smooth_ms`, in the normalized domain, and mapped
//!    to real units. Channel values are finished here.
//! 5 to 7 run per voice (`voice.rs`).
//!
//! The channel is the patch's effect chain: the effects before the first
//! stereo one run on the mono sum of the voices, level and pan then spread it
//! to stereo, the stereo effects follow, and width is last (today's order).

use crate::curve::{self, CurvePool, CURVE_STRIDE};
use crate::grain::{GrainCloud, VoiceGrain};
use crate::patch::*;
use crate::protocol::*;
use crate::samples::SamplePool;
use crate::seq::{Clip, Event, Track};
use crate::voice::{LayerView, Note, Voice, N_BANDS, TICK};
use dsp::bands::{band_centres, band_q};
use dsp::filters::Svf;
use dsp::frame;
use dsp::layer_fx::{width, Crush, Reverb, StereoDelay, Tape};
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

/// The channel's effect instances (the patch's chain uses at most one of each in this stage).
struct Channel {
    crush: Crush,
    tape: Tape,
    delay: StereoDelay,
    reverb: Reverb,
}

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
}

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
                crush: Crush::new(sr, seed ^ 0x27d4_eb2f),
                tape: Tape::new(sr, seed ^ 0x1656_67b1),
                delay: StereoDelay::new(sr),
                reverb: Reverb::new(sr, seed ^ 0xc2b2_ae35),
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
            grain: GrainCloud::new(sr, seed ^ 0x7f4a_7c15),
            max_partials: [crate::voice::MAX_PARTIALS; VOICES_PER_LAYER],
            age: 0,
            live: Box::new([0.0; MAX_VALUES]),
            metered: index == 0,
            follow_rec: [0.0; CURVE_STRIDE],
            follow_gain: 0.0,
        };
        let classic = crate::classic::patch_blob(KIND_MELODY, index as u32);
        let mut p = Patch::new_boxed();
        p.parse(&classic).expect("the built-in classic patch is valid");
        l.set_patch(&p);
        l
    }

    // ------------------------------------------------------------ patch

    pub fn patch(&self) -> &Patch {
        &self.patch
    }

    pub fn n_values(&self) -> usize {
        self.patch.n_values
    }

    /// Replace the patch (LOAD_PATCH). An identical patch changes nothing.
    /// Otherwise every voice is silenced, the values return to the patch's starting values and
    /// the channel's tails are cleared.
    pub fn set_patch(&mut self, p: &Patch) {
        if *self.patch == *p {
            return;
        }
        (*self.patch).clone_from(p);
        for v in 0..VOICES_PER_LAYER {
            self.voices[v].kill();
        }
        self.slots = [Slot::default(); VOICES_PER_LAYER];
        let n = p.n_values;
        let dt = TICK as f32 / self.sr;
        for i in 0..n {
            let d = p.descs[i] as usize;
            self.base[i] = p.init[i];
            self.influence[i] = p.init[i];
            // Until a layer blob says otherwise, Influence moves a value by its descriptor's group.
            self.groups[i] = 1 << (P::desc(d).group as u32);
            let ms = P::desc(d).smooth_ms;
            self.smooth[i] = if ms > 0.0 { 1.0 - (-dt * 1000.0 / ms).exp() } else { 1.0 };
        }
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
        for v in 0..VOICES_PER_LAYER {
            self.voices[v].bind(&p.voices[0], 0);
            self.voices[v].kill();
        }
        self.reset_noise();
        self.n_states = 0;
        self.morph_applied = None;
        self.grain.clear_grains();
        self.chan.delay.reset();
        self.chan.reverb.reset();
        self.resolve();
        self.norm[..n].copy_from_slice(&self.target[..n]);
        self.map_all();
        self.settled = true;
    }

    /// Grain sample id of the patch (its first voice patch with grains), if any.
    pub fn grain_sample(&self) -> Option<usize> {
        self.patch.voices().iter().find_map(|vp| vp.grains.map(|g| g.2 as usize))
    }

    // ------------------------------------------------------------ stored sound

    pub fn set_param(&mut self, set: u8, i: usize, v: f32) {
        if i >= self.patch.n_values || !v.is_finite() {
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
        let n = self.patch.n_values;
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
        let nv = self.patch.n_values;
        let stride = nv + N_MACROS + 1;
        let clean = |x: f32, d: f32| if x.is_finite() { x.clamp(0.0, 1.0) } else { d };
        for (k, st) in w.chunks_exact(stride).take(n.min(MAX_STATES)).enumerate() {
            for i in 0..nv {
                self.states[k * MAX_VALUES + i] = clean(st[i], self.patch.init[i]);
            }
            for m in 0..N_MACROS {
                self.state_macros[k][m] = clean(st[nv + m], 0.5);
            }
            self.state_amount[k] = clean(st[nv + N_MACROS], 0.0);
        }
        self.n_states = n.min(MAX_STATES);
        self.morph_applied = None;
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
        let (a, b) = (a as usize, b as usize);
        // Exact at the ends, so a settled morph sits on the state's own values.
        let mix = |p: f32, q: f32| if x <= 0.0 { p } else if x >= 1.0 { q } else { p + (q - p) * x };
        for i in 0..self.patch.n_values {
            self.base[i] = mix(self.states[a * MAX_VALUES + i], self.states[b * MAX_VALUES + i]);
        }
        for m in 0..N_MACROS {
            self.macros[m] = mix(self.state_macros[a][m], self.state_macros[b][m]);
        }
        self.infl_amount = mix(self.state_amount[a], self.state_amount[b]);
        self.dirty = true;
    }

    /// Steps 3 and 4.
    fn resolve(&mut self) {
        let n = self.patch.n_values;
        let a = if self.infl_amount.is_nan() { 0.0 } else { self.infl_amount.clamp(0.0, 1.0) };
        let fz = |x: f32| if x.is_finite() { x } else { 0.0 };
        for i in 0..n {
            let b = fz(self.base[i]);
            self.target[i] = if self.groups[i] & self.infl_mask != 0 {
                ((1.0 - a) * b + a * fz(self.influence[i])).clamp(0.0, 1.0)
            } else {
                b.clamp(0.0, 1.0)
            };
        }
        let p = &*self.patch;
        for r in p.layer_routes() {
            let u = if self.macros[r.arg as usize].is_nan() { 0.5 } else { self.macros[r.arg as usize] };
            let c = r.polar(r.shape(u));
            let amt = P::to_real(p.descs[r.amount as usize] as usize, self.base[r.amount as usize]);
            let via = if r.via == SRC_NONE { 1.0 } else { self.macros[r.via_arg as usize].clamp(0.0, 1.0) };
            let k = amt * r.unit * via;
            for &t in &r.targets[..r.n_targets as usize] {
                let t = t as usize;
                if r.mode == MODE_SCALE {
                    self.target[t] *= 1.0 + k * c;
                } else {
                    self.target[t] += k * c;
                }
            }
        }
        for v in self.target[..n].iter_mut() {
            *v = v.clamp(0.0, 1.0);
        }
        self.settled = false;
        self.dirty = false;
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
        for v in 0..VOICES_PER_LAYER {
            let s = &mut self.slots[v];
            if !s.seq && !s.follow && s.left == u32::MAX && (s.live_key == cents || (s.live_key.is_nan() && cents.is_nan())) {
                s.left = 0;
                self.voices[v].release(&self.patch.voices[s.vp]);
            }
        }
    }

    pub fn release_all(&mut self) {
        for v in 0..VOICES_PER_LAYER {
            if self.voices[v].active() && !self.voices[v].released() {
                self.voices[v].release(&self.patch.voices[self.slots[v].vp]);
            }
            self.slots[v].left = 0;
        }
    }

    pub fn release_sequenced(&mut self) {
        for v in 0..VOICES_PER_LAYER {
            if self.slots[v].seq && self.voices[v].active() && !self.voices[v].released() {
                self.voices[v].release(&self.patch.voices[self.slots[v].vp]);
                self.slots[v].left = 0;
            }
        }
    }

    pub fn panic(&mut self) {
        for v in self.voices.iter_mut() {
            v.kill();
        }
        self.slots = [Slot::default(); VOICES_PER_LAYER];
        self.grain.clear_grains();
        self.chan.delay.reset();
        self.chan.reverb.reset();
        self.reset_noise();
    }

    /// Fresh noise banks. A single voice patch uses the old layer bank's seed,
    /// kit slots the old kit banks' seeds.
    fn reset_noise(&mut self) {
        let (sr, seed) = (self.sr, self.seed);
        let single = self.patch.n_voices <= 1;
        self.noise_banks = std::array::from_fn(|k| ShapedNoise::new(sr, bank_seed(seed, if single { k } else { k + 1 })));
    }

    fn view<'a>(norm: &'a [f32], real: &'a [f32], descs: &'a [u16], t: &'a Touched, depth: [f32; 6], max_partials: usize, follow_gain: f32) -> LayerView<'a> {
        LayerView { norm, real, descs, touched: &t.list[..t.n], is_touched: &t.is, is_amount: &t.amount, from_real: &t.from_real, depth, max_partials, follow_gain }
    }

    fn start_voice(&mut self, slot: Slot, note: &Note, track: &Track, curves: &CurvePool, len: usize) {
        let v = self.alloc_voice();
        self.age += 1;
        self.slots[v] = Slot { age: self.age, ..slot };
        let mut nf = [0.0f32; CURVE_STRIDE];
        let frame = self.read_frame(v, track, curves, &mut nf);
        let k = slot.vp;
        if self.voices[v].vp != k {
            self.voices[v].bind(&self.patch.voices[k], k);
        }
        let lv = Self::view(&self.norm[..], &self.real[..], &self.patch.descs, &self.touched[k], depths(&self.depth, self.follow), self.max_partials[v], self.follow_gain);
        self.voices[v].start(&self.patch.voices[k], &lv, note, frame.then_some(&nf), len);
        self.grain.voice_start(v);
        self.voice_on[v] = true;
        self.layer_on = true;
    }

    // ------------------------------------------------------------ control rate

    /// Per tick, before the voices: steps 3-4 when dirty, then smoothing and the channel.
    pub fn control_tick(&mut self) {
        if self.dirty {
            self.resolve();
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
        }
        let p = &*self.patch;
        let r = &self.real;
        let (mut mono, mut stereo) = (false, false);
        for fx in p.channel() {
            let b = fx.base as usize;
            match fx.code {
                P::T_CRUSH => {
                    let (bits, rate) = (r[b + P::crush::BITS], r[b + P::crush::RATE]);
                    self.chan.crush.set(bits, rate);
                    mono |= bits < 15.99 || rate < self.sr * 0.999;
                }
                P::T_TAPE => {
                    let amt = r[b + P::tape::AMOUNT];
                    self.chan.tape.set(amt);
                    mono |= amt > 0.0;
                }
                P::T_DELAY => {
                    self.chan.delay.set(r[b + P::delay::TIME], r[b + P::delay::FEEDBACK], r[b + P::delay::SHIFT], r[b + P::delay::MIX]);
                    stereo |= self.chan.delay.active();
                }
                P::T_REVERB => {
                    self.chan.reverb.set(r[b + P::reverb::SIZE], r[b + P::reverb::DAMP], r[b + P::reverb::MIX], r[b + P::reverb::FREEZE]);
                    stereo |= self.chan.reverb.active();
                }
                _ => {}
            }
        }
        self.mono_on = mono;
        let _ = stereo;
        let lvl = if self.mute { 0.0 } else { dsp::db_to_lin(self.level_db) };
        let a = (self.pan + 1.0) * std::f32::consts::FRAC_PI_4;
        self.gl_t = lvl * a.cos();
        self.gr_t = lvl * a.sin();
        // Fixed per tick so the ramp does not depend on how the tick is split.
        self.gl_step = (self.gl_t - self.gl) / TICK as f32;
        self.gr_step = (self.gr_t - self.gr) / TICK as f32;
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
            let lv = Self::view(&self.norm[..], &self.real[..], &self.patch.descs, &self.touched[k], dep, self.max_partials[v], self.follow_gain);
            self.voices[v].control(&self.patch.voices[k], &lv, frame.then_some(&nf), TICK);
            self.slots[v].clip_t += units_per_sample * TICK as f64;
            noise_lvl[k] += self.voices[v].noise_level();
            let l = self.voices[v].loudness();
            if l > loudest.0 {
                loudest = (l, v);
            }
        }
        // Meters (the selected layer only): the loudest voice's values over the layer's.
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
        self.stereo_on = self.chan.delay.active() || self.chan.reverb.active();
        let gain_moving = (self.gl - self.gl_t).abs() > 1e-6 || (self.gr - self.gr_t).abs() > 1e-6;
        self.layer_on = self.voice_on.iter().any(|&a| a)
            || self.noise_banks.iter().any(|n| n.on)
            || self.grain.active()
            || self.stereo_on
            || gain_moving;
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
                v.render(&self.patch.voices[v.vp], bus);
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
        let mut peak = 0.0f32;
        for i in 0..n {
            let mut x = bus[i];
            if mono {
                for fx in p.channel() {
                    match fx.code {
                        P::T_CRUSH => x = self.chan.crush.process(x),
                        P::T_TAPE => x = self.chan.tape.process(x),
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
                for fx in p.channel() {
                    match fx.code {
                        P::T_DELAY => (a, b) = self.chan.delay.process(a, b),
                        P::T_REVERB => (a, b) = self.chan.reverb.process(a, b),
                        _ => {}
                    }
                }
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
