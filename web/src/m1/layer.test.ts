import { describe, expect, it } from 'vitest';
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { neutralSet, toNorm, P } from '../gen/params';
import { classicPatch } from '../synth/classic';
import { layoutOf } from '../synth/patch';
import { encodeLayer } from './layer';
import * as pr from './protocol';

describe('encodeLayer', () => {
  const f = (b: ArrayBuffer) => new Float32Array(b);
  const melody = layoutOf(classicPatch('melody'));
  const beat = layoutOf(classicPatch('beat'));
  const val = (w: Float32Array, i: number) => w[pr.LAYER_HEADER_WORDS + i];

  it('has the blob size and magic, and a patch blob', () => {
    const b = encodeLayer({ kind: 'melody' });
    expect(b.layer.byteLength).toBe(pr.layerWords(melody.keys.length) * 4);
    expect(new Uint32Array(b.layer)[0]).toBe(pr.LAYER_MAGIC);
    expect(new Uint32Array(b.patch)[0]).toBe(0x33505745);
  });

  it('defaults to the neutral set and engine default fields', () => {
    const w = f(encodeLayer({ kind: 'texture' }).layer);
    expect(w[pr.LAYER_OFF_FIELDS + pr.LAYER_KIND]).toBe(pr.KIND_TEXTURE);
    expect(w[pr.LAYER_OFF_FIELDS + pr.LAYER_FOLLOW]).toBe(1);
    expect(w[pr.LAYER_OFF_FIELDS + pr.LAYER_INFLUENCE_MASK]).toBe(0x1f);
    const n = neutralSet();
    expect(val(w, melody.index.get('partials1.tilt')!)).toBe(n[P['src.tilt']]);
    expect(val(w, melody.index.get('filter1.cutoff')!)).toBe(n[P['filt.cutoff']]);
    const m = melody.keys.length;
    expect([...w.subarray(pr.LAYER_HEADER_WORDS + m, pr.LAYER_HEADER_WORDS + m + 8)]).toEqual(Array(8).fill(0.5));
  });

  it('converts real units with toNorm and leaves other values neutral', () => {
    const w = f(encodeLayer({
      kind: 'beat', level: -6, pan: 0.25, mute: true, follow: 0.5, kitCents: [3000, 4000, 5000, 6000],
      influence: { amount: 0.4, groups: ['tone', 'space'] },
      base: { 'verb.mix': 0.3 },
      kit: [undefined, { 'env.decay': 0.5, 'tone_bands.2': 6 }],
    }).layer);
    const fld = (k: number) => w[pr.LAYER_OFF_FIELDS + k];
    expect([fld(pr.LAYER_LEVEL), fld(pr.LAYER_PAN), fld(pr.LAYER_MUTE), fld(pr.LAYER_KIND), fld(pr.LAYER_FOLLOW)])
      .toEqual([-6, 0.25, 1, pr.KIND_BEAT, 0.5]);
    expect(fld(pr.LAYER_INFLUENCE_AMOUNT)).toBeCloseTo(0.4, 6);
    expect(fld(pr.LAYER_INFLUENCE_MASK)).toBe((1 << pr.GROUP_TONE) | (1 << pr.GROUP_SPACE));
    expect([12, 13, 14, 15].map((k) => fld(k))).toEqual([3000, 4000, 5000, 6000]);
    expect(val(w, beat.index.get('reverb1.mix')!)).toBeCloseTo(toNorm(P['verb.mix'], 0.3), 6);
    expect(val(w, beat.index.get('k2.env1.decay')!)).toBeCloseTo(toNorm(P['env.decay'], 0.5), 6);
    expect(val(w, beat.index.get('k2.partials1.tone_bands.2')!)).toBeCloseTo(0.6, 6); // +6 dB of -30..30
    expect(val(w, beat.index.get('k1.env1.decay')!)).toBeCloseTo(toNorm(P['env.decay'], 0.3), 6); // slot 1 neutral
  });

  it('rejects unknown ids', () => {
    expect(() => encodeLayer({ kind: 'melody', base: { 'nope.x': 1 } })).toThrow(/unknown/);
  });

  // The real engine accepts what we encode (needs a built klont.wasm).
  const wasm = resolve(import.meta.dirname, '../../public/wasm/klont.wasm');
  it.skipIf(!existsSync(wasm))('loads into the engine with LOAD_OK', () => {
    const { ex, e } = pr.instantiateEngine(readFileSync(wasm), 48000, 128);
    const load = (kind: number, blob: ArrayBuffer) => {
      const t = pr.readBufferTable(ex.memory.buffer, ex.ew_buffers(e));
      const b = new Uint8Array(blob);
      new Uint8Array(ex.memory.buffer, t.staging, b.length).set(b);
      return ex.ew_load(e, 2, kind, t.staging, b.length);
    };
    const enc = encodeLayer({ kind: 'beat', base: { 'env.decay': 1 } }, 2);
    expect(load(pr.LOAD_PATCH, enc.patch)).toBe(pr.LOAD_OK);
    expect(load(pr.LOAD_LAYER, enc.layer)).toBe(pr.LOAD_OK);
    // A melody layer blob does not fit the beat patch: one value per value of the patch.
    expect(load(pr.LOAD_LAYER, encodeLayer({ kind: 'melody' }).layer)).toBe(pr.LOAD_MALFORMED);
    const bad = enc.layer.slice(0);
    new Uint32Array(bad)[0] = 0;
    expect(load(pr.LOAD_LAYER, bad)).toBe(pr.LOAD_BAD_MAGIC);
    ex.ew_free(e);
  });
});
