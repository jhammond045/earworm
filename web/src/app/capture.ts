// Hold to capture into the selected layer, or press Record to start and again to stop (a pass,
// stage S5). A stopped grid section gets one bar of count-in from the engine before the transport
// and the take start together. The click is not in the mic buffer.
//
// A recorded pass also keeps what is played live while it runs (record/record.ts): MIDI and
// on-screen notes, moves of values, macros, level, pan, wheel, bend and pressure, and scene taps.
// When it ends, the take (if the mic recorded one) and all of that go into the document as one
// undo step. A take into a listening layer keeps its audio and features but no notes of its own:
// the layer's notes come from MIDI or the keys.

import { loadKlontBytes, micConstraints } from '../m1/klont-host';
import { importedClip, importPass, liveTierOver, toMono } from '../project/import-take';
import { inputAt } from '../synth/patch';
import { LAYER_LEVEL, LAYER_PAN, M_COUNT_SEQ, M_PLAYING, M_SECTION, MSG_RECORD, MSG_SET_LAYER, MSG_SET_MACRO, MSG_SET_PARAM, SET_BASE } from '../m1/protocol';
import { listeningClip, Pass, passCommands, stateValue, withHand, type RecordHooks } from '../record/record';
import { layoutOf } from '../synth/patch';
import type { Clip, Project } from '../project/types';
import type { App } from '../m3/app';
import { ANCHORS_VERSION, type TaughtSlot } from '../m2/analysis';
import { anchorsRef, noteCurvesRef } from '../m3/session';
import { isPoly, polyOptions } from '../record/poly';
import { tuneFromTake } from './tune';
import { newId } from '../project/commands';
import { clockMark, placeTake, type ClockMark } from '../project/overdub';
import type { Take } from '../project/types';
import { MAX_TAKE_SEC, MIN_TAKE_SEC, POLY_MAX_TAKE_SEC } from '../m2/take';

const FRAME_RATE = 100;
const FRAME_STRIDE = 25;

/** Poly takes stream through their pass and can run 5 minutes; mono takes stop at 59 s. */
const maxTakeSec = (a: Active) => (a.poly ? POLY_MAX_TAKE_SEC : MAX_TAKE_SEC);

interface Active {
  layerId: string;
  layerIdx: number;
  f0: number;
  mark: ClockMark | null;
  /** Record (press to start, press to stop) rather than hold. */
  latched: boolean;
  /** The mic records a take. */
  mic: boolean;
  /** The layer listens: its take keeps no notes, and Live follow is not armed. */
  listening: boolean;
  /** The layer is armed in Poly (docs/poly.md P3): the take comes back as notes with their own
   *  curves. There is no live poly sound yet, so Live follow is not armed either. */
  poly: boolean;
  pass: Pass | null;
  marks: ClockMark[];
  sectionId: string;
  timer: number;
  /** Count-in has been asked for and the take has not started. */
  counting: boolean;
  seq: number;
}

export class AppCapture implements RecordHooks {
  private cap: Active | null = null;
  /** The newest clock reading and when it was taken (performance.now()). */
  private lastMark: ClockMark | null = null;
  private lastMarkAt = 0;
  /** Whether a layer listens (the Sound panel's Listen), and the scene a layer was left in. */
  listeningTo: (layerId: string) => boolean = () => false;
  sceneOf: (layerId: string) => string | undefined = () => undefined;
  private busy = false;
  /** Round trip in milliseconds, edited from the project panel. */
  roundTripMs = 30;
  /** Taught kit, used when the next take is a beat. Null slots are untaught. */
  taught: (TaughtSlot | null)[] | null = null;

  constructor(private readonly app: App, private readonly status: (s: string) => void, private readonly onPlaced: () => void) {}

  get holding(): boolean { return !!this.cap; }
  /** A pass records (RecordHooks). */
  get recording(): boolean { return !!this.cap?.pass; }

  // ---- RecordHooks: what the page plays while a pass records

  /** The engine sample sounding now, from the newest clock reading. */
  private nowSample(): number {
    const m = this.lastMark;
    const sr = this.app.host?.sampleRate ?? 48000;
    return m ? m.sample + Math.max(0, performance.now() - this.lastMarkAt) * sr / 1000 : 0;
  }
  noteOn(layer: string, cents: number, slot: number, vel: number): number {
    return this.cap?.pass ? this.cap.pass.noteOn(layer, cents, slot, vel, this.nowSample()) : 0;
  }
  noteOff(handle: number): void { this.cap?.pass?.noteOff(handle, this.nowSample()); }
  scene(layer: string, state: string, ms: number): void { this.cap?.pass?.scene(layer, state, ms, this.nowSample()); }
  /** A hand move: kept in the pass, and told to the engine as a message too, so the engine takes
   *  the target from its lane while the pass records (MSG_RECORD). */
  move(layer: string, target: string, v: number): void {
    const pass = this.cap?.pass;
    const host = this.app.host;
    if (!pass || !host) return;
    pass.move(layer, target, v, this.nowSample());
    const i = this.app.doc.layers.findIndex((x) => x.id === layer);
    if (i < 0) return;
    const m = /^macro([1-8])$/.exec(target);
    if (m) host.send(MSG_SET_MACRO, i, Number(m[1]) - 1, 0, v);
    else if (target === 'layer.level') host.send(MSG_SET_LAYER, i, 0, LAYER_LEVEL, v);
    else if (target === 'layer.pan') host.send(MSG_SET_LAYER, i, 0, LAYER_PAN, v);
    else if (!['wheel', 'bend', 'pressure'].includes(target)) {
      const k = layoutOf(this.app.doc.layers[i].patch).index.get(target);
      if (k !== undefined) host.send(MSG_SET_PARAM, i, SET_BASE, k, v);
    }
  }
  overlay(doc: Project): Project {
    const pass = this.cap?.pass;
    if (!pass || !pass.moves.length) return doc;
    try { return withHand(doc, pass.moves, (l) => this.app.ui.editState(l).id); } catch { return doc; }
  }

  /** Press Record: start a pass, or end the one that records. */
  toggleRecord(): void {
    if (this.cap?.latched) void this.end();
    else if (!this.cap) this.start(true);
  }

  /** The pass starts recording now: the engine hears MSG_RECORD and the clock starts. */
  private beginPass(a: Active): void {
    if (!a.latched || !this.app.host) return;
    a.pass = new Pass(this.nowSample());
    for (const l of this.app.doc.layers) {
      const s = this.sceneOf(l.id);
      if (s) a.pass.sceneAtStart.set(l.id, s);
    }
    this.app.host.send(MSG_RECORD, 0, 0, 0, 1);
  }

  /** First frame of the take that is actually recording, or null during the count-in. */
  originFrame(): number | null {
    const a = this.cap;
    return a && !a.counting ? a.f0 : null;
  }

  /** Drop a take that is in progress. Used when the audio device changes. */
  cancel(): void {
    const a = this.cap;
    const host = this.app.host;
    if (!a || !host) return;
    this.cap = null;
    window.clearTimeout(a.timer);
    if (a.pass) host.send(MSG_RECORD, 0, 0, 0, 0);
    if (a.mic && !a.listening && !a.poly) host.arm(a.layerIdx, false);
    if (a.counting) host.stop();
    this.status('Capture paused.');
  }

  private ready(): boolean { return !!this.app.host && !!this.app.stream && !!this.app.analysis; }

  async enableMic(): Promise<void> {
    const { app } = this;
    if (!app.host || app.stream) return;
    app.stream = await navigator.mediaDevices.getUserMedia(micConstraints());
    app.host.connectMic(app.stream);
    this.status('Mic on. Hold Capture, or hold C.');
  }

  /** Called from the animation frame: finishes a count-in, and grabs the clock once the transport is playing. */
  poll(): void {
    const a = this.cap;
    const host = this.app.host;
    if (!a || !host) return;
    const m = this.app.readMeters();
    if (m) {
      this.lastMark = clockMark(m);
      this.lastMarkAt = performance.now();
    }
    if (a.counting) {
      if (!m || m[M_COUNT_SEQ] === a.seq) return;
      a.counting = false;
      a.mark = clockMark(m);
      a.f0 = host.framesTotal();
      if (a.mic && !a.listening && !a.poly) host.arm(a.layerIdx, true);
      this.beginPass(a);
      this.status('Recording…');
      return;
    }
    if (!a.mark && m && m[M_PLAYING] === 1) a.mark = clockMark(m);
    if (a.pass && m && m[M_PLAYING] === 1) a.marks.push(clockMark(m));
  }

  /** Hold capture, or (`latched`) a recorded pass, which also records without a mic. */
  start(latched = false): void {
    const { app } = this;
    const host = app.host;
    if (!host || this.cap || this.busy) return;
    const mic = !!app.stream && !!app.analysis;
    if (!mic && !latched) { app.fail('The mic is off. Press Enable mic first.'); return; }
    const doc = app.doc;
    const layerIdx = doc.layers.findIndex((l) => l.id === app.ui.layerId);
    if (layerIdx < 0) { app.fail('Pick a layer to capture into.'); return; }
    void host.ctx.resume();
    const meters = app.readMeters();
    const playing = !!meters && meters[M_PLAYING] === 1;
    let sectionIdx = doc.sections.findIndex((s) => s.id === app.ui.sectionId);
    if (playing && doc.sections[meters![M_SECTION]]) sectionIdx = meters![M_SECTION];
    if (sectionIdx < 0) return;
    const section = doc.sections[sectionIdx];
    const layerId = doc.layers[layerIdx].id;
    const a: Active = {
      layerId, layerIdx, f0: host.framesTotal(), mark: playing ? clockMark(meters!) : null,
      sectionId: section.id, timer: 0, counting: false, seq: meters ? meters[M_COUNT_SEQ] : 0,
      latched, mic, listening: this.listeningTo(layerId), pass: null, marks: [], poly: isPoly(doc.layers[layerIdx]),
    };
    if (meters) { this.lastMark = clockMark(meters); this.lastMarkAt = performance.now(); }
    if (!playing && section.feel === 'grid') {
      host.seek(sectionIdx, 0);
      host.countIn(section.bpm);
      a.counting = true;
      a.mark = null;
      this.status('Count-in…');
    } else {
      if (mic && !a.listening && !a.poly) host.arm(layerIdx, true);
      if (!playing) { host.seek(sectionIdx, 0); host.play(); }
      this.beginPass(a);
      this.status(latched ? `Recording a pass${mic ? ` (the mic into ${doc.layers[layerIdx].name})` : ''}… press Record again to stop.` : `Recording into ${doc.layers[layerIdx].name}…`);
    }
    a.timer = window.setTimeout(() => { void this.end(); }, maxTakeSec(a) * 1000);
    this.cap = a;
    if (app.ui.sectionId !== a.sectionId) { app.ui.sectionId = a.sectionId; app.ui.changed(); }
  }

  async end(): Promise<void> {
    const a = this.cap;
    const host = this.app.host;
    if (!a || !host) return;
    this.cap = null;
    window.clearTimeout(a.timer);
    if (a.counting) {
      host.stop();
      this.status('Count-in cancelled.');
      return;
    }
    if (a.pass) {
      host.send(MSG_RECORD, 0, 0, 0, 0);
      a.pass.finish(this.nowSample());
      if (!a.marks.length && this.lastMark) a.marks.push(this.lastMark);
    }
    if (a.mic && !a.listening && !a.poly) host.arm(a.layerIdx, false);
    const n = a.mic ? Math.min(host.framesTotal(), a.f0 + maxTakeSec(a) * FRAME_RATE) - a.f0 : 0;
    const takeOk = a.mic && n >= MIN_TAKE_SEC * FRAME_RATE;
    if (!takeOk && (!a.pass || a.pass.empty)) { this.status(a.mic ? `Ignored: ${(n / FRAME_RATE).toFixed(2)} s is under ${MIN_TAKE_SEC} s.` : 'Nothing was played.'); return; }
    this.busy = true;
    try {
      await this.place(a, takeOk ? n : 0);
      this.onPlaced();
    } catch (e) {
      this.status('');
      this.app.fail(e instanceof Error ? e.message : String(e));
    } finally {
      this.busy = false;
    }
  }

  /** Import an audio file as a take on the selected layer, from the start of the section
   *  (docs/quant.md Q2). The layer gets an input element at mix 1 if it has none, so the take
   *  plays as audio; Feel then stretches it. On a layer set to Poly the file comes back as notes
   *  with their own curves, as a Poly capture does, and no input element is added. One undo step. */
  async importFile(file: File, layerId: string, sectionId: string): Promise<void> {
    const { app } = this;
    const host = app.host;
    if (!host || !app.analysis) throw new Error('Start the audio first.');
    if (this.cap || this.busy) throw new Error('Finish recording first.');
    const layer = app.doc.layers.find((l) => l.id === layerId);
    if (!layer) throw new Error('Pick a layer to import into.');
    const poly = isPoly(layer);
    this.busy = true;
    try {
      this.status(`Reading ${file.name}…`);
      const buf = await host.ctx.decodeAudioData(await file.arrayBuffer());
      const x = toMono(Array.from({ length: buf.numberOfChannels }, (_, c) => buf.getChannelData(c)), host.sampleRate,
        poly ? POLY_MAX_TAKE_SEC : undefined);
      const { audio, frames } = await liveTierOver(await loadKlontBytes(), x, host.sampleRate);
      const n = frames.length / FRAME_STRIDE;
      if (n < MIN_TAKE_SEC * FRAME_RATE) throw new Error(`${file.name} is under ${MIN_TAKE_SEC} s.`);
      this.status(poly ? `Reading the notes of ${(n / FRAME_RATE).toFixed(1)} s…` : `Second pass on ${(n / FRAME_RATE).toFixed(1)} s…`);
      const pass = await importPass(app.analysis, layer, audio, frames, host.sampleRate, this.taught ?? undefined);
      const sp = 'second' in pass ? pass.second : null;
      const pp = 'poly' in pass ? pass.poly : null;
      const take: Take = {
        id: newId('take'), audio: '', frames: '', rate: host.sampleRate, latencyMs: 0, frameCount: n, kind: layer.kind,
        refCents: sp ? sp.refCents : layer.tune.refCents, root: sp ? sp.root : layer.tune.root,
        scale: sp ? sp.scale : layer.tune.scale, bpm: sp ? sp.bpm : null,
      };
      take.audio = `audio-${take.id}`;
      take.frames = `frames-${take.id}`;
      if (pp) {
        take.noteCurves = noteCurvesRef(take.id);
        await app.session.putTake(take.id, { audio, frames, noteCurves: pp.curves });
      } else {
        take.anchors = anchorsRef(take.id);
        take.anchorsVersion = ANCHORS_VERSION;
        await app.session.putTake(take.id, { audio, frames, anchors: sp!.anchors });
      }
      if (!app.doc.sections.some((s) => s.id === sectionId)) throw new Error('The section was removed while importing.');
      const clip = importedClip(app.doc, sectionId, layerId, take, pp ? pp.notes : sp!.notes, pp ? [] : sp!.hits);
      const needsInput = !poly && inputAt(layer.patch) < 0;
      const tune = sp && layer.kind !== 'beat' ? tuneFromTake(sp) : null;
      const state = app.ui.editState(layer).id;
      app.store.batch('Import take', (run) => {
        run('addTake', { take });
        run('setClip', { section: sectionId, layer: layerId, clip });
        if (needsInput) run('addChannelFx', { layer: layerId, type: 'input' });
        if (needsInput) run('setValues', { layer: layerId, state, values: { 'input1.mix': 1 } });
        if (tune) run('setLayer', { id: layerId, tune });
      });
      app.ui.select(layerId, []);
      app.clearError();
      const what = 'poly' in pass
        ? `${pass.poly.notes.length} notes (Poly, ${pass.opts.instrument}, up to ${pass.opts.channels} at once)`
        : `${layer.kind === 'beat' ? `${sp!.hits.length} hits` : `${sp!.notes.length} notes`}, ${sp!.anchors.length / 3} anchors`;
      this.status(`Imported ${file.name}: ${(n / FRAME_RATE).toFixed(1)} s, ${what}.${needsInput ? ' The layer plays it as audio (an input element at mix 1).' : ''}`);
    } finally {
      this.busy = false;
    }
  }

  /** Put the take (n frames; 0: none) and the pass into the document as one undo step. */
  private async place(a: Active, n: number): Promise<void> {
    const { app } = this;
    const host = app.host!;
    const layer = app.doc.layers.find((l) => l.id === a.layerId);
    if (!layer) throw new Error('The layer was removed while recording.');
    if (n === 0 || a.listening) { await this.placePass(a, n); return; }
    if (a.poly && isPoly(layer)) { await this.placePoly(a, n); return; }
    this.status(`Second pass on ${(n / FRAME_RATE).toFixed(1)} s…`);
    const mic = host.readMic(a.f0 * host.frameLen(), n * host.frameLen());
    const frames = host.readFrames(a.f0, n);
    const sp = await app.analysis!.secondPass(mic.slice(), host.sampleRate, frames.slice(), {
      kind: layer.kind, taught: layer.kind === 'beat' ? this.taught ?? undefined : undefined,
    });
    for (let i = 0; i < Math.min(n, sp.pitch.length); i++) frames[i * FRAME_STRIDE] = sp.pitch[i];
    if (!a.mark) throw new Error('The transport did not report a position, so the take cannot be placed.');
    const mark = a.mark;
    const rt = this.roundTripMs;
    const take: Take = {
      id: newId('take'), audio: '', frames: '', rate: host.sampleRate, latencyMs: rt, frameCount: n, kind: layer.kind,
      refCents: sp.refCents, root: sp.root, scale: sp.scale, bpm: sp.bpm,
    };
    take.audio = `audio-${take.id}`;
    take.frames = `frames-${take.id}`;
    take.anchors = anchorsRef(take.id);
    take.anchorsVersion = ANCHORS_VERSION;
    await app.session.putTake(take.id, { audio: mic, frames, anchors: sp.anchors });
    if (!app.doc.sections.some((s) => s.id === a.sectionId)) throw new Error('The section was removed while recording.');
    const clip = placeTake({
      doc: app.doc, sectionId: a.sectionId, layerId: a.layerId, take, startSample: a.f0 * host.frameLen(), mark,
      roundTripSamples: Math.round((rt * host.sampleRate) / 1000), notes: sp.notes, hits: sp.hits,
    });
    const tune = layer.kind === 'beat' ? null : tuneFromTake(sp);
    const cmds = this.passCmds(a, clip);
    app.store.batch(a.pass ? 'Record pass' : 'Capture take', (run) => {
      run('addTake', { take });
      run('setClip', { section: a.sectionId, layer: a.layerId, clip });
      if (tune) run('setLayer', { id: a.layerId, tune });
      for (const c of cmds) run(c.name as 'setClip', c.args as never);
    });
    app.ui.select(a.layerId, []);
    app.clearError();
    const what = layer.kind === 'beat' ? `${sp.hits.length} hits` : `${sp.notes.length} notes`;
    this.status(`Placed ${what}${a.pass ? `, and ${this.passWhat(a.pass)}` : ''}. Round trip ${rt.toFixed(0)} ms.`);
  }

  /** A poly take (docs/poly.md P3): the note model reads the take in the analysis worker; its
   *  notes go into the clip, each naming its curve, which is stored with the take. */
  private async placePoly(a: Active, n: number): Promise<void> {
    const { app } = this;
    const host = app.host!;
    const layer = app.doc.layers.find((l) => l.id === a.layerId)!;
    this.status(`Reading the notes of ${(n / FRAME_RATE).toFixed(1)} s…`);
    const mic = host.readMic(a.f0 * host.frameLen(), n * host.frameLen());
    const frames = host.readFrames(a.f0, n);
    const opts = polyOptions(layer);
    const pp = await app.analysis!.polyPass(mic, host.sampleRate, opts);
    if (!a.mark) throw new Error('The transport did not report a position, so the take cannot be placed.');
    const rt = this.roundTripMs;
    const take: Take = {
      id: newId('take'), audio: '', frames: '', rate: host.sampleRate, latencyMs: rt, frameCount: n, kind: layer.kind,
      refCents: layer.tune.refCents, root: layer.tune.root, scale: layer.tune.scale, bpm: null,
    };
    take.audio = `audio-${take.id}`;
    take.frames = `frames-${take.id}`;
    take.noteCurves = noteCurvesRef(take.id);
    await app.session.putTake(take.id, { audio: mic, frames, noteCurves: pp.curves });
    if (!app.doc.sections.some((s) => s.id === a.sectionId)) throw new Error('The section was removed while recording.');
    const clip = placeTake({
      doc: app.doc, sectionId: a.sectionId, layerId: a.layerId, take, startSample: a.f0 * host.frameLen(), mark: a.mark,
      roundTripSamples: Math.round((rt * host.sampleRate) / 1000), notes: pp.notes, hits: [],
    });
    const cmds = this.passCmds(a, clip);
    app.store.batch(a.pass ? 'Record pass' : 'Capture take', (run) => {
      run('addTake', { take });
      run('setClip', { section: a.sectionId, layer: a.layerId, clip });
      for (const c of cmds) run(c.name as 'setClip', c.args as never);
    });
    app.ui.select(a.layerId, []);
    app.clearError();
    this.status(`Placed ${pp.notes.length} notes (Poly, ${opts.instrument}, up to ${opts.channels} at once; read in ${(pp.ms / 1000).toFixed(1)} s)${a.pass ? `, and ${this.passWhat(a.pass)}` : ''}. Round trip ${rt.toFixed(0)} ms.`);
  }

  /** The pass's clip edits, building on the take's clip when there is one. */
  private passCmds(a: Active, takeClip: Clip | null) {
    if (!a.pass || !a.marks.length) return [];
    const host = this.app.host!;
    const outLatency = Math.round(((host.ctx.outputLatency || host.ctx.baseLatency || 0) * host.sampleRate));
    const editState = (l: Parameters<typeof stateValue>[0]) => this.app.ui.editState(l).id;
    const clips = new Map<string, Clip>(takeClip ? [[`${a.sectionId}/${a.layerId}`, takeClip]] : []);
    return passCommands(this.app.doc, a.pass, { marks: a.marks, outLatency }, host.sampleRate,
      (l, target) => stateValue(l, target, editState(l)), clips, editState);
  }

  private passWhat(p: Pass): string {
    const parts = [`${p.notes.length} played notes`, `${new Set(p.moves.map((m) => `${m.layer}/${m.target}`)).size} lanes`];
    if (p.scenes.length) parts.push(`${p.scenes.length} scene taps`);
    return parts.join(', ');
  }

  /** A pass without a take, or with a take into a listening layer (its audio and features, no notes). */
  private async placePass(a: Active, n: number): Promise<void> {
    const { app } = this;
    const host = app.host!;
    let take: Take | null = null;
    let clip: Clip | null = null;
    if (n > 0) {
      const layer = app.doc.layers.find((l) => l.id === a.layerId)!;
      const mic = host.readMic(a.f0 * host.frameLen(), n * host.frameLen());
      const frames = host.readFrames(a.f0, n);
      if (!a.mark) throw new Error('The transport did not report a position, so the take cannot be placed.');
      const rt = this.roundTripMs;
      take = {
        id: newId('take'), audio: '', frames: '', rate: host.sampleRate, latencyMs: rt, frameCount: n, kind: layer.kind,
        refCents: layer.tune.refCents, root: layer.tune.root, scale: layer.tune.scale, bpm: null,
      };
      take.audio = `audio-${take.id}`;
      take.frames = `frames-${take.id}`;
      await app.session.putTake(take.id, { audio: mic, frames });
      if (!app.doc.sections.some((s) => s.id === a.sectionId)) throw new Error('The section was removed while recording.');
      clip = listeningClip(app.doc, a.sectionId, a.layerId, take, a.f0 * host.frameLen(), a.mark, Math.round((rt * host.sampleRate) / 1000), n / FRAME_RATE);
    }
    const cmds = this.passCmds(a, clip);
    const t = take, c = clip;
    app.store.batch('Record pass', (run) => {
      if (t && c) {
        run('addTake', { take: t });
        run('setClip', { section: a.sectionId, layer: a.layerId, clip: c });
      }
      for (const x of cmds) run(x.name as 'setClip', x.args as never);
    });
    app.clearError();
    this.status(`Recorded ${a.pass ? this.passWhat(a.pass) : 'the pass'}${t ? `, and ${(n / FRAME_RATE).toFixed(1)} s of the mic into a listening layer` : ''}.`);
  }
}
