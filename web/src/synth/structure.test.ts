// Stage S3 (docs/synth.md "S3: editing patches"): structure commands, run order and caps in the
// app, the cost meter against the engine's own voice count, and check 5: every factory preset,
// saved and loaded again, renders bit-identically.
import { readFileSync, readdirSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { LOAD_OK, MSG_NOTE_OFF, MSG_NOTE_ON, MSG_SELECT_LAYER, M_MAX_VOICES, instantiateEngine, type Msg } from '../m1/protocol';
import { Core } from '../m1/render-core';
import { commands, newProject } from '../project/commands';
import { Compiler } from '../project/compile';
import { applyOps } from '../project/patch';
import type { Project } from '../project/types';
import { parsePreset, presetFromLayer, presetText, type PresetFile } from '../presets/presets';
import { classicPatch } from './classic';
import { layerBudget, layoutOf, patchBlob, runOrder, voiceCount, type Patch } from './patch';

const wasm = readFileSync(resolve(import.meta.dirname, '../../public/wasm/klont.wasm'));
const SR = 48000;
const CHUNK = 4096;

const run = (doc: Project, name: keyof typeof commands, a: unknown): Project =>
  applyOps(doc, (commands[name] as (d: Project, x: unknown) => never)(doc, a)).doc;

function engine(doc: Project): Core {
  const { ex } = instantiateEngine(wasm, SR, CHUNK);
  const core = new Core(ex, SR, CHUNK);
  for (const u of new Compiler().compile(doc, () => null)) expect(core.load(u.kind, u.slot, u.blob), u.what).toBe(LOAD_OK);
  return core;
}

describe('structure commands', () => {
  it('add a source wired where the first source goes, a processor on the end, with values in every state', () => {
    let doc = newProject('s3');
    const l = doc.layers[0];
    doc = run(doc, 'addState', { layer: l.id });
    doc = run(doc, 'addModule', { layer: l.id, slot: 0, type: 'osc' });
    doc = run(doc, 'addModule', { layer: l.id, slot: 0, type: 'filter' });
    const p = doc.layers[0].patch;
    const vp = p.voices[0];
    expect(vp.cables).toContainEqual({ from: 'osc1', to: 'amp1', port: 'in' });
    expect(vp.cables).toContainEqual({ from: 'filter1', to: 'filter2', port: 'in' });
    expect(vp.out).toEqual(['filter2']);
    const keys = layoutOf(p).keys;
    for (const st of doc.layers[0].states) expect(Object.keys(st.values).sort()).toEqual([...keys].sort());
    expect(() => patchBlob(p, 0)).not.toThrow();
  });

  it('remove a module with its cables, the routes into it and its values', () => {
    let doc = newProject('s3');
    const id = doc.layers[0].id;
    doc = run(doc, 'addModule', { layer: id, slot: 0, type: 'osc' });
    doc = run(doc, 'addRoute', { layer: id, route: { source: 'lfo1', target: 'osc1.fine' } });
    doc = run(doc, 'addRoute', { layer: id, route: { source: 'macro2', target: 'osc1.level' } });
    doc = run(doc, 'removeModule', { layer: id, slot: 0, id: 'osc1' });
    const p = doc.layers[0].patch;
    expect(p.voices[0].modules.some((m) => m.id === 'osc1')).toBe(false);
    expect(JSON.stringify(p)).not.toContain('osc1');
    expect(Object.keys(doc.layers[0].states[0].values).some((k) => k.includes('osc1'))).toBe(false);
  });

  it('wire a phase input, refuse a loop and the caps in plain words', () => {
    let doc = newProject('s3');
    const id = doc.layers[0].id;
    doc = run(doc, 'addModule', { layer: id, slot: 0, type: 'osc' });
    doc = run(doc, 'addModule', { layer: id, slot: 0, type: 'osc' });
    doc = run(doc, 'setInputs', { layer: id, slot: 0, id: 'osc2', port: 'pm', from: ['osc1'] });
    // osc1 now feeds osc2; osc2 already feeds amp1, like osc1: the rack lists osc2 later, fine.
    expect(runOrder(doc.layers[0].patch.voices[0]).map((m) => m.id).indexOf('osc1')).toBeLessThan(runOrder(doc.layers[0].patch.voices[0]).map((m) => m.id).indexOf('osc2'));
    // Moving osc2 to the top of the rack does not change what runs first.
    doc = run(doc, 'moveModule', { layer: id, slot: 0, id: 'osc2', to: 0 });
    const order = runOrder(doc.layers[0].patch.voices[0]).map((m) => m.id);
    expect(order.indexOf('osc1')).toBeLessThan(order.indexOf('osc2'));
    expect(() => run(doc, 'setInputs', { layer: id, slot: 0, id: 'amp1', port: 'in', from: ['partials1', 'filter1'] })).toThrow(/loop/);
    expect(() => run(doc, 'setInputs', { layer: id, slot: 0, id: 'filter1', port: 'pm', from: ['osc1'] })).toThrow(/phase input/);
    doc = run(doc, 'addModule', { layer: id, slot: 0, type: 'osc' });
    doc = run(doc, 'addModule', { layer: id, slot: 0, type: 'osc' });
    expect(() => run(doc, 'addModule', { layer: id, slot: 0, type: 'osc' })).toThrow('a voice has at most 4 osc modules');
    for (let i = 0; i < 2; i++) doc = run(doc, 'addChannelFx', { layer: id, type: 'cfilter' });
    doc = run(doc, 'addChannelFx', { layer: id, type: 'delay' });
    expect(() => run(doc, 'addChannelFx', { layer: id, type: 'delay' })).toThrow('at most 2 delays');
    doc = run(doc, 'addChannelFx', { layer: id, type: 'crush' });
    // A new crush goes before the delays (it runs on the voices, before level and pan).
    expect(doc.layers[0].patch.channel.map((x) => x.type)).toEqual(['crush', 'tape', 'crush', 'delay', 'reverb', 'cfilter', 'cfilter', 'delay']);
    expect(() => run(doc, 'moveChannelFx', { layer: id, id: 'crush2', to: 7 })).toThrow(/before any delay or reverb/);
    expect(() => run(doc, 'removeMod', { layer: id, slot: 0, id: 'env1' })).toThrow(/ends the voice/);
    expect(() => run(doc, 'removeMod', { layer: id, slot: 0, id: 'env2' })).toThrow('env2 drives shaped_noise1');
  });

  it('bypass is a patch edit that keeps every value', () => {
    let doc = newProject('s3');
    const id = doc.layers[0].id;
    const before = doc.layers[0].states[0].values;
    doc = run(doc, 'setBypass', { layer: id, id: 'reverb1', bypass: true });
    expect(doc.layers[0].patch.channel.find((x) => x.id === 'reverb1')!.bypass).toBe(true);
    expect(doc.layers[0].states[0].values).toBe(before);
  });
});

describe('cost and voice count', () => {
  const budget = layerBudget([classicPatch('melody'), classicPatch('beat')]);

  it('the classic patches get 8 voices; a heavier patch fewer, the same number in the app and the engine', () => {
    expect(voiceCount(classicPatch('melody'), budget)).toBe(8);
    expect(voiceCount(classicPatch('beat'), budget)).toBe(8);
    let doc = newProject('cost');
    const id = doc.layers[0].id;
    for (const type of ['osc', 'osc', 'osc', 'osc', 'noise', 'noise', 'sampler', 'filter'] as const) doc = run(doc, 'addModule', { layer: id, slot: 0, type });
    for (const type of ['delay', 'cfilter', 'cfilter', 'cfilter'] as const) doc = run(doc, 'addChannelFx', { layer: id, type });
    for (let i = 0; i < 8; i++) doc = run(doc, 'addRoute', { layer: id, route: { source: 'lfo1', target: i % 2 ? 'partials1.tilt' : 'osc1.fine' } });
    const p = doc.layers[0].patch;
    const want = voiceCount(p, budget);
    expect(want).toBeLessThan(8);
    const core = engine(doc);
    core.send([[MSG_SELECT_LAYER, 0, 0, 0, 0]]);
    core.render(128);
    expect(core.meter()[M_MAX_VOICES]).toBe(want);
    core.free();
  });
});

/** Two seconds of the layer playing notes (melody) or the four kit slots (beat). */
function render(doc: Project, li: number): Float32Array {
  const core = engine(doc);
  const beat = doc.layers[li].kind === 'beat';
  const on: Msg[] = beat ? [0, 1, 2, 3].map((k) => [MSG_NOTE_ON, li, k, 0, NaN]) : [4800, 5500].map((c) => [MSG_NOTE_ON, li, 0, 0, c]);
  const off: Msg[] = beat ? [] : [4800, 5500].map((c) => [MSG_NOTE_OFF, li, 0, 0, c]);
  const n = Math.round((2 * SR) / CHUNK);
  const out = new Float32Array(2 * n * CHUNK);
  for (let c = 0; c < n; c++) {
    core.send(c === 0 ? on : c === 12 ? off : []);
    core.render(CHUNK);
    const o = core.out(CHUNK);
    out.set(o.l, 2 * c * CHUNK);
    out.set(o.r, (2 * c + 1) * CHUNK);
  }
  core.free();
  return out;
}

const bits = (a: Float32Array) => new Uint32Array(a.buffer, a.byteOffset, a.length);

describe('check 5: each factory preset saved and loaded again renders bit-identically', () => {
  const dir = resolve(import.meta.dirname, '../presets');
  const files = readdirSync(dir).filter((f) => f.endsWith('.json'));
  it('there are ten', () => expect(files).toHaveLength(10));
  for (const f of files) {
    it(f, () => {
      const factory = parsePreset(JSON.parse(readFileSync(resolve(dir, f), 'utf8')));
      const into = (p: PresetFile) => {
        let doc = newProject('presets');
        // Start from a different structure, so loading has to replace it.
        doc = run(doc, 'addModule', { layer: doc.layers[0].id, slot: 0, type: 'osc' });
        const li = p.kind === 'beat' ? 1 : 0;
        const l = doc.layers[li];
        return { doc: run(doc, 'loadPreset', { layer: l.id, state: l.states[0].id, preset: p }), li };
      };
      const a = into(factory);
      const saved = presetText(presetFromLayer(a.doc.layers[a.li], a.doc.layers[a.li].states[0], factory.name));
      const b = into(parsePreset(JSON.parse(saved)));
      expect(b.doc.layers[b.li].patch).toEqual(factory.patch as Patch);
      const ra = render(a.doc, a.li), rb = render(b.doc, b.li);
      let peak = 0;
      for (const x of ra) peak = Math.max(peak, Math.abs(x));
      expect(peak, 'the preset sounds').toBeGreaterThan(0.01);
      expect(bits(ra).every((x, i) => x === bits(rb)[i])).toBe(true);
    }, 60_000);
  }

  it('refuses a broken or foreign file in plain words', () => {
    expect(() => parsePreset({ format: 'other' })).toThrow('not an Earworm preset');
    const good = JSON.parse(readFileSync(resolve(dir, 'bell.json'), 'utf8'));
    expect(() => parsePreset({ ...good, v: 2 })).toThrow(/newer Earworm/);
    const loop = structuredClone(good);
    loop.patch.voices[0].cables.push({ from: 'filter1', to: 'amp1', port: 'in' });
    expect(() => parsePreset(loop)).toThrow(/loop/);
    expect(parsePreset({ ...good, values: { ...good.values, 'nonsense.key': 0.3 } }).values['nonsense.key']).toBeUndefined();
    let doc = newProject('k');
    expect(() => run(doc, 'loadPreset', { layer: doc.layers[1].id, state: doc.layers[1].states[0].id, preset: good })).toThrow(/beat layer/);
    doc = run(doc, 'loadPreset', { layer: doc.layers[0].id, state: doc.layers[0].states[0].id, preset: good });
    expect(doc.layers[0].patch).toEqual(good.patch);
  });
});
