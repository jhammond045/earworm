// Overdub: hold to record a take on the selected layer while the track plays, run the second pass,
// and place the result in the clip so it lines up with what the singer heard (TDD "Devices and
// calibration", take alignment). There is no feedback check and no automatic mute: the mic is only
// ever turned on by the user, and it stays on until the page closes.

import { micConstraints } from '../m1/klont-host';
import { M_PLAYING, M_SECTION } from '../m1/protocol';
import { newId } from '../project/commands';
import { clockMark, placeTake, type ClockMark } from '../project/overdub';
import type { Take } from '../project/types';
import { $, type App } from './app';
import { measureRoundTrip } from './roundtrip';

/** Takes shorter than this are ignored; the mic ring holds 60 s (same limits as the M2 page). */
const MIN_TAKE_SEC = 0.5;
const MAX_TAKE_SEC = 59;
const FRAME_RATE = 100;
const FRAME_STRIDE = 25;

interface Active {
  layerId: string;
  layerIdx: number;
  f0: number;
  mark: ClockMark | null;
  /** The press started the transport; the mark is taken once it reports playing. */
  startedTransport: boolean;
  sectionId: string;
  timer: number;
  poll: number;
}

export class Capture {
  private cap: Active | null = null;
  private busy = false;
  private rtSource: 'estimate' | 'saved' | 'measured' = 'estimate';

  constructor(private readonly app: App) {
    const hold = $('hold');
    hold.addEventListener('pointerdown', (e) => { hold.setPointerCapture(e.pointerId); this.start(); });
    hold.addEventListener('pointerup', () => { void this.end(); });
    hold.addEventListener('pointercancel', () => { void this.end(); });

    const typing = (t: EventTarget | null) =>
      (t instanceof HTMLInputElement && (t.type === 'text' || t.type === 'number' || t.type === 'search')) ||
      t instanceof HTMLTextAreaElement || t instanceof HTMLSelectElement || (t instanceof HTMLElement && t.isContentEditable);
    window.addEventListener('keydown', (e) => {
      if (e.code !== 'Space' || typing(e.target) || !this.ready()) return;
      e.preventDefault();
      if (!e.repeat) this.start();
    });
    window.addEventListener('keyup', (e) => {
      if (e.code !== 'Space' || typing(e.target) || !this.ready()) return;
      e.preventDefault();
      void this.end();
    });
    window.addEventListener('blur', () => { void this.end(); });

    const field = $<HTMLInputElement>('roundTrip');
    field.addEventListener('change', () => {
      const v = Number(field.value);
      if (!Number.isFinite(v) || v < 0) { this.loadRoundTrip(); return; }
      this.rtSource = 'saved';
      void this.saveRoundTrip(v);
      this.showRtNote();
    });
    $('measureRt').addEventListener('click', () => { void this.measure(); });
  }

  private ready(): boolean { return !!this.app.host && !!this.app.stream && !!this.app.analysis; }

  /** Called when the mic becomes available. */
  enable(): void {
    ($('hold') as HTMLButtonElement).disabled = false;
    ($('measureRt') as HTMLButtonElement).disabled = false;
    this.status('Ready. Hold Space and sing or beatbox.');
    void this.loadRoundTrip();
  }

  private status(s: string): void { $('capStatus').textContent = s; }

  // ---------------------------------------------------------------- round trip

  /** Settings key for this input and output pair (device labels; ids are not stable across sessions). */
  private pairKey(): string {
    const input = this.app.stream?.getAudioTracks()[0]?.label || 'default input';
    const ctx = this.app.ctx as (AudioContext & { sinkId?: string | { type: string } }) | null;
    const sink = ctx?.sinkId;
    const output = (typeof sink === 'string' ? sink : '') || 'default output';
    return `roundTrip/${input}|${output}`;
  }

  private estimateMs(): number {
    const c = this.app.ctx;
    return c ? ((c.baseLatency || 0) + (c.outputLatency || 0)) * 1000 + 10 : 10;
  }

  async loadRoundTrip(): Promise<void> {
    const saved = (await this.app.session.kv.get('settings', this.pairKey())) as number | undefined;
    const field = $<HTMLInputElement>('roundTrip');
    if (typeof saved === 'number' && Number.isFinite(saved)) { field.value = saved.toFixed(1); this.rtSource = 'saved'; }
    else { field.value = this.estimateMs().toFixed(1); this.rtSource = 'estimate'; }
    this.showRtNote();
  }

  private async saveRoundTrip(ms: number): Promise<void> {
    await this.app.session.kv.put('settings', this.pairKey(), ms);
  }

  private showRtNote(): void {
    $('rtNote').textContent = this.rtSource === 'estimate'
      ? 'Estimate: Chrome\'s reported base and output latency plus 10 ms. Measure it for a better figure.'
      : this.rtSource === 'measured' ? 'Measured and saved for this input and output pair.' : 'Saved for this input and output pair.';
  }

  roundTripMs(): number {
    const v = Number($<HTMLInputElement>('roundTrip').value);
    return Number.isFinite(v) && v >= 0 ? v : this.estimateMs();
  }

  private async measure(): Promise<void> {
    const { ctx, stream, host } = this.app;
    if (!ctx || !stream) { this.app.fail('Enable the mic first.'); return; }
    const btn = $<HTMLButtonElement>('measureRt');
    btn.disabled = true;
    host?.stop();
    this.status('Measuring: five chirps...');
    try {
      const r = await measureRoundTrip(ctx, stream);
      if (r.medianMs === null) { this.app.fail(r.error ?? 'No round trip found.'); this.status('Round trip not found.'); return; }
      $<HTMLInputElement>('roundTrip').value = r.medianMs.toFixed(1);
      this.rtSource = 'measured';
      await this.saveRoundTrip(r.medianMs);
      this.showRtNote();
      this.app.clearError();
      this.status(`Round trip ${r.medianMs.toFixed(1)} ms (${r.found} of 5 chirps found).`);
    } catch (e) {
      this.app.fail(e instanceof Error ? e.message : String(e));
    } finally {
      btn.disabled = false;
    }
  }

  // ---------------------------------------------------------------- hold to capture

  start(): void {
    const { app } = this;
    const host = app.host;
    if (!host || this.cap || this.busy) return;
    if (!app.stream || !app.analysis) { app.fail('The mic is off. Press Enable mic (or Start) first.'); return; }
    const doc = app.doc;
    const layerIdx = doc.layers.findIndex((l) => l.id === app.ui.layerId);
    if (layerIdx < 0) { app.fail('Pick a layer to capture into.'); return; }
    void host.ctx.resume();
    const m = app.readMeters();
    const playing = !!m && m[M_PLAYING] === 1;
    // Capture into the section that is playing, else the selected one.
    let sectionIdx = doc.sections.findIndex((s) => s.id === app.ui.sectionId);
    if (playing && doc.sections[m![M_SECTION]]) sectionIdx = m![M_SECTION];
    if (sectionIdx < 0) return;
    const f0 = host.framesTotal();
    host.arm(layerIdx, true);
    if (!playing) { host.seek(sectionIdx, 0); host.play(); }
    const a: Active = {
      layerId: doc.layers[layerIdx].id, layerIdx, f0, mark: playing ? clockMark(m!) : null, startedTransport: !playing,
      sectionId: doc.sections[sectionIdx].id, timer: 0, poll: 0,
    };
    a.timer = window.setTimeout(() => { void this.end(); }, MAX_TAKE_SEC * 1000);
    // After a transport start, wait for the first meter block that says playing and use it as the mark.
    if (!a.mark) {
      a.poll = window.setInterval(() => {
        const mm = app.readMeters();
        if (mm && mm[M_PLAYING] === 1) { a.mark = clockMark(mm); window.clearInterval(a.poll); }
      }, 15);
    }
    this.cap = a;
    $('hold').classList.add('down');
    if (app.ui.sectionId !== a.sectionId) { app.ui.sectionId = a.sectionId; app.ui.changed(); }
    this.status(`Capturing into ${doc.layers[layerIdx].name}...`);
  }

  async end(): Promise<void> {
    const a = this.cap;
    const host = this.app.host;
    if (!a || !host) return;
    this.cap = null;
    window.clearTimeout(a.timer);
    window.clearInterval(a.poll);
    host.arm(a.layerIdx, false);
    $('hold').classList.remove('down');
    const n = Math.min(host.framesTotal(), a.f0 + MAX_TAKE_SEC * FRAME_RATE) - a.f0;
    if (n < MIN_TAKE_SEC * FRAME_RATE) { this.status(`Ignored: ${(n / FRAME_RATE).toFixed(2)} s is under ${MIN_TAKE_SEC} s.`); return; }
    this.busy = true;
    try {
      await this.process(a, n);
    } catch (e) {
      this.status('');
      this.app.fail(e instanceof Error ? e.message : String(e));
    } finally {
      this.busy = false;
    }
  }

  private async process(a: Active, n: number): Promise<void> {
    const { app } = this;
    const host = app.host!;
    const analysis = app.analysis!;
    const L = host.frameLen();
    const sr = host.sampleRate;
    const layer = app.doc.layers.find((l) => l.id === a.layerId);
    if (!layer) throw new Error('The layer was removed while recording.');
    this.status(`Second pass on ${(n / FRAME_RATE).toFixed(1)} s...`);
    const mic = host.readMic(a.f0 * L, n * L);
    const frames = host.readFrames(a.f0, n);
    const sp = await analysis.secondPass(mic.slice(), sr, frames.slice(), { kind: layer.kind });
    // The repaired pitch replaces the live tier's raw pitch in slot 0; the tuner reads it from there.
    for (let i = 0; i < Math.min(n, sp.pitch.length); i++) frames[i * FRAME_STRIDE] = sp.pitch[i];

    const mark = a.mark;
    if (!mark) throw new Error('The transport did not report a position, so the take cannot be placed.');
    const rt = this.roundTripMs();
    const take: Take = {
      id: newId('take'), audio: '', frames: '', rate: sr, latencyMs: rt, frameCount: n, kind: layer.kind,
      refCents: sp.refCents, root: sp.root, scale: sp.scale, bpm: sp.bpm,
    };
    take.audio = `audio-${take.id}`;
    take.frames = `frames-${take.id}`;
    await app.session.putTake(take.id, { audio: mic, frames });

    const doc = app.doc;
    if (!doc.sections.some((s) => s.id === a.sectionId)) throw new Error('The section was removed while recording.');
    if (!doc.layers.some((l) => l.id === a.layerId)) throw new Error('The layer was removed while recording.');
    const clip = placeTake({
      doc, sectionId: a.sectionId, layerId: a.layerId, take, startSample: a.f0 * L, mark,
      roundTripSamples: Math.round((rt * sr) / 1000), notes: sp.notes, hits: sp.hits,
    });
    app.store.batch('Capture take', (run) => {
      run('addTake', { take });
      run('setClip', { section: a.sectionId, layer: a.layerId, clip });
    });
    app.ui.select(a.layerId, []);
    app.clearError();
    const what = layer.kind === 'beat' ? `${sp.hits.length} hits` : `${sp.notes.length} notes`;
    this.status(`Take of ${(n / FRAME_RATE).toFixed(1)} s placed: ${what}, round trip ${rt.toFixed(1)} ms (${this.rtSource}).`);
  }

  /** The mic, asked for on demand (Start, or Enable mic later). */
  async enableMic(): Promise<void> {
    const { app } = this;
    if (!app.host || app.stream) return;
    app.stream = await navigator.mediaDevices.getUserMedia(micConstraints());
    app.host.connectMic(app.stream);
    this.enable();
  }
}
