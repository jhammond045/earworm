//! The classic patch (docs/synth.md "The classic patch and today's presets"):
//! the fixed Klont chain of milestones M1 to M4 written as a patch, with
//! today's gesture, hit-shape, LFO and macro maps as its default routes.
//!
//! ```text
//! voice:   partials1 -> amp1 -+
//!          click1 ------------+-> clang1 -> fold1 -> comb1 -> filter1 -> out
//!          env1 (amp), env2 (noise, decay mode), env3 (pitch, one-shot),
//!          lfo1, lfo2, lfo3 (drift: the old chaos)
//! layer:   shaped_noise1 (driven by env2), grains1 (driven by env1)
//! channel: crush1 -> tape1 -> [level, pan] -> delay1 -> reverb1 -> width
//! ```
//!
//! The engine starts every layer on the melody form of this patch, and Rust
//! tests, the benchmark and the analysis crate build their sounds with it.
//! The app's compiler builds the same blob from the document's classic patch
//! (`web/src/synth/classic.ts`); a test checks the two agree byte for byte.
//!
//! The values keep the old parameters' descriptors, so an old 96-value set
//! maps onto them unchanged: `Layout::bind` says which values each old
//! parameter (the Klont vocabulary) becomes. A route whose amount was a fixed
//! constant (a gesture map, a macro weight) carries the constant in its unit
//! and starts with amount 1, so the constant stays exact.

use crate::patch::*;
use crate::protocol::*;
use params as P;

/// One route of the classic voice patch.
struct R {
    src: u16,
    arg: u16,
    /// Target: a (module, param offset) in the voice patch, pitch, or the amount of route `n`.
    tgt: Tgt,
    mode: u8,
    pol: u8,
    latch: bool,
    group: u8,
    /// Amount: descriptor and real value, or a Klont vocabulary parameter it is bound to.
    amount: Amt,
    unit: f32,
    in_lo: f32,
    in_hi: f32,
    points: Option<[[f32; 2]; 2]>,
}

#[derive(Clone, Copy)]
enum Tgt {
    /// (module name, value offset).
    V(&'static str, usize),
    Bands(&'static str, usize),
    Octaves(&'static str, usize),
    Pitch,
    /// The amount of the route with this index.
    Amount(usize),
}

#[derive(Clone, Copy)]
enum Amt {
    /// A fixed starting value with the generic route descriptor.
    Const(f32),
    /// Bound to a vocabulary parameter (its descriptor and value).
    Vocab(usize),
}

const fn r(src: u16, tgt: Tgt, mode: u8, pol: u8, amount: Amt) -> R {
    R { src, arg: 0, tgt, mode, pol, latch: false, group: GROUP_NONE, amount, unit: 1.0, in_lo: 0.0, in_hi: 1.0, points: None }
}

/// 2 ln 2 / ln(hi / lo), in f64 so the app's compiler gets the same f32.
fn ln2_over(lo: f64, hi: f64) -> f32 {
    (2.0 * std::f64::consts::LN_2 / (hi / lo).ln()) as f32
}

/// The voice routes, in evaluation order (index matters: route 0 moves route 25's amount).
fn routes() -> Vec<R> {
    use Amt::*;
    use Tgt::*;
    let follow = |mut x: R| {
        x.latch = true;
        x.group = GROUP_FOLLOW;
        x
    };
    let hold = |module: &'static str, off: usize, lo: f64, hi: f64| {
        follow(R { in_lo: 0.05f64.log2() as f32, in_hi: 0.8f64.log2() as f32, ..r(SRC_NOTE_HOLD, V(module, off), MODE_ADD, POL_BOTH, Const(ln2_over(lo, hi))) })
    };
    let lfo = |k: u16| SRC_MOD_BASE + 3 + k;
    vec![
        // Hit shape (beat layers; the layer leaves the fields empty elsewhere), scaled by Follow.
        follow(R { in_lo: -1.0e6, in_hi: 1.0e6, ..r(SRC_NOTE_BEND, Amount(25), MODE_ADD, POL_UP, Const(-1.0)) }),
        follow(R { in_lo: 0.001, in_hi: 1.0, ..r(SRC_NOTE_ATTACK, V("env1", P::env::ATTACK), MODE_PULL, POL_UP, Const(1.0)) }),
        follow(r(SRC_NOTE_BURST, V("click1", P::click::START), MODE_ADD, POL_UP, Const(0.5))),
        hold("env1", P::env::HOLD, 0.0005, 2.0),
        hold("env1", P::env::DECAY, 0.005, 8.0),
        hold("env2", P::env::DECAY, 0.005, 4.0),
        follow(r(SRC_NOTE_VOWEL, V("partials1", P::partials::TILT), MODE_ADD, POL_BOTH, Const(0.2))),
        follow(r(SRC_NOTE_VOWEL, V("partials1", P::partials::F1_FREQ), MODE_ADD, POL_BOTH, Const(0.25))),
        follow(r(SRC_NOTE_TONE, V("partials1", P::partials::LEVEL), MODE_PULL, POL_UP, Const(1.0))),
        follow(R { points: Some([[0.0, 1.0], [1.0, 0.0]]), ..r(SRC_NOTE_TONE, V("shaped_noise1", P::shaped_noise::LEVEL), MODE_PULL, POL_UP, Const(1.0)) }),
        follow(r(SRC_NOTE_END_CLICK, V("click1", P::click::END), MODE_ADD, POL_UP, Const(1.0))),
        // Note hardness sharpens the start.
        R { latch: true, group: GROUP_HARD, ..r(SRC_NOTE_HARDNESS, V("click1", P::click::START), MODE_ADD, POL_UP, Const(0.5)) },
        R { latch: true, group: GROUP_HARD, ..r(SRC_NOTE_HARDNESS, V("env1", P::env::ATTACK), MODE_ADD, POL_UP, Const(-0.3)) },
        // Velocity and the sung volume on everything the voice sounds.
        r(SRC_NOTE_VELOCITY, V("amp1", P::amp::GAIN), MODE_SCALE, POL_DOWN, Const(1.0)),
        r(SRC_NOTE_VELOCITY, V("shaped_noise1", P::shaped_noise::LEVEL), MODE_SCALE, POL_DOWN, Const(1.0)),
        r(SRC_NOTE_VELOCITY, V("grains1", P::grains::LEVEL), MODE_SCALE, POL_DOWN, Const(1.0)),
        R { group: GROUP_LOUD, ..r(SRC_VOICE_VOLUME, V("amp1", P::amp::GAIN), MODE_SCALE, POL_DOWN, Const(1.0)) },
        R { group: GROUP_LOUD, ..r(SRC_VOICE_VOLUME, V("shaped_noise1", P::shaped_noise::LEVEL), MODE_SCALE, POL_DOWN, Const(1.0)) },
        R { group: GROUP_LOUD, ..r(SRC_VOICE_VOLUME, V("grains1", P::grains::LEVEL), MODE_SCALE, POL_DOWN, Const(1.0)) },
        // Openness, breath and mouth.
        R { group: GROUP_OPEN, ..r(SRC_VOICE_OPENNESS, V("partials1", P::partials::TILT), MODE_ADD, POL_UP, Const(0.4)) },
        R { group: GROUP_OPEN, ..r(SRC_VOICE_OPENNESS, V("partials1", P::partials::F1_FREQ), MODE_ADD, POL_UP, Const(0.25)) },
        R { group: GROUP_OPEN, ..r(SRC_VOICE_OPENNESS, V("filter1", P::filter::CUTOFF), MODE_ADD, POL_UP, Const(0.2)) },
        R { group: GROUP_BREATH, ..r(SRC_VOICE_BREATH, V("shaped_noise1", P::shaped_noise::LEVEL), MODE_ADD, POL_UP, Const(0.5)) },
        R { group: GROUP_MOUTH, ..r(SRC_VOICE_VOWEL, Bands("partials1", P::partials::TONE_BANDS), MODE_PULL, POL_UP, Const(1.0)) },
        // Envelopes and LFOs.
        r(SRC_MOD_BASE, V("amp1", P::amp::GAIN), MODE_SCALE, POL_DOWN, Const(1.0)),
        R { unit: 100.0, ..r(SRC_MOD_BASE + 2, Pitch, MODE_ADD, POL_UP, Vocab(P::ENV_PITCH_AMT)) },
        r(lfo(0), Pitch, MODE_ADD, POL_BOTH, Vocab(P::LFO1_TO_PITCH)),
        r(lfo(1), Pitch, MODE_ADD, POL_BOTH, Vocab(P::LFO2_TO_PITCH)),
        R { unit: 50.0, ..r(lfo(2), Pitch, MODE_ADD, POL_BOTH, Vocab(P::CHAOS_AMT)) },
        r(lfo(0), Octaves("filter1", P::filter::CUTOFF), MODE_ADD, POL_BOTH, Vocab(P::LFO1_TO_FILTER)),
        r(lfo(1), Octaves("filter1", P::filter::CUTOFF), MODE_ADD, POL_BOTH, Vocab(P::LFO2_TO_FILTER)),
        r(lfo(2), Octaves("filter1", P::filter::CUTOFF), MODE_ADD, POL_BOTH, Vocab(P::CHAOS_AMT)),
        r(lfo(0), V("amp1", P::amp::GAIN), MODE_SCALE, POL_DOWN, Vocab(P::LFO1_TO_LEVEL)),
        R { unit: 0.25, ..r(lfo(1), V("grains1", P::grains::POS), MODE_ADD, POL_BOTH, Vocab(P::LFO2_TO_GRAIN)) },
    ]
}

/// SRC_MOD with the mod index added: mods are env1, env2, env3, lfo1, lfo2, lfo3.
const SRC_MOD_BASE: u16 = 1000;

/// Modules of the voice patch in run order, with the vocabulary parameter
/// (or a fixed value) behind each of their values.
const MODULES: [(&str, u32); 7] = [
    ("partials1", P::T_PARTIALS),
    ("amp1", P::T_AMP),
    ("click1", P::T_CLICK),
    ("clang1", P::T_CLANG),
    ("fold1", P::T_FOLD),
    ("comb1", P::T_COMB),
    ("filter1", P::T_FILTER),
];
const CABLES: [(usize, usize); 6] = [(0, 1), (1, 3), (2, 3), (3, 4), (4, 5), (5, 6)];
const MODS: [(&str, u32); 6] = [("env1", P::T_ENV), ("env2", P::T_ENV), ("env3", P::T_ENV), ("lfo1", P::T_LFO), ("lfo2", P::T_LFO), ("lfo3", P::T_LFO)];
const CHANNEL: [(&str, u32); 4] = [("crush1", P::T_CRUSH), ("tape1", P::T_TAPE), ("delay1", P::T_DELAY), ("reverb1", P::T_REVERB)];

/// What a value of the classic patch is: its descriptor, and the vocabulary
/// parameter it comes from (or a fixed starting value).
#[derive(Clone, Copy, Debug)]
pub struct Val {
    pub desc: usize,
    pub vocab: Option<usize>,
    /// Real-unit starting value when not bound to the vocabulary.
    pub fixed: f32,
}

fn vocab_of(module: &str, param: &str, i: usize) -> Option<usize> {
    let p = |id: usize| Some(id);
    match (module, param) {
        ("partials1", "level") => p(P::SRC_HARM_MIX),
        ("partials1", "tilt") => p(P::SRC_TILT),
        ("partials1", "oddeven") => p(P::SRC_ODDEVEN),
        ("partials1", "stretch") => p(P::SRC_STRETCH),
        ("partials1", "jitter") => p(P::SRC_JITTER),
        ("partials1", "modal") => p(P::SRC_MODAL),
        ("partials1", "damp") => p(P::SRC_DAMP),
        ("partials1", "f1_freq") => p(P::SRC_F1_FREQ),
        ("partials1", "f1_gain") => p(P::SRC_F1_GAIN),
        ("partials1", "f2_freq") => p(P::SRC_F2_FREQ),
        ("partials1", "f2_gain") => p(P::SRC_F2_GAIN),
        ("partials1", "tone_bands") => p(P::TONE_BANDS + i),
        ("click1", "start") => p(P::STRIKE_CLICK),
        ("click1", "end") => p(P::STRIKE_END_CLICK),
        ("click1", "colour") => p(P::STRIKE_COLOR),
        ("clang1", "amount") => p(P::CLANG_AMT),
        ("clang1", "ratio") => p(P::CLANG_RATIO),
        ("fold1", "amount") => p(P::FOLD_AMT),
        ("fold1", "bias") => p(P::FOLD_BIAS),
        ("comb1", "amount") => p(P::COMB_AMT),
        ("comb1", "tune") => p(P::COMB_TUNE),
        ("comb1", "decay") => p(P::COMB_DECAY),
        ("filter1", "cutoff") => p(P::FILT_CUTOFF),
        ("filter1", "res") => p(P::FILT_RES),
        ("filter1", "morph") => p(P::FILT_MORPH),
        ("filter1", "track") => p(P::FILT_TRACK),
        ("env1", "attack") => p(P::ENV_ATTACK),
        ("env1", "hold") => p(P::ENV_HOLD),
        ("env1", "decay") => p(P::ENV_DECAY),
        ("env1", "sustain") => p(P::ENV_SUSTAIN),
        ("env1", "release") => p(P::ENV_RELEASE),
        ("env2", "decay") => p(P::ENV_NOISE_DECAY),
        ("env2", "release") => p(P::ENV_RELEASE),
        ("env3", "decay") => p(P::ENV_PITCH_TIME),
        ("lfo1", "rate") => p(P::LFO1_RATE),
        ("lfo2", "rate") => p(P::LFO2_RATE),
        ("lfo3", "rate") => p(P::CHAOS_RATE),
        ("shaped_noise1", "level") => p(P::SRC_NOISE_MIX),
        ("shaped_noise1", "bands") => p(P::NOISE_BANDS + i),
        ("grains1", "level") => p(P::SRC_GRAIN_MIX),
        ("grains1", "size") => p(P::GRAIN_SIZE),
        ("grains1", "density") => p(P::GRAIN_DENSITY),
        ("grains1", "pos") => p(P::GRAIN_POS),
        ("grains1", "spray") => p(P::GRAIN_SPRAY),
        ("grains1", "scatter") => p(P::GRAIN_SCATTER),
        ("crush1", "bits") => p(P::CRUSH_BITS),
        ("crush1", "rate") => p(P::CRUSH_RATE),
        ("tape1", "amount") => p(P::TAPE_AMT),
        ("delay1", "time") => p(P::DELAY_TIME),
        ("delay1", "feedback") => p(P::DELAY_FB),
        ("delay1", "shift") => p(P::DELAY_SHIFT),
        ("delay1", "mix") => p(P::DELAY_MIX),
        ("reverb1", "size") => p(P::VERB_SIZE),
        ("reverb1", "damp") => p(P::VERB_DAMP),
        ("reverb1", "mix") => p(P::VERB_MIX),
        ("reverb1", "freeze") => p(P::VERB_FREEZE),
        _ => None,
    }
}

/// Fixed values: the envelope modes and the third LFO's wave.
fn fixed_of(module: &str, param: &str) -> Option<f32> {
    match (module, param) {
        ("env2", "mode") => Some(1.0),
        ("env3", "mode") => Some(2.0),
        ("env2" | "env3", "sustain") => Some(0.0),
        ("lfo3", "wave") => Some(1.0),
        _ => None,
    }
}

/// Value layout of a classic patch.
pub struct Layout {
    pub kind: u32,
    pub vals: Vec<Val>,
    /// Value index of (voice patch, module or mod name) -> first value.
    pub bases: Vec<(usize, &'static str, usize)>,
    /// Value index of the voice patches' ranges.
    pub ranges: Vec<(usize, usize)>,
    /// Route amounts per voice patch, in route order.
    pub route_amounts: Vec<Vec<usize>>,
    pub voicing: Vec<usize>,
    pub width: usize,
    /// Macro routes: (macro, vocabulary parameter, targets, amount value, weight).
    pub macro_routes: Vec<(usize, usize, Vec<usize>, usize, f32)>,
}

impl Layout {
    pub fn base(&self, vp: usize, name: &str) -> usize {
        self.bases.iter().find(|b| b.0 == vp && b.1 == name).map(|b| b.2).expect("classic module")
    }

    /// Values an old parameter becomes. `set` 0 is the layer's own set (on a
    /// beat layer only its channel values are used), 1..=4 a kit slot.
    pub fn bind(&self, set: usize, vocab: usize) -> Vec<usize> {
        let beat = self.kind == KIND_BEAT;
        let (lo, hi) = if beat && set > 0 { self.ranges[set - 1] } else if beat { (self.ranges[3].1, self.vals.len()) } else { (0, self.vals.len()) };
        if !beat && set > 0 {
            return Vec::new();
        }
        (lo..hi).filter(|&i| self.vals[i].vocab == Some(vocab)).collect()
    }
}

fn mod_names(k: usize) -> &'static str {
    MODS[k].0
}

/// The value layout of the classic patch for a layer kind.
pub fn layout(kind: u32) -> Layout {
    let n_vp = if kind == KIND_BEAT { KIT_SLOTS } else { 1 };
    let mut l = Layout {
        kind,
        vals: Vec::new(),
        bases: Vec::new(),
        ranges: Vec::new(),
        route_amounts: Vec::new(),
        voicing: Vec::new(),
        width: 0,
        macro_routes: Vec::new(),
    };
    let push_module = |l: &mut Layout, vp: usize, name: &'static str, code: u32| {
        let t = P::module_type(code).unwrap();
        l.bases.push((vp, name, l.vals.len()));
        for p in t.params {
            for i in 0..p.count as usize {
                let vocab = vocab_of(name, p.name, i);
                let desc = vocab.unwrap_or(p.desc as usize);
                let fixed = fixed_of(name, p.name).unwrap_or(P::desc(desc).neutral);
                l.vals.push(Val { desc, vocab, fixed });
            }
        }
    };
    let rs = routes();
    for vp in 0..n_vp {
        let lo = l.vals.len();
        for (name, code) in MODULES {
            push_module(&mut l, vp, name, code);
        }
        for (name, code) in MODS {
            push_module(&mut l, vp, name, code);
        }
        l.voicing.push(l.vals.len());
        l.vals.push(Val { desc: P::D_VOICING_DEPTH, vocab: None, fixed: 1.0 });
        push_module(&mut l, vp, "shaped_noise1", P::T_SHAPED_NOISE);
        push_module(&mut l, vp, "grains1", P::T_GRAINS);
        let mut amounts = Vec::new();
        for x in &rs {
            amounts.push(l.vals.len());
            let v = match x.amount {
                Amt::Const(_) => Val { desc: P::D_ROUTE_AMOUNT, vocab: None, fixed: 1.0 },
                Amt::Vocab(i) => Val { desc: i, vocab: Some(i), fixed: P::DESCS[i].neutral },
            };
            l.vals.push(v);
        }
        l.route_amounts.push(amounts);
        l.ranges.push((lo, l.vals.len()));
    }
    for (name, code) in CHANNEL {
        push_module(&mut l, 0, name, code);
    }
    l.width = l.vals.len();
    l.vals.push(Val { desc: P::WIDTH, vocab: Some(P::WIDTH), fixed: 1.0 });
    // Macro weights become routes, macro by macro so each value sums them in the old order.
    for k in 0..N_MACROS {
        for i in 0..P::COUNT {
            let w = P::MACRO_W[k][i];
            if w == 0.0 {
                continue;
            }
            // Group the targets by their place in a voice patch (one route reaches all kit slots).
            let mut groups: Vec<Vec<usize>> = Vec::new();
            let sets: Vec<usize> = if kind == KIND_BEAT { vec![0, 1, 2, 3, 4] } else { vec![0] };
            let per_set: Vec<Vec<usize>> = sets.iter().map(|&s| l.bind(s, i)).collect();
            let width = per_set.iter().map(|v| v.len()).max().unwrap_or(0);
            for j in 0..width {
                let g: Vec<usize> = per_set.iter().filter_map(|v| v.get(j).copied()).collect();
                groups.push(g);
            }
            for g in groups {
                let amount = l.vals.len();
                l.vals.push(Val { desc: P::D_ROUTE_AMOUNT, vocab: None, fixed: 1.0 });
                l.macro_routes.push((k, i, g, amount, w));
            }
        }
    }
    l
}

/// Starting value (normalized) of every value of `l`, from old sets.
pub fn values(l: &Layout, base: &P::ParamSet, kit: &[P::ParamSet; KIT_SLOTS]) -> Vec<f32> {
    let beat = l.kind == KIND_BEAT;
    l.vals
        .iter()
        .enumerate()
        .map(|(i, v)| match v.vocab {
            Some(id) => {
                let set = if beat { l.ranges.iter().position(|r| i >= r.0 && i < r.1).map_or(0, |k| k + 1) } else { 0 };
                if set == 0 { base[id] } else { kit[set - 1][id] }
            }
            None => P::to_norm(v.desc, v.fixed),
        })
        .collect()
}

struct Blob(Vec<u8>);

impl Blob {
    fn u(&mut self, x: u32) {
        self.0.extend_from_slice(&x.to_le_bytes());
    }
    fn f(&mut self, x: f32) {
        self.0.extend_from_slice(&x.to_le_bytes());
    }
}

/// A route added to the classic patch (tests and the benchmark): its source,
/// target (a module or mod name of the voice patch and a value offset, a
/// layer LFO's value, or pitch), shaper and starting amount (real units).
#[derive(Clone, Copy, Debug)]
pub struct Extra {
    pub src: u16,
    pub arg: u16,
    pub target: ExtraTarget,
    pub mode: u8,
    pub pol: u8,
    pub amount: f32,
    pub unit: f32,
    pub in_lo: f32,
    pub in_hi: f32,
    pub bend: f32,
    pub rise_ms: f32,
    pub fall_ms: f32,
    /// Shaper points (up to three here), or none.
    pub points: Option<[[f32; 2]; 3]>,
    pub via: (u16, u16),
}

#[derive(Clone, Copy, Debug)]
pub enum ExtraTarget {
    /// (module name, value offset) in the voice patch (kit slot 1's on a beat layer, for a layer route).
    V(&'static str, usize),
    /// A log-scaled value moved in octaves.
    Octaves(&'static str, usize),
    /// A 16-band shape starting at the offset.
    Bands(&'static str, usize),
    Pitch,
}

impl Extra {
    pub const fn new(src: u16, arg: u16, target: ExtraTarget, amount: f32) -> Extra {
        Extra {
            src,
            arg,
            target,
            mode: MODE_ADD,
            pol: POL_UP,
            amount,
            unit: 1.0,
            in_lo: 0.0,
            in_hi: 1.0,
            bend: 0.0,
            rise_ms: 0.0,
            fall_ms: 0.0,
            points: None,
            via: (SRC_NONE, 0),
        }
    }
}

/// The LOAD_PATCH blob of the classic patch for a layer kind. Its grains
/// read sample `sample` (the app gives layer i sample i).
pub fn patch_blob(kind: u32, sample: u32) -> Vec<u8> {
    patch_blob_ext(kind, sample, &[], &[], 0, true)
}

/// The classic patch with `voice` routes added to every voice patch, `layer`
/// routes added to the layer, and `lfos` layer LFOs (sine, 1 Hz). The added
/// routes' amounts and the LFOs' values follow the classic values. Without
/// `classic_routes` the voice patches have only the added routes (the
/// classic amounts stay as unused values).
pub fn patch_blob_ext(kind: u32, sample: u32, voice: &[Extra], layer: &[Extra], lfos: usize, classic_routes: bool) -> Vec<u8> {
    let l = layout(kind);
    let rs = if classic_routes { routes() } else { Vec::new() };
    let n_vp = l.ranges.len();
    let classic_n = l.vals.len();
    let lfo_base = classic_n;
    let voice_amt = |vp: usize, i: usize| lfo_base + lfos * P::lfo::N + vp * voice.len() + i;
    let layer_amt = |i: usize| lfo_base + lfos * P::lfo::N + n_vp * voice.len() + i;
    let n_values = layer_amt(layer.len());
    let mut descs: Vec<usize> = l.vals.iter().map(|v| v.desc).collect();
    let mut init: Vec<f32> = l.vals.iter().map(|v| P::to_norm(v.desc, v.fixed)).collect();
    for _ in 0..lfos {
        let t = P::module_type(P::T_LFO).unwrap();
        for p in t.params {
            descs.push(p.desc as usize);
            init.push(if p.name == "rate" { P::to_norm(p.desc as usize, 1.0) } else { P::to_norm(p.desc as usize, P::desc(p.desc as usize).neutral) });
        }
    }
    for _ in 0..n_vp {
        for x in voice {
            descs.push(P::D_ROUTE_AMOUNT);
            init.push(P::to_norm(P::D_ROUTE_AMOUNT, x.amount));
        }
    }
    for x in layer {
        descs.push(P::D_ROUTE_AMOUNT);
        init.push(P::to_norm(P::D_ROUTE_AMOUNT, x.amount));
    }
    let mut b = Blob(Vec::new());
    b.u(PATCH_MAGIC);
    b.u(n_values as u32);
    b.u(n_vp as u32);
    b.u(CHANNEL.len() as u32);
    b.u((l.macro_routes.len() + layer.len()) as u32);
    b.u(l.width as u32);
    b.u(lfos as u32);
    for &d in &descs {
        b.u(d as u32);
    }
    for &x in &init {
        b.f(x);
    }
    for vp in 0..n_vp {
        let (lo, hi) = l.ranges[vp];
        b.u(MODULES.len() as u32);
        b.u(CABLES.len() as u32);
        b.u(1);
        b.u(MODS.len() as u32);
        b.u((rs.len() + voice.len()) as u32);
        b.u(lo as u32);
        b.u(hi as u32);
        b.u(l.voicing[vp] as u32);
        b.u(l.base(vp, "shaped_noise1") as u32);
        b.u(1); // driven by env2
        b.u(l.base(vp, "grains1") as u32);
        b.u(0); // driven by env1
        b.u(sample);
        for (name, code) in MODULES {
            b.u(code);
            b.u(l.base(vp, name) as u32);
        }
        for (from, to) in CABLES {
            b.u(from as u32);
            b.u(to as u32);
            b.u(0);
        }
        b.u(6); // out: filter1
        for (k, (_, code)) in MODS.iter().enumerate() {
            b.u(*code);
            b.u(l.base(vp, mod_names(k)) as u32);
        }
        for (i, x) in rs.iter().enumerate() {
            let (src, arg) = if x.src >= SRC_MOD_BASE { (SRC_MOD, x.src - SRC_MOD_BASE) } else { (x.src, x.arg) };
            b.u(src as u32);
            b.u(arg as u32);
            let (kind_t, targets): (u8, Vec<usize>) = match x.tgt {
                Tgt::V(m, off) => (TGT_VALUE, vec![l.base(vp, m) + off]),
                Tgt::Bands(m, off) => (TGT_BANDS, vec![l.base(vp, m) + off]),
                Tgt::Octaves(m, off) => (TGT_OCTAVES, vec![l.base(vp, m) + off]),
                Tgt::Pitch => (TGT_PITCH, vec![]),
                Tgt::Amount(n) => (TGT_VALUE, vec![l.route_amounts[vp][n]]),
            };
            b.u(kind_t as u32);
            b.u(targets.len() as u32);
            for t in 0..MAX_TARGETS {
                b.u(targets.get(t).copied().unwrap_or(0) as u32);
            }
            b.u(l.route_amounts[vp][i] as u32);
            b.u(SRC_NONE as u32);
            b.u(0);
            b.u(x.mode as u32);
            b.u(x.pol as u32);
            b.u(x.latch as u32);
            b.u(x.group as u32);
            b.f(match x.amount {
                Amt::Const(a) => a * x.unit,
                _ => x.unit,
            });
            b.f(x.in_lo);
            b.f(x.in_hi);
            b.f(0.0);
            b.f(0.0);
            b.f(0.0);
            let pts: Vec<[f32; 2]> = x.points.map_or(Vec::new(), |p| p.to_vec());
            b.u(pts.len() as u32);
            for k in 0..MAX_POINTS {
                let p = pts.get(k).copied().unwrap_or([0.0, 0.0]);
                b.f(p[0]);
                b.f(p[1]);
            }
        }
        for (i, x) in voice.iter().enumerate() {
            extra_route(&mut b, x, |m, off| l.base(vp, m) + off, voice_amt(vp, i));
        }
    }
    for (name, code) in CHANNEL {
        b.u(code);
        b.u(l.base(0, name) as u32);
    }
    for k in 0..lfos {
        b.u(P::T_LFO);
        b.u((lfo_base + k * P::lfo::N) as u32);
    }
    for (k, _, targets, amount, w) in &l.macro_routes {
        b.u(SRC_MACRO as u32);
        b.u(*k as u32);
        b.u(TGT_VALUE as u32);
        b.u(targets.len() as u32);
        for t in 0..MAX_TARGETS {
            b.u(targets.get(t).copied().unwrap_or(0) as u32);
        }
        b.u(*amount as u32);
        b.u(SRC_NONE as u32);
        b.u(0);
        b.u(MODE_ADD as u32);
        b.u(POL_BOTH as u32);
        b.u(0);
        b.u(GROUP_NONE as u32);
        b.f(*w);
        b.f(0.0);
        b.f(1.0);
        b.f(0.0);
        b.f(0.0);
        b.f(0.0);
        b.u(0);
        for _ in 0..2 * MAX_POINTS {
            b.f(0.0);
        }
    }
    for (i, x) in layer.iter().enumerate() {
        extra_route(&mut b, x, |m, off| l.base(0, m) + off, layer_amt(i));
    }
    b.0
}

/// The benchmark's 48 voice routes (bench.rs "routes"), all moving every tick: the envelope gate,
/// a vibrato, and 46 more from the envelopes, LFOs and note fields into the partial bank's
/// spectrum (tilt, odd/even, formants, a band shape, fine tune), the filter, the amp and the click:
/// the kinds of value the classic patch already moves every tick. With `heavy` they also move
/// stretch, jitter and damp, which make the partial bank rebuild its frequency and ring tables every
/// tick. Neither switches fold, clang or comb on: that is the cost of a heavier sound (stage S3's
/// cost meter), not of modulation.
pub fn bench_routes(heavy: bool) -> Vec<Extra> {
    use ExtraTarget::*;
    let mut v = vec![
        Extra { mode: MODE_SCALE, pol: POL_DOWN, ..Extra::new(SRC_MOD, 0, V("amp1", P::amp::GAIN), 1.0) },
        Extra { pol: POL_BOTH, unit: 100.0, ..Extra::new(SRC_MOD, 3, Pitch, 0.2) },
    ];
    let light = [
        V("partials1", P::partials::TILT),
        V("partials1", P::partials::ODDEVEN),
        V("partials1", P::partials::F1_FREQ),
        V("partials1", P::partials::F1_GAIN),
        V("partials1", P::partials::F2_FREQ),
        V("partials1", P::partials::F2_GAIN),
        V("partials1", P::partials::FINE),
        Bands("partials1", P::partials::TONE_BANDS),
        V("filter1", P::filter::RES),
        Octaves("filter1", P::filter::CUTOFF),
        V("amp1", P::amp::GAIN),
        V("click1", P::click::COLOUR),
    ];
    let table = [V("partials1", P::partials::STRETCH), V("partials1", P::partials::JITTER), V("partials1", P::partials::DAMP)];
    let targets: Vec<ExtraTarget> = if heavy { light.iter().chain(table.iter()).copied().collect() } else { light.to_vec() };
    // (source, arg): env1..env3, lfo1..lfo3, velocity, pitch.
    let sources = [(SRC_MOD, 0), (SRC_MOD, 1), (SRC_MOD, 2), (SRC_MOD, 3), (SRC_MOD, 4), (SRC_MOD, 5), (SRC_NOTE_VELOCITY, 0), (SRC_NOTE_PITCH, 0)];
    let mut i = 0;
    while v.len() < 48 {
        let (src, arg) = sources[i % sources.len()];
        let mut x = Extra::new(src, arg, targets[i % targets.len()], 0.05);
        if src == SRC_MOD && arg >= 3 {
            x.pol = POL_BOTH;
        }
        if src == SRC_NOTE_PITCH {
            (x.in_lo, x.in_hi) = (3600.0, 6000.0);
        }
        if i % 3 == 0 {
            x.bend = 0.3;
        }
        if i % 4 == 1 {
            x.points = Some([[0.0, 0.0], [0.5, 1.0], [1.0, 0.2]]);
        }
        if i % 5 == 2 {
            (x.rise_ms, x.fall_ms) = (5.0, 50.0);
        }
        v.push(x);
        i += 1;
    }
    v
}

fn extra_route(b: &mut Blob, x: &Extra, at: impl Fn(&'static str, usize) -> usize, amount: usize) {
    b.u(x.src as u32);
    b.u(x.arg as u32);
    match x.target {
        ExtraTarget::V(m, off) | ExtraTarget::Octaves(m, off) | ExtraTarget::Bands(m, off) => {
            let k = match x.target {
                ExtraTarget::Octaves(..) => TGT_OCTAVES,
                ExtraTarget::Bands(..) => TGT_BANDS,
                _ => TGT_VALUE,
            };
            b.u(k as u32);
            b.u(1);
            b.u(at(m, off) as u32);
        }
        ExtraTarget::Pitch => {
            b.u(TGT_PITCH as u32);
            b.u(0);
            b.u(0);
        }
    }
    for _ in 1..MAX_TARGETS {
        b.u(0);
    }
    b.u(amount as u32);
    b.u(x.via.0 as u32);
    b.u(x.via.1 as u32);
    b.u(x.mode as u32);
    b.u(x.pol as u32);
    b.u(0);
    b.u(GROUP_NONE as u32);
    b.f(x.unit);
    b.f(x.in_lo);
    b.f(x.in_hi);
    b.f(x.bend);
    b.f(x.rise_ms);
    b.f(x.fall_ms);
    let pts: &[[f32; 2]] = x.points.as_ref().map_or(&[], |p| &p[..]);
    b.u(pts.len() as u32);
    for k in 0..MAX_POINTS {
        let p = pts.get(k).copied().unwrap_or([0.0, 0.0]);
        b.f(p[0]);
        b.f(p[1]);
    }
}

/// Influence group bit of each value: the bound parameter's aspect group
/// when Influence moves it (the layer's own set only), else 0.
pub fn group_bits(l: &Layout) -> Vec<u32> {
    let beat = l.kind == KIND_BEAT;
    l.vals
        .iter()
        .enumerate()
        .map(|(i, v)| match v.vocab {
            Some(id) if !beat || i >= l.ranges[KIT_SLOTS - 1].1 => 1 << (P::DESCS[id].group as u32),
            _ => 0,
        })
        .collect()
}

/// Layer blob v2 for a classic patch from old sets: fields (LAYER_* order),
/// base and kit sets, macros and the Influence target (a 96-value set).
pub fn layer_blob(kind: u32, fields: &[f32; LAYER_FIELDS], base: &P::ParamSet, kit: &[P::ParamSet; KIT_SLOTS], macros: &[f32; N_MACROS], influence: &P::ParamSet) -> Vec<u8> {
    let l = layout(kind);
    let vals = values(&l, base, kit);
    let target = values(&l, influence, kit);
    let groups = group_bits(&l);
    let mut b = Blob(Vec::new());
    b.u(LAYER_MAGIC);
    for &f in fields {
        b.f(f);
    }
    b.u(vals.len() as u32);
    vals.iter().for_each(|&v| b.f(v));
    macros.iter().for_each(|&v| b.f(v));
    target.iter().for_each(|&v| b.f(v));
    groups.iter().for_each(|&g| b.u(g));
    b.0
}

/// Default fields of a layer (as the old blob encoders wrote them).
pub fn default_fields(kind: u32) -> [f32; LAYER_FIELDS] {
    let mut f = [0.0; LAYER_FIELDS];
    f[LAYER_KIND as usize] = kind as f32;
    f[LAYER_DEPTH_LOUD as usize..=LAYER_DEPTH_MOUTH as usize].copy_from_slice(&[1.0, 0.5, 0.5, 0.5, 1.0]);
    f[LAYER_FOLLOW as usize] = 1.0;
    f[LAYER_INFLUENCE_MASK as usize] = 31.0;
    f[LAYER_KIT_CENTS as usize..LAYER_KIT_CENTS as usize + KIT_SLOTS].copy_from_slice(&[2400.0, 4800.0, 6000.0, 7200.0]);
    f
}

/// States blob v2 for a classic patch: each state's base set, macros and
/// Influence amount; the kit sets are the same in every state.
pub fn states_blob(kind: u32, states: &[(P::ParamSet, [f32; N_MACROS], f32)], kit: &[P::ParamSet; KIT_SLOTS]) -> Vec<u8> {
    let l = layout(kind);
    let mut b = Blob(Vec::new());
    b.u(STATES_MAGIC);
    b.u(states.len() as u32);
    b.u(l.vals.len() as u32);
    for (base, macros, amount) in states {
        values(&l, base, kit).iter().for_each(|&v| b.f(v));
        macros.iter().for_each(|&v| b.f(v));
        b.f(*amount);
    }
    b.0
}

/// Value indices of an old parameter on a classic layer of `kind` (layer set).
pub fn index(kind: u32, vocab: usize) -> Vec<usize> {
    layout(kind).bind(0, vocab)
}

/// Value indices of an old parameter in kit slot `slot` of a classic beat layer.
pub fn kit_index(slot: usize, vocab: usize) -> Vec<usize> {
    layout(KIND_BEAT).bind(slot + 1, vocab)
}

/// Set an old parameter (normalized) on a classic layer: one SET_PARAM per value it became.
pub fn set_param(e: &mut crate::Engine, layer: u8, kind: u32, vocab: usize, norm: f32) {
    for i in index(kind, vocab) {
        e.push(MSG_SET_PARAM, layer, SET_BASE, i as u16, norm, 0);
    }
}

/// A classic layer from old sets: LOAD_PATCH then LOAD_LAYER. Returns the
/// first load status that is not LOAD_OK, else LOAD_OK.
#[allow(clippy::too_many_arguments)]
pub fn load(e: &mut crate::Engine, layer: u32, fields: &[f32; LAYER_FIELDS], base: &P::ParamSet, kit: &[P::ParamSet; KIT_SLOTS], macros: &[f32; N_MACROS], influence: &P::ParamSet) -> i32 {
    let kind = fields[LAYER_KIND as usize].round() as u32;
    let s = e.load_bytes(layer, LOAD_PATCH, &patch_blob(kind, layer));
    if s != LOAD_OK {
        return s;
    }
    e.load_bytes(layer, LOAD_LAYER, &layer_blob(kind, fields, base, kit, macros, influence))
}

/// A LOAD_SAMPLE blob.
pub fn sample_blob(rate: f32, ref_cents: f32, data: &[f32]) -> Vec<u8> {
    let mut b = Blob(Vec::with_capacity(16 + 4 * data.len()));
    b.u(SAMPLE_MAGIC);
    b.f(rate);
    b.f(ref_cents);
    b.u(data.len() as u32);
    data.iter().for_each(|&x| b.f(x));
    b.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_patches_parse() {
        for kind in [KIND_MELODY, KIND_BEAT] {
            let mut p = Patch::new_boxed();
            p.parse(&patch_blob(kind, 0)).expect("classic patch parses");
            let l = layout(kind);
            assert_eq!(p.n_values, l.vals.len());
            assert!(p.n_values <= MAX_VALUES);
            assert_eq!(p.voices()[0].fused_amp, Some(1));
        }
    }

    /// The app's compiler (web/src/synth) builds the same blobs; its test compares against these
    /// files. CLASSIC_WRITE=1 rewrites them.
    #[test]
    fn classic_blobs_match_the_committed_fixtures() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/src/synth/fixtures");
        for (kind, name) in [(KIND_MELODY, "melody"), (KIND_BEAT, "beat")] {
            let blob = patch_blob(kind, 3);
            let path = format!("{dir}/classic-{name}.bin");
            if std::env::var("CLASSIC_WRITE").is_ok() {
                std::fs::create_dir_all(dir).unwrap();
                std::fs::write(&path, &blob).unwrap();
            }
            assert_eq!(std::fs::read(&path).expect("fixture: run with CLASSIC_WRITE=1"), blob, "{name}");
        }
    }

    #[test]
    fn every_old_parameter_but_smear_binds_on_melody() {
        for i in 0..P::COUNT {
            let n = index(KIND_MELODY, i).len();
            if i == P::SMEAR_AMT || i == P::SMEAR_FREEZE {
                assert_eq!(n, 0);
            } else if i == P::ENV_RELEASE || i == P::CHAOS_AMT {
                assert_eq!(n, 2, "env1 and env2 share the release; chaos sets both drift routes");
            } else {
                assert_eq!(n, 1, "{}", P::DESCS[i].id);
            }
        }
    }
}
