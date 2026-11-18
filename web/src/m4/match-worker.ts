// Runs the Influence match off the main thread. The analysis module needs no
// imports; it is fetched and instantiated once.

import { MatchCore, type Match, type MatchOptions } from './match';

export type MatchRequest =
  | { id: number; op: 'init' }
  | { id: number; op: 'match'; signal: Float32Array; sampleRate: number; opts: MatchOptions };

export type MatchReply = { id: number; result: Match | null } | { id: number; error: string };

const scope = self as unknown as {
  onmessage: ((e: MessageEvent<MatchRequest>) => void) | null;
  postMessage(msg: unknown, transfer?: Transferable[]): void;
};

let core: Promise<MatchCore> | null = null;
const load = () => (core ??= MatchCore.fromUrl('/wasm/analysis.wasm'));

scope.onmessage = async (e) => {
  const req = e.data;
  try {
    const c = await load();
    if (req.op === 'init') {
      scope.postMessage({ id: req.id, result: null } satisfies MatchReply);
    } else {
      const r = c.match(req.signal, req.sampleRate, req.opts);
      scope.postMessage({ id: req.id, result: r } satisfies MatchReply, [r.target.buffer]);
    }
  } catch (err) {
    scope.postMessage({ id: req.id, error: String(err) } satisfies MatchReply);
  }
};
