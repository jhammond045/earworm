// Patches (docs/synth.md "Patches", "Serialization"): the document form of a layer's sound
// structure, its value layout and its compiled LOAD_PATCH blob (crates/klont/src/patch.rs).
//
// A value's key is `<id>.<param>` (`filter1.cutoff`, `env2.decay`, `route.lfo1_pitch`); a band
// shape gives 16 keys (`partials1.tone_bands.0` to `.15`). On a beat layer every key of kit slot
// n's voice patch starts with `k<n>.`. The layout (the order of values in the engine's vector) is
// fixed by the patch: for each voice patch its modules, mods, voicing depth, layer sources and
// route amounts; then the channel's effects, width, the layer LFOs and the layer routes' amounts.
//
// Sources (docs/synth.md "Sources"): per voice the voice patch's mods (`env1`, `lfo2`), `note.*` and
// `voice.*`; per layer `macro1`..`macro8`, `wheel`, `bend`, `pressure`, `layer.lfo1`/`layer.lfo2` and
// `layer.*` (the layer's sung features). A voice route takes a per-voice source; a route whose source
// is per layer is a layer route (patch.routes). Either may take a layer source as its via.

import { ALL_DESCS, DESC_ID, MODULE_TYPES } from '../gen/modules';
import { toNorm } from './values';

export type VoiceModuleType = 'partials' | 'click' | 'filter' | 'amp' | 'fold' | 'clang' | 'comb';
export type ChannelFxType = 'crush' | 'tape' | 'delay' | 'reverb';
export type Group = 'loud' | 'open' | 'hard' | 'breath' | 'mouth' | 'follow';

/** An envelope or LFO. `trigger: 'onset'` also restarts an envelope at each sung onset. A layer
 *  LFO's id is `layer.lfo1` or `layer.lfo2`. */
export interface ModDef { id: string; type: 'env' | 'lfo'; trigger?: 'note' | 'onset' }

export interface Route {
  id: string;
  /** 'env1', 'lfo2' (a mod of the voice patch), 'note.velocity', 'voice.volume', 'macro3', ... */
  source: string;
  /** A value key, 'pitch', 'bands:<key>' (a 16-band shape), 'oct:<key>' (a log-scaled value moved
   *  in octaves after mapping). Layer routes may write 'k*.<key>' for every kit slot at once. */
  target: string;
  mode: 'add' | 'scale' | 'pull';
  inLo: number; inHi: number;
  bend: number;
  points: [number, number][] | null;
  riseMs: number; fallMs: number;
  latch: boolean;
  polarity: 'up' | 'down' | 'both';
  via: string | null;
  group: Group | null;
  /** Target units per unit of amount (default: 1, or 4800 cents for pitch). */
  unit?: number;
}

export interface VoicePatch {
  modules: { id: string; type: VoiceModuleType }[];
  cables: { from: string; to: string; port: 'in' }[];
  /** Modules summed into the voice output. */
  out: string[];
  /** env1..env4, lfo1..lfo4; the first must be an envelope (it ends the voice). */
  mods: ModDef[];
  routes: Route[];
}

export interface LayerSource {
  id: string;
  type: 'shaped_noise' | 'grains';
  /** Index of the voice patch whose voices feed it. */
  voice: number;
  /** The envelope of that voice patch that drives its level. */
  drive: string;
}

export interface ChannelFx { id: string; type: ChannelFxType; bypass: boolean }

export interface Patch {
  /** 1 for melody and texture, 4 for beat (one per kit slot). */
  voices: VoicePatch[];
  layerSources: LayerSource[];
  /** Signal order. */
  channel: ChannelFx[];
  /** Layer LFOs: at most two, ids `layer.lfo1` and `layer.lfo2`. */
  lfos: ModDef[];
  /** Layer routes. */
  routes: Route[];
  macroNames: string[];
  /** Descriptor per key (without the kit prefix) where it is not the module table's default.
   *  The classic patch keeps the old parameters' ranges this way. */
  descs?: Record<string, string>;
  /** Starting value per key (without the kit prefix), real units, where it is not the
   *  descriptor's neutral. A value missing from a state takes it. */
  defaults?: Record<string, number>;
}

// ---- engine codes (crates/klont/src/patch.rs)

export const PATCH_MAGIC = 0x32505745;
export const MAX_POINTS = 8;
export const MAX_TARGETS = 4;
const NONE = 0xffffffff;

/** Sung features, in the engine's order (pitch to voicing, then the vowel, then intensity). */
export const FEATURES = ['pitch', 'volume', 'openness', 'breath', 'hardness', 'onset', 'voicing', 'vowel', 'intensity'] as const;
const NOTE_FIELDS = ['pitch', 'velocity', 'hardness', 'attack', 'burst', 'bend', 'hold', 'vowel', 'tone', 'endClick'] as const;

/** Per-voice sources that are not the voice patch's own mods. */
const VOICE_SRC: Record<string, number> = {
  ...Object.fromEntries(NOTE_FIELDS.map((f, i) => [`note.${f}`, 10 + i])),
  ...Object.fromEntries(FEATURES.map((f, i) => [`voice.${f}`, 30 + i])),
};
/** Per-layer sources: [code, arg]. */
const LAYER_SRC: Record<string, [number, number]> = {
  ...Object.fromEntries(Array.from({ length: 8 }, (_, k) => [`macro${k + 1}`, [50, k]])),
  wheel: [51, 0], bend: [51, 1], pressure: [51, 2],
  'layer.lfo1': [52, 0], 'layer.lfo2': [52, 1],
  ...Object.fromEntries(FEATURES.map((f, i) => [`layer.${f}`, [60 + i, 0]])),
};
const SRC_MOD = 1;
/** Added to an envelope's type code: it also restarts at each sung onset. */
const MOD_ONSET = 0x10000;

/** Every per-voice source name (the voice patch's mods aside). */
export const VOICE_SOURCES: readonly string[] = Object.keys(VOICE_SRC);
/** Every per-layer source name. */
export const LAYER_SOURCES: readonly string[] = Object.keys(LAYER_SRC);
/** A source that is one per layer (its routes are layer routes). */
export const isLayerSource = (s: string): boolean => s in LAYER_SRC;
/** Sources whose value runs -1..1 (they enter the shaper as 0..1). */
export const isBipolarSource = (s: string, mods: ModDef[] = []): boolean =>
  s === 'bend' || /^layer\.lfo[12]$/.test(s) || mods.some((m) => m.id === s && m.type === 'lfo');
const TGT = { value: 0, pitch: 1, bands: 2, oct: 3 } as const;
const MODE = { add: 0, scale: 1, pull: 2 } as const;
const POL = { up: 0, down: 1, both: 2 } as const;
const GROUP: Record<Group, number> = { loud: 1, open: 2, hard: 3, breath: 4, mouth: 5, follow: 6 };

// ---- layout

export interface Layout {
  keys: string[];
  /** [first, end) of each voice patch's values. */
  ranges: [number, number][];
  /** Descriptor id per value. */
  descs: number[];
  /** Starting value per value, normalized. */
  init: number[];
  index: Map<string, number>;
}

const prefix = (patch: Patch, vp: number) => (patch.voices.length > 1 ? `k${vp + 1}.` : '');

export function moduleKeys(id: string, type: string): { key: string; local: string; desc: number }[] {
  const t = MODULE_TYPES[type] ?? fail(`unknown module type ${type}`);
  const out: { key: string; local: string; desc: number }[] = [];
  for (const p of t.params) {
    for (let i = 0; i < p.count; i++) {
      const local = p.count > 1 ? `${id}.${p.name}.${i}` : `${id}.${p.name}`;
      out.push({ key: local, local, desc: p.count > 1 ? p.desc + i : p.desc });
    }
  }
  return out;
}

function fail(msg: string): never {
  throw new RangeError(`patch: ${msg}`);
}

/** The value layout of a patch. */
export function layoutOf(patch: Patch): Layout {
  const keys: string[] = [], descs: number[] = [], init: number[] = [];
  const ranges: [number, number][] = [];
  const add = (pre: string, local: string, desc: number) => {
    const named = patch.descs?.[local];
    const d = named !== undefined ? (DESC_ID[named] ?? fail(`unknown descriptor ${named}`)) : desc;
    const def = patch.defaults?.[local];
    keys.push(pre + local);
    descs.push(d);
    init.push(toNorm(d, def ?? ALL_DESCS[d].neutral));
  };
  patch.voices.forEach((vp, k) => {
    const pre = prefix(patch, k);
    const lo = keys.length;
    for (const m of vp.modules) for (const x of moduleKeys(m.id, m.type)) add(pre, x.local, x.desc);
    for (const m of vp.mods) for (const x of moduleKeys(m.id, m.type)) add(pre, x.local, x.desc);
    add(pre, 'voicing.depth', DESC_ID['voicing.depth']);
    for (const s of patch.layerSources.filter((x) => x.voice === k)) for (const x of moduleKeys(s.id, s.type)) add(pre, x.local, x.desc);
    for (const r of vp.routes) add(pre, `route.${r.id}`, DESC_ID['route.amount']);
    ranges.push([lo, keys.length]);
  });
  for (const fx of patch.channel) for (const x of moduleKeys(fx.id, fx.type)) add('', x.local, x.desc);
  add('', 'width', DESC_ID['width']);
  for (const m of patch.lfos) for (const x of moduleKeys(m.id, m.type)) add('', x.local, x.desc);
  for (const r of patch.routes) add('', `route.${r.id}`, DESC_ID['route.amount']);
  const index = new Map(keys.map((k, i) => [k, i]));
  if (index.size !== keys.length) fail('two values have the same key');
  return { keys, ranges, descs, init, index };
}

// ---- blob

class Words {
  private buf = new ArrayBuffer(1 << 16);
  private v = new DataView(this.buf);
  n = 0;
  private room(): void {
    if ((this.n + 1) * 4 <= this.buf.byteLength) return;
    const b = new ArrayBuffer(this.buf.byteLength * 2);
    new Uint8Array(b).set(new Uint8Array(this.buf));
    this.buf = b;
    this.v = new DataView(b);
  }
  u(x: number): void { this.room(); this.v.setUint32(this.n++ * 4, x >>> 0, true); }
  f(x: number): void { this.room(); this.v.setFloat32(this.n++ * 4, x, true); }
  done(): ArrayBuffer { return this.buf.slice(0, this.n * 4); }
}

/** Value indices of a target key; 'k*.' reaches every kit slot. */
function targets(lay: Layout, patch: Patch, key: string, vpPrefix: string): number[] {
  if (key.startsWith('k*.')) {
    return patch.voices.map((_, k) => lay.index.get(`${prefix(patch, k)}${key.slice(3)}`) ?? fail(`no value ${key} in kit slot ${k + 1}`));
  }
  return [lay.index.get(vpPrefix + key) ?? fail(`no value ${vpPrefix}${key}`)];
}

function writeRoute(w: Words, r: Route, lay: Layout, patch: Patch, vp: number | null): void {
  const pre = vp === null ? '' : prefix(patch, vp);
  const mods = vp === null ? [] : patch.voices[vp].mods;
  const layerSource = (s: string): [number, number] | null => {
    const l = LAYER_SRC[s];
    if (!l) return null;
    if (l[0] === 52 && !patch.lfos.some((m) => m.id === s)) fail(`route ${r.id}: the patch has no ${s}`);
    return l[0] === 52 ? [52, patch.lfos.findIndex((m) => m.id === s)] : l;
  };
  const voiceSource = (s: string): [number, number] | null => {
    const m = mods.findIndex((x) => x.id === s);
    if (m >= 0) return [SRC_MOD, m];
    return s in VOICE_SRC ? [VOICE_SRC[s], 0] : null;
  };
  const source = (s: string): [number, number] =>
    (vp === null ? layerSource(s) : voiceSource(s))
    ?? fail(isLayerSource(s) ? `route ${r.id}: ${s} is one per layer, so its route is a layer route` : vp === null ? `route ${r.id}: ${s} is per voice, so its route belongs to a voice patch` : `unknown source ${s}`);
  const via = (s: string): [number, number] => voiceSource(s) ?? layerSource(s) ?? fail(`route ${r.id}: unknown via ${s}`);
  const [src, arg] = source(r.source);
  w.u(src);
  w.u(arg);
  let kind: number = TGT.value, ts: number[] = [];
  if (r.target === 'pitch') kind = TGT.pitch;
  else if (r.target.startsWith('bands:')) { kind = TGT.bands; ts = targets(lay, patch, `${r.target.slice(6)}.0`, pre); }
  else if (r.target.startsWith('oct:')) { kind = TGT.oct; ts = targets(lay, patch, r.target.slice(4), pre); }
  else ts = targets(lay, patch, r.target, pre);
  if (ts.length > MAX_TARGETS) fail(`route ${r.id} has more than ${MAX_TARGETS} targets`);
  w.u(kind);
  w.u(ts.length);
  for (let t = 0; t < MAX_TARGETS; t++) w.u(ts[t] ?? 0);
  w.u(lay.index.get(`${pre}route.${r.id}`)!);
  const [vs, va] = r.via ? (vp === null ? (layerSource(r.via) ?? fail(`route ${r.id}: a layer route's via must be one per layer`)) : via(r.via)) : [0, 0];
  w.u(vs);
  w.u(va);
  w.u(MODE[r.mode]);
  w.u(POL[r.polarity]);
  w.u(r.latch ? 1 : 0);
  w.u(r.group ? GROUP[r.group] : 0);
  w.f(r.unit ?? (kind === TGT.pitch ? 4800 : 1));
  w.f(r.inLo);
  w.f(r.inHi);
  w.f(r.bend);
  w.f(r.riseMs);
  w.f(r.fallMs);
  const pts = r.points ?? [];
  if (pts.length > MAX_POINTS) fail(`route ${r.id} has more than ${MAX_POINTS} points`);
  w.u(pts.length);
  for (let k = 0; k < MAX_POINTS; k++) {
    w.f(pts[k]?.[0] ?? 0);
    w.f(pts[k]?.[1] ?? 0);
  }
}

/** The LOAD_PATCH blob of a patch whose grains read sample `sample`. */
export function patchBlob(patch: Patch, sample: number): ArrayBuffer {
  const lay = layoutOf(patch);
  const w = new Words();
  const channel = patch.channel.filter((x) => !x.bypass);
  w.u(PATCH_MAGIC);
  w.u(lay.keys.length);
  w.u(patch.voices.length);
  w.u(channel.length);
  w.u(patch.routes.length);
  w.u(lay.index.get('width')!);
  if (patch.lfos.length > 2 || patch.lfos.some((m) => m.type !== 'lfo' || !/^layer\.lfo[12]$/.test(m.id))) fail('layer LFOs are layer.lfo1 and layer.lfo2');
  w.u(patch.lfos.length);
  lay.descs.forEach((d) => w.u(d));
  lay.init.forEach((x) => w.f(x));
  patch.voices.forEach((vp, k) => {
    const pre = prefix(patch, k);
    const at = (key: string) => lay.index.get(pre + key) ?? fail(`no value ${pre}${key}`);
    const mi = (id: string) => { const i = vp.modules.findIndex((m) => m.id === id); return i >= 0 ? i : fail(`no module ${id}`); };
    const first = (id: string, type: string) => at(moduleKeys(id, type)[0].local);
    const [lo, hi] = lay.ranges[k];
    w.u(vp.modules.length);
    w.u(vp.cables.length);
    w.u(vp.out.length);
    w.u(vp.mods.length);
    w.u(vp.routes.length);
    w.u(lo);
    w.u(hi);
    w.u(at('voicing.depth'));
    for (const type of ['shaped_noise', 'grains'] as const) {
      const s = patch.layerSources.find((x) => x.voice === k && x.type === type);
      if (!s) {
        w.u(NONE);
        w.u(0);
        if (type === 'grains') w.u(0);
        continue;
      }
      w.u(first(s.id, s.type));
      w.u(vp.mods.findIndex((m) => m.id === s.drive));
      if (type === 'grains') w.u(sample);
    }
    for (const m of vp.modules) {
      w.u(MODULE_TYPES[m.type].code);
      w.u(first(m.id, m.type));
    }
    for (const c of vp.cables) {
      w.u(mi(c.from));
      w.u(mi(c.to));
      w.u(0);
    }
    for (const o of vp.out) w.u(mi(o));
    for (const m of vp.mods) {
      w.u(MODULE_TYPES[m.type].code + (m.trigger === 'onset' ? MOD_ONSET : 0));
      w.u(first(m.id, m.type));
    }
    for (const r of vp.routes) writeRoute(w, r, lay, patch, k);
  });
  for (const fx of channel) {
    w.u(MODULE_TYPES[fx.type].code);
    w.u(lay.index.get(moduleKeys(fx.id, fx.type)[0].local)!);
  }
  for (const m of patch.lfos) {
    w.u(MODULE_TYPES.lfo.code);
    w.u(lay.index.get(moduleKeys(m.id, m.type)[0].local)!);
  }
  for (const r of patch.routes) writeRoute(w, r, lay, patch, null);
  return w.done();
}
