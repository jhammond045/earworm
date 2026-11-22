// The Perform view (docs/synth.md "Live use" and "Interface sketch", stage S4): play the selected
// layer from a MIDI device or from the screen.
//
// - MIDI notes play the layer their channel is set to (else the selected one); on a beat layer the
//   General MIDI kick, snare, closed and open hat play kit slots 1 to 4. The mod wheel, pitch bend
//   and channel pressure are sources (routed in the Mod view). Other controllers set what Learn
//   mapped them to: press Learn, touch a control on screen, move the controller. A controller
//   sets the value outright and is committed as one undo step once it rests for 500 ms.
// - The map is kept with the project. The last map learned for a device is also kept in settings
//   as that device's default, used by projects that have no map for it.
// - On-screen keys, an XY pad (two values of your choice), the eight macros and stand-ins for the
//   wheel, bend and pressure work without Web MIDI (Safari has none).
// - Scenes: the layer's states as buttons. A tap morphs to the state over the layer's scene time.
//
// The main-thread delay from each MIDI note to its control-ring write is kept, and shown.

import { MACRO_NAMES } from '../gen/params';
import { M_LISTEN, MSG_MORPH_TO, MSG_SET_PERF } from '../m1/protocol';
import { SLOT_NAMES } from '../m2/sounds';
import type { App } from '../m3/app';
import { holdButton } from '../m3/hold';
import { applyOps } from '../project/patch';
import { commands } from '../project/commands';
import type { Kv } from '../project/persist';
import { MIDI_CHANNELS, SCENE_MS, type Layer, type PerfTarget, type Project } from '../project/types';
import { layoutOf } from '../synth/patch';
import { channelLayer, mappedTarget, MOD_WHEEL_CC, notePlays, noteCents, parseMidi, percentile, toDefault, type DefaultMapping } from '../perform/midi';
import { keyLabel, layerFeature, noteName } from './synth-ui';

/** How long a controller must rest before its value is committed (one undo step). */
const REST_MS = 500;
const NOTE_LETTERS = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];
const keysOf = (l: Layer) => layoutOf(l.patch).index;

/** A target's name in the interface. */
function targetName(doc: Project, t: PerfTarget): string {
  const l = doc.layers.find((x) => x.id === t.layer);
  const m = /^macro([1-8])$/.exec(t.key);
  const what = m ? `macro ${MACRO_NAMES[Number(m[1]) - 1]}` : keyLabel(t.key);
  return l ? `${l.name}: ${what}` : what;
}

/** A target's value in the state being edited, 0..1. */
function targetValue(app: App, doc: Project, t: PerfTarget): number {
  const l = doc.layers.find((x) => x.id === t.layer);
  if (!l) return 0;
  const st = app.ui.editState(l);
  const m = /^macro([1-8])$/.exec(t.key);
  if (m) return st.macros[Number(m[1]) - 1] ?? 0.5;
  const lay = layoutOf(l.patch);
  return st.values[t.key] ?? lay.init[lay.index.get(t.key) ?? 0] ?? 0;
}

/** `doc` with the values in `pending` set in each layer's edited state. */
function withValues(app: App, doc: Project, pending: Map<string, { t: PerfTarget; v: number }>): Project {
  let d = doc;
  const byLayer = new Map<string, { values: Record<string, number>; macros: Map<number, number> }>();
  for (const { t, v } of pending.values()) {
    const e = byLayer.get(t.layer) ?? { values: {} as Record<string, number>, macros: new Map<number, number>() };
    const m = /^macro([1-8])$/.exec(t.key);
    if (m) e.macros.set(Number(m[1]) - 1, v);
    else e.values[t.key] = v;
    byLayer.set(t.layer, e);
  }
  for (const [id, e] of byLayer) {
    const l = d.layers.find((x) => x.id === id);
    if (!l) continue;
    const st = app.ui.editState(l);
    if (Object.keys(e.values).length) d = applyOps(d, commands.setValues(d, { layer: id, state: st.id, values: e.values })).doc;
    if (e.macros.size) {
      const macros = Array.from({ length: 8 }, (_, k) => e.macros.get(k) ?? st.macros[k] ?? 0.5);
      d = applyOps(d, commands.setState(d, { layer: id, id: st.id, macros })).doc;
    }
  }
  return d;
}

interface Held { layerIdx: number; cents: number; rec: number }

export interface PerformDeps {
  layer: () => Layer | undefined;
  /** Listen is a session setting kept by the Sound panel. */
  listening: (id: string) => boolean;
  toggleListen: (id: string) => void;
  kv: Kv;
  /** Record a pass (app/capture.ts): press to start, press to stop. */
  toggleRecord: () => void;
  recording: () => boolean;
}

export class PerformPanel {
  /** Values a controller or the pad moved that are not committed yet, by layer/key. */
  private pending = new Map<string, { t: PerfTarget; v: number }>();
  private restTimer = 0;
  private learn: 'off' | 'pick' | PerfTarget = 'off';
  private access: MIDIAccess | null = null;
  private midiError = '';
  private defaults = new Map<string, DefaultMapping[]>();
  private held = new Map<string, Held>();
  /** Milliseconds from a MIDI note event to its control-ring write, newest last. */
  readonly delays: number[] = [];
  private armed = -1;
  private scene = new Map<string, number>();
  private octave = 4;
  private built = '';
  private els: Record<string, HTMLElement> = {};
  private lastPaint = 0;
  readonly midiAvailable: boolean;

  constructor(private app: App, private box: HTMLElement, private deps: PerformDeps) {
    const nomidi = new URLSearchParams(globalThis.location?.search ?? '').has('nomidi');
    this.midiAvailable = !nomidi && typeof navigator !== 'undefined' && typeof navigator.requestMIDIAccess === 'function';
    // Learn: the next control touched (pointer or keyboard focus) is the target.
    const pick = (e: Event) => {
      if (this.learn !== 'pick') return;
      const t = this.targetOf(e.target);
      if (!t) return;
      this.learn = t;
      this.paintLearn();
    };
    document.addEventListener('pointerdown', pick, true);
    document.addEventListener('focusin', pick, true);
    if (this.midiAvailable) void this.connectIfAllowed();
  }

  /** The state a tapped scene left the layer in, if any (a recorded pass starts from it). */
  sceneOf(layerId: string): string | undefined {
    const k = this.scene.get(layerId);
    return k === undefined ? undefined : this.app.doc.layers.find((x) => x.id === layerId)?.states[k]?.id;
  }

  /** The value an on-screen control sets, if Learn can map it. */
  private targetOf(el: EventTarget | null): PerfTarget | null {
    if (!(el instanceof Element)) return null;
    const l = this.deps.layer();
    if (!l) return null;
    const perf = el.closest<HTMLElement>('[data-perf-key]');
    if (perf) return { layer: l.id, key: perf.dataset.perfKey! };
    const knob = el.closest<HTMLElement>('.knob[data-key]');
    if (knob) return { layer: l.id, key: knob.dataset.key! };
    const macro = /^macro-(\d)$/.exec((el as HTMLElement).id ?? '');
    if (macro) return { layer: l.id, key: `macro${Number(macro[1]) + 1}` };
    return null;
  }

  // ---- values from controllers and the pad

  /** Set `t` to `v` now (the engine hears it at once) and commit once nothing has moved for REST_MS. */
  setValue(t: PerfTarget, v: number): void {
    this.app.rec.move(t.layer, t.key, Math.min(1, Math.max(0, v)));
    this.pending.set(`${t.layer}/${t.key}`, { t, v: Math.min(1, Math.max(0, v)) });
    this.app.previewDoc((d) => withValues(this.app, d, this.pending));
    window.clearTimeout(this.restTimer);
    this.restTimer = window.setTimeout(() => this.commit(), REST_MS);
  }

  private commit(): void {
    if (!this.pending.size) return;
    // While a pass records, where the hand left things is committed with the pass (one undo step).
    if (this.app.rec.recording) { this.pending = new Map(); return; }
    const pending = this.pending;
    this.pending = new Map();
    this.app.runRestoring(() => this.app.store.batch('Controller', (run) => {
      const doc = withValues(this.app, this.app.doc, pending);
      for (const l of doc.layers) {
        const before = this.app.doc.layers.find((x) => x.id === l.id);
        if (!before) continue;
        const st = this.app.ui.editState(before);
        const now = l.states.find((s) => s.id === st.id)!;
        const values: Record<string, number> = {};
        for (const k of Object.keys(now.values)) if (now.values[k] !== st.values[k]) values[k] = now.values[k];
        if (Object.keys(values).length) run('setValues', { layer: l.id, state: st.id, values });
        if (now.macros.some((m, k) => m !== st.macros[k])) run('setState', { layer: l.id, id: st.id, macros: now.macros });
      }
    }));
  }

  // ---- Web MIDI

  private async connectIfAllowed(): Promise<void> {
    try {
      const p = await navigator.permissions?.query({ name: 'midi' as PermissionName });
      if (p?.state === 'granted') await this.connect();
    } catch { /* no permission query for midi: wait for the button */ }
  }

  async connect(): Promise<void> {
    if (!this.midiAvailable || this.access) return;
    try {
      this.access = await navigator.requestMIDIAccess({ sysex: false });
      this.midiError = '';
      const hook = () => {
        for (const input of this.access!.inputs.values()) input.onmidimessage = (e) => this.onMidi(input, e);
        this.releaseGone();
        this.render(true);
      };
      this.access.onstatechange = hook;
      hook();
    } catch (e) {
      this.midiError = e instanceof Error ? e.message : String(e);
      this.render(true);
    }
  }

  private inputs(): MIDIInput[] {
    return this.access ? [...this.access.inputs.values()].filter((i) => i.state === 'connected') : [];
  }

  /** Notes held on a device that went away are let go. */
  private releaseGone(): void {
    const live = new Set(this.inputs().map((i) => i.id));
    for (const [k, h] of this.held) {
      if (!live.has(k.split('/')[0])) { this.app.host?.noteOff(h.layerIdx, h.cents); this.app.rec.noteOff(h.rec); this.held.delete(k); }
    }
  }

  private async defaultsFor(device: string): Promise<DefaultMapping[] | undefined> {
    if (!this.defaults.has(device)) {
      try {
        const d = await this.deps.kv.get('settings', `midi:${device}`);
        this.defaults.set(device, Array.isArray(d) ? (d as DefaultMapping[]) : []);
      } catch { this.defaults.set(device, []); }
    }
    return this.defaults.get(device);
  }

  private onMidi(input: MIDIInput, e: MIDIMessageEvent): void {
    const host = this.app.host;
    const ev = e.data ? parseMidi(e.data) : null;
    if (!ev || !host) return;
    const doc = this.app.doc;
    const device = input.name ?? input.id;
    const l = channelLayer(doc, ev.channel, this.app.ui.layerId);
    if (!l) return;
    const i = doc.layers.indexOf(l);
    switch (ev.kind) {
      case 'on': {
        const p = notePlays(l, ev.note);
        if (!p) return;
        const key = `${input.id}/${ev.channel}/${ev.note}`;
        const was = this.held.get(key);
        if (was) { host.noteOff(was.layerIdx, was.cents); this.app.rec.noteOff(was.rec); }
        host.noteOn(i, p.cents, p.slot, ev.velocity);
        const rec = this.app.rec.noteOn(l.id, p.cents, p.slot, ev.velocity);
        // e.timeStamp is when the browser got the message, on the performance.now() clock.
        if (e.timeStamp > 0) {
          this.delays.push(performance.now() - e.timeStamp);
          if (this.delays.length > 4000) this.delays.splice(0, this.delays.length - 4000);
        }
        this.held.set(key, { layerIdx: i, cents: p.cents, rec });
        return;
      }
      case 'off': {
        const key = `${input.id}/${ev.channel}/${ev.note}`;
        const h = this.held.get(key);
        if (h) { host.noteOff(h.layerIdx, h.cents); this.app.rec.noteOff(h.rec); this.held.delete(key); }
        return;
      }
      case 'bend': host.send(MSG_SET_PERF, i, 1, 0, ev.value); this.app.rec.move(l.id, 'bend', ev.value); this.showPerf(1, ev.value); return;
      case 'pressure': host.send(MSG_SET_PERF, i, 2, 0, ev.value); this.app.rec.move(l.id, 'pressure', ev.value); this.showPerf(2, ev.value); return;
      case 'cc': {
        if (ev.cc === MOD_WHEEL_CC) { host.send(MSG_SET_PERF, i, 0, 0, ev.value); this.app.rec.move(l.id, 'wheel', ev.value); this.showPerf(0, ev.value); return; }
        if (typeof this.learn === 'object') {
          const target = this.learn;
          this.learn = 'off';
          if (this.app.run(() => this.app.store.dispatch('learnMidi', { device, channel: ev.channel, cc: ev.cc, target }))) {
            const d = toDefault(this.app.doc, device);
            this.defaults.set(device, d);
            void this.deps.kv.put('settings', `midi:${device}`, d).catch(() => {});
            this.status(`Controller ${ev.cc} on channel ${ev.channel + 1} of ${device} sets ${targetName(this.app.doc, target)}.`);
          }
          this.render(true);
          return;
        }
        const own = doc.perform?.map.some((m) => m.device === device);
        const apply = (defs: DefaultMapping[] | undefined) => {
          const t = mappedTarget(this.app.doc, defs, device, ev.channel, ev.cc, keysOf);
          if (t) this.setValue(t, ev.value);
        };
        if (own || this.defaults.has(device)) apply(this.defaults.get(device));
        else void this.defaultsFor(device).then(apply);
        return;
      }
    }
  }

  // ---- the view

  private statusText = '';
  private status(text: string): void {
    this.statusText = text;
    const s = this.els.status;
    if (s) s.textContent = text;
  }

  private paintLearn(): void {
    const b = this.els.learn as HTMLButtonElement | undefined;
    document.body.classList.toggle('learning', this.learn === 'pick');
    if (!b) return;
    b.setAttribute('aria-pressed', String(this.learn !== 'off'));
    if (this.learn === 'off') return;
    if (!this.inputs().length) this.status('No MIDI device is connected. Connect one, then Learn again.');
    else if (this.learn === 'pick') this.status('Learn: touch a slider here, in Simple (macros) or in Patch.');
    else this.status(`Learn: now move a controller for ${targetName(this.app.doc, this.learn)}.`);
  }

  private showPerf(k: number, v: number): void {
    const input = this.els[`perf${k}`] as HTMLInputElement | undefined;
    if (input && document.activeElement !== input) input.value = String(Math.round(v * 100));
  }

  /** Rebuild when the layer, its states or its patch changed (or `force`), then paint values. */
  render(force = false): void {
    const l = this.deps.layer();
    if (!l) return;
    const sig = [l.id, l.kind, l.states.map((s) => `${s.id}:${s.name}`).join(','), JSON.stringify(this.app.doc.perform ?? null),
      this.inputs().map((x) => x.name).join(','), this.access ? 1 : 0, this.midiError, this.octave, l.patch.voices.length, this.app.doc.layers.map((x) => x.id + x.name).join(',')].join('|');
    if (force || sig !== this.built || !this.box.firstChild) {
      this.built = sig;
      this.build(l);
    }
    this.paintValues(l);
  }

  private fieldset(legend: string, ...kids: HTMLElement[]): HTMLFieldSetElement {
    const f = document.createElement('fieldset');
    const lg = document.createElement('legend');
    lg.textContent = legend;
    f.append(lg, ...kids);
    return f;
  }

  private row(...kids: (HTMLElement | string)[]): HTMLDivElement {
    const r = document.createElement('div');
    r.className = 'row';
    r.append(...kids);
    return r;
  }

  private button(text: string, onClick: () => void, title = ''): HTMLButtonElement {
    const b = document.createElement('button');
    b.type = 'button';
    b.textContent = text;
    if (title) b.title = title;
    b.addEventListener('click', onClick);
    return b;
  }

  private slider(label: string, min: number, max: number, value: number): HTMLLabelElement {
    const lab = document.createElement('label');
    const t = document.createElement('span');
    t.textContent = label;
    const input = document.createElement('input');
    input.type = 'range';
    input.min = String(min);
    input.max = String(max);
    input.step = '1';
    input.value = String(value);
    input.setAttribute('aria-label', label);
    lab.append(t, input);
    return lab;
  }

  private build(l: Layer): void {
    const app = this.app;
    const doc = app.doc;
    const i = doc.layers.indexOf(l);
    this.els = {};
    const box = this.box;
    box.replaceChildren();

    // Layer, Listen, Arm, MIDI.
    const pickLayer = document.createElement('select');
    pickLayer.setAttribute('aria-label', 'Layer to play');
    for (const x of doc.layers) pickLayer.append(new Option(x.name, x.id, false, x.id === l.id));
    pickLayer.addEventListener('change', () => { app.ui.layerId = pickLayer.value; app.ui.changed(); });
    const layLab = document.createElement('label');
    layLab.append('Layer ', pickLayer);
    const listen = this.button(this.deps.listening(l.id) ? 'Listening' : 'Listen', () => { this.deps.toggleListen(l.id); this.render(true); },
      'The mic drives this layer\'s sung features while its notes come from MIDI or the keys.');
    listen.setAttribute('aria-pressed', String(this.deps.listening(l.id)));
    this.els.listen = listen;
    const arm = this.button(this.armed === i ? 'Armed' : 'Arm', () => {
      const host = app.host;
      if (!host) { app.fail('Press Start first.'); return; }
      if (!app.stream) { app.fail('The mic is off. Press Enable mic first.'); return; }
      const on = this.armed !== i;
      if (this.armed >= 0) host.arm(this.armed, false);
      if (on) host.arm(i, true);
      this.armed = on ? i : -1;
      this.render(true);
    }, 'Live follow: your voice plays this layer.');
    arm.setAttribute('aria-pressed', String(this.armed === i));

    const midiLine = document.createElement('span');
    midiLine.className = 'dim';
    const kids: HTMLElement[] = [layLab, listen, arm, midiLine];
    if (!this.midiAvailable) {
      midiLine.textContent = 'This browser has no Web MIDI (Safari, for one). The keys, pad, wheel and macros below still play.';
    } else if (!this.access) {
      midiLine.textContent = this.midiError ? `MIDI: ${this.midiError}` : 'MIDI: not connected.';
      kids.push(this.button('Connect MIDI', () => { void this.connect(); }, 'The browser asks before a page may use MIDI devices.'));
    } else {
      const names = this.inputs().map((x) => x.name ?? x.id);
      midiLine.textContent = names.length ? `MIDI: ${names.join(', ')}` : 'MIDI: no device connected.';
    }
    const learn = this.button('Learn', () => {
      this.learn = this.learn === 'off' ? 'pick' : 'off';
      if (this.learn === 'off') this.status('');
      this.paintLearn();
    }, 'Press Learn, touch a control on screen, then move a controller.');
    learn.setAttribute('aria-pressed', String(this.learn !== 'off'));
    learn.disabled = !this.midiAvailable;
    this.els.learn = learn;
    kids.push(learn);
    const rec = this.button(this.deps.recording() ? 'Recording' : 'Record', () => { this.deps.toggleRecord(); this.render(true); },
      'Press to record a pass, press again to stop: notes, knob moves, the wheel and scene taps go into the clips as one undo step.');
    rec.setAttribute('aria-pressed', String(this.deps.recording()));
    this.els.record = rec;
    kids.push(rec);
    const status = document.createElement('p');
    status.className = 'hint';
    status.setAttribute('role', 'status');
    status.textContent = this.statusText;
    this.els.status = status;
    box.append(this.row(...kids), status);

    // Sung features.
    const meters = document.createElement('div');
    meters.className = 'meters feat';
    for (const name of ['volume', 'intensity', 'hardness', 'pitch'] as const) {
      const m = document.createElement('div');
      m.className = 'meter';
      m.dataset.feature = name;
      const n = document.createElement('span');
      n.className = 'name';
      n.textContent = name;
      const t = document.createElement('span');
      t.className = 'track';
      const fill = document.createElement('span');
      fill.className = 'fill';
      t.append(fill);
      const num = document.createElement('span');
      num.className = 'num';
      m.append(n, t, num);
      meters.append(m);
    }
    this.els.meters = meters;
    box.append(this.fieldset('Sung', meters));

    // Keys.
    const keys = document.createElement('div');
    keys.className = 'perfkeys';
    keys.setAttribute('role', 'group');
    keys.setAttribute('aria-label', l.kind === 'beat' ? 'Kit pads' : 'Keys');
    const play = (b: HTMLButtonElement, slot: number, cents: number) => {
      let on: Held | null = null;
      holdButton(b, () => {
        const host = app.host;
        if (!host) { app.fail('Press Start first.'); return; }
        void host.ctx.resume();
        host.noteOn(i, cents, slot, 0.8);
        on = { layerIdx: i, cents, rec: app.rec.noteOn(l.id, cents, slot, 0.8) };
      }, () => { if (on) { app.host?.noteOff(on.layerIdx, on.cents); app.rec.noteOff(on.rec); } on = null; });
    };
    if (l.kind === 'beat') {
      SLOT_NAMES.forEach((name, k) => {
        const b = document.createElement('button');
        b.type = 'button';
        b.className = 'pad';
        b.textContent = `${k + 1} ${name}`;
        play(b, k, l.kit[k]?.cents ?? 4800);
        keys.append(b);
      });
      box.append(this.fieldset('Pads', keys));
    } else {
      for (let n = 0; n <= 12; n++) {
        const note = 12 * (this.octave + 1) + n;
        const b = document.createElement('button');
        b.type = 'button';
        const name = NOTE_LETTERS[n % 12];
        b.className = name.length > 1 ? 'key black' : 'key';
        b.textContent = `${name}${Math.floor(note / 12) - 1}`;
        b.setAttribute('aria-label', `Play ${name}${Math.floor(note / 12) - 1}`);
        play(b, 0, noteCents(note));
        keys.append(b);
      }
      const down = this.button('Octave −', () => { this.octave = Math.max(0, this.octave - 1); this.render(true); });
      const up = this.button('Octave +', () => { this.octave = Math.min(8, this.octave + 1); this.render(true); });
      box.append(this.fieldset('Keys', keys, this.row(down, up)));
    }

    // Macros, XY pad, wheel, bend, pressure.
    const macros = document.createElement('div');
    macros.className = 'sliders';
    MACRO_NAMES.forEach((name, k) => {
      const lab = this.slider(name, 0, 100, 50);
      const input = lab.querySelector('input')!;
      input.dataset.perfKey = `macro${k + 1}`;
      input.setAttribute('aria-label', `${name} macro`);
      input.addEventListener('input', () => this.setValue({ layer: l.id, key: `macro${k + 1}` }, Number(input.value) / 100));
      this.els[`macro${k}`] = input;
      macros.append(lab);
    });
    box.append(this.fieldset('Macros', macros));

    const perf = document.createElement('div');
    perf.className = 'sliders';
    ([['Mod wheel', 0, 0, 100], ['Pitch bend', 1, -100, 100], ['Pressure', 2, 0, 100]] as const).forEach(([name, k, lo, hi]) => {
      const lab = this.slider(name, lo, hi, 0);
      const input = lab.querySelector('input')!;
      const send = () => {
        app.host?.send(MSG_SET_PERF, i, k, 0, Number(input.value) / 100);
        app.rec.move(l.id, (['wheel', 'bend', 'pressure'] as const)[k], Number(input.value) / 100);
      };
      input.addEventListener('input', send);
      // The bend springs back to the centre when let go, as a bend wheel does.
      if (k === 1) input.addEventListener('pointerup', () => { input.value = '0'; send(); });
      this.els[`perf${k}`] = input;
      perf.append(lab);
    });
    const perfHint = document.createElement('p');
    perfHint.className = 'hint';
    perfHint.textContent = 'The wheel, bend and pressure move only what routes take from them: add one in the Mod view.';
    box.append(this.fieldset('Expression', perf, perfHint));

    box.append(this.fieldset('XY pad', ...this.xyPad(l)));

    // Scenes.
    const scenes = document.createElement('div');
    scenes.className = 'row';
    l.states.forEach((s, k) => {
      const b = this.button(s.name || `State ${k + 1}`, () => {
        const host = app.host;
        if (!host) { app.fail('Press Start first.'); return; }
        host.send(MSG_MORPH_TO, i, k, 0, l.sceneMs ?? SCENE_MS);
        app.rec.scene(l.id, s.id, l.sceneMs ?? SCENE_MS);
        this.scene.set(l.id, k);
        scenes.querySelectorAll('button').forEach((x, j) => x.setAttribute('aria-pressed', String(j === k)));
      });
      b.setAttribute('aria-pressed', String((this.scene.get(l.id) ?? -1) === k));
      scenes.append(b);
    });
    const time = document.createElement('input');
    time.type = 'number';
    time.min = '0';
    time.max = '60000';
    time.step = '10';
    time.value = String(l.sceneMs ?? SCENE_MS);
    time.setAttribute('aria-label', 'Scene time in milliseconds');
    time.addEventListener('change', () => { app.run(() => app.store.dispatch('setLayer', { id: l.id, sceneMs: Number(time.value) })); });
    const tl = document.createElement('label');
    tl.append('Morph ', time, ' ms');
    const sceneHint = document.createElement('p');
    sceneHint.className = 'hint';
    sceneHint.textContent = l.states.length < 2
      ? 'Each state of the layer is a scene. Add a state in the Simple view to have two.'
      : 'A tap morphs to that state. While the song plays a clip with morph markers, the markers win.';
    box.append(this.fieldset('Scenes', scenes, this.row(tl), sceneHint));

    // The controller map, channels, timing.
    box.append(this.fieldset('MIDI', ...this.midiSection(l)));
  }

  private xyPad(l: Layer): HTMLElement[] {
    const app = this.app;
    const pf = app.doc.perform;
    const options = (sel: HTMLSelectElement, cur: PerfTarget | null) => {
      sel.append(new Option('(nothing)', ''));
      MACRO_NAMES.forEach((name, k) => sel.append(new Option(`macro ${name}`, `macro${k + 1}`)));
      for (const key of layoutOf(l.patch).keys) sel.append(new Option(keyLabel(key), key));
      sel.value = cur && cur.layer === l.id ? cur.key : '';
    };
    const axes = [0, 1].map((a) => {
      const sel = document.createElement('select');
      sel.setAttribute('aria-label', a === 0 ? 'What the pad moves across' : 'What the pad moves up');
      options(sel, pf?.xy[a] ?? null);
      return sel;
    });
    const commitAxes = () => {
      const t = (s: HTMLSelectElement) => (s.value ? { layer: l.id, key: s.value } : null);
      app.run(() => app.store.dispatch('setPerform', { xy: [t(axes[0]), t(axes[1])] }));
    };
    axes.forEach((s) => s.addEventListener('change', commitAxes));
    const pad = document.createElement('div');
    pad.className = 'xy';
    pad.tabIndex = 0;
    pad.setAttribute('role', 'group');
    pad.setAttribute('aria-label', 'XY pad. Drag, or use the arrow keys.');
    const dot = document.createElement('span');
    dot.className = 'dot';
    pad.append(dot);
    this.els.xy = pad;
    const targets = () => (app.doc.perform?.xy ?? [null, null]).map((t) => (t && t.layer === l.id ? t : null));
    const move = (x: number, y: number) => {
      const [tx, ty] = targets();
      if (tx) this.setValue(tx, x);
      if (ty) this.setValue(ty, y);
      dot.style.left = `${x * 100}%`;
      dot.style.top = `${(1 - y) * 100}%`;
    };
    const fromEvent = (e: PointerEvent) => {
      const r = pad.getBoundingClientRect();
      move(Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)), Math.min(1, Math.max(0, 1 - (e.clientY - r.top) / r.height)));
    };
    let dragging = false;
    pad.addEventListener('pointerdown', (e) => {
      if (!targets().some(Boolean)) { this.status('Pick what the pad moves first.'); return; }
      dragging = true;
      try { pad.setPointerCapture(e.pointerId); } catch { /* synthetic pointer */ }
      fromEvent(e);
    });
    pad.addEventListener('pointermove', (e) => { if (dragging) fromEvent(e); });
    pad.addEventListener('pointerup', () => { dragging = false; });
    pad.addEventListener('pointercancel', () => { dragging = false; });
    pad.addEventListener('keydown', (e) => {
      const d = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, 1], ArrowDown: [0, -1] }[e.key];
      if (!d) return;
      e.preventDefault();
      const [tx, ty] = targets();
      const step = e.shiftKey ? 0.1 : 0.02;
      const x = tx ? targetValue(app, withValues(app, app.doc, this.pending), tx) : 0.5;
      const y = ty ? targetValue(app, withValues(app, app.doc, this.pending), ty) : 0.5;
      move(Math.min(1, Math.max(0, x + d[0] * step)), Math.min(1, Math.max(0, y + d[1] * step)));
    });
    const lx = document.createElement('label');
    lx.append('Across ', axes[0]);
    const ly = document.createElement('label');
    ly.append('Up ', axes[1]);
    return [this.row(lx, ly), pad];
  }

  private midiSection(l: Layer): HTMLElement[] {
    const app = this.app;
    const doc = app.doc;
    const out: HTMLElement[] = [];
    const map = doc.perform?.map ?? [];
    const list = document.createElement('ul');
    list.className = 'maplist';
    for (const m of map) {
      const li = document.createElement('li');
      li.append(`${m.device}, channel ${m.channel + 1}, controller ${m.cc} → ${targetName(doc, m.target)} `);
      li.append(this.button('Forget', () => { app.run(() => app.store.dispatch('forgetMidi', { device: m.device, channel: m.channel, cc: m.cc })); }));
      list.append(li);
    }
    const none = document.createElement('p');
    none.className = 'hint';
    none.textContent = map.length ? 'Kept with the project. The last map learned for a device is also its default for projects that have none.'
      : 'No controllers learned yet. Notes, the mod wheel, bend and pressure work without Learn.';
    out.push(list, none);

    const chans = document.createElement('details');
    const sum = document.createElement('summary');
    sum.textContent = 'Which layer each MIDI channel plays';
    chans.append(sum);
    const grid = document.createElement('div');
    grid.className = 'sliders';
    for (let c = 0; c < MIDI_CHANNELS; c++) {
      const sel = document.createElement('select');
      sel.setAttribute('aria-label', `Layer MIDI channel ${c + 1} plays`);
      sel.append(new Option('the selected layer', ''));
      for (const x of doc.layers) sel.append(new Option(x.name, x.id));
      sel.value = doc.perform?.channels[c] ?? '';
      sel.addEventListener('change', () => { app.run(() => app.store.dispatch('setPerform', { channel: c, layer: sel.value })); });
      const lab = document.createElement('label');
      lab.append(`Ch ${c + 1} `, sel);
      grid.append(lab);
    }
    chans.append(grid);
    const beatHint = document.createElement('p');
    beatHint.className = 'hint';
    beatHint.textContent = 'On a beat layer, notes 36, 38, 42 and 46 (General MIDI kick, snare, closed and open hat) play kit slots 1 to 4.';
    chans.append(beatHint);
    out.push(chans);

    const timing = document.createElement('p');
    timing.className = 'hint';
    timing.setAttribute('aria-live', 'off');
    this.els.timing = timing;
    out.push(this.row(timing, this.button('Clear', () => { this.delays.length = 0; this.paintTiming(); }, 'Forget the timings so far')));
    void l;
    return out;
  }

  private paintTiming(): void {
    const t = this.els.timing;
    if (!t) return;
    const d = this.delays;
    t.textContent = d.length
      ? `MIDI note to engine, last ${d.length}: median ${percentile(d, 50).toFixed(2)} ms, 95% ${percentile(d, 95).toFixed(2)} ms, worst ${Math.max(...d).toFixed(2)} ms.`
      : 'MIDI note to engine: no notes yet.';
  }

  private paintValues(l: Layer): void {
    const doc = withValues(this.app, this.app.doc, this.pending);
    const cur = doc.layers.find((x) => x.id === l.id) ?? l;
    const st = this.app.ui.editState(cur);
    for (let k = 0; k < 8; k++) {
      const input = this.els[`macro${k}`] as HTMLInputElement | undefined;
      if (input && document.activeElement !== input) input.value = String(Math.round((st.macros[k] ?? 0.5) * 100));
    }
    const dot = this.els.xy?.querySelector<HTMLElement>('.dot');
    if (dot) {
      const [tx, ty] = (doc.perform?.xy ?? [null, null]).map((t) => (t && t.layer === l.id ? t : null));
      dot.hidden = !tx && !ty;
      dot.style.left = `${(tx ? targetValue(this.app, doc, tx) : 0.5) * 100}%`;
      dot.style.top = `${(1 - (ty ? targetValue(this.app, doc, ty) : 0.5)) * 100}%`;
    }
    this.paintTiming();
    this.paintLearn();
  }

  /** Every animation frame: the sung meters and the Listen light; the timing a few times a second. */
  frame(m: Float32Array | null): void {
    const l = this.deps.layer();
    if (!l || !m) return;
    this.els.meters?.querySelectorAll<HTMLElement>('.meter').forEach((el) => {
      const name = el.dataset.feature!;
      const x = layerFeature(m, name);
      const fill = el.querySelector<HTMLElement>('.fill')!;
      const num = el.querySelector<HTMLElement>('.num')!;
      if (name === 'pitch') {
        fill.style.width = Number.isFinite(x) ? `${Math.min(1, Math.max(0, (x - 3600) / 2400)) * 100}%` : '0';
        num.textContent = Number.isFinite(x) ? noteName(x) : '—';
      } else {
        fill.style.width = Number.isFinite(x) ? `${Math.min(1, Math.max(0, x)) * 100}%` : '0';
        num.textContent = Number.isFinite(x) ? x.toFixed(2) : '—';
      }
    });
    const i = this.app.doc.layers.indexOf(l);
    this.els.listen?.classList.toggle('down', ((m[M_LISTEN] >>> 0) & (1 << i)) !== 0);
    const rec = this.els.record;
    if (rec && rec.getAttribute('aria-pressed') !== String(this.deps.recording())) {
      rec.setAttribute('aria-pressed', String(this.deps.recording()));
      rec.textContent = this.deps.recording() ? 'Recording' : 'Record';
    }
    // Values a controller is moving, the pad's dot and the timing, a few times a second.
    if (performance.now() - this.lastPaint > 100) {
      this.lastPaint = performance.now();
      this.paintValues(l);
    }
  }
}
