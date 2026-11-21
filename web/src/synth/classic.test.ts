import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { DESCS } from '../gen/params';
import { classicPatch, vocabKeys } from './classic';
import { layoutOf, patchBlob } from './patch';

const fixture = (name: string) => new Uint8Array(readFileSync(resolve(import.meta.dirname, `fixtures/classic-${name}.bin`)));

describe('classic patch', () => {
  for (const [kind, name] of [['melody', 'melody'], ['beat', 'beat']] as const) {
    it(`compiles to the blob the engine's own classic patch is (${name})`, () => {
      const ts = new Uint8Array(patchBlob(classicPatch(kind), 3));
      const rs = fixture(name);
      expect(ts.length).toBe(rs.length);
      const a = new DataView(ts.buffer), b = new DataView(rs.buffer, rs.byteOffset);
      const n = a.getUint32(4, true);
      // Starting values are computed in f64 here and in f32 in Rust: allow the last bit.
      const init0 = 6 + n, init1 = 6 + 2 * n;
      for (let w = 0; w < ts.length / 4; w++) {
        const x = a.getUint32(w * 4, true), y = b.getUint32(w * 4, true);
        if (w >= init0 && w < init1) expect(Math.abs(a.getFloat32(w * 4, true) - b.getFloat32(w * 4, true)), `init ${w - init0}`).toBeLessThan(2e-7);
        else expect(x, `word ${w}`).toBe(y);
      }
    });
  }

  it('binds every old parameter but smear on a melody layer', () => {
    const lay = layoutOf(classicPatch('melody'));
    for (const d of DESCS) {
      const keys = vocabKeys('melody', 0, d.id);
      if (d.id.startsWith('smear.')) expect(keys).toEqual([]);
      else {
        expect(keys.length, d.id).toBeGreaterThan(0);
        for (const k of keys) expect(lay.index.has(k), k).toBe(true);
      }
    }
  });

  it('splits a beat layer into the channel and four kit slots', () => {
    const lay = layoutOf(classicPatch('beat'));
    expect(vocabKeys('beat', 0, 'verb.mix')).toEqual(['reverb1.mix']);
    expect(vocabKeys('beat', 0, 'src.tilt')).toEqual([]);
    expect(vocabKeys('beat', 3, 'src.tilt')).toEqual(['k3.partials1.tilt']);
    for (let s = 1; s <= 4; s++) for (const k of vocabKeys('beat', s, 'env.release')) expect(lay.index.has(k), k).toBe(true);
  });
});
