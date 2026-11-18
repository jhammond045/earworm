// Encoders for the grain source and states blobs (crates/klont/src/protocol.rs
// "Grain source blob" and "States blob"). All words are 32-bit little endian.

import { GRAIN_HEADER_WORDS, GRAIN_MAGIC, GRAIN_MAX_SAMPLES, MAX_STATES, N_MACROS, STATES_MAGIC, STATE_WORDS } from './protocol';

export { GRAIN_MAGIC, GRAIN_MAX_SAMPLES, MAX_STATES, STATES_MAGIC, STATE_WORDS };

const QUIET_NAN = 0x7fc00000;
const PARAMS = 96;

/** LOAD_GRAIN blob: a mono source at `sampleRate`. `refCents` is the source's
 * pitch in cents above C0, or null when it has none (the engine then plays it
 * at its own speed for a note at C4). An empty `data` clears the layer's source.
 * Throws a RangeError past GRAIN_MAX_SAMPLES. */
export function encodeGrain(sampleRate: number, refCents: number | null, data: Float32Array): ArrayBuffer {
  if (data.length > GRAIN_MAX_SAMPLES) {
    throw new RangeError(`grain: ${data.length} samples is more than the ${GRAIN_MAX_SAMPLES} the engine holds`);
  }
  if (data.length > 0 && !(Number.isFinite(sampleRate) && sampleRate > 0)) {
    throw new RangeError(`grain: sample rate must be positive, got ${sampleRate}`);
  }
  const buf = new ArrayBuffer((GRAIN_HEADER_WORDS + data.length) * 4);
  const v = new DataView(buf);
  v.setUint32(0, GRAIN_MAGIC, true);
  v.setFloat32(4, sampleRate, true);
  if (refCents === null || !Number.isFinite(refCents)) v.setUint32(8, QUIET_NAN, true);
  else v.setFloat32(8, refCents, true);
  v.setUint32(12, data.length, true);
  new Float32Array(buf, GRAIN_HEADER_WORDS * 4, data.length).set(data);
  return buf;
}

export interface StateInput {
  /** 96 normalized values in ParamSet order. */
  base: ArrayLike<number>;
  /** 8 macro positions 0..1 (0.5 neutral). */
  macros: ArrayLike<number>;
  /** Influence amount 0..1. */
  influenceAmount: number;
}

/** LOAD_STATES blob: 1 to MAX_STATES states (an empty list clears the layer's
 * states). Throws a RangeError for the wrong lengths or too many states. */
export function encodeStates(states: StateInput[]): ArrayBuffer {
  if (states.length > MAX_STATES) throw new RangeError(`states: ${states.length} is more than ${MAX_STATES}`);
  const buf = new ArrayBuffer((2 + states.length * STATE_WORDS) * 4);
  const v = new DataView(buf);
  v.setUint32(0, STATES_MAGIC, true);
  v.setUint32(4, states.length, true);
  states.forEach((s, k) => {
    if (s.base.length !== PARAMS) throw new RangeError(`states: state ${k} base has ${s.base.length} values, wanted ${PARAMS}`);
    if (s.macros.length !== N_MACROS) throw new RangeError(`states: state ${k} has ${s.macros.length} macros, wanted ${N_MACROS}`);
    let at = 8 + k * STATE_WORDS * 4;
    const put = (x: number) => {
      v.setFloat32(at, x, true);
      at += 4;
    };
    for (let i = 0; i < PARAMS; i++) put(s.base[i]);
    for (let i = 0; i < N_MACROS; i++) put(s.macros[i]);
    put(s.influenceAmount);
  });
  return buf;
}
