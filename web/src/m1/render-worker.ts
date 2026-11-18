// Worker that renders with its own klont.wasm instance: song export, the
// worst-case benchmark and per-block timing. Messages in and out are typed in
// offline.ts. The render loops yield to the event loop between chunks so a
// 'cancel' message gets through.

import { BLOCK, renderSong, runBlockTiming, runWorstCase, type Hooks, type RenderOptions, type SongBlobs } from './render-core';
import type { EngineExports } from './protocol';

export type WorkerRequest =
  | { t: 'render'; id: number; song: SongBlobs; opts: RenderOptions }
  | { t: 'worst'; id: number; budget: number; seconds: number }
  | { t: 'blocks'; id: number; budget: number; seconds: number }
  | { t: 'cancel'; id: number };

const WASM_URL = `${import.meta.env.BASE_URL}wasm/klont.wasm`;
let modPromise: Promise<WebAssembly.Module> | null = null;

/** One compiled module per worker; each job gets a fresh instance (fresh memory, no leftovers). */
async function instance(): Promise<EngineExports> {
  modPromise ??= fetch(WASM_URL).then(async (r) => {
    if (!r.ok) throw new Error(`klont.wasm: HTTP ${r.status}`);
    return WebAssembly.compile(await r.arrayBuffer());
  });
  return new WebAssembly.Instance(await modPromise, {}).exports as unknown as EngineExports;
}

const cancelled = new Set<number>();
const chan = new MessageChannel();
// A macrotask hop without the 4 ms setTimeout clamp.
const yieldFn = () => new Promise<void>((res) => { chan.port1.onmessage = () => res(); chan.port2.postMessage(0); });

const ctx = self as unknown as { postMessage(m: unknown, t?: Transferable[]): void; onmessage: ((e: MessageEvent) => void) | null };

ctx.onmessage = async (ev: MessageEvent<WorkerRequest>) => {
  const m = ev.data;
  if (m.t === 'cancel') { cancelled.add(m.id); return; }
  const hooks: Hooks = {
    yieldFn,
    cancelled: () => cancelled.has(m.id),
    onProgress: (p) => ctx.postMessage({ t: 'progress', id: m.id, ...p }),
  };
  try {
    const ex = await instance();
    if (m.t === 'render') {
      const r = await renderSong(ex, m.song, m.opts, hooks);
      ctx.postMessage({ t: 'done', id: m.id, result: r }, [r.left.buffer, r.right.buffer]);
    } else if (m.t === 'worst') {
      ctx.postMessage({ t: 'done', id: m.id, result: await runWorstCase(ex, m.budget, m.seconds, hooks) });
    } else if (m.t === 'blocks') {
      ctx.postMessage({ t: 'done', id: m.id, result: await runBlockTiming(ex, m.budget, m.seconds, hooks) });
    }
  } catch (e) {
    ctx.postMessage({ t: 'error', id: m.id, message: e instanceof Error ? e.message : String(e) });
  } finally {
    cancelled.delete(m.id);
  }
};

export { BLOCK };
