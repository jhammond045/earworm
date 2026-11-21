// Factory presets (docs/synth.md "Serialization", "The classic patch and today's presets"): the ten
// sounds of m2/sounds.ts as preset files of the classic patch, one JSON file each in this folder.
// presets.test.ts writes them (WRITE_PRESETS=1, or `pnpm gen:presets`) and checks they are current.

import { soundToSet } from '../m1/layer';
import { BEAT_KITS, MELODY_SOUNDS } from '../m2/sounds';
import { classicPatch, valuesFromVocab } from '../synth/classic';
import type { Patch } from '../synth/patch';
import type { LayerKind } from '../project/types';

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
