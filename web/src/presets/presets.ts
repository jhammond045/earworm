// Presets (docs/synth.md "Serialization", "The classic patch and today's presets"). Factory presets
// are the ten sounds of m2/sounds.ts as preset files of the classic patch, one JSON file each in this
// folder; presets.test.ts writes them (WRITE_PRESETS=1, or `pnpm gen:presets`) and checks they are
// current. User presets are the same format, saved from a layer, kept in the settings store and
// exported or imported as single files. An imported file is untrusted: parsePreset checks it.

import { soundToSet } from '../m1/layer';
import { BEAT_KITS, MELODY_SOUNDS } from '../m2/sounds';
import { classicPatch, valuesFromVocab } from '../synth/classic';
import { layoutOf, patchBlob, type Patch } from '../synth/patch';
import type { Layer, LayerKind, State } from '../project/types';

/** One layer's sound without its notes. */
export interface PresetFile {
  format: 'earworm-patch';
  v: 1;
  name: string;
  kind: LayerKind;
  patch: Patch;
  /** Normalized value per key of the patch. */
  values: Record<string, number>;
  macros: number[];
  /** Beat layers: default pitch of the four kit slots; null for melody and texture. */
  kitCents: number[] | null;
}

const set = (s: Record<string, number> | undefined) => Array.from(soundToSet(s));

/** File name of a preset: its name in lower case, words joined by hyphens. */
export const presetFileName = (name: string): string => `${name.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '')}.json`;

/** The factory presets built from m2/sounds.ts, melody sounds first. */
export function factoryPresets(): PresetFile[] {
  const melody = MELODY_SOUNDS.map((m): PresetFile => ({
    format: 'earworm-patch', v: 1, name: m.name, kind: 'melody', patch: classicPatch('melody'),
    values: valuesFromVocab('melody', set(m.sound)), macros: new Array<number>(8).fill(0.5), kitCents: null,
  }));
  const beat = BEAT_KITS.map((k): PresetFile => ({
    format: 'earworm-patch', v: 1, name: k.name, kind: 'beat', patch: classicPatch('beat'),
    values: valuesFromVocab('beat', set(k.base), k.kit.map(set)), macros: new Array<number>(8).fill(0.5), kitCents: [...k.kitCents],
  }));
  return [...melody, ...beat];
}

const fail = (msg: string): never => { throw new RangeError(msg); };
const isObj = (x: unknown): x is Record<string, unknown> => typeof x === 'object' && x !== null && !Array.isArray(x);
const inUnit = (x: unknown) => typeof x === 'number' && Number.isFinite(x);

/** A preset checked and cleaned: a known format, a patch that compiles with as many voice patches
 *  as its kind needs, values 0..1 for keys the patch has (others dropped), eight macros, and four
 *  kit pitches on a kit. Throws a RangeError in plain words. */
export function parsePreset(x: unknown): PresetFile {
  if (!isObj(x) || x.format !== 'earworm-patch') fail('this is not an Earworm preset file');
  const o = x as Record<string, unknown>;
  if (o.v !== 1) fail(`this preset was saved by a newer Earworm (format ${String(o.v)})`);
  const name = typeof o.name === 'string' && o.name.trim() ? o.name.trim().slice(0, 80) : fail('the preset has no name');
  const kind = o.kind === 'melody' || o.kind === 'beat' || o.kind === 'texture' ? o.kind : fail(`"${name}": unknown layer kind`);
  if (!isObj(o.patch)) fail(`"${name}" has no patch`);
  const raw = o.patch as Record<string, unknown>;
  const patch = {
    voices: raw.voices, layerSources: raw.layerSources, channel: raw.channel, lfos: raw.lfos, routes: raw.routes, macroNames: raw.macroNames,
    ...(raw.descs !== undefined ? { descs: raw.descs } : {}), ...(raw.defaults !== undefined ? { defaults: raw.defaults } : {}),
  } as Patch;
  try {
    if (!Array.isArray(patch.voices) || patch.voices.length !== (kind === 'beat' ? 4 : 1)) fail(kind === 'beat' ? 'a kit has four voice patches' : 'it needs one voice patch');
    for (const k of ['layerSources', 'channel', 'lfos', 'routes', 'macroNames'] as const) if (!Array.isArray(patch[k])) fail(`its ${k} is missing`);
    patchBlob(patch, 0);
  } catch (e) {
    fail(`"${name}": its patch does not work here (${e instanceof Error ? e.message.replace(/^patch: /, '') : String(e)})`);
  }
  const keys = new Set(layoutOf(patch).keys);
  const values: Record<string, number> = {};
  if (!isObj(o.values)) fail(`"${name}" has no values`);
  for (const [k, v] of Object.entries(o.values as Record<string, unknown>)) {
    if (!keys.has(k)) continue;
    if (!inUnit(v)) fail(`"${name}": value ${k} is not a number`);
    values[k] = Math.min(1, Math.max(0, v as number));
  }
  const macros = Array.isArray(o.macros) && o.macros.length === 8 && o.macros.every(inUnit)
    ? (o.macros as number[]).map((m) => Math.min(1, Math.max(0, m)))
    : new Array<number>(8).fill(0.5);
  let kitCents: number[] | null = null;
  if (kind === 'beat') {
    const kc = o.kitCents;
    kitCents = Array.isArray(kc) && kc.length === 4 && kc.every((c) => inUnit(c) && (c as number) >= 0 && (c as number) <= 12000) ? [...(kc as number[])] : fail(`"${name}": a kit needs four slot pitches`);
  }
  return { format: 'earworm-patch', v: 1, name, kind, patch, values, macros, kitCents };
}

/** The sound of a layer's state as a preset. */
export function presetFromLayer(l: Layer, st: State, name: string): PresetFile {
  return {
    format: 'earworm-patch', v: 1, name, kind: l.kind, patch: l.patch,
    values: { ...st.values }, macros: [...st.macros], kitCents: l.kind === 'beat' ? l.kit.map((k) => k.cents) : null,
  };
}

/** The layer's state sounds like this preset: same kind, patch, values and kit pitches. */
export function presetMatches(l: Layer, st: State, p: PresetFile): boolean {
  if ((p.kind === 'beat') !== (l.kind === 'beat')) return false;
  if (JSON.stringify(p.patch) !== JSON.stringify(l.patch)) return false;
  const lay = layoutOf(p.patch);
  if (!lay.keys.every((k, i) => (p.values[k] ?? lay.init[i]) === st.values[k])) return false;
  return l.kind !== 'beat' || !p.kitCents || p.kitCents.every((c, i) => l.kit[i]?.cents === c);
}

/** The text of a preset file, as written to disk. */
export const presetText = (p: PresetFile): string => `${JSON.stringify(p, null, 1)}\n`;
