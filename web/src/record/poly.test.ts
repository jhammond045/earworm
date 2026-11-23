// Stage P3 checks (docs/poly.md "P3: poly takes"), through the committed klont.wasm and
// analysis.wasm and the app's own placement, compiler, store and journal:
// 3. Record 30 s of a synthetic guitar (strummed chords) through the live engine with a layer
//    armed in Poly; commit; live playback from the start and an export null within -90 dB; two
//    exports are bit-identical. Again at the take length limit, 5 minutes (POLY_MAX_TAKE_SEC).
// 4. Undo removes the pass in one step; the journal replays it after a reload.
// The recording is driven as S5's record test drives it (record.test.ts): the mic's audio into the
// engine 128 samples at a time, its high-passed audio and frames kept as a take, the clock from
// the meter block. Then the poly pass reads the take as the analysis worker does, a second at a
// time, and the take is placed with its notes and note curves.
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { uploadsToSong } from '../app/mix';
import * as pr from '../m1/protocol';
import { Core, renderSong } from '../m1/render-core';
import { AnalysisCore, type PolyPass } from '../m2/analysis';
import { newId, newProject } from '../project/commands';
import { Compiler, type Upload } from '../project/compile';
import { clockMark, placeTake, type ClockMark } from '../project/overdub';
import { memoryKv, ProjectDb } from '../project/persist';
import { Store } from '../project/store';
import type { Project, Take } from '../project/types';
import { POLY_MAX_TAKE_SEC } from '../m2/take';
import { polyOptions } from './poly';
import { guitar, STRUM_S, type Played } from './synth-guitar';

const SR = 48000;
const Q = 128;
const SECS = 30;
const wasm = readFileSync(new URL('../../public/wasm/klont.wasm', import.meta.url));
const analysisWasm = readFileSync(new URL('../../public/wasm/analysis.wasm', import.meta.url));
const exportsOf = () => new WebAssembly.Instance(new WebAssembly.Module(wasm), {}).exports as unknown as pr.EngineExports;

/** One free section 2 s longer than the take; the melody layer is armed in Poly (guitar, 6 channels). */
function project(secs = SECS): Store {
  const store = new Store(newProject('P3'));
  const sec = store.doc.sections[0].id;
  const mel = store.doc.layers[0];
  store.dispatch('setSection', { id: sec, feel: 'free', seconds: secs + 2 });
  store.dispatch('setLoop', { loop: false });
  store.dispatch('setLayer', { id: mel.id, capture: { mode: 'poly', instrument: 'guitar', channels: 6 }, level: -12 });
  return store;
}

function load(core: Core, uploads: Upload[]): void {
  for (const u of uploads) expect(core.load(u.kind, u.slot, u.blob), u.what).toBe(pr.LOAD_OK);
}

interface Recorded { mic: Float32Array; frames: Float32Array; marks: ClockMark[]; start: number; played: Played[] }

/** Record the guitar on a live engine with the song playing. A layer armed in Poly has no live
 *  sound yet, so nothing is armed: the engine keeps the mic's audio and frames as it always does. */
function record(core: Core, secs = SECS): Recorded {
  const { audio, played } = guitar(secs * SR, SR);
  const mic = new Float32Array(secs * SR);
  const frames = new Float32Array((secs * 100 + 1) * 25);
  let nf = 0;
  const marks: ClockMark[] = [];
  const start = core.meter()[pr.M_CLOCK_HI] * 2 ** 20 + core.meter()[pr.M_CLOCK_LO];
  core.send([[pr.MSG_RECORD, 0, 0, 0, 1], [pr.MSG_PLAY, 0, 0, 0, 0]]);
  for (let b = 0; b < (secs * SR) / Q; b++) {
    core.process(Q, audio.subarray(b * Q, (b + 1) * Q));
    mic.set(core.mic(Q), b * Q);
    const f = core.frameRecords();
    frames.set(f, nf);
    nf += f.length;
    if (b % 6 === 0) marks.push(clockMark(core.meter()));
  }
  core.send([[pr.MSG_RECORD, 0, 0, 0, 0]]);
  return { mic, frames: frames.slice(0, nf), marks, start, played };
}

/** Commit as the app does (app/capture.ts placePoly): the poly pass, then the take and its clip as
 *  one undo step. */
function commit(store: Store, r: Recorded): { take: Take; pass: PolyPass } {
  const doc = store.doc;
  const mel = doc.layers[0];
  const sec = doc.sections[0];
  const core = AnalysisCore.fromBytes(analysisWasm);
  const pass = core.polyPass(r.mic, SR, polyOptions(mel));
  const take: Take = {
    id: newId('take'), audio: '', frames: '', rate: SR, latencyMs: 0, frameCount: Math.floor(r.frames.length / 25), kind: mel.kind,
    refCents: mel.tune.refCents, root: mel.tune.root, scale: mel.tune.scale, bpm: null,
  };
  take.audio = `audio-${take.id}`;
  take.frames = `frames-${take.id}`;
  take.noteCurves = `notecurves-${take.id}`;
  const clip = placeTake({ doc, sectionId: sec.id, layerId: mel.id, take, startSample: r.start, mark: r.marks[0], roundTripSamples: 0, notes: pass.notes, hits: [] });
  expect(store.batch('Capture take', (run) => {
    run('addTake', { take });
    run('setClip', { section: sec.id, layer: mel.id, clip });
  })).toBe(true);
  return { take, pass };
}

const dbfs = (x: number) => 20 * Math.log10(Math.max(x, 1e-30));
const worst = (a: Float32Array, b: Float32Array) => { let m = 0; for (let i = 0; i < Math.min(a.length, b.length); i++) m = Math.max(m, Math.abs(a[i] - b[i])); return m; };
const peak = (a: Float32Array) => a.reduce((m, v) => Math.max(m, Math.abs(v)), 0);

/** Check 3 on a take of `secs` seconds. */
async function check3(secs: number): Promise<void> {
  const store = project(secs);
  const compiler = new Compiler();
  const core = new Core(exportsOf(), SR, Q);
  load(core, compiler.compile(store.doc, () => null));
  for (let i = 0; i < SR / Q; i++) core.process(Q);
  const t0 = performance.now();
  const r = record(core, secs);
  const recordMs = performance.now() - t0;
  const { take, pass } = commit(store, r);

  // The notes: each played note found once, within 50 ms, same semitone (docs/poly.md scoring).
  const doc = store.doc;
  const clip = doc.sections[0].clips[doc.layers[0].id]!;
  expect(clip.kind).toBe('melody');
  const notes = clip.kind === 'melody' ? clip.notes : [];
  expect(notes.length).toBe(pass.notes.length);
  expect(notes.every((n) => n.curve !== undefined && n.curve >= 0 && n.curve < pass.notes.length)).toBe(true);
  const used = new Set<number>();
  let found = 0;
  for (const p of r.played) {
    const i = pass.notes.findIndex((n, k) => !used.has(k) && n.midi === p.midi && Math.abs(n.t - p.t) <= 0.05);
    if (i >= 0) { used.add(i); found++; }
  }
  // A note that ends where another starts does not overlap it. Times are f32 seconds, so at a few
  // minutes the end can read a microsecond past the start; allow 1 ms.
  const most = Math.max(...pass.notes.map((n) => pass.notes.filter((m) => m.t <= n.t && m.t + m.dur > n.t + 1e-3).length));
  console.log(`P3 check 3 (${secs} s): ${r.played.length} notes played in ${secs / STRUM_S} strums, ${pass.notes.length} read (${found} of them played, ${(100 * found / r.played.length).toFixed(0)}%), at most ${most} at once; the poly pass took ${(pass.ms / 1000).toFixed(2)} s`);
  expect(found / r.played.length).toBeGreaterThan(0.6);
  expect(most).toBeLessThanOrEqual(6);

  const curves = (t: Take) => (t.frames === `frames-${t.id}` ? r.frames : null);
  const audio = (t: Take) => (t.audio === `audio-${t.id}` ? r.mic : null);
  const noteCurves = (t: Take) => (t.id === take.id ? pass.curves : null);
  const opts = { audioFor: audio, noteCurvesFor: noteCurves };
  const uploads = compiler.compile(doc, curves, opts);
  expect(uploads.some((u) => u.kind === pr.LOAD_NOTE_CURVES)).toBe(true);
  const track = uploads.find((u) => u.what === 'track')!.blob;
  expect(new Uint32Array(track, 0, 1)[0]).toBe(0x34545745); // "EWT4": the notes name their curves

  // Live playback on the engine that recorded: stop, let it settle, play from the start.
  load(core, uploads);
  core.send([[pr.MSG_STOP, 0, 0, 0, 0]]);
  for (let i = 0; i < (3 * SR) / Q; i++) core.process(Q);
  core.seek(0, 0);
  core.send([[pr.MSG_PLAY, 0, 0, 0, 0]]);
  const n = secs * SR;
  const live = { l: new Float32Array(n), r: new Float32Array(n) };
  for (let b = 0; b < n / Q; b++) {
    core.process(Q);
    const o = core.out(Q);
    live.l.set(o.l, b * Q);
    live.r.set(o.r, b * Q);
  }
  core.free();

  const exportOnce = () => renderSong(exportsOf(), uploadsToSong(new Compiler().compile(doc, curves, opts)), { once: true, settle: true, maxSeconds: secs });
  const a = await exportOnce();
  const b = await exportOnce();
  expect(a.frames).toBe(n);
  const d = Math.max(worst(live.l, a.left), worst(live.r, a.right));
  console.log(`P3 check 3 (${secs} s): live playback against the export: worst difference ${dbfs(d).toFixed(1)} dBFS (peak ${peak(a.left).toFixed(2)}); recording ${secs} s took ${(recordMs / 1000).toFixed(2)} s in 128-sample calls; the export rendered at ${a.realtime.toFixed(1)}x real time`);
  expect(peak(a.left)).toBeGreaterThan(0.05);
  expect(dbfs(d)).toBeLessThanOrEqual(-90);
  expect(Buffer.from(a.left.buffer).equals(Buffer.from(b.left.buffer))).toBe(true);
  expect(Buffer.from(a.right.buffer).equals(Buffer.from(b.right.buffer))).toBe(true);

  // The note curves are what the voices play: without them the export differs.
  const plain = await renderSong(exportsOf(), uploadsToSong(new Compiler().compile(doc, curves, { audioFor: audio })), { once: true, settle: true, maxSeconds: secs });
  expect(Buffer.from(plain.left.buffer).equals(Buffer.from(a.left.buffer))).toBe(false);
}

describe('P3: a poly take', () => {
  it('records, plays back live as the export renders it, and two exports are bit-identical (check 3)', () => check3(SECS), 180_000);

  it('does the same at the take length limit, 5 minutes', () => check3(POLY_MAX_TAKE_SEC), 900_000);

  it('is one undo step, and the journal replays it after a reload (check 4)', async () => {
    const kv = memoryKv();
    const db = new ProjectDb(kv);
    const setup = project();
    await db.create(setup.doc);
    const doc0: Project = setup.doc;
    const store = new Store(setup.doc);
    store.subscribe((d, entry) => db.append(d.id, entry, d, store.history.toJSON()));
    const core = new Core(exportsOf(), SR, Q);
    load(core, new Compiler().compile(store.doc, () => null));
    const r = record(core);
    core.free();
    const { take } = commit(store, r);
    const after = store.doc;
    await db.flush();
    const back = (await new ProjectDb(kv).open(store.doc.id))!;
    expect(back.doc).toEqual(after);
    expect(back.doc.takes[take.id].noteCurves).toBe(`notecurves-${take.id}`);
    expect(back.history.undo.length).toBe(1);
    expect(store.undo()).toBe(true);
    expect(store.doc).toEqual(doc0);
    expect(store.redo()).toBe(true);
    expect(store.doc).toEqual(after);
  }, 120_000);
});
