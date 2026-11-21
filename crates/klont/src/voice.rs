//! One voice: it runs its voice patch's program (docs/synth.md "Inside a
//! voice"). At note start the voice is given the patch and the note; at every
//! control tick it advances its modulation sources, evaluates its routes on
//! top of the layer's resolved values and hands each module its controls; in
//! between it renders spans that never cross a tick, running the modules in
//! program order, each over the whole span.
//!
//! Resolution steps 5 to 7 happen here: the voice starts from the layer's
//! values (step 5), adds its per-voice routes, then applies pull routes (step
//! 6), clamps, maps to real units and multiplies by scale routes (step 7).
//! A pull moves its target in the target's real units toward the source
//! value (the raw value within in_lo..in_hi, or the curve's output spread
//! across that range), as today's hit-shape maps did in seconds; a latched
//! pull holds the value it set at note start. A route into another route's
//! amount adds to that amount in its own units, by the source value itself
//! (the hit's bend deepens the pitch envelope in semitones, as it did).
//! Two of today's rules are fixed rules of the voice, not routes: the voicing
//! swap (unvoiced frames trade the pitched level for noise, silent frames
//! mute the voice's amps and layer sources) and the end stop of a hit.
//!
//! Control rate: the layer calls `control` at every global tick boundary
//! (every TICK samples) and `start` at any sample. Between calls `render` is
//! called on consecutive sub-spans whose lengths add up to the `len` given
//! to the last `control`/`start`. Amplitudes ramp linearly across that `len`,
//! so a note can start mid-tick and still ramp to the next boundary.
//! The output must not depend on how a span is split into `render` calls.

use crate::curve::{CURVE_STRIDE, TUNED};
use crate::modules::{Env, EnvEvent, EnvTimes, LfoSrc, Partials, PartialsCtl, Stage, cl};
use crate::patch::*;
use dsp::filters::Svf;
use dsp::fx::{Clang, Click, CombPair, Folder, fast_exp2, fast_log2};
use dsp::{C0_HZ, DENORMAL, frame};
use params as P;

/// Samples per control tick.
pub const TICK: usize = 32;
pub const MAX_PARTIALS: usize = 64;
pub const N_BANDS: usize = 16;

const CHOKE_S: f32 = 0.003;
/// Output peak below this (-100 dB) counts as silent.
const SILENT_PEAK: f32 = 1e-5;
const OUT_LIMIT: f32 = 32.0;
/// Voicing crossfade (TDD "Vowel shape: the vocoder path").
const VOICE_XFADE_S: f32 = 0.010;
/// A stored hold at or below this counts as none, whatever routes add.
const HOLD_OFF_S: f32 = 0.0006;
/// `buf_of` mark for a module whose output is silent this span.
const SILENT: u8 = u8::MAX;
/// Seeds of the four LFOs' drift sequences: the third keeps the old chaos seed.
const LFO_SALT: [u32; MAX_LFOS] = [0x1b87_3593, 0xcc9e_2d51, 0, 0x85eb_ca6b];

/// Intensity bands (of the 16 from 80 Hz to 12 kHz): about 300 to 800 Hz
/// (centres 304, 425, 594 Hz) and about 2 to 6 kHz (2.3, 3.2, 4.4, 6.2 kHz).
const INTENSITY_LO: std::ops::RangeInclusive<usize> = 4..=6;
const INTENSITY_HI: std::ops::RangeInclusive<usize> = 10..=13;
/// The tilt difference that spans the whole 0..1 range, in dB either side of 0.5.
const INTENSITY_DB: f32 = 10.0;

/// Vocal effort from a frame's bands (docs/synth.md "The sung features"):
/// the mean level of the 2 to 6 kHz bands minus that of the 300 to 800 Hz
/// bands, -10..+10 dB mapped to 0..1. The bands already have the singer's
/// average removed, so 0.5 is the singer's usual tilt. NaN without bands.
pub fn intensity(bands: &[f32]) -> f32 {
    let mean = |r: std::ops::RangeInclusive<usize>| {
        let n = (r.end() - r.start() + 1) as f32;
        r.map(|i| bands[i]).sum::<f32>() / n
    };
    let d = mean(INTENSITY_HI) - mean(INTENSITY_LO);
    if d.is_finite() { (0.5 + d / (2.0 * INTENSITY_DB)).clamp(0.0, 1.0) } else { f32::NAN }
}

/// The note a voice plays, fixed for its life. Hit-shape fields are NaN when
/// the note has none (melody notes, live notes).
#[derive(Clone, Copy, Debug)]
pub struct Note {
    /// Pitch in cents above C0 (a beat hit's resolved pitch).
    pub cents: f32,
    pub velocity: f32,
    pub hard: f32,
    pub attack: f32,
    pub burst: f32,
    pub bend: f32,
    pub hold: f32,
    pub vowel: f32,
    pub tone: f32,
    pub end_click: f32,
    /// Hit shape endStop (already gated by Follow).
    pub end_stop: bool,
    /// Started by Live follow: the gate gain scales it.
    pub follow: bool,
}

impl Default for Note {
    fn default() -> Self {
        Note {
            cents: 4800.0,
            velocity: 1.0,
            hard: 0.0,
            attack: f32::NAN,
            burst: f32::NAN,
            bend: f32::NAN,
            hold: f32::NAN,
            vowel: f32::NAN,
            tone: f32::NAN,
            end_click: f32::NAN,
            end_stop: false,
            follow: false,
        }
    }
}

/// What the layer gives a voice each tick.
pub struct LayerView<'a> {
    /// Resolved values after step 4, normalized and in real units.
    pub norm: &'a [f32],
    pub real: &'a [f32],
    pub descs: &'a [u16],
    /// Values a voice route or the voicing swap of this voice patch writes.
    pub touched: &'a [u16],
    pub is_touched: &'a [bool],
    /// Values that are the amount of one of this voice patch's routes.
    pub is_amount: &'a [bool],
    /// Values only note-start routes move: they start from their real value
    /// mapped back, as the old note-start maps did.
    pub from_real: &'a [bool],
    /// Group depths: loud, open, hard, breath, mouth, then follow.
    pub depth: [f32; 6],
    pub max_partials: usize,
    /// Live follow gate gain for the follow voice.
    pub follow_gain: f32,
    /// The layer's source values this tick (macros, perf, layer LFOs, layer features), for vias.
    pub lsrc: &'a [f32; N_LAYER_SOURCES],
}

/// Value `i` in real units for a voice: its own when a route moved it this tick, else the layer's.
#[inline]
fn val_of(moved: &[bool], rv: &[f32], real: &[f32], i: usize) -> f32 {
    if moved[i] { rv[i] } else { real[i] }
}

// ------------------------------------------------------------------ processors

struct ClickMod {
    s: Click,
    e: Click,
    end_post: bool,
    /// End click amount (fixed at note start) and the colour of the last tick.
    end: f32,
    colour: f32,
}

struct FilterMod {
    svf: Svf,
    key: (u32, u32),
    on: bool,
    w: [f32; 3],
}

struct Chain<T> {
    fx: T,
    on: bool,
    dirty: bool,
}

#[derive(Clone, Copy)]
struct AmpMod {
    gain: f32,
    step: f32,
    target: f32,
}

/// Which pool instance a program module uses.
#[derive(Clone, Copy, Debug, Default)]
enum Inst {
    #[default]
    None,
    Partials,
    Click,
    Filter(u8),
    Amp(u8),
    Fold,
    Clang,
    Comb,
}

/// Raw per-voice source values: note fields (SRC_NOTE_* order), sung
/// features (`feature_index` order) and the layer's sources (vias only); NaN when absent.
struct Sources {
    note: [f32; 10],
    voice: [f32; N_FEATURES],
    layer: [f32; N_LAYER_SOURCES],
}

pub struct Voice {
    sr: f32,
    partials: Partials,
    click: ClickMod,
    filters: [FilterMod; 2],
    amps: [AmpMod; 4],
    fold: Chain<Folder>,
    clang: Chain<Clang>,
    comb: Chain<CombPair>,
    inst: [Inst; MAX_MODULES],
    /// Program module whose buffer a module's output is (a fused amp reads the bank's).
    alias: [u8; MAX_MODULES],
    bufs: [[f32; TICK]; MAX_MODULES],
    /// The buffer holding each module's output this span (its own, or a passed-on input).
    buf_of: [u8; MAX_MODULES],
    inb: [f32; TICK],

    envs: [Env; MAX_ENVS],
    lfos: [LfoSrc; MAX_LFOS],
    /// Env or LFO index of each mod, and its current value (LFOs: times level, bipolar).
    mod_slot: [u8; MAX_MODS],
    mod_val: [f32; MAX_MODS],
    /// Index of the voice patch in the layer's patch.
    pub vp: usize,

    /// Voice values: normalized after routes, real units after scale routes
    /// (only the touched values of the voice patch are kept here).
    nv: Box<[f32; MAX_VALUES]>,
    rv: Box<[f32; MAX_VALUES]>,
    scale: Box<[f32; MAX_VALUES]>,
    /// Real value a pull route left (kept exact rather than mapped back from `nv`).
    pulled: Box<[f32; MAX_VALUES]>,
    /// What routes add to a route amount, in its real units.
    amt_add: Box<[f32; MAX_VALUES]>,
    /// Octaves routes move a log-scaled value by after mapping.
    oct: Box<[f32; MAX_VALUES]>,
    /// A route moved the value this tick (otherwise it keeps the layer's value exactly).
    moved: Box<[bool; MAX_VALUES]>,
    /// The values moved this tick.
    dirty: Box<[u16; MAX_VALUES]>,
    n_dirty: usize,
    /// Routes that can move anything for this note, in order (a note field the note lacks, or a
    /// note source that sits at the shaper's zero, never will).
    act: [u8; MAX_ROUTES],
    n_act: usize,
    smooth: [f32; MAX_ROUTES],
    latched: [f32; MAX_ROUTES],
    latched_raw: [f32; MAX_ROUTES],
    /// What each latched pull route set its targets to at note start (real units).
    held: [[f32; MAX_TARGETS]; MAX_ROUTES],
    /// Set while `start` resolves: latched pulls record what they set.
    holding: bool,
    /// Each route's shaped output y (0..1) this tick, NaN when it had no value (meters).
    out: [f32; MAX_ROUTES],
    /// The frame's onset at the last tick (onset-triggered envelopes restart on a rising one).
    onset_prev: f32,

    note: Note,
    note_src: [f32; 10],
    note_t: f32,
    cents_now: f32,
    vmix: f32,
    gate: f32,
    /// Per tick, before the choke gain: shaped-noise and grain levels, and loudness.
    noise_lvl: f32,
    grain_lvl: f32,
    loud: f32,
    left: usize,

    cg: f32,
    cg_t: f32,
    cg_step: f32,
    keep_click: bool,
    peak_acc: f32,
    peak_last: f32,
}

#[inline]
fn fin01(x: f32) -> f32 {
    if x.is_finite() { x.clamp(0.0, 1.0) } else { 0.0 }
}

/// The sung features of a frame record, in `feature_index` order: pitch
/// (tuned, else raw; cents), volume, openness, breath, hardness, onset,
/// voicing, intensity.
pub fn features(f: &[f32; CURVE_STRIDE]) -> [f32; N_FEATURES] {
    let pitch = if f[TUNED].is_finite() { f[TUNED] } else { f[frame::PITCH] };
    [
        pitch,
        fin01(f[frame::LOUDNESS]),
        fin01(f[frame::OPENNESS]),
        fin01(f[frame::BREATHINESS]),
        fin01(f[frame::HARDNESS]),
        fin01(f[frame::ONSET]),
        f[frame::VOICING],
        intensity(&f[frame::BANDS..frame::BANDS + N_BANDS]),
    ]
}

/// A via scales an amount by its source in 0..1 (a bipolar source by its distance from centre).
#[inline]
pub fn via_value(bipolar: bool, x: f32) -> f32 {
    if !x.is_finite() {
        0.0
    } else if bipolar {
        x.abs().min(1.0)
    } else {
        x
    }
}

impl Voice {
    pub fn new(sample_rate: f32, seed: u32) -> Self {
        let sr = sample_rate.max(8000.0);
        let s = seed ^ 0x5bd1_e995;
        Voice {
            sr,
            partials: Partials::new(sr, seed),
            click: ClickMod {
                s: Click::new(sr, seed ^ 0x1234_5679),
                e: Click::new(sr, seed ^ 0x9e37_79b1),
                end_post: false,
                end: 0.0,
                colour: 3000.0,
            },
            filters: std::array::from_fn(|_| FilterMod { svf: Svf::default(), key: (0, 0), on: false, w: [0.0; 3] }),
            amps: [AmpMod { gain: 0.0, step: 0.0, target: 0.0 }; 4],
            fold: Chain { fx: Folder::new(sr), on: false, dirty: false },
            clang: Chain { fx: Clang::new(sr), on: false, dirty: false },
            comb: Chain { fx: CombPair::new(sr), on: false, dirty: false },
            inst: [Inst::None; MAX_MODULES],
            alias: std::array::from_fn(|i| i as u8),
            bufs: [[0.0; TICK]; MAX_MODULES],
            buf_of: std::array::from_fn(|i| i as u8),
            inb: [0.0; TICK],
            envs: std::array::from_fn(|_| Env::new()),
            lfos: std::array::from_fn(|j| LfoSrc::new(s ^ LFO_SALT[j])),
            mod_slot: [0; MAX_MODS],
            mod_val: [0.0; MAX_MODS],
            vp: 0,
            nv: Box::new([0.0; MAX_VALUES]),
            rv: Box::new([0.0; MAX_VALUES]),
            scale: Box::new([1.0; MAX_VALUES]),
            pulled: Box::new([f32::NAN; MAX_VALUES]),
            amt_add: Box::new([0.0; MAX_VALUES]),
            oct: Box::new([0.0; MAX_VALUES]),
            moved: Box::new([false; MAX_VALUES]),
            dirty: Box::new([0; MAX_VALUES]),
            n_dirty: 0,
            act: [0; MAX_ROUTES],
            n_act: 0,
            smooth: [0.0; MAX_ROUTES],
            latched: [0.0; MAX_ROUTES],
            latched_raw: [0.0; MAX_ROUTES],
            held: [[0.0; MAX_TARGETS]; MAX_ROUTES],
            holding: false,
            out: [f32::NAN; MAX_ROUTES],
            onset_prev: 0.0,
            note: Note::default(),
            note_src: [f32::NAN; 10],
            note_t: 0.0,
            cents_now: 5700.0,
            vmix: 1.0,
            gate: 1.0,
            noise_lvl: 0.0,
            grain_lvl: 0.0,
            loud: 0.0,
            left: 0,
            cg: 1.0,
            cg_t: 1.0,
            cg_step: 1.0 / (CHOKE_S * sr),
            keep_click: false,
            peak_acc: 0.0,
            peak_last: 0.0,
        }
    }

    // ------------------------------------------------------------ program

    /// Assign pool instances to the voice patch's modules. A voice that moves
    /// between kit slots keeps its state, as it did when every slot used the
    /// same chain; a new patch kills the voice first.
    pub fn bind(&mut self, vp: &VoicePatch, index: usize) {
        self.vp = index;
        let (mut nf, mut na) = (0u8, 0u8);
        for (m, md) in vp.modules().iter().enumerate() {
            self.inst[m] = match md.code {
                P::T_PARTIALS => Inst::Partials,
                P::T_CLICK => Inst::Click,
                P::T_FILTER => {
                    nf += 1;
                    Inst::Filter(nf - 1)
                }
                P::T_AMP => {
                    na += 1;
                    Inst::Amp(na - 1)
                }
                P::T_FOLD => Inst::Fold,
                P::T_CLANG => Inst::Clang,
                P::T_COMB => Inst::Comb,
                _ => Inst::None,
            };
            self.alias[m] = m as u8;
        }
        if let (Some(a), Some(p)) = (vp.fused_amp, vp.find(P::T_PARTIALS)) {
            self.alias[a as usize] = p as u8;
        }
        let (mut ne, mut nl) = (0u8, 0u8);
        for (k, m) in vp.mods().iter().enumerate() {
            if m.code == P::T_ENV {
                self.mod_slot[k] = ne;
                ne += 1;
            } else {
                self.mod_slot[k] = nl;
                nl += 1;
            }
        }
    }

    // ------------------------------------------------------------ notes

    /// Start a note. `len` samples remain until the next tick boundary.
    pub fn start(&mut self, vp: &VoicePatch, lv: &LayerView, note: &Note, frame: Option<&[f32; CURVE_STRIDE]>, len: usize) {
        if !self.active() {
            self.reset_all();
        }
        self.note = *note;
        self.note_src = Self::note_sources(note);
        // A restart keeps phases and amplitudes; only a choke in progress is cancelled.
        self.cg_t = 1.0;
        self.click.end_post = false;
        self.click.e.reset();
        self.note_t = 0.0;
        self.vmix = 1.0;
        self.peak_acc = 0.0;
        self.peak_last = 0.0;
        for (k, m) in vp.mods().iter().enumerate() {
            if m.code == P::T_LFO {
                self.lfos[self.mod_slot[k] as usize].start();
                self.mod_val[k] = 0.0;
            }
        }
        if vp.find(P::T_PARTIALS).is_some() {
            self.partials.note_start();
        }
        // Latched routes take their source now; smoothing starts at it.
        let src = self.sources(frame, lv);
        self.onset_prev = frame.map_or(0.0, |f| fin01(f[frame::ONSET]));
        self.n_act = 0;
        for (i, r) in vp.routes().iter().enumerate() {
            let y = self.shaped(r, vp, &src, frame);
            self.latched[i] = y;
            self.latched_raw[i] = self.raw(r.source, r.arg, &src);
            self.smooth[i] = y;
            // Note sources are fixed for the note: drop a route they leave without a value or at zero.
            let note = (SRC_NOTE_PITCH..=SRC_NOTE_END_CLICK).contains(&r.source);
            let idle = note && (!y.is_finite() || (r.mode != MODE_PULL && r.via == SRC_NONE && r.polar(y) == 0.0));
            if !idle {
                self.act[self.n_act] = i as u8;
                self.n_act += 1;
            }
        }
        // Envelope times and click amounts from the values with the note's routes applied.
        self.holding = true;
        self.resolve(vp, lv, &src, frame, true, false, 0.0);
        self.holding = false;
        for (k, m) in vp.mods().iter().enumerate() {
            if m.code == P::T_ENV {
                let b = m.base as usize;
                let v = |i: usize| val_of(&self.moved[..], &self.rv[..], lv.real, b + i);
                let hold_off = lv.real[b + P::env::HOLD] <= HOLD_OFF_S;
                let t = EnvTimes {
                    mode: v(P::env::MODE).round() as u8,
                    attack: v(P::env::ATTACK),
                    hold: if hold_off { 0.0 } else { v(P::env::HOLD) },
                    decay: v(P::env::DECAY),
                    sustain: v(P::env::SUSTAIN),
                    release: v(P::env::RELEASE),
                };
                self.envs[self.mod_slot[k] as usize].start(t);
            }
        }
        if let Some(c) = vp.find(P::T_CLICK) {
            let b = vp.modules[c].base as usize;
            let v = |i: usize| val_of(&self.moved[..], &self.rv[..], lv.real, b + i);
            let (amt, colour) = (cl(v(P::click::START), 0.0, 1.0), cl(v(P::click::COLOUR), 40.0, 20_000.0));
            self.click.end = cl(v(P::click::END), 0.0, 1.0);
            self.click.colour = colour;
            if amt > 0.0 {
                self.click.s.trigger(amt, colour);
            }
        }
        self.tick(vp, lv, frame, len.max(1), true);
    }

    /// Once per tick while active. `len` is normally TICK.
    pub fn control(&mut self, vp: &VoicePatch, lv: &LayerView, frame: Option<&[f32; CURVE_STRIDE]>, len: usize) {
        if !self.active() {
            return;
        }
        self.peak_last = self.peak_acc;
        self.peak_acc = 0.0;
        self.tick(vp, lv, frame, len.max(1), false);
    }

    /// Note off: the envelopes enter their release; when the first one does, the end click fires.
    pub fn release(&mut self, vp: &VoicePatch) {
        let mut released = false;
        for (k, m) in vp.mods().iter().enumerate() {
            if m.code == P::T_ENV {
                let r = self.envs[self.mod_slot[k] as usize].release();
                if k == 0 {
                    released = r;
                }
            }
        }
        if released {
            self.end_click(false);
        }
    }

    fn end_click(&mut self, post: bool) {
        if self.click.end > 0.0 {
            self.click.e.trigger(self.click.end, self.click.colour);
            // The click is added after the choke gain so the fade does not eat it.
            self.click.end_post = post;
        }
    }

    /// Fade to silence in 3 ms, then go idle.
    pub fn choke(&mut self) {
        let e = &mut self.envs[0];
        if e.stage == Stage::Idle || e.stage == Stage::Choke {
            return;
        }
        e.stage = Stage::Choke;
        self.cg_t = 0.0;
    }

    /// Silence at once and clear all state.
    pub fn kill(&mut self) {
        self.reset_all();
    }

    /// Sounding or ringing (an idle voice costs nothing and may be reused).
    pub fn active(&self) -> bool {
        self.envs[0].stage != Stage::Idle
            || self.noise_level() > 1e-4
            || self.click.s.active()
            || self.click.e.active()
            || self.peak_last.max(self.peak_acc) > SILENT_PEAK
    }

    pub fn released(&self) -> bool {
        matches!(self.envs[0].stage, Stage::Release | Stage::Choke)
    }

    /// First envelope times velocity, sung volume and gate, for budget sharing and voice stealing.
    pub fn loudness(&self) -> f32 {
        self.loud * self.cg
    }

    /// Shaped-noise amplitude this voice asks of its layer for the current tick.
    pub fn noise_level(&self) -> f32 {
        self.noise_lvl * self.cg
    }

    /// Grain amplitude this voice asks of the cloud for the current tick.
    pub fn grain_level(&self) -> f32 {
        self.grain_lvl * self.cg
    }

    /// Value of mod `k` (an LFO times its level) at the last tick.
    pub fn mod_value(&self, k: usize) -> f32 {
        self.mod_val[k]
    }

    /// Audible partials at the last control tick before the budget cut.
    pub fn wanted_partials(&self) -> usize {
        self.partials.wanted()
    }

    /// Partials actually sounding at the last control tick.
    pub fn active_partials(&self) -> usize {
        self.partials.kept()
    }

    /// Current pitch in cents including routes.
    pub fn pitch_cents(&self) -> f32 {
        self.cents_now
    }

    /// Value `i` in real units after this voice's routes, or None when no route moved it.
    pub fn real(&self, i: usize) -> Option<f32> {
        self.moved[i].then(|| self.rv[i])
    }

    /// Value `i` normalized after this voice's routes, or None when no route moved it.
    pub fn norm(&self, i: usize) -> Option<f32> {
        self.moved[i].then(|| self.nv[i])
    }

    /// Route `i`'s shaped output (0..1) at the last tick, NaN when its source had no value.
    pub fn route_out(&self, i: usize) -> f32 {
        self.out[i]
    }

    // ------------------------------------------------------------ state

    fn reset_all(&mut self) {
        self.partials.reset();
        self.left = 0;
        for e in self.envs.iter_mut() {
            e.reset();
        }
        self.noise_lvl = 0.0;
        self.grain_lvl = 0.0;
        self.loud = 0.0;
        self.click.s.reset();
        if !self.keep_click {
            self.click.e.reset();
            self.click.end_post = false;
        }
        self.clang.on = false;
        self.fold.on = false;
        self.comb.on = false;
        if self.clang.dirty || self.fold.dirty || self.comb.dirty {
            self.clang.fx.reset();
            self.fold.fx.reset();
            self.comb.fx.reset();
            self.clang.dirty = false;
            self.fold.dirty = false;
            self.comb.dirty = false;
        }
        for f in self.filters.iter_mut() {
            f.svf.reset();
            f.key = (0, 0);
            f.on = false;
        }
        for a in self.amps.iter_mut() {
            *a = AmpMod { gain: 0.0, step: 0.0, target: 0.0 };
        }
        self.cg = 1.0;
        self.cg_t = 1.0;
        self.peak_acc = 0.0;
        self.peak_last = 0.0;
        if !self.cents_now.is_finite() {
            self.cents_now = 5700.0;
        }
    }

    // ------------------------------------------------------------ routes

    /// Raw values of the per-voice sources that are not mods, and the layer's sources.
    fn sources(&self, frame: Option<&[f32; CURVE_STRIDE]>, lv: &LayerView) -> Sources {
        let mut s = Sources { note: self.note_src, voice: [f32::NAN; N_FEATURES], layer: *lv.lsrc };
        if let Some(f) = frame {
            s.voice = features(f);
        }
        s
    }

    /// The note's source values (SRC_NOTE_* order), fixed for the note.
    fn note_sources(n: &Note) -> [f32; 10] {
        let ok = |x: f32| if x.is_finite() { x } else { f32::NAN };
        let lg = |x: f32| if x.is_finite() && x > 0.0 { x.log2() } else { f32::NAN };
        [
                n.cents,
                n.velocity,
                if n.hard.is_finite() { n.hard.min(1.0) } else { f32::NAN },
                ok(n.attack),
                ok(n.burst),
                ok(n.bend),
                lg(n.hold),
                ok(n.vowel),
                ok(n.tone),
                ok(n.end_click),
        ]
    }

    #[inline]
    fn raw(&self, src: u16, arg: u16, s: &Sources) -> f32 {
        match src {
            SRC_MOD => self.mod_val[arg as usize],
            SRC_NOTE_PITCH..=SRC_NOTE_END_CLICK => s.note[(src - SRC_NOTE_PITCH) as usize],
            SRC_VOICE_PITCH..=SRC_VOICE_VOICING | SRC_VOICE_INTENSITY => feature_index(src).map_or(f32::NAN, |i| s.voice[i]),
            _ => layer_source_index(src, arg).map_or(f32::NAN, |i| s.layer[i]),
        }
    }

    /// Source of route `r` through its shaper (latch and smoothing aside); NaN without a value.
    #[inline]
    fn shaped(&self, r: &Route, vp: &VoicePatch, s: &Sources, frame: Option<&[f32; CURVE_STRIDE]>) -> f32 {
        if r.source == SRC_VOICE_VOWEL {
            return if frame.is_some() { 1.0 } else { f32::NAN };
        }
        let x = self.raw(r.source, r.arg, s);
        let x = if r.bipolar(vp.mods()) { (x + 1.0) * 0.5 } else { x };
        if x.is_finite() { r.shape(x) } else { f32::NAN }
    }

    /// First write to a value this tick: a value only note-start routes move starts from its real
    /// value mapped back, as the old note-start maps did.
    #[inline]
    fn mark(&mut self, lv: &LayerView, t: usize) {
        if !self.moved[t] {
            self.moved[t] = true;
            self.dirty[self.n_dirty] = t as u16;
            self.n_dirty += 1;
            self.nv[t] = if lv.from_real[t] { P::to_norm(lv.descs[t] as usize, lv.real[t]) } else { lv.norm[t] };
            self.scale[t] = 1.0;
            self.pulled[t] = f32::NAN;
            self.amt_add[t] = 0.0;
            self.oct[t] = 0.0;
        }
    }

    /// y and the movement c of route `i` this tick, or None when its source has no value.
    #[allow(clippy::too_many_arguments)]
    #[inline]
    fn route_c(&mut self, i: usize, r: &Route, vp: &VoicePatch, s: &Sources, frame: Option<&[f32; CURVE_STRIDE]>, first: bool, dt: f32) -> Option<(f32, f32)> {
        // A bipolar mod through an identity shaper with both polarities moves by its raw value.
        if r.source == SRC_MOD && r.identity && !r.latch && !r.smoothed() && r.polarity == POL_BOTH && r.bipolar(vp.mods()) {
            let x = self.mod_val[r.arg as usize];
            return x.is_finite().then_some(((x + 1.0) * 0.5, x));
        }
        let y = if r.latch {
            self.latched[i]
        } else {
            let y = self.shaped(r, vp, s, frame);
            if r.smoothed() && y.is_finite() {
                if first || !self.smooth[i].is_finite() {
                    self.smooth[i] = y;
                } else {
                    let ms = if y > self.smooth[i] { r.rise_ms } else { r.fall_ms };
                    let a = if ms > 0.0 { 1.0 - (-dt * 1000.0 / ms).exp() } else { 1.0 };
                    self.smooth[i] += a * (y - self.smooth[i]);
                }
                self.smooth[i]
            } else {
                y
            }
        };
        y.is_finite().then(|| (y, r.polar(y)))
    }

    /// The source in its own units: the raw value within in_lo..in_hi, or
    /// the curve's output `y` spread across that range.
    fn source_value(&self, i: usize, r: &Route, y: f32, s: &Sources) -> f32 {
        if r.bend == 0.0 && r.n_points < 2 {
            let x = if r.latch { self.latched_raw[i] } else { self.raw(r.source, r.arg, s) };
            x.clamp(r.in_lo.min(r.in_hi), r.in_lo.max(r.in_hi))
        } else {
            r.in_lo + y * (r.in_hi - r.in_lo)
        }
    }

    /// (amount x unit, group depth x via): a route moves its target by
    /// `ku * (c * dv)`, the order the old fixed maps multiplied in. An amount
    /// another route moves is read from this voice's values as they stand.
    #[inline]
    fn k(&self, r: &Route, lv: &LayerView, s: &Sources) -> (f32, f32) {
        let a = r.amount as usize;
        let amount = if self.moved[a] && lv.is_amount[a] {
            let d = P::desc(lv.descs[a] as usize);
            (lv.real[a] + self.amt_add[a]).clamp(d.min, d.max)
        } else if self.moved[a] {
            P::to_real(lv.descs[a] as usize, self.nv[a].clamp(0.0, 1.0))
        } else {
            lv.real[a]
        };
        let depth = if r.group == GROUP_NONE { 1.0 } else { lv.depth[r.group as usize - 1] };
        let via = if r.via == SRC_NONE { 1.0 } else { via_value(r.via_bipolar, self.raw(r.via, r.via_arg, s)) };
        (amount * r.unit, if via == 1.0 { depth } else { depth * via })
    }

    /// Steps 5 to 7: start from the layer values, apply the routes (only
    /// those with non-mod sources when `mods` is false), clamp, pull, voicing
    /// swap, map, scale. Fills `nv`, `rv` for the touched values and returns
    /// `base` (cents) with the pitch routes added one by one.
    #[allow(clippy::too_many_arguments)]
    fn resolve(&mut self, vp: &VoicePatch, lv: &LayerView, s: &Sources, frame: Option<&[f32; CURVE_STRIDE]>, first: bool, mods: bool, base: f32) -> f32 {
        for &t in &self.dirty[..self.n_dirty] {
            self.moved[t as usize] = false;
        }
        self.n_dirty = 0;
        self.out[..vp.n_routes].fill(f32::NAN);
        let mut pitch = base;
        let dt = TICK as f32 / self.sr;
        // Add and scale routes; pull routes come after the clamp.
        for x in 0..self.n_act {
            let i = self.act[x] as usize;
            let r = &vp.routes[i];
            if (!mods && r.source == SRC_MOD) || r.mode == MODE_PULL || (frame.is_none() && is_feature_source(r.source)) {
                continue;
            }
            let (ku, dv) = self.k(r, lv, s);
            if (ku == 0.0 && !r.smoothed()) || !(ku.is_finite() && dv.is_finite()) {
                continue;
            }
            let Some((y, c)) = self.route_c(i, r, vp, s, frame, first, dt) else { continue };
            self.out[i] = y;
            let m = |x: f32| ku * if dv == 1.0 { x } else { x * dv };
            // Into another route's amount: in the amount's real units, by the source value itself.
            if r.target_kind == TGT_VALUE && r.mode == MODE_ADD && lv.is_amount[r.targets[0] as usize] {
                let d = m(self.source_value(i, r, y, s));
                if d != 0.0 {
                    for &t in &r.targets[..r.n_targets as usize] {
                        self.mark(lv, t as usize);
                        self.amt_add[t as usize] += d;
                    }
                }
                continue;
            }
            let d = m(c);
            // A route that moves nothing this tick leaves the value exactly as the layer has it.
            if d == 0.0 {
                continue;
            }
            match (r.target_kind, r.mode) {
                (TGT_PITCH, _) => pitch += d,
                (_, MODE_SCALE) => {
                    for &t in &r.targets[..r.n_targets as usize] {
                        self.mark(lv, t as usize);
                        self.scale[t as usize] *= 1.0 + d;
                    }
                }
                (TGT_BANDS, _) => {
                    let t0 = r.targets[0] as usize;
                    for b in 0..N_BANDS {
                        self.mark(lv, t0 + b);
                        self.nv[t0 + b] += d;
                    }
                }
                (TGT_OCTAVES, _) => {
                    for &t in &r.targets[..r.n_targets as usize] {
                        self.mark(lv, t as usize);
                        self.oct[t as usize] += d;
                    }
                }
                _ => {
                    for &t in &r.targets[..r.n_targets as usize] {
                        self.mark(lv, t as usize);
                        self.nv[t as usize] += d;
                    }
                }
            }
        }
        for x in 0..self.n_dirty {
            let t = self.dirty[x] as usize;
            self.nv[t] = self.nv[t].clamp(0.0, 1.0);
        }
        for x in 0..self.n_act {
            let i = self.act[x] as usize;
            let r = &vp.routes[i];
            if r.mode != MODE_PULL || (!mods && r.source == SRC_MOD) {
                continue;
            }
            let (ku, dv) = self.k(r, lv, s);
            let k = ku * dv;
            if r.source == SRC_VOICE_VOWEL {
                // Mouth: each band moves toward the sung vowel shape.
                let Some(f) = frame else { continue };
                self.out[i] = 1.0;
                if !(k > 0.0) {
                    continue;
                }
                let t0 = r.targets[0] as usize;
                for b in 0..N_BANDS {
                    let v = f[frame::BANDS + b];
                    if v.is_finite() {
                        self.mark(lv, t0 + b);
                        let y = P::to_norm(lv.descs[t0 + b] as usize, v.clamp(-30.0, 30.0));
                        self.nv[t0 + b] += k * (y - self.nv[t0 + b]);
                    }
                }
                continue;
            }
            if !k.is_finite() || (k == 0.0 && !r.latch) {
                continue;
            }
            let Some((y, _)) = self.route_c(i, r, vp, s, frame, first, dt) else { continue };
            self.out[i] = y;
            // The target moves in its own units toward the source value.
            let yr = self.source_value(i, r, y, s);
            for (j, &t) in r.targets[..r.n_targets as usize].iter().enumerate() {
                let t = t as usize;
                let d = lv.descs[t] as usize;
                self.mark(lv, t);
                // A latched pull keeps what it set at note start.
                let next = if r.latch && !self.holding {
                    self.held[i][j]
                } else {
                    let cur = if self.pulled[t].is_nan() { P::to_real(d, self.nv[t]) } else { self.pulled[t] };
                    let (lo, hi) = (P::desc(d).min, P::desc(d).max);
                    (cur + k * (yr - cur)).clamp(lo, hi)
                };
                if r.latch && self.holding {
                    self.held[i][j] = next;
                }
                self.pulled[t] = next;
                self.nv[t] = P::to_norm(d, next);
            }
        }
        // Voicing swap: unvoiced frames crossfade the pitched level into the noise level over 10 ms.
        if let Some(f) = frame {
            let target = if f[frame::VOICING] < 0.5 { 1.0 } else { 0.0 };
            if mods {
                let dv = dt / VOICE_XFADE_S;
                self.vmix += (target - self.vmix).clamp(-dv, dv);
            }
            let depth = lv.real[vp.voicing as usize];
            let m = if depth == 1.0 { self.vmix } else { 1.0 - depth * (1.0 - self.vmix) };
            if let (Some(p), Some((nb, _))) = (vp.partials, vp.noise) {
                let (hi, ni) = (vp.modules[p as usize].base as usize + P::partials::LEVEL, nb as usize + P::shaped_noise::LEVEL);
                self.mark(lv, hi);
                self.mark(lv, ni);
                let (h, n) = (self.nv[hi].clamp(0.0, 1.0), self.nv[ni].clamp(0.0, 1.0));
                self.nv[hi] = h * m;
                self.nv[ni] = n * m + h.max(n) * (1.0 - m);
                self.pulled[hi] = f32::NAN;
                self.pulled[ni] = f32::NAN;
            }
        }
        for x in 0..self.n_dirty {
            let t = self.dirty[x] as usize;
            let n = self.nv[t].clamp(0.0, 1.0);
            self.nv[t] = n;
            let mut r = if self.pulled[t].is_nan() { P::to_real(lv.descs[t] as usize, n) } else { self.pulled[t] };
            if self.oct[t] != 0.0 {
                r *= fast_exp2(self.oct[t]);
            }
            self.rv[t] = if self.scale[t] == 1.0 { r } else { r * self.scale[t] };
        }
        pitch
    }

    // ------------------------------------------------------------ control rate

    /// Control-rate update. `first` is true when called from `start`: LFOs
    /// sit at their note-start values.
    fn tick(&mut self, vp: &VoicePatch, lv: &LayerView, frame: Option<&[f32; CURVE_STRIDE]>, len: usize, first: bool) {
        let dt = len as f32 / self.sr;
        if !first {
            self.note_t += dt;
        }
        let s = self.sources(frame, lv);
        // Onset-triggered envelopes restart on a rising sung onset.
        let onset = frame.map_or(0.0, |f| fin01(f[frame::ONSET]));
        let retrigger = !first && onset > 0.0 && self.onset_prev <= 0.0;
        self.onset_prev = onset;
        // Sources first: the first envelope, then the others (the decay mode
        // reads the first one's stage), the LFOs with the layer's values.
        let end_stop = self.note.end_stop;
        let mut event = EnvEvent::None;
        let mut first_rel = false;
        for (k, m) in vp.mods().iter().enumerate() {
            let slot = self.mod_slot[k] as usize;
            if m.code == P::T_ENV {
                if retrigger && m.onset {
                    let t = self.envs[slot].t;
                    self.envs[slot].start(t);
                }
                let ev = self.envs[slot].tick(dt, end_stop && k == 0, first_rel, self.note_t);
                if k == 0 {
                    event = ev;
                    first_rel = self.envs[slot].stage == Stage::Release;
                }
                self.mod_val[k] = self.envs[slot].level;
            } else {
                let b = m.base as usize;
                let (rate, wave, level) = (lv.real[b + P::lfo::RATE], lv.real[b + P::lfo::WAVE], lv.real[b + P::lfo::LEVEL]);
                let x = if first { self.lfos[slot].value } else { self.lfos[slot].tick(rate, wave, dt) };
                self.mod_val[k] = if level == 1.0 { x } else { x * level };
            }
        }
        if event == EnvEvent::EndStop {
            self.cg_t = 0.0;
            self.end_click(true);
        }

        let base = if s.voice[0].is_finite() { s.voice[0] } else { self.note.cents };
        let cents = self.resolve(vp, lv, &s, frame, first, true, cl(base, -4800.0, 16800.0));
        let nyq = 0.45 * self.sr;
        let f0 = (C0_HZ * fast_exp2(cents * (1.0 / 1200.0))).clamp(8.0, nyq);
        if !f0.is_finite() || !cents.is_finite() {
            self.reset_all();
            return;
        }
        self.cents_now = cents;

        // The gate: silent frames mute the voice's amps and layer sources; Live follow's gate envelope scales them.
        self.gate = if frame.is_some_and(|f| f[frame::VOICING] > 1.5) { 0.0 } else { 1.0 };
        if self.note.follow {
            self.gate *= lv.follow_gain;
        }
        let gate = self.gate;
        let env1 = self.mod_val[0];
        let rv = &self.rv[..];
        let moved = &self.moved[..];
        let val = |i: usize| val_of(moved, rv, lv.real, i);
        let vol = if s.voice[1].is_finite() { (1.0 - lv.depth[0]) + lv.depth[0] * s.voice[1] } else { 1.0 };
        self.loud = env1 * cl(vol * gate, 0.0, 8.0) * cl(self.note.velocity, 0.0, 2.0);
        self.noise_lvl = match vp.noise {
            Some((b, d)) => cl(val(b as usize + P::shaped_noise::LEVEL), 0.0, 8.0) * self.mod_val[d as usize] * gate,
            None => 0.0,
        };
        self.grain_lvl = match vp.grains {
            Some((b, d, _)) => cl(val(b as usize + P::grains::LEVEL), 0.0, 8.0) * self.mod_val[d as usize] * gate,
            None => 0.0,
        };

        // Modules.
        let attack = self.envs[0].stage == Stage::Attack;
        let decay = self.envs[0].t.decay;
        for (m, md) in vp.modules().iter().enumerate() {
            let b = md.base as usize;
            match self.inst[m] {
                Inst::Partials => {
                    let mut tone = [0.0; N_BANDS];
                    for (i, t) in tone.iter_mut().enumerate() {
                        *t = val(b + P::partials::TONE_BANDS + i);
                    }
                    let c = PartialsCtl {
                        tilt: val(b + P::partials::TILT),
                        oddeven: val(b + P::partials::ODDEVEN),
                        stretch: val(b + P::partials::STRETCH),
                        jitter: val(b + P::partials::JITTER),
                        modal: val(b + P::partials::MODAL),
                        damp: val(b + P::partials::DAMP),
                        f1_freq: val(b + P::partials::F1_FREQ),
                        f1_gain: val(b + P::partials::F1_GAIN),
                        f2_freq: val(b + P::partials::F2_FREQ),
                        f2_gain: val(b + P::partials::F2_GAIN),
                        tone_bands: tone,
                        max_partials: lv.max_partials,
                    }
                    .clean();
                    let shift = val(b + P::partials::OCTAVE) * 1200.0 + val(b + P::partials::FINE);
                    let (pc, pf) = if shift == 0.0 {
                        (cents, f0)
                    } else {
                        let pc = cents + shift;
                        (pc, (C0_HZ * fast_exp2(pc * (1.0 / 1200.0))).clamp(8.0, nyq))
                    };
                    let mut g = cl(val(b + P::partials::LEVEL), 0.0, 2.0);
                    if let Some(a) = vp.fused_amp {
                        g *= cl(val(vp.modules[a as usize].base as usize + P::amp::GAIN), 0.0, 8.0) * gate;
                    }
                    self.partials.control(&c, pc, pf, g, decay, attack, len);
                }
                Inst::Amp(a) => {
                    if vp.fused_amp == Some(m as u8) {
                        continue;
                    }
                    let amp = &mut self.amps[a as usize];
                    amp.gain = amp.target;
                    amp.target = cl(val(b + P::amp::GAIN), 0.0, 8.0) * gate;
                    amp.step = (amp.target - amp.gain) / len as f32;
                }
                Inst::Click => self.click.colour = cl(val(b + P::click::COLOUR), 40.0, 20_000.0),
                Inst::Clang => {
                    let amt = cl(val(b + P::clang::AMOUNT), 0.0, 1.0);
                    self.clang.on = amt > 0.0;
                    if self.clang.on {
                        self.clang.fx.set(f0, cl(val(b + P::clang::RATIO), 0.1, 32.0), amt);
                        self.clang.dirty = true;
                    }
                }
                Inst::Fold => {
                    let amt = cl(val(b + P::fold::AMOUNT), 0.0, 1.0);
                    self.fold.on = amt > 0.0;
                    if self.fold.on {
                        self.fold.fx.set(amt, cl(val(b + P::fold::BIAS), -1.0, 1.0));
                        self.fold.dirty = true;
                    }
                }
                Inst::Comb => {
                    let amt = cl(val(b + P::comb::AMOUNT), 0.0, 1.0);
                    self.comb.on = amt > 0.0;
                    if self.comb.on {
                        self.comb.fx.set(f0, cl(val(b + P::comb::TUNE), 0.1, 16.0), cl(val(b + P::comb::DECAY), 0.005, 20.0), amt);
                        self.comb.dirty = true;
                    }
                }
                Inst::Filter(fi) => {
                    // Octave routes may sweep past the value's 20 kHz; the filter's own limit applies below.
                    let cutoff = cl(val(b + P::filter::CUTOFF), 20.0, 0.45 * self.sr);
                    let res = cl(val(b + P::filter::RES), 0.0, 1.0);
                    let morph = cl(val(b + P::filter::MORPH), 0.0, 1.0);
                    let track = cl(val(b + P::filter::TRACK), -2.0, 2.0);
                    let sr = self.sr;
                    let f = &mut self.filters[fi as usize];
                    let tr = fast_exp2(track * fast_log2(f0 * (1.0 / 261.63)));
                    let cut = (cutoff * tr).clamp(20.0, 0.45 * sr);
                    let on = !(morph <= 0.0 && cut >= 18_000.0);
                    if on {
                        let q = 0.5 + 19.5 * res * res;
                        let key = (cut.to_bits(), q.to_bits());
                        if !f.on {
                            f.svf.reset();
                            f.key = (0, 0);
                        }
                        if key != f.key {
                            f.key = key;
                            f.svf.set(sr, cut, q);
                        }
                        f.w = if morph < 0.5 {
                            let w = morph * 2.0;
                            [1.0 - w, w / q, 0.0]
                        } else {
                            let w = morph * 2.0 - 1.0;
                            [0.0, (1.0 - w) / q, w]
                        };
                    }
                    f.on = on;
                }
                Inst::None => {}
            }
        }
        self.left = len;
    }

    // ------------------------------------------------------------ audio rate

    /// Add this voice into `out` (mono).
    pub fn render(&mut self, vp: &VoicePatch, out: &mut [f32]) {
        if !self.active() {
            return;
        }
        let mut pos = 0;
        while pos < out.len() {
            let mut m = (out.len() - pos).min(TICK);
            if self.left > 0 {
                m = m.min(self.left);
            }
            self.render_piece(vp, &mut out[pos..pos + m]);
            pos += m;
        }
    }

    fn finish_choke(&mut self) {
        // Everything is silent: clear the bank and chain. A running end click
        // (end stop) keeps playing; it is added after the choke gain.
        self.keep_click = self.click.end_post;
        self.reset_all();
        self.keep_click = false;
    }

    /// Run the program over `m` samples into the module buffers. A processor that is off and has
    /// one input passes that input's buffer on instead of copying it.
    fn run_program(&mut self, vp: &VoicePatch, m: usize) {
        for k in 0..vp.n_modules {
            self.buf_of[k] = k as u8;
            match self.inst[k] {
                Inst::Partials => self.partials.render(&mut self.bufs[k][..m]),
                Inst::Click => {
                    let post = self.click.end_post;
                    // An idle click adds nothing and keeps its state: mark its output silent.
                    if !self.click.s.active() && (post || !self.click.e.active()) {
                        self.buf_of[k] = SILENT;
                        continue;
                    }
                    for y in self.bufs[k][..m].iter_mut() {
                        let s = self.click.s.process();
                        *y = if post { s } else { s + self.click.e.process() };
                    }
                }
                inst => {
                    if self.alias[k] as usize != k {
                        self.buf_of[k] = self.buf_of[self.alias[k] as usize];
                        continue;
                    }
                    let on = match inst {
                        Inst::Amp(_) => true,
                        Inst::Clang => self.clang.on,
                        Inst::Fold => self.fold.on,
                        Inst::Comb => self.comb.on,
                        Inst::Filter(fi) => self.filters[fi as usize].on,
                        _ => false,
                    };
                    // Sum the cables into this module.
                    let mut n_in = 0;
                    let mut only = 0usize;
                    for c in &vp.cables[..vp.n_cables] {
                        if c.to as usize != k {
                            continue;
                        }
                        let src = self.buf_of[c.from as usize] as usize;
                        if src == SILENT as usize {
                            continue;
                        }
                        if n_in == 0 {
                            only = src;
                        } else {
                            if n_in == 1 {
                                self.inb[..m].copy_from_slice(&self.bufs[only][..m]);
                            }
                            for i in 0..m {
                                self.inb[i] += self.bufs[src][i];
                            }
                        }
                        n_in += 1;
                    }
                    if !on && n_in <= 1 {
                        self.buf_of[k] = if n_in == 1 { only as u8 } else { SILENT };
                        continue;
                    }
                    if n_in == 0 {
                        self.inb[..m].fill(0.0);
                    } else if n_in == 1 {
                        self.inb[..m].copy_from_slice(&self.bufs[only][..m]);
                    }
                    let (x, y) = (&self.inb, &mut self.bufs[k]);
                    match inst {
                        Inst::Amp(a) => {
                            let amp = &mut self.amps[a as usize];
                            for i in 0..m {
                                amp.gain += amp.step;
                                y[i] = x[i] * amp.gain;
                            }
                        }
                        Inst::Clang if self.clang.on => {
                            for i in 0..m {
                                y[i] = self.clang.fx.process(x[i]);
                            }
                        }
                        Inst::Fold if self.fold.on => {
                            for i in 0..m {
                                y[i] = self.fold.fx.process(x[i]);
                            }
                        }
                        Inst::Comb if self.comb.on => {
                            for i in 0..m {
                                y[i] = self.comb.fx.process(x[i]);
                            }
                        }
                        Inst::Filter(fi) if self.filters[fi as usize].on => {
                            let f = &mut self.filters[fi as usize];
                            for i in 0..m {
                                let o = f.svf.process(x[i]);
                                y[i] = o.lp * f.w[0] + o.bp * f.w[1] + o.hp * f.w[2] + DENORMAL;
                            }
                        }
                        _ => y[..m].copy_from_slice(&x[..m]),
                    }
                }
            }
        }
    }

    fn render_piece(&mut self, vp: &VoicePatch, out: &mut [f32]) {
        let m = out.len();
        self.run_program(vp, m);
        // Output: the out modules, the choke gain, the post end click.
        let mut pk = 0.0f32;
        let mut ok = true;
        let mut dead = false;
        for i in 0..m {
            let mut x = 0.0;
            if !dead {
                let mut first = true;
                for &o in &vp.out[..vp.n_out] {
                    let src = self.buf_of[o as usize];
                    if src == SILENT {
                        continue;
                    }
                    let b = self.bufs[src as usize][i];
                    x = if first { b } else { x + b };
                    first = false;
                }
            }
            if self.cg != self.cg_t {
                if self.cg_t < self.cg {
                    self.cg = (self.cg - self.cg_step).max(self.cg_t);
                } else {
                    self.cg = (self.cg + self.cg_step).min(self.cg_t);
                }
            }
            let mut y = x * self.cg;
            if self.click.end_post {
                y += self.click.e.process();
            }
            if !y.is_finite() {
                ok = false;
                break;
            }
            y = y.clamp(-OUT_LIMIT, OUT_LIMIT);
            pk = pk.max(y.abs());
            out[i] += y;
            if self.cg == 0.0 && self.cg_t == 0.0 {
                // Choke complete: go idle at exactly this sample; the rest of the span is silent.
                self.finish_choke();
                dead = true;
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
                self.partials.span_end();
                for a in self.amps.iter_mut() {
                    a.gain = a.target;
                    a.step = 0.0;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intensity_reads_the_documented_bands() {
        let mut c = [0.0f32; frame::MAX_BANDS];
        dsp::bands::band_centres(N_BANDS, &mut c);
        for i in INTENSITY_LO {
            assert!((250.0..850.0).contains(&c[i]), "low band {i} at {} Hz", c[i]);
        }
        for i in INTENSITY_HI {
            assert!((2000.0..6500.0).contains(&c[i]), "high band {i} at {} Hz", c[i]);
        }
    }

    #[test]
    fn intensity_maps_the_tilt_to_0_1() {
        let mut b = [0.0f32; N_BANDS];
        assert_eq!(intensity(&b), 0.5, "the singer's usual tilt");
        for i in INTENSITY_HI {
            b[i] = 5.0;
        }
        assert!((intensity(&b) - 0.75).abs() < 1e-6, "5 dB brighter");
        for i in INTENSITY_LO {
            b[i] = 20.0;
        }
        assert_eq!(intensity(&b), 0.0, "clamped");
        b[0] = f32::NAN;
        assert_eq!(intensity(&b), 0.0, "bands outside both ranges do not count");
        b[INTENSITY_HI.start().to_owned()] = f32::NAN;
        assert!(intensity(&b).is_nan());
    }
}
