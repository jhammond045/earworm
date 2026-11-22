import { describe, expect, it } from 'vitest';
import { memoryBlobs, memoryKv, ProjectDb, type Kv } from './persist';
import { Store } from './store';
import { newProject } from './commands';
import { SCHEMA, type Project } from './types';
import { migrate } from './migrate';
import { vocabFromValues } from '../synth/classic';

/** A schema 4 document written back in the schema 3 shape: no patches, 96-value states and kit slot sets. */
function toV3(p: Project): Record<string, any> {
  const doc = JSON.parse(JSON.stringify(p)) as Record<string, any>;
  doc.schema = 3;
  doc.layers = p.layers.map((l) => {
    const { patch, ...rest } = JSON.parse(JSON.stringify(l)) as Record<string, any>;
    void patch;
    return {
      ...rest,
      states: l.states.map(({ values, ...st }) => ({ ...st, base: vocabFromValues(l.kind, values) })),
      kit: l.kit.map((k, i) => ({ cents: k.cents, params: vocabFromValues(l.kind, l.states[0].values, i + 1) })),
    };
  });
  return doc;
}


// Wires a Store to a ProjectDb the way the app does: every change is appended.
function session(db: ProjectDb, doc: Project, history?: ConstructorParameters<typeof Store>[1]) {
  const store = new Store(doc, history);
  store.subscribe((d, entry) => db.append(d.id, entry, d, store.history.toJSON()));
  return store;
}

const level = (store: Store, v: number) => store.dispatch('setLayer', { id: store.doc.layers[0]!.id, level: v });
const journalKeys = (kv: Kv, id: string) => kv.keys('journal', `${id}/`);

describe('ProjectDb', () => {
  it('create then open returns the document with empty history', async () => {
    const kv = memoryKv();
    const db = new ProjectDb(kv);
    const doc = newProject('Song');
    await db.create(doc);
    const r = await new ProjectDb(kv).open(doc.id);
    expect(r?.doc).toEqual(doc);
    expect(r?.history).toEqual({ undo: [], redo: [] });
    expect(await db.open('nope')).toBeNull();
  });

  it('writes queued entries in one transaction and keys them by padded seq', async () => {
    const kv = memoryKv();
    let calls = 0;
    const putMany = kv.putMany.bind(kv);
    kv.putMany = (rows) => (calls++, putMany(rows));
    const db = new ProjectDb(kv, { flushMs: 10_000 });
    const doc = newProject('Song');
    await db.create(doc);
    calls = 0;
    const store = session(db, doc);
    for (let i = 0; i < 5; i++) level(store, -(i + 1));
    expect(db.pending).toBe(5);
    await db.flush();
    expect(db.pending).toBe(0);
    expect(calls).toBe(1);
    expect(await journalKeys(kv, doc.id)).toEqual([1, 2, 3, 4, 5].map((n) => `${doc.id}/${String(n).padStart(10, '0')}`));
  });

  it('flushes by itself within flushMs', async () => {
    const kv = memoryKv();
    const db = new ProjectDb(kv, { flushMs: 5 });
    const doc = newProject('Song');
    await db.create(doc);
    const store = session(db, doc);
    level(store, -3);
    expect(db.pending).toBe(1);
    await new Promise((r) => setTimeout(r, 40));
    expect(db.pending).toBe(0);
    expect((await journalKeys(kv, doc.id)).length).toBe(1);
  });

  it('keeps the project index row current', async () => {
    const kv = memoryKv();
    const db = new ProjectDb(kv);
    const doc = newProject('Song');
    await db.create(doc);
    const store = session(db, doc);
    store.dispatch('renameProject', { name: 'Renamed' });
    await db.flush();
    expect((await db.list()).map((r) => r.name)).toEqual(['Renamed']);
  });

  it('recovers the state as of the last flush after a crash', async () => {
    const kv = memoryKv();
    const db = new ProjectDb(kv, { flushMs: 10_000, snapshotEvery: 50 });
    const doc = newProject('Song');
    await db.create(doc);
    const store = session(db, doc);
    let atFlush = store.doc;
    for (let i = 0; i < 120; i++) {
      level(store, -(i % 50) - 1);
      if (i === 112) {
        await db.flush();
        atFlush = store.doc;
      }
    }
    expect(db.pending).toBe(7); // never flushed: lost in the crash
    const reopened = await new ProjectDb(kv).open(doc.id);
    expect(reopened?.doc).toEqual(atFlush);
    expect(reopened?.doc).not.toEqual(store.doc);
  });

  it('trims the journal after each snapshot', async () => {
    const kv = memoryKv();
    const db = new ProjectDb(kv, { flushMs: 10_000, snapshotEvery: 50 });
    const doc = newProject('Song');
    await db.create(doc);
    const store = session(db, doc);
    for (let i = 0; i < 120; i++) {
      level(store, -(i % 50) - 1);
      if (i % 7 === 6) await db.flush();
    }
    await db.flush();
    const left = await journalKeys(kv, doc.id);
    // 120 entries, snapshots at the first flush at or past 50 and 100: fewer than 50 rows left.
    expect(left.length).toBeLessThan(50);
    const snap = (await kv.get('projects', `snap/${doc.id}`)) as { seq: number };
    expect(snap.seq).toBeGreaterThanOrEqual(100);
    expect(left.every((k) => Number(k.split('/')[1]) > snap.seq)).toBe(true);
    const reopened = await new ProjectDb(kv).open(doc.id);
    expect(reopened?.doc).toEqual(store.doc);
  });

  it('recovers when the crash came between snapshot and trim', async () => {
    const kv = memoryKv();
    const db = new ProjectDb(kv, { flushMs: 10_000, snapshotEvery: 10 });
    const doc = newProject('Song');
    await db.create(doc);
    const store = session(db, doc);
    const del = kv.del.bind(kv);
    kv.del = async () => {
      throw new Error('crash before trim');
    };
    for (let i = 0; i < 10; i++) level(store, -(i + 1));
    await db.flush().catch(() => {});
    kv.del = del;
    expect((await journalKeys(kv, doc.id)).length).toBe(10); // snapshot written, journal untrimmed
    const reopened = await new ProjectDb(kv).open(doc.id);
    expect(reopened?.doc).toEqual(store.doc); // replayed nothing twice
  });

  it('drops a corrupt trailing row and everything after it', async () => {
    const kv = memoryKv();
    const db = new ProjectDb(kv, { flushMs: 10_000 });
    const doc = newProject('Song');
    await db.create(doc);
    const store = session(db, doc);
    const states = [store.doc];
    for (let i = 0; i < 6; i++) {
      level(store, -(i + 1));
      states.push(store.doc);
    }
    await db.flush();
    const k = (n: number) => `${doc.id}/${String(n).padStart(10, '0')}`;
    await kv.put('journal', k(5), { kind: 'do', group: { name: 'half' } }); // missing ops
    const reopened = await new ProjectDb(kv).open(doc.id);
    expect(reopened?.doc).toEqual(states[4]);
    expect(await journalKeys(kv, doc.id)).toEqual([1, 2, 3, 4].map(k));

    await kv.put('journal', k(3), 'garbage');
    const again = await new ProjectDb(kv).open(doc.id);
    expect(again?.doc).toEqual(states[2]);
  });

  it('keeps the undo and redo stacks across a reopen', async () => {
    const kv = memoryKv();
    const db = new ProjectDb(kv);
    const doc = newProject('Song');
    await db.create(doc);
    const store = session(db, doc);
    const first = store.doc.layers[0]!.level;
    level(store, -10);
    level(store, -20);
    level(store, -30);
    store.undo();
    await db.flush();

    const r = await new ProjectDb(kv).open(doc.id);
    expect(r?.history.undo.length).toBe(2);
    expect(r?.history.redo.length).toBe(1);
    expect(r?.doc.layers[0]!.level).toBe(-20);

    // The restored session can keep undoing and redoing, and keep saving.
    const db2 = new ProjectDb(kv);
    const again = session(db2, r!.doc, r!.history);
    expect(again.redo()).toBe(true);
    expect(again.doc.layers[0]!.level).toBe(-30);
    again.undo();
    again.undo();
    again.undo();
    expect(again.doc.layers[0]!.level).toBe(first);
    await db2.flush();
    const last = await new ProjectDb(kv).open(doc.id);
    expect(last?.doc.layers[0]!.level).toBe(first);
    expect(last?.history.undo.length).toBe(0);
    expect(last?.history.redo.length).toBe(3);
  });

  it('continues numbering after a reopen without overwriting rows', async () => {
    const kv = memoryKv();
    const db = new ProjectDb(kv);
    const doc = newProject('Song');
    await db.create(doc);
    const s1 = session(db, doc);
    level(s1, -1);
    level(s1, -2);
    await db.flush();
    const r = (await new ProjectDb(kv).open(doc.id))!;
    const db2 = new ProjectDb(kv);
    await db2.open(doc.id);
    const s2 = session(db2, r.doc, r.history);
    level(s2, -3);
    await db2.flush();
    expect((await journalKeys(kv, doc.id)).length).toBe(3);
    expect((await new ProjectDb(kv).open(doc.id))?.doc).toEqual(s2.doc);
  });

  it('removes a project and lists projects newest first', async () => {
    const kv = memoryKv();
    const db = new ProjectDb(kv);
    const a = newProject('A');
    const b = newProject('B');
    await db.create(a);
    await db.create(b);
    const store = session(db, a);
    level(store, -4);
    await db.flush();
    expect((await db.list()).map((r) => r.id)).toEqual([a.id, b.id]);
    await db.remove(a.id);
    expect((await db.list()).map((r) => r.id)).toEqual([b.id]);
    expect(await db.open(a.id)).toBeNull();
    expect(await journalKeys(kv, a.id)).toEqual([]);
  });
});

// What a schema 1 app left behind: a snapshot and journal with no morph lanes anywhere.
function asSchema1<T>(v: T): T {
  const strip = (x: unknown): unknown => {
    if (Array.isArray(x)) return x.map(strip);
    if (x && typeof x === 'object') {
      return Object.fromEntries(Object.entries(x).filter(([k]) => k !== 'morph').map(([k, y]) => [k, strip(y)]));
    }
    return x;
  };
  return strip(JSON.parse(JSON.stringify(v))) as T;
}

describe('schema migration on open', () => {
  it('a schema 3 snapshot replays its journal before the 3 -> 4 step and reports the history reset', async () => {
    const kv = memoryKv();
    const db = new ProjectDb(kv, { flushMs: 10_000, snapshotEvery: 1000 });
    const start = newProject('Three');
    await db.create(start);
    const store = session(db, start);
    const mel = store.doc.layers[0]!.id;
    const sec = store.doc.sections[0]!.id;
    store.dispatch('setLayer', { id: mel, level: -4 });
    store.dispatch('addNote', { section: sec, layer: mel, note: { id: 'n1', t: 0, dur: 480, cents: 5000, hard: 0.5 } });
    const final = store.doc;
    await db.flush();
    const snap = (await kv.get('projects', `snap/${start.id}`)) as { doc: Project; history: unknown; seq: number };
    await kv.put('projects', `snap/${start.id}`, { ...snap, doc: toV3(snap.doc) });
    expect((await journalKeys(kv, start.id)).length).toBeGreaterThan(0);

    const opened = await new ProjectDb(kv).open(start.id);
    expect(opened!.historyReset).toBe(true);
    expect(opened!.history).toEqual({ undo: [], redo: [] });
    expect(opened!.doc.schema).toBe(5);
    expect(opened!.doc.layers[0].level).toBe(-4);
    const clip = opened!.doc.sections[0].clips[mel] as { notes: { id: string; vel?: number }[] };
    expect(clip.notes.map((n) => [n.id, n.vel])).toEqual([['n1', 1]]);
    expect(opened!.doc.layers.map((l) => l.states[0].values)).toEqual(final.layers.map((l) => l.states[0].values));
  });

  it('upgrades an old snapshot and replays old journal entries into the new shape', async () => {
    const kv = memoryKv();
    const db = new ProjectDb(kv, { flushMs: 10_000, snapshotEvery: 1000 });
    const start = newProject('Old');
    await db.create(start);
    const store = session(db, start);
    const mel = store.doc.layers[0]!.id;
    const sec = store.doc.sections[0]!.id;
    store.dispatch('addNote', { section: sec, layer: mel, note: { id: 'n1', t: 0, dur: 480, cents: 5000, hard: 0.5 } }); // creates a clip
    store.dispatch('addNote', { section: sec, layer: mel, note: { id: 'n2', t: 960, dur: 480, cents: 5200, hard: 0.5 } });
    store.dispatch('duplicateSection', { id: sec }); // an op carrying a whole section
    store.dispatch('setClip', { section: sec, layer: mel, clip: null }); // its inverse carries a whole clip
    store.undo();
    const expectedDoc = store.doc;
    store.redo(); // the doc the redo stack leads to
    const redone = store.doc;
    store.undo();
    await db.flush();

    // Rewrite storage as the old app would have written it.
    const snap = (await kv.get('projects', `snap/${start.id}`)) as { doc: Project; history: unknown; seq: number };
    await kv.put('projects', `snap/${start.id}`, { ...snap, doc: { ...asSchema1(toV3(snap.doc)), schema: 1 } });
    for (const k of await journalKeys(kv, start.id)) await kv.put('journal', k, asSchema1(await kv.get('journal', k)));
    const stored = JSON.stringify(await kv.get('journal', (await journalKeys(kv, start.id))[0]!));
    expect(stored).not.toContain('morph');

    const opened = await new ProjectDb(kv).open(start.id);
    expect(opened!.doc.schema).toBe(SCHEMA);
    // The journal replays in the old shape, then the document moves to schema 4 (notes get velocity 1).
    expect(opened!.doc).toEqual(migrate(toV3(expectedDoc)));
    for (const sc of opened!.doc.sections) for (const c of Object.values(sc.clips)) expect(c.morph).toEqual([]);
    expect(opened!.doc.layers[0].states[0].values).toEqual(expectedDoc.layers[0].states[0].values);

    // Undo steps from before schema 4 cannot come along: the history starts fresh, and says so.
    expect(opened!.historyReset).toBe(true);
    expect(opened!.history).toEqual({ undo: [], redo: [] });
    expect(redone).not.toEqual(expectedDoc);
    // The upgraded snapshot is written back: opening again is plain.
    const twice = await new ProjectDb(kv).open(start.id);
    expect(twice!.historyReset).toBe(false);
    expect(twice!.doc).toEqual(opened!.doc);
    const again = new Store(twice!.doc, twice!.history);
    again.dispatch('addNote', { section: sec, layer: mel, note: { id: 'n3', t: 1920, dur: 480, cents: 5400, hard: 0.5 } });
    expect(again.undo()).toBe(true);
    expect(again.doc).toEqual(opened!.doc);
  });

  it('writes the upgraded snapshot back and drops the old journal, so new entries do not mix shapes', async () => {
    const kv = memoryKv();
    const db = new ProjectDb(kv, { flushMs: 10_000 });
    const start = newProject('Old');
    await db.create(start);
    const store = session(db, start);
    level(store, -4);
    await db.flush();
    const snap = (await kv.get('projects', `snap/${start.id}`)) as { doc: Project };
    await kv.put('projects', `snap/${start.id}`, { ...snap, doc: { ...asSchema1(snap.doc), schema: 1 } });

    const db2 = new ProjectDb(kv, { flushMs: 10_000 });
    const opened = await db2.open(start.id);
    expect(await journalKeys(kv, start.id)).toEqual([]);
    expect(((await kv.get('projects', `snap/${start.id}`)) as { doc: Project; seq: number }).doc.schema).toBe(SCHEMA);
    const s2 = session(db2, opened!.doc, opened!.history);
    level(s2, -8);
    await db2.flush();
    expect(await journalKeys(kv, start.id)).toHaveLength(1);
    const reopened = await new ProjectDb(kv).open(start.id);
    expect(reopened!.doc.layers[0]!.level).toBe(-8);
    expect(reopened!.doc.schema).toBe(SCHEMA);
  });

  it('refuses a project saved by a newer app', async () => {
    const kv = memoryKv();
    const db = new ProjectDb(kv);
    const doc = { ...newProject('Future'), schema: 99 } as Project;
    await db.create(doc);
    await expect(new ProjectDb(kv).open(doc.id)).rejects.toThrow(/newer version/);
  });
});

describe('memory stores', () => {
  it('memoryKv filters keys by prefix and clones values', async () => {
    const kv = memoryKv();
    const v = { a: [1] };
    await kv.put('settings', 'x/1', v);
    await kv.put('settings', 'x/2', 2);
    await kv.put('settings', 'y/1', 3);
    v.a.push(2);
    expect(await kv.get('settings', 'x/1')).toEqual({ a: [1] });
    expect(await kv.keys('settings', 'x/')).toEqual(['x/1', 'x/2']);
    expect(await kv.get('settings', 'none')).toBeUndefined();
  });

  it('memoryBlobs copies data in and out', async () => {
    const b = memoryBlobs();
    const src = new Float32Array([1, 2, 3]);
    await b.put('audio-1', src);
    src[0] = 9;
    const got = await b.get('audio-1');
    expect([...got!]).toEqual([1, 2, 3]);
    expect(await b.get('missing')).toBeNull();
    expect(await b.list()).toEqual(['audio-1']);
    await b.del('audio-1');
    expect(await b.list()).toEqual([]);
  });
});
