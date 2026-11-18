import { describe, expect, it } from 'vitest';
import { GRAIN_MAGIC, GRAIN_MAX_SAMPLES, MAX_STATES, STATES_MAGIC, STATE_WORDS, encodeGrain, encodeStates } from './blobs';

const words = (b: ArrayBuffer) => new Uint32Array(b);
const floats = (b: ArrayBuffer) => new Float32Array(b);

describe('encodeGrain', () => {
  it('writes magic, rate, reference pitch, count and samples', () => {
    const b = encodeGrain(44100, 5700, Float32Array.from([0.5, -0.25, 1]));
    expect(b.byteLength).toBe(16 + 12);
    const u = words(b), f = floats(b);
    expect([u[0], f[1], f[2], u[3]]).toEqual([GRAIN_MAGIC, 44100, 5700, 3]);
    expect(Array.from(f.slice(4))).toEqual([0.5, -0.25, 1]);
  });

  it('writes NaN for an unpitched source', () => {
    expect(words(encodeGrain(48000, null, new Float32Array(1)))[2]).toBe(0x7fc00000);
    expect(words(encodeGrain(48000, NaN, new Float32Array(1)))[2]).toBe(0x7fc00000);
  });

  it('does not alias the input', () => {
    const d = Float32Array.from([1, 2]);
    const b = encodeGrain(48000, 0, d);
    d[0] = 9;
    expect(floats(b)[4]).toBe(1);
  });

  it('writes an empty source as a clear', () => {
    const b = encodeGrain(48000, null, new Float32Array(0));
    expect(b.byteLength).toBe(16);
    expect(words(b)[3]).toBe(0);
  });

  it('takes the maximum and throws past it', () => {
    expect(encodeGrain(48000, 0, new Float32Array(GRAIN_MAX_SAMPLES)).byteLength).toBe(16 + GRAIN_MAX_SAMPLES * 4);
    expect(() => encodeGrain(48000, 0, new Float32Array(GRAIN_MAX_SAMPLES + 1))).toThrow(RangeError);
    expect(() => encodeGrain(0, 0, new Float32Array(4))).toThrow(RangeError);
  });
});

describe('encodeStates', () => {
  const state = (k: number) => ({
    base: Float32Array.from({ length: 96 }, (_, i) => (i + k) / 100),
    macros: Float32Array.from({ length: 8 }, (_, i) => i / 10),
    influenceAmount: 0.25 * k,
  });

  it('writes magic, count and 108 words per state', () => {
    expect(STATE_WORDS).toBe(108);
    const b = encodeStates([state(0), state(2)]);
    expect(b.byteLength).toBe((2 + 2 * 108) * 4);
    const u = words(b), f = floats(b);
    expect([u[0], u[1]]).toEqual([STATES_MAGIC, 2]);
    expect(f[2 + 5]).toBeCloseTo(0.05, 6);
    expect(f[2 + 108 + 5]).toBeCloseTo(0.07, 6);
    expect(f[2 + 96 + 3]).toBeCloseTo(0.3, 6);
    expect(f[2 + 108 + 104]).toBe(0.5);
    // The three trailing words are zero.
    expect([u[2 + 105], u[2 + 106], u[2 + 107]]).toEqual([0, 0, 0]);
  });

  it('accepts plain arrays, an empty list and exactly MAX_STATES', () => {
    const plain = { base: new Array(96).fill(0.5), macros: new Array(8).fill(0.5), influenceAmount: 0 };
    expect(encodeStates([plain]).byteLength).toBe((2 + 108) * 4);
    expect(words(encodeStates([]))[1]).toBe(0);
    expect(encodeStates(Array.from({ length: MAX_STATES }, () => plain)).byteLength).toBe((2 + 8 * 108) * 4);
  });

  it('rejects wrong lengths and too many states', () => {
    expect(() => encodeStates([{ ...state(0), base: new Float32Array(95) }])).toThrow(RangeError);
    expect(() => encodeStates([{ ...state(0), macros: new Float32Array(7) }])).toThrow(RangeError);
    expect(() => encodeStates(Array.from({ length: MAX_STATES + 1 }, () => state(0)))).toThrow(RangeError);
  });
});
