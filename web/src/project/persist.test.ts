import { describe, expect, it } from 'vitest';
import { memoryBlobs, memoryKv, ProjectDb, type Kv } from './persist';
import { Store } from './store';
import { newProject } from './commands';
import type { Project } from './types';

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
