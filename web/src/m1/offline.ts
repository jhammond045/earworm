// Promise-based main-thread wrapper around the render worker.

import type { BlockTimingResult, RenderOptions, RenderResult, SongBlobs, WorstResult } from './render-core';
import type { WorkerRequest } from './render-worker';
export type { BlockTimingResult, RenderOptions, RenderResult, SongBlobs, WorstResult } from './render-core';

export interface Progress { frames: number; seconds: number; ended: boolean }

export interface Job<T> {
  promise: Promise<T>;
  /** Stops after the current chunk. A cancelled render resolves with stopReason 'cancelled' (worst/blocks reject). */
  cancel(): void;
}

type NoId<T> = T extends unknown ? Omit<T, 'id'> : never;

export class OfflineRenderer {
  private readonly worker = new Worker(new URL('./render-worker.ts', import.meta.url), { type: 'module' });
  private nextId = 1;

  private run<T>(req: NoId<WorkerRequest>, onProgress?: (p: Progress) => void): Job<T> {
    const id = this.nextId++;
    const promise = new Promise<T>((resolve, reject) => {
      const onMsg = (ev: MessageEvent) => {
        const m = ev.data;
        if (m.id !== id) return;
        if (m.t === 'progress') onProgress?.(m);
        else {
          this.worker.removeEventListener('message', onMsg);
          if (m.t === 'done') resolve(m.result as T);
          else reject(new Error(m.message));
        }
      };
      this.worker.addEventListener('message', onMsg);
      this.worker.addEventListener('error', (e) => reject(new Error(e.message || 'render worker failed')), { once: true });
      this.worker.postMessage({ ...req, id });
    });
    return { promise, cancel: () => this.worker.postMessage({ t: 'cancel', id }) };
  }

  /** Render a song to stereo Float32Arrays (48 kHz by default). The blobs are copied, not transferred. */
  render(song: SongBlobs, opts: RenderOptions = {}, onProgress?: (p: Progress) => void): Job<RenderResult> {
    return this.run({ t: 'render', song, opts }, onProgress);
  }
  /** Worst-case scene (8 layers x 8 voices x 64 partials) for `seconds` at a fixed partial budget. */
  worstCase(budget: number, seconds: number, onProgress?: (p: Progress) => void): Job<WorstResult> {
    return this.run({ t: 'worst', budget, seconds }, onProgress);
  }
  /** Same scene in 128-frame blocks, each timed: percentiles in ms against the 2.67 ms quantum. */
  blockTiming(budget: number, seconds: number, onProgress?: (p: Progress) => void): Job<BlockTimingResult> {
    return this.run({ t: 'blocks', budget, seconds }, onProgress);
  }

  terminate(): void { this.worker.terminate(); }
}
