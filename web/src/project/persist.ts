// Local storage for projects (TDD "Persistence"): the document snapshot and journal in
// IndexedDB, take audio and frames in the origin private file system.

import type { BlobRef, Project } from './types';
import type { HistoryState, JournalEntry } from './history';
import { Store } from './store';
import type { StorageReply, StorageRequest } from './storage-worker';

// ---- key/value store ----

export interface Kv {
  get(store: string, key: string): Promise<unknown>;
  put(store: string, key: string, v: unknown): Promise<void>;
  del(store: string, key: string): Promise<void>;
  keys(store: string, prefix?: string): Promise<string[]>;
  /** Atomically write several records (one IndexedDB transaction). */
  putMany(rows: { store: string; key: string; value: unknown }[]): Promise<void>;
}

const STORES = ['projects', 'journal', 'settings'] as const;

function wrap<T>(req: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

function done(tx: IDBTransaction): Promise<void> {
  return new Promise((resolve, reject) => {
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error);
    tx.onabort = () => reject(tx.error ?? new Error('transaction aborted'));
  });
}

export async function idbKv(dbName = 'earworm'): Promise<Kv> {
  const db = await new Promise<IDBDatabase>((resolve, reject) => {
    const open = indexedDB.open(dbName, 1);
    open.onupgradeneeded = () => {
      for (const s of STORES) if (!open.result.objectStoreNames.contains(s)) open.result.createObjectStore(s);
    };
    open.onsuccess = () => resolve(open.result);
    open.onerror = () => reject(open.error);
  });
  // Another tab upgrading the schema must not be blocked by this connection.
  db.onversionchange = () => db.close();
  return {
    async get(store, key) {
      return wrap(db.transaction(store).objectStore(store).get(key));
    },
    async put(store, key, v) {
      const tx = db.transaction(store, 'readwrite');
      tx.objectStore(store).put(v, key);
      await done(tx);
    },
    async del(store, key) {
      const tx = db.transaction(store, 'readwrite');
      tx.objectStore(store).delete(key);
      await done(tx);
    },
    async keys(store, prefix = '') {
      const range = prefix ? IDBKeyRange.bound(prefix, prefix + '￿') : undefined;
      return (await wrap(db.transaction(store).objectStore(store).getAllKeys(range))) as string[];
    },
    async putMany(rows) {
      if (!rows.length) return;
      const names = [...new Set(rows.map((r) => r.store))];
      const tx = db.transaction(names, 'readwrite');
      for (const r of rows) tx.objectStore(r.store).put(r.value, r.key);
      await done(tx);
    },
  };
}

/** In-memory Kv for tests and for browsers without IndexedDB. Values are cloned like IndexedDB does. */
export function memoryKv(): Kv {
  const stores = new Map<string, Map<string, unknown>>();
  const of = (s: string) => {
    let m = stores.get(s);
    if (!m) stores.set(s, (m = new Map()));
    return m;
  };
  return {
    async get(store, key) {
      const v = of(store).get(key);
      return v === undefined ? undefined : structuredClone(v);
    },
    async put(store, key, v) {
      of(store).set(key, structuredClone(v));
    },
    async del(store, key) {
      of(store).delete(key);
    },
    async keys(store, prefix = '') {
      return [...of(store).keys()].filter((k) => k.startsWith(prefix)).sort();
    },
    async putMany(rows) {
      // Clone everything first so a failure leaves nothing half written.
      const cloned = rows.map((r) => ({ ...r, value: structuredClone(r.value) }));
      for (const r of cloned) of(r.store).set(r.key, r.value);
    },
  };
}

// ---- blobs ----

export interface BlobStore {
  put(ref: BlobRef, data: Float32Array): Promise<void>;
  get(ref: BlobRef): Promise<Float32Array | null>;
  del(ref: BlobRef): Promise<void>;
  list(): Promise<BlobRef[]>;
}

export function memoryBlobs(): BlobStore {
  const m = new Map<BlobRef, Float32Array>();
  return {
    async put(ref, data) {
      m.set(ref, data.slice());
    },
    async get(ref) {
      const v = m.get(ref);
      return v ? v.slice() : null;
    },
    async del(ref) {
      m.delete(ref);
    },
    async list() {
      return [...m.keys()];
    },
  };
}

// One worker serves every project; requests are matched to replies by id.
let worker: Worker | null = null;
let nextReq = 1;
const waiting = new Map<number, { resolve: (r: StorageReply & { ok: true }) => void; reject: (e: Error) => void }>();

function storageWorker(): Worker {
  if (worker) return worker;
  const w = new Worker(new URL('./storage-worker.ts', import.meta.url), { type: 'module' });
  w.onmessage = (e: MessageEvent<StorageReply>) => {
    const p = waiting.get(e.data.id);
    if (!p) return;
    waiting.delete(e.data.id);
    if (e.data.ok) p.resolve(e.data);
    else p.reject(new Error(e.data.error));
  };
  w.onerror = (e) => {
    // A dead worker fails everything in flight; the next call starts a fresh one.
    const err = new Error(e.message || 'storage worker failed');
    for (const p of waiting.values()) p.reject(err);
    waiting.clear();
    w.terminate();
    if (worker === w) worker = null;
  };
  worker = w;
  return w;
}

type Without<T> = T extends unknown ? Omit<T, 'id'> : never;

function call(req: Without<StorageRequest>, transfer: Transferable[] = []): Promise<StorageReply & { ok: true }> {
  const id = nextReq++;
  return new Promise((resolve, reject) => {
    waiting.set(id, { resolve, reject });
    storageWorker().postMessage({ ...req, id }, transfer);
  });
}

export function opfsBlobs(projectId: string): BlobStore {
  return {
    async put(ref, data) {
      // Copy, so the caller keeps its array (transferring would detach it).
      const copy = data.slice().buffer;
      await call({ op: 'put', project: projectId, ref, data: copy }, [copy]);
    },
    async get(ref) {
      const r = await call({ op: 'get', project: projectId, ref });
      return r.data ? new Float32Array(r.data) : null;
    },
    async del(ref) {
      await call({ op: 'del', project: projectId, ref });
    },
    async list() {
      return (await call({ op: 'list', project: projectId })).refs ?? [];
    },
  };
}

// ---- project database ----

interface Snapshot { doc: Project; history: HistoryState; seq: number }
export interface ProjectRow { id: string; name: string; updated: number }

let lastStamp = 0;
const SNAP = (id: string) => `snap/${id}`;
const INDEX = (id: string) => `index/${id}`;
const jkey = (id: string, seq: number) => `${id}/${String(seq).padStart(10, '0')}`;
const jseq = (key: string) => Number(key.slice(key.lastIndexOf('/') + 1));

/** A complete journal entry. Anything else is a partly written trailing group. */
function validEntry(v: unknown): v is JournalEntry {
  if (!v || typeof v !== 'object') return false;
  const e = v as Partial<JournalEntry>;
  if (e.kind !== 'do' && e.kind !== 'undo' && e.kind !== 'redo') return false;
  const g = e.group;
  return !!g && typeof g === 'object' && Array.isArray(g.ops) && Array.isArray(g.inverse) && typeof g.name === 'string';
}

interface Tracked {
  queue: JournalEntry[];
  doc: Project;
  history: HistoryState;
  /** Highest journal seq written; null until read from the kv. */
  seq: number | null;
  /** Journal entries written since the last snapshot. */
  since: number;
  snapSeq: number;
}

export class ProjectDb {
  private readonly flushMs: number;
  private readonly snapshotEvery: number;
  private readonly projects = new Map<string, Tracked>();
  private timer: ReturnType<typeof setTimeout> | null = null;
  private chain: Promise<void> = Promise.resolve();

  constructor(private readonly kv: Kv, opts: { flushMs?: number; snapshotEvery?: number } = {}) {
    this.flushMs = opts.flushMs ?? 300;
    this.snapshotEvery = opts.snapshotEvery ?? 50;
  }

  /** Queued entries not yet written. */
  get pending(): number {
    let n = 0;
    for (const p of this.projects.values()) n += p.queue.length;
    return n;
  }

  async create(doc: Project): Promise<void> {
    const snap: Snapshot = { doc, history: { undo: [], redo: [] }, seq: 0 };
    await this.kv.putMany([
      { store: 'projects', key: SNAP(doc.id), value: snap },
      { store: 'projects', key: INDEX(doc.id), value: this.row(doc) },
    ]);
    this.projects.set(doc.id, { queue: [], doc, history: snap.history, seq: 0, since: 0, snapSeq: 0 });
  }

  async open(id: string): Promise<{ doc: Project; history: HistoryState } | null> {
    await this.flush(); // our own queued writes must be visible to our own read
    const snap = (await this.kv.get('projects', SNAP(id))) as Snapshot | undefined;
    if (!snap || !snap.doc) return null;
    const keys = (await this.kv.keys('journal', `${id}/`)).filter((k) => jseq(k) > snap.seq);
    const store = new Store(snap.doc, snap.history);
    let seq = snap.seq;
    let n = 0;
    for (let i = 0; i < keys.length; i++) {
      const v = await this.kv.get('journal', keys[i]!);
      if (!validEntry(v)) {
        // Trailing partial group: drop it and everything after it, so new
        // entries do not land beside stale rows.
        for (const k of keys.slice(i)) await this.kv.del('journal', k);
        break;
      }
      store.replay(v);
      seq = jseq(keys[i]!);
      n++;
    }
    const history = store.history.toJSON();
    this.projects.set(id, { queue: [], doc: store.doc, history, seq, since: n, snapSeq: snap.seq });
    return { doc: store.doc, history };
  }

  append(id: string, entry: JournalEntry, doc: Project, history: HistoryState): void {
    let p = this.projects.get(id);
    if (!p) {
      p = { queue: [], doc, history, seq: null, since: 0, snapSeq: 0 };
      this.projects.set(id, p);
    }
    p.queue.push(entry);
    p.doc = doc;
    p.history = history;
    if (!this.timer) {
      this.timer = setTimeout(() => {
        this.timer = null;
        // A failed write keeps the entries queued; the next flush retries.
        this.flush().catch(() => {});
      }, this.flushMs);
    }
  }

  /** Resolves when everything queued so far is written. */
  flush(): Promise<void> {
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    const run = this.chain.then(() => this.writeAll());
    this.chain = run.catch(() => {});
    return run;
  }

  async list(): Promise<ProjectRow[]> {
    const rows: ProjectRow[] = [];
    for (const k of await this.kv.keys('projects', 'index/')) {
      const r = (await this.kv.get('projects', k)) as ProjectRow | undefined;
      if (r) rows.push(r);
    }
    return rows.sort((a, b) => b.updated - a.updated);
  }

  async remove(id: string): Promise<void> {
    const p = this.projects.get(id);
    if (p) p.queue = [];
    this.projects.delete(id);
    await this.chain; // let an in-flight write finish before deleting under it
    await this.kv.del('projects', SNAP(id));
    await this.kv.del('projects', INDEX(id));
    for (const k of await this.kv.keys('journal', `${id}/`)) await this.kv.del('journal', k);
  }

  private row(doc: Project): ProjectRow {
    // Strictly increasing, so two saves in the same millisecond still list in save order.
    lastStamp = Math.max(Date.now(), lastStamp + 1);
    return { id: doc.id, name: doc.name, updated: lastStamp };
  }

  private async writeAll(): Promise<void> {
    for (const [id, p] of [...this.projects]) {
      if (!p.queue.length) continue;
      await this.writeProject(id, p);
    }
  }

  private async writeProject(id: string, p: Tracked): Promise<void> {
    if (p.seq === null) {
      // Appended without open() or create(): continue after whatever is stored.
      const snap = (await this.kv.get('projects', SNAP(id))) as Snapshot | undefined;
      const keys = await this.kv.keys('journal', `${id}/`);
      p.snapSeq = snap?.seq ?? 0;
      p.seq = keys.length ? Math.max(p.snapSeq, jseq(keys[keys.length - 1]!)) : p.snapSeq;
      p.since = keys.filter((k) => jseq(k) > p.snapSeq).length;
    }
    const batch = p.queue;
    const doc = p.doc;
    const history = p.history;
    p.queue = [];
    let seq = p.seq;
    const rows: { store: string; key: string; value: unknown }[] = [];
    for (const entry of batch) rows.push({ store: 'journal', key: jkey(id, ++seq), value: entry });
    rows.push({ store: 'projects', key: INDEX(id), value: this.row(doc) });
    const since = p.since + batch.length;
    const snapshot = since >= this.snapshotEvery;
    if (snapshot) rows.push({ store: 'projects', key: SNAP(id), value: { doc, history, seq } satisfies Snapshot });
    try {
      await this.kv.putMany(rows);
    } catch (e) {
      p.queue = [...batch, ...p.queue];
      throw e;
    }
    p.seq = seq;
    if (snapshot) {
      p.since = 0;
      p.snapSeq = seq;
      // Trim after the snapshot is durable. A crash in between leaves rows that
      // open() skips because their seq is not above the snapshot's.
      for (const k of await this.kv.keys('journal', `${id}/`)) {
        if (jseq(k) <= seq) await this.kv.del('journal', k);
      }
    } else {
      p.since = since;
    }
  }
}

/** Flush when the tab is hidden or closing: the last chance to write before a page may be discarded. */
export function flushOnHide(db: ProjectDb): () => void {
  if (typeof document === 'undefined') return () => {};
  const onVis = () => {
    if (document.visibilityState === 'hidden') void db.flush().catch(() => {});
  };
  const onHide = () => void db.flush().catch(() => {});
  document.addEventListener('visibilitychange', onVis);
  window.addEventListener('pagehide', onHide);
  return () => {
    document.removeEventListener('visibilitychange', onVis);
    window.removeEventListener('pagehide', onHide);
  };
}

let askedPersist = false;

export async function storageEstimate(): Promise<{ usage: number; quota: number; persisted: boolean }> {
  const s = typeof navigator !== 'undefined' ? navigator.storage : undefined;
  let persisted = false;
  try {
    // Ask once per page; later calls only read the answer.
    persisted = askedPersist ? ((await s?.persisted?.()) ?? false) : ((await s?.persist?.()) ?? false);
    askedPersist = true;
  } catch {
    // Not allowed here; report as not persisted.
  }
  try {
    const e = await s?.estimate?.();
    return { usage: e?.usage ?? 0, quota: e?.quota ?? 0, persisted };
  } catch {
    return { usage: 0, quota: 0, persisted };
  }
}
