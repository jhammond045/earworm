// The route and value commands on a real document: where a route goes, what it adds to every
// state, that undo takes it back, and the plain refusals.
import { describe, expect, it } from 'vitest';
import { commands, newProject } from '../project/commands';
import { applyOps } from '../project/patch';
import { Store } from '../project/store';
import type { Project } from '../project/types';
import { layoutOf, patchBlob } from './patch';
import { findRoute } from './routes';
import { classicVectors } from './classic';
import { layerBlob } from '../project/compile';

function doc(): Project {
  let d = newProject('t');
  d = applyOps(d, commands.addLayer(d, { kind: 'beat' })).doc;
  return d;
}

describe('route commands', () => {
  it('a per-voice source makes a route of the voice patch; its amount goes into every state', () => {
    const s = new Store(doc());
    const l = s.doc.layers[0];
    s.dispatch('addState', { layer: l.id });
    expect(s.dispatch('addRoute', { layer: l.id, route: { id: 'h', source: 'note.hardness', target: 'env1.attack' }, amount: -0.4 })).toBe(true);
    const p = s.doc.layers[0].patch;
    expect(findRoute(p, 0, 'h')?.target).toBe('env1.attack');
    for (const st of s.doc.layers[0].states) expect(st.values['route.h']).toBeCloseTo(0.3);
    expect(layoutOf(p).keys).toContain('route.h');
    expect(() => patchBlob(p, 0)).not.toThrow();
    s.undo();
    expect(findRoute(s.doc.layers[0].patch, 0, 'h')).toBeNull();
    expect(s.doc.layers[0].states[0].values['route.h']).toBeUndefined();
  });

  it('a per-layer source makes a layer route, and a layer LFO the patch lacks is added', () => {
    const s = new Store(doc());
    const l = s.doc.layers[0];
    s.dispatch('addRoute', { layer: l.id, route: { id: 'w', source: 'layer.lfo2', target: 'reverb1.mix' } });
    const p = s.doc.layers[0].patch;
    expect(findRoute(p, 'layer', 'w')?.polarity).toBe('both');
    expect(p.lfos.map((m) => m.id)).toEqual(['layer.lfo2']);
    expect(layoutOf(p).keys).toContain('layer.lfo2.rate');
  });

  it('on a beat layer a voice route goes to the kit slot asked for, by local or full key', () => {
    const s = new Store(doc());
    const beat = s.doc.layers.find((x) => x.kind === 'beat')!;
    s.dispatch('addRoute', { layer: beat.id, slot: 2, route: { id: 'v', source: 'note.velocity', target: 'k3.filter1.cutoff' } });
    const p = s.doc.layers.find((x) => x.id === beat.id)!.patch;
    expect(findRoute(p, 2, 'v')?.target).toBe('filter1.cutoff');
    expect(findRoute(p, 0, 'v')).toBeNull();
    expect(s.doc.layers.find((x) => x.id === beat.id)!.states[0].values['k3.route.v']).toBeDefined();
  });

  it('setRoute changes the shaper and keeps the route in place; removeRoute drops it and its amount', () => {
    const s = new Store(doc());
    const l = s.doc.layers[0];
    s.dispatch('addRoute', { layer: l.id, route: { id: 'a', source: 'macro1', target: 'filter1.res' } });
    s.dispatch('addRoute', { layer: l.id, route: { id: 'b', source: 'macro2', target: 'filter1.res' } });
    s.dispatch('setRoute', { layer: l.id, scope: 'layer', id: 'a', fields: { bend: 0.5, points: [[0, 0], [1, 0.5]], riseMs: 10 } });
    const routes = s.doc.layers[0].patch.routes;
    expect(routes.slice(-2).map((r) => r.id)).toEqual(['a', 'b']);
    expect(routes.at(-2)).toMatchObject({ bend: 0.5, riseMs: 10, points: [[0, 0], [1, 0.5]] });
    s.dispatch('removeRoute', { layer: l.id, scope: 'layer', id: 'a' });
    expect(findRoute(s.doc.layers[0].patch, 'layer', 'a')).toBeNull();
    expect(s.doc.layers[0].states[0].values['route.a']).toBeUndefined();
  });

  it('setValues sets values by key and refuses keys the patch does not have', () => {
    const s = new Store(doc());
    const l = s.doc.layers[0];
    const st = l.states[0];
    s.dispatch('setValues', { layer: l.id, state: st.id, values: { 'filter1.res': 0.7 } });
    expect(s.doc.layers[0].states[0].values['filter1.res']).toBe(0.7);
    expect(() => s.dispatch('setValues', { layer: l.id, state: st.id, values: { 'nope.x': 0.1 } })).toThrow(/no value nope.x/);
  });

  it('refuses in plain words', () => {
    const d = doc();
    const l = d.layers[0];
    const add = (route: { source: string; target: string } & Record<string, unknown>) => () => commands.addRoute(d, { layer: l.id, route });
    expect(add({ source: 'lfo1', target: 'lfo2.rate' })).toThrow(/modulate itself/);
    expect(add({ source: 'macro1', target: 'pitch' })).toThrow(/pitch is per voice/);
    expect(add({ source: 'note.velocity', target: 'delay1.mix' })).toThrow(/only a layer source/);
    expect(add({ source: 'note.velocity', target: 'nothing.here' })).toThrow(/no value/);
    expect(add({ source: 'macro1', target: 'filter1.res', mode: 'scale' })).toThrow(/gain/);
    expect(add({ source: 'macro1', target: 'filter1.res', latch: true })).toThrow(/latch/);
  });

  it('a classic patch with routes added keeps its binding to the old parameters (Influence still reaches it)', () => {
    const s = new Store(doc());
    const l = s.doc.layers[0];
    s.dispatch('addRoute', { layer: l.id, route: { id: 'x', source: 'layer.lfo1', target: 'filter1.res' } });
    const cur = s.doc.layers[0];
    const target = new Array(96).fill(0.25);
    const v = classicVectors(cur.kind, cur.states[0].values, target, cur.patch);
    const lay = layoutOf(cur.patch);
    expect(v.values.length).toBe(lay.keys.length);
    const cut = lay.index.get('filter1.cutoff')!;
    expect(v.target[cut]).toBe(0.25);
    expect(v.groups[cut]).not.toBe(0);
    expect(v.groups[lay.index.get('route.x')!]).toBe(0);
    expect(new Uint32Array(layerBlob(cur), 17 * 4, 1)[0]).toBe(lay.keys.length);
  });
});
