// DOM panels: project bar, section strip and fields, layer cards. Each builds its controls once
// per structure (same sections, same layers) and afterwards only updates values, so a slider
// that is being dragged is never rebuilt under the pointer.

import { SCALES } from '../host/protocol';
import { soundToSet } from '../m1/layer';
import { LAYER_DEPTH_LOUD, LAYER_DEPTH_MOUTH, LAYER_FOLLOW, LAYER_LEVEL, LAYER_PAN, MSG_SET_LAYER, M_PLAYING } from '../m1/protocol';
import { BEAT_KITS, MELODY_SOUNDS } from '../m2/sounds';
import { commands } from '../project/commands';
import { applyOps } from '../project/patch';
import { storageEstimate } from '../project/persist';
import { MAX_LAYERS, type Layer, type LayerKind, type Project, type State, type Tune } from '../project/types';
import { $, type App } from './app';

// ---------------------------------------------------------------- small builders

export function h<K extends keyof HTMLElementTagNameMap>(tag: K, props: Partial<HTMLElementTagNameMap[K]> & { class?: string } = {},
                                                  ...kids: (Node | string)[]): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  const { class: cls, ...rest } = props as Record<string, unknown>;
  if (cls) e.className = cls as string;
  Object.assign(e, rest);
  if ((tag === 'input' || tag === 'select') && !(e as HTMLInputElement).name) (e as HTMLInputElement).name = `f${++uid}`;
  e.append(...kids);
  return e;
}

let uid = 0;
export const nextId = (p: string) => `${p}-${++uid}`;

export type Updater = (doc: Project) => void;

export const focused = (el: Element) => document.activeElement === el;

export interface RangeOpts {
  label: string; min: number; max: number; step: number; digits?: number; unit?: string;
  get: (doc: Project) => number;
  /** Fires on every move of the slider (the engine hears it; the document is not touched). */
  preview: (v: number) => void;
  /** Fires on release or a key press (one command). */
  commit: (v: number) => void;
}

export function rangeCtl(parent: HTMLElement, o: RangeOpts): Updater {
  const id = nextId('rng');
  const out = h('output', { class: 'mono small' });
  out.setAttribute('for', id);
  out.setAttribute('aria-live', 'off');
  const input = h('input', { type: 'range', id, min: String(o.min), max: String(o.max), step: String(o.step) });
  const show = () => { out.textContent = `${Number(input.value).toFixed(o.digits ?? 2)}${o.unit ?? ''}`; };
  input.addEventListener('input', () => { show(); o.preview(Number(input.value)); });
  input.addEventListener('change', () => { show(); o.commit(Number(input.value)); });
  parent.append(h('div', { class: 'ctl' }, h('label', { htmlFor: id }, o.label), out, input));
  return (doc) => {
    if (focused(input)) return;
    input.value = String(o.get(doc));
    show();
  };
}

interface SelectOpts { label: string; options: string[]; get: (doc: Project) => number; commit: (v: number) => void }

function selectCtl(parent: HTMLElement, o: SelectOpts): Updater {
  const id = nextId('sel');
  const sel = h('select', { id });
  o.options.forEach((t, i) => sel.append(new Option(t, String(i))));
  sel.addEventListener('change', () => o.commit(Number(sel.value)));
  parent.append(h('div', { class: 'ctl' }, h('label', { htmlFor: id }, o.label), sel));
  return (doc) => {
    if (focused(sel)) return;
    const v = o.get(doc);
    if (![...sel.options].some((x) => x.value === String(v))) sel.append(new Option('Custom', String(v)));
    sel.value = String(v);
  };
}

// ---------------------------------------------------------------- project bar

const MB = 1024 * 1024;
const fmtBytes = (n: number) => (n >= 1024 * MB ? `${(n / (1024 * MB)).toFixed(1)} GB` : `${(n / MB).toFixed(1)} MB`);

export function initProjectBar(app: App, onProjectChange: () => void): { refresh: Updater } {
  const name = $<HTMLInputElement>('projName');
  const list = $<HTMLSelectElement>('projList');

  async function fillList(): Promise<void> {
    const rows = await app.session.db.list();
    list.replaceChildren(...rows.map((r) => new Option(r.name || '(unnamed)', r.id)));
    list.value = app.doc.id;
  }

  name.addEventListener('change', () => {
    const v = name.value.trim();
    if (!v) { name.value = app.doc.name; return; }
    app.run(() => app.store.dispatch('renameProject', { name: v }));
  });
  list.addEventListener('pointerdown', () => { void fillList(); });
  list.addEventListener('focus', () => { void fillList(); });

  $('projNew').addEventListener('click', async () => {
    await app.session.create('Untitled');
    onProjectChange();
  });
  $('projOpen').addEventListener('click', async () => {
    const id = list.value;
    if (!id || id === app.doc.id) return;
    if (!(await app.session.open(id))) app.fail('That project could not be opened.');
    onProjectChange();
  });
  $('projDelete').addEventListener('click', async () => {
    const doc = app.doc;
    if (!confirm(`Delete "${doc.name}" and its takes? This cannot be undone.`)) return;
    await app.session.remove(doc.id);
    await app.session.init();
    onProjectChange();
  });

  // Save status and storage use.
  setInterval(() => {
    const n = app.session.db.pending;
    $('saveStatus').textContent = n === 0 ? 'Saved' : `Saving ${n}`;
  }, 150);
  const usage = async () => {
    const e = await storageEstimate();
    const pct = e.quota > 0 ? (100 * e.usage) / e.quota : 0;
    $('storage').textContent = e.quota > 0
      ? `Storage: ${fmtBytes(e.usage)} of ${fmtBytes(e.quota)} (${pct.toFixed(1)}%)${e.persisted ? ', persistent' : ''}`
      : 'Storage: not reported by this browser';
    $('storageWarn').hidden = !(e.quota > 0 && e.usage / e.quota >= 0.8);
  };
  void usage();
  setInterval(() => { void usage(); }, 10000);
  app.session.onChange((_d, kind) => { if (kind === 'swap') void usage(); });

  return {
    refresh(doc) {
      if (!focused(name)) name.value = doc.name;
      if (!focused(list)) void fillList();
    },
  };
}

// ---------------------------------------------------------------- sections

export interface SectionPanel { refresh: Updater; markPlaying(index: number): void }

export function initSections(app: App): SectionPanel {
  const strip = $('strip');
  const fields = $('secFields');
  let sig = '';
  let fieldSig = '';
  let updaters: Updater[] = [];
  let buttons: HTMLButtonElement[] = [];

  const sec = () => app.doc.sections.find((s) => s.id === app.ui.sectionId);
  const index = () => app.doc.sections.findIndex((s) => s.id === app.ui.sectionId);

  function pick(i: number): void {
    const s = app.doc.sections[i];
    if (!s || s.id === app.ui.sectionId) return;
    app.ui.sectionId = s.id;
    app.ui.clearSelection();
    // While playing, selecting a section queues a jump to it (the engine lands on a section boundary).
    const m = app.readMeters();
    if (app.host && m && m[M_PLAYING] === 1) {
      app.host.jump(i);
      app.ui.pendingJump = i;
    }
    app.ui.changed();
    app.refreshAll();
  }

  function build(doc: Project): void {
    buttons = doc.sections.map((s, i) => {
      const b = h('button', { type: 'button' });
      b.addEventListener('click', () => pick(i));
      return b;
    });
    strip.replaceChildren(...buttons);
  }

  function buildFields(doc: Project): void {
    fields.replaceChildren();
    updaters = [];
    const feel = sec()?.feel ?? 'grid';
    const num = (label: string, key: 'beats' | 'seconds' | 'bpm' | 'repeats' | 'rampMs', step: string, min: string) => {
      const input = h('input', { type: 'number', min, step });
      input.setAttribute('aria-label', `Section ${label}`);
      input.addEventListener('change', () => {
        const v = Number(input.value);
        const s = sec();
        if (!s || !Number.isFinite(v)) { app.refreshAll(); return; }
        app.run(() => app.store.dispatch('setSection', { id: s.id, [key]: v }));
      });
      fields.append(h('label', {}, label, input));
      updaters.push((d) => { const s = d.sections.find((x) => x.id === app.ui.sectionId); if (s && !focused(input)) input.value = String(s[key]); });
    };
    const name = h('input', { type: 'text', size: 10 });
    name.setAttribute('aria-label', 'Section name');
    name.addEventListener('change', () => { const s = sec(); if (s) app.run(() => app.store.dispatch('setSection', { id: s.id, name: name.value })); });
    fields.append(h('label', {}, 'Name', name));
    updaters.push((d) => { const s = d.sections.find((x) => x.id === app.ui.sectionId); if (s && !focused(name)) name.value = s.name; });

    const fs = h('select', {});
    fs.append(new Option('Grid (beats)', 'grid'), new Option('Free (seconds)', 'free'));
    fs.setAttribute('aria-label', 'Section feel');
    fs.addEventListener('change', () => { const s = sec(); if (s) app.run(() => app.store.dispatch('setSection', { id: s.id, feel: fs.value as 'grid' | 'free' })); });
    fields.append(h('label', {}, 'Feel', fs));
    updaters.push((d) => { const s = d.sections.find((x) => x.id === app.ui.sectionId); if (s && !focused(fs)) fs.value = s.feel; });

    if (feel === 'grid') { num('Beats', 'beats', '1', '1'); num('Bpm', 'bpm', '1', '20'); } else num('Seconds', 'seconds', 'any', '0.1');
    num('Repeats', 'repeats', '1', '1');
    num('Ramp ms', 'rampMs', '10', '0');
    void doc;
  }

  function label(s: Project['sections'][number]): string {
    const len = s.feel === 'grid' ? `${s.beats} beats, ${s.bpm} bpm` : `${s.seconds} s`;
    return `${s.name}\n${len}${s.repeats > 1 ? `, x${s.repeats}` : ''}`;
  }

  $('secAdd').addEventListener('click', () => {
    const cur = sec();
    const r = app.run(() => app.store.dispatch('addSection', { after: cur?.id, feel: cur?.feel ?? 'grid', bpm: cur?.bpm }));
    if (r) { const i = Math.max(0, index()) + 1; app.ui.sectionId = app.doc.sections[i]?.id ?? app.ui.sectionId; app.refreshAll(); }
  });
  $('secDup').addEventListener('click', () => {
    const s = sec();
    if (s && app.run(() => app.store.dispatch('duplicateSection', { id: s.id }))) { app.ui.sectionId = app.doc.sections[index() + 1].id; app.refreshAll(); }
  });
  $('secRemove').addEventListener('click', () => { const s = sec(); if (s) app.run(() => app.store.dispatch('removeSection', { id: s.id })); });
  $('secLeft').addEventListener('click', () => { const s = sec(); const i = index(); if (s && i > 0) app.run(() => app.store.dispatch('moveSection', { id: s.id, to: i - 1 })); });
  $('secRight').addEventListener('click', () => { const s = sec(); const i = index(); if (s && i < app.doc.sections.length - 1) app.run(() => app.store.dispatch('moveSection', { id: s.id, to: i + 1 })); });

  return {
    refresh(doc) {
      const newSig = doc.sections.map((s) => s.id).join(',');
      if (newSig !== sig) { sig = newSig; build(doc); }
      doc.sections.forEach((s, i) => {
        const b = buttons[i];
        const t = label(s);
        if (b.dataset.t !== t) {
          b.dataset.t = t;
          b.replaceChildren(h('strong', {}, s.name), h('br'), h('span', { class: 'small' }, t.split('\n')[1]));
        }
        b.setAttribute('aria-pressed', String(s.id === app.ui.sectionId));
        b.setAttribute('aria-label', `Section ${s.name}, ${t.split('\n')[1]}${s.id === app.ui.sectionId ? ', selected' : ''}`);
      });
      const f = `${app.ui.sectionId}|${sec()?.feel}`;
      if (f !== fieldSig) { fieldSig = f; buildFields(doc); }
      updaters.forEach((u) => u(doc));
      const i = index();
      ($('secLeft') as HTMLButtonElement).disabled = i <= 0;
      ($('secRight') as HTMLButtonElement).disabled = i < 0 || i >= doc.sections.length - 1;
      ($('secRemove') as HTMLButtonElement).disabled = doc.sections.length <= 1;
    },
    markPlaying(playing) {
      buttons.forEach((b, i) => {
        b.classList.toggle('playing', i === playing);
        b.classList.toggle('queued', i === app.ui.pendingJump && i !== playing);
      });
    },
  };
}

// ---------------------------------------------------------------- layers

const NOTE_NAMES = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];

const soundNames = (kind: LayerKind): string[] => (kind === 'beat' ? BEAT_KITS.map((k) => k.name) : MELODY_SOUNDS.map((m) => `${m.name} (${m.blurb})`));

/** The normalized sets a preset puts in a layer (setLayerSound args). */
function presetArgs(kind: LayerKind, i: number): { base: number[]; kit?: { params: number[]; cents: number }[] } {
  if (kind === 'beat') {
    const k = BEAT_KITS[i];
    return {
      base: Array.from(soundToSet(k.base)),
      kit: Array.from({ length: 4 }, (_, s) => ({ params: Array.from(soundToSet(k.kit[s])), cents: k.kitCents[s] })),
    };
  }
  return { base: Array.from(soundToSet(MELODY_SOUNDS[i].sound)) };
}

let presetCache: { melody: number[][]; beat: number[][] } | null = null;
/** Index of the preset whose base matches the state being edited, or -1 (Custom). */
function matchPreset(l: Layer, state: State = l.states[0]): number {
  presetCache ??= {
    melody: MELODY_SOUNDS.map((_, i) => presetArgs('melody', i).base),
    beat: BEAT_KITS.map((_, i) => presetArgs('beat', i).base),
  };
  const sets = l.kind === 'beat' ? presetCache.beat : presetCache.melody;
  const base = state?.base;
  return sets.findIndex((s) => base && s.length === base.length && s.every((v, k) => v === base[k]));
}

export interface LayerPanel { refresh: Updater; markActive(): void }

export function initLayers(app: App): LayerPanel {
  const box = $('layers');
  let sig = '';
  const cards = new Map<string, { root: HTMLElement; updaters: Updater[] }>();

  const idx = (id: string) => app.doc.layers.findIndex((l) => l.id === id);
  const layer = (id: string) => app.doc.layers.find((l) => l.id === id);

  /** Put a one-field change straight into the engine's layer (not the document). */
  const direct = (id: string, field: number, v: number) => {
    const i = idx(id);
    if (app.host && i >= 0) app.host.send(MSG_SET_LAYER, i, 0, field, v);
  };
  const commit = (id: string, args: { name?: string; level?: number; pan?: number; mute?: boolean; follow?: number; depth?: { loud?: number; mouth?: number } }) => {
    app.run(() => app.store.dispatch('setLayer', { id, ...args }));
  };
  const tunePreview = (id: string, patch: Partial<Tune>) => {
    const doc = app.doc;
    try {
      const preview = applyOps(doc, commands.setLayer(doc, { id, tune: patch })).doc;
      void app.sync.request(preview);
    } catch { /* the commit on release reports bad values */ }
  };

  function buildCard(l: Layer): { root: HTMLElement; updaters: Updater[] } {
    const id = l.id;
    const ups: Updater[] = [];
    const root = h('fieldset', { class: 'panel layer' });
    const legend = h('legend', { class: 'small' });
    root.append(legend);
    const title = h('h3', {});
    const swatch = h('span', { class: 'swatch' });
    swatch.style.background = `#${l.color.toString(16).padStart(6, '0')}`;
    const nameIn = h('input', { type: 'text', size: 12 });
    nameIn.setAttribute('aria-label', `Layer ${l.name} name`);
    nameIn.addEventListener('change', () => commit(id, { name: nameIn.value }));
    ups.push((d) => { const x = d.layers.find((y) => y.id === id); if (x && !focused(nameIn)) nameIn.value = x.name; });
    const radioId = nextId('cap');
    const radio = h('input', { type: 'radio', name: 'capLayer', id: radioId, value: id });
    radio.addEventListener('change', () => { if (radio.checked) { app.ui.layerId = id; app.ui.changed(); } });
    ups.push(() => { radio.checked = app.ui.layerId === id; });
    const kindTag = h('span', { class: 'small muted' }, l.kind);
    const del = h('button', { type: 'button' }, 'Remove');
    del.setAttribute('aria-label', `Remove layer ${l.name}`);
    del.addEventListener('click', () => app.run(() => app.store.dispatch('removeLayer', { id })));
    title.append(swatch, nameIn, kindTag, h('label', { htmlFor: radioId, class: 'small' }, radio, ' Capture'), del);
    root.append(title);

    const g = h('div', { class: 'grid' });
    root.append(g);

    // Sound preset.
    ups.push(selectCtl(g, {
      label: l.kind === 'beat' ? 'Kit' : 'Sound', options: soundNames(l.kind),
      get: (d) => { const x = d.layers.find((y) => y.id === id); return x ? matchPreset(x, app.ui.editState(x)) : -1; },
      commit: (v) => {
        const a = presetArgs(l.kind, v);
        const x = layerOf(app.doc, id);
        const st = app.ui.editState(x);
        // setLayerSound writes the first state (and the kit); any other state is written with setState.
        if (st.id === x.states[0].id) app.run(() => app.store.dispatch('setLayerSound', { id, base: a.base, ...(a.kit ? { kit: a.kit } : {}) }));
        else app.run(() => app.store.dispatch('setState', { layer: id, id: st.id, base: a.base }));
      },
    }));
    ups.push(rangeCtl(g, {
      label: 'Level (dB)', min: -60, max: 12, step: 0.5, digits: 1, unit: ' dB', get: (d) => layerOf(d, id).level,
      preview: (v) => direct(id, LAYER_LEVEL, v), commit: (v) => commit(id, { level: v }),
    }));
    ups.push(rangeCtl(g, {
      label: 'Pan', min: -1, max: 1, step: 0.01, get: (d) => layerOf(d, id).pan,
      preview: (v) => direct(id, LAYER_PAN, v), commit: (v) => commit(id, { pan: v }),
    }));
    const muteId = nextId('mute');
    const mute = h('input', { type: 'checkbox', id: muteId });
    mute.addEventListener('change', () => commit(id, { mute: mute.checked }));
    g.append(h('div', {}, h('label', { htmlFor: muteId }, mute, ' Mute')));
    ups.push((d) => { mute.checked = layerOf(d, id).mute; });
    if (l.kind === 'beat') {
      ups.push(rangeCtl(g, {
        label: 'Follow (how much of the hit shape is used)', min: 0, max: 1, step: 0.01, get: (d) => layerOf(d, id).follow,
        preview: (v) => direct(id, LAYER_FOLLOW, v), commit: (v) => commit(id, { follow: v }),
      }));
    }
    ups.push(rangeCtl(g, {
      label: 'Loud depth', min: 0, max: 1, step: 0.01, get: (d) => layerOf(d, id).depth.loud,
      preview: (v) => direct(id, LAYER_DEPTH_LOUD, v), commit: (v) => commit(id, { depth: { loud: v } }),
    }));
    ups.push(rangeCtl(g, {
      label: 'Mouth depth', min: 0, max: 1, step: 0.01, get: (d) => layerOf(d, id).depth.mouth,
      preview: (v) => direct(id, LAYER_DEPTH_MOUTH, v), commit: (v) => commit(id, { depth: { mouth: v } }),
    }));

    if (l.kind !== 'beat') {
      // Tune shapes the stored take's pitch curve; a preview re-tunes it in the worker and reloads the curve.
      const det = h('details', { class: 'tune' }, h('summary', {}, 'Tune (applies to the recorded pitch curve)'));
      const tg = h('div', { class: 'grid' });
      det.append(tg);
      root.append(det);
      const tune = <K extends keyof Tune>(k: K) => (d: Project) => layerOf(d, id).tune[k] as number;
      const tr = (label: string, k: keyof Tune, min: number, max: number, step: number, digits: number, unit = '') =>
        ups.push(rangeCtl(tg, { label, min, max, step, digits, unit, get: tune(k),
          preview: (v) => tunePreview(id, { [k]: v }),
          commit: (v) => app.run(() => app.store.dispatch('setLayer', { id, tune: { [k]: v } })) }));
      tr('Snap', 'snap', 0, 1, 0.01, 2);
      tr('Speed (lag of the snap)', 'speedMs', 0, 250, 1, 0, ' ms');
      ups.push(selectCtl(tg, { label: 'Slides', options: ['Glide', 'Gliss', 'Jump'], get: tune('slides'),
        commit: (v) => app.run(() => app.store.dispatch('setLayer', { id, tune: { slides: v as 0 | 1 | 2 } })) }));
      tr('Vibrato (0 flatten, 1 keep, 2 exaggerate)', 'vibrato', 0, 2, 0.01, 2);
      tr('Octave', 'octave', -2, 2, 1, 0);
      ups.push(selectCtl(tg, { label: 'Root', options: NOTE_NAMES, get: tune('root'),
        commit: (v) => app.run(() => app.store.dispatch('setLayer', { id, tune: { root: v } })) }));
      ups.push(selectCtl(tg, { label: 'Scale', options: SCALES, get: tune('scale'),
        commit: (v) => app.run(() => app.store.dispatch('setLayer', { id, tune: { scale: v } })) }));
    }
    return { root, updaters: ups };
  }

  function layerOf(d: Project, id: string): Layer {
    return d.layers.find((x) => x.id === id) ?? d.layers[0];
  }

  // Add layer.
  const kind = $<HTMLSelectElement>('addKind');
  const snd = $<HTMLSelectElement>('addSound');
  const fillSounds = () => snd.replaceChildren(...soundNames(kind.value as LayerKind).map((t, i) => new Option(t, String(i))));
  kind.addEventListener('change', fillSounds);
  fillSounds();
  $('addLayer').addEventListener('click', () => {
    const k = kind.value as LayerKind;
    app.run(() => app.store.dispatch('addLayer', { kind: k, sound: Number(snd.value) }));
  });

  void layer;
  return {
    refresh(doc) {
      const newSig = doc.layers.map((l) => `${l.id}:${l.kind}`).join(',');
      if (newSig !== sig) {
        sig = newSig;
        cards.clear();
        box.replaceChildren();
        for (const l of doc.layers) {
          const c = buildCard(l);
          cards.set(l.id, c);
          box.append(c.root);
        }
      }
      for (const c of cards.values()) c.updaters.forEach((u) => u(doc));
      $('layerCount').textContent = `${doc.layers.length} of ${MAX_LAYERS} layers`;
      ($('addLayer') as HTMLButtonElement).disabled = doc.layers.length >= MAX_LAYERS;
      this.markActive();
    },
    markActive() {
      for (const [id, c] of cards) c.root.classList.toggle('active', id === app.ui.layerId);
    },
  };
}
