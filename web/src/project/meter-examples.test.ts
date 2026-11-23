// Listening examples for stage R1 (docs/R1.md "Listening"): an audio file taken in as the app's
// Import take does (the live tier, then the second pass for notes, hits and anchors), placed from
// the section start on a layer that plays its raw input beside its synth voice, locked to Meter 1,
// and rendered by the wasm engine as written, at 1 2 3 and at 1 3 4 2. Runs only with
// R1_EXAMPLES=<dir>; R1_SONG=<wav> adds a song of your own (R1_SONG_BPM, default 116).
import { readFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { basename, resolve } from 'node:path';
import { describe, it } from 'vitest';
import { encodeWav } from '../common/wav';
import { uploadsToSong } from '../app/mix';
import { instantiateEngine } from '../m1/protocol';
import { renderSong } from '../m1/render-core';
import { AnalysisCore } from '../m2/analysis';
import { commands, newProject } from './commands';
import { Compiler } from './compile';
import { importedClip, liveTierOver } from './import-take';
import { METER1 } from './meter';
import { applyOps } from './patch';
import type { LayerKind, Project, Take } from './types';

const SR = 48000;
const out = process.env.R1_EXAMPLES;
const klont = readFileSync(new URL('../../public/wasm/klont.wasm', import.meta.url));
const analysis = readFileSync(new URL('../../public/wasm/analysis.wasm', import.meta.url));

/** A mono 16-bit or float WAV file at 48 kHz. */
function readWav(path: string): Float32Array {
  const b = readFileSync(path);
  const v = new DataView(b.buffer, b.byteOffset, b.byteLength);
  let at = 12, fmt = 1, ch = 1, bits = 16, rate = SR;
  while (at + 8 <= b.length) {
    const id = b.toString('ascii', at, at + 4), len = v.getUint32(at + 4, true);
    if (id === 'fmt ') {
      fmt = v.getUint16(at + 8, true); ch = v.getUint16(at + 10, true); rate = v.getUint32(at + 12, true); bits = v.getUint16(at + 22, true);
    } else if (id === 'data') {
      if (rate !== SR) throw new Error(`${path}: ${rate} Hz, not ${SR}`);
      const n = len / (bits / 8) / ch;
      const x = new Float32Array(n);
      for (let i = 0; i < n; i++) {
        const o = at + 8 + i * ch * (bits / 8);
        x[i] = fmt === 3 ? v.getFloat32(o, true) : v.getInt16(o, true) / 32768;
      }
      return x;
    }
    at += 8 + len + (len & 1);
  }
  throw new Error(`${path}: no data`);
}

async function example(name: string, path: string, kind: LayerKind, bpm: number): Promise<void> {
  // Whole bars of 4 only, so the take is exactly one pass of the loop: a longer take wraps its
  // notes into the loop, which is not what these examples are about.
  const whole = readWav(path);
  const beats = Math.max(4, Math.floor(((whole.length / SR) * bpm) / 60 / 4) * 4);
  const x = whole.slice(0, Math.round((beats * 60 * SR) / bpm));
  const imp = await liveTierOver(klont, x, SR);
  const sp = AnalysisCore.fromBytes(analysis).secondPass(imp.audio, SR, imp.frames, { kind });
  const run = (d: Project, ops: ReturnType<typeof commands.setMeter>) => applyOps(d, ops).doc;
  let d = newProject(name);
  d = run(d, commands.setSection(d, { id: d.sections[0].id, beats, bpm, repeats: 1 }));
  const l = d.layers.find((y) => (kind === 'beat' ? y.kind === 'beat' : y.kind !== 'beat'))!;
  for (const o of d.layers.filter((y) => y.id !== l.id)) d = run(d, commands.removeLayer(d, { id: o.id }));
  d = run(d, commands.addChannelFx(d, { layer: l.id, type: 'input' }));
  d = run(d, commands.setValues(d, { layer: l.id, state: d.layers[0].states[0].id, values: { 'input1.mix': 1 } }));
  const nFrames = imp.frames.length / 25;
  const take: Take = { id: 'take', audio: 'a', frames: 'f', rate: SR, latencyMs: 0, frameCount: nFrames, kind,
    refCents: sp.refCents, root: sp.root, scale: sp.scale, bpm, anchors: 'an', anchorsVersion: 1 };
  d = run(d, commands.addTake(d, { take }));
  d = run(d, commands.setClip(d, { section: d.sections[0].id, layer: l.id, clip: importedClip(d, d.sections[0].id, l.id, take, sp.notes, sp.hits) }));
  d = run(d, commands.lockClips(d, { layer: l.id, meter: METER1 }));
  const renders: [string, number[] | null][] = [['0-as-written', null], ['1-123', [1, 2, 3]], ['2-1342', [1, 3, 4, 2]]];
  const outs: Float32Array[] = [];
  for (const [, play] of renders) {
    const doc = run(d, commands.setMeterPlay(d, { section: d.sections[0].id, meter: METER1, play }));
    const ups = new Compiler().compile(doc, () => imp.frames, { audioFor: () => imp.audio, anchorsFor: () => sp.anchors });
    const { ex } = instantiateEngine(klont, SR, 4096);
    const res = await renderSong(ex, uploadsToSong(ups), { maxSeconds: 30 });
    outs.push(res.left);
  }
  // One gain per example, from its as-written render, so the three can be compared by ear.
  const peak = outs[0].reduce((m, v) => Math.max(m, Math.abs(v)), 1e-9);
  mkdirSync(out!, { recursive: true });
  renders.forEach(([suffix], i) => {
    const y = outs[i].map((v) => (v * 0.7) / peak);
    void encodeWav(y, SR).arrayBuffer().then((buf) => writeFileSync(resolve(out!, `${name}-${suffix}.wav`), Buffer.from(buf)));
  });
  console.log(`${name}: ${basename(path)}, ${beats} beats at ${bpm} bpm, ${sp.notes.length} notes, ${sp.hits.length} hits, ${sp.anchors ? sp.anchors.length / 3 : 0} anchors`);
}

describe.skipIf(!out)('R1 listening examples', () => {
  const docs = resolve(import.meta.dirname, '../../../docs/q2-examples');
  it('a drum take', () => example('drums', resolve(docs, 'drums-0-as-played.wav'), 'beat', 100), 60_000);
  it('a sung take', () => example('sung', resolve(docs, 'sung-0-as-played.wav'), 'melody', 100), 60_000);
  it.skipIf(!process.env.R1_SONG)('a song', () => example('song', process.env.R1_SONG!, 'melody', Number(process.env.R1_SONG_BPM ?? 116)), 60_000);
});
