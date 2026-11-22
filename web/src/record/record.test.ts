// Stage S5 checks (docs/synth.md "S5: recording and automation"), through the committed
// klont.wasm and the app's own recorder, compiler, store and journal:
// 1. Record 30 s with MIDI notes, controller moves and singing into a listening layer. Live
//    playback from the start and an export null within -90 dB; two exports are bit-identical.
// 2. Undo removes a recorded pass as one step; redo puts it back.
// 3. Killing the tab after a recorded pass and reopening restores the lanes (journal replay).
// Flex quantize (docs/quant.md, Q1) re-runs check 1 in a 120 bpm grid section with Feel on in
// both recorded clips (Feel needs a grid): live playback and the export stay bit-identical.
// The recording is driven the way the app drives it: messages into the engine's control ring
// before each 128-sample quantum, the same events into a Pass, the mic's audio and frames into a
// take, and the clock from the meter block. The engine-only S5 checks are crates/klont/tests/s5.rs.
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { uploadsToSong } from '../app/mix';
import * as pr from '../m1/protocol';
import { Core, renderSong } from '../m1/render-core';
import { newId, newProject } from '../project/commands';
import { Compiler, type Upload } from '../project/compile';
import { clockMark, type ClockMark } from '../project/overdub';
import { memoryKv, ProjectDb } from '../project/persist';
import { Store } from '../project/store';
import type { Project, Take } from '../project/types';
import { layoutOf } from '../synth/patch';
import { listeningClip, Pass, passCommands, stateValue } from './record';

const SR = 48000;
const Q = 128;
const SECS = 30;
const wasm = readFileSync(new URL('../../public/wasm/klont.wasm', import.meta.url));
const exportsOf = () => new WebAssembly.Instance(new WebAssembly.Module(wasm), {}).exports as unknown as pr.EngineExports;

/** Singing: 220 Hz with harmonics, vibrato and phrases (1.2 s on, 0.4 s off) that swell. */
function singing(n: number): Float32Array {
  const out = new Float32Array(n);
  let ph = 0;
  for (let i = 0; i < n; i++) {
    const t = i / SR;
    const inPhrase = t % 1.6;
    if (inPhrase > 1.2) continue;
    const env = Math.min(1, inPhrase / 0.02) * Math.min(1, (1.2 - inPhrase) / 0.05) * (0.5 + 0.5 * Math.sin(t * 0.7));
    ph += (2 * Math.PI * 220 * (1 + 0.01 * Math.sin(2 * Math.PI * 5 * t))) / SR;
    let v = 0;
    for (let k = 1; k <= 5; k++) v += Math.sin(k * ph) / k;
    out[i] = 0.2 * env * v;
  }
  return out;
}

/** The project: one free section of 32 s; the melody layer listens, has an input element (the
 *  raw voice beside the synth) and a route from its sung volume to its filter, and two states; the
 *  beat layer plays MIDI hits. */
function project(grid = false): { store: Store; doc0: Project } {
  const store = new Store(newProject('S5'));
  const doc = store.doc;
  const [mel, beat] = doc.layers;
  const sec = doc.sections[0].id;
  store.dispatch('setSection', grid ? { id: sec, feel: 'grid', bpm: 120, beats: 64 } : { id: sec, feel: 'free', seconds: 32 });
  store.dispatch('setLoop', { loop: false });
  store.dispatch('addChannelFx', { layer: mel.id, type: 'input' });
  store.dispatch('addRoute', { layer: mel.id, route: { source: 'layer.volume', target: 'filter1.cutoff' }, amount: 0.4 });
  const st = store.doc.layers[0].states[0].id;
  store.dispatch('setValues', { layer: mel.id, state: st, values: { 'input1.mix': 0.3 } });
  store.dispatch('addState', { layer: mel.id, from: st, name: 'Dark', id: 'dark' });
  store.dispatch('setValues', { layer: mel.id, state: 'dark', values: { 'filter1.cutoff': 0.25, 'input1.mix': 0.6 } });
  void beat;
  return { store, doc0: store.doc };
}

/** Load what the compiler made into an engine. */
function load(core: Core, uploads: Upload[]): void {
  // Patches first: layer and states blobs must match them (compile.ts orders them so).
  for (const u of uploads) expect(core.load(u.kind, u.slot, u.blob), u.what).toBe(pr.LOAD_OK);
}

interface Recorded { pass: Pass; marks: ClockMark[]; mic: Float32Array; frames: Float32Array; start: number }

/** Record one pass on a live engine: the song plays from the start, layer 0 listens to the
 *  singing, MIDI plays both layers, controllers move a value, a macro, the wheel and the mix, and
 *  two scenes are tapped. */
function record(core: Core, doc: Project): Recorded {
  const [mel, beat] = doc.layers;
  const lay = layoutOf(mel.patch);
  const cutoff = lay.index.get('filter1.cutoff')!;
  const mix = lay.index.get('input1.mix')!;
  const mic = singing(SECS * SR);
  const blocks = (SECS * SR) / Q;
  const micOut = new Float32Array(SECS * SR);
  const frames: number[] = [];
  const marks: ClockMark[] = [];
  // The engine's clock (samples since it started), as the meter block reports it.
  const start = core.meter()[pr.M_CLOCK_HI] * 2 ** 20 + core.meter()[pr.M_CLOCK_LO];
  core.send([[pr.MSG_LISTEN, 0, 0, 0, 1], [pr.MSG_RECORD, 0, 0, 0, 1], [pr.MSG_PLAY, 0, 0, 0, 0]]);
  core.process(Q, mic.subarray(0, Q));
  micOut.set(core.mic(Q), 0);
  frames.push(...core.frameRecords());
  let clock = start + Q;
  const pass = new Pass(start);
  marks.push(clockMark(core.meter()));
  const offs: { at: number; h: number; layer: number; cents: number }[] = [];
  /** Did a multiple of `period` seconds pass between the last block and this one? */
  const every = (period: number) => Math.floor((clock - start) / (period * SR)) !== Math.floor((clock - start - Q) / (period * SR));
  const at = (sec: number) => clock - start >= sec * SR && clock - start - Q < sec * SR;
  for (let b = 1; b < blocks; b++) {
    const t = (clock - start) / SR;
    const msgs: pr.Msg[] = [];
    // Melody notes every 500 ms, held 300 ms; kit hits every 250 ms, held 100 ms.
    if (every(0.5)) {
      const k = Math.round(t / 0.5);
      const cents = 4500 + 100 * (k % 12);
      const vel = Math.round((0.5 + 0.1 * (k % 5)) * 1000);
      msgs.push([pr.MSG_NOTE_ON, 0, 0, vel, cents]);
      offs.push({ at: clock + 0.3 * SR, h: pass.noteOn(mel.id, cents, 0, vel / 1000, clock), layer: 0, cents });
    }
    if (every(0.25)) {
      const slot = Math.round(t / 0.25) % 3;
      const cents = beat.kit[slot].cents;
      msgs.push([pr.MSG_NOTE_ON, 1, slot, 900, cents]);
      offs.push({ at: clock + 0.1 * SR, h: pass.noteOn(beat.id, cents, slot, 0.9, clock), layer: 1, cents });
    }
    for (const o of offs.filter((x) => x.at <= clock)) {
      msgs.push([pr.MSG_NOTE_OFF, o.layer, 0, 0, o.cents]);
      pass.noteOff(o.h, clock);
      offs.splice(offs.indexOf(o), 1);
    }
    // A controller sweeps the filter (2-6 s), a macro (10-14 s), the wheel (16-20 s) and the mix
    // from the synth to the voice (22-28 s), one move every 20 ms.
    if (every(0.02)) {
      const ramp = (a: number, z: number) => Math.min(1, Math.max(0, (t - a) / (z - a)));
      if (t >= 2 && t < 6) { const v = 0.3 + 0.6 * ramp(2, 6); msgs.push([pr.MSG_SET_PARAM, 0, pr.SET_BASE, cutoff, v]); pass.move(mel.id, 'filter1.cutoff', v, clock); }
      if (t >= 10 && t < 14) { const v = ramp(10, 14); msgs.push([pr.MSG_SET_MACRO, 0, 1, 0, v]); pass.move(mel.id, 'macro2', v, clock); }
      if (t >= 16 && t < 20) { const v = ramp(16, 20); msgs.push([pr.MSG_SET_PERF, 0, 0, 0, v]); pass.move(mel.id, 'wheel', v, clock); }
      if (t >= 22 && t < 28) { const v = ramp(22, 28); msgs.push([pr.MSG_SET_PARAM, 0, pr.SET_BASE, mix, v]); pass.move(mel.id, 'input1.mix', v, clock); }
    }
    // Scenes at 8 s (Dark over 400 ms) and 24 s (back over 1 s).
    if (at(8)) { msgs.push([pr.MSG_MORPH_TO, 0, 1, 0, 400]); pass.scene(mel.id, 'dark', 400, clock); }
    if (at(24)) { msgs.push([pr.MSG_MORPH_TO, 0, 0, 0, 1000]); pass.scene(mel.id, mel.states[0].id, 1000, clock); }
    if (msgs.length) core.send(msgs);
    core.process(Q, mic.subarray(b * Q, (b + 1) * Q));
    micOut.set(core.mic(Q), b * Q);
    frames.push(...core.frameRecords());
    clock += Q;
    // The page reads the meters about every animation frame.
    if (b % 6 === 0) marks.push(clockMark(core.meter()));
  }
  core.send([[pr.MSG_RECORD, 0, 0, 0, 0]]);
  pass.finish(clock);
  return { pass, marks, mic: micOut, frames: Float32Array.from(frames), start };
}

/** Commit a recorded pass as the app does: the take and the pass, one undo group. */
function commit(store: Store, r: Recorded): void {
  const doc = store.doc;
  const mel = doc.layers[0];
  const sec = doc.sections[0];
  const n = Math.floor(r.frames.length / 25);
  const take: Take = {
    id: newId('take'), audio: '', frames: '', rate: SR, latencyMs: 0, frameCount: n, kind: mel.kind,
    refCents: 0, root: 0, scale: 0, bpm: null,
  };
  take.audio = `audio-${take.id}`;
  take.frames = `frames-${take.id}`;
  const clip = listeningClip(doc, sec.id, mel.id, take, r.start, r.marks[0], 0, n / 100);
  const cmds = passCommands(doc, r.pass, { marks: r.marks, outLatency: 0 }, SR, (l, target) => stateValue(l, target), new Map([[`${sec.id}/${mel.id}`, clip]]));
  expect(store.batch('Record pass', (run) => {
    run('addTake', { take });
    run('setClip', { section: sec.id, layer: mel.id, clip });
    for (const c of cmds) run(c.name as 'setClip', c.args as never);
  })).toBe(true);
  // Where the hand left the filter, the mix and the macro is now the layer's own.
  const st = store.doc.layers[0].states[0];
  expect(st.values['input1.mix']).toBeCloseTo(1, 2);
  expect(st.macros[1]).toBeCloseTo(1, 1);
}

const dbfs = (x: number) => 20 * Math.log10(Math.max(x, 1e-30));
const worst = (a: Float32Array, b: Float32Array) => { let m = 0; for (let i = 0; i < Math.min(a.length, b.length); i++) m = Math.max(m, Math.abs(a[i] - b[i])); return m; };
const peak = (a: Float32Array) => a.reduce((m, v) => Math.max(m, Math.abs(v)), 0);

/** Check 1: record, commit, play back live from the start, export twice. With `flex`, the section
 *  is a grid and both recorded clips get Feel before playback. */
async function checkOne(flex: boolean): Promise<void> {
    const { store } = project(flex);
    const compiler = new Compiler();
    const core = new Core(exportsOf(), SR, Q);
    load(core, compiler.compile(store.doc, () => null));
    // Let the values settle before the pass, as a live engine that has sat still has them.
    for (let i = 0; i < SR / Q; i++) core.process(Q);
    const t0 = performance.now();
    const r = record(core, store.doc);
    const recordMs = performance.now() - t0;
    commit(store, r);
    if (flex) {
      // 16ths swung 60% at 80% strength on the melody (its MIDI notes are its anchors); 8ths at full
      // strength on the beat layer with kit slot 2 left alone.
      const sec = store.doc.sections[0].id;
      const [mel, beat] = store.doc.layers;
      store.dispatch('setFlex', { section: sec, layer: mel.id, flex: { on: true, grid: 240, triplets: false, feel: { kind: 'swing', amount: 0.6, unit: 240 }, strength: 0.8, catch: 120 } });
      store.dispatch('setFlex', { section: sec, layer: beat.id, flex: { on: true, grid: 480, triplets: true, feel: { kind: 'straight' }, strength: 1, catch: 240, slotStrength: [1, 1, 0, 1] } });
    }

    const doc = store.doc;
    const clip = doc.sections[0].clips[doc.layers[0].id]!;
    const beatClip = doc.sections[0].clips[doc.layers[1].id]!;
    const played = (id: string) => r.pass.notes.filter((x) => x.layer === id).length;
    expect(played(doc.layers[0].id)).toBe(59);
    expect(clip.kind === 'melody' && clip.notes.length).toBe(59);
    expect(beatClip.kind === 'beat' && beatClip.hits.length).toBe(played(doc.layers[1].id));
    expect(clip.auto?.map((l) => l.target).sort()).toEqual(['filter1.cutoff', 'input1.mix', 'macro2', 'wheel']);
    expect(clip.morph.map((m) => m.state)).toEqual([doc.layers[0].states[0].id, 'dark', doc.layers[0].states[0].id]);
    const curve = (t: Take) => (t.frames === `frames-${t.id}` ? r.frames : null);
    const audio = (t: Take) => (t.audio === `audio-${t.id}` ? r.mic : null);

    // Live playback, on the engine that recorded: the app sends what changed, stops, and plays
    // from the start. Listen is held off while the transport plays a take on the layer.
    load(core, compiler.compile(doc, curve, { audioFor: audio }));
    core.send([[pr.MSG_STOP, 0, 0, 0, 0]]);
    for (let i = 0; i < (3 * SR) / Q; i++) core.process(Q);
    core.seek(0, 0);
    core.send([[pr.MSG_LISTEN, 0, 0, 0, 0], [pr.MSG_PLAY, 0, 0, 0, 0]]);
    const n = SECS * SR;
    const live = { l: new Float32Array(n), r: new Float32Array(n) };
    for (let b = 0; b < n / Q; b++) {
      core.process(Q);
      const o = core.out(Q);
      live.l.set(o.l, b * Q);
      live.r.set(o.r, b * Q);
    }
    core.free();

    const exportOnce = () => renderSong(exportsOf(), uploadsToSong(new Compiler().compile(doc, curve, { audioFor: audio })), { once: true, settle: true, maxSeconds: SECS });
    const a = await exportOnce();
    const b = await exportOnce();
    expect(a.frames).toBe(n);
    if (flex) {
      // Feel moved what plays: the track differs from the same document with Feel off.
      const track = (d: Project) => new Compiler().compile(d, curve, { audioFor: audio }).find((u) => u.what === 'track')!.blob;
      const off = { ...doc, sections: doc.sections.map((s) => ({ ...s, clips: Object.fromEntries(Object.entries(s.clips).map(([k, c]) => [k, { ...c!, flex: undefined }])) })) };
      expect(Buffer.from(track(doc)).equals(Buffer.from(track(off)))).toBe(false);
    }
    const d = Math.max(worst(live.l, a.left), worst(live.r, a.right));
    console.log(`${flex ? 'Q1: S5 check 1 with Feel on, grid section' : 'S5 check 1'}: ${SECS} s pass (${r.pass.notes.length} notes and hits, ${r.pass.moves.length} moves, ${r.pass.scenes.length} scene taps); live playback against the export: worst difference ${dbfs(d).toFixed(1)} dBFS (peak ${peak(a.left).toFixed(2)}); recording ${SECS} s took ${(recordMs / 1000).toFixed(2)} s in 128-sample calls; the export rendered at ${a.realtime.toFixed(1)}x real time`);
    expect(peak(a.left)).toBeGreaterThan(0.05);
    expect(dbfs(d)).toBeLessThanOrEqual(-90);
    expect(Buffer.from(a.left.buffer).equals(Buffer.from(b.left.buffer))).toBe(true);
    expect(Buffer.from(a.right.buffer).equals(Buffer.from(b.right.buffer))).toBe(true);
}

describe('S5: a recorded pass', () => {
  it('plays back live as the export renders it, and two exports are bit-identical (check 1)', () => checkOne(false), 120_000);

  it('with Feel on in the recorded clips (docs/quant.md Q1), plays back live as the export renders it', () => checkOne(true), 120_000);

  it('is one undo step, and redo puts it back (check 2)', () => {
    const { store, doc0 } = project();
    const core = new Core(exportsOf(), SR, Q);
    load(core, new Compiler().compile(store.doc, () => null));
    const r = record(core, store.doc);
    core.free();
    commit(store, r);
    const after = store.doc;
    expect(after).not.toBe(doc0);
    expect(store.undo()).toBe(true);
    expect(store.doc).toEqual(doc0);
    expect(store.redo()).toBe(true);
    expect(store.doc).toEqual(after);
  }, 60_000);

  it('survives a killed tab: reopening replays the journal and the lanes are back (check 3)', async () => {
    const kv = memoryKv();
    const db = new ProjectDb(kv);
    const { store: setup } = project();
    await db.create(setup.doc);
    // The app's wiring: every change goes to the journal.
    const store = new Store(setup.doc);
    store.subscribe((d, entry) => db.append(d.id, entry, d, store.history.toJSON()));
    const core = new Core(exportsOf(), SR, Q);
    load(core, new Compiler().compile(store.doc, () => null));
    const r = record(core, store.doc);
    core.free();
    commit(store, r);
    // The journal's write timer fires (300 ms after the change), then the tab dies: nothing
    // else is written, no snapshot is taken.
    await db.flush();
    expect((await kv.keys('journal', `${store.doc.id}/`)).length).toBe(1);
    const back = (await new ProjectDb(kv).open(store.doc.id))!;
    expect(back.doc).toEqual(store.doc);
    const lanes = back.doc.sections[0].clips[back.doc.layers[0].id]!.auto!;
    expect(lanes.length).toBe(4);
    expect(lanes.every((l) => l.points.length > 10)).toBe(true);
    expect(back.history.undo.length).toBe(1);
  }, 60_000);
});
