// Hold to capture into the selected layer. A stopped grid section gets one bar of count-in
// from the engine before the transport and the take start together. The click is not in the mic buffer.

import { micConstraints } from '../m1/klont-host';
import { M_COUNT_SEQ, M_PLAYING, M_SECTION } from '../m1/protocol';
import type { App } from '../m3/app';
import { tuneFromTake } from './tune';
import { newId } from '../project/commands';
import { clockMark, placeTake, type ClockMark } from '../project/overdub';
import type { Take } from '../project/types';

const MIN_TAKE_SEC = 0.5;
const MAX_TAKE_SEC = 59;
const FRAME_RATE = 100;
const FRAME_STRIDE = 25;

interface Active {
  layerId: string;
  layerIdx: number;
  f0: number;
  mark: ClockMark | null;
  sectionId: string;
  timer: number;
  /** Count-in has been asked for and the take has not started. */
  counting: boolean;
  seq: number;
}

export class AppCapture {
  private cap: Active | null = null;
  private busy = false;
  /** Round trip in milliseconds, edited from the project panel. */
  roundTripMs = 30;

  constructor(private readonly app: App, private readonly status: (s: string) => void, private readonly onPlaced: () => void) {}

  get holding(): boolean { return !!this.cap; }

  private ready(): boolean { return !!this.app.host && !!this.app.stream && !!this.app.analysis; }

  async enableMic(): Promise<void> {
    const { app } = this;
    if (!app.host || app.stream) return;
    app.stream = await navigator.mediaDevices.getUserMedia(micConstraints());
    app.host.connectMic(app.stream);
    this.status('Mic on. Hold Capture, or hold Space.');
  }

  /** Called from the animation frame: finishes a count-in, and grabs the clock once the transport is playing. */
  poll(): void {
    const a = this.cap;
    const host = this.app.host;
    if (!a || !host) return;
    const m = this.app.readMeters();
    if (a.counting) {
      if (!m || m[M_COUNT_SEQ] === a.seq) return;
      a.counting = false;
      a.mark = clockMark(m);
      a.f0 = host.framesTotal();
      host.arm(a.layerIdx, true);
      this.status('Recording…');
      return;
    }
    if (!a.mark && m && m[M_PLAYING] === 1) a.mark = clockMark(m);
  }

  start(): void {
    const { app } = this;
    const host = app.host;
    if (!host || this.cap || this.busy) return;
    if (!app.stream || !app.analysis) { app.fail('The mic is off. Press Enable mic first.'); return; }
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
    const a: Active = {
      layerId: doc.layers[layerIdx].id, layerIdx, f0: host.framesTotal(), mark: playing ? clockMark(meters!) : null,
      sectionId: section.id, timer: 0, counting: false, seq: meters ? meters[M_COUNT_SEQ] : 0,
    };
    if (!playing && section.feel === 'grid') {
      host.seek(sectionIdx, 0);
      host.countIn(section.bpm);
      a.counting = true;
      a.mark = null;
      this.status('Count-in…');
    } else {
      host.arm(layerIdx, true);
      if (!playing) { host.seek(sectionIdx, 0); host.play(); }
      this.status(`Recording into ${doc.layers[layerIdx].name}…`);
    }
    a.timer = window.setTimeout(() => { void this.end(); }, MAX_TAKE_SEC * 1000);
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
    host.arm(a.layerIdx, false);
    const n = Math.min(host.framesTotal(), a.f0 + MAX_TAKE_SEC * FRAME_RATE) - a.f0;
    if (n < MIN_TAKE_SEC * FRAME_RATE) { this.status(`Ignored: ${(n / FRAME_RATE).toFixed(2)} s is under ${MIN_TAKE_SEC} s.`); return; }
    this.busy = true;
    try {
      await this.place(a, n);
      this.onPlaced();
    } catch (e) {
      this.status('');
      this.app.fail(e instanceof Error ? e.message : String(e));
    } finally {
      this.busy = false;
    }
  }

  private async place(a: Active, n: number): Promise<void> {
    const { app } = this;
    const host = app.host!;
    const layer = app.doc.layers.find((l) => l.id === a.layerId);
    if (!layer) throw new Error('The layer was removed while recording.');
    this.status(`Second pass on ${(n / FRAME_RATE).toFixed(1)} s…`);
    const mic = host.readMic(a.f0 * host.frameLen(), n * host.frameLen());
    const frames = host.readFrames(a.f0, n);
    const sp = await app.analysis!.secondPass(mic.slice(), host.sampleRate, frames.slice(), { kind: layer.kind });
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
    await app.session.putTake(take.id, { audio: mic, frames });
    if (!app.doc.sections.some((s) => s.id === a.sectionId)) throw new Error('The section was removed while recording.');
    const clip = placeTake({
      doc: app.doc, sectionId: a.sectionId, layerId: a.layerId, take, startSample: a.f0 * host.frameLen(), mark,
      roundTripSamples: Math.round((rt * host.sampleRate) / 1000), notes: sp.notes, hits: sp.hits,
    });
    const tune = layer.kind === 'beat' ? null : tuneFromTake(sp);
    app.store.batch('Capture take', (run) => {
      run('addTake', { take });
      run('setClip', { section: a.sectionId, layer: a.layerId, clip });
      if (tune) run('setLayer', { id: a.layerId, tune });
    });
    app.ui.select(a.layerId, []);
    app.clearError();
    const what = layer.kind === 'beat' ? `${sp.hits.length} hits` : `${sp.notes.length} notes`;
    this.status(`Placed ${what}. Round trip ${rt.toFixed(0)} ms.`);
  }
}
