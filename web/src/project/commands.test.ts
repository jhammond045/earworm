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
    const clip: MelodyClip = { kind: 'melody', take: 't1', takes: ['t1'], loopLen: 1920, timeSnap: 0, notes: [note('a', 4000)], warp: [] };
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
    const clip: MelodyClip = { kind: 'melody', take: 't2', takes: ['t1', 't2'], loopLen: 1920, timeSnap: 0, notes: [], warp: [] };
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
