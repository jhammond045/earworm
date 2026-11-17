// Runs the Rust instant match off the main thread. The analysis module needs
// no imports; it is fetched and instantiated once, on the first message.

import { MATCH_LEN } from '../host/protocol';

interface Exports {
  memory: WebAssembly.Memory;
  an_alloc(len: number): number;
  an_free(ptr: number, len: number): void;
  an_match(ptr: number, len: number, sampleRate: number, nBands: number): number;
}

export interface MatchRequest {
  id: number;
  signal: Float32Array;
  sampleRate: number;
  nBands: number;
}

export type MatchReply =
  | { id: number; floats: Float32Array; ms: number }
  | { id: number; error: string };

const scope = self as unknown as {
  onmessage: ((e: MessageEvent<MatchRequest>) => void) | null;
  postMessage(msg: unknown, transfer?: Transferable[]): void;
};

let instance: Promise<Exports> | null = null;

function load(): Promise<Exports> {
  instance ??= (async () => {
    const res = await fetch('/wasm/analysis.wasm');
    if (!res.ok) throw new Error(`analysis.wasm: HTTP ${res.status}`);
    const { instance: inst } = await WebAssembly.instantiate(await res.arrayBuffer(), {});
    return inst.exports as unknown as Exports;
  })();
  return instance;
}

scope.onmessage = async (e) => {
  const { id, signal, sampleRate, nBands } = e.data;
  try {
    const x = await load();
    const t0 = performance.now();
    const ptr = x.an_alloc(signal.length);
    // Allocation can grow memory, so take the view after it.
    new Float32Array(x.memory.buffer, ptr, signal.length).set(signal);
    const out = x.an_match(ptr, signal.length, sampleRate, nBands);
    const floats = new Float32Array(x.memory.buffer, out, MATCH_LEN).slice();
    x.an_free(ptr, signal.length);
    const reply: MatchReply = { id, floats, ms: performance.now() - t0 };
    scope.postMessage(reply, [floats.buffer]);
  } catch (err) {
    const reply: MatchReply = { id, error: String(err) };
    scope.postMessage(reply);
  }
};
