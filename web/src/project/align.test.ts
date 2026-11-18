// M3 overdub alignment, end to end in Node with the real engine and second pass.
//
// A click plays (beat layer, a hit on every beat of a 4-beat, 120 bpm loop). A synthetic singer
// sings a short note on every beat exactly as heard, and what reaches the mic is that voice one
// round trip (30 ms) later. A melody layer is armed, the mic and frames are captured the way the
// M2 page does, the second pass runs, and placeTake puts the notes on the timeline. With the
// round trip passed in, notes should land on the beats; with 0 they should be about 30 ms late.
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { COUNT, neutralSet } from '../gen/params';
import * as pr from '../m1/protocol';
import { encodeMessage, instantiateEngine, readBufferTable } from '../m1/protocol';
import { AnalysisCore, type SecondPass } from '../m2/analysis';
import { median } from '../m2/take';
import { Compiler } from './compile';
import { clockMark, placeTake } from './overdub';
import type { Layer, MelodyClip, Project, Take } from './types';

const SR = 48000;
const BLOCK = 128;
const STRIDE = 25;
const FRAME = SR / 100;
// 27.3 ms: deliberately not a multiple of the 10 ms frame, so onset timing error shows up.
const RT = Math.round(0.0273 * SR); // 1310 samples
const TICKS_PER_SAMPLE = (120 * 960) / (60 * SR);
const MS_PER_TICK = 1000 / ((120 * 960) / 60);
const klont = readFileSync(new URL('../../public/wasm/klont.wasm', import.meta.url));
const analysis = readFileSync(new URL('../../public/wasm/analysis.wasm', import.meta.url));

const neutral = Array.from(neutralSet());
const tune = { snap: 1, speedMs: 0, slides: 0 as const, vibrato: 1, levelOut: 0, root: 0, scale: 0, refCents: 5700, octave: 0 };
function layer(id: string, kind: Layer['kind']): Layer {
  return {
    id, name: id, kind, color: 0, level: 0, pan: 0, mute: false, tune, follow: 1,
    depth: { loud: 1, open: 0.5, hard: 0.5, breath: 0.5, mouth: 1 },
    states: [{ id: 's', name: 's', base: neutral.slice(0, COUNT), macros: [0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5], influenceAmount: 0 }],
    kit: kind === 'beat' ? [2400, 4800, 6000, 7200].map((cents) => ({ params: neutral.slice(), cents })) : [],
  };
}
const shape = { attack: 0.002, burst: 0.5, cents: null, bend: 0, hold: 0.03, vowel: 0, tone: 0.5, endStop: 0, endClick: 0 };

function project(): Project {
  const hits = [0, 1, 2, 3].map((b) => ({ id: `h${b}`, t: b * 960, dur: 100, slot: 0, strength: 1, shape }));
  return {
    schema: 1, id: 'p', name: 'p', loop: true, takes: {},
    layers: [layer('mel', 'melody'), layer('click', 'beat')],
    sections: [{ id: 's', name: 'A', feel: 'grid', beats: 4, seconds: 0, bpm: 120, repeats: 1, rampMs: 0,
      clips: { click: { kind: 'beat', take: '', takes: [], loopLen: 3840, timeSnap: 0, hits } } }],
  };
}

/** Sung notes on every beat of the transport as heard (beat k is at engine sample 24000 k), reaching the mic `rt` later. */
function singer(totalSamples: number, rt: number): Float32Array {
  const x = new Float32Array(totalSamples);
  const pitches = [220, 262, 196, 247];
  for (let k = 0; 24000 * k + rt + 0.3 * SR < totalSamples; k++) {
    const hz = pitches[k % 4];
    const a = 24000 * k + rt;
    const n = Math.round(0.3 * SR);
    for (let h = 1; h * hz < SR * 0.45; h++) {
      const f = h * hz;
      const bell = (fc: number, bw: number) => Math.exp(-((f - fc) ** 2) / (2 * bw * bw));
      const amp = (0.25 / h) * (0.2 + bell(700, 150) + 0.6 * bell(1100, 200));
      const w = (2 * Math.PI * f) / SR;
      for (let i = 0; i < n; i++) {
        const env = Math.min(1, i / (0.01 * SR), (n - i) / (0.02 * SR));
        x[a + i] += amp * env * Math.sin(w * i);
      }
    }
  }
  return x;
}

function run(roundTripSamples: number) {
  const { ex, e } = instantiateEngine(klont, SR, 4096);
  const mem = () => ex.memory.buffer as ArrayBuffer;
  const tb = () => readBufferTable(mem(), ex.ew_buffers(e));
  const send = (msgs: pr.Msg[]) => {
    const t = tb();
    const v = new DataView(mem(), t.control, pr.CONTROL_CAP * pr.MSG_BYTES);
    msgs.forEach((m, i) => encodeMessage(v, i * pr.MSG_BYTES, m[0], m[1], m[2], m[3], m[4], m[5] ?? 0));
    new Uint32Array(mem(), t.controlCount, 1)[0] = msgs.length;
  };

  // Load the document through the compiler.
  const doc = project();
  for (const u of new Compiler().compile(doc, () => null)) {
    const t = tb();
    new Uint8Array(mem(), t.staging, u.blob.byteLength).set(new Uint8Array(u.blob));
    expect(ex.ew_load(e, u.slot, u.kind, t.staging, u.blob.byteLength), u.what).toBe(pr.LOAD_OK);
  }
  send([[pr.MSG_PLAY, 0, 0, 0, 0]]);

  const total = 240000;
  const armAt = 898 * BLOCK; // after the singer's 5th note ends, before the 6th starts
  const x = singer(total, RT); // the singer's delay is fixed; only the compensation changes
  const mic = new Float32Array(total);
  const frames: number[] = [];
  let f0 = -1;
  let meters = new Float32Array(pr.METER_LEN);
  for (let pos = 0; pos < total; pos += BLOCK) {
    const n = Math.min(BLOCK, total - pos);
    if (pos === armAt) {
      send([[pr.MSG_ARM, 0, 0, 0, 1]]);
      f0 = frames.length / STRIDE;
    }
    new Float32Array(mem(), tb().input, n).set(x.subarray(pos, pos + n));
    ex.ew_process(e, n);
    const t = tb();
    mic.set(new Float32Array(mem(), t.input, n), pos);
    const count = new Uint32Array(mem(), t.framesCount, 1)[0];
    frames.push(...new Float32Array(mem(), t.frames, count * STRIDE));
    meters = new Float32Array(mem(), t.meter, pr.METER_LEN).slice();
  }
  ex.ew_free(e);

  const nFrames = frames.length / STRIDE - f0;
  const startSample = f0 * FRAME;
  const rec = Float32Array.from(frames.slice(f0 * STRIDE));
  const audio = mic.slice(startSample, startSample + nFrames * FRAME);
  const sp: SecondPass = AnalysisCore.fromBytes(analysis).secondPass(audio, SR, rec, { kind: 'melody' });

  const mark = clockMark(meters);
  const take: Take = { id: 'take', audio: 'a', frames: 'f', rate: SR, latencyMs: (1000 * roundTripSamples) / SR, frameCount: nFrames,
    kind: 'melody', refCents: sp.refCents, root: sp.root, scale: sp.scale, bpm: null };
  const clip = placeTake({ doc, sectionId: 's', layerId: 'mel', take, startSample, mark, roundTripSamples, notes: sp.notes, hits: [] }) as MelodyClip;
  return { clip, mark, startSample, notes: sp.notes };
}

/** Signed distance of a tick position from the nearest beat, in ms. */
const beatErrorMs = (ticks: number) => (((ticks + 480) % 960 + 960) % 960 - 480) * MS_PER_TICK;

describe('overdub alignment (M3)', () => {
  const comp = run(RT);
  const raw = run(0);

  it('the clock counts the samples processed and the transport sits where the clock says', () => {
    expect(comp.mark.sample).toBe(240000);
    expect(comp.mark.playing).toBe(true);
    expect(comp.mark.unitsPerSample).toBeCloseTo(TICKS_PER_SAMPLE, 6);
    expect(Math.abs(comp.mark.pos - ((240000 * TICKS_PER_SAMPLE) % 3840))).toBeLessThan(1);
  });

  it('with the round trip subtracted, notes land on the beats', () => {
    const errs = comp.clip.notes.map((n) => beatErrorMs(n.t));
    console.log(`round trip ${((1000 * RT) / SR).toFixed(1)} ms compensated: ${comp.clip.notes.length} notes, per-note error ms: ${errs.map((v) => v.toFixed(1)).join(', ')}; median ${median(errs).toFixed(1)} ms`);
    expect(comp.clip.notes.length).toBe(5);
    for (const v of errs) expect(Math.abs(v)).toBeLessThan(15);
  });

  it('with no compensation the notes are about one round trip late', () => {
    const errs = raw.clip.notes.map((n) => beatErrorMs(n.t));
    console.log(`round trip 0 (no compensation): ${raw.clip.notes.length} notes, per-note error ms: ${errs.map((v) => v.toFixed(1)).join(', ')}; median ${median(errs).toFixed(1)} ms`);
    expect(raw.clip.notes.length).toBe(5);
    expect(median(errs)).toBeGreaterThan(20);
    expect(median(errs)).toBeLessThan(45);
    // The difference between the two is the round trip itself, exactly.
    const d = raw.clip.notes.map((n, i) => beatErrorMs(n.t) - beatErrorMs(comp.clip.notes[i].t));
    for (const v of d) expect(v).toBeCloseTo((1000 * RT) / SR, 3);
  });
});
