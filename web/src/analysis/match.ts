// Main-thread client for the match worker.

import { MATCH_DURATION, MATCH_LEN, MATCH_NOISE, MATCH_PITCH, MATCH_PITCHED, MATCH_TONE, MATCH_VOICED, MAX_BANDS, PARAMS } from '../host/protocol';
import type { MatchReply } from './match-worker';

export interface Match {
  scalars: Float32Array;
  /** MAX_BANDS floats; only the first nBands are meaningful. */
  tone: Float32Array;
  noise: Float32Array;
  /** Cents above C0. */
  pitchCents: number;
  durationS: number;
  voicedFraction: number;
  pitched: boolean;
  /** Time spent inside the wasm call. */
  ms: number;
}

export function decodeMatch(floats: ArrayLike<number>, ms = 0): Match {
  if (floats.length < MATCH_LEN) throw new Error(`match result has ${floats.length} floats, expected ${MATCH_LEN}`);
  const f = Float32Array.from(floats);
  return {
    scalars: f.slice(0, PARAMS.length),
    tone: f.slice(MATCH_TONE, MATCH_TONE + MAX_BANDS),
    noise: f.slice(MATCH_NOISE, MATCH_NOISE + MAX_BANDS),
    pitchCents: f[MATCH_PITCH],
    durationS: f[MATCH_DURATION],
    voicedFraction: f[MATCH_VOICED],
    pitched: f[MATCH_PITCHED] > 0.5,
    ms,
  };
}

let worker: Worker | null = null;
let nextId = 1;
const pending = new Map<number, { resolve(m: Match): void; reject(e: Error): void }>();

function getWorker(): Worker {
  if (worker) return worker;
  const w = new Worker(new URL('./match-worker.ts', import.meta.url), { type: 'module' });
  w.onmessage = (e: MessageEvent<MatchReply>) => {
    const p = pending.get(e.data.id);
    if (!p) return;
    pending.delete(e.data.id);
    if ('error' in e.data) p.reject(new Error(e.data.error));
    else p.resolve(decodeMatch(e.data.floats, e.data.ms));
  };
  w.onerror = (e) => {
    for (const p of pending.values()) p.reject(new Error(e.message || 'match worker failed'));
    pending.clear();
    worker = null;
  };
  worker = w;
  return w;
}

/** Measure `signal` (mono). The caller's array is copied, not transferred. */
export function matchSound(signal: Float32Array, sampleRate: number, nBands: number): Promise<Match> {
  return new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, { resolve, reject });
    const copy = signal.slice();
    getWorker().postMessage({ id, signal: copy, sampleRate, nBands }, [copy.buffer]);
  });
}
