// The whole capture path the M2 page runs, without a browser or a mic:
// a sung phrase goes through the live engine (klont.wasm, layer 0 armed),
// its high-passed mic signal and frame records go through the second pass
// (analysis.wasm), and the result is built into layer, curve and track blobs
// and played back by a fresh engine. Checks the notes found and that the
// loop plays them at the right pitch.
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import * as pr from '../m1/protocol';
import { encodeMessage, instantiateEngine, readBufferTable } from '../m1/protocol';
import { encodeLayer } from '../m1/layer';
import { AnalysisCore } from './analysis';
import { buildCurveBlob, buildTrackBlob, loopSeconds } from './take';

const SR = 48000;
const BLOCK = 128;
const STRIDE = 25;
const klont = readFileSync(new URL('../../public/wasm/klont.wasm', import.meta.url));
const analysis = readFileSync(new URL('../../public/wasm/analysis.wasm', import.meta.url));

const centsOf = (hz: number) => 1200 * Math.log2(hz / 16.351597831287414);

/** Three sung notes with formants, a short gap after each. */
function phrase(): { x: Float32Array; notes: { start: number; hz: number }[] } {
  const notes = [{ hz: 220, start: 0.3 }, { hz: 262, start: 0.95 }, { hz: 196, start: 1.6 }];
  const x = new Float32Array(Math.round(2.4 * SR));
  for (const { hz, start } of notes) {
    const a = Math.round(start * SR), n = Math.round(0.5 * SR);
    for (let k = 1; k * hz < SR * 0.45; k++) {
      const f = k * hz;
      const bell = (fc: number, bw: number) => Math.exp(-((f - fc) ** 2) / (2 * bw * bw));
      const amp = (0.25 / k) * (0.2 + bell(700, 150) + 0.6 * bell(1100, 200));
      const w = (2 * Math.PI * f) / SR;
      for (let i = 0; i < n; i++) {
        const env = Math.min(1, i / (0.01 * SR), (n - i) / (0.02 * SR));
        x[a + i] += amp * env * Math.sin(w * i);
      }
    }
  }
  return { x, notes };
}

function engine() {
  const { ex, e } = instantiateEngine(klont, SR, 4096);
  const t = () => readBufferTable(ex.memory.buffer as ArrayBuffer, ex.ew_buffers(e));
  const send = (msgs: pr.Msg[]) => {
    const tb = t();
    const v = new DataView(ex.memory.buffer as ArrayBuffer, tb.control, pr.CONTROL_CAP * pr.MSG_BYTES);
    msgs.forEach((m, i) => encodeMessage(v, i * pr.MSG_BYTES, m[0], m[1], m[2], m[3], m[4], m[5] ?? 0));
    new Uint32Array(ex.memory.buffer as ArrayBuffer, tb.controlCount, 1)[0] = msgs.length;
  };
  const load = (kind: number, slot: number, blob: ArrayBuffer) => {
    const tb = t();
    new Uint8Array(ex.memory.buffer as ArrayBuffer, tb.staging, blob.byteLength).set(new Uint8Array(blob));
    return ex.ew_load(e, slot, kind, tb.staging, blob.byteLength);
  };
  return { ex, e, t, send, load };
}

/** Autocorrelation f0 in Hz, 0 when unclear. */
function f0(x: Float32Array): number {
  let best = 0, bestLag = 0, e0 = 0;
  for (const v of x) e0 += v * v;
  if (e0 < 1e-9) return 0;
  for (let lag = Math.floor(SR / 1000); lag < Math.floor(SR / 70); lag++) {
    let c = 0;
    for (let i = 0; i + lag < x.length; i++) c += x[i] * x[i + lag];
    if (c > best) { best = c; bestLag = lag; }
  }
  return best > 0.3 * e0 ? SR / bestLag : 0;
}

describe('capture pipeline', () => {
  it('live capture, second pass and playback agree on the notes', () => {
    const { x, notes } = phrase();

    // 1. Live: layer 0 armed, the frame records and the high-passed mic kept as the page keeps them.
    const live = engine();
    live.send([[pr.MSG_ARM, 0, 0, 0, 1]]);
    const mic = new Float32Array(x.length);
    const frames: number[] = [];
    for (let pos = 0; pos < x.length; pos += BLOCK) {
      const n = Math.min(BLOCK, x.length - pos);
      const tb = live.t();
      const input = new Float32Array(live.ex.memory.buffer as ArrayBuffer, tb.input, n);
      input.set(x.subarray(pos, pos + n));
      live.ex.ew_process(live.e, n);
      const tb2 = live.t();
      mic.set(new Float32Array(live.ex.memory.buffer as ArrayBuffer, tb2.input, n), pos);
      const count = new Uint32Array(live.ex.memory.buffer as ArrayBuffer, tb2.framesCount, 1)[0];
      frames.push(...new Float32Array(live.ex.memory.buffer as ArrayBuffer, tb2.frames, count * STRIDE));
    }
    live.ex.ew_free(live.e);
    const nFrames = frames.length / STRIDE;
    const L = SR / 100;
    const rec = Float32Array.from(frames);
    const audio = mic.subarray(0, nFrames * L);

    // 2. Second pass and tuning.
    const core = AnalysisCore.fromBytes(analysis);
    const sp = core.secondPass(audio, SR, rec, { kind: 'melody' });
    expect(sp.kind).toBe('melody');
    expect(sp.notes.length).toBe(3);
    sp.notes.forEach((nt, i) => {
      expect(Math.abs(nt.t - notes[i].start)).toBeLessThan(0.05);
      expect(Math.abs(nt.cents - centsOf(notes[i].hz))).toBeLessThan(30);
    });
    const { tuned } = core.tune(rec, sp.pitch, {
      snap: 1, speedMs: 0, slides: 0, vibrato: 1, root: sp.root, scale: sp.scale, refCents: sp.refCents, levelOut: 0,
    });

    // 3. Playback on a fresh engine: layer, curve, track, play.
    const play = engine();
    expect(play.load(pr.LOAD_LAYER, 0, encodeLayer({ kind: 'melody', base: { 'env.sustain': 1, 'env.release': 0.03 } }))).toBe(pr.LOAD_OK);
    expect(play.load(pr.LOAD_CURVE, 0, buildCurveBlob(rec, tuned))).toBe(pr.LOAD_OK);
    expect(play.load(pr.LOAD_TRACK, 0, buildTrackBlob({ kind: 'melody', layer: 0, curveSlot: 0, sp, frameCount: nFrames })))
      .toBe(pr.LOAD_OK);
    play.send([[pr.MSG_PLAY, 0, 0, 0, 0]]);
    const loop = loopSeconds(sp, nFrames);
    const out = new Float32Array(Math.round(loop * SR));
    for (let pos = 0; pos < out.length; pos += 1024) {
      const n = play.ex.ew_render(play.e, Math.min(1024, out.length - pos));
      out.set(new Float32Array(play.ex.memory.buffer as ArrayBuffer, play.t().outputL, n), pos);
    }
    play.ex.ew_free(play.e);
    for (const nt of notes) {
      const mid = Math.round((nt.start + 0.25) * SR);
      const hz = f0(out.subarray(mid, mid + 2048));
      const err = Math.abs(centsOf(hz) - centsOf(nt.hz));
      console.log(`note ${nt.hz} Hz: played ${hz.toFixed(1)} Hz (${err.toFixed(0)} cents off)`);
      expect(err).toBeLessThan(50);
    }
  });
});
