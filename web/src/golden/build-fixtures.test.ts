// Builds the golden-render fixtures from the engine and compiler as they were before the
// modular synth (docs/synth.md, stage S1). Run once with GOLDEN_WRITE=1; the files it writes
// under fixtures/ are the record of today's sound, and golden.test.ts compares the current
// engine against them.
//
// Five projects are made the way the app makes them: document commands through a Store, and
// takes captured by the live engine from a synthetic voice, run through the second pass and
// placed with placeTake, as app/capture.ts does. For each project the fixture keeps:
//   doc.json         the schema 3 document
//   uploads/*.bin    every blob today's compiler sends for it (the reference render input)
//   uploads.json     kind, slot and file of each upload, in send order
//   curves/<id>.f32  the tuned curve of each take, as m3/sync.ts builds it
//   blobs/<ref>.f32  document blobs the compiler reads (take frames, Influence sources)
// The three app demos and the M1 demo song are kept the same way (the demo song as its
// committed blobs in web/public/m1/demo).
import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, it } from 'vitest';
import { DEMOS } from '../app/demos';
import { uploadsToSong } from '../app/mix';
import { soundToSet } from '../m1/layer';
import * as pr from '../m1/protocol';
import { encodeMessage, instantiateEngine, readBufferTable } from '../m1/protocol';
import { renderSong } from '../m1/render-core';
import { AnalysisCore, FRAME_STRIDE } from '../m2/analysis';
import { BEAT_KITS, MELODY_SOUNDS } from '../m2/sounds';
import { withTuned } from '../m2/take';
import { newProject } from '../project/commands';
import { Compiler } from '../project/compile';
import { placeTake } from '../project/overdub';
import { Store } from '../project/store';
import { PPQ, type Layer, type Project, type Take } from '../project/types';

const SR = 48000;
const BLOCK = 128;
const here = import.meta.dirname;
const klont = readFileSync(resolve(here, '../../public/wasm/klont.wasm'));
const analysis = readFileSync(resolve(here, '../../public/wasm/analysis.wasm'));

// ---- synthetic performances

/** A seeded generator, so the fixtures can be rebuilt bit for bit. */
function rng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

interface SungNote { start: number; len: number; hz: number; vowel: 'a' | 'o' | 'i'; breath?: number; swell?: boolean }

const FORMANTS = { a: [[750, 120, 1], [1150, 160, 0.6]], o: [[450, 90, 1], [800, 120, 0.5]], i: [[300, 70, 0.7], [2300, 200, 0.6]] } as const;

/** Sung notes with formants, vibrato, a loudness swell and some breath noise. */
function sing(notes: SungNote[], secs: number, seed: number): Float32Array {
  const x = new Float32Array(Math.round(secs * SR));
  const r = rng(seed);
  for (const nt of notes) {
    const a = Math.round(nt.start * SR), n = Math.round(nt.len * SR);
    let ph = 0;
    for (let i = 0; i < n && a + i < x.length; i++) {
      const t = i / SR;
      const vib = 1 + 0.006 * Math.sin(2 * Math.PI * 5.5 * t) * Math.min(1, t / 0.3);
      const f0 = nt.hz * vib;
      ph += (2 * Math.PI * f0) / SR;
      const env = Math.min(1, i / (0.02 * SR), (n - i) / (0.04 * SR)) * (nt.swell ? 0.3 + 0.7 * Math.min(1, t / (nt.len * 0.7)) : 1);
      let s = 0;
      for (let k = 1; k * f0 < SR * 0.45 && k < 40; k++) {
        const f = k * f0;
        let g = 0.05;
        for (const [fc, bw, gain] of FORMANTS[nt.vowel]) g += gain * Math.exp(-((f - fc) ** 2) / (2 * bw * bw));
        s += ((0.25 / k) * g) * Math.sin(k * ph);
      }
      s += (nt.breath ?? 0) * (r() * 2 - 1) * 0.15;
      x[a + i] += env * s;
    }
  }
  return x;
}

/** Beatbox-like hits: kick (falling sine), snare (noise and tone), hat (bright noise). */
function beatbox(hits: { t: number; kind: 'k' | 's' | 'h' }[], secs: number, seed: number): Float32Array {
  const x = new Float32Array(Math.round(secs * SR));
  const r = rng(seed);
  for (const h of hits) {
    const a = Math.round(h.t * SR);
    const n = Math.round((h.kind === 'h' ? 0.06 : 0.18) * SR);
    let ph = 0, lp = 0, prev = 0;
    for (let i = 0; i < n && a + i < x.length; i++) {
      const t = i / SR;
      let s = 0;
      if (h.kind === 'k') {
        ph += (2 * Math.PI * (60 + 120 * Math.exp(-t / 0.03))) / SR;
        s = 0.8 * Math.sin(ph) * Math.exp(-t / 0.08);
      } else if (h.kind === 's') {
        ph += (2 * Math.PI * 190) / SR;
        const w = r() * 2 - 1;
        lp += 0.5 * (w - lp);
        s = (0.35 * Math.sin(ph) + 0.5 * lp) * Math.exp(-t / 0.05);
      } else {
        const w = r() * 2 - 1;
        s = 0.4 * (w - prev) * Math.exp(-t / 0.015);
        prev = w;
      }
      x[a + i] += s;
    }
  }
  return x;
}

// ---- capture, as app/capture.ts does it

interface Captured { mic: Float32Array; frames: Float32Array; frameCount: number }

/** Run `x` through a live engine with `layerDef` armed on layer 0; keep the high-passed mic and frame records. */
function liveCapture(x: Float32Array, beat: boolean): Captured {
  const { ex, e } = instantiateEngine(klont, SR, 4096);
  const tb = () => readBufferTable(ex.memory.buffer as ArrayBuffer, ex.ew_buffers(e));
  const send = (msgs: pr.Msg[]) => {
    const t = tb();
    const v = new DataView(ex.memory.buffer as ArrayBuffer, t.control, pr.CONTROL_CAP * pr.MSG_BYTES);
    msgs.forEach((m, i) => encodeMessage(v, i * pr.MSG_BYTES, m[0], m[1], m[2], m[3], m[4], m[5] ?? 0));
    new Uint32Array(ex.memory.buffer as ArrayBuffer, t.controlCount, 1)[0] = msgs.length;
  };
  if (beat) send([[pr.MSG_SET_LAYER, 0, 0, pr.LAYER_KIND, pr.KIND_BEAT]]);
  else send([]);
  ex.ew_process(e, 0);
  send([[pr.MSG_ARM, 0, 0, 0, 1]]);
  const mic = new Float32Array(x.length);
  const frames: number[] = [];
  for (let pos = 0; pos < x.length; pos += BLOCK) {
    const n = Math.min(BLOCK, x.length - pos);
    new Float32Array(ex.memory.buffer as ArrayBuffer, tb().input, n).set(x.subarray(pos, pos + n));
    ex.ew_process(e, n);
    const t = tb();
    mic.set(new Float32Array(ex.memory.buffer as ArrayBuffer, t.input, n), pos);
    const count = new Uint32Array(ex.memory.buffer as ArrayBuffer, t.framesCount, 1)[0];
    frames.push(...new Float32Array(ex.memory.buffer as ArrayBuffer, t.frames, count * FRAME_STRIDE));
  }
  ex.ew_free(e);
  const frameCount = frames.length / FRAME_STRIDE;
  return { mic: mic.subarray(0, frameCount * (SR / 100)), frames: Float32Array.from(frames), frameCount };
}

interface Built { doc: Project; blobs: Record<string, Float32Array> }

let takeNo = 0;

/** Capture `x` into `layerId` at the start of `sectionId` and place it, as app/capture.ts does. */
function captureInto(store: Store, blobs: Record<string, Float32Array>, sectionId: string, layerId: string, x: Float32Array): void {
  const layer = store.doc.layers.find((l) => l.id === layerId)!;
  const beat = layer.kind === 'beat';
  const cap = liveCapture(x, beat);
  const core = AnalysisCore.fromBytes(analysis);
  const sp = core.secondPass(cap.mic.slice(), SR, cap.frames.slice(), { kind: layer.kind });
  const frames = cap.frames;
  for (let i = 0; i < Math.min(cap.frameCount, sp.pitch.length); i++) frames[i * FRAME_STRIDE] = sp.pitch[i];
  const id = `take_golden${++takeNo}`;
  const take: Take = {
    id, audio: `audio-${id}`, frames: `frames-${id}`, rate: SR, latencyMs: 0, frameCount: cap.frameCount, kind: layer.kind,
    refCents: sp.refCents, root: sp.root, scale: sp.scale, bpm: sp.bpm,
  };
  blobs[take.frames] = frames;
  const si = store.doc.sections.findIndex((s) => s.id === sectionId);
  const clip = placeTake({
    doc: store.doc, sectionId, layerId, take, startSample: 0,
    mark: { sample: 0, section: si, pos: 0, unitsPerSample: 0, playing: true },
    roundTripSamples: 0, notes: sp.notes, hits: sp.hits,
  });
  store.batch('Capture take', (run) => {
    run('addTake', { take });
    run('setClip', { section: sectionId, layer: layerId, clip });
    if (!beat) run('setLayer', { id: layerId, tune: { ...layer.tune, root: Math.max(0, Math.min(11, Math.round(sp.root))), scale: Math.max(0, Math.round(sp.scale)), refCents: Math.max(-50, Math.min(50, sp.refCents)) } });
  });
}

const set96 = (s: Record<string, number>) => Array.from(soundToSet(s));

function setMelodySound(store: Store, layerId: string, i: number): void {
  store.dispatch('setLayerSound', { id: layerId, base: set96(MELODY_SOUNDS[i].sound) });
}

function setKit(store: Store, layerId: string, i: number): void {
  const k = BEAT_KITS[i];
  store.dispatch('setLayerSound', { id: layerId, base: set96(k.base), kit: k.kit.map((s, j) => ({ params: set96(s), cents: k.kitCents[j] })) });
}

function macros(store: Store, layer: Layer, m: number[]): void {
  store.dispatch('setState', { layer: layer.id, id: layer.states[0].id, macros: m });
}

// ---- the five projects

/** A sung melody take on Choir pad, with space and air on the macros, panned, at a lower level. */
function sungMelody(): Built {
  const blobs: Record<string, Float32Array> = {};
  const store = new Store(newProject('Golden: sung melody'));
  const sec = store.doc.sections[0].id;
  store.dispatch('setSection', { id: sec, bpm: 100, beats: 8 });
  const mel = store.doc.layers[0];
  setMelodySound(store, mel.id, 0);
  const x = sing([
    { start: 0.2, len: 0.7, hz: 220, vowel: 'a' }, { start: 1.0, len: 0.6, hz: 247, vowel: 'o', swell: true },
    { start: 1.75, len: 0.9, hz: 262, vowel: 'i', breath: 0.6 }, { start: 2.8, len: 1.0, hz: 196, vowel: 'a', swell: true },
  ], 4.2, 11);
  captureInto(store, blobs, sec, mel.id, x);
  const l = store.doc.layers[0];
  macros(store, l, [0.6, 0.5, 0.5, 0.5, 0.8, 0.5, 0.7, 0.6]);
  store.dispatch('setLayer', { id: l.id, level: -4, pan: -0.35 });
  return { doc: store.doc, blobs };
}

/** A beatboxed take on the percussion kit, Follow lowered, with grit; the click is off. */
function beatTake(): Built {
  const blobs: Record<string, Float32Array> = {};
  const store = new Store(newProject('Golden: beat take'));
  const sec = store.doc.sections[0].id;
  store.dispatch('setSection', { id: sec, bpm: 110, beats: 8 });
  const beat = store.doc.layers[1];
  setKit(store, beat.id, 1);
  const hits: { t: number; kind: 'k' | 's' | 'h' }[] = [];
  const beatS = 60 / 110;
  for (let i = 0; i < 16; i++) {
    const t = 0.1 + i * beatS / 2;
    hits.push({ t, kind: i % 4 === 0 ? 'k' : i % 4 === 2 ? 's' : 'h' });
  }
  captureInto(store, blobs, sec, beat.id, beatbox(hits, 0.1 + 8 * beatS + 0.3, 21));
  const l = store.doc.layers[1];
  store.dispatch('setLayer', { id: l.id, follow: 0.6, pan: 0.2 });
  macros(store, l, [0.5, 0.6, 0.5, 0.75, 0.6, 0.8, 0.5, 0.4]);
  store.dispatch('setLayer', { id: store.doc.layers[0].id, mute: true });
  return { doc: store.doc, blobs };
}

/** Three states on a texture and a melody, morph markers of several shapes, and an Influence with a
 *  grain source played as sound, part masked. */
function statesInfluence(): Built {
  const blobs: Record<string, Float32Array> = {};
  const store = new Store(newProject('Golden: states and influence'));
  const sec = store.doc.sections[0].id;
  store.dispatch('setSection', { id: sec, bpm: 90, beats: 12 });
  store.dispatch('addLayer', { kind: 'texture', name: 'Pad', sound: 4 });
  const mel = store.doc.layers[0].id;
  const pad = store.doc.layers[2].id;
  setMelodySound(store, mel, 5);
  [0, 1, 2, 3].forEach((i) => store.dispatch('addNote', { section: sec, layer: mel, note: { id: `m${i}`, t: i * 3 * PPQ, dur: 2 * PPQ, cents: 5500 + i * 200, hard: 0.6 } }));
  store.dispatch('addNote', { section: sec, layer: pad, note: { id: 'pad', t: 0, dur: 12 * PPQ, cents: 4500, hard: 0.1 } });
  for (const [layer, states] of [[pad, ['Bright', 'Dark']], [mel, ['Wide']]] as const) {
    for (const name of states) store.dispatch('addState', { layer, name, id: `${layer}-${name}` });
  }
  const P = (l: string) => store.doc.layers.find((x) => x.id === l)!;
  store.dispatch('setState', { layer: pad, id: `${pad}-Bright`, base: set96(MELODY_SOUNDS[1].sound), macros: [0.8, 0.5, 0.6, 0.5, 0.6, 0.5, 0.5, 0.5] });
  store.dispatch('setState', { layer: pad, id: `${pad}-Dark`, base: set96({ ...MELODY_SOUNDS[4].sound, 'filt.cutoff': 900, 'filt.res': 0.4, 'lfo1.to_filter': 1, 'lfo1.rate': 0.8 }), macros: [0.3, 0.7, 0.5, 0.5, 0.7, 0.5, 0.6, 0.6] });
  store.dispatch('setState', { layer: mel, id: `${mel}-Wide`, base: set96({ ...MELODY_SOUNDS[5].sound, 'delay.mix': 0.3, 'verb.mix': 0.4, 'width': 1.6 }), macros: [0.5, 0.5, 0.7, 0.6, 0.5, 0.5, 0.5, 0.5] });
  const marks: [string, number, string, number, 'line' | 'ease' | 'late' | 'step' | 'wobble'][] = [
    [pad, 0, P(pad).states[0].id, 0, 'step'], [pad, 3 * PPQ, `${pad}-Bright`, 900, 'line'], [pad, 6 * PPQ, `${pad}-Dark`, 1500, 'wobble'],
    [pad, 9 * PPQ, P(pad).states[0].id, 700, 'late'], [mel, 0, P(mel).states[0].id, 0, 'step'], [mel, 5 * PPQ, `${mel}-Wide`, 1200, 'ease'],
  ];
  for (const [layer, t, state, lenMs, shape] of marks) store.dispatch('addMorphMarker', { section: sec, layer, marker: { t, state, lenMs, shape } });
  const n = 24000;
  const tone = new Float32Array(n);
  const r = rng(5);
  for (let i = 0; i < n; i++) tone[i] = 0.3 * Math.sin((2 * Math.PI * 330 * i) / SR) + 0.1 * Math.sin((2 * Math.PI * 990 * i) / SR) + 0.03 * (r() * 2 - 1);
  blobs['grain-golden-tone'] = tone;
  store.dispatch('setInfluence', {
    layer: pad,
    influence: { target: set96({ ...MELODY_SOUNDS[2].sound, 'src.grain_mix': 0.6, 'grain.density': 30, 'grain.spray': 0.3, 'lfo2.to_grain': 0.5 }), mask: 0x1b, source: 'grain-golden-tone', sourceName: 'Golden tone', sourceRate: SR, grain: { start: 2000, end: 22000, refCents: 4700 }, useSound: true },
  });
  store.dispatch('setInfluenceAmount', { layer: pad, state: P(pad).states[0].id, amount: 0.5 });
  store.dispatch('setInfluenceAmount', { layer: pad, state: `${pad}-Dark`, amount: 0.9 });
  store.dispatch('setLayer', { id: store.doc.layers[1].id, mute: true });
  return { doc: store.doc, blobs };
}

/** Two grid sections and a free one, tempo ramp, repeats, four layers with drawn notes and hits. */
function arrangement(): Built {
  const store = new Store(newProject('Golden: arrangement'));
  const a = store.doc.sections[0].id;
  store.dispatch('setSection', { id: a, bpm: 120, beats: 4, repeats: 2 });
  store.dispatch('addSection', { feel: 'grid', beats: 4, bpm: 96 });
  store.dispatch('addSection', { feel: 'free', seconds: 2.5 });
  const [, b, c] = store.doc.sections.map((s) => s.id);
  store.dispatch('setSection', { id: b, rampMs: 800 });
  store.dispatch('addLayer', { kind: 'melody', name: 'Bass', sound: 6 });
  store.dispatch('addLayer', { kind: 'texture', name: 'Air', sound: 4 });
  const [mel, beat, bass, air] = store.doc.layers.map((l) => l.id);
  setMelodySound(store, mel, 1);
  setKit(store, beat, 0);
  const r = rng(77);
  for (const sec of [a, b]) {
    for (let i = 0; i < 8; i++) {
      store.dispatch('addNote', { section: sec, layer: mel, note: { id: `${sec}m${i}`, t: i * PPQ / 2, dur: PPQ / 2.5, cents: 6000 + 100 * Math.floor(r() * 12), hard: r() } });
    }
    for (let i = 0; i < 4; i++) store.dispatch('addNote', { section: sec, layer: bass, note: { id: `${sec}b${i}`, t: i * PPQ, dur: PPQ * 0.8, cents: 3600 + [0, 500, 700, 300][i], hard: 0.3 } });
    for (let i = 0; i < 8; i++) {
      const slot = i % 4 === 0 ? 0 : i % 4 === 2 ? 1 : i % 2 ? 2 : 3;
      store.dispatch('addHit', { section: sec, layer: beat, hit: { id: `${sec}h${i}`, t: i * PPQ / 2, dur: PPQ / 4, slot, strength: 0.5 + 0.5 * r(),
        shape: { attack: 0.002 + 0.01 * r(), burst: r(), cents: i === 5 ? 5000 : null, bend: 2 * r() - 1, hold: 0.03 + 0.2 * r(), vowel: r(), tone: r(), endStop: i === 7 ? 1 : 0, endClick: i === 7 ? 0.5 : 0 } } });
    }
  }
  store.dispatch('addNote', { section: c, layer: air, note: { id: 'air', t: 0.1, dur: 2.2, cents: 5200, hard: 0 } });
  store.dispatch('addNote', { section: c, layer: mel, note: { id: 'cm', t: 0.4, dur: 1.5, cents: 6700, hard: 0.9 } });
  store.dispatch('setLayer', { id: bass, level: -2, pan: 0.1 });
  store.dispatch('setLayer', { id: air, level: -8, pan: -0.5 });
  const L = (id: string) => store.doc.layers.find((l) => l.id === id)!;
  macros(store, L(mel), [0.7, 0.4, 0.6, 0.6, 0.6, 0.7, 0.5, 0.3]);
  macros(store, L(beat), [0.5, 0.5, 0.5, 0.6, 0.55, 0.5, 0.5, 0.5]);
  store.dispatch('setLoop', { loop: false });
  return { doc: store.doc, blobs: {} };
}

/** A sung take on Glass an octave up, gesture depths changed, wobble and air on the macros. */
function gestures(): Built {
  const blobs: Record<string, Float32Array> = {};
  const store = new Store(newProject('Golden: gestures'));
  const sec = store.doc.sections[0].id;
  store.dispatch('setSection', { id: sec, bpm: 84, beats: 8 });
  const mel = store.doc.layers[0];
  setMelodySound(store, mel.id, 2);
  const x = sing([
    { start: 0.15, len: 1.1, hz: 175, vowel: 'o', swell: true }, { start: 1.4, len: 0.5, hz: 196, vowel: 'i', breath: 0.9 },
    { start: 2.0, len: 1.3, hz: 220, vowel: 'a' }, { start: 3.5, len: 0.8, hz: 165, vowel: 'i', swell: true, breath: 0.3 },
  ], 4.6, 31);
  captureInto(store, blobs, sec, mel.id, x);
  const l = store.doc.layers[0];
  store.dispatch('setLayer', { id: l.id, depth: { loud: 0.6, open: 0.9, hard: 0.7, breath: 0.8, mouth: 1 }, tune: { ...l.tune, octave: 1, snap: 0.6 } });
  macros(store, store.doc.layers[0], [0.5, 0.55, 0.75, 0.5, 0.6, 0.5, 0.8, 0.5]);
  store.dispatch('setLayer', { id: store.doc.layers[1].id, mute: true });
  return { doc: store.doc, blobs };
}

// ---- write

const CASES: [string, () => Built][] = [
  ['sung-melody', sungMelody], ['beat-take', beatTake], ['states-influence', statesInfluence], ['arrangement', arrangement], ['gestures', gestures],
  ...DEMOS.map((d): [string, () => Built] => [`demo-${d.id}`, () => ({ doc: d.doc, blobs: d.blobs })]),
];

function curveFor(core: AnalysisCore, blobs: Record<string, Float32Array>, take: Take, layer: Layer): Float32Array {
  const frames = blobs[take.frames];
  const t = layer.tune;
  const pitch = new Float32Array(frames.length / FRAME_STRIDE);
  for (let i = 0; i < pitch.length; i++) pitch[i] = frames[i * FRAME_STRIDE];
  const { tuned } = core.tune(frames.slice(), pitch, { snap: t.snap, speedMs: t.speedMs, slides: t.slides, vibrato: t.vibrato, root: t.root, scale: t.scale, refCents: t.refCents, levelOut: t.levelOut });
  return withTuned(frames, tuned, t.octave);
}

function grainFor(blobs: Record<string, Float32Array>, l: Layer) {
  const g = l.influence?.grain;
  const src = l.influence ? blobs[l.influence.source] : undefined;
  if (!g || !src) return null;
  return { data: src.subarray(g.start, g.end), rate: l.influence!.sourceRate, refCents: g.refCents };
}

const f32 = (a: Float32Array) => new Uint8Array(a.buffer, a.byteOffset, a.byteLength);

describe.runIf(process.env.GOLDEN_WRITE === '1')('golden fixtures', () => {
  it('writes the fixtures from today\'s compiler and engine', async () => {
    const root = resolve(here, 'fixtures');
    rmSync(root, { recursive: true, force: true });
    const core = AnalysisCore.fromBytes(analysis);
    const summary: Record<string, unknown> = {};
    for (const [name, build] of CASES) {
      const { doc, blobs } = build();
      const dir = resolve(root, name);
      for (const d of ['uploads', 'curves', 'blobs']) mkdirSync(resolve(dir, d), { recursive: true });
      writeFileSync(resolve(dir, 'doc.json'), JSON.stringify(doc, null, 1));
      const curves = new Map<string, Float32Array>();
      const uploads = new Compiler().compile(doc, (take, layer) => {
        const c = curveFor(core, blobs, take, layer);
        curves.set(take.id, c);
        return c;
      }, { grainFor: (l) => grainFor(blobs, l) });
      for (const [id, c] of curves) writeFileSync(resolve(dir, 'curves', `${id}.f32`), f32(c));
      for (const [ref, b] of Object.entries(blobs)) if (!ref.startsWith('audio-')) writeFileSync(resolve(dir, 'blobs', `${ref}.f32`), f32(b));
      const list = uploads.map((u, i) => {
        const file = `${String(i).padStart(2, '0')}-${u.kind}-${u.slot}.bin`;
        writeFileSync(resolve(dir, 'uploads', file), new Uint8Array(u.blob));
        return { kind: u.kind, slot: u.slot, what: u.what, file };
      });
      writeFileSync(resolve(dir, 'uploads.json'), JSON.stringify(list, null, 1));
      const { ex } = instantiateEngine(klont, SR, 4096);
      const res = await renderSong(ex, uploadsToSong(uploads), { maxSeconds: 20 });
      summary[name] = { seconds: res.frames / SR, peak: res.peak, stop: res.stopReason, layers: doc.layers.length, takes: Object.keys(doc.takes).length };
    }
    writeFileSync(resolve(root, 'summary.json'), JSON.stringify(summary, null, 1));
    console.log(summary);
  }, 300_000);
});
