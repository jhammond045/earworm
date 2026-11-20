// Small built-in projects. Each one is an ordinary document, so opening it is the same as Open.

import { soundToSet } from '../m1/layer';
import { BEAT_KITS, MELODY_SOUNDS } from '../m2/sounds';
import { newProject } from '../project/commands';
import { Store } from '../project/store';
import { PPQ, type HitShape, type Project } from '../project/types';

export interface BuiltDemo {
  id: string;
  name: string;
  blurb: string;
  doc: Project;
  blobs: Record<string, Float32Array>;
}

const SHAPE: HitShape = { attack: 0.004, burst: 0.4, cents: null, bend: 0, hold: 0.05, vowel: 0.4, tone: 0.6, endStop: 0, endClick: 0 };

function base(name: string, bpm: number, beats: number): { store: Store; sec: string } {
  const store = new Store(newProject(name));
  const sec = store.doc.sections[0].id;
  store.dispatch('setSection', { id: sec, name: 'A', bpm, beats });
  return { store, sec };
}

function setSound(store: Store, layerId: string, index: number, kind: 'melody' | 'beat'): void {
  const sound = kind === 'beat' ? BEAT_KITS[index].base : MELODY_SOUNDS[index].sound;
  const kit = kind === 'beat' ? BEAT_KITS[index].kit.map((s, i) => ({ params: Array.from(soundToSet(s)), cents: BEAT_KITS[index].kitCents[i] })) : undefined;
  store.dispatch('setLayerSound', { id: layerId, base: Array.from(soundToSet(sound)), kit });
}

/** A four-bar hum: C D E G, then the same a step up. */
export function melodyDemo(): BuiltDemo {
  const { store, sec } = base('Demo: Melody', 96, 8);
  const mel = store.doc.layers[0].id;
  setSound(store, mel, 0, 'melody');
  const tune = [6000, 6200, 6400, 6700, 6200, 6400, 6700, 6900];
  tune.forEach((cents, i) => {
    store.dispatch('addNote', { section: sec, layer: mel, note: { id: `n${i}`, t: i * PPQ, dur: PPQ * 0.9, cents, hard: 0.35 } });
  });
  store.dispatch('setMetro', { on: true, level: 0.55, bar: 4 });
  return { id: 'melody', name: 'Melody', blurb: 'Eight notes on a choir pad, click on.', doc: store.doc, blobs: {} };
}

/** Kick, snare and hats for two bars. */
export function beatDemo(): BuiltDemo {
  const { store, sec } = base('Demo: Beat', 108, 4);
  const beat = store.doc.layers[1].id;
  setSound(store, beat, 0, 'beat');
  const hits: { t: number; slot: number }[] = [];
  for (let step = 0; step < 16; step++) {
    const t = step * (PPQ / 4);
    if (step % 8 === 0) hits.push({ t, slot: 0 });
    else if (step % 8 === 4) hits.push({ t, slot: 1 });
    if (step % 2 === 0) hits.push({ t, slot: 2 });
  }
  hits.forEach((h, i) => {
    store.dispatch('addHit', { section: sec, layer: beat, hit: { id: `h${i}`, t: h.t, dur: 200, slot: h.slot, strength: h.slot === 2 ? 0.45 : 0.85, shape: SHAPE } });
  });
  store.dispatch('setMetro', { on: true, voice: 'kit', layerId: beat, slot: 2, level: 0.35, bar: 4 });
  return { id: 'beat', name: 'Beat', blurb: 'Kick, snare and hats. The click uses the hat slot.', doc: store.doc, blobs: {} };
}

/** A texture, two states, a morph, and an Influence target made from a short tone. */
export function statesDemo(): BuiltDemo {
  const { store, sec } = base('Demo: States', 80, 8);
  store.dispatch('addLayer', { kind: 'texture', name: 'Pad', sound: 4 });
  const pad = store.doc.layers[store.doc.layers.length - 1].id;
  store.dispatch('addNote', { section: sec, layer: pad, note: { id: 'pad', t: 0, dur: 8 * PPQ, cents: 4800, hard: 0.2 } });
  store.dispatch('addState', { layer: pad, name: 'Bright', id: 'bright' });
  const bright = Array.from(soundToSet(MELODY_SOUNDS[1].sound));
  store.dispatch('setState', { layer: pad, id: 'bright', base: bright, macros: [0.7, 0.6, 0.5, 0.5, 0.5, 0.5, 0.4, 0.5] });
  store.dispatch('addMorphMarker', { section: sec, layer: pad, marker: { t: 0, state: store.doc.layers.find((l) => l.id === pad)!.states[0].id, lenMs: 0, shape: 'step' } });
  store.dispatch('addMorphMarker', { section: sec, layer: pad, marker: { t: 4 * PPQ, state: 'bright', lenMs: 600, shape: 'ease' } });
  const n = 4800;
  const tone = new Float32Array(n);
  for (let i = 0; i < n; i++) tone[i] = Math.sin((2 * Math.PI * 660 * i) / 48000) * 0.35;
  const target = Array.from(soundToSet(MELODY_SOUNDS[2].sound));
  store.dispatch('setInfluence', {
    layer: pad,
    influence: { target, mask: 0x1f, source: 'grain-demo-glass', sourceName: 'Glass tone', sourceRate: 48000, grain: { start: 0, end: n, refCents: 6400 }, useSound: false },
  });
  store.dispatch('setInfluenceAmount', { layer: pad, state: 'bright', amount: 0.65 });
  store.dispatch('setMetro', { on: false, bar: 4 });
  return {
    id: 'states', name: 'States', blurb: 'A pad that morphs, with an Influence target on the second state.',
    doc: store.doc, blobs: { 'grain-demo-glass': tone },
  };
}

export const DEMOS: BuiltDemo[] = [melodyDemo(), beatDemo(), statesDemo()];

export function demoById(id: string): BuiltDemo | undefined {
  return DEMOS.find((d) => d.id === id);
}
