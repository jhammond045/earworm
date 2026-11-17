import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { generate } from '../../../tools/paramgen/paramgen';
import { COUNT, DESCS, MACRO_NAMES, MACRO_W, N_MACROS, P, neutralSet, toNorm, toReal } from './params';

const root = resolve(import.meta.dirname, '../../..');
const read = (p: string) => readFileSync(resolve(root, p), 'utf8');
const toml = read('crates/params/params.toml');

describe('generated files', () => {
  const g = generate(toml);
  it('are up to date with params.toml (run pnpm gen:params)', () => {
    expect(read('crates/params/src/table.rs')).toBe(g.rust);
    expect(read('web/src/gen/params.ts')).toBe(g.ts);
    expect(read('web/src/gen/params.schema.json')).toBe(g.schema);
    expect(read('web/src/gen/params-dictionary.md')).toBe(g.dictionary);
  });
  it('schema is strict and complete', () => {
    const s = JSON.parse(g.schema);
    expect(s.additionalProperties).toBe(false);
    expect(s.required).toEqual(Object.keys(s.properties));
    expect(s.required.length).toBe(66);
    expect(s.properties.tone_bands.items.type).toBe('number');
  });
});

describe('generator validation', () => {
  it('rejects bad input with a line number', () => {
    expect(() => generate(toml.replace('scale     = "lin"', 'scale     = "cubic"'))).toThrow(/params\.toml:\d+/);
    expect(() => generate(toml + '\nnonsense\n')).toThrow(/params\.toml:\d+/);
  });
});

describe('table', () => {
  it('has 96 parameters and consistent shapes', () => {
    expect(COUNT).toBe(96);
    expect(DESCS.length).toBe(COUNT);
    expect(MACRO_NAMES.length).toBe(N_MACROS);
    expect(MACRO_W.every((r) => r.length === COUNT)).toBe(true);
    expect(new Set(DESCS.map((d) => d.id)).size).toBe(COUNT);
    expect(DESCS[P['src.tilt']].id).toBe('src.tilt');
  });
  it('round trips toReal and toNorm', () => {
    for (let i = 0; i < COUNT; i++) {
      for (const n of [0, 0.01, 0.25, 0.5, 0.77, 1]) {
        expect(toNorm(i, toReal(i, n))).toBeCloseTo(n, 4);
      }
    }
  });
  it('neutralSet matches the table neutrals', () => {
    const s = neutralSet();
    for (let i = 0; i < COUNT; i++) {
      expect(s[i]).toBeGreaterThanOrEqual(0);
      expect(s[i]).toBeLessThanOrEqual(1);
      const d = DESCS[i];
      expect(toReal(i, s[i])).toBeCloseTo(d.neutral, 3);
    }
  });
  it('keeps odd inputs finite and in range', () => {
    for (let i = 0; i < COUNT; i++) {
      for (const n of [NaN, Infinity, -Infinity, -3, 3]) {
        const r = toReal(i, n);
        expect(r).toBeGreaterThanOrEqual(DESCS[i].min);
        expect(r).toBeLessThanOrEqual(DESCS[i].max);
        const b = toNorm(i, n);
        expect(b).toBeGreaterThanOrEqual(0);
        expect(b).toBeLessThanOrEqual(1);
      }
    }
  });
});
