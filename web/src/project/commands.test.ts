import { describe, expect, it } from 'vitest';
import { commands, newId, newProject } from './commands';
import { applyOps } from './patch';
import type { Op } from './patch';
import { Store } from './store';
import type { BeatClip, Hit, MelodyClip, Project, Take } from './types';

function deepFreeze<T>(v: T): T {
  if (typeof v === 'object' && v !== null && !Object.isFrozen(v)) {
    Object.freeze(v);
    for (const x of Object.values(v)) deepFreeze(x);
  }
  return v;
}

const shape = { attack: 0, burst: 0, cents: null, bend: 0, hold: 0, vowel: 0, tone: 0, endStop: 0, endClick: 0 };
const hit = (id: string, t: number, slot = 0): Hit => ({ id, t, dur: 120, slot, strength: 0.8, shape });
const note = (id: string, t: number, cents = 4800) => ({ id, t, dur: 480, cents, hard: 0.5 });
const take = (id: string): Take => ({ id, audio: `${id}.a`, frames: `${id}.f`, rate: 16000, latencyMs: 30, frameCount: 100, kind: 'melody',
  refCents: 0, root: 0, scale: 0, bpm: null });

function setup() {
  const store = new Store(newProject('Test'));
  const sec = store.doc.sections[0].id;
  const [mel, beat] = store.doc.layers.map((l) => l.id);
  return { store, sec, mel, beat };
}
const clipOf = (s: Store, sec: string, layer: string) => s.doc.sections.find((x) => x.id === sec)!.clips[layer];

describe('newProject', () => {
  it('has one grid section A, a melody and a beat layer', () => {
    const p = newProject('Song');
    expect(p.name).toBe('Song');
    expect(p.sections).toHaveLength(1);
    expect(p.sections[0]).toMatchObject({ name: 'A', feel: 'grid', beats: 8, bpm: 100, repeats: 1 });
    expect(p.layers.map((l) => [l.name, l.kind])).toEqual([['Melody', 'melody'], ['Beat', 'beat']]);
    expect(p.layers[0].states[0].base).toHaveLength(96);
    expect(p.layers[1].kit).toHaveLength(4);
    expect(p.layers[1].kit[0].params).toHaveLength(96);
    expect(p.loop).toBe(true);
    expect(newId('x')).not.toBe(newId('x'));
  });
});

describe('commands never mutate', () => {
  it('works on a deep-frozen project and keeps untouched layers by identity', () => {
    const doc = deepFreeze(newProject('Frozen'));
    const mel = doc.layers[0].id;
    const ops = commands.setLayer(doc, { id: mel, level: -6, tune: { snap: 0.5 } });
    const next = applyOps(doc, ops).doc;
    expect(next.layers[0].level).toBe(-6);
    expect(next.layers[0].tune.snap).toBe(0.5);
    expect(next.layers[1]).toBe(doc.layers[1]);
    expect(next.sections).toBe(doc.sections);
    expect(next.layers[0].states).toBe(doc.layers[0].states);
    expect(doc.layers[0].level).toBe(0);
  });

  it('keeps sibling clips and sections the same object', () => {
    const { store, sec, mel, beat } = setup();
    store.dispatch('addSection', { feel: 'grid' });
    store.dispatch('addNote', { section: sec, layer: mel, note: note('n1', 0) });
    store.dispatch('addHit', { section: sec, layer: beat, hit: hit('h1', 0) });
    const before = store.doc;
    store.dispatch('addNote', { section: sec, layer: mel, note: note('n2', 480) });
    expect(store.doc.sections[1]).toBe(before.sections[1]);
    expect(store.doc.sections[0].clips[beat]).toBe(before.sections[0].clips[beat]);
    expect(store.doc.layers).toBe(before.layers);
    expect((clipOf(store, sec, mel) as MelodyClip).notes[0]).toBe((before.sections[0].clips[mel] as MelodyClip).notes[0]);
  });
});

describe('project and sections', () => {
  it('renames, and skips a same-name rename', () => {
    const { store } = setup();
    expect(store.dispatch('renameProject', { name: 'New' })).toBe(true);
    expect(store.doc.name).toBe('New');
    expect(store.dispatch('renameProject', { name: 'New' })).toBe(false);
  });

  it('adds sections after a given one, with the next unused letter', () => {
    const { store, sec } = setup();
    store.dispatch('addSection', { feel: 'grid', bpm: 120 });
    store.dispatch('addSection', { after: sec, feel: 'free', seconds: 5 });
    expect(store.doc.sections.map((s) => s.name)).toEqual(['A', 'C', 'B']);
    expect(store.doc.sections[1]).toMatchObject({ feel: 'free', seconds: 5 });
    expect(store.doc.sections[2].bpm).toBe(120);
  });

  it('moves, duplicates and removes sections; the last cannot go', () => {
    const { store, sec } = setup();
    store.dispatch('addSection', { feel: 'grid' });
    const b = store.doc.sections[1].id;
    store.dispatch('moveSection', { id: sec, to: 1 });
    expect(store.doc.sections.map((s) => s.id)).toEqual([b, sec]);
    expect(store.dispatch('moveSection', { id: sec, to: 1 })).toBe(false);
    store.dispatch('duplicateSection', { id: sec });
    expect(store.doc.sections).toHaveLength(3);
    expect(store.doc.sections[2].id).not.toBe(sec);
    store.dispatch('removeSection', { id: b });
    store.dispatch('removeSection', { id: sec });
    expect(() => store.dispatch('removeSection', { id: store.doc.sections[0].id })).toThrow(/last section/);
  });

  it('feel change converts note, hit and loop times at the section bpm', () => {
    const { store, sec, mel, beat } = setup();
    // 100 bpm: one beat = 0.6 s = 960 ticks
    store.dispatch('addNote', { section: sec, layer: mel, note: { ...note('n1', 960), dur: 480 } });
    store.dispatch('addHit', { section: sec, layer: beat, hit: hit('h1', 1920) });
    store.dispatch('setSection', { id: sec, feel: 'free' });
    const m = clipOf(store, sec, mel) as MelodyClip;
    expect(m.loopLen).toBeCloseTo(8 * 0.6);
    expect(m.notes[0].t).toBeCloseTo(0.6);
    expect(m.notes[0].dur).toBeCloseTo(0.3);
    expect((clipOf(store, sec, beat) as BeatClip).hits[0].t).toBeCloseTo(1.2);
    store.dispatch('setSection', { id: sec, feel: 'grid' });
    const back = clipOf(store, sec, mel) as MelodyClip;
    expect(back.notes[0].t).toBe(960);
    expect(back.notes[0].dur).toBe(480);
    expect(back.loopLen).toBe(7680);
  });

  it('a bpm change on a grid section leaves clips alone', () => {
    const { store, sec, mel } = setup();
    store.dispatch('addNote', { section: sec, layer: mel, note: note('n1', 960) });
    const clip = clipOf(store, sec, mel);
    store.dispatch('setSection', { id: sec, bpm: 140, repeats: 2 });
    expect(clipOf(store, sec, mel)).toBe(clip);
    expect(store.doc.sections[0]).toMatchObject({ bpm: 140, repeats: 2 });
  });

  it('rejects bad input with a plain RangeError', () => {
    const { store, sec } = setup();
    expect(() => store.dispatch('setSection', { id: 'nope', name: 'x' })).toThrow(RangeError);
    expect(() => store.dispatch('setSection', { id: sec, bpm: 5 })).toThrow(/bpm/);
    expect(() => store.dispatch('setSection', { id: sec, repeats: 0 })).toThrow(/repeats/);
    expect(() => store.dispatch('addSection', { after: 'zzz', feel: 'grid' })).toThrow(/unknown section id/);
  });
});

describe('layers', () => {
  it('adds a layer from a preset and stops at 8', () => {
    const { store } = setup();
    store.dispatch('addLayer', { kind: 'melody', sound: 2 });
    expect(store.doc.layers[2].name).toBe('Melody 2');
    expect(store.doc.layers[2].states[0].base).toHaveLength(96);
    store.dispatch('addLayer', { kind: 'beat', name: 'Perc', sound: 1 });
    expect(store.doc.layers[3].kit).toHaveLength(4);
    for (let i = store.doc.layers.length; i < 8; i++) store.dispatch('addLayer', { kind: 'texture' });
    expect(() => store.dispatch('addLayer', { kind: 'melody' })).toThrow(/too many layers/);
    expect(() => store.dispatch('addLayer', { kind: 'melody', sound: 99 } as never)).toThrow(RangeError);
  });

  it('removes a layer and its clips', () => {
    const { store, sec, mel } = setup();
    store.dispatch('addNote', { section: sec, layer: mel, note: note('n1', 0) });
    store.dispatch('removeLayer', { id: mel });
    expect(store.doc.layers).toHaveLength(1);
    expect(Object.keys(store.doc.sections[0].clips)).toEqual([]);
    store.undo();
    expect(clipOf(store, sec, mel)).toBeDefined();
  });

  it('sets fields, partial depth and tune, and validates ranges', () => {
    const { store, mel } = setup();
    store.dispatch('setLayer', { id: mel, name: 'Lead', level: -3, pan: 0.5, mute: true, depth: { loud: 0.2 }, tune: { slides: 2, octave: 1 }, follow: 0.4 });
    const l = store.doc.layers[0];
    expect(l).toMatchObject({ name: 'Lead', level: -3, pan: 0.5, mute: true, follow: 0.4 });
    expect(l.depth).toMatchObject({ loud: 0.2, open: 0.5 });
    expect(l.tune).toMatchObject({ slides: 2, octave: 1, snap: 0 });
    expect(() => store.dispatch('setLayer', { id: mel, level: 40 })).toThrow(/level/);
    expect(() => store.dispatch('setLayer', { id: mel, pan: 2 })).toThrow(/pan/);
    expect(() => store.dispatch('setLayer', { id: 'x', pan: 0 })).toThrow(/unknown layer id/);
    expect(store.dispatch('setLayer', { id: mel, level: -3 })).toBe(false);
  });

  it('sets a layer sound; kit only on beat layers', () => {
    const { store, mel, beat } = setup();
    const base = new Array(96).fill(0.25);
    store.dispatch('setLayerSound', { id: mel, base });
    expect(store.doc.layers[0].states[0].base).toBe(base);
    const kit = store.doc.layers[1].kit.map((k) => ({ ...k, cents: 3000 }));
    store.dispatch('setLayerSound', { id: beat, kit });
    expect(store.doc.layers[1].kit[2].cents).toBe(3000);
    expect(() => store.dispatch('setLayerSound', { id: mel, kit })).toThrow(/beat layer/);
    expect(() => store.dispatch('setLayerSound', { id: mel, base: [1, 2] })).toThrow(/96/);
  });
});

describe('clips and notes', () => {
  it('addNote creates a clip sized to the section and keeps notes sorted', () => {
    const { store, sec, mel } = setup();
    store.dispatch('addNote', { section: sec, layer: mel, note: note('b', 960) });
    store.dispatch('addNote', { section: sec, layer: mel, note: note('a', 0) });
    store.dispatch('addNote', { section: sec, layer: mel, note: note('c', 480) });
    const c = clipOf(store, sec, mel) as MelodyClip;
    expect(c.loopLen).toBe(8 * 960);
    expect(c.notes.map((n) => n.id)).toEqual(['a', 'c', 'b']);
    expect(() => store.dispatch('addNote', { section: sec, layer: mel, note: note('a', 5) })).toThrow(/already exists/);
  });

  it('clamps cents, wraps times, keeps dur above 0', () => {
    const { store, sec, mel } = setup();
    store.dispatch('addNote', { section: sec, layer: mel, note: { id: 'n', t: 8 * 960 + 10, dur: 0, cents: 99999, hard: 3 } });
    const n = (clipOf(store, sec, mel) as MelodyClip).notes[0];
    expect(n).toMatchObject({ t: 10, dur: 1, cents: 12000, hard: 1 });
    store.dispatch('moveNotes', { section: sec, layer: mel, ids: ['n'], dt: -20, dcents: -99999 });
    expect((clipOf(store, sec, mel) as MelodyClip).notes[0]).toMatchObject({ t: 8 * 960 - 10, cents: 0 });
    store.dispatch('resizeNote', { section: sec, layer: mel, id: 'n', dur: -5 });
    expect((clipOf(store, sec, mel) as MelodyClip).notes[0].dur).toBe(1);
    expect(() => store.dispatch('addNote', { section: sec, layer: mel, note: { id: 'z', t: NaN, dur: 1, cents: 0, hard: 0 } })).toThrow(RangeError);
  });

  it('free sections use a 10 ms minimum duration', () => {
    const { store } = setup();
    store.dispatch('addSection', { feel: 'free', seconds: 4 });
    const f = store.doc.sections[1].id;
    const mel = store.doc.layers[0].id;
    store.dispatch('addNote', { section: f, layer: mel, note: { id: 'n', t: 4.5, dur: 0, cents: 100, hard: 0 } });
    expect((clipOf(store, f, mel) as MelodyClip).notes[0]).toMatchObject({ t: 0.5, dur: 0.01 });
  });

  it('moveNotes wraps modulo loopLen and re-sorts; unknown ids throw; zero move is no change', () => {
    const { store, sec, mel } = setup();
    for (const [id, t] of [['a', 0], ['b', 960], ['c', 1920]] as const) store.dispatch('addNote', { section: sec, layer: mel, note: note(id, t) });
    store.dispatch('moveNotes', { section: sec, layer: mel, ids: ['a', 'b'], dt: 1920, dcents: 100 });
    const c = clipOf(store, sec, mel) as MelodyClip;
    expect(c.notes.map((n) => [n.id, n.t])).toEqual([['a', 1920], ['c', 1920], ['b', 2880]]);
    expect(c.notes.find((n) => n.id === 'a')!.cents).toBe(4900);
    expect(store.dispatch('moveNotes', { section: sec, layer: mel, ids: ['a'], dt: 0, dcents: 0 })).toBe(false);
    expect(() => store.dispatch('moveNotes', { section: sec, layer: mel, ids: ['zz'], dt: 1, dcents: 0 })).toThrow(/unknown note id/);
    expect(store.dispatch('moveNotes', { section: sec, layer: mel, ids: ['c'], dt: 8 * 960, dcents: 0 })).toBe(false);
  });

  it('deleteNotes removes several', () => {
    const { store, sec, mel } = setup();
    for (const [id, t] of [['a', 0], ['b', 960], ['c', 1920]] as const) store.dispatch('addNote', { section: sec, layer: mel, note: note(id, t) });
    store.dispatch('deleteNotes', { section: sec, layer: mel, ids: ['a', 'c'] });
    expect((clipOf(store, sec, mel) as MelodyClip).notes.map((n) => n.id)).toEqual(['b']);
  });

  it('refuses notes on a beat layer and hits on a melody layer', () => {
    const { store, sec, mel, beat } = setup();
    expect(() => store.dispatch('addNote', { section: sec, layer: beat, note: note('n', 0) })).toThrow(/beat layer/);
    expect(() => store.dispatch('addHit', { section: sec, layer: mel, hit: hit('h', 0) })).toThrow(/not a beat layer/);
  });

  it('setClip places and clears a clip, and rejects a mismatched kind', () => {
    const { store, sec, mel, beat } = setup();
    const clip: MelodyClip = { kind: 'melody', take: 't1', takes: ['t1'], loopLen: 1920, timeSnap: 0, morph: [], notes: [note('a', 4000)], warp: [] };
    store.batch('place take', (run) => {
      run('addTake', { take: take('t1') });
      run('setClip', { section: sec, layer: mel, clip });
    });
    expect(store.doc.takes.t1.id).toBe('t1');
    expect((clipOf(store, sec, mel) as MelodyClip).notes[0].t).toBe(4000 % 1920);
    expect(() => store.dispatch('setClip', { section: sec, layer: beat, clip })).toThrow(/does not fit/);
    store.dispatch('setClip', { section: sec, layer: mel, clip: null });
    expect(clipOf(store, sec, mel)).toBeUndefined();
    expect(store.dispatch('setClip', { section: sec, layer: mel, clip: null })).toBe(false);
  });

  it('setClipFields changes loopLen (wrapping notes) and timeSnap', () => {
    const { store, sec, mel } = setup();
    store.dispatch('addNote', { section: sec, layer: mel, note: note('a', 5000) });
    store.dispatch('setClipFields', { section: sec, layer: mel, loopLen: 3840, timeSnap: 240 });
    const c = clipOf(store, sec, mel) as MelodyClip;
    expect(c).toMatchObject({ loopLen: 3840, timeSnap: 240 });
    expect(c.notes[0].t).toBe(5000 - 3840);
    expect(() => store.dispatch('setClipFields', { section: sec, layer: mel, loopLen: 0 })).toThrow(/loopLen/);
    expect(() => store.dispatch('setClipFields', { section: sec, layer: store.doc.layers[1].id, timeSnap: 1 })).toThrow(/no clip/);
  });

  it('quantize snaps starts to the grid and wraps a rounded-up end', () => {
    const { store, sec, mel } = setup();
    store.dispatch('addNote', { section: sec, layer: mel, note: note('a', 250) });
    store.dispatch('addNote', { section: sec, layer: mel, note: note('b', 7600) });
    store.dispatch('addNote', { section: sec, layer: mel, note: note('c', 500) });
    store.dispatch('quantize', { section: sec, layer: mel, grid: 480, ids: ['a', 'b'] });
    const c = clipOf(store, sec, mel) as MelodyClip;
    expect(c.notes.map((n) => [n.id, n.t])).toEqual([['b', 0], ['a', 480], ['c', 500]].sort((x, y) => (x[1] as number) - (y[1] as number)));
    expect(store.dispatch('quantize', { section: sec, layer: mel, grid: 480, ids: ['a'] })).toBe(false);
  });

  it('selectTake only accepts a take in the stack', () => {
    const { store, sec, mel } = setup();
    const clip: MelodyClip = { kind: 'melody', take: 't2', takes: ['t1', 't2'], loopLen: 1920, timeSnap: 0, morph: [], notes: [], warp: [] };
    store.dispatch('setClip', { section: sec, layer: mel, clip });
    store.dispatch('selectTake', { section: sec, layer: mel, take: 't1' });
    expect(clipOf(store, sec, mel).take).toBe('t1');
    expect(() => store.dispatch('selectTake', { section: sec, layer: mel, take: 'zz' })).toThrow(/not in this clip/);
  });
});

describe('hits', () => {
  it('adds, moves with wrap, changes slot, quantizes and deletes', () => {
    const { store, sec, beat } = setup();
    store.dispatch('addHit', { section: sec, layer: beat, hit: hit('a', 100) });
    store.dispatch('addHit', { section: sec, layer: beat, hit: hit('b', 1000, 1) });
    store.dispatch('moveHits', { section: sec, layer: beat, ids: ['a'], dt: -200 });
    expect((clipOf(store, sec, beat) as BeatClip).hits.find((h) => h.id === 'a')!.t).toBe(8 * 960 - 100);
    store.dispatch('setHitSlot', { section: sec, layer: beat, ids: ['a', 'b'], slot: 3 });
    expect((clipOf(store, sec, beat) as BeatClip).hits.map((h) => h.slot)).toEqual([3, 3]);
    expect(() => store.dispatch('setHitSlot', { section: sec, layer: beat, ids: ['a'], slot: 4 })).toThrow(/slot/);
    store.dispatch('quantize', { section: sec, layer: beat, grid: 960 });
    expect((clipOf(store, sec, beat) as BeatClip).hits.map((h) => h.t)).toEqual([0, 960]);
    store.dispatch('deleteHits', { section: sec, layer: beat, ids: ['a'] });
    expect((clipOf(store, sec, beat) as BeatClip).hits.map((h) => h.id)).toEqual(['b']);
  });
});

describe('commands return ops only for real changes', () => {
  it('returns [] when nothing changes', () => {
    const doc = newProject('x');
    const ops: Op[] = commands.setLayer(doc, { id: doc.layers[0].id, level: 0, name: doc.layers[0].name });
    expect(ops).toEqual([]);
  });
});

// ---------- M4: Influence, states, morph markers, kit slot ----------

import { DESCS } from '../gen/params';
import type { Influence } from './types';

const ramp = (f: (i: number) => number) => Array.from({ length: 96 }, (_, i) => f(i));
const influence = (over: Partial<Influence> = {}): Influence => ({
  target: ramp(() => 1), mask: 0x1f, source: 'grain-a', sourceName: 'kettle', sourceRate: 16000,
  grain: { start: 100, end: 900, refCents: 6000 }, useSound: true, ...over,
});

describe('Influence commands', () => {
  it('setInfluence sets and clears, and rejects bad values', () => {
    const { store, mel } = setup();
    const infl = influence();
    expect(store.dispatch('setInfluence', { layer: mel, influence: infl })).toBe(true);
    expect(store.doc.layers[0].influence).toEqual(infl);
    expect(store.dispatch('setInfluence', { layer: mel, influence: null })).toBe(true);
    expect('influence' in store.doc.layers[0]).toBe(false);
    expect(store.dispatch('setInfluence', { layer: mel, influence: null })).toBe(false);
    store.undo();
    expect(store.doc.layers[0].influence).toEqual(infl);
    const bad = (i: Partial<Influence>) => () => store.dispatch('setInfluence', { layer: mel, influence: influence(i) });
    expect(bad({ target: ramp(() => 1.5) })).toThrow(RangeError);
    expect(bad({ target: [0.5] })).toThrow(RangeError);
    expect(bad({ mask: 32 })).toThrow(RangeError);
    expect(bad({ mask: 1.5 })).toThrow(RangeError);
    expect(bad({ source: 'take-1' })).toThrow(RangeError);
    expect(bad({ sourceRate: 0 })).toThrow(RangeError);
    expect(bad({ grain: { start: 5, end: 5, refCents: null } })).toThrow(RangeError);
    expect(() => store.dispatch('setInfluence', { layer: 'nope', influence: null })).toThrow(RangeError);
  });

  it('setInfluenceAmount and setInfluenceMask validate and are no-ops when unchanged', () => {
    const { store, mel } = setup();
    const st = store.doc.layers[0].states[0].id;
    expect(store.dispatch('setInfluenceAmount', { layer: mel, state: st, amount: 0.4 })).toBe(true);
    expect(store.doc.layers[0].states[0].influenceAmount).toBe(0.4);
    expect(store.dispatch('setInfluenceAmount', { layer: mel, state: st, amount: 0.4 })).toBe(false);
    expect(() => store.dispatch('setInfluenceAmount', { layer: mel, state: st, amount: 1.1 })).toThrow(RangeError);
    expect(() => store.dispatch('setInfluenceAmount', { layer: mel, state: 'x', amount: 0.1 })).toThrow(RangeError);
    expect(() => store.dispatch('setInfluenceMask', { layer: mel, mask: 3 })).toThrow(/no Influence/);
    store.dispatch('setInfluence', { layer: mel, influence: influence() });
    expect(store.dispatch('setInfluenceMask', { layer: mel, mask: 3 })).toBe(true);
    expect(store.doc.layers[0].influence!.mask).toBe(3);
    expect(() => store.dispatch('setInfluenceMask', { layer: mel, mask: 99 })).toThrow(RangeError);
  });

  it('commitInfluence blends per masked group, then clears the Influence and the amount', () => {
    const { store, mel } = setup();
    const layer0 = store.doc.layers[0];
    const st = layer0.states[0];
    const base = ramp((i) => (i % 10) / 10);
    store.dispatch('setState', { layer: mel, id: st.id, base });
    store.dispatch('setInfluence', { layer: mel, influence: influence({ target: ramp(() => 1), mask: 1 | 4 }) }); // tone and motion
    store.dispatch('setInfluenceAmount', { layer: mel, state: st.id, amount: 0.5 });
    store.dispatch('commitInfluence', { layer: mel, state: st.id });
    const after = store.doc.layers[0];
    expect(after.influence).toBeUndefined();
    expect(after.states[0].influenceAmount).toBe(0);
    const bit = { tone: 1, shape: 2, motion: 4, grit: 8, space: 16 } as const;
    const seen = new Set<string>();
    after.states[0].base.forEach((v, i) => {
      const g = DESCS[i].group;
      seen.add(g);
      const masked = (1 | 4) & bit[g];
      expect(v).toBeCloseTo(masked ? 0.5 * base[i] + 0.5 : base[i], 12);
    });
    expect(seen.size).toBeGreaterThan(3); // the table really spans several groups
    store.undo();
    expect(store.doc.layers[0].influence).toBeTruthy();
    expect(store.doc.layers[0].states[0].base).toEqual(base);
    expect(() => store.dispatch('commitInfluence', { layer: mel, state: 'x' })).toThrow(RangeError);
  });

  it('commitInfluence at amount 0 keeps the base, at 1 with a full mask takes the target', () => {
    const { store, mel } = setup();
    const st = store.doc.layers[0].states[0];
    const base = st.base.slice();
    const target = ramp((i) => ((i * 7) % 11) / 10);
    store.dispatch('setInfluence', { layer: mel, influence: influence({ target }) });
    store.dispatch('commitInfluence', { layer: mel, state: st.id });
    expect(store.doc.layers[0].states[0].base).toEqual(base);
    store.dispatch('setInfluence', { layer: mel, influence: influence({ target }) });
    store.dispatch('setInfluenceAmount', { layer: mel, state: st.id, amount: 1 });
    store.dispatch('commitInfluence', { layer: mel, state: st.id });
    expect(store.doc.layers[0].states[0].base).toEqual(target);
  });

  it('commitInfluence needs an Influence', () => {
    const { store, mel } = setup();
    expect(() => store.dispatch('commitInfluence', { layer: mel, state: store.doc.layers[0].states[0].id })).toThrow(/no Influence/);
  });
});

describe('state commands', () => {
  it('addState copies a state, names it, caps at 8 and rejects bad ids', () => {
    const { store, mel } = setup();
    const first = store.doc.layers[0].states[0];
    store.dispatch('addState', { layer: mel });
    const s2 = store.doc.layers[0].states[1];
    expect(s2.name).toBe('State 2');
    expect(s2.base).toEqual(first.base);
    expect(s2.base).not.toBe(first.base);
    expect(s2.id).not.toBe(first.id);
    store.dispatch('addState', { layer: mel, from: s2.id, name: 'Wide', id: 'st_fixed' });
    expect(store.doc.layers[0].states[2]).toMatchObject({ id: 'st_fixed', name: 'Wide' });
    expect(() => store.dispatch('addState', { layer: mel, id: 'st_fixed' })).toThrow(/already exists/);
    expect(() => store.dispatch('addState', { layer: mel, from: 'nope' })).toThrow(RangeError);
    for (let i = 3; i < 8; i++) store.dispatch('addState', { layer: mel });
    expect(store.doc.layers[0].states).toHaveLength(8);
    expect(() => store.dispatch('addState', { layer: mel })).toThrow(/too many states/);
  });

  it('setState changes only what is given and validates ranges', () => {
    const { store, mel } = setup();
    const id = store.doc.layers[0].states[0].id;
    expect(store.dispatch('setState', { layer: mel, id, name: 'Dark', macros: [0, 1, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5], influenceAmount: 0.25 })).toBe(true);
    expect(store.doc.layers[0].states[0]).toMatchObject({ name: 'Dark', influenceAmount: 0.25 });
    expect(store.dispatch('setState', { layer: mel, id, name: 'Dark' })).toBe(false);
    expect(() => store.dispatch('setState', { layer: mel, id, base: ramp(() => 1.2) })).toThrow(RangeError);
    expect(() => store.dispatch('setState', { layer: mel, id, base: [0.5] })).toThrow(RangeError);
    expect(() => store.dispatch('setState', { layer: mel, id, macros: [0.5] })).toThrow(RangeError);
    expect(() => store.dispatch('setState', { layer: mel, id, influenceAmount: -0.1 })).toThrow(RangeError);
    expect(() => store.dispatch('setState', { layer: mel, id: 'nope', name: 'x' })).toThrow(RangeError);
  });

  it('removeState refuses the last state and drops the markers that use it', () => {
    const { store, sec, mel } = setup();
    expect(() => store.dispatch('removeState', { layer: mel, id: store.doc.layers[0].states[0].id })).toThrow(/last state/);
    const a = store.doc.layers[0].states[0].id;
    store.dispatch('addState', { layer: mel, id: 'b' });
    store.dispatch('addState', { layer: mel, id: 'c' });
    for (const [t, state] of [[0, a], [960, 'b'], [1920, 'c'], [2880, 'b']] as const) {
      store.dispatch('addMorphMarker', { section: sec, layer: mel, marker: { t, state } });
    }
    store.dispatch('removeState', { layer: mel, id: 'b' });
    expect(store.doc.layers[0].states.map((s) => s.id)).toEqual([a, 'c']);
    expect((clipOf(store, sec, mel) as MelodyClip).morph.map((m) => m.state)).toEqual([a, 'c']);
    store.undo();
    expect((clipOf(store, sec, mel) as MelodyClip).morph).toHaveLength(4);
    expect(store.doc.layers[0].states).toHaveLength(3);
  });
});

describe('morph markers', () => {
  it('addMorphMarker creates the clip, keeps markers sorted and defaults len and shape', () => {
    const { store, sec, mel, beat } = setup();
    const a = store.doc.layers[0].states[0].id;
    store.dispatch('addState', { layer: mel, id: 'b' });
    store.dispatch('addMorphMarker', { section: sec, layer: mel, marker: { t: 1920, state: 'b', lenMs: 500, shape: 'ease' } });
    store.dispatch('addMorphMarker', { section: sec, layer: mel, marker: { t: 480.4, state: a } });
    const c = clipOf(store, sec, mel) as MelodyClip;
    expect(c.loopLen).toBe(3840 * 2);
    expect(c.morph).toEqual([{ t: 480, state: a, lenMs: 0, shape: 'line' }, { t: 1920, state: 'b', lenMs: 500, shape: 'ease' }]);
    store.dispatch('addMorphMarker', { section: sec, layer: beat, marker: { t: 0, state: store.doc.layers[1].states[0].id } });
    expect(clipOf(store, sec, beat).kind).toBe('beat');
  });

  it('rejects times outside the loop, unknown states, bad lengths and shapes', () => {
    const { store, sec, mel } = setup();
    const a = store.doc.layers[0].states[0].id;
    const add = (marker: Record<string, unknown>) => () => store.dispatch('addMorphMarker', { section: sec, layer: mel, marker: { t: 0, state: a, ...marker } as never });
    expect(add({ t: -1 })).toThrow(RangeError);
    expect(add({ t: 7680 })).toThrow(RangeError);
    expect(add({ t: NaN })).toThrow(RangeError);
    expect(add({ state: 'zzz' })).toThrow(/unknown state/);
    expect(add({ lenMs: -5 })).toThrow(RangeError);
    expect(add({ shape: 'zigzag' })).toThrow(RangeError);
    expect(clipOf(store, sec, mel)).toBeUndefined();
  });

  it('moveMorphMarker changes fields, re-sorts on t, and removeMorphMarker removes', () => {
    const { store, sec, mel } = setup();
    const a = store.doc.layers[0].states[0].id;
    store.dispatch('addState', { layer: mel, id: 'b' });
    store.dispatch('addMorphMarker', { section: sec, layer: mel, marker: { t: 100, state: a } });
    store.dispatch('addMorphMarker', { section: sec, layer: mel, marker: { t: 200, state: 'b' } });
    store.dispatch('moveMorphMarker', { section: sec, layer: mel, index: 0, t: 300, lenMs: 250, shape: 'wobble' });
    expect((clipOf(store, sec, mel) as MelodyClip).morph).toEqual([
      { t: 200, state: 'b', lenMs: 0, shape: 'line' }, { t: 300, state: a, lenMs: 250, shape: 'wobble' }]);
    expect(store.dispatch('moveMorphMarker', { section: sec, layer: mel, index: 0, t: 200 })).toBe(false);
    store.dispatch('moveMorphMarker', { section: sec, layer: mel, index: 0, state: a });
    expect((clipOf(store, sec, mel) as MelodyClip).morph[0].state).toBe(a);
    expect(() => store.dispatch('moveMorphMarker', { section: sec, layer: mel, index: 2, t: 1 })).toThrow(RangeError);
    expect(() => store.dispatch('moveMorphMarker', { section: sec, layer: mel, index: 0, t: 99999 })).toThrow(RangeError);
    expect(() => store.dispatch('moveMorphMarker', { section: sec, layer: mel, index: 0, state: 'q' })).toThrow(RangeError);
    store.dispatch('removeMorphMarker', { section: sec, layer: mel, index: 1 });
    expect((clipOf(store, sec, mel) as MelodyClip).morph).toHaveLength(1);
    expect(() => store.dispatch('removeMorphMarker', { section: sec, layer: mel, index: 1 })).toThrow(RangeError);
    expect(() => store.dispatch('removeMorphMarker', { section: sec, layer: store.doc.layers[1].id, index: 0 })).toThrow(/no clip/);
  });

  it('feel changes convert marker times, and a shorter loop wraps them', () => {
    const { store, sec, mel } = setup();
    const a = store.doc.layers[0].states[0].id;
    store.dispatch('addMorphMarker', { section: sec, layer: mel, marker: { t: 1920, state: a, lenMs: 300 } }); // one beat * 2 at 100 bpm
    store.dispatch('setSection', { id: sec, feel: 'free' });
    const secs = 1920 * (60 / (100 * 960));
    expect((clipOf(store, sec, mel) as MelodyClip).morph[0].t).toBeCloseTo(secs, 9);
    expect((clipOf(store, sec, mel) as MelodyClip).morph[0].lenMs).toBe(300); // milliseconds do not follow the feel
    store.dispatch('setSection', { id: sec, feel: 'grid' });
    expect((clipOf(store, sec, mel) as MelodyClip).morph[0].t).toBe(1920);
    store.dispatch('setClipFields', { section: sec, layer: mel, loopLen: 1000 });
    expect((clipOf(store, sec, mel) as MelodyClip).morph[0].t).toBe(920);
  });

  it('setClip fills a missing morph lane and rejects markers with unknown states', () => {
    const { store, sec, mel } = setup();
    const old = { kind: 'melody', take: '', takes: [], loopLen: 1920, timeSnap: 0, notes: [], warp: [] } as unknown as MelodyClip;
    store.dispatch('setClip', { section: sec, layer: mel, clip: old });
    expect((clipOf(store, sec, mel) as MelodyClip).morph).toEqual([]);
    const bad: MelodyClip = { ...old, morph: [{ t: 0, state: 'ghost', lenMs: 0, shape: 'line' }] };
    expect(() => store.dispatch('setClip', { section: sec, layer: mel, clip: bad })).toThrow(/unknown state/);
  });
});

describe('setKitSlot', () => {
  it('replaces one slot of a beat layer and validates', () => {
    const { store, mel, beat } = setup();
    const params = ramp((i) => i / 100);
    expect(store.dispatch('setKitSlot', { layer: beat, slot: 2, params, cents: 3600 })).toBe(true);
    expect(store.doc.layers[1].kit[2]).toEqual({ params, cents: 3600 });
    expect(store.doc.layers[1].kit[1]).toBe(store.doc.layers[1].kit[1]);
    expect(store.dispatch('setKitSlot', { layer: beat, slot: 2, params, cents: 3600 })).toBe(false);
    expect(() => store.dispatch('setKitSlot', { layer: beat, slot: 4, params, cents: 0 })).toThrow(RangeError);
    expect(() => store.dispatch('setKitSlot', { layer: beat, slot: 0, params: ramp(() => 2), cents: 0 })).toThrow(RangeError);
    expect(() => store.dispatch('setKitSlot', { layer: beat, slot: 0, params, cents: -1 })).toThrow(RangeError);
    expect(() => store.dispatch('setKitSlot', { layer: mel, slot: 0, params, cents: 0 })).toThrow(/not a beat layer/);
    store.undo();
    expect(store.doc.layers[1].kit[2].cents).not.toBe(3600);
  });
});

describe('newProject (M4)', () => {
  it('has one state per layer, empty morph lanes and the current schema', () => {
    const p = newProject('x');
    expect(p.schema).toBe(3);
    expect(p.metro?.bar).toBe(4);
    expect(p.metro?.layerId).toBe(p.layers[1].id);
    for (const l of p.layers) expect(l.states).toHaveLength(1);
    expect(commands.addNote(p, { section: p.sections[0].id, layer: p.layers[0].id, note: note('a', 0) })[0]).toMatchObject({ value: { morph: [] } });
  });
});

describe('metronome and unused takes', () => {
  it('sets the click and undoes it', () => {
    const store = new Store(newProject('x'));
    expect(store.dispatch('setMetro', { on: true, level: 0.4, voice: 'kit', bar: 3, slot: 1 })).toBe(true);
    expect(store.doc.metro).toMatchObject({ on: true, level: 0.4, voice: 'kit', bar: 3, slot: 1 });
    store.undo();
    expect(store.doc.metro?.on).toBe(false);
    expect(() => store.dispatch('setMetro', { bar: 0 })).toThrow(RangeError);
    expect(() => store.dispatch('setMetro', { voice: 'bell' as 'tick' })).toThrow(RangeError);
  });

  it('forgets a take nothing points at, and refuses one a clip uses', () => {
    const store = new Store(newProject('x'));
    const sec = store.doc.sections[0].id;
    const mel = store.doc.layers[0].id;
    const take = { id: 'take_a', audio: 'audio-take_a', frames: 'frames-take_a', rate: 48000, latencyMs: 0, frameCount: 1, kind: 'melody' as const, refCents: 0, root: 0, scale: 0, bpm: null };
    store.dispatch('addTake', { take });
    const clip: MelodyClip = { kind: 'melody', take: 'take_a', takes: ['take_a'], loopLen: 7680, timeSnap: 0, morph: [], notes: [], warp: [] };
    store.dispatch('setClip', { section: sec, layer: mel, clip });
    expect(() => store.dispatch('forgetTakes', { ids: ['take_a'] })).toThrow(/still used/);
    store.dispatch('setClip', { section: sec, layer: mel, clip: null });
    expect(store.doc.takes.take_a).toBeTruthy();
    expect(store.dispatch('forgetTakes', { ids: ['take_a'] })).toBe(true);
    expect(store.doc.takes.take_a).toBeUndefined();
    store.undo();
    expect(store.doc.takes.take_a?.id).toBe('take_a');
  });
});
