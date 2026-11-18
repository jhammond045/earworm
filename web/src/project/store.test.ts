import { describe, expect, it } from 'vitest';
import { newProject } from './commands';
import type { CommandArgs, CommandName } from './commands';
import type { JournalEntry } from './history';
import { Store } from './store';
import type { Clip, Hit, MelodyClip, Project, Take } from './types';

const shape = { attack: 0, burst: 0, cents: null, bend: 0, hold: 0, vowel: 0, tone: 0, endStop: 0, endClick: 0 };

describe('Store basics', () => {
  it('dispatch records a group; undo and redo restore the document', () => {
    const s = new Store(newProject('T'));
    const start = s.doc;
    expect(s.dispatch('renameProject', { name: 'A' })).toBe(true);
    expect(s.dispatch('renameProject', { name: 'A' })).toBe(false);
    expect(s.history.toJSON().undo).toHaveLength(1);
    expect(s.undo()).toBe(true);
    expect(s.doc).toStrictEqual(start);
    expect(s.history.canRedo).toBe(true);
    expect(s.redo()).toBe(true);
    expect(s.doc.name).toBe('A');
    expect(s.redo()).toBe(false);
    s.undo();
    s.dispatch('renameProject', { name: 'B' });
    expect(s.history.canRedo).toBe(false);
  });

  it('batch is one undo group, and a throw inside changes nothing', () => {
    const s = new Store(newProject('T'));
    const start = s.doc;
    const sec = start.sections[0].id;
    const mel = start.layers[0].id;
    s.batch('drag', (run) => {
      run('renameProject', { name: 'X' });
      run('addNote', { section: sec, layer: mel, note: { id: 'n', t: 0, dur: 100, cents: 100, hard: 0 } });
      run('moveNotes', { section: sec, layer: mel, ids: ['n'], dt: 480, dcents: 0 });
    });
    expect(s.history.toJSON().undo).toHaveLength(1);
    expect(((s.doc.sections[0].clips[mel]) as MelodyClip).notes[0].t).toBe(480);
    s.undo();
    expect(s.doc).toStrictEqual(start);
    s.redo();
    const after = s.doc;
    expect(() => s.batch('bad', (run) => {
      run('renameProject', { name: 'Y' });
      run('removeLayer', { id: 'nope' });
    })).toThrow(RangeError);
    expect(s.doc).toBe(after);
    expect(s.history.toJSON().undo).toHaveLength(1);
    expect(s.batch('nothing', () => {})).toBe(false);
  });

  it('subscribers get the doc and entry; unsubscribe works', () => {
    const s = new Store(newProject('T'));
    const seen: JournalEntry['kind'][] = [];
    const off = s.subscribe((doc, e) => { expect(doc).toBe(s.doc); seen.push(e.kind); });
    s.dispatch('renameProject', { name: 'A' });
    s.undo();
    s.redo();
    off();
    s.undo();
    expect(seen).toEqual(['do', 'undo', 'redo']);
  });

  it('keeps at most 500 groups', () => {
    const s = new Store(newProject('T'));
    for (let i = 0; i < 520; i++) s.dispatch('renameProject', { name: `n${i}` });
    expect(s.history.toJSON().undo).toHaveLength(500);
  });
});

// ---- seeded random commands ----

function rng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function randomAction(s: Store, r: () => number, n: number): void {
  const doc: Project = s.doc;
  const pick = <T>(a: T[]): T => a[Math.floor(r() * a.length)];
  const secs = doc.sections;
  const sec = pick(secs);
  const layer = pick(doc.layers);
  const clip: Clip | undefined = layer ? sec.clips[layer.id] : undefined;
  const d = <K extends CommandName>(name: K, args: CommandArgs<K>) => s.dispatch(name, args);
  const grid = sec.feel === 'grid';
  const len = clip ? clip.loopLen : grid ? sec.beats * 960 : sec.seconds;
  const time = () => (grid ? Math.floor(r() * len) : r() * len);
  const items = clip ? (clip.kind === 'beat' ? clip.hits : clip.notes) : [];
  const some = () => items.filter(() => r() < 0.5).map((x) => x.id);
  const k = Math.floor(r() * 24);
  switch (k) {
    case 0: d('renameProject', { name: `p${n}` }); break;
    case 1: if (secs.length < 6) d('addSection', { after: r() < 0.5 ? sec.id : undefined, feel: r() < 0.5 ? 'grid' : 'free', beats: 4 + Math.floor(r() * 8), seconds: 2 + r() * 6, bpm: 60 + Math.floor(r() * 100) }); break;
    case 2: if (secs.length > 1) d('removeSection', { id: sec.id }); break;
    case 3: d('moveSection', { id: sec.id, to: Math.floor(r() * secs.length) }); break;
    case 4: d('setSection', { id: sec.id, feel: sec.feel === 'grid' ? 'free' : 'grid', ...(r() < 0.5 ? { bpm: 70 + Math.floor(r() * 80) } : {}) }); break;
    case 5: d('setSection', { id: sec.id, bpm: 60 + Math.floor(r() * 100), repeats: 1 + Math.floor(r() * 3), name: `s${n}` }); break;
    case 6: if (secs.length < 6) d('duplicateSection', { id: sec.id }); break;
    case 7: if (doc.layers.length < 8) d('addLayer', { kind: pick(['melody', 'beat', 'texture'] as const), sound: Math.floor(r() * 2) }); break;
    case 8: if (doc.layers.length > 1) d('removeLayer', { id: layer.id }); break;
    case 9: d('setLayer', { id: layer.id, level: -30 + r() * 40, pan: r() * 2 - 1, mute: r() < 0.5, tune: { snap: r(), octave: Math.floor(r() * 3) }, depth: { loud: r() } }); break;
    case 10: d('setLayerSound', { id: layer.id, base: Array.from({ length: 96 }, () => r()) }); break;
    case 11: {
      const id = `take${n}`;
      const t: Take = { id, audio: `${id}.a`, frames: `${id}.f`, rate: 16000, latencyMs: 20, frameCount: 50, kind: 'melody', refCents: 0, root: 0, scale: 0, bpm: null };
      s.batch('place take', (run) => {
        run('addTake', { take: t });
        const ln = 960 * (1 + Math.floor(r() * 4));
        run('setClip', { section: sec.id, layer: layer.id, clip: layer.kind === 'beat'
          ? { kind: 'beat', take: id, takes: [id], loopLen: ln, timeSnap: 0, hits: [{ id: `h${n}`, t: 5 * ln, dur: 100, slot: 1, strength: 0.5, shape } as Hit] }
          : { kind: 'melody', take: id, takes: [id], loopLen: ln, timeSnap: 0, notes: [{ id: `n${n}`, t: 5 * ln + 7, dur: 0, cents: 5000, hard: 0.5 }], warp: [] } });
      });
      break;
    }
    case 12: if (clip) d('setClip', { section: sec.id, layer: layer.id, clip: null }); break;
    case 13: if (clip) d('setClipFields', { section: sec.id, layer: layer.id, loopLen: Math.max(1, Math.floor(len / 2)), timeSnap: grid ? 240 : 0.25 }); break;
    case 14: case 15:
      if (layer.kind === 'beat') d('addHit', { section: sec.id, layer: layer.id, hit: { id: `h${n}`, t: time(), dur: 100, slot: Math.floor(r() * 4), strength: r(), shape } });
      else d('addNote', { section: sec.id, layer: layer.id, note: { id: `n${n}`, t: time(), dur: grid ? 480 : 0.3, cents: Math.floor(r() * 9000), hard: r() } });
      break;
    case 16: if (clip && items.length) {
      if (clip.kind === 'beat') d('moveHits', { section: sec.id, layer: layer.id, ids: some(), dt: (r() - 0.5) * len * 2 });
      else d('moveNotes', { section: sec.id, layer: layer.id, ids: some(), dt: (r() - 0.5) * len * 2, dcents: (r() - 0.5) * 5000 });
    } break;
    case 17: if (clip && clip.kind !== 'beat' && clip.notes.length) d('resizeNote', { section: sec.id, layer: layer.id, id: pick(clip.notes).id, dur: grid ? 100 + r() * 900 : r() }); break;
    case 18: if (clip && items.length) {
      if (clip.kind === 'beat') d('deleteHits', { section: sec.id, layer: layer.id, ids: some() });
      else d('deleteNotes', { section: sec.id, layer: layer.id, ids: some() });
    } break;
    case 19: if (clip && clip.kind === 'beat' && clip.hits.length) d('setHitSlot', { section: sec.id, layer: layer.id, ids: some(), slot: Math.floor(r() * 4) }); break;
    case 20: if (clip) d('quantize', { section: sec.id, layer: layer.id, grid: grid ? 240 : 0.25 }); break;
    case 21: if (clip && clip.takes.length) d('selectTake', { section: sec.id, layer: layer.id, take: pick(clip.takes) }); break;
    case 22: s.batch('two', (run) => { run('renameProject', { name: `b${n}` }); run('setSection', { id: sec.id, name: `c${n}` }); }); break;
    default: d('setLayer', { id: layer.id, name: `L${n}`, color: Math.floor(r() * 0xffffff) });
  }
}

function run500(seed: number): { start: Project; s: Store; entries: JournalEntry[]; dispatched: number } {
  const start = newProject('Random');
  const s = new Store(start);
  const entries: JournalEntry[] = [];
  s.subscribe((_d, e) => entries.push(e));
  const r = rng(seed);
  for (let i = 0; i < 500; i++) randomAction(s, r, i);
  return { start, s, entries, dispatched: 500 };
}

describe('random commands', () => {
  it('undo all restores the start, redo all restores the end, and history survives JSON', () => {
    const { start, s } = run500(1234);
    const end = s.doc;
    const groups = s.history.toJSON().undo.length;
    expect(groups).toBeGreaterThan(300);
    expect(end).not.toEqual(start);
    // Every layer/section invariant the commands promise.
    for (const sec of end.sections) {
      for (const clip of Object.values(sec.clips)) {
        const items = clip.kind === 'beat' ? clip.hits : clip.notes;
        for (const it of items) {
          expect(it.t).toBeGreaterThanOrEqual(0);
          expect(it.t).toBeLessThan(clip.loopLen);
          expect(it.dur).toBeGreaterThan(0);
        }
      }
    }
    expect(end.layers.length).toBeLessThanOrEqual(8);

    const saved = JSON.stringify({ doc: end, history: s.history.toJSON() });
    const { doc: doc2, history } = JSON.parse(saved) as { doc: Project; history: ReturnType<Store['history']['toJSON']> };
    const s2 = new Store(doc2, history);

    for (const store of [s, s2]) {
      let n = 0;
      while (store.undo()) n++;
      expect(n).toBe(groups);
      expect(JSON.parse(JSON.stringify(store.doc))).toStrictEqual(JSON.parse(JSON.stringify(start)));
      expect(store.doc).toStrictEqual(start);
      n = 0;
      while (store.redo()) n++;
      expect(n).toBe(groups);
      expect(store.doc).toStrictEqual(end);
    }
  });

  it('replaying the journal into a fresh store reproduces doc and history', () => {
    const { start, s, entries } = run500(99);
    // Mix in undo and redo so all three entry kinds are replayed.
    for (let i = 0; i < 40; i++) s.undo();
    for (let i = 0; i < 15; i++) s.redo();
    const r = rng(5);
    for (let i = 0; i < 20; i++) randomAction(s, r, 1000 + i);
    for (let i = 0; i < 5; i++) s.undo();
    expect(new Set(entries.map((e) => e.kind))).toEqual(new Set(['do', 'undo', 'redo']));

    // Journal entries cross storage as JSON.
    const journal = JSON.parse(JSON.stringify(entries)) as JournalEntry[];
    const fresh = new Store(start);
    for (const e of journal) fresh.replay(e);
    expect(JSON.stringify(fresh.doc)).toBe(JSON.stringify(s.doc));
    expect(JSON.stringify(fresh.history.toJSON())).toBe(JSON.stringify(s.history.toJSON()));
  });

  it('replay does not call subscribers', () => {
    const { start, entries } = run500(7);
    const fresh = new Store(start);
    let called = 0;
    fresh.subscribe(() => { called++; });
    for (const e of entries.slice(0, 20)) fresh.replay(e);
    expect(called).toBe(0);
  });
});
