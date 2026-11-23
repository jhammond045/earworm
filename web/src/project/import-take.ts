// Import an audio file as a take (docs/quant.md "Importing a sample", stage Q2). Flexing a sample
// or a loop needs it as a take: the file is decoded to mono at the engine's rate, run through the
// live tier as a capture would be (the same high-pass and the same frames, from a klont.wasm
// instance of its own), and given to the second pass for its notes or hits and its anchors. The
// take is placed at the start of the section, on a layer with an input element at mix 1, so it
// plays as audio and Feel can stretch it. On a layer set to Poly the file goes to the poly pass
// instead, as a Poly capture would (docs/poly.md P3): notes with their own curves.

import type { EngineExports } from '../m1/protocol';
import { readBufferTable } from '../m1/protocol';
import { placeTake } from './overdub';
import type { Hit as TakeHit, Note as TakeNote, PolyOptions, PolyPass, SecondPass, SecondPassOptions } from '../m2/analysis';
import { isPoly, polyOptions } from '../record/poly';
import type { Clip, Layer, Project, Take } from './types';

const BLOCK = 128;
const FRAME_STRIDE = 25;
/** The longest take a capture keeps, and so an import. */
export const MAX_IMPORT_SEC = 59;

/** Mono, averaged over the channels, capped at `maxSec`. */
export function toMono(channels: Float32Array[], rate: number, maxSec = MAX_IMPORT_SEC): Float32Array {
  const n = Math.min(channels[0]?.length ?? 0, Math.floor(maxSec * rate));
  const out = new Float32Array(n);
  for (const ch of channels) for (let i = 0; i < n; i++) out[i] += ch[i] / channels.length;
  return out;
}

/** Run the live tier over `x` as a capture does: the high-passed audio the engine leaves in its
 *  input buffer and the frame records it makes, from a fresh engine in its own instance. Only
 *  whole 10 ms frames are kept, with their audio. */
export async function liveTierOver(bytes: BufferSource, x: Float32Array, rate: number): Promise<{ audio: Float32Array; frames: Float32Array }> {
  const { instance } = await WebAssembly.instantiate(bytes, {});
  const ex = instance.exports as unknown as EngineExports;
  const e = ex.ew_init(rate, BLOCK);
  try {
    const n = Math.ceil(x.length / BLOCK) * BLOCK;
    const audio = new Float32Array(n);
    const frames: Float32Array[] = [];
    for (let pos = 0; pos < n; pos += BLOCK) {
      const t = readBufferTable(ex.memory.buffer as ArrayBuffer, ex.ew_buffers(e));
      const input = new Float32Array(ex.memory.buffer, t.input, BLOCK);
      input.fill(0);
      input.set(x.subarray(pos, Math.min(pos + BLOCK, x.length)));
      ex.ew_process(e, BLOCK);
      const t2 = readBufferTable(ex.memory.buffer as ArrayBuffer, ex.ew_buffers(e));
      audio.set(new Float32Array(ex.memory.buffer, t2.input, BLOCK), pos);
      const count = new Uint32Array(ex.memory.buffer, t2.framesCount, 1)[0];
      frames.push(new Float32Array(ex.memory.buffer, t2.frames, count * FRAME_STRIDE).slice());
    }
    const all = new Float32Array(frames.reduce((k, f) => k + f.length, 0));
    let k = 0;
    for (const f of frames) { all.set(f, k); k += f.length; }
    const nFrames = Math.min(all.length / FRAME_STRIDE, Math.floor(x.length / (rate / 100)));
    return { audio: audio.slice(0, nFrames * (rate / 100)), frames: all.slice(0, nFrames * FRAME_STRIDE) };
  } finally {
    ex.ew_free(e);
  }
}

/** The clip an imported take makes: placed from the start of the section, as a take sung from
 *  the downbeat with no round trip would be. */
export function importedClip(doc: Project, sectionId: string, layerId: string, take: Take, notes: TakeNote[], hits: TakeHit[]): Clip {
  const si = doc.sections.findIndex((s) => s.id === sectionId);
  return placeTake({
    doc, sectionId, layerId, take, startSample: 0, roundTripSamples: 0, notes, hits,
    mark: { sample: 0, section: si, pos: 0, unitsPerSample: 0, playing: false },
  });
}

/** The analysis an import needs: the worker client or AnalysisCore. */
export interface ImportAnalysis {
  secondPass(audio: Float32Array, sampleRate: number, frames: Float32Array, opts?: SecondPassOptions): SecondPass | Promise<SecondPass>;
  polyPass(audio: Float32Array, sampleRate: number, opts: PolyOptions): PolyPass | Promise<PolyPass>;
}

export type ImportPass = { poly: PolyPass; opts: PolyOptions } | { second: SecondPass };

/** Read an imported take for `layer`: the poly pass on a layer set to Poly, else the second pass,
 *  whose repaired pitch is written into `frames` as a capture's is. */
export async function importPass(an: ImportAnalysis, layer: Layer, audio: Float32Array, frames: Float32Array, rate: number,
                                 taught?: SecondPassOptions['taught']): Promise<ImportPass> {
  if (isPoly(layer)) {
    const opts = polyOptions(layer);
    return { poly: await an.polyPass(audio.slice(), rate, opts), opts };
  }
  const sp = await an.secondPass(audio.slice(), rate, frames.slice(), {
    kind: layer.kind, taught: layer.kind === 'beat' ? taught : undefined,
  });
  const n = frames.length / FRAME_STRIDE;
  for (let i = 0; i < Math.min(n, sp.pitch.length); i++) frames[i * FRAME_STRIDE] = sp.pitch[i];
  return { second: sp };
}
