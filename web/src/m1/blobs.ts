// Encoders for the sample, layer and states blobs (crates/klont/src/protocol.rs "Sample blob",
// "Layer blob v2", "States blob v2"). All words are 32-bit little endian.

import {
  LAYER_FIELDS, LAYER_HEADER_WORDS, LAYER_MAGIC, LAYER_OFF_FIELDS, LAYER_OFF_N, MAX_STATES, MAX_VALUES, N_MACROS,
  SAMPLE_HEADER_WORDS, SAMPLE_MAGIC, SAMPLE_POOL_FLOATS, STATES_MAGIC, layerWords, stateWords,
} from './protocol';

export { MAX_STATES, SAMPLE_MAGIC, SAMPLE_POOL_FLOATS, STATES_MAGIC };

const QUIET_NAN = 0x7fc00000;

/** LOAD_SAMPLE blob: a mono sample at `sampleRate`. `refCents` is its pitch in cents above C0, or
 *  null when it has none (grains then play it at its own speed for a note at C4). An empty `data`
 *  clears the sample. Throws a RangeError past the pool's size. */
export function encodeSample(sampleRate: number, refCents: number | null, data: Float32Array): ArrayBuffer {
  if (data.length > SAMPLE_POOL_FLOATS) {
    throw new RangeError(`sample: ${data.length} samples is more than the ${SAMPLE_POOL_FLOATS} the engine holds`);
  }
  if (data.length > 0 && !(Number.isFinite(sampleRate) && sampleRate > 0)) {
    throw new RangeError(`sample: sample rate must be positive, got ${sampleRate}`);
  }
  const buf = new ArrayBuffer((SAMPLE_HEADER_WORDS + data.length) * 4);
  const v = new DataView(buf);
  v.setUint32(0, SAMPLE_MAGIC, true);
  v.setFloat32(4, sampleRate, true);
  if (refCents === null || !Number.isFinite(refCents)) v.setUint32(8, QUIET_NAN, true);
  else v.setFloat32(8, refCents, true);
  v.setUint32(12, data.length, true);
  new Float32Array(buf, SAMPLE_HEADER_WORDS * 4, data.length).set(data);
  return buf;
}

export interface LayerInput {
  /** LAYER_* fields (level, pan, mute, kind, depths, follow, Influence amount and mask, kit cents). */
  fields: ArrayLike<number>;
  /** One normalized value per value of the layer's patch. */
  values: ArrayLike<number>;
  /** 8 macro positions, 0.5 neutral. */
  macros: ArrayLike<number>;
  /** Influence target per value. */
  target: ArrayLike<number>;
  /** Per value: 1 << its aspect group when Influence moves it, else 0. */
  groups: ArrayLike<number>;
}

/** LOAD_LAYER blob v2. Throws a RangeError for mismatched lengths. */
export function encodeLayerBlob(l: LayerInput): ArrayBuffer {
  const n = l.values.length;
  if (n > MAX_VALUES) throw new RangeError(`layer: ${n} values, the engine holds ${MAX_VALUES}`);
  if (l.target.length !== n || l.groups.length !== n) throw new RangeError('layer: values, target and groups differ in length');
  if (l.fields.length !== LAYER_FIELDS || l.macros.length !== N_MACROS) throw new RangeError('layer: wrong field or macro count');
  const buf = new ArrayBuffer(layerWords(n) * 4);
  const u = new Uint32Array(buf);
  const f = new Float32Array(buf);
  u[0] = LAYER_MAGIC;
  f.set(l.fields, LAYER_OFF_FIELDS);
  u[LAYER_OFF_N] = n;
  let at = LAYER_HEADER_WORDS;
  f.set(l.values, at);
  f.set(l.macros, (at += n));
  f.set(l.target, (at += N_MACROS));
  u.set(l.groups, at + n);
  return buf;
}

export interface StateInput {
  /** One normalized value per value of the layer's patch. */
  values: ArrayLike<number>;
  /** 8 macro positions 0..1 (0.5 neutral). */
  macros: ArrayLike<number>;
  /** Influence amount 0..1. */
  influenceAmount: number;
}

/** LOAD_STATES blob v2 for a patch of `n` values: 0 to MAX_STATES states (none clears the layer's
 *  states). Throws a RangeError for the wrong lengths or too many states. */
export function encodeStates(n: number, states: StateInput[]): ArrayBuffer {
  if (states.length > MAX_STATES) throw new RangeError(`states: ${states.length} is more than ${MAX_STATES}`);
  const buf = new ArrayBuffer((3 + states.length * stateWords(n)) * 4);
  const v = new DataView(buf);
  v.setUint32(0, STATES_MAGIC, true);
  v.setUint32(4, states.length, true);
  v.setUint32(8, n, true);
  const f = new Float32Array(buf, 12);
  states.forEach((s, k) => {
    if (s.values.length !== n) throw new RangeError(`states: state ${k} has ${s.values.length} values, wanted ${n}`);
    if (s.macros.length !== N_MACROS) throw new RangeError(`states: state ${k} has ${s.macros.length} macros, wanted ${N_MACROS}`);
    const at = k * stateWords(n);
    f.set(s.values, at);
    f.set(s.macros, at + n);
    f[at + n + N_MACROS] = s.influenceAmount;
  });
  return buf;
}
