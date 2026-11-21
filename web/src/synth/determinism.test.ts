// Stage S2 check 2 (docs/synth.md "S2: the modulation matrix"): two offline renders with every
// source type routed are bit-identical. Two of the golden fixture projects (a sung melody with its
// take, and a beat take whose hits carry every hit-shape field) get routes from every source: the
// envelopes (one restarting at each sung onset), the LFOs on every wave, the note fields, the sung
// features of the voice and of the layer (intensity and the vowel included), the macros, the mod
// wheel, bend and pressure, and both layer LFOs. Each is rendered twice by fresh engines, with the
// same macro and performance moves at the same samples.
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { ALL_LAYERS, LOAD_OK, LOAD_TRACK, MSG_PLAY, MSG_SELECT_LAYER, MSG_SET_MACRO, MSG_SET_PERF, M_ROUTES, instantiateEngine, type Msg } from '../m1/protocol';
import { Core } from '../m1/render-core';
import { commands } from '../project/commands';
import { Compiler } from '../project/compile';
import { migrate } from '../project/migrate';
import { applyOps } from '../project/patch';
import type { Project, Take } from '../project/types';
import { FEATURES, layoutOf } from './patch';
import { toNorm } from './values';

const wasm = readFileSync(resolve(import.meta.dirname, '../../public/wasm/klont.wasm'));
const fixtures = resolve(import.meta.dirname, '../golden/fixtures');
const SR = 48000;
const CHUNK = 4096;
const SECONDS = 8;

function load(name: string): { doc: Project; curves: Map<string, Float32Array>; blobs: Map<string, Float32Array> } {
  const dir = resolve(fixtures, name);
  const f32 = (p: string) => { const b = readFileSync(p); return new Float32Array(b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength)); };
  const files = (sub: string) => {
    const d = resolve(dir, sub);
    return new Map((existsSync(d) ? readdirSync(d) : []).map((f) => [f.replace(/\.f32$/, ''), f32(resolve(d, f))]));
  };
  return { doc: migrate(JSON.parse(readFileSync(resolve(dir, 'doc.json'), 'utf8'))), curves: files('curves'), blobs: files('blobs') };
}

/** Every source type routed into layer `li` of `doc` (beat layers through kit slot 1). */
function routeEverything(doc: Project, li: number): Project {
  const run = (name: keyof typeof commands, a: unknown) => { doc = applyOps(doc, (commands[name] as (d: Project, x: unknown) => never)(doc, a)).doc; };
  const l = doc.layers[li];
  const pre = l.kind === 'beat' ? 'k1.' : '';
  const add = (source: string, target: string, more: Record<string, unknown> = {}, amount = 0.3) =>
    run('addRoute', { layer: l.id, slot: 0, route: { source, target, ...more }, amount });
  // Voice sources. The classic patch already routes env1, env3, lfo1..lfo3, velocity, hardness,
  // every hit field, volume, openness, breath and the vowel; these add the rest.
  add('env2', 'filter1.res');
  add('note.pitch', 'partials1.stretch', {}, 0.05);
  for (const f of ['pitch', 'hardness', 'onset', 'voicing', 'intensity']) add(`voice.${f}`, 'partials1.oddeven', { riseMs: 5, fallMs: 120 }, 0.1);
  // Layer sources.
  add('macro1', `${pre}filter1.cutoff`);
  add('wheel', `${pre}partials1.damp`, { via: 'pressure' });
  add('bend', `${pre}partials1.jitter`);
  add('pressure', 'delay1.mix');
  add('layer.lfo1', 'reverb1.mix');
  add('layer.lfo2', `${pre}clang1.amount`, { via: 'wheel' });
  for (const f of FEATURES) {
    if (f === 'vowel') add('layer.vowel', `bands:${pre}shaped_noise1.bands`, { mode: 'pull' }, 0.5);
    else add(`layer.${f}`, 'tape1.amount', {}, 0.05);
  }
  // A voice LFO via a layer source, an envelope restarting at each onset, and every LFO wave.
  add('lfo1', 'pitch', { via: 'wheel' }, 0.01);
  const vp = doc.layers[li].patch.voices[0];
  const mods = vp.mods.map((m) => (m.id === 'env2' ? { ...m, trigger: 'onset' as const } : m));
  const patch = { ...doc.layers[li].patch, voices: doc.layers[li].patch.voices.map((v, k) => (k === 0 ? { ...v, mods } : v)) };
  doc = applyOps(doc, [{ op: 'set', path: ['layers', li, 'patch'], value: patch }]).doc;
  const lay = layoutOf(patch);
  const st = doc.layers[li].states[0];
  const waves = { [`${pre}lfo1.wave`]: 0.2, [`${pre}lfo2.wave`]: 0.4, [`${pre}lfo3.wave`]: 0.6, 'layer.lfo1.wave': 0.8, 'layer.lfo2.wave': 1, 'layer.lfo1.rate': 3, 'layer.lfo2.rate': 0.7 };
  const values: Record<string, number> = {};
  for (const [k, v] of Object.entries(waves)) values[k] = toNorm(lay.descs[lay.index.get(k)!], v);
  values[`${pre}lfo1.rate`] = toNorm(lay.descs[lay.index.get(`${pre}lfo1.rate`)!], 5);
  run('setValues', { layer: l.id, state: st.id, values });
  return doc;
}

/** Moves during the render: macro 1 of layer `li`, the wheel, bend and pressure, at fixed chunks. */
const moves = (li: number): Record<number, Msg[]> => ({
  10: [[MSG_SET_MACRO, li, 0, 0, 0.9], [MSG_SET_PERF, ALL_LAYERS, 0, 0, 0.7]],
  25: [[MSG_SET_PERF, ALL_LAYERS, 1, 0, -0.4], [MSG_SET_PERF, li, 2, 0, 0.6]],
  40: [[MSG_SET_PERF, ALL_LAYERS, 1, 0, 0.8], [MSG_SET_MACRO, li, 0, 0, 0.2]],
});

function render(doc: Project, li: number, curves: Map<string, Float32Array>, blobs: Map<string, Float32Array>): { l: Float32Array; r: Float32Array; routed: number } {
  const uploads = new Compiler().compile(doc, (t: Take) => curves.get(t.id) ?? null, {
    grainFor: (l) => {
      const g = l.influence?.grain;
      const d = l.influence ? blobs.get(l.influence.source) : undefined;
      return g && d ? { data: d.subarray(g.start, g.end), rate: l.influence!.sourceRate, refCents: g.refCents } : null;
    },
  });
  const { ex } = instantiateEngine(wasm, SR, CHUNK);
  const core = new Core(ex, SR, CHUNK);
  for (const u of uploads) {
    let blob = u.blob;
    if (u.kind === LOAD_TRACK) { blob = blob.slice(0); new Uint32Array(blob, 0, 6)[5] = 0; }
    expect(core.load(u.kind, u.slot, blob), u.what).toBe(LOAD_OK);
  }
  const n = Math.round((SECONDS * SR) / CHUNK);
  const l = new Float32Array(n * CHUNK), r = new Float32Array(n * CHUNK);
  const seen = new Set<number>();
  for (let c = 0; c < n; c++) {
    core.send(c === 0 ? [[MSG_SELECT_LAYER, li, 0, 0, 0], [MSG_PLAY, 0, 0, 0, 0]] : moves(li)[c] ?? []);
    core.render(CHUNK);
    const o = core.out(CHUNK);
    l.set(o.l, c * CHUNK);
    r.set(o.r, c * CHUNK);
    const m = core.meter();
    for (let i = 0; i < 256; i++) if (Number.isFinite(m[M_ROUTES + i])) seen.add(i);
  }
  core.free();
  return { l, r, routed: seen.size };
}

const bits = (a: Float32Array) => new Uint32Array(a.buffer, a.byteOffset, a.length);
const same = (a: Float32Array, b: Float32Array) => a.length === b.length && bits(a).every((x, i) => x === bits(b)[i]);

describe('every source type routed: two offline renders are bit-identical', () => {
  for (const [name, layer] of [['sung-melody', 0], ['beat-take', 0]] as const) {
    it(name, () => {
      const { doc, curves, blobs } = load(name);
      const li = name === 'beat-take' ? doc.layers.findIndex((x) => x.kind === 'beat') : layer;
      const routed = routeEverything(doc, li);
      const a = render(routed, li, curves, blobs);
      const b = render(routed, li, curves, blobs);
      expect(same(a.l, b.l) && same(a.r, b.r)).toBe(true);
      // The routes are live (outputs on the meters) and change the sound.
      expect(a.routed).toBeGreaterThan(40);
      const plain = render(doc, li, curves, blobs);
      expect(same(a.l, plain.l)).toBe(false);
      let peak = 0;
      for (const x of a.l) peak = Math.max(peak, Math.abs(x));
      expect(peak).toBeGreaterThan(0.01);
    }, 120_000);
  }
});
