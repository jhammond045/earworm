// Keeps the engine in step with the document. After a change the Compiler says which blobs
// are new and each goes to host.load, one pass at a time: while a pass runs, later requests
// only replace the target, so the next pass compiles the newest document (coalescing).
//
// Tuned curves come from the analysis worker and arrive late. `curveFor` returns null until
// the curve for (take, tune settings, octave) is ready, then asks for a new pass.

import { LOAD_NAMES, LOAD_OK } from '../m1/protocol';
import type { KlontHost } from '../m1/klont-host';
import { CURVE_STRIDE } from '../m1/track';
import type { Analysis, TuneSettings } from '../m2/analysis';
import { withTuned } from '../m2/take';
import { Compiler } from '../project/compile';
import type { Layer, Project, Take } from '../project/types';

export interface LoadRecord { what: string; status: number; name: string }

export interface SyncResult {
  /** The pass that covered this request. */
  pass: number;
  /** True when nothing was sent (no engine yet). */
  skipped: boolean;
  compileMs: number;
  /** Compile start until the last host.load resolved. */
  loadMs: number;
  /** Request until its pass finished: what the editor waits for. */
  totalMs: number;
  uploads: LoadRecord[];
  /** Compile or load failures, plain text. */
  errors: string[];
}

interface Waiter { resolve: (r: SyncResult) => void; t0: number }

const MAX_CURVE_CACHE = 48;

export class Sync {
  host: KlontHost | null = null;
  analysis: Analysis | null = null;
  readonly compiler = new Compiler();
  /** Frame records for a take, or null while they are not loaded. */
  framesFor: (take: Take) => Float32Array | null = () => null;
  /** Called when a pass ends (status line, errors). */
  onPass: (r: SyncResult) => void = () => {};

  private target: Project | null = null;
  private dirty = false;
  private running = false;
  private waiters: Waiter[] = [];
  private passes = 0;
  private curves = new Map<string, Float32Array>();
  private computing = new Set<string>();

  /** Compile `doc` and send what changed. Resolves when a pass that started after this call has finished. */
  request(doc: Project): Promise<SyncResult> {
    this.target = doc;
    this.dirty = true;
    return new Promise((resolve) => {
      this.waiters.push({ resolve, t0: performance.now() });
      void this.run();
    });
  }

  /** The engine was replaced or restarted: send everything again. */
  reset(): void {
    this.compiler.reset();
    this.curves.clear();
  }

  private async run(): Promise<void> {
    if (this.running) return;
    this.running = true;
    try {
      while (this.dirty) {
        this.dirty = false;
        const doc = this.target!;
        const waiters = this.waiters;
        this.waiters = [];
        const r = await this.pass(doc);
        const now = performance.now();
        for (const w of waiters) w.resolve({ ...r, totalMs: now - w.t0 });
        this.onPass(r);
      }
    } finally {
      this.running = false;
    }
  }

  private async pass(doc: Project): Promise<SyncResult> {
    const base = { pass: ++this.passes, skipped: false, compileMs: 0, loadMs: 0, totalMs: 0, uploads: [] as LoadRecord[], errors: [] as string[] };
    const host = this.host;
    if (!host) return { ...base, skipped: true };
    const t0 = performance.now();
    let uploads;
    try {
      uploads = this.compiler.compile(doc, (take, layer) => this.curveFor(take, layer));
    } catch (e) {
      base.errors.push(e instanceof Error ? e.message : String(e));
      base.compileMs = performance.now() - t0;
      return base;
    }
    base.compileMs = performance.now() - t0;
    for (const u of uploads) {
      const status = await host.load(u.kind, u.slot, u.blob);
      base.uploads.push({ what: u.what, status, name: LOAD_NAMES[status] ?? String(status) });
      if (status !== LOAD_OK) {
        base.errors.push(`${u.what}: ${LOAD_NAMES[status] ?? status}`);
        // The engine may be missing this blob; send it again with the next change.
        this.compiler.reset();
      }
    }
    base.loadMs = performance.now() - t0;
    return base;
  }

  // ---- curves

  private curveFor(take: Take, layer: Layer): Float32Array | null {
    const frames = this.framesFor(take);
    if (!frames) return null;
    const t = layer.tune;
    const settings: TuneSettings = { snap: t.snap, speedMs: t.speedMs, slides: t.slides, vibrato: t.vibrato, root: t.root, scale: t.scale,
      refCents: t.refCents, levelOut: t.levelOut };
    const key = `${take.id}|${JSON.stringify(settings)}|${t.octave}`;
    const hit = this.curves.get(key);
    if (hit) {
      this.curves.delete(key); // keep recently used entries at the end
      this.curves.set(key, hit);
      return hit;
    }
    if (!this.analysis || this.computing.has(key)) return null;
    this.computing.add(key);
    const pitch = new Float32Array(frames.length / CURVE_STRIDE);
    for (let i = 0; i < pitch.length; i++) pitch[i] = frames[i * CURVE_STRIDE];
    this.analysis.tune(frames.slice(), pitch, settings).then((r) => {
      this.curves.set(key, withTuned(frames, r.tuned, t.octave));
      while (this.curves.size > MAX_CURVE_CACHE) this.curves.delete(this.curves.keys().next().value as string);
      this.computing.delete(key);
      if (this.target) void this.request(this.target);
    }, () => { this.computing.delete(key); });
    return null;
  }

  /** The curve the engine uses for a take and layer, if ready (the timeline draws ribbons from it). */
  cachedCurve(take: Take, layer: Layer): Float32Array | null {
    const t = layer.tune;
    const settings: TuneSettings = { snap: t.snap, speedMs: t.speedMs, slides: t.slides, vibrato: t.vibrato, root: t.root, scale: t.scale,
      refCents: t.refCents, levelOut: t.levelOut };
    return this.curves.get(`${take.id}|${JSON.stringify(settings)}|${t.octave}`) ?? null;
  }
}
