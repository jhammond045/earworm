import { describe, expect, it } from 'vitest';
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { P, neutralSet, toNorm } from '../gen/params';
import { encodeLayer } from './layer';
import * as pr from './protocol';

describe('encodeLayer', () => {
  const f = (b: ArrayBuffer) => new Float32Array(b);

  it('has the blob size and magic', () => {
    const b = encodeLayer({ kind: 'melody' });
    expect(b.byteLength).toBe(pr.LAYER_WORDS * 4);
    expect(new Uint32Array(b)[0]).toBe(pr.LAYER_MAGIC);
  });

  it('defaults to the neutral set and engine default fields', () => {
    const w = f(encodeLayer({ kind: 'texture' }));
    expect(w[pr.LAYER_OFF_FIELDS + pr.LAYER_KIND]).toBe(pr.KIND_TEXTURE);
    expect(w[pr.LAYER_OFF_FIELDS + pr.LAYER_FOLLOW]).toBe(1);
    expect(w[pr.LAYER_OFF_FIELDS + pr.LAYER_INFLUENCE_MASK]).toBe(0x1f);
    expect([...w.subarray(pr.LAYER_OFF_BASE, pr.LAYER_OFF_BASE + 96)]).toEqual([...neutralSet()]);
    expect([...w.subarray(pr.LAYER_OFF_MACROS, pr.LAYER_OFF_MACROS + 8)]).toEqual(Array(8).fill(0.5));
  });

  it('converts real units with toNorm and leaves other values neutral', () => {
    const w = f(encodeLayer({
      kind: 'beat', level: -6, pan: 0.25, mute: true, follow: 0.5, kitCents: [3000, 4000, 5000, 6000],
      influence: { amount: 0.4, groups: ['tone', 'space'] },
      base: { 'verb.mix': 0.3 },
      kit: [undefined, { 'env.decay': 0.5, 'tone_bands.2': 6 }],
    }));
    const fld = (k: number) => w[pr.LAYER_OFF_FIELDS + k];
    expect([fld(pr.LAYER_LEVEL), fld(pr.LAYER_PAN), fld(pr.LAYER_MUTE), fld(pr.LAYER_KIND), fld(pr.LAYER_FOLLOW)])
      .toEqual([-6, 0.25, 1, pr.KIND_BEAT, 0.5]);
    expect(fld(pr.LAYER_INFLUENCE_AMOUNT)).toBeCloseTo(0.4, 6);
    expect(fld(pr.LAYER_INFLUENCE_MASK)).toBe((1 << pr.GROUP_TONE) | (1 << pr.GROUP_SPACE));
    expect([12, 13, 14, 15].map((k) => fld(k))).toEqual([3000, 4000, 5000, 6000]);
    expect(w[pr.LAYER_OFF_BASE + P['verb.mix']]).toBeCloseTo(toNorm(P['verb.mix'], 0.3), 6);
    const k1 = pr.LAYER_OFF_KIT + 96;
    expect(w[k1 + P['env.decay']]).toBeCloseTo(toNorm(P['env.decay'], 0.5), 6);
    expect(w[k1 + 64 + 2]).toBeCloseTo(0.6, 6); // +6 dB of -30..30
    expect(w[pr.LAYER_OFF_KIT + P['env.decay']]).toBeCloseTo(toNorm(P['env.decay'], 0.3), 6); // slot 0 neutral
  });

  it('rejects unknown ids', () => {
    expect(() => encodeLayer({ kind: 'melody', base: { 'nope.x': 1 } })).toThrow(/unknown/);
  });

  // The real engine accepts what we encode (needs a built klont.wasm).
  const wasm = resolve(import.meta.dirname, '../../public/wasm/klont.wasm');
  it.skipIf(!existsSync(wasm))('loads into the engine with LOAD_OK', () => {
    const { ex, e } = pr.instantiateEngine(readFileSync(wasm), 48000, 128);
    const t = pr.readBufferTable(ex.memory.buffer, ex.ew_buffers(e));
    const b = new Uint8Array(encodeLayer({ kind: 'melody', base: { 'env.decay': 1 } }));
    new Uint8Array(ex.memory.buffer, t.staging, b.length).set(b);
    expect(ex.ew_load(e, 2, pr.LOAD_LAYER, t.staging, b.length)).toBe(pr.LOAD_OK);
    new Uint8Array(ex.memory.buffer, t.staging, 4).fill(0);
    expect(ex.ew_load(e, 2, pr.LOAD_LAYER, t.staging, b.length)).toBe(pr.LOAD_BAD_MAGIC);
    ex.ew_free(e);
  });
});
