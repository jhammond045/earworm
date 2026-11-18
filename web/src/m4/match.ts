// The Influence match (TDD "Sound Influence"), from the main thread.
// `MatchCore` is the wasm wrapper the worker and the tests use; `Matcher`
// wraps the worker (match-worker.ts); `liveMatch` reruns the match while a
// capture grows. The flat layout is `FULL_MATCH_LEN` in
// crates/analysis/src/full.rs (a test reads that file and checks the numbers).

import { COUNT } from '../gen/params';

/** Floats after the 96 target values; see `FULL_MATCH_LEN` in crates/analysis/src/full.rs. */
export const FULL_EXTRA = 10;
export const FULL_MATCH_LEN = COUNT + FULL_EXTRA;

/** `flags` bits of `an_match_full`. */
export const FLAG_DECAYING = 1;
export const FLAG_USE_SOUND = 2;
export const FLAG_NO_SOUND = 4;

export interface MatchOptions {
  /** The audio is one hit for a kit slot: the decaying-sound rows are on. */
  decaying?: boolean;
  /** Raise src.grain_mix in the target. Left out: on for unpitched sources only. */
  useSound?: boolean;
}

export interface GrainSuggestion {
  /** Sample range of the steady region in the analysed signal. */
  start: number;
  end: number;
  /** Cents above C0, or null when the region is unpitched. */
  refCents: number | null;
}

export interface Match {
  /** 96 normalized values in params order (64 scalars, 16 tone bands, 16 noise bands). */
  target: Float32Array;
  pitched: boolean;
  /** Cents above C0 to play the match at (C4 when unpitched). */
  pitchCents: number;
  /** How long to hold the audition note, seconds. */
  durationS: number;
  voicedFraction: number;
  /** The motion rows (LFO, chaos) rest on under two cycles, or under 2 s of steady sound. */
  provisional: boolean;
  /** src.grain_mix was raised in `target`. */
  useSound: boolean;
  grain: GrainSuggestion | null;
  /** Wall-clock milliseconds the call took (0 when decoded by hand). */
  ms: number;
}

export function matchFlags(opts: MatchOptions = {}): number {
  let f = 0;
  if (opts.decaying) f |= FLAG_DECAYING;
  if (opts.useSound === true) f |= FLAG_USE_SOUND;
  if (opts.useSound === false) f |= FLAG_NO_SOUND;
  return f;
}

export function decodeMatch(floats: ArrayLike<number>, ms = 0): Match {
  if (floats.length < FULL_MATCH_LEN) throw new Error(`match result has ${floats.length} floats, expected ${FULL_MATCH_LEN}`);
  const f = (i: number) => floats[COUNT + i];
  const ref = f(8);
  return {
    target: Float32Array.from({ length: COUNT }, (_, i) => floats[i]),
    pitched: f(0) !== 0,
    pitchCents: f(1),
    durationS: f(2),
    voicedFraction: f(3),
    provisional: f(4) !== 0,
    useSound: f(5) !== 0,
    grain: f(9) !== 0 ? { start: f(6), end: f(7), refCents: Number.isNaN(ref) ? null : ref } : null,
    ms,
  };
}

// ---- wasm ----

interface Exports {
  memory: WebAssembly.Memory;
  an_alloc(len: number): number;
  an_free(ptr: number, len: number): void;
  an_match_full(ptr: number, len: number, sampleRate: number, flags: number): number;
}

/** The analysis wasm module, for the match only. Synchronous; the worker and the tests use it directly. */
export class MatchCore {
  private constructor(private x: Exports) {}

  static fromBytes(bytes: BufferSource): MatchCore {
    const inst = new WebAssembly.Instance(new WebAssembly.Module(bytes), {});
    return new MatchCore(inst.exports as unknown as Exports);
  }

  static async fromUrl(url: string): Promise<MatchCore> {
    const res = await fetch(url);
    if (!res.ok) throw new Error(`${url}: HTTP ${res.status}`);
    const { instance } = await WebAssembly.instantiate(await res.arrayBuffer(), {});
    return new MatchCore(instance.exports as unknown as Exports);
  }

  match(signal: Float32Array, sampleRate: number, opts: MatchOptions = {}): Match {
    const t0 = performance.now();
    const len = Math.max(1, signal.length);
    const ptr = this.x.an_alloc(len);
    try {
      // Allocation can grow memory, so take the view after it.
      new Float32Array(this.x.memory.buffer, ptr, signal.length).set(signal);
      const out = this.x.an_match_full(ptr, signal.length, sampleRate, matchFlags(opts));
      const floats = new Float32Array(this.x.memory.buffer, out, FULL_MATCH_LEN).slice();
      return decodeMatch(floats, performance.now() - t0);
    } finally {
      this.x.an_free(ptr, len);
    }
  }
}

// ---- worker client ----

import type { MatchReply, MatchRequest } from './match-worker';

type Pending = { resolve(v: any): void; reject(e: Error): void };
type Body = MatchRequest extends infer R ? (R extends { id: number } ? Omit<R, 'id'> : never) : never;

/** Anything `liveMatch` can ask for a match: the worker client, or a `MatchCore` in a wrapper. */
export interface MatchSource {
  match(signal: Float32Array, sampleRate: number, opts?: MatchOptions): Promise<Match>;
}

export class Matcher implements MatchSource {
  private nextId = 1;
  private pending = new Map<number, Pending>();

  private constructor(private worker: Worker) {
    worker.onmessage = (e: MessageEvent<MatchReply>) => {
      const p = this.pending.get(e.data.id);
      if (!p) return;
      this.pending.delete(e.data.id);
      if ('error' in e.data) p.reject(new Error(e.data.error));
      else p.resolve(e.data.result);
    };
    worker.onerror = (e) => {
      for (const p of this.pending.values()) p.reject(new Error(e.message || 'match worker failed'));
      this.pending.clear();
    };
  }

  /** Start the worker and load the wasm once. */
  static async create(): Promise<Matcher> {
    const m = new Matcher(new Worker(new URL('./match-worker.ts', import.meta.url), { type: 'module' }));
    await m.call({ op: 'init' });
    return m;
  }

  private call<T>(req: Body, transfer: Transferable[] = []): Promise<T> {
    return new Promise((resolve, reject) => {
      const id = this.nextId++;
      this.pending.set(id, { resolve, reject });
      this.worker.postMessage({ ...req, id }, transfer);
    });
  }

  /** The caller's array is copied, not transferred (a capture goes on growing). */
  match(signal: Float32Array, sampleRate: number, opts: MatchOptions = {}): Promise<Match> {
    const s = signal.slice();
    return this.call({ op: 'match', signal: s, sampleRate, opts }, [s.buffer]);
  }

  dispose(): void {
    this.worker.terminate();
    for (const p of this.pending.values()) p.reject(new Error('matcher disposed'));
    this.pending.clear();
  }
}

// ---- matching while it listens ----

/** The TDD's publish rate for a new target while listening. */
export const LIVE_INTERVAL_MS = 250;
/** Tone parameters are available from the first 100 ms of sound. */
export const LIVE_MIN_SECONDS = 0.1;

export interface LiveMatchOptions {
  sampleRate: number;
  /** Rerun period. Default 250 ms. */
  intervalMs?: number;
  match?: MatchOptions;
  /** Called when a run fails; the next tick still runs. */
  onError?: (e: Error) => void;
}

export interface LiveMatch {
  /** Stop the timer. A run already in flight still delivers. */
  stop(): void;
  /**
   * Stop and run the match once over the whole capture: "on release the same
   * code runs once over the whole capture and the result replaces the running
   * estimate". `onMatch` is called with it too.
   */
  finish(): Promise<Match>;
}

/**
 * Rerun the match on a growing capture every 250 ms (TDD "Matching while it
 * listens", the lazy version).
 *
 * `read` returns everything captured so far (a view of a bigger buffer is
 * fine). A run is skipped while the last one is still going, and when nothing
 * new has arrived since the last run.
 *
 * Ceiling: each run starts over on the whole capture, so the cost grows with
 * the capture until the spectral work reaches its fixed frame cap (120
 * frames, reached after about 1.2 s of loud sound); after that only the
 * loudness, pitch and motion tracks grow, linearly. Measured natively, 13 s of audio matches in
 * about 120 ms; the browser's wasm is two to three times slower, and every run
 * also copies the capture to the worker (2 MB per 10 s at 48 kHz). A capture
 * far past ten seconds, or a slower machine, would want the running sums the
 * TDD describes (harmonic levels, band levels and residual energy accumulated
 * per frame, fits only at onsets and offsets) instead of rerunning.
 */
export function liveMatch(
  read: () => Float32Array,
  matcher: MatchSource,
  onMatch: (m: Match) => void,
  opts: LiveMatchOptions,
): LiveMatch {
  const interval = opts.intervalMs ?? LIVE_INTERVAL_MS;
  let busy = false;
  let lastLen = -1;
  let stopped = false;

  const run = async (force: boolean): Promise<Match | null> => {
    const sig = read();
    if (!force && (sig.length === lastLen || sig.length < LIVE_MIN_SECONDS * opts.sampleRate)) return null;
    lastLen = sig.length;
    const m = await matcher.match(sig, opts.sampleRate, opts.match);
    return m;
  };

  const timer = setInterval(() => {
    if (busy || stopped) return;
    busy = true;
    run(false)
      .then((m) => {
        if (m) onMatch(m);
      })
      .catch((e) => opts.onError?.(e instanceof Error ? e : new Error(String(e))))
      .finally(() => {
        busy = false;
      });
  }, interval);

  const stop = () => {
    stopped = true;
    clearInterval(timer);
  };

  return {
    stop,
    async finish() {
      stop();
      const m = (await run(true))!;
      onMatch(m);
      return m;
    },
  };
}
