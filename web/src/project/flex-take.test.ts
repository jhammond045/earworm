// Flex quantize on a sung take, end to end in Node (docs/quant.md, Q1 check "a flexed sung take:
// every note's curve starts within 1 ms of its moved note"). A singer sings eight detached notes
// loosely around the 8ths of a 120 bpm loop; the live engine captures the mic and frames, the
// second pass (analysis.wasm) finds the notes and the flex anchors, placeTake puts the take on the
// timeline, and the clip gets Feel at 8ths. The compiled track blob is read back as the engine
// reads it: each moved note's warp map must give the take second where that note was sung.
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { neutralSet } from '../gen/params';
import * as pr from '../m1/protocol';
import { encodeMessage, instantiateEngine, readBufferTable } from '../m1/protocol';
import { AnalysisCore } from '../m2/analysis';
import { classicPatch, valuesFromVocab } from '../synth/classic';
import { Compiler } from './compile';
import { flexClip, quantizeFlex, warpToTake } from './flex';
import { clockMark, placeTake } from './overdub';
import type { Flex, Layer, MelodyClip, Project, Take } from './types';

const SR = 48000;
const BLOCK = 128;
const STRIDE = 25;
const FRAME = SR / 100;
const UPS = (120 * 960) / 60;
const klont = readFileSync(new URL('../../public/wasm/klont.wasm', import.meta.url));
const analysis = readFileSync(new URL('../../public/wasm/analysis.wasm', import.meta.url));

/** How far each note is sung from its 8th, ms. */
const OFFSETS = [18, -25, 30, 12, -15, 35, -8, 22];

function project(): Project {
  const neutral = Array.from(neutralSet());
  const layer: Layer = {
    id: 'mel', name: 'mel', kind: 'melody', color: 0, level: 0, pan: 0, mute: false, follow: 1,
    tune: { snap: 0, speedMs: 0, slides: 0, vibrato: 1, levelOut: 0, root: 0, scale: 0, refCents: 0, octave: 0 },
    depth: { loud: 1, open: 0.5, hard: 0.5, breath: 0.5, mouth: 1 }, patch: classicPatch('melody'), kit: [],
    states: [{ id: 's', name: 's', values: valuesFromVocab('melody', neutral, []), macros: new Array(8).fill(0.5), influenceAmount: 0 }],
  };
  return {
    schema: 5, id: 'p', name: 'p', loop: true, takes: {}, layers: [layer],
    sections: [{ id: 's', name: 'A', feel: 'grid', beats: 8, seconds: 0, bpm: 120, repeats: 1, rampMs: 0, clips: {} }],
  };
}

/** Eight 180 ms sung notes, note k at 8th k + 1 of the transport (0.25 s apart) plus its offset. */
function singer(total: number, at0: number): Float32Array {
  const x = new Float32Array(total);
  const pitches = [220, 247, 262, 294, 330, 294, 262, 247];
  OFFSETS.forEach((ms, k) => {
    const hz = pitches[k];
    const a = at0 + Math.round((0.25 * (k + 1) + ms / 1000) * SR);
    const n = Math.round(0.18 * SR);
    for (let h = 1; h * hz < SR * 0.45; h++) {
      const f = h * hz;
      const bell = (fc: number, bw: number) => Math.exp(-((f - fc) ** 2) / (2 * bw * bw));
      const amp = (0.25 / h) * (0.2 + bell(700, 150) + 0.6 * bell(1100, 200));
      const w = (2 * Math.PI * f) / SR;
      for (let i = 0; i < n; i++) x[a + i] += amp * Math.min(1, i / (0.008 * SR), (n - i) / (0.02 * SR)) * Math.sin(w * i);
    }
  });
  return x;
}

/** The first clip's events (t) and warp anchors (take seconds, clip time) of a v1 to v3 track blob. */
function readTrack(blob: ArrayBuffer): { events: number[]; warp: { takeSec: number; clipT: number }[] } {
  const u = new Uint32Array(blob), f = new Float32Array(blob);
  const [magic, nSec, nClips, nEvents] = [u[0], u[1], u[2], u[3]];
  const clipWords = magic === 0x33545745 ? 16 : magic === 0x32545745 ? 12 : 8;
  const c = 8 + nSec * 8;
  const evBase = c + nClips * clipWords;
  const warpBase = evBase + nEvents * 16;
  const events = Array.from({ length: u[c + 4] }, (_, i) => f[evBase + (u[c + 3] + i) * 16]);
  const warp = Array.from({ length: u[c + 7] }, (_, i) => ({ takeSec: f[warpBase + 2 * (u[c + 6] + i)], clipT: f[warpBase + 2 * (u[c + 6] + i) + 1] }));
  return { events, warp };
}

describe('flex on a sung take (Q1)', () => {
  it('moves each note onto the 8ths and its curve still starts where the note was sung', () => {
    const { ex, e } = instantiateEngine(klont, SR, 4096);
    const mem = () => ex.memory.buffer as ArrayBuffer;
    const tb = () => readBufferTable(mem(), ex.ew_buffers(e));
    const send = (msgs: pr.Msg[]) => {
      const t = tb();
      const v = new DataView(mem(), t.control, pr.CONTROL_CAP * pr.MSG_BYTES);
      msgs.forEach((m, i) => encodeMessage(v, i * pr.MSG_BYTES, m[0], m[1], m[2], m[3], m[4], m[5] ?? 0));
      new Uint32Array(mem(), t.controlCount, 1)[0] = msgs.length;
    };
    const doc0 = project();
    for (const up of new Compiler().compile(doc0, () => null)) {
      const t = tb();
      new Uint8Array(mem(), t.staging, up.blob.byteLength).set(new Uint8Array(up.blob));
      expect(ex.ew_load(e, up.slot, up.kind, t.staging, up.blob.byteLength), up.what).toBe(pr.LOAD_OK);
    }
    // Arm and play together; the take starts with the first block.
    send([[pr.MSG_PLAY, 0, 0, 0, 0], [pr.MSG_ARM, 0, 0, 0, 1]]);
    const total = Math.round(2.6 * SR / BLOCK) * BLOCK;
    const x = singer(total, 0);
    const mic = new Float32Array(total);
    const frames: number[] = [];
    let meters = new Float32Array(pr.METER_LEN);
    for (let pos = 0; pos < total; pos += BLOCK) {
      new Float32Array(mem(), tb().input, BLOCK).set(x.subarray(pos, pos + BLOCK));
      ex.ew_process(e, BLOCK);
      const t = tb();
      mic.set(new Float32Array(mem(), t.input, BLOCK), pos);
      frames.push(...new Float32Array(mem(), t.frames, new Uint32Array(mem(), t.framesCount, 1)[0] * STRIDE));
      meters = new Float32Array(mem(), t.meter, pr.METER_LEN).slice();
    }
    ex.ew_free(e);

    const nFrames = frames.length / STRIDE;
    const rec = Float32Array.from(frames);
    const sp = AnalysisCore.fromBytes(analysis).secondPass(mic.slice(0, nFrames * FRAME), SR, rec, { kind: 'melody' });
    expect(sp.notes.length).toBe(8);
    expect(sp.anchors.length / 3).toBe(8);
    const take: Take = { id: 'take', audio: 'a', frames: 'f', rate: SR, latencyMs: 0, frameCount: nFrames, kind: 'melody',
      refCents: sp.refCents, root: sp.root, scale: sp.scale, bpm: null, anchors: 'anchors-take', anchorsVersion: 1 };
    // The engine's transport started with the first sample processed.
    const clip = placeTake({ doc: doc0, sectionId: 's', layerId: 'mel', take, startSample: clockMark(meters).sample - total, mark: clockMark(meters), roundTripSamples: 0, notes: sp.notes, hits: [] }) as MelodyClip;
    const flexed: MelodyClip = { ...clip, flex: quantizeFlex(undefined, 480) };
    const doc: Project = { ...doc0, takes: { take }, sections: [{ ...doc0.sections[0], clips: { mel: flexed } }] };

    const track = new Compiler().compile(doc, (t) => (t.id === 'take' ? rec : null), { anchorsFor: (t) => (t.id === 'take' ? sp.anchors : null) })
      .find((up) => up.what === 'track')!;
    const blob = readTrack(track.blob);
    const moved = flexClip(flexed as MelodyClip & { flex: Flex }, sp.anchors, UPS).notes!;
    const off8 = (t: number) => ((((t + 240) % 480) + 480) % 480 - 240) / UPS * 1000;
    const rows = clip.notes.map((n, i) => {
      const played = moved[i];
      const at = Math.fround(played.t);
      expect(blob.events).toContain(at);
      const sung = warpToTake(clip.warp, n.t, 1 / UPS);
      const read = warpToTake(blob.warp, at, 1 / UPS);
      return { before: off8(n.t), after: off8(played.t), curveMs: (read - sung) * 1000 };
    });
    console.log(`flexed sung take, 8 notes at 8ths: off the grid before ${rows.map((r) => r.before.toFixed(1)).join(', ')} ms; after ${rows.map((r) => r.after.toFixed(2)).join(', ')} ms; curve start against the sung start ${rows.map((r) => r.curveMs.toFixed(3)).join(', ')} ms`);
    for (const r of rows) {
      expect(Math.abs(r.curveMs)).toBeLessThanOrEqual(1);
      expect(Math.abs(r.after)).toBeLessThan(1);
    }
    expect(Math.max(...rows.map((r) => Math.abs(r.before)))).toBeGreaterThan(20);
  });
});
