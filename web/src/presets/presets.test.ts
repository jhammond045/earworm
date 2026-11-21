import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { newProject } from '../project/commands';
import { Store } from '../project/store';
import { factoryPresets, presetFileName, type PresetFile } from './presets';

const dir = import.meta.dirname;
const json = (p: PresetFile) => `${JSON.stringify(p, null, 1)}\n`;

describe('factory presets', () => {
  const presets = factoryPresets();

  it('are the ten sounds of m2/sounds.ts, written as files (WRITE_PRESETS=1 rewrites them)', () => {
    expect(presets.map((p) => p.name)).toHaveLength(10);
    for (const p of presets) {
      const file = resolve(dir, presetFileName(p.name));
      if (process.env.WRITE_PRESETS === '1') writeFileSync(file, json(p));
      expect(existsSync(file), `${file} missing: run pnpm gen:presets`).toBe(true);
      expect(readFileSync(file, 'utf8'), p.name).toBe(json(p));
    }
  });

  it('hold the values a new layer gets from the same sound', () => {
    const store = new Store(newProject('presets'));
    store.dispatch('addLayer', { kind: 'melody', sound: 2 });
    store.dispatch('addLayer', { kind: 'beat', sound: 1 });
    const [, , glass, perc] = store.doc.layers;
    expect(presets[2].values).toEqual(glass.states[0].values);
    expect(presets[9].values).toEqual(perc.states[0].values);
    expect(presets[9].kitCents).toEqual(perc.kit.map((k) => k.cents));
    expect(presets[2].patch).toEqual(glass.patch);
  });
});
