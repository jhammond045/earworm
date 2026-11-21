import { describe, expect, it } from 'vitest';
import { MAX_STATES, SAMPLE_MAGIC, SAMPLE_POOL_FLOATS, STATES_MAGIC, encodeLayerBlob, encodeSample, encodeStates } from './blobs';
import * as pr from './protocol';

const words = (b: ArrayBuffer) => new Uint32Array(b);
const floats = (b: ArrayBuffer) => new Float32Array(b);

describe('encodeSample', () => {
  it('writes magic, rate, reference pitch, count and samples', () => {
    const b = encodeSample(44100, 5700, Float32Array.from([0.5, -0.25, 1]));
    expect(b.byteLength).toBe(16 + 12);
    const u = words(b), f = floats(b);
    expect([u[0], f[1], f[2], u[3]]).toEqual([SAMPLE_MAGIC, 44100, 5700, 3]);
    expect(Array.from(f.slice(4))).toEqual([0.5, -0.25, 1]);
  });

  it('writes NaN for an unpitched source', () => {
    expect(words(encodeSample(48000, null, new Float32Array(1)))[2]).toBe(0x7fc00000);
    expect(words(encodeSample(48000, NaN, new Float32Array(1)))[2]).toBe(0x7fc00000);
  });

  it('does not alias the input', () => {
    const d = Float32Array.from([1, 2]);
    const b = encodeSample(48000, 0, d);
    d[0] = 9;
    expect(floats(b)[4]).toBe(1);
  });

  it('writes an empty sample as a clear', () => {
    const b = encodeSample(48000, null, new Float32Array(0));
    expect(b.byteLength).toBe(16);
    expect(words(b)[3]).toBe(0);
  });

  it('throws past the pool and for a bad rate', () => {
    expect(() => encodeSample(48000, 0, new Float32Array(SAMPLE_POOL_FLOATS + 1))).toThrow(RangeError);
    expect(() => encodeSample(0, 0, new Float32Array(4))).toThrow(RangeError);
  });
});

describe('encodeLayerBlob', () => {
  it('writes magic, fields, the value count, values, macros, target and group bits', () => {
    const fields = Float32Array.from({ length: pr.LAYER_FIELDS }, (_, i) => i);
    const b = encodeLayerBlob({ fields, values: [0.1, 0.2, 0.3], macros: new Array(8).fill(0.5), target: [0.7, 0.8, 0.9], groups: [1, 0, 16] });
    expect(b.byteLength).toBe(pr.layerWords(3) * 4);
    const u = words(b), f = floats(b);
    expect(u[0]).toBe(pr.LAYER_MAGIC);
    expect(f[pr.LAYER_OFF_FIELDS + 5]).toBe(5);
    expect(u[pr.LAYER_OFF_N]).toBe(3);
    const at = pr.LAYER_HEADER_WORDS;
    expect(f[at + 1]).toBeCloseTo(0.2, 6);
    expect(f[at + 3]).toBe(0.5);
    expect(f[at + 3 + 8 + 2]).toBeCloseTo(0.9, 6);
    expect([u[at + 3 + 8 + 3], u[at + 3 + 8 + 4], u[at + 3 + 8 + 5]]).toEqual([1, 0, 16]);
  });

  it('rejects mismatched lengths', () => {
    const fields = new Float32Array(pr.LAYER_FIELDS);
    expect(() => encodeLayerBlob({ fields, values: [0], macros: new Array(8).fill(0.5), target: [0, 0], groups: [0] })).toThrow(RangeError);
    expect(() => encodeLayerBlob({ fields, values: [0], macros: [0.5], target: [0], groups: [0] })).toThrow(RangeError);
  });
});

describe('encodeStates', () => {
  const N = 10;
  const state = (k: number) => ({
    values: Float32Array.from({ length: N }, (_, i) => (i + k) / 100),
    macros: Float32Array.from({ length: 8 }, (_, i) => i / 10),
    influenceAmount: 0.25 * k,
  });

  it('writes magic, count, value count and n + 9 words per state', () => {
    expect(pr.stateWords(N)).toBe(N + 9);
    const b = encodeStates(N, [state(0), state(2)]);
    expect(b.byteLength).toBe((3 + 2 * (N + 9)) * 4);
    const u = words(b), f = floats(b);
    expect([u[0], u[1], u[2]]).toEqual([STATES_MAGIC, 2, N]);
    expect(f[3 + 5]).toBeCloseTo(0.05, 6);
    expect(f[3 + (N + 9) + 5]).toBeCloseTo(0.07, 6);
    expect(f[3 + N + 3]).toBeCloseTo(0.3, 6);
    expect(f[3 + (N + 9) + N + 8]).toBe(0.5);
  });

  it('accepts plain arrays, an empty list and exactly MAX_STATES', () => {
    const plain = { values: new Array(N).fill(0.5), macros: new Array(8).fill(0.5), influenceAmount: 0 };
    expect(encodeStates(N, [plain]).byteLength).toBe((3 + N + 9) * 4);
    expect(words(encodeStates(N, []))[1]).toBe(0);
    expect(encodeStates(N, Array.from({ length: MAX_STATES }, () => plain)).byteLength).toBe((3 + 8 * (N + 9)) * 4);
  });

  it('rejects wrong lengths and too many states', () => {
    expect(() => encodeStates(N, [{ ...state(0), values: new Float32Array(N - 1) }])).toThrow(RangeError);
    expect(() => encodeStates(N, [{ ...state(0), macros: new Float32Array(7) }])).toThrow(RangeError);
    expect(() => encodeStates(N, Array.from({ length: MAX_STATES + 1 }, () => state(0)))).toThrow(RangeError);
  });
});
