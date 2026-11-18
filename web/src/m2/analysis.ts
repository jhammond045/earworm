// The capture second pass, from the main thread. `Analysis` wraps the worker
// (analysis-worker.ts); `AnalysisCore` is the wasm wrapper both the worker and
// the tests use. Flat layouts match crates/analysis/src/second.rs.

export type ClipKind = 'melody' | 'beat' | 'texture';

/** Take seconds; `cents` above C0 (median pitch). */
export interface Note { t: number; dur: number; cents: number; hard: number }

/** Seconds for `attack` and `hold`, semitones per second for `bend`, 0 or 1 for `endStop`/`endClick`. */
export interface HitShape {
  attack: number; burst: number; cents: number | null; bend: number; hold: number;
  vowel: number; tone: number; endStop: number; endClick: number;
}

export interface Hit {
  t: number; dur: number; slot: number; strength: number; shape: HitShape;
  /** The 6 grouping features, unstandardized: ln attack (ms), burst, low-band share, centroid (octaves from 1 kHz), tone, ln hold (ms). */
  features: number[];
  /** `live_hit_features` of the frame at the onset: brightness, low, noise. */
  live: number[];
}

export interface TuneSettings {
  snap: number; speedMs: number; slides: 0 | 1 | 2; vibrato: number; root: number;
  scale: number; refCents: number; levelOut: number;
}

export interface TaughtSlot { centre: number[]; spread: number[] }

export interface SecondPassOptions {
  kind?: 'auto' | ClipKind;
  /** Kit groups for beats; 0 or missing = best silhouette. */
  kinds?: number;
  /** Four entries, one per slot. */
  taught?: (TaughtSlot | null)[];
}

export interface SecondPass {
  kind: ClipKind; voicedFraction: number; onsetsPerSec: number;
  pitch: Float32Array;
  repaired: number;
  refCents: number;
  root: number; scale: number;
  bpm: number | null; beats: number | null; loopSec: number;
  notes: Note[]; hits: Hit[]; kinds: number;
  ms: number;
}

// ---- Flat layouts (second.rs `to_floats`, `Options::from_floats`; lib.rs `an_tune`) ----

export const FRAME_STRIDE = 25;
export const OUT_HEADER = 16;
export const NOTE_LEN = 4;
export const HIT_LEN = 22;
export const OPTIONS_LEN = 2 + 4 * 13;
export const TUNE_SETTINGS_LEN = 8;
export const SLOTS = 4;
/** Spread floor per grouping feature (beats.rs `FEATURE_FLOOR`): the smallest spread a taught slot keeps. */
export const FEATURE_FLOOR = [1.0, 0.2, 0.15, 0.7, 0.4, 0.5];

const KINDS: ClipKind[] = ['melody', 'beat', 'texture'];

export function decodeSecondPass(floats: ArrayLike<number>, ms = 0): SecondPass {
  if (floats.length < OUT_HEADER) throw new Error(`second pass result has ${floats.length} floats, expected at least ${OUT_HEADER}`);
  const f = (i: number) => floats[i];
  const nFrames = f(11), nNotes = f(12), nHits = f(13), hitLen = f(14);
  if (hitLen !== HIT_LEN) throw new Error(`second pass hit record is ${hitLen} floats, expected ${HIT_LEN}`);
  const want = OUT_HEADER + nFrames + nNotes * NOTE_LEN + nHits * HIT_LEN;
  if (floats.length < want) throw new Error(`second pass result has ${floats.length} floats, header says ${want}`);
  const kind = KINDS[f(0)];
  if (!kind) throw new Error(`second pass kind ${f(0)}`);
  const pitch = Float32Array.from({ length: nFrames }, (_, i) => floats[OUT_HEADER + i]);
  let o = OUT_HEADER + nFrames;
  const notes: Note[] = [];
  for (let i = 0; i < nNotes; i++, o += NOTE_LEN) notes.push({ t: f(o), dur: f(o + 1), cents: f(o + 2), hard: f(o + 3) });
  const hits: Hit[] = [];
  for (let i = 0; i < nHits; i++, o += HIT_LEN) {
    const cents = f(o + 6);
    hits.push({
      t: f(o), dur: f(o + 1), slot: f(o + 2), strength: f(o + 3),
      shape: {
        attack: f(o + 4), burst: f(o + 5), cents: Number.isNaN(cents) ? null : cents, bend: f(o + 7), hold: f(o + 8),
        vowel: f(o + 9), tone: f(o + 10), endStop: f(o + 11), endClick: f(o + 12),
      },
      features: [0, 1, 2, 3, 4, 5].map((d) => f(o + 13 + d)),
      live: [0, 1, 2].map((d) => f(o + 19 + d)),
    });
  }
  const bpm = f(7), beats = f(8);
  return {
    kind, voicedFraction: f(1), onsetsPerSec: f(2), pitch, repaired: f(3), refCents: f(4), root: f(5), scale: f(6),
    bpm: Number.isNaN(bpm) ? null : bpm, beats: Number.isNaN(beats) ? null : beats, loopSec: f(9),
    notes, hits, kinds: f(10), ms,
  };
}

/** Flat options for `an_second`. */
export function encodeOptions(opts: SecondPassOptions = {}): Float32Array {
  const out = new Float32Array(OPTIONS_LEN);
  out[0] = opts.kind === 'melody' ? 1 : opts.kind === 'beat' ? 2 : opts.kind === 'texture' ? 3 : 0;
  out[1] = Math.max(0, Math.round(opts.kinds ?? 0));
  for (let s = 0; s < SLOTS; s++) {
    const t = opts.taught?.[s];
    if (!t) continue;
    const o = 2 + s * 13;
    out[o] = 1;
    for (let d = 0; d < 6; d++) {
      out[o + 1 + d] = t.centre[d];
      out[o + 7 + d] = t.spread[d];
    }
  }
  return out;
}

/** Flat settings for `an_tune`. */
export function encodeTuneSettings(s: TuneSettings): Float32Array {
  return Float32Array.from([s.snap, s.speedMs, s.slides, s.vibrato, s.root, s.scale, s.refCents, s.levelOut]);
}

/**
 * "Teach my kit": one centre and spread per slot from the hits captured for it
 * (`hits[slot]`, any number of slots up to four; a slot with no hits stays null).
 * The spread is the standard deviation of each feature, never below `FEATURE_FLOOR`.
 */
export function teach(hits: Hit[][]): (TaughtSlot | null)[] {
  const out: (TaughtSlot | null)[] = [];
  for (let s = 0; s < SLOTS; s++) {
    const h = hits[s] ?? [];
    if (h.length === 0) {
      out.push(null);
      continue;
    }
    const centre: number[] = [];
    const spread: number[] = [];
    for (let d = 0; d < 6; d++) {
      const m = h.reduce((a, x) => a + x.features[d], 0) / h.length;
      const sd = Math.sqrt(h.reduce((a, x) => a + (x.features[d] - m) ** 2, 0) / h.length);
      centre.push(m);
      spread.push(Math.max(sd, FEATURE_FLOOR[d]));
    }
    out.push({ centre, spread });
  }
  return out;
}

// ---- wasm ----

interface Exports {
  memory: WebAssembly.Memory;
  an_alloc(len: number): number;
  an_free(ptr: number, len: number): void;
  an_result_len(): number;
  an_second(audio: number, audioLen: number, sr: number, frames: number, framesLen: number, opts: number, optsLen: number): number;
  an_tune(frames: number, framesLen: number, pitch: number, settings: number): number;
}

/** The analysis wasm module. Synchronous; the worker and the tests use it directly. */
export class AnalysisCore {
  private constructor(private x: Exports) {}

  static fromBytes(bytes: BufferSource): AnalysisCore {
    const inst = new WebAssembly.Instance(new WebAssembly.Module(bytes), {});
    return new AnalysisCore(inst.exports as unknown as Exports);
  }

  static async fromUrl(url: string): Promise<AnalysisCore> {
    const res = await fetch(url);
    if (!res.ok) throw new Error(`${url}: HTTP ${res.status}`);
    const { instance } = await WebAssembly.instantiate(await res.arrayBuffer(), {});
    return new AnalysisCore(instance.exports as unknown as Exports);
  }

  /** Copy `data` into wasm memory. Returns the pointer; free with `release`. */
  private put(data: Float32Array): number {
    const len = Math.max(1, data.length);
    const ptr = this.x.an_alloc(len);
    // Allocation can grow memory, so take the view after it.
    new Float32Array(this.x.memory.buffer, ptr, data.length).set(data);
    return ptr;
  }

  private release(ptr: number, data: Float32Array): void {
    this.x.an_free(ptr, Math.max(1, data.length));
  }

  private result(ptr: number): Float32Array {
    return new Float32Array(this.x.memory.buffer, ptr, this.x.an_result_len()).slice();
  }

  secondPass(audio: Float32Array, sampleRate: number, frames: Float32Array, opts: SecondPassOptions = {}): SecondPass {
    const flat = encodeOptions(opts);
    const t0 = performance.now();
    const a = this.put(audio), f = this.put(frames), o = this.put(flat);
    let floats: Float32Array;
    try {
      floats = this.result(this.x.an_second(a, audio.length, sampleRate, f, frames.length, o, flat.length));
    } finally {
      this.release(a, audio);
      this.release(f, frames);
      this.release(o, flat);
    }
    return decodeSecondPass(floats, performance.now() - t0);
  }

  /** Tuned pitch and loudness (after Level out) per frame of a stored take; `pitch` is the repaired curve. */
  tune(frames: Float32Array, pitch: Float32Array, s: TuneSettings): { tuned: Float32Array; loudness: Float32Array } {
    const n = Math.floor(frames.length / FRAME_STRIDE);
    if (pitch.length < n) throw new Error(`pitch has ${pitch.length} values for ${n} frames`);
    const flat = encodeTuneSettings(s);
    const f = this.put(frames), p = this.put(pitch.subarray(0, n)), st = this.put(flat);
    try {
      const out = this.result(this.x.an_tune(f, frames.length, p, st));
      return { tuned: out.slice(0, n), loudness: out.slice(n, 2 * n) };
    } finally {
      this.release(f, frames);
      this.release(p, pitch.subarray(0, n));
      this.release(st, flat);
    }
  }
}

// ---- worker client ----

import type { AnalysisReply, AnalysisRequest } from './analysis-worker';

type Pending = { resolve(v: any): void; reject(e: Error): void };
type Body = AnalysisRequest extends infer R ? (R extends { id: number } ? Omit<R, 'id'> : never) : never;

export class Analysis {
  private nextId = 1;
  private pending = new Map<number, Pending>();

  private constructor(private worker: Worker) {
    worker.onmessage = (e: MessageEvent<AnalysisReply>) => {
      const p = this.pending.get(e.data.id);
      if (!p) return;
      this.pending.delete(e.data.id);
      if ('error' in e.data) p.reject(new Error(e.data.error));
      else p.resolve(e.data.result);
    };
    worker.onerror = (e) => {
      for (const p of this.pending.values()) p.reject(new Error(e.message || 'analysis worker failed'));
      this.pending.clear();
    };
  }

  /** Start the worker and load the wasm once. */
  static async create(): Promise<Analysis> {
    const a = new Analysis(new Worker(new URL('./analysis-worker.ts', import.meta.url), { type: 'module' }));
    await a.call({ op: 'init' });
    return a;
  }

  private call<T>(req: Body, transfer: Transferable[] = []): Promise<T> {
    return new Promise((resolve, reject) => {
      const id = this.nextId++;
      this.pending.set(id, { resolve, reject });
      this.worker.postMessage({ ...req, id }, transfer);
    });
  }

  /** The caller's arrays are copied, not transferred. */
  secondPass(audio: Float32Array, sampleRate: number, frames: Float32Array, opts: SecondPassOptions = {}): Promise<SecondPass> {
    const a = audio.slice(), f = frames.slice();
    return this.call({ op: 'second', audio: a, sampleRate, frames: f, opts }, [a.buffer, f.buffer]);
  }

  tune(frames: Float32Array, pitch: Float32Array, s: TuneSettings): Promise<{ tuned: Float32Array; loudness: Float32Array }> {
    const f = frames.slice(), p = pitch.slice();
    return this.call({ op: 'tune', frames: f, pitch: p, settings: s }, [f.buffer, p.buffer]);
  }

  teach(hits: Hit[][]): (TaughtSlot | null)[] {
    return teach(hits);
  }

  dispose(): void {
    this.worker.terminate();
    for (const p of this.pending.values()) p.reject(new Error('analysis disposed'));
    this.pending.clear();
  }
}
