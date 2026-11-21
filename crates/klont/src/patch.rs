//! Compiled patches (`LOAD_PATCH`, docs/synth.md "Patches" and "Modulation").
//!
//! A patch is the structure of one layer's sound: up to four voice patches
//! (one per kit slot on a beat layer), each a list of voice modules in run
//! order with the cables between them, its modulation sources (envelopes and
//! LFOs), its per-voice routes and the layer sources it feeds (shaped noise,
//! grains); then the channel chain and the layer routes. The values those
//! modules read live in the layer's value vector (LAYER blob v2); the patch
//! says which descriptor maps each value to real units.
//!
//! The engine does not trust the compiler: `Patch::parse` checks every count,
//! index and type, and refuses anything out of range or cyclic with
//! LOAD_MALFORMED, leaving the old patch in place. Parsing writes into a
//! patch allocated at start-up, so loading never allocates.
//!
//! # Blob layout (32-bit little-endian words; `f` marks an f32)
//! ```text
//! 0  magic PATCH_MAGIC          1  n_values (1..=MAX_VALUES)
//! 2  n_voice_patches (1..=4)    3  n_channel (0..=MAX_CHANNEL)
//! 4  n_layer_routes             5  width value index
//! 6  n_values descriptor ids
//!    n_values f starting values (normalized): where the values sit when the
//!    patch loads, before a layer blob sets them (a route's default amount)
//! per voice patch:
//!    n_modules, n_cables, n_out, n_mods, n_routes, value_lo, value_hi,
//!    voicing value index,
//!    noise value base (NONE: no shaped noise), noise drive (mod index),
//!    grains value base (NONE: no grains), grains drive (mod index), grains sample id,
//!    modules  n_modules x [type code, value base]
//!    cables   n_cables  x [from module, to module, port]
//!    out      n_out     x [module]
//!    mods     n_mods    x [type code, value base]       (mods[0] is the envelope that ends the voice)
//!    routes   n_routes  x ROUTE_WORDS
//! channel   n_channel x [type code, value base]
//! layer routes n_layer_routes x ROUTE_WORDS
//! ```
//! A route (ROUTE_WORDS words):
//! ```text
//! 0 source  1 source arg  2 target kind  3 n_targets  4..8 targets
//! 8 amount value index  9 via source  10 via arg  11 mode  12 polarity
//! 13 latch  14 group  15f unit  16f in_lo  17f in_hi  18f bend
//! 19f rise ms  20f fall ms  21 n_points  22..38f points (x, y) x 8
//! ```

pub use crate::protocol::MAX_VALUES;
use crate::protocol::{KIT_SLOTS, N_MACROS};
use params as P;

pub const PATCH_MAGIC: u32 = 0x3150_5745; // "EWP1"
pub const MAX_MODULES: usize = 16;
pub const MAX_CABLES: usize = 32;
pub const MAX_MODS: usize = 8;
pub const MAX_ENVS: usize = 4;
pub const MAX_LFOS: usize = 4;
pub const MAX_ROUTES: usize = 48;
pub const MAX_LAYER_ROUTES: usize = 64;
pub const MAX_CHANNEL: usize = 8;
pub const MAX_TARGETS: usize = 4;
pub const MAX_POINTS: usize = 8;
pub use crate::protocol::MAX_SAMPLES;
pub const ROUTE_WORDS: usize = 22 + 2 * MAX_POINTS;
pub const NONE: u32 = u32::MAX;

/// Per-type caps in one voice patch (docs/synth.md "Rules for the graph").
fn voice_cap(code: u32) -> usize {
    match code {
        P::T_PARTIALS | P::T_CLICK | P::T_FOLD | P::T_CLANG | P::T_COMB => 1,
        P::T_FILTER => 2,
        P::T_AMP => 4,
        _ => 0,
    }
}

// Route sources. Per voice: MOD, NOTE_*, VOICE_*. Per layer: MACRO.
pub const SRC_NONE: u16 = 0;
/// arg = index into the voice patch's mods.
pub const SRC_MOD: u16 = 1;
pub const SRC_NOTE_PITCH: u16 = 10;
pub const SRC_NOTE_VELOCITY: u16 = 11;
pub const SRC_NOTE_HARDNESS: u16 = 12;
/// The hit's attack time in seconds.
pub const SRC_NOTE_ATTACK: u16 = 13;
pub const SRC_NOTE_BURST: u16 = 14;
/// Semitones.
pub const SRC_NOTE_BEND: u16 = 15;
/// log2 of the hit's hold time in seconds.
pub const SRC_NOTE_HOLD: u16 = 16;
pub const SRC_NOTE_VOWEL: u16 = 17;
pub const SRC_NOTE_TONE: u16 = 18;
pub const SRC_NOTE_END_CLICK: u16 = 19;
/// Cents above C0.
pub const SRC_VOICE_PITCH: u16 = 30;
pub const SRC_VOICE_VOLUME: u16 = 31;
pub const SRC_VOICE_OPENNESS: u16 = 32;
pub const SRC_VOICE_BREATH: u16 = 33;
pub const SRC_VOICE_HARDNESS: u16 = 34;
pub const SRC_VOICE_ONSET: u16 = 35;
pub const SRC_VOICE_VOICING: u16 = 36;
/// 16 band levels in dB; only for pull routes into a 16-band shape.
pub const SRC_VOICE_VOWEL: u16 = 37;
/// arg = macro 0..7.
pub const SRC_MACRO: u16 = 50;

pub const TGT_VALUE: u8 = 0;
/// The voice pitch, in cents (unit converts the amount).
pub const TGT_PITCH: u8 = 1;
/// 16 consecutive values from targets[0] (a band shape).
pub const TGT_BANDS: u8 = 2;
/// A log-scaled value moved in octaves after it is mapped to real units,
/// so modulation can reach past the value's own range (a filter cutoff
/// swept above 20 kHz, up to what the module allows).
pub const TGT_OCTAVES: u8 = 3;

pub const MODE_ADD: u8 = 0;
pub const MODE_SCALE: u8 = 1;
pub const MODE_PULL: u8 = 2;

pub const POL_UP: u8 = 0;
pub const POL_DOWN: u8 = 1;
pub const POL_BOTH: u8 = 2;

/// Groups: the route's amount is also scaled by this depth of the layer.
pub const GROUP_NONE: u8 = 0;
pub const GROUP_LOUD: u8 = 1;
pub const GROUP_OPEN: u8 = 2;
pub const GROUP_HARD: u8 = 3;
pub const GROUP_BREATH: u8 = 4;
pub const GROUP_MOUTH: u8 = 5;
pub const GROUP_FOLLOW: u8 = 6;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Route {
    pub source: u16,
    pub arg: u16,
    pub target_kind: u8,
    pub n_targets: u8,
    pub targets: [u16; MAX_TARGETS],
    pub amount: u16,
    pub via: u16,
    pub via_arg: u16,
    pub mode: u8,
    pub polarity: u8,
    pub latch: bool,
    pub group: u8,
    pub unit: f32,
    pub in_lo: f32,
    pub in_hi: f32,
    pub bend: f32,
    pub rise_ms: f32,
    pub fall_ms: f32,
    pub n_points: u8,
    pub points: [[f32; 2]; MAX_POINTS],
    /// in 0..1, no bend, no points, no smoothing: y = clamp(x).
    pub identity: bool,
}

impl Route {
    const EMPTY: Route = Route {
        source: SRC_NONE,
        arg: 0,
        target_kind: TGT_VALUE,
        n_targets: 0,
        targets: [0; MAX_TARGETS],
        amount: 0,
        via: SRC_NONE,
        via_arg: 0,
        mode: MODE_ADD,
        polarity: POL_UP,
        latch: false,
        group: GROUP_NONE,
        unit: 1.0,
        in_lo: 0.0,
        in_hi: 1.0,
        bend: 0.0,
        rise_ms: 0.0,
        fall_ms: 0.0,
        n_points: 0,
        points: [[0.0; 2]; MAX_POINTS],
        identity: true,
    };

    /// The source is bipolar (-1..1 enters the shaper as 0..1).
    pub fn bipolar(&self, mods: &[Mod]) -> bool {
        self.source == SRC_MOD && mods[self.arg as usize].code == P::T_LFO
    }

    /// The shaper: a raw source value (0..1, or a bipolar value mapped to
    /// 0..1 by the caller) to the curve output y in 0..1.
    pub fn shape(&self, x: f32) -> f32 {
        let u = if self.identity {
            x.clamp(0.0, 1.0)
        } else {
            let d = self.in_hi - self.in_lo;
            if d == 0.0 { if x >= self.in_hi { 1.0 } else { 0.0 } } else { ((x - self.in_lo) / d).clamp(0.0, 1.0) }
        };
        if self.n_points >= 2 {
            let p = &self.points[..self.n_points as usize];
            if u <= p[0][0] {
                return p[0][1];
            }
            for w in p.windows(2) {
                if u <= w[1][0] {
                    let span = w[1][0] - w[0][0];
                    let t = if span > 0.0 { (u - w[0][0]) / span } else { 1.0 };
                    return w[0][1] + (w[1][1] - w[0][1]) * t;
                }
            }
            return p[p.len() - 1][1];
        }
        if self.bend == 0.0 { u } else { u.powf((-2.0 * self.bend).exp2()) }
    }

    /// Polarity: y (0..1) to the movement c.
    pub fn polar(&self, y: f32) -> f32 {
        match self.polarity {
            POL_UP => y,
            POL_DOWN => y - 1.0,
            _ => 2.0 * y - 1.0,
        }
    }

    /// Needs per-voice state (smoothing).
    pub fn smoothed(&self) -> bool {
        self.rise_ms > 0.0 || self.fall_ms > 0.0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Module {
    pub code: u32,
    pub base: u16,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cable {
    pub from: u8,
    pub to: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Mod {
    pub code: u32,
    pub base: u16,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoicePatch {
    pub n_modules: usize,
    pub modules: [Module; MAX_MODULES],
    pub n_cables: usize,
    pub cables: [Cable; MAX_CABLES],
    pub n_out: usize,
    pub out: [u8; MAX_MODULES],
    pub n_mods: usize,
    pub mods: [Mod; MAX_MODS],
    pub n_routes: usize,
    pub routes: [Route; MAX_ROUTES],
    /// The voice patch's own values: [lo, hi) of the layer's vector.
    pub lo: usize,
    pub hi: usize,
    pub voicing: u16,
    /// Shaped noise of this voice patch: value base and the mod driving it.
    pub noise: Option<(u16, u8)>,
    /// Grains: value base, driving mod, sample id.
    pub grains: Option<(u16, u8, u8)>,
    /// An amp whose only input is the partial bank and which is the bank's
    /// only reader: its gain is applied to the partials' own amplitude ramps.
    pub fused_amp: Option<u8>,
    /// The partial bank's module index.
    pub partials: Option<u8>,
}

impl VoicePatch {
    const EMPTY: VoicePatch = VoicePatch {
        n_modules: 0,
        modules: [Module { code: 0, base: 0 }; MAX_MODULES],
        n_cables: 0,
        cables: [Cable { from: 0, to: 0 }; MAX_CABLES],
        n_out: 0,
        out: [0; MAX_MODULES],
        n_mods: 0,
        mods: [Mod { code: 0, base: 0 }; MAX_MODS],
        n_routes: 0,
        routes: [Route::EMPTY; MAX_ROUTES],
        lo: 0,
        hi: 0,
        voicing: 0,
        noise: None,
        grains: None,
        fused_amp: None,
        partials: None,
    };

    pub fn modules(&self) -> &[Module] {
        &self.modules[..self.n_modules]
    }
    pub fn mods(&self) -> &[Mod] {
        &self.mods[..self.n_mods]
    }
    pub fn routes(&self) -> &[Route] {
        &self.routes[..self.n_routes]
    }
    /// First module of a type.
    pub fn find(&self, code: u32) -> Option<usize> {
        self.modules().iter().position(|m| m.code == code)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ChannelFx {
    pub code: u32,
    pub base: u16,
}

#[derive(Clone, PartialEq)]
pub struct Patch {
    pub n_values: usize,
    pub descs: [u16; MAX_VALUES],
    pub init: [f32; MAX_VALUES],
    pub n_voices: usize,
    pub voices: [VoicePatch; KIT_SLOTS],
    pub n_channel: usize,
    pub channel: [ChannelFx; MAX_CHANNEL],
    pub width: u16,
    pub n_layer_routes: usize,
    pub layer_routes: [Route; MAX_LAYER_ROUTES],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Malformed;

struct Words<'a> {
    b: &'a [u8],
    at: usize,
}

impl Words<'_> {
    fn u(&mut self) -> Result<u32, Malformed> {
        let i = self.at * 4;
        if i + 4 > self.b.len() {
            return Err(Malformed);
        }
        self.at += 1;
        Ok(u32::from_le_bytes([self.b[i], self.b[i + 1], self.b[i + 2], self.b[i + 3]]))
    }
    fn f(&mut self) -> Result<f32, Malformed> {
        let x = f32::from_bits(self.u()?);
        if x.is_finite() { Ok(x) } else { Err(Malformed) }
    }
    fn small(&mut self, max: usize) -> Result<usize, Malformed> {
        let x = self.u()? as usize;
        if x <= max { Ok(x) } else { Err(Malformed) }
    }
}

fn check(ok: bool) -> Result<(), Malformed> {
    if ok { Ok(()) } else { Err(Malformed) }
}

impl Patch {
    /// An empty patch, for allocation at start-up; `parse` fills it.
    pub fn new_boxed() -> Box<Patch> {
        Box::new(Patch {
            n_values: 0,
            descs: [0; MAX_VALUES],
            init: [0.0; MAX_VALUES],
            n_voices: 0,
            voices: [VoicePatch::EMPTY; KIT_SLOTS],
            n_channel: 0,
            channel: [ChannelFx::default(); MAX_CHANNEL],
            width: 0,
            n_layer_routes: 0,
            layer_routes: [Route::EMPTY; MAX_LAYER_ROUTES],
        })
    }

    pub fn voices(&self) -> &[VoicePatch] {
        &self.voices[..self.n_voices]
    }
    pub fn channel(&self) -> &[ChannelFx] {
        &self.channel[..self.n_channel]
    }
    pub fn layer_routes(&self) -> &[Route] {
        &self.layer_routes[..self.n_layer_routes]
    }

    /// Parse and check a LOAD_PATCH blob into `self`. On error `self` is left
    /// in an unspecified state: parse into a scratch patch and copy on success.
    pub fn parse(&mut self, bytes: &[u8]) -> Result<(), Malformed> {
        let mut w = Words { b: bytes, at: 0 };
        check(w.u()? == PATCH_MAGIC)?;
        let n_values = w.small(MAX_VALUES)?;
        check(n_values > 0)?;
        let n_voices = w.small(KIT_SLOTS)?;
        check(n_voices > 0)?;
        let n_channel = w.small(MAX_CHANNEL)?;
        let n_layer_routes = w.small(MAX_LAYER_ROUTES)?;
        let width = w.u()? as usize;
        check(width < n_values)?;
        self.n_values = n_values;
        self.n_voices = n_voices;
        self.n_channel = n_channel;
        self.n_layer_routes = n_layer_routes;
        self.width = width as u16;
        for i in 0..n_values {
            let d = w.u()? as usize;
            check(d < P::N_DESCS)?;
            self.descs[i] = d as u16;
        }
        for i in 0..n_values {
            self.init[i] = w.f()?.clamp(0.0, 1.0);
        }
        for k in 0..n_voices {
            let mut vp = VoicePatch::EMPTY;
            parse_voice(&mut w, &mut vp, n_values)?;
            self.voices[k] = vp;
        }
        // Voice patches own disjoint value ranges.
        for a in 0..n_voices {
            for b in a + 1..n_voices {
                let (x, y) = (&self.voices[a], &self.voices[b]);
                check(x.hi <= y.lo || y.hi <= x.lo)?;
            }
        }
        let (mut delays, mut reverbs, mut stereo) = (0, 0, false);
        for c in 0..n_channel {
            let code = w.u()?;
            let base = w.u()? as usize;
            let t = P::module_type(code).ok_or(Malformed)?;
            check(t.kind == P::ModKind::Channel && base + t.n_values <= n_values)?;
            match code {
                P::T_DELAY => {
                    delays += 1;
                    stereo = true;
                }
                P::T_REVERB => {
                    reverbs += 1;
                    stereo = true;
                }
                // Mono effects run on the voice sum before level and pan
                // spread it to stereo (as today), so they must come first.
                _ => check(!stereo)?,
            }
            self.channel[c] = ChannelFx { code, base: base as u16 };
        }
        check(delays <= 1 && reverbs <= 1)?;
        for r in 0..n_layer_routes {
            let route = parse_layer_route(&mut w, n_values)?;
            self.layer_routes[r] = route;
        }
        check(order_ok(self.layer_routes()))?;
        check(w.at * 4 == bytes.len())?;
        // Scale routes may only multiply scalable values.
        for vp in self.voices() {
            for r in vp.routes().iter().chain(self.layer_routes()) {
                if r.mode == MODE_SCALE && r.target_kind == TGT_VALUE {
                    for &t in &r.targets[..r.n_targets as usize] {
                        check(P::SCALABLE[self.descs[t as usize] as usize])?;
                    }
                }
                if r.target_kind == TGT_OCTAVES {
                    for &t in &r.targets[..r.n_targets as usize] {
                        check(P::desc(self.descs[t as usize] as usize).scale == P::Scale::Log)?;
                    }
                }
            }
        }
        Ok(())
    }
}

/// A route that modulates another route's amount must come before it.
fn order_ok(routes: &[Route]) -> bool {
    routes.iter().enumerate().all(|(i, r)| {
        routes[i..].iter().all(|o| o.target_kind != TGT_VALUE || !o.targets[..o.n_targets as usize].contains(&r.amount))
    })
}

fn parse_voice(w: &mut Words, vp: &mut VoicePatch, n_values: usize) -> Result<(), Malformed> {
    vp.n_modules = w.small(MAX_MODULES)?;
    vp.n_cables = w.small(MAX_CABLES)?;
    vp.n_out = w.small(MAX_MODULES)?;
    vp.n_mods = w.small(MAX_MODS)?;
    vp.n_routes = w.small(MAX_ROUTES)?;
    vp.lo = w.u()? as usize;
    vp.hi = w.u()? as usize;
    check(vp.lo < vp.hi && vp.hi <= n_values)?;
    let in_range = |base: usize, n: usize| base >= vp.lo && base + n <= vp.hi;
    let voicing = w.u()? as usize;
    check(in_range(voicing, 1))?;
    vp.voicing = voicing as u16;
    let (nb, nd) = (w.u()?, w.u()?);
    let (gb, gd, gs) = (w.u()?, w.u()?, w.u()?);

    let mut count = [0usize; 256];
    for m in 0..vp.n_modules {
        let code = w.u()?;
        let base = w.u()? as usize;
        let t = P::module_type(code).ok_or(Malformed)?;
        check(t.kind == P::ModKind::Voice && in_range(base, t.n_values))?;
        count[code as usize & 255] += 1;
        check(count[code as usize & 255] <= voice_cap(code))?;
        vp.modules[m] = Module { code, base: base as u16 };
    }
    check(vp.n_modules > 0)?;
    for c in 0..vp.n_cables {
        let from = w.small(MAX_MODULES - 1)?;
        let to = w.small(MAX_MODULES - 1)?;
        let port = w.u()?;
        // Modules are given in run order, so a cable only runs forward: no cycles.
        check(from < to && to < vp.n_modules && port == 0)?;
        let dst = vp.modules[to].code;
        check(dst != P::T_PARTIALS && dst != P::T_CLICK)?;
        vp.cables[c] = Cable { from: from as u8, to: to as u8 };
    }
    for o in 0..vp.n_out {
        let m = w.small(MAX_MODULES - 1)?;
        check(m < vp.n_modules)?;
        vp.out[o] = m as u8;
    }
    let (mut envs, mut lfos) = (0, 0);
    for k in 0..vp.n_mods {
        let code = w.u()?;
        let base = w.u()? as usize;
        let t = P::module_type(code).ok_or(Malformed)?;
        check(t.kind == P::ModKind::Source && in_range(base, t.n_values))?;
        if code == P::T_ENV {
            envs += 1;
        } else {
            lfos += 1;
        }
        vp.mods[k] = Mod { code, base: base as u16 };
    }
    check(vp.n_mods > 0 && vp.mods[0].code == P::T_ENV && envs <= MAX_ENVS && lfos <= MAX_LFOS)?;
    let drive = |d: u32| (d as usize) < vp.n_mods && vp.mods[d as usize].code == P::T_ENV;
    vp.noise = if nb == NONE {
        None
    } else {
        check(in_range(nb as usize, P::shaped_noise::N) && drive(nd))?;
        Some((nb as u16, nd as u8))
    };
    vp.grains = if gb == NONE {
        None
    } else {
        check(in_range(gb as usize, P::grains::N) && drive(gd) && (gs as usize) < MAX_SAMPLES)?;
        Some((gb as u16, gd as u8, gs as u8))
    };
    for r in 0..vp.n_routes {
        let route = parse_route_voice(w, vp, n_values)?;
        vp.routes[r] = route;
    }
    check(order_ok(vp.routes()))?;
    vp.fused_amp = fused_amp(vp);
    vp.partials = vp.find(P::T_PARTIALS).map(|p| p as u8);
    Ok(())
}

fn fused_amp(vp: &VoicePatch) -> Option<u8> {
    let p = vp.find(P::T_PARTIALS)?;
    let cables = &vp.cables[..vp.n_cables];
    if cables.iter().filter(|c| c.from as usize == p).count() != 1 || vp.out[..vp.n_out].contains(&(p as u8)) {
        return None;
    }
    let c = cables.iter().find(|c| c.from as usize == p)?;
    let a = c.to as usize;
    let only_input = cables.iter().filter(|x| x.to as usize == a).count() == 1;
    (vp.modules[a].code == P::T_AMP && only_input).then_some(a as u8)
}

fn parse_route_voice(w: &mut Words, vp: &VoicePatch, n_values: usize) -> Result<Route, Malformed> {
    let r = read_route(w, n_values)?;
    let voice_src = |s: u16, a: u16| match s {
        SRC_MOD => (a as usize) < vp.n_mods,
        SRC_NOTE_PITCH..=SRC_NOTE_END_CLICK => true,
        SRC_VOICE_PITCH..=SRC_VOICE_VOICING => true,
        _ => false,
    };
    let vowel = r.source == SRC_VOICE_VOWEL;
    check(voice_src(r.source, r.arg) || vowel)?;
    check(r.via == SRC_NONE || voice_src(r.via, r.via_arg))?;
    check(!vowel || (r.target_kind == TGT_BANDS && r.mode == MODE_PULL))?;
    let in_voice = |t: usize, n: usize| t >= vp.lo && t + n <= vp.hi;
    match r.target_kind {
        TGT_PITCH => check(r.mode == MODE_ADD)?,
        TGT_BANDS => check(r.n_targets == 1 && in_voice(r.targets[0] as usize, 16) && r.mode != MODE_SCALE)?,
        TGT_OCTAVES => {
            check(r.mode == MODE_ADD)?;
            for &t in &r.targets[..r.n_targets as usize] {
                check(in_voice(t as usize, 1))?;
            }
        }
        _ => {
            for &t in &r.targets[..r.n_targets as usize] {
                check(in_voice(t as usize, 1))?;
                // Envelopes and LFOs cannot modulate envelope or LFO values.
                if r.source == SRC_MOD {
                    for m in vp.mods() {
                        let n = P::module_type(m.code).map_or(0, |t| t.n_values);
                        check(!(t >= m.base && (t as usize) < m.base as usize + n))?;
                    }
                }
            }
        }
    }
    Ok(r)
}

fn parse_layer_route(w: &mut Words, n_values: usize) -> Result<Route, Malformed> {
    let r = read_route(w, n_values)?;
    // Layer routes: macros only in this stage (layer LFOs, MIDI and layer features come with the matrix).
    check(r.source == SRC_MACRO && (r.arg as usize) < N_MACROS)?;
    check(r.via == SRC_NONE || (r.via == SRC_MACRO && (r.via_arg as usize) < N_MACROS))?;
    check(r.target_kind == TGT_VALUE && r.mode != MODE_PULL && !r.latch)?;
    Ok(r)
}

fn read_route(w: &mut Words, n_values: usize) -> Result<Route, Malformed> {
    let mut r = Route::EMPTY;
    r.source = w.small(u16::MAX as usize)? as u16;
    r.arg = w.small(u16::MAX as usize)? as u16;
    r.target_kind = w.small(TGT_OCTAVES as usize)? as u8;
    r.n_targets = w.small(MAX_TARGETS)? as u8;
    for t in 0..MAX_TARGETS {
        let x = w.u()?;
        if t < r.n_targets as usize {
            check((x as usize) < n_values)?;
            r.targets[t] = x as u16;
        }
    }
    check(r.target_kind == TGT_PITCH || r.n_targets >= 1)?;
    r.amount = w.small(n_values - 1)? as u16;
    r.via = w.small(u16::MAX as usize)? as u16;
    r.via_arg = w.small(u16::MAX as usize)? as u16;
    r.mode = w.small(2)? as u8;
    r.polarity = w.small(2)? as u8;
    r.latch = w.small(1)? == 1;
    r.group = w.small(GROUP_FOLLOW as usize)? as u8;
    r.unit = w.f()?;
    r.in_lo = w.f()?;
    r.in_hi = w.f()?;
    r.bend = w.f()?.clamp(-1.0, 1.0);
    r.rise_ms = w.f()?.max(0.0);
    r.fall_ms = w.f()?.max(0.0);
    r.n_points = w.small(MAX_POINTS)? as u8;
    for k in 0..MAX_POINTS {
        let (x, y) = (w.f()?, w.f()?);
        r.points[k] = [x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)];
    }
    let pts = &r.points[..r.n_points as usize];
    check(pts.windows(2).all(|p| p[0][0] <= p[1][0]))?;
    r.identity = r.in_lo == 0.0 && r.in_hi == 1.0 && r.bend == 0.0 && r.n_points < 2;
    Ok(r)
}
