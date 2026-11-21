// Stage S2 check 1 (docs/synth.md "S2: the modulation matrix"): each shaper field, on the real
// engine (klont.wasm), and the rule that no source can modulate itself, in the app's checks and in
// the engine's.
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { LAYER_DEPTH_OPEN, LOAD_MALFORMED, LOAD_OK, LOAD_PATCH, MSG_NOTE_ON, MSG_SET_LAYER, MSG_SET_MACRO, MSG_SET_PARAM, MSG_SET_PERF,
  M_LAYER_SRC, M_LIVE, M_ROUTES, M_STORED, SET_BASE, instantiateEngine, type Msg } from '../m1/protocol';
import { Core } from '../m1/render-core';
import { layoutOf, patchBlob, type Patch, type Route } from './patch';
import { checkRoute, makeRoute, withRoute, type RouteFields } from './routes';
import { toNorm } from './values';
import { DESC_ID } from '../gen/modules';

const wasm = readFileSync(resolve(import.meta.dirname, '../../public/wasm/klont.wasm'));
const SR = 48000;
const BLOCK = 128;

/** One voice patch (partials, amp, filter; env1 and lfo1), no channel. env1 gates the amp. */
function minimal(): Patch {
  return {
    voices: [{
      modules: [{ id: 'partials1', type: 'partials' }, { id: 'amp1', type: 'amp' }, { id: 'filter1', type: 'filter' }],
      cables: [{ from: 'partials1', to: 'amp1', port: 'in' }, { from: 'amp1', to: 'filter1', port: 'in' }],
      out: ['filter1'],
      mods: [{ id: 'env1', type: 'env' }, { id: 'env2', type: 'env' }, { id: 'lfo1', type: 'lfo' }],
      routes: [makeRoute('gate', 'env1', 'amp1.gain', { mode: 'scale', polarity: 'down' })],
    }],
    layerSources: [], channel: [], lfos: [], routes: [], macroNames: [],
    defaults: { 'route.gate': 1, 'filter1.res': 0.5 },
  };
}

class Rig {
  readonly core: Core;
  readonly lay: ReturnType<typeof layoutOf>;
  constructor(readonly patch: Patch, amounts: Record<string, number> = {}) {
    const { ex } = instantiateEngine(wasm, SR, BLOCK);
    this.core = new Core(ex, SR, BLOCK);
    patch = { ...patch, defaults: { ...patch.defaults, ...amounts } };
    this.patch = patch;
    expect(this.core.load(LOAD_PATCH, 0, patchBlob(patch, 0))).toBe(LOAD_OK);
    this.lay = layoutOf(patch);
  }
  idx(key: string): number {
    const i = this.lay.index.get(key);
    if (i === undefined) throw new Error(`no ${key}`);
    return i;
  }
  /** Render `ms` milliseconds; `msgs` go with the first block. */
  run(ms: number, msgs: Msg[] = []): void {
    const blocks = Math.max(1, Math.round((ms / 1000) * SR / BLOCK));
    for (let b = 0; b < blocks; b++) {
      this.core.send(b === 0 ? msgs : []);
      this.core.render(BLOCK);
    }
  }
  stored(key: string): number { return this.core.meter()[M_STORED + this.idx(key)]; }
  live(key: string): number { return this.core.meter()[M_LIVE + this.idx(key)]; }
  /** Output (shaped y) of layer route `j` or of route `i` of voice patch 0. */
  layerOut(j: number): number { return this.core.meter()[M_ROUTES + 192 + j]; }
  voiceOut(i: number): number { return this.core.meter()[M_ROUTES + i]; }
  macro(k: number, v: number): Msg { return [MSG_SET_MACRO, 0, k, 0, v]; }
}

/** A patch with one layer route from macro 1 into filter1.res, amount `amount` (real). */
function macroRoute(fields: RouteFields, amount = 0.5): Rig {
  const p = withRoute(minimal(), makeRoute('m', 'macro1', 'filter1.res', fields), 'layer');
  return new Rig(p, { 'route.m': amount });
}

const near = (x: number, y: number, eps = 2e-3) => expect(Math.abs(x - y), `${x} vs ${y}`).toBeLessThan(eps);

describe('the shaper on the engine', () => {
  it('in_lo and in_hi pick the part of the source that is used', () => {
    const r = macroRoute({ inLo: 0.25, inHi: 0.75 });
    r.run(300, [r.macro(0, 0.1)]);
    near(r.layerOut(0), 0);
    near(r.stored('filter1.res'), 0.5);
    r.run(300, [r.macro(0, 0.5)]);
    near(r.layerOut(0), 0.5);
    near(r.stored('filter1.res'), 0.5 + 0.5 * 0.5);
    r.run(300, [r.macro(0, 0.9)]);
    near(r.layerOut(0), 1);
    near(r.stored('filter1.res'), 1);
  });

  it('bend bows the line: y = u ^ 2^(-2 bend)', () => {
    for (const bend of [-0.5, 0.5]) {
      const r = macroRoute({ bend });
      r.run(300, [r.macro(0, 0.5)]);
      near(r.layerOut(0), Math.pow(0.5, Math.pow(2, -2 * bend)));
    }
  });

  it('points replace the bend, joined by straight lines', () => {
    const r = macroRoute({ points: [[0, 0], [0.5, 1], [1, 0]], bend: 0.9 });
    r.run(300, [r.macro(0, 0.25)]);
    near(r.layerOut(0), 0.5);
    r.run(300, [r.macro(0, 0.5)]);
    near(r.layerOut(0), 1);
    r.run(300, [r.macro(0, 0.875)]);
    near(r.layerOut(0), 0.25);
  });

  it('polarity: up adds y, down adds y - 1, both adds 2y - 1', () => {
    const want = { up: 0.5 + 0.4 * 0.25, down: 0.5 + 0.4 * -0.75, both: 0.5 + 0.4 * -0.5 };
    for (const polarity of ['up', 'down', 'both'] as const) {
      const r = macroRoute({ polarity }, 0.4);
      r.run(300, [r.macro(0, 0.25)]);
      near(r.stored('filter1.res'), want[polarity]);
    }
  });

  it('rise and fall smooth the source separately', () => {
    const r = macroRoute({ riseMs: 100, fallMs: 20 });
    r.run(50, [r.macro(0, 0)]);
    r.run(100, [r.macro(0, 1)]);
    // One-pole per 32-sample tick: after the rise time about 1 - 1/e.
    near(r.layerOut(0), 1 - Math.exp(-1), 0.03);
    r.run(900);
    near(r.layerOut(0), 1);
    r.run(20, [r.macro(0, 0)]);
    near(r.layerOut(0), Math.exp(-1), 0.03);
  });

  it('latch holds the source at note start', () => {
    const p = minimal();
    const free = withRoute(p, makeRoute('free', 'env1', 'filter1.res'), 0);
    const both = withRoute(free, makeRoute('held', 'env1', 'filter1.res', { latch: true }), 0);
    const r = new Rig(both, { 'route.free': 0.1, 'route.held': 0.1 });
    r.run(200, [[MSG_NOTE_ON, 0, 0, 0, 5700]]);
    // env1 starts at 0 and rises: the latched route keeps 0, the free one follows.
    near(r.voiceOut(2), 0);
    expect(r.voiceOut(1)).toBeGreaterThan(0.5);
  });

  it('via scales the amount by a second source', () => {
    const r = macroRoute({ via: 'macro2' }, 0.4);
    r.run(300, [r.macro(0, 1), r.macro(1, 0.5)]);
    near(r.stored('filter1.res'), 0.5 + 0.4 * 0.5);
    r.run(300, [r.macro(1, 0)]);
    near(r.stored('filter1.res'), 0.5);
  });

  it('a voice route may take a layer source as its via (the wheel deciding the depth)', () => {
    const p = withRoute(minimal(), makeRoute('v', 'note.velocity', 'filter1.res', { via: 'wheel' }), 0);
    const r = new Rig(p, { 'route.v': 0.4 });
    r.run(100, [[MSG_NOTE_ON, 0, 0, 0, 5700], [MSG_SET_PERF, 0, 0, 0, 0.25]]);
    near(r.live('filter1.res'), 0.5 + 0.4 * 0.25);
  });

  it('group scales the amount by that group depth of the layer', () => {
    const p = withRoute(minimal(), makeRoute('g', 'note.velocity', 'filter1.res', { group: 'open' }), 0);
    const r = new Rig(p, { 'route.g': 0.4 });
    r.run(100, [[MSG_SET_LAYER, 0, 0, LAYER_DEPTH_OPEN, 0.25], [MSG_NOTE_ON, 0, 0, 0, 5700]]);
    near(r.live('filter1.res'), 0.5 + 0.4 * 0.25);
  });

  it('the amount is a value a state holds and a message can set', () => {
    const r = macroRoute({}, 0.2);
    r.run(300, [r.macro(0, 1)]);
    near(r.stored('filter1.res'), 0.7);
    r.run(300, [[MSG_SET_PARAM, 0, SET_BASE, r.idx('route.m'), toNorm(DESC_ID['route.amount'], -0.3)]]);
    near(r.stored('filter1.res'), 0.2);
  });

  it('scale multiplies a gain; pull moves the target toward the source value', () => {
    const s = withRoute(minimal(), makeRoute('s', 'macro1', 'amp1.gain', { mode: 'scale', polarity: 'down' }), 'layer');
    const rs = new Rig(s, { 'route.s': 1 });
    rs.run(300, [rs.macro(0, 0.5)]);
    // amp.gain is 0..2 with 1 neutral (0.5 normalized); halved.
    near(rs.stored('amp1.gain'), 0.25);
    const pl = withRoute(minimal(), makeRoute('p', 'macro1', 'filter1.res', { mode: 'pull' }), 'layer');
    const rp = new Rig(pl, { 'route.p': 0.5 });
    rp.run(300, [rp.macro(0, 0.9)]);
    near(rp.stored('filter1.res'), 0.5 + 0.5 * (0.9 - 0.5));
  });

  it('a bipolar source enters the shaper as 0..1 (the bend)', () => {
    const r = new Rig(withRoute(minimal(), makeRoute('b', 'bend', 'filter1.res'), 'layer'), { 'route.b': 0.4 });
    r.run(300, [[MSG_SET_PERF, 0, 1, 0, -0.5]]);
    near(r.layerOut(0), 0.25);
    near(r.stored('filter1.res'), 0.5 + 0.4 * (2 * 0.25 - 1));
    expect(r.core.meter()[M_LAYER_SRC + 9]).toBe(-0.5);
  });

  it('a layer LFO moves a value every tick', () => {
    const p = withRoute(minimal(), makeRoute('l', 'layer.lfo1', 'filter1.res'), 'layer');
    const r = new Rig({ ...p, defaults: { ...p.defaults, 'layer.lfo1.rate': 4 } }, { 'route.l': 0.3 });
    const seen: number[] = [];
    for (let i = 0; i < 25; i++) { r.run(20); seen.push(r.stored('filter1.res')); }
    expect(Math.max(...seen) - Math.min(...seen)).toBeGreaterThan(0.3);
  });
});

describe('no source modulates itself', () => {
  const p = minimal();
  const bad: [string, Route, number | 'layer', Patch][] = [
    ['a voice LFO into an envelope', makeRoute('x', 'lfo1', 'env1.attack'), 0, p],
    ['an envelope into an LFO', makeRoute('x', 'env2', 'lfo1.rate'), 0, p],
    ['a layer LFO into the other layer LFO', makeRoute('x', 'layer.lfo1', 'layer.lfo2.rate'), 'layer',
      { ...p, lfos: [{ id: 'layer.lfo1', type: 'lfo' }, { id: 'layer.lfo2', type: 'lfo' }] }],
    ['a layer LFO into a voice envelope', makeRoute('x', 'layer.lfo1', 'env1.decay'), 'layer', p],
  ];
  for (const [what, r, scope, base] of bad) {
    it(`refuses ${what}`, () => {
      expect(() => checkRoute(base, r, scope)).toThrow(/modulate itself/);
      // The engine does not trust the app: built without the check, the patch is refused.
      const patch = withRoute(base, r, scope);
      const { ex } = instantiateEngine(wasm, SR, BLOCK);
      expect(new Core(ex, SR, BLOCK).load(LOAD_PATCH, 0, patchBlob(patch, 0))).toBe(LOAD_MALFORMED);
    });
  }

  it('lets other sources move envelope and LFO settings (a macro setting the LFO rate)', () => {
    const r = makeRoute('x', 'macro1', 'layer.lfo1.rate');
    const base = { ...p, lfos: [{ id: 'layer.lfo1', type: 'lfo' as const }] };
    expect(() => checkRoute(base, r, 'layer')).not.toThrow();
    const { ex } = instantiateEngine(wasm, SR, BLOCK);
    expect(new Core(ex, SR, BLOCK).load(LOAD_PATCH, 0, patchBlob(withRoute(base, r, 'layer'), 0))).toBe(LOAD_OK);
  });

  it('refuses two routes that move each other\'s amounts', () => {
    let q = withRoute(p, makeRoute('a', 'macro1', 'route.b'), 'layer');
    q = withRoute(q, makeRoute('b', 'macro2', 'route.a'), 'layer');
    const { ex } = instantiateEngine(wasm, SR, BLOCK);
    expect(new Core(ex, SR, BLOCK).load(LOAD_PATCH, 0, patchBlob(q, 0))).toBe(LOAD_MALFORMED);
  });
});
