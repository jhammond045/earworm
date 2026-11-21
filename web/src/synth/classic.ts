// The classic patch (docs/synth.md "The classic patch and today's presets"): the fixed Klont chain
// of milestones M1 to M4 written as a patch, with today's gesture, hit-shape, LFO and macro maps as
// its default routes. crates/klont/src/classic.rs builds the same patch in Rust; classic.test.ts
// checks the two blobs agree.
//
// The Klont vocabulary (the old 96 parameters, gen/params.ts) maps onto the classic patch's values
// with VOCAB_KEYS. The values keep the old parameters' descriptors, so an old normalized set maps
// over unchanged. Influence matching and old 96-value sets in the app go through this binding.

import { DESCS, MACRO_NAMES, MACRO_W } from '../gen/params';
import { DESC_ID, MODULE_TYPES } from '../gen/modules';
import { KIT_SLOTS, type LayerKind } from '../project/types';
import { layoutOf, type Patch, type Route, type VoicePatch } from './patch';
import { toNorm } from './values';

const r = (id: string, source: string, target: string, mode: Route['mode'], polarity: Route['polarity'], more: Partial<Route> = {}): Route => ({
  id, source, target, mode, polarity, inLo: 0, inHi: 1, bend: 0, points: null, riseMs: 0, fallMs: 0, latch: false, via: null, group: null, ...more,
});
const follow = { latch: true, group: 'follow' as const };
const f32 = Math.fround;
/** 2 ln 2 / ln(hi / lo): moves a log-scaled value by one hit-hold doubling. */
const ln2Over = (lo: number, hi: number) => f32((2 * Math.LN2) / Math.log(hi / lo));
const holdIn = { inLo: f32(Math.log2(0.05)), inHi: f32(Math.log2(0.8)) };

/** The voice routes, in evaluation order. A fixed constant rides in `unit` with amount 1. */
function voiceRoutes(): Route[] {
  return [
    // Hit shape (beat layers; the engine leaves the fields empty elsewhere), scaled by Follow.
    r('hit_bend', 'note.bend', 'route.env3_pitch', 'add', 'up', { ...follow, inLo: -1e6, inHi: 1e6, unit: -1 }),
    r('hit_attack', 'note.attack', 'env1.attack', 'pull', 'up', { ...follow, inLo: f32(0.001), inHi: 1 }),
    r('hit_burst', 'note.burst', 'click1.start', 'add', 'up', { ...follow, unit: 0.5 }),
    r('hit_hold', 'note.hold', 'env1.hold', 'add', 'both', { ...follow, ...holdIn, unit: ln2Over(0.0005, 2) }),
    r('hit_hold_decay', 'note.hold', 'env1.decay', 'add', 'both', { ...follow, ...holdIn, unit: ln2Over(0.005, 8) }),
    r('hit_hold_noise', 'note.hold', 'env2.decay', 'add', 'both', { ...follow, ...holdIn, unit: ln2Over(0.005, 4) }),
    r('hit_vowel_tilt', 'note.vowel', 'partials1.tilt', 'add', 'both', { ...follow, unit: f32(0.2) }),
    r('hit_vowel_f1', 'note.vowel', 'partials1.f1_freq', 'add', 'both', { ...follow, unit: 0.25 }),
    r('hit_tone', 'note.tone', 'partials1.level', 'pull', 'up', follow),
    r('hit_tone_noise', 'note.tone', 'shaped_noise1.level', 'pull', 'up', { ...follow, points: [[0, 1], [1, 0]] }),
    r('hit_end_click', 'note.endClick', 'click1.end', 'add', 'up', follow),
    // Note hardness sharpens the start.
    r('hard_click', 'note.hardness', 'click1.start', 'add', 'up', { latch: true, group: 'hard', unit: 0.5 }),
    r('hard_attack', 'note.hardness', 'env1.attack', 'add', 'up', { latch: true, group: 'hard', unit: f32(-0.3) }),
    // Velocity and the sung volume on everything the voice sounds.
    r('vel_amp', 'note.velocity', 'amp1.gain', 'scale', 'down'),
    r('vel_noise', 'note.velocity', 'shaped_noise1.level', 'scale', 'down'),
    r('vel_grains', 'note.velocity', 'grains1.level', 'scale', 'down'),
    r('loud_amp', 'voice.volume', 'amp1.gain', 'scale', 'down', { group: 'loud' }),
    r('loud_noise', 'voice.volume', 'shaped_noise1.level', 'scale', 'down', { group: 'loud' }),
    r('loud_grains', 'voice.volume', 'grains1.level', 'scale', 'down', { group: 'loud' }),
    // Openness, breath and mouth.
    r('open_tilt', 'voice.openness', 'partials1.tilt', 'add', 'up', { group: 'open', unit: f32(0.4) }),
    r('open_f1', 'voice.openness', 'partials1.f1_freq', 'add', 'up', { group: 'open', unit: 0.25 }),
    r('open_cutoff', 'voice.openness', 'filter1.cutoff', 'add', 'up', { group: 'open', unit: f32(0.2) }),
    r('breath_noise', 'voice.breath', 'shaped_noise1.level', 'add', 'up', { group: 'breath', unit: 0.5 }),
    r('mouth', 'voice.vowel', 'bands:partials1.tone_bands', 'pull', 'up', { group: 'mouth' }),
    // Envelopes and LFOs.
    r('env1_amp', 'env1', 'amp1.gain', 'scale', 'down'),
    r('env3_pitch', 'env3', 'pitch', 'add', 'up', { unit: 100 }),
    r('lfo1_pitch', 'lfo1', 'pitch', 'add', 'both', { unit: 1 }),
    r('lfo2_pitch', 'lfo2', 'pitch', 'add', 'both', { unit: 1 }),
    r('lfo3_pitch', 'lfo3', 'pitch', 'add', 'both', { unit: 50 }),
    r('lfo1_cutoff', 'lfo1', 'oct:filter1.cutoff', 'add', 'both'),
    r('lfo2_cutoff', 'lfo2', 'oct:filter1.cutoff', 'add', 'both'),
    r('lfo3_cutoff', 'lfo3', 'oct:filter1.cutoff', 'add', 'both'),
    r('lfo1_level', 'lfo1', 'amp1.gain', 'scale', 'down'),
    r('lfo2_grains', 'lfo2', 'grains1.pos', 'add', 'both', { unit: 0.25 }),
  ];
}

/** Old parameter id -> the keys (without the kit prefix) it becomes, in layout order. */
export const VOCAB_KEYS: Readonly<Record<string, readonly string[]>> = (() => {
  const m: Record<string, string[]> = {
    'src.harm_mix': ['partials1.level'], 'src.noise_mix': ['shaped_noise1.level'], 'src.grain_mix': ['grains1.level'],
    'grain.size': ['grains1.size'], 'grain.density': ['grains1.density'], 'grain.pos': ['grains1.pos'],
    'grain.spray': ['grains1.spray'], 'grain.scatter': ['grains1.scatter'],
    'fold.amt': ['fold1.amount'], 'fold.bias': ['fold1.bias'], 'clang.amt': ['clang1.amount'], 'clang.ratio': ['clang1.ratio'],
    'comb.amt': ['comb1.amount'], 'comb.tune': ['comb1.tune'], 'comb.decay': ['comb1.decay'],
    'crush.bits': ['crush1.bits'], 'crush.rate': ['crush1.rate'], 'tape.amt': ['tape1.amount'],
    'filt.cutoff': ['filter1.cutoff'], 'filt.res': ['filter1.res'], 'filt.morph': ['filter1.morph'], 'filt.track': ['filter1.track'],
    'env.attack': ['env1.attack'], 'env.hold': ['env1.hold'], 'env.decay': ['env1.decay'], 'env.sustain': ['env1.sustain'],
    'env.release': ['env1.release', 'env2.release'], 'env.noise_decay': ['env2.decay'],
    'env.pitch_amt': ['route.env3_pitch'], 'env.pitch_time': ['env3.decay'],
    'strike.click': ['click1.start'], 'strike.color': ['click1.colour'], 'strike.end_click': ['click1.end'],
    'lfo1.rate': ['lfo1.rate'], 'lfo1.to_pitch': ['route.lfo1_pitch'], 'lfo1.to_filter': ['route.lfo1_cutoff'], 'lfo1.to_level': ['route.lfo1_level'],
    'lfo2.rate': ['lfo2.rate'], 'lfo2.to_pitch': ['route.lfo2_pitch'], 'lfo2.to_filter': ['route.lfo2_cutoff'], 'lfo2.to_grain': ['route.lfo2_grains'],
    'chaos.rate': ['lfo3.rate'], 'chaos.amt': ['route.lfo3_pitch', 'route.lfo3_cutoff'],
    'delay.time': ['delay1.time'], 'delay.fb': ['delay1.feedback'], 'delay.shift': ['delay1.shift'], 'delay.mix': ['delay1.mix'],
    'verb.size': ['reverb1.size'], 'verb.damp': ['reverb1.damp'], 'verb.mix': ['reverb1.mix'], 'verb.freeze': ['reverb1.freeze'],
    width: ['width'],
  };
  for (const p of ['tilt', 'oddeven', 'stretch', 'jitter', 'modal', 'damp', 'f1_freq', 'f1_gain', 'f2_freq', 'f2_gain']) m[`src.${p}`] = [`partials1.${p}`];
  for (let b = 0; b < 16; b++) {
    m[`tone_bands.${b}`] = [`partials1.tone_bands.${b}`];
    m[`noise_bands.${b}`] = [`shaped_noise1.bands.${b}`];
  }
  return m;
})();

/** Keys of the channel (the layer's own set on a beat layer). */
const CHANNEL_KEY = /^(crush1|tape1|delay1|reverb1)\.|^width$/;

function classicVoice(): VoicePatch {
  return {
    modules: [
      { id: 'partials1', type: 'partials' }, { id: 'amp1', type: 'amp' }, { id: 'click1', type: 'click' },
      { id: 'clang1', type: 'clang' }, { id: 'fold1', type: 'fold' }, { id: 'comb1', type: 'comb' }, { id: 'filter1', type: 'filter' },
    ],
    cables: [['partials1', 'amp1'], ['amp1', 'clang1'], ['click1', 'clang1'], ['clang1', 'fold1'], ['fold1', 'comb1'], ['comb1', 'filter1']]
      .map(([from, to]) => ({ from, to, port: 'in' as const })),
    out: ['filter1'],
    mods: [
      { id: 'env1', type: 'env' }, { id: 'env2', type: 'env' }, { id: 'env3', type: 'env' },
      { id: 'lfo1', type: 'lfo' }, { id: 'lfo2', type: 'lfo' }, { id: 'lfo3', type: 'lfo' },
    ],
    routes: voiceRoutes(),
  };
}

/** Module table default descriptor of a key, e.g. 'env2.decay' -> env.decay. */
function defaultDesc(key: string): number | null {
  const [id, param, band] = key.split('.');
  const t = MODULE_TYPES[id.replace(/\d+$/, '')];
  const p = t?.params.find((x) => x.name === param);
  return p ? p.desc + (band ? Number(band) : 0) : null;
}

/** The classic patch for a layer kind: one voice patch, or four on a beat layer. */
export function classicPatch(kind: LayerKind): Patch {
  const n = kind === 'beat' ? KIT_SLOTS : 1;
  const descs: Record<string, string> = {};
  for (const [id, keys] of Object.entries(VOCAB_KEYS)) {
    for (const k of keys) if (defaultDesc(k) !== DESC_ID[id]) descs[k] = id;
  }
  const defaults: Record<string, number> = { 'env2.mode': 1, 'env3.mode': 2, 'env2.sustain': 0, 'env3.sustain': 0, 'lfo3.wave': 1 };
  const voices = Array.from({ length: n }, classicVoice);
  for (const rt of voices[0].routes) if (!rt.id.match(/^(env3_pitch|lfo\d_(pitch|cutoff|level|grains))$/)) defaults[`route.${rt.id}`] = 1;
  const patch: Patch = {
    voices,
    layerSources: voices.flatMap((_, k) => [
      { id: 'shaped_noise1', type: 'shaped_noise' as const, voice: k, drive: 'env2' },
      { id: 'grains1', type: 'grains' as const, voice: k, drive: 'env1' },
    ]),
    channel: [
      { id: 'crush1', type: 'crush', bypass: false }, { id: 'tape1', type: 'tape', bypass: false },
      { id: 'delay1', type: 'delay', bypass: false }, { id: 'reverb1', type: 'reverb', bypass: false },
    ],
    lfos: [],
    routes: [],
    macroNames: [...MACRO_NAMES],
    descs,
    defaults,
  };
  // Macro weights become layer routes, macro by macro so each value sums them in the old order.
  const lay = layoutOf(patch);
  const ord = (k: string) => lay.index.get(kind === 'beat' && !CHANNEL_KEY.test(k) ? `k1.${k}` : k) ?? -1;
  for (let m = 0; m < MACRO_NAMES.length; m++) {
    DESCS.forEach((d, i) => {
      const w = MACRO_W[m][i];
      if (w === 0) return;
      const keys = [...(VOCAB_KEYS[d.id] ?? [])].filter((k) => ord(k) >= 0).sort((a, b) => ord(a) - ord(b));
      keys.forEach((k, j) => {
        const id = `macro${m + 1}_${i}_${j}`;
        patch.routes.push(r(id, `macro${m + 1}`, kind === 'beat' && !CHANNEL_KEY.test(k) ? `k*.${k}` : k, 'add', 'both', { unit: w }));
        defaults[`route.${id}`] = 1;
      });
    });
  }
  return patch;
}

/** Is this patch the classic one (the only structure stage S1 plays). */
export function isClassic(p: Patch): boolean {
  return p.voices.length >= 1 && p.voices[0].modules[0]?.id === 'partials1' && p.channel.length === 4;
}

/** Keys an old parameter becomes in a layer of `kind`: set 0 is the layer's own set (on a beat
 *  layer only its channel keys), set 1..4 a kit slot. */
export function vocabKeys(kind: LayerKind, set: number, id: string): string[] {
  const keys = VOCAB_KEYS[id] ?? [];
  if (kind !== 'beat') return set === 0 ? [...keys] : [];
  if (set === 0) return keys.filter((k) => CHANNEL_KEY.test(k));
  return keys.filter((k) => !CHANNEL_KEY.test(k)).map((k) => `k${set}.${k}`);
}

export interface Binding {
  /** 0 the layer's own set, 1..4 a kit slot. */
  set: number;
  /** Index of the old parameter (gen/params.ts DESCS order). */
  vocab: number;
}

const bindingCache = new Map<string, (Binding | null)[]>();
const patchBindings = new WeakMap<Patch, (Binding | null)[]>();

/** For each value of a classic patch of `kind` (or of `patch`, a classic patch with routes or layer
 *  LFOs added), the old parameter it is bound to, if any. */
export function bindingOf(kind: LayerKind, patch?: Patch): (Binding | null)[] {
  const key = kind === 'beat' ? 'beat' : 'melody';
  let b = patch ? patchBindings.get(patch) : bindingCache.get(key);
  if (b) return b;
  const lay = layoutOf(patch ?? classicPatch(kind));
  b = lay.keys.map(() => null);
  const sets = kind === 'beat' ? [0, 1, 2, 3, 4] : [0];
  DESCS.forEach((d, i) => {
    for (const s of sets) for (const k of vocabKeys(kind, s, d.id)) {
      const at = lay.index.get(k);
      if (at !== undefined) b![at] = { set: s, vocab: i };
    }
  });
  if (patch) patchBindings.set(patch, b);
  else bindingCache.set(key, b);
  return b;
}

/** Values of a classic layer from old normalized sets: the layer's own set and the kit slots'.
 *  Values bound to nothing take the patch's starting value. */
export function valuesFromVocab(kind: LayerKind, base: ArrayLike<number>, kit: ArrayLike<number>[] = []): Record<string, number> {
  const p = classicPatch(kind);
  const lay = layoutOf(p);
  const bind = bindingOf(kind);
  const out: Record<string, number> = {};
  lay.keys.forEach((k, i) => {
    const b = bind[i];
    const src = b ? (b.set === 0 ? base : kit[b.set - 1]) : undefined;
    out[k] = src && src[b!.vocab] !== undefined ? src[b!.vocab] : lay.init[i];
  });
  return out;
}

/** An old normalized set read back from a classic layer's values: set 0 the layer's own set, 1..4 a
 *  kit slot. Old parameters with no value (smear; voice parameters of a beat layer's own set) are
 *  neutral; one bound to several values takes the first. */
export function vocabFromValues(kind: LayerKind, values: Record<string, number>, set = 0): number[] {
  const out = DESCS.map((d, i) => neutralOf(i));
  DESCS.forEach((d, i) => {
    const k = vocabKeys(kind, set, d.id)[0];
    if (k !== undefined && values[k] !== undefined) out[i] = values[k];
  });
  return out;
}

/** Set old parameters (a normalized set) onto a classic layer's values, for set 0 or a kit slot. */
export function withVocab(kind: LayerKind, values: Record<string, number>, set: number, vocab: ArrayLike<number>): Record<string, number> {
  const out = { ...values };
  DESCS.forEach((d, i) => {
    for (const k of vocabKeys(kind, set, d.id)) out[k] = vocab[i];
  });
  return out;
}

const neutralOf = (i: number) => toNorm(i, DESCS[i].neutral);

/** Values, Influence target and group bits in layout order for a classic layer blob. `target` is
 *  the Influence target as an old normalized set (the layer's own set), or null for none. `patch`:
 *  the layer's patch when it is the classic one with routes or layer LFOs added. */
export function classicVectors(kind: LayerKind, values: Record<string, number>, target: ArrayLike<number> | null, patch?: Patch): { values: number[]; target: number[]; groups: number[] } {
  const lay = layoutOf(patch ?? classicPatch(kind));
  const bind = bindingOf(kind, patch);
  const v = lay.keys.map((k, i) => values[k] ?? lay.init[i]);
  const t = v.map((x, i) => {
    const b = bind[i];
    return target && b && b.set === 0 ? target[b.vocab] : x;
  });
  const g = bind.map((b) => (b && b.set === 0 ? 1 << GROUP_BIT[DESCS[b.vocab].group] : 0));
  return { values: v, target: t, groups: g };
}

const GROUP_BIT = { tone: 0, shape: 1, motion: 2, grit: 3, space: 4 } as const;

/** The old parameters of a layer's state (its own set), for code that still reads 96-value sets. */
export function stateVocab(l: { kind: LayerKind }, st: { values: Record<string, number> }): number[] {
  return vocabFromValues(l.kind, st.values);
}

/** Value indices of each old parameter in the layer's own set, for MSG_SET_PARAM. */
export function vocabIndices(kind: LayerKind, patch?: Patch): number[][] {
  const lay = layoutOf(patch ?? classicPatch(kind));
  return DESCS.map((d) => vocabKeys(kind, 0, d.id).map((k) => lay.index.get(k)).filter((x): x is number => x !== undefined));
}
