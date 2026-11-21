// Encoder for classic layers (crates/klont/src/protocol.rs "Layer blob v2", the classic patch in
// synth/classic.ts). Sounds are given in real units (Hz, seconds, dB...) of the old parameters and
// converted here.

import { DESCS, neutralSet, toNorm } from '../gen/params';
import { GROUP_GRIT, GROUP_MOTION, GROUP_SHAPE, GROUP_SPACE, GROUP_TONE, KIND_BEAT, KIND_MELODY, KIND_TEXTURE,
  KIT_SLOTS, LAYER_DEPTH_LOUD, LAYER_FIELDS, LAYER_FOLLOW, LAYER_INFLUENCE_AMOUNT, LAYER_INFLUENCE_MASK,
  LAYER_KIND, LAYER_KIT_CENTS, LAYER_LEVEL, LAYER_MUTE, LAYER_PAN, N_MACROS } from './protocol';
import { encodeLayerBlob } from './blobs';
import { classicPatch, classicVectors, valuesFromVocab } from '../synth/classic';
import { patchBlob } from '../synth/patch';

/** Parameter id ("env.decay", "tone_bands.3") -> value in real units. */
export type Sound = Record<string, number>;

export type LayerKind = 'melody' | 'beat' | 'texture';
export type AspectGroup = 'tone' | 'shape' | 'motion' | 'grit' | 'space';

export interface LayerDef {
  kind: LayerKind;
  /** dB, -60..12. Default 0. */
  level?: number;
  /** -1..1. Default 0. */
  pan?: number;
  mute?: boolean;
  /** Gesture depths 0..1. Defaults match the engine: loud 1, open .5, hard .5, breath .5, mouth 1. */
  depths?: { loud?: number; open?: number; hard?: number; breath?: number; mouth?: number };
  /** Hit-shape follow 0..1 (beat layers). Default 1. */
  follow?: number;
  /** Influence target sound and how much of it (0..1) in the groups listed (default all). */
  influence?: { amount: number; groups?: AspectGroup[]; sound?: Sound };
  /** Default pitch of the four kit slots, cents above C0. */
  kitCents?: number[];
  /** Base sound over neutral. For beat layers it carries the space, crush and tape settings only. */
  base?: Sound;
  /** Up to four kit sounds over neutral (beat layers). Missing slots stay neutral. */
  kit?: (Sound | undefined)[];
  /** Macro positions 0..1, 0.5 neutral. */
  macros?: number[];
}

const KIND: Record<LayerKind, number> = { melody: KIND_MELODY, beat: KIND_BEAT, texture: KIND_TEXTURE };
const GROUP_BIT: Record<AspectGroup, number> = {
  tone: 1 << GROUP_TONE, shape: 1 << GROUP_SHAPE, motion: 1 << GROUP_MOTION, grit: 1 << GROUP_GRIT, space: 1 << GROUP_SPACE,
};

const INDEX = new Map(DESCS.map((d, i) => [d.id, i]));

/** Neutral set with the given real-unit values applied. Unknown ids throw. */
export function soundToSet(sound: Sound | undefined): Float32Array {
  const s = neutralSet();
  for (const [id, real] of Object.entries(sound ?? {})) {
    const i = INDEX.get(id);
    if (i === undefined) throw new RangeError(`layer: unknown parameter "${id}"`);
    if (!Number.isFinite(real)) throw new RangeError(`layer: "${id}" must be finite`);
    s[i] = toNorm(i, real);
  }
  return s;
}

/** A classic layer from a definition: the LOAD_PATCH and LOAD_LAYER blobs. Layer `slot`'s grains
 *  read sample `slot`. */
export function encodeLayer(def: LayerDef, slot = 0): { patch: ArrayBuffer; layer: ArrayBuffer } {
  const fields = new Float32Array(LAYER_FIELDS);
  const d = def.depths ?? {};
  fields[LAYER_LEVEL] = def.level ?? 0;
  fields[LAYER_PAN] = def.pan ?? 0;
  fields[LAYER_MUTE] = def.mute ? 1 : 0;
  fields[LAYER_KIND] = KIND[def.kind];
  fields.set([d.loud ?? 1, d.open ?? 0.5, d.hard ?? 0.5, d.breath ?? 0.5, d.mouth ?? 1], LAYER_DEPTH_LOUD);
  fields[LAYER_FOLLOW] = def.follow ?? 1;
  fields[LAYER_INFLUENCE_AMOUNT] = def.influence?.amount ?? 0;
  const groups = def.influence?.groups;
  fields[LAYER_INFLUENCE_MASK] = groups ? groups.reduce((m, g) => m | GROUP_BIT[g], 0) : 0x1f;
  const cents = def.kitCents ?? [2400, 4800, 6000, 7200];
  for (let k = 0; k < KIT_SLOTS; k++) fields[LAYER_KIT_CENTS + k] = cents[k] ?? [2400, 4800, 6000, 7200][k];
  if ((def.kit?.length ?? 0) > KIT_SLOTS) throw new RangeError('layer: more than 4 kit sounds');
  const kit = Array.from({ length: KIT_SLOTS }, (_, k) => soundToSet(def.kit?.[k]));
  const values = valuesFromVocab(def.kind, soundToSet(def.base), kit);
  const macros = Array.from({ length: N_MACROS }, (_, k) => def.macros?.[k] ?? 0.5);
  const v = classicVectors(def.kind, values, soundToSet(def.influence?.sound));
  return {
    patch: patchBlob(classicPatch(def.kind), slot),
    layer: encodeLayerBlob({ fields, values: v.values, macros, target: v.target, groups: v.groups }),
  };
}
