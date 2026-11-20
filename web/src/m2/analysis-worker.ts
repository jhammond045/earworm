// Runs the capture second pass and the stored-take tune off the main thread.
// The analysis module needs no imports; it is fetched and instantiated once.

import { AnalysisCore, type SecondPass, type SecondPassOptions, type TuneSettings } from './analysis';

export type AnalysisRequest =
  | { id: number; op: 'init' }
  | { id: number; op: 'second'; audio: Float32Array; sampleRate: number; frames: Float32Array; opts: SecondPassOptions }
  | { id: number; op: 'tune'; frames: Float32Array; pitch: Float32Array; settings: TuneSettings };

export type AnalysisReply =
  | { id: number; result: SecondPass | { tuned: Float32Array; loudness: Float32Array } | null }
  | { id: number; error: string };

const scope = self as unknown as {
  onmessage: ((e: MessageEvent<AnalysisRequest>) => void) | null;
  postMessage(msg: unknown, transfer?: Transferable[]): void;
};

let core: Promise<AnalysisCore> | null = null;
const load = () => (core ??= AnalysisCore.fromUrl(`${import.meta.env.BASE_URL}wasm/analysis.wasm`));

scope.onmessage = async (e) => {
  const req = e.data;
  try {
    const c = await load();
    if (req.op === 'init') {
      scope.postMessage({ id: req.id, result: null } satisfies AnalysisReply);
    } else if (req.op === 'second') {
      const r = c.secondPass(req.audio, req.sampleRate, req.frames, req.opts);
      scope.postMessage({ id: req.id, result: r } satisfies AnalysisReply, [r.pitch.buffer]);
    } else {
      const r = c.tune(req.frames, req.pitch, req.settings);
      scope.postMessage({ id: req.id, result: r } satisfies AnalysisReply, [r.tuned.buffer, r.loudness.buffer]);
    }
  } catch (err) {
    scope.postMessage({ id: req.id, error: String(err) } satisfies AnalysisReply);
  }
};
