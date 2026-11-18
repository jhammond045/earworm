// Seeded edit generator for the "edit while playing" gate. Pure: it reads the current
// document and a random source and returns one action that is valid for that document
// (existing ids, in-range values). The page applies it through the store, so the mix of
// commands is what a person editing a song would produce, not a synthetic burst.

import { newProject, type CommandArgs, type CommandName } from '../project/commands';
import { Store } from '../project/store';
import { PPQ, type Clip, type Layer, type Project, type Section } from '../project/types';

export type StressAction =
  | { type: 'cmd'; name: CommandName; args: unknown; label: string }
  | { type: 'undo' }
  | { type: 'redo' };

export type Rand = () => number;

/** Small seeded generator (mulberry32). */
export function seeded(seed: number): Rand {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const pick = <T>(r: Rand, a: readonly T[]): T => a[Math.floor(r() * a.length)];
const between = (r: Rand, lo: number, hi: number) => lo + r() * (hi - lo);
const intBetween = (r: Rand, lo: number, hi: number) => Math.floor(between(r, lo, hi + 1));

const HIT_SHAPE = { attack: 0.004, burst: 0.5, cents: null, bend: 0, hold: 0.06, vowel: 0.5, tone: 0.5, endStop: 0, endClick: 0 };

/** The scratch project for the gate: 3 grid sections, 6 layers (3 melody, 1 texture, 2 beat), each section with notes
 *  and hits in every layer. */
export function generateProject(seed: number, name = 'M3 gate test'): Project {
  const r = seeded(seed);
  const s = new Store(newProject(name));
  s.dispatch('addSection', { feel: 'grid', beats: 8, bpm: 100 });
  s.dispatch('addSection', { feel: 'grid', beats: 8, bpm: 110 });
  s.dispatch('addLayer', { kind: 'melody' });
  s.dispatch('addLayer', { kind: 'texture' });
  s.dispatch('addLayer', { kind: 'beat', sound: 1 });
  s.dispatch('addLayer', { kind: 'melody', sound: 1 });
  const doc = s.doc;
  for (const sec of doc.sections) {
    const len = sec.beats * PPQ;
    for (const lay of doc.layers) {
      if (lay.kind === 'beat') {
        for (let i = 0; i < 8; i++) {
          s.dispatch('addHit', { section: sec.id, layer: lay.id, hit: {
            t: (i * len) / 8, dur: PPQ / 4, slot: intBetween(r, 0, 3), strength: between(r, 0.4, 1), shape: { ...HIT_SHAPE },
          } });
        }
      } else {
        for (let i = 0; i < 6; i++) {
          s.dispatch('addNote', { section: sec.id, layer: lay.id, note: {
            t: Math.round((i * len) / 6), dur: PPQ / 2, cents: 4800 + 100 * intBetween(r, 0, 24), hard: r(),
          } });
        }
      }
    }
  }
  return s.doc;
}

interface ClipRef { section: Section; layer: Layer; clip: Clip }

function clips(doc: Project, pred: (c: Clip) => boolean): ClipRef[] {
  const out: ClipRef[] = [];
  for (const section of doc.sections) {
    for (const layer of doc.layers) {
      const clip = section.clips[layer.id];
      if (clip && pred(clip)) out.push({ section, layer, clip });
    }
  }
  return out;
}

const hasNotes = (min: number) => (c: Clip) => c.kind !== 'beat' && c.notes.length >= min;
const hasHits = (min: number) => (c: Clip) => c.kind === 'beat' && c.hits.length >= min;

function some<T extends { id: string }>(r: Rand, items: readonly T[], max = 2): string[] {
  const n = Math.min(items.length, intBetween(r, 1, max));
  const ids = items.map((x) => x.id);
  const out: string[] = [];
  while (out.length < n) {
    const id = pick(r, ids);
    if (!out.includes(id)) out.push(id);
  }
  return out;
}

type Gen = (doc: Project, r: Rand) => StressAction | null;

function cmd<K extends CommandName>(name: K, args: CommandArgs<K>, label: string): StressAction {
  return { type: 'cmd', name, args, label };
}

const GENERATORS: { weight: number; group: string; gen: Gen }[] = [
  { weight: 10, group: 'moveNotes', gen: (doc, r) => {
    const c = clips(doc, hasNotes(1)); if (!c.length) return null;
    const x = pick(r, c);
    const notes = (x.clip as { notes: { id: string }[] }).notes;
    return cmd('moveNotes', { section: x.section.id, layer: x.layer.id, ids: some(r, notes), dt: Math.round(between(r, -1200, 1200)),
      dcents: 100 * intBetween(r, -7, 7) }, 'move notes');
  } },
  { weight: 6, group: 'addNote', gen: (doc, r) => {
    const c = clips(doc, hasNotes(0)); if (!c.length) return null;
    const x = pick(r, c);
    return cmd('addNote', { section: x.section.id, layer: x.layer.id, note: {
      t: Math.round(between(r, 0, x.clip.loopLen)), dur: Math.round(between(r, 120, 960)), cents: 4800 + 100 * intBetween(r, 0, 24), hard: r(),
    } }, 'add note');
  } },
  { weight: 4, group: 'deleteNotes', gen: (doc, r) => {
    const c = clips(doc, hasNotes(3)); if (!c.length) return null;
    const x = pick(r, c);
    const notes = (x.clip as { notes: { id: string }[] }).notes;
    return cmd('deleteNotes', { section: x.section.id, layer: x.layer.id, ids: [pick(r, notes).id] }, 'delete note');
  } },
  { weight: 4, group: 'resizeNote', gen: (doc, r) => {
    const c = clips(doc, hasNotes(1)); if (!c.length) return null;
    const x = pick(r, c);
    const notes = (x.clip as { notes: { id: string }[] }).notes;
    return cmd('resizeNote', { section: x.section.id, layer: x.layer.id, id: pick(r, notes).id, dur: Math.round(between(r, 60, 1920)) }, 'resize note');
  } },
  { weight: 8, group: 'moveHits', gen: (doc, r) => {
    const c = clips(doc, hasHits(1)); if (!c.length) return null;
    const x = pick(r, c);
    const hits = (x.clip as { hits: { id: string }[] }).hits;
    return cmd('moveHits', { section: x.section.id, layer: x.layer.id, ids: some(r, hits), dt: Math.round(between(r, -960, 960)) }, 'move hits');
  } },
  { weight: 5, group: 'addHit', gen: (doc, r) => {
    const c = clips(doc, hasHits(0)); if (!c.length) return null;
    const x = pick(r, c);
    return cmd('addHit', { section: x.section.id, layer: x.layer.id, hit: {
      t: Math.round(between(r, 0, x.clip.loopLen)), dur: PPQ / 4, slot: intBetween(r, 0, 3), strength: between(r, 0.3, 1), shape: { ...HIT_SHAPE },
    } }, 'add hit');
  } },
  { weight: 3, group: 'deleteHits', gen: (doc, r) => {
    const c = clips(doc, hasHits(3)); if (!c.length) return null;
    const x = pick(r, c);
    const hits = (x.clip as { hits: { id: string }[] }).hits;
    return cmd('deleteHits', { section: x.section.id, layer: x.layer.id, ids: [pick(r, hits).id] }, 'delete hit');
  } },
  { weight: 5, group: 'setHitSlot', gen: (doc, r) => {
    const c = clips(doc, hasHits(1)); if (!c.length) return null;
    const x = pick(r, c);
    const hits = (x.clip as { hits: { id: string }[] }).hits;
    return cmd('setHitSlot', { section: x.section.id, layer: x.layer.id, ids: some(r, hits), slot: intBetween(r, 0, 3) }, 'change hit slot');
  } },
  { weight: 4, group: 'level', gen: (doc, r) => cmd('setLayer', { id: pick(r, doc.layers).id, level: Math.round(between(r, -24, 0) * 2) / 2 }, 'layer level') },
  { weight: 3, group: 'pan', gen: (doc, r) => cmd('setLayer', { id: pick(r, doc.layers).id, pan: Math.round(between(r, -1, 1) * 10) / 10 }, 'layer pan') },
  { weight: 3, group: 'mute', gen: (doc, r) => cmd('setLayer', { id: pick(r, doc.layers).id, mute: r() < 0.5 }, 'layer mute') },
  { weight: 3, group: 'bpm', gen: (doc, r) => cmd('setSection', { id: pick(r, doc.sections).id, bpm: intBetween(r, 80, 140) }, 'section bpm') },
  { weight: 2, group: 'repeats', gen: (doc, r) => cmd('setSection', { id: pick(r, doc.sections).id, repeats: intBetween(r, 1, 3) }, 'section repeats') },
  { weight: 2, group: 'beats', gen: (doc, r) => cmd('setSection', { id: pick(r, doc.sections).id, beats: intBetween(r, 4, 16) }, 'section beats') },
  { weight: 2, group: 'addSection', gen: (doc, r) => doc.sections.length >= 6 ? null
      : cmd('addSection', { after: pick(r, doc.sections).id, feel: 'grid', beats: intBetween(r, 4, 8) }, 'add section') },
  { weight: 2, group: 'removeSection', gen: (doc, r) => doc.sections.length <= 2 ? null
      : cmd('removeSection', { id: pick(r, doc.sections).id }, 'remove section') },
  { weight: 3, group: 'quantize', gen: (doc, r) => {
    const c = clips(doc, (k) => (k.kind === 'beat' ? k.hits.length > 0 : k.notes.length > 0)); if (!c.length) return null;
    const x = pick(r, c);
    return cmd('quantize', { section: x.section.id, layer: x.layer.id, grid: pick(r, [PPQ / 4, PPQ / 2]) }, 'quantize');
  } },
];

/** Group names of the generator mix, for the report and the test. */
export const STRESS_GROUPS: string[] = [...GENERATORS.map((g) => g.group), 'undo', 'redo'];

const UNDO_WEIGHT = 5, REDO_WEIGHT = 3;

/** The next edit for `doc`. `canUndo` and `canRedo` say whether the history has something to undo or redo. */
export function nextAction(doc: Project, r: Rand, canUndo: boolean, canRedo: boolean): StressAction {
  const total = GENERATORS.reduce((s, g) => s + g.weight, 0) + UNDO_WEIGHT + REDO_WEIGHT;
  for (let tries = 0; tries < 20; tries++) {
    let x = r() * total;
    if ((x -= UNDO_WEIGHT) < 0) { if (canUndo) return { type: 'undo' }; continue; }
    if ((x -= REDO_WEIGHT) < 0) { if (canRedo) return { type: 'redo' }; continue; }
    for (const g of GENERATORS) {
      if ((x -= g.weight) < 0) {
        const a = g.gen(doc, r);
        if (a) return a;
        break;
      }
    }
  }
  // Always possible: a layer level change.
  return cmd('setLayer', { id: doc.layers[0].id, level: Math.round(between(r, -24, 0) * 2) / 2 }, 'layer level');
}

/** Apply an action through the store. Returns false when it changed nothing. Throws if the command is invalid. */
export function applyAction(store: Store, a: StressAction): boolean {
  if (a.type === 'undo') return store.undo();
  if (a.type === 'redo') return store.redo();
  return store.dispatch(a.name, a.args as never);
}

export function actionLabel(a: StressAction): string {
  return a.type === 'cmd' ? a.label : a.type;
}
