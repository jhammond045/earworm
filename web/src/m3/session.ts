// The open project: its Store, the autosave hookup, and the take blobs held in memory.
// Everything else on the page reads `session.store` (it changes when another project is opened).

import { newProject } from '../project/commands';
import type { HistoryState, JournalEntry } from '../project/history';
import { opfsBlobs, ProjectDb, type BlobStore, type Kv } from '../project/persist';
import { Store } from '../project/store';
import type { Layer, Project, Take } from '../project/types';

export interface TakeData {
  /** High-passed mic audio at the take's rate. Null if the blob is missing. */
  audio: Float32Array | null;
  /** 25-float frame records; slot 0 holds the repaired pitch. */
  frames: Float32Array;
}

export type ChangeKind = 'change' | 'swap';
type Listener = (doc: Project, kind: ChangeKind, entry: JournalEntry | null) => void;

export const audioRef = (takeId: string) => `audio-${takeId}`;
export const framesRef = (takeId: string) => `frames-${takeId}`;

/** What the compiler's grainFor returns (project/compile.ts GrainFor). */
export interface GrainSource { data: Float32Array; rate: number; refCents: number | null }

const MAX_GRAIN_CACHE = 16;

export class Session {
  store!: Store;
  blobs!: BlobStore;
  /** Take id -> data, loaded when a project opens or a take is captured. */
  readonly takes = new Map<string, TakeData>();
  /** Influence source blob id ('grain-...') -> mono audio at the source's own rate. */
  readonly grains = new Map<string, Float32Array>();
  /** Grain regions already sliced, by blob, region and rate: the same array comes back until the source changes. */
  private grainCache = new Map<string, { src: Float32Array; out: GrainSource }>();
  private grainLoading = new Map<string, Promise<void>>();
  /** Set when OPFS could not be used (takes then live in memory only). */
  blobError: string | null = null;
  private listeners = new Set<Listener>();
  private unsub: (() => void) | null = null;
  private loading = new Map<string, Promise<void>>();

  constructor(readonly kv: Kv, readonly db: ProjectDb, private readonly makeBlobs: (id: string) => BlobStore = opfsBlobs) {}

  get doc(): Project { return this.store.doc; }

  /** Called after every change and after the open project is replaced. */
  onChange(fn: Listener): () => void {
    this.listeners.add(fn);
    return () => { this.listeners.delete(fn); };
  }

  /** Open the project named by settings 'lastProject' (or `preferId`), else the newest, else a new "Untitled". */
  async init(preferId?: string): Promise<void> {
    const want = preferId ?? ((await this.kv.get('settings', 'lastProject')) as string | undefined);
    if (want && (await this.open(want, preferId === undefined))) return;
    const rows = await this.db.list();
    for (const r of rows) if (await this.open(r.id)) return;
    await this.create('Untitled');
  }

  async create(name: string, remember = true): Promise<void> {
    await this.adopt(newProject(name), remember);
  }

  /** Save a ready-made document as a new project and open it. */
  async adopt(doc: Project, remember = true): Promise<void> {
    await this.db.flush();
    await this.db.create(doc);
    await this.attach(new Store(doc), remember);
  }

  async open(id: string, remember = true): Promise<boolean> {
    await this.db.flush();
    const r = await this.db.open(id);
    if (!r) return false;
    await this.attach(new Store(r.doc, r.history), remember);
    return true;
  }

  async remove(id: string): Promise<void> {
    await this.db.remove(id);
    try {
      const b = this.makeBlobs(id);
      for (const ref of await b.list()) await b.del(ref);
    } catch { /* no blob store: nothing to delete */ }
    if ((await this.kv.get('settings', 'lastProject')) === id) await this.kv.del('settings', 'lastProject');
  }

  private async attach(store: Store, remember: boolean): Promise<void> {
    this.unsub?.();
    this.store = store;
    this.blobs = this.makeBlobs(store.doc.id);
    this.blobError = null;
    this.takes.clear();
    this.loading.clear();
    this.grains.clear();
    this.grainCache.clear();
    this.grainLoading.clear();
    this.unsub = store.subscribe((doc, entry) => {
      this.db.append(doc.id, entry, doc, store.history.toJSON() as HistoryState);
      this.emit(doc, 'change', entry);
    });
    if (remember) await this.kv.put('settings', 'lastProject', store.doc.id);
    await Promise.all([
      ...Object.values(store.doc.takes).map((t) => this.loadTake(t.id)),
      ...store.doc.layers.flatMap((l) => (l.influence ? [this.loadGrain(l.influence.source)] : [])),
    ]);
    this.emit(store.doc, 'swap', null);
  }

  /** Load a take's blobs into memory (once). Resolves when done, also when the blobs are missing. */
  loadTake(id: string): Promise<void> {
    if (this.takes.has(id)) return Promise.resolve();
    let p = this.loading.get(id);
    if (!p) {
      const blobs = this.blobs;
      p = (async () => {
        try {
          const [frames, audio] = await Promise.all([blobs.get(framesRef(id)), blobs.get(audioRef(id))]);
          if (blobs === this.blobs && frames) this.takes.set(id, { frames, audio });
        } catch (e) {
          this.blobError = e instanceof Error ? e.message : String(e);
        } finally {
          this.loading.delete(id);
        }
      })();
      this.loading.set(id, p);
    }
    return p;
  }

  /** Frame records for a take, or null while they load (or if they are gone). A later change event says when they arrive. */
  framesFor(take: Take): Float32Array | null {
    const d = this.takes.get(take.id);
    if (d) return d.frames;
    void this.loadTake(take.id).then(() => {
      if (this.takes.has(take.id)) this.emit(this.store.doc, 'change', null);
    });
    return null;
  }

  /** Keep a new take: in memory at once, in the blob store in the background (errors are remembered, not thrown). */
  async putTake(id: string, data: TakeData): Promise<void> {
    this.takes.set(id, data);
    try {
      await this.blobs.put(framesRef(id), data.frames);
      if (data.audio) await this.blobs.put(audioRef(id), data.audio);
    } catch (e) {
      this.blobError = e instanceof Error ? e.message : String(e);
      throw e;
    }
  }

  /** Keep an Influence source: in memory at once, in the blob store in the background (like a take). */
  async putGrain(ref: string, data: Float32Array): Promise<void> {
    this.grains.set(ref, data);
    try {
      await this.blobs.put(ref, data);
    } catch (e) {
      this.blobError = e instanceof Error ? e.message : String(e);
      throw e;
    }
  }

  /** Load an Influence source's audio into memory (once). Resolves when done, also when the blob is missing. */
  loadGrain(ref: string): Promise<void> {
    if (this.grains.has(ref)) return Promise.resolve();
    let p = this.grainLoading.get(ref);
    if (!p) {
      const blobs = this.blobs;
      p = (async () => {
        try {
          const data = await blobs.get(ref);
          if (blobs === this.blobs && data) this.grains.set(ref, data);
        } catch (e) {
          this.blobError = e instanceof Error ? e.message : String(e);
        } finally {
          this.grainLoading.delete(ref);
        }
      })();
      this.grainLoading.set(ref, p);
    }
    return p;
  }

  /** The layer's grain source for the compiler: its Influence source cut to the grain region, or null while the
   *  audio loads (a later change event says when it arrives). The same array comes back for the same blob and
   *  region, because the compiler sends a grain again only when the array is a different one. */
  grainFor(layer: Layer): GrainSource | null {
    const infl = layer.influence;
    if (!infl || !infl.grain) return null;
    const src = this.grains.get(infl.source);
    if (!src) {
      void this.loadGrain(infl.source).then(() => {
        if (this.grains.has(infl.source)) this.emit(this.store.doc, 'change', null);
      });
      return null;
    }
    const start = Math.max(0, Math.min(src.length - 1, Math.floor(infl.grain.start)));
    const end = Math.max(start + 1, Math.min(src.length, Math.floor(infl.grain.end)));
    const key = `${infl.source}|${start}|${end}|${infl.sourceRate}|${infl.grain.refCents}`;
    const hit = this.grainCache.get(key);
    if (hit && hit.src === src) {
      this.grainCache.delete(key);
      this.grainCache.set(key, hit);
      return hit.out;
    }
    const out: GrainSource = { data: src.slice(start, end), rate: infl.sourceRate, refCents: infl.grain.refCents };
    this.grainCache.set(key, { src, out });
    while (this.grainCache.size > MAX_GRAIN_CACHE) this.grainCache.delete(this.grainCache.keys().next().value as string);
    return out;
  }

  private emit(doc: Project, kind: ChangeKind, entry: JournalEntry | null): void {
    for (const fn of [...this.listeners]) fn(doc, kind, entry);
  }
}
