//! A small LOAD_PATCH writer for engine tests and the benchmark. The app's compiler
//! (web/src/synth/patch.ts) is the real one; this one writes the same format from a plain
//! description, so tests can build patches the classic patch does not cover.
//!
//! Values are laid out as the app lays them out: per voice patch its modules, its mods, the
//! voicing depth and its route amounts; then the channel's effects and width. A value starts at
//! its descriptor's neutral unless `set` gives it a real value.

use crate::patch::*;
use params as P;

/// A route of a voice patch into a module value (`param` is the offset in the module; `pitch`
/// routes ignore both) with a fixed amount (real, -1..1).
#[derive(Clone, Copy, Debug)]
pub struct RouteSpec {
    pub src: u16,
    pub arg: u16,
    pub kind: u8,
    pub module: usize,
    pub param: usize,
    pub mode: u8,
    pub polarity: u8,
    pub amount: f32,
    pub unit: f32,
}

impl RouteSpec {
    pub fn new(src: u16, arg: u16, module: usize, param: usize, amount: f32) -> Self {
        RouteSpec { src, arg, kind: TGT_VALUE, module, param, mode: MODE_ADD, polarity: POL_BOTH, amount, unit: 1.0 }
    }
}

#[derive(Clone, Debug, Default)]
pub struct VoiceSpec {
    /// Module type codes in run order.
    pub modules: Vec<u32>,
    /// (from, to, port) by module index.
    pub cables: Vec<(usize, usize, u8)>,
    pub out: Vec<usize>,
    /// Env and LFO type codes; the first must be an envelope.
    pub mods: Vec<u32>,
    pub routes: Vec<RouteSpec>,
    /// (module index, parameter offset, real value); module indices past the modules are mods.
    pub set: Vec<(usize, usize, f32)>,
    /// The sample a sampler plays.
    pub sample: Option<u32>,
    /// Shaped noise driven by this mod (index into `mods`) at this level.
    pub noise: Option<(u32, f32)>,
    /// Grains driven by this mod, reading this sample, at this level.
    pub grains: Option<(u32, u32, f32)>,
}

/// A channel effect: type code and bypass.
pub type FxSpec = (u32, bool);

struct W(Vec<u8>);
impl W {
    fn u(&mut self, x: u32) {
        self.0.extend_from_slice(&x.to_le_bytes());
    }
    fn f(&mut self, x: f32) {
        self.u(x.to_bits());
    }
}

fn params_of(code: u32) -> &'static P::ModuleType {
    P::module_type(code).expect("known module type")
}

/// The blob of a patch with these voice patches and channel; `chan_set` gives (effect, offset,
/// real value).
pub fn patch(voices: &[VoiceSpec], channel: &[FxSpec], chan_set: &[(usize, usize, f32)]) -> Vec<u8> {
    let mut descs: Vec<u16> = Vec::new();
    let mut init: Vec<f32> = Vec::new();
    let push = |code: u32, set: &mut dyn FnMut(usize) -> Option<f32>, descs: &mut Vec<u16>, init: &mut Vec<f32>| -> usize {
        let base = descs.len();
        let t = params_of(code);
        let mut off = 0;
        for p in t.params {
            for j in 0..p.count as usize {
                let d = p.desc as usize + if p.count > 1 { j } else { 0 };
                descs.push(d as u16);
                let real = set(off).unwrap_or(P::desc(d).neutral);
                init.push(P::to_norm(d, real));
                off += 1;
            }
        }
        base
    };
    struct Laid {
        modules: Vec<usize>,
        mods: Vec<usize>,
        voicing: usize,
        noise: Option<usize>,
        grains: Option<usize>,
        amounts: Vec<usize>,
        lo: usize,
        hi: usize,
    }
    let mut laid = Vec::new();
    for v in voices {
        let lo = descs.len();
        let mut modules = Vec::new();
        for (m, &code) in v.modules.iter().chain(&v.mods).enumerate() {
            let mut set = |off: usize| v.set.iter().find(|s| s.0 == m && s.1 == off).map(|s| s.2);
            modules.push(push(code, &mut set, &mut descs, &mut init));
        }
        let mods = modules.split_off(v.modules.len());
        let voicing = descs.len();
        descs.push(P::D_VOICING_DEPTH as u16);
        init.push(1.0);
        let noise = v.noise.map(|(_, lvl)| push(P::T_SHAPED_NOISE, &mut |o| (o == P::shaped_noise::LEVEL).then_some(lvl), &mut descs, &mut init));
        let grains = v.grains.map(|(_, _, lvl)| push(P::T_GRAINS, &mut |o| (o == P::grains::LEVEL).then_some(lvl), &mut descs, &mut init));
        let mut amounts = Vec::new();
        for r in &v.routes {
            amounts.push(descs.len());
            descs.push(P::D_ROUTE_AMOUNT as u16);
            init.push(P::to_norm(P::D_ROUTE_AMOUNT, r.amount));
        }
        laid.push(Laid { modules, mods, voicing, noise, grains, amounts, lo, hi: descs.len() });
    }
    let mut chan = Vec::new();
    for (c, &(code, _)) in channel.iter().enumerate() {
        let mut set = |off: usize| chan_set.iter().find(|s| s.0 == c && s.1 == off).map(|s| s.2);
        chan.push(push(code, &mut set, &mut descs, &mut init));
    }
    let width = descs.len();
    let wd = P::DESCS.iter().position(|d| d.id == "width").expect("width descriptor");
    descs.push(wd as u16);
    init.push(P::to_norm(wd, P::desc(wd).neutral));

    let mut w = W(Vec::new());
    w.u(PATCH_MAGIC);
    w.u(descs.len() as u32);
    w.u(voices.len() as u32);
    w.u(channel.len() as u32);
    w.u(0);
    w.u(width as u32);
    w.u(0);
    descs.iter().for_each(|&d| w.u(d as u32));
    init.iter().for_each(|&x| w.f(x));
    for (v, l) in voices.iter().zip(&laid) {
        w.u(v.modules.len() as u32);
        w.u(v.cables.len() as u32);
        w.u(v.out.len() as u32);
        w.u(v.mods.len() as u32);
        w.u(v.routes.len() as u32);
        w.u(l.lo as u32);
        w.u(l.hi as u32);
        w.u(l.voicing as u32);
        match (l.noise, v.noise) {
            (Some(b), Some((d, _))) => {
                w.u(b as u32);
                w.u(d);
            }
            _ => {
                w.u(NONE);
                w.u(0);
            }
        }
        match (l.grains, v.grains) {
            (Some(b), Some((d, s, _))) => {
                w.u(b as u32);
                w.u(d);
                w.u(s);
            }
            _ => {
                w.u(NONE);
                w.u(0);
                w.u(0);
            }
        }
        w.u(v.sample.unwrap_or(NONE));
        for (m, &code) in v.modules.iter().enumerate() {
            w.u(code);
            w.u(l.modules[m] as u32);
        }
        for &(a, b, port) in &v.cables {
            w.u(a as u32);
            w.u(b as u32);
            w.u(port as u32);
        }
        v.out.iter().for_each(|&o| w.u(o as u32));
        for (k, &code) in v.mods.iter().enumerate() {
            w.u(code);
            w.u(l.mods[k] as u32);
        }
        for (i, r) in v.routes.iter().enumerate() {
            w.u(r.src as u32);
            w.u(r.arg as u32);
            w.u(r.kind as u32);
            let all = |m: usize| if m < l.modules.len() { l.modules[m] } else { l.mods[m - l.modules.len()] };
            if r.kind == TGT_PITCH {
                w.u(0);
                (0..MAX_TARGETS).for_each(|_| w.u(0));
            } else {
                w.u(1);
                w.u((all(r.module) + r.param) as u32);
                (1..MAX_TARGETS).for_each(|_| w.u(0));
            }
            w.u(l.amounts[i] as u32);
            w.u(SRC_NONE as u32);
            w.u(0);
            w.u(r.mode as u32);
            w.u(r.polarity as u32);
            w.u(0);
            w.u(GROUP_NONE as u32);
            w.f(r.unit);
            w.f(0.0);
            w.f(1.0);
            w.f(0.0);
            w.f(0.0);
            w.f(0.0);
            w.u(0);
            (0..2 * MAX_POINTS).for_each(|_| w.f(0.0));
        }
    }
    for (c, &(code, bypass)) in channel.iter().enumerate() {
        w.u(code | if bypass { CH_BYPASS } else { 0 });
        w.u(chan[c] as u32);
    }
    w.0
}

/// One sounding voice: a module (or a chain) into an amp the first envelope opens, the out.
/// `source` modules come first; the amp is last.
pub fn simple_voice(source: &[u32]) -> VoiceSpec {
    let mut modules = source.to_vec();
    modules.push(P::T_AMP);
    let amp = modules.len() - 1;
    VoiceSpec {
        cables: (0..amp).map(|m| (m, amp, PORT_IN)).collect(),
        out: vec![amp],
        mods: vec![P::T_ENV],
        routes: vec![RouteSpec { mode: MODE_SCALE, polarity: POL_DOWN, ..RouteSpec::new(SRC_MOD, 0, amp, P::amp::GAIN, 1.0) }],
        set: vec![(amp + 1, P::env::SUSTAIN, 1.0), (amp + 1, P::env::RELEASE, 0.05)],
        modules,
        ..VoiceSpec::default()
    }
}

// ---------------------------------------------------------------- heavy patches

/// A voice: `pre` sources into `procs` processors in a chain into an amp the first envelope
/// opens; extra envelopes and LFOs; `routes` more routes from LFO 1 into the first source's
/// `route_param` (or, with `oct`, the filter cutoff in octaves).
pub fn voice(pre: &[u32], procs: &[u32], envs: usize, lfos: usize, routes: usize, route_param: usize) -> VoiceSpec {
    let mut modules: Vec<u32> = pre.to_vec();
    modules.extend_from_slice(procs);
    modules.push(P::T_AMP);
    let amp = modules.len() - 1;
    let mut cables: Vec<(usize, usize, u8)> = Vec::new();
    let first = pre.len();
    for m in 0..first {
        cables.push((m, first, PORT_IN));
    }
    for m in first..amp {
        cables.push((m, m + 1, PORT_IN));
    }
    let mut mods = vec![P::T_ENV; 1 + envs];
    mods.extend(std::iter::repeat_n(P::T_LFO, lfos.max(1)));
    let lfo1 = modules.len() + 1 + envs;
    let mut set: Vec<(usize, usize, f32)> = vec![(amp + 1, P::env::SUSTAIN, 1.0), (lfo1, P::lfo::RATE, 3.0)];
    for (m, &code) in modules.iter().enumerate() {
        match code {
            P::T_PARTIALS => set.push((m, P::partials::TILT, -1.5)),
            P::T_SAMPLER => set.push((m, P::sampler::LOOP, 1.0)),
            P::T_CLANG => set.push((m, P::clang::AMOUNT, 0.5)),
            P::T_FOLD => set.push((m, P::fold::AMOUNT, 0.5)),
            P::T_COMB => set.push((m, P::comb::AMOUNT, 0.5)),
            P::T_FILTER => set.push((m, P::filter::CUTOFF, 2000.0)),
            _ => {}
        }
    }
    let mut rs = vec![RouteSpec { mode: MODE_SCALE, polarity: POL_DOWN, ..RouteSpec::new(SRC_MOD, 0, amp, P::amp::GAIN, 1.0) }];
    for _ in 0..routes {
        rs.push(RouteSpec { polarity: POL_BOTH, ..RouteSpec::new(SRC_MOD, (1 + envs) as u16, 0, route_param, 0.001) });
    }
    VoiceSpec { modules, cables, out: vec![amp], mods, routes: rs, set, sample: pre.contains(&P::T_SAMPLER).then_some(0), ..VoiceSpec::default() }
}

/// The heaviest voice patch the caps allow: 16 modules (every source at its cap, every
/// processor on, two filters and two amps), 4 envelopes,
/// 4 LFOs, 48 routes from the LFOs into the partial bank's spectrum (so it rebuilds every tick),
/// an oscillator frequency-modulating another.
pub fn heaviest_voice() -> VoiceSpec {
    let pre = [P::T_PARTIALS, P::T_OSC, P::T_OSC, P::T_OSC, P::T_OSC, P::T_NOISE, P::T_NOISE, P::T_SAMPLER, P::T_CLICK];
    let procs = [P::T_CLANG, P::T_FOLD, P::T_COMB, P::T_FILTER, P::T_FILTER, P::T_AMP];
    let mut v = voice(&pre, &procs, 3, 4, 0, 0);
    v.cables.push((1, 2, PORT_PM));
    let lfo = |k: usize| (4 + k) as u16;
    for i in 0..47 {
        let p = [P::partials::TILT, P::partials::ODDEVEN, P::partials::F1_FREQ, P::partials::F1_GAIN, P::partials::STRETCH, P::partials::DAMP][i % 6];
        v.routes.push(RouteSpec { polarity: POL_BOTH, ..RouteSpec::new(SRC_MOD, lfo(i % 4), 0, p, 0.05) });
    }
    v.noise = Some((0, 0.3));
    v.grains = Some((0, 0, 0.3));
    v
}

/// The fullest channel: crush, tape, three filters, two delays and a reverb, all on.
pub fn fullest_channel() -> (Vec<FxSpec>, Vec<(usize, usize, f32)>) {
    let ch = vec![
        (P::T_CRUSH, false),
        (P::T_TAPE, false),
        (P::T_CFILTER, false),
        (P::T_DELAY, false),
        (P::T_DELAY, false),
        (P::T_CFILTER, false),
        (P::T_REVERB, false),
        (P::T_CFILTER, false),
    ];
    let set = vec![
        (0, P::crush::BITS, 10.0),
        (1, P::tape::AMOUNT, 0.4),
        (2, P::cfilter::CUTOFF, 4000.0),
        (3, P::delay::MIX, 0.3),
        (4, P::delay::MIX, 0.3),
        (5, P::cfilter::CUTOFF, 3000.0),
        (6, P::reverb::MIX, 0.3),
        (7, P::cfilter::MORPH, 0.5),
    ];
    (ch, set)
}

/// The heaviest patch the caps allow (benchmark and tests).
pub fn heaviest_patch() -> Vec<u8> {
    let (ch, cs) = fullest_channel();
    patch(&[heaviest_voice()], &ch, &cs)
}
