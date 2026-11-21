// Every change to the project goes through one of these. A command is pure: it reads the
// document, checks its arguments (RangeError with a plain message), and returns patch ops.
// It returns [] when nothing would change. Store (store.ts) applies the ops and records them.

import { DESCS } from '../gen/params';
import type { Group } from '../gen/params';
import { soundToSet } from '../m1/layer';
import type { Sound } from '../m1/layer';
import { BEAT_KITS, MELODY_SOUNDS } from '../m2/sounds';
import type { Op, Path } from './patch';
import { classicPatch, valuesFromVocab, vocabFromValues, withVocab } from '../synth/classic';
import { KIT_SLOTS, MAX_LAYERS, MAX_STATES, MORPH_SHAPES, PPQ, SCHEMA, defaultMetro } from './types';
import type { BeatClip, Clip, Feel, GestureDepth, Hit, Influence, KitSlot, Layer, LayerKind, MelodyClip, Metro, MorphMarker,
  MorphShape, Note, Project, Section, State, Take, Tune } from './types';

const PARAMS = 96;
const MAX_CENTS = 12000;
const MIN_FREE_DUR = 0.01;
const MIN_BPM = 20;
const MAX_BPM = 400;
/** Mask bit of each aspect group (the engine's GROUP_* order). */
const GROUP_BIT: Record<Group, number> = { tone: 1, shape: 2, motion: 4, grit: 8, space: 16 };
const ALL_GROUPS = 0x1f;
const N_MACROS = 8;
const LAYER_COLORS = [0xe4572e, 0x29a3a3, 0xf3a712, 0x6a4c93, 0x4c9f38, 0x2f6fd0, 0xc9486b, 0x8a8f98];

let counter = 0;
/** A fresh id like "sec_3f9a1c0b2d44". Uses crypto.randomUUID when present. */
export function newId(prefix: string): string {
  const g = globalThis as { crypto?: { randomUUID?: () => string } };
  const hex = g.crypto?.randomUUID
    ? g.crypto.randomUUID().replace(/-/g, '').slice(0, 12)
    : `${Date.now().toString(16)}${(counter++).toString(16)}${Math.floor(Math.random() * 0xffffff).toString(16)}`.slice(-12);
  return `${prefix}_${hex}`;
}

const fail = (msg: string): never => { throw new RangeError(msg); };

function fin(v: unknown, what: string): number {
  if (typeof v !== 'number' || !Number.isFinite(v)) fail(`${what} must be a finite number`);
  return v as number;
}
function inRange(v: unknown, what: string, lo: number, hi: number): number {
  const n = fin(v, what);
  if (n < lo || n > hi) fail(`${what} must be between ${lo} and ${hi}`);
  return n;
}
function whole(v: unknown, what: string, lo: number, hi: number): number {
  const n = inRange(v, what, lo, hi);
  if (!Number.isInteger(n)) fail(`${what} must be a whole number`);
  return n;
}
const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
const str = (v: unknown, what: string): string => (typeof v === 'string' ? v : fail(`${what} must be a string`));

// ---------- layer and project factories ----------

const defaultTune = (): Tune => ({ snap: 0, speedMs: 60, slides: 0, vibrato: 1, levelOut: 0, root: 0, scale: 0, refCents: 0, octave: 0 });
const defaultDepth = (): GestureDepth => ({ loud: 1, open: 0.5, hard: 0.5, breath: 0.5, mouth: 1 });
const set96 = (s: Sound | undefined): number[] => Array.from(soundToSet(s));

function makeLayer(kind: LayerKind, name: string, soundIndex: number, colorIndex: number): Layer {
  let values: Record<string, number>;
  let kit: KitSlot[] = [];
  if (kind === 'beat') {
    const k = BEAT_KITS[soundIndex] ?? fail(`unknown kit preset ${soundIndex}`);
    values = valuesFromVocab(kind, set96(k.base), k.kit.map(set96));
    kit = Array.from({ length: KIT_SLOTS }, (_, i) => ({ cents: k.kitCents[i] }));
  } else {
    const m = MELODY_SOUNDS[soundIndex] ?? fail(`unknown sound preset ${soundIndex}`);
    values = valuesFromVocab(kind, set96(m.sound));
  }
  return {
    id: newId('lay'), name, kind, color: LAYER_COLORS[colorIndex % LAYER_COLORS.length],
    level: 0, pan: 0, mute: false, patch: classicPatch(kind),
    states: [{ id: newId('st'), name: 'Base', values, macros: new Array(N_MACROS).fill(0.5), influenceAmount: 0 }],
    tune: defaultTune(), depth: defaultDepth(), follow: 1, kit,
  };
}

function makeSection(name: string, feel: Feel, beats: number, seconds: number, bpm: number): Section {
  return { id: newId('sec'), name, feel, beats, seconds, bpm, repeats: 1, rampMs: 0, clips: {} };
}

/** A new project: one grid section "A" of 8 beats at 100 bpm, a melody layer and a beat layer. */
export function newProject(name: string): Project {
  const melody = makeLayer('melody', 'Melody', 0, 0);
  const beat = makeLayer('beat', 'Beat', 0, 1);
  return {
    schema: SCHEMA, id: newId('proj'), name,
    sections: [makeSection('A', 'grid', 8, 8, 100)],
    layers: [melody, beat],
    takes: {}, loop: true,
    metro: defaultMetro(beat.id),
  };
}

// ---------- lookups ----------

function secIdx(doc: Project, id: string): number {
  const i = doc.sections.findIndex((s) => s.id === id);
  return i < 0 ? fail(`unknown section id "${id}"`) : i;
}
function layIdx(doc: Project, id: string): number {
  const i = doc.layers.findIndex((l) => l.id === id);
  return i < 0 ? fail(`unknown layer id "${id}"`) : i;
}
const sectionLength = (s: Section) => (s.feel === 'grid' ? s.beats * PPQ : s.seconds);
const minDur = (s: Section) => (s.feel === 'grid' ? 1 : MIN_FREE_DUR);
const hasClip = (s: Section, layer: string) => Object.prototype.hasOwnProperty.call(s.clips, layer);

interface Where { si: number; li: number; section: Section; layer: Layer; clipPath: Path }
function where(doc: Project, sectionId: string, layerId: string): Where {
  const si = secIdx(doc, sectionId);
  const li = layIdx(doc, layerId);
  return { si, li, section: doc.sections[si], layer: doc.layers[li], clipPath: ['sections', si, 'clips', layerId] };
}
function melodyClip(w: Where): MelodyClip | undefined {
  if (w.layer.kind === 'beat') fail(`layer "${w.layer.name}" is a beat layer and has hits, not notes`);
  const c = hasClip(w.section, w.layer.id) ? w.section.clips[w.layer.id] : undefined;
  return c as MelodyClip | undefined;
}
function beatClip(w: Where): BeatClip | undefined {
  if (w.layer.kind !== 'beat') fail(`layer "${w.layer.name}" is not a beat layer and has notes, not hits`);
  const c = hasClip(w.section, w.layer.id) ? w.section.clips[w.layer.id] : undefined;
  return c as BeatClip | undefined;
}

// ---------- time helpers ----------

/** Put a time into [0, len). Grid times are whole ticks. */
function wrapT(t: number, len: number, grid: boolean): number {
  const x = grid ? Math.round(t) : t;
  const m = ((x % len) + len) % len;
  return m >= len ? 0 : m;
}

function normNote(n: Note, len: number, grid: boolean, min: number): Note {
  fin(n.t, 'note t'); fin(n.dur, 'note dur'); fin(n.cents, 'note cents'); fin(n.hard, 'note hard');
  const t = wrapT(n.t, len, grid);
  const dur = Math.max(min, grid ? Math.round(n.dur) : n.dur);
  const cents = clamp(n.cents, 0, MAX_CENTS);
  const hard = clamp(n.hard, 0, 1);
  return t === n.t && dur === n.dur && cents === n.cents && hard === n.hard ? n : { ...n, t, dur, cents, hard };
}

function normHit(h: Hit, len: number, grid: boolean, min: number): Hit {
  fin(h.t, 'hit t'); fin(h.dur, 'hit dur'); fin(h.strength, 'hit strength');
  if (!Number.isInteger(h.slot) || h.slot < 0 || h.slot >= KIT_SLOTS) fail(`hit slot must be a whole number from 0 to ${KIT_SLOTS - 1}`);
  const t = wrapT(h.t, len, grid);
  const dur = Math.max(min, grid ? Math.round(h.dur) : h.dur);
  const strength = clamp(h.strength, 0, 1);
  return t === h.t && dur === h.dur && strength === h.strength ? h : { ...h, t, dur, strength };
}

/** Check a morph marker and put its time inside the loop. `states` (ids) is checked when given. */
function normMarker(m: MorphMarker, len: number, grid: boolean, states: Set<string> | null): MorphMarker {
  fin(m.t, 'marker t'); fin(m.lenMs, 'marker lenMs');
  if (m.lenMs < 0) fail('marker lenMs must be 0 or more');
  if (!MORPH_SHAPES.includes(m.shape)) fail(`marker shape must be one of ${MORPH_SHAPES.join(', ')}`);
  if (typeof m.state !== 'string') fail('marker state must be a state id');
  if (states && !states.has(m.state)) fail(`unknown state id "${m.state}"`);
  const t = wrapT(m.t, len, grid);
  return t === m.t ? m : { ...m, t };
}

const sortByT = <T extends { t: number }>(a: T[]): T[] => a.slice().sort((x, y) => x.t - y.t);
const sameItems = <T>(a: T[], b: T[]) => a.length === b.length && a.every((x, i) => x === b[i]);

function insertSorted<T extends { t: number }>(a: T[], item: T): number {
  let i = a.length;
  while (i > 0 && a[i - 1].t > item.t) i--;
  return i;
}

/** Make a clip valid for its section: times inside [0, loopLen), durations at least the minimum, clamped values.
 *  Returns the same object when it already is. */
function normClip(clip: Clip, section: Section, states: Set<string> | null = null): Clip {
  const grid = section.feel === 'grid';
  const min = minDur(section);
  if (!(typeof clip.loopLen === 'number' && clip.loopLen > 0 && Number.isFinite(clip.loopLen))) fail('clip loopLen must be above 0');
  if (typeof clip.timeSnap !== 'number' || clip.timeSnap < 0) fail('clip timeSnap must be 0 or more');
  // A clip built by older code may lack the morph lane. Markers keep to the loop like notes do.
  let morph = clip.morph ?? [];
  const wrapped = morph.map((m) => normMarker(m, clip.loopLen, grid, states));
  if (!clip.morph || !sameItems(wrapped, morph)) morph = sortByT(wrapped);
  const withMorph = morph === clip.morph ? clip : { ...clip, morph };
  if (withMorph.kind === 'beat') {
    const hits = withMorph.hits.map((h) => normHit(h, withMorph.loopLen, grid, min));
    return sameItems(hits, withMorph.hits) ? withMorph : { ...withMorph, hits };
  }
  const notes = withMorph.notes.map((n) => normNote(n, withMorph.loopLen, grid, min));
  return sameItems(notes, withMorph.notes) ? withMorph : { ...withMorph, notes };
}

/** Convert every clip time in a section between ticks and seconds. */
function convertClip(clip: Clip, factor: number, toGrid: boolean): Clip {
  const conv = (v: number) => (toGrid ? Math.round(v * factor) : v * factor);
  const min = toGrid ? 1 : MIN_FREE_DUR;
  const loopLen = Math.max(min, conv(clip.loopLen));
  const morph = (clip.morph ?? []).map((m) => ({ ...m, t: wrapT(conv(m.t), loopLen, toGrid) }));
  const base = { loopLen, timeSnap: conv(clip.timeSnap), morph: sortByT(morph) };
  if (clip.kind === 'beat') {
    const hits = clip.hits.map((h) => ({ ...h, t: wrapT(conv(h.t), loopLen, toGrid), dur: Math.max(min, conv(h.dur)) }));
    return { ...clip, ...base, hits };
  }
  const notes = clip.notes.map((n) => ({ ...n, t: wrapT(conv(n.t), loopLen, toGrid), dur: Math.max(min, conv(n.dur)) }));
  const warp = clip.warp.map((a) => ({ ...a, clipT: conv(a.clipT) }));
  return { ...clip, ...base, notes, warp };
}

function idSet(ids: string[], have: { id: string }[], what: string): Set<string> {
  if (!Array.isArray(ids)) fail(`${what} ids must be a list`);
  const known = new Set(have.map((x) => x.id));
  for (const id of ids) if (!known.has(id)) fail(`unknown ${what} id "${id}"`);
  return new Set(ids);
}

// ---------- argument types ----------

export interface SectionFields { name: string; feel: Feel; beats: number; seconds: number; bpm: number; repeats: number; rampMs: number }
export interface LayerFields {
  name: string; level: number; pan: number; mute: boolean; color: number; follow: number;
  depth: Partial<GestureDepth>; tune: Partial<Tune>;
}
type WithId<T> = { id?: string } & Omit<T, 'id'>;

const sectionKeys: (keyof SectionFields)[] = ['name', 'feel', 'beats', 'seconds', 'bpm', 'repeats', 'rampMs'];

function checkSectionField(k: keyof SectionFields, v: unknown): void {
  switch (k) {
    case 'name': str(v, 'section name'); break;
    case 'feel': if (v !== 'grid' && v !== 'free') fail('feel must be "grid" or "free"'); break;
    case 'beats': if (fin(v, 'beats') <= 0) fail('beats must be above 0'); break;
    case 'seconds': if (fin(v, 'seconds') <= 0) fail('seconds must be above 0'); break;
    case 'bpm': inRange(v, 'bpm', MIN_BPM, MAX_BPM); break;
    case 'repeats': if (!Number.isInteger(v) || (v as number) < 1) fail('repeats must be a whole number, 1 or more'); break;
    case 'rampMs': if (fin(v, 'rampMs') < 0) fail('rampMs must be 0 or more'); break;
  }
}

function checkTune(k: keyof Tune, v: unknown): void {
  switch (k) {
    case 'snap': inRange(v, 'tune snap', 0, 1); break;
    case 'speedMs': if (fin(v, 'tune speedMs') < 0) fail('tune speedMs must be 0 or more'); break;
    case 'slides': if (v !== 0 && v !== 1 && v !== 2) fail('tune slides must be 0, 1 or 2'); break;
    case 'vibrato': if (fin(v, 'tune vibrato') < 0) fail('tune vibrato must be 0 or more'); break;
    case 'levelOut': inRange(v, 'tune levelOut', 0, 1); break;
    case 'root': if (!Number.isInteger(v) || (v as number) < 0 || (v as number) > 11) fail('tune root must be a whole number from 0 to 11'); break;
    case 'scale': if (!Number.isInteger(v) || (v as number) < 0) fail('tune scale must be a whole number, 0 or more'); break;
    case 'refCents': fin(v, 'tune refCents'); break;
    case 'octave': if (!Number.isInteger(v) || (v as number) < -4 || (v as number) > 4) fail('tune octave must be a whole number from -4 to 4'); break;
    default: fail(`unknown tune field "${String(k)}"`);
  }
}

function checkParams(v: unknown, what: string): number[] {
  if (!Array.isArray(v) || v.length !== PARAMS) fail(`${what} must be ${PARAMS} numbers`);
  for (const x of v as unknown[]) fin(x, what);
  return v as number[];
}


/** Values in 0..1, exactly `n` of them. */
function unitArray(v: unknown, n: number, what: string): number[] {
  if (!Array.isArray(v) || v.length !== n) fail(`${what} must be ${n} numbers`);
  for (const x of v as unknown[]) inRange(x, what, 0, 1);
  return v as number[];
}

/** A marker for a command: strict, the time must already be inside the loop (grid times are rounded to ticks). */
function normMarkerInside(m: MarkerFields, len: number, grid: boolean, layer: Layer): MarkerFields {
  fin(m.t, 'marker t');
  const t = grid ? Math.round(m.t) : m.t;
  if (t < 0 || t >= len) fail(`marker t must be inside the loop (0 up to ${len})`);
  return normMarker({ ...m, t }, len, grid, new Set(layer.states.map((x) => x.id)));
}

function stateIdx(l: Layer, id: string): number {
  const i = l.states.findIndex((s) => s.id === id);
  return i < 0 ? fail(`unknown state id "${id}"`) : i;
}

/** Blend a base toward an Influence target by `amount`, per aspect group the mask lets through
 *  (TDD resolution step 2: p = (1 - a m) b + a m t). */
export function blendInfluence(base: number[], target: number[], mask: number, amount: number): number[] {
  return base.map((b, i) => {
    const am = mask & GROUP_BIT[DESCS[i].group] ? amount : 0;
    return (1 - am) * b + am * target[i];
  });
}

function checkInfluence(v: unknown): Influence {
  if (!v || typeof v !== 'object') fail('influence must be an object or null');
  const x = v as Influence;
  unitArray(x.target, PARAMS, 'influence target');
  if (!Number.isInteger(x.mask) || x.mask < 0 || x.mask > ALL_GROUPS) fail(`influence mask must be a whole number from 0 to ${ALL_GROUPS}`);
  if (typeof x.source !== 'string' || !x.source.startsWith('grain-')) fail('influence source must be a blob id starting with "grain-"');
  str(x.sourceName, 'influence sourceName');
  if (fin(x.sourceRate, 'influence sourceRate') <= 0) fail('influence sourceRate must be above 0');
  if (typeof x.useSound !== 'boolean') fail('influence useSound must be true or false');
  if (x.grain !== null) {
    if (!x.grain || typeof x.grain !== 'object') fail('influence grain must be an object or null');
    const g = x.grain;
    if (fin(g.start, 'grain start') < 0 || fin(g.end, 'grain end') <= g.start) fail('grain region must have 0 <= start < end');
    if (g.refCents !== null) fin(g.refCents, 'grain refCents');
  }
  return x;
}

export interface MarkerFields { t: number; state: string; lenMs: number; shape: MorphShape }

// ---------- the commands ----------

export const commands = {
  renameProject(doc: Project, a: { name: string }): Op[] {
    str(a.name, 'project name');
    return a.name === doc.name ? [] : [{ op: 'set', path: ['name'], value: a.name }];
  },

  /** Whether playback returns to the first section after the last. */
  setLoop(doc: Project, a: { loop: boolean }): Op[] {
    if (typeof a.loop !== 'boolean') fail('loop must be true or false');
    return a.loop === doc.loop ? [] : [{ op: 'set', path: ['loop'], value: a.loop }];
  },

  /** Metronome click. Omitted fields keep their current value. */
  setMetro(doc: Project, a: Partial<Metro>): Op[] {
    const beat = doc.layers.find((l) => l.kind === 'beat');
    const cur = doc.metro ?? defaultMetro(beat?.id ?? doc.layers[0]?.id ?? '');
    const next: Metro = {
      on: a.on !== undefined ? a.on === true : cur.on,
      level: a.level !== undefined ? inRange(a.level, 'metronome level', 0, 1) : cur.level,
      voice: a.voice !== undefined ? (a.voice === 'kit' ? 'kit' : a.voice === 'tick' ? 'tick' : fail('metronome voice must be tick or kit')) : cur.voice,
      layerId: a.layerId !== undefined ? str(a.layerId, 'metronome layer') : cur.layerId,
      slot: a.slot !== undefined ? whole(a.slot, 'metronome slot', 0, KIT_SLOTS - 1) : cur.slot,
      bar: a.bar !== undefined ? whole(a.bar, 'metronome bar', 1, 32) : cur.bar,
    };
    if (next.layerId && !doc.layers.some((l) => l.id === next.layerId)) fail('metronome layer is not in this project');
    if (next.voice === cur.voice && next.on === cur.on && next.level === cur.level && next.layerId === cur.layerId && next.slot === cur.slot && next.bar === cur.bar && doc.metro) return [];
    const ops: Op[] = [{ op: 'set', path: ['metro'], value: next }];
    if (doc.schema < SCHEMA) ops.push({ op: 'set', path: ['schema'], value: SCHEMA });
    return ops;
  },

  /** Drop take records nothing in the document still uses. The caller deletes the blobs. */
  forgetTakes(doc: Project, a: { ids: string[] }): Op[] {
    if (!Array.isArray(a.ids) || a.ids.length === 0) return [];
    const used = new Set<string>();
    for (const s of doc.sections) {
      for (const c of Object.values(s.clips)) {
        if (c.take) used.add(c.take);
        for (const id of c.takes) used.add(id);
      }
    }
    const ops: Op[] = [];
    for (const id of a.ids) {
      if (typeof id !== 'string' || !doc.takes[id]) continue;
      if (used.has(id)) fail(`take ${id} is still used by a clip`);
      ops.push({ op: 'set', path: ['takes', id], value: undefined });
    }
    return ops;
  },

  addSection(doc: Project, a: { after?: string; feel: Feel; beats?: number; seconds?: number; bpm?: number }): Op[] {
    checkSectionField('feel', a.feel);
    if (a.beats !== undefined) checkSectionField('beats', a.beats);
    if (a.seconds !== undefined) checkSectionField('seconds', a.seconds);
    if (a.bpm !== undefined) checkSectionField('bpm', a.bpm);
    const ref = a.after !== undefined ? doc.sections[secIdx(doc, a.after)] : doc.sections[doc.sections.length - 1];
    const at = a.after !== undefined ? secIdx(doc, a.after) + 1 : doc.sections.length;
    const used = new Set(doc.sections.map((s) => s.name));
    const letter = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ'.split('').find((l) => !used.has(l));
    const name = letter ?? `S${doc.sections.length + 1}`;
    const sec = makeSection(name, a.feel, a.beats ?? 8, a.seconds ?? 8, a.bpm ?? ref?.bpm ?? 100);
    return [{ op: 'ins', path: ['sections', at], value: sec }];
  },

  removeSection(doc: Project, a: { id: string }): Op[] {
    const i = secIdx(doc, a.id);
    if (doc.sections.length === 1) fail('the last section cannot be removed');
    return [{ op: 'rm', path: ['sections', i] }];
  },

  /** `to` is the index the section ends up at. */
  moveSection(doc: Project, a: { id: string; to: number }): Op[] {
    const i = secIdx(doc, a.id);
    if (!Number.isInteger(a.to) || a.to < 0 || a.to >= doc.sections.length) fail(`section position must be 0 to ${doc.sections.length - 1}`);
    if (a.to === i) return [];
    return [{ op: 'rm', path: ['sections', i] }, { op: 'ins', path: ['sections', a.to], value: doc.sections[i] }];
  },

  /** Changing feel converts clip times at the grid side's bpm: ticks = seconds * bpm * 960 / 60.
   *  Going grid to free uses the section's current bpm; free to grid uses the new bpm if given. */
  setSection(doc: Project, a: { id: string } & Partial<SectionFields>): Op[] {
    const i = secIdx(doc, a.id);
    const s = doc.sections[i];
    const ops: Op[] = [];
    for (const k of sectionKeys) {
      if (a[k] === undefined) continue;
      checkSectionField(k, a[k]);
      if (a[k] !== s[k]) ops.push({ op: 'set', path: ['sections', i, k], value: a[k] });
    }
    if (a.feel !== undefined && a.feel !== s.feel) {
      const toGrid = a.feel === 'grid';
      const bpm = toGrid ? (a.bpm ?? s.bpm) : s.bpm;
      const factor = toGrid ? (bpm * PPQ) / 60 : 60 / (bpm * PPQ);
      for (const [layer, clip] of Object.entries(s.clips)) {
        ops.push({ op: 'set', path: ['sections', i, 'clips', layer], value: convertClip(clip, factor, toGrid) });
      }
    }
    return ops;
  },

  duplicateSection(doc: Project, a: { id: string }): Op[] {
    const i = secIdx(doc, a.id);
    const s = doc.sections[i];
    return [{ op: 'ins', path: ['sections', i + 1], value: { ...s, id: newId('sec'), name: `${s.name} copy` } }];
  },

  /** `sound` is a preset index: MELODY_SOUNDS for melody and texture, BEAT_KITS for beat. */
  addLayer(doc: Project, a: { kind: LayerKind; name?: string; sound?: number }): Op[] {
    if (a.kind !== 'melody' && a.kind !== 'beat' && a.kind !== 'texture') fail('layer kind must be melody, beat or texture');
    if (doc.layers.length >= MAX_LAYERS) fail(`too many layers (${MAX_LAYERS} max)`);
    const same = doc.layers.filter((l) => l.kind === a.kind).length;
    const label = a.kind[0].toUpperCase() + a.kind.slice(1);
    const name = a.name !== undefined ? str(a.name, 'layer name') : same === 0 ? label : `${label} ${same + 1}`;
    const sound = a.sound ?? (a.kind === 'texture' ? 4 : 0);
    if (!Number.isInteger(sound)) fail('sound must be a preset index');
    return [{ op: 'ins', path: ['layers', doc.layers.length], value: makeLayer(a.kind, name, sound, doc.layers.length) }];
  },

  /** Also removes the layer's clips from every section. */
  removeLayer(doc: Project, a: { id: string }): Op[] {
    const i = layIdx(doc, a.id);
    const ops: Op[] = [{ op: 'rm', path: ['layers', i] }];
    doc.sections.forEach((s, si) => {
      if (hasClip(s, a.id)) ops.push({ op: 'set', path: ['sections', si, 'clips', a.id], value: undefined });
    });
    return ops;
  },

  setLayer(doc: Project, a: { id: string } & Partial<LayerFields>): Op[] {
    const i = layIdx(doc, a.id);
    const l = doc.layers[i];
    const ops: Op[] = [];
    const put = (path: Path, value: unknown, old: unknown) => { if (value !== old) ops.push({ op: 'set', path: ['layers', i, ...path], value }); };
    if (a.name !== undefined) put(['name'], str(a.name, 'layer name'), l.name);
    if (a.level !== undefined) put(['level'], inRange(a.level, 'level', -60, 12), l.level);
    if (a.pan !== undefined) put(['pan'], inRange(a.pan, 'pan', -1, 1), l.pan);
    if (a.mute !== undefined) put(['mute'], typeof a.mute === 'boolean' ? a.mute : fail('mute must be true or false'), l.mute);
    if (a.color !== undefined) put(['color'], Number.isInteger(a.color) && a.color >= 0 && a.color <= 0xffffff ? a.color : fail('color must be 0xRRGGBB'), l.color);
    if (a.follow !== undefined) put(['follow'], inRange(a.follow, 'follow', 0, 1), l.follow);
    for (const [k, v] of Object.entries(a.depth ?? {})) {
      if (!(k in l.depth)) fail(`unknown depth field "${k}"`);
      put(['depth', k], inRange(v, `depth ${k}`, 0, 1), l.depth[k as keyof GestureDepth]);
    }
    for (const [k, v] of Object.entries(a.tune ?? {})) {
      checkTune(k as keyof Tune, v);
      put(['tune', k], v, l.tune[k as keyof Tune]);
    }
    return ops;
  },

  /** `base` (96 values of the old parameters) replaces state 0's sound; `kit` replaces the four kit
   *  slots' sounds in every state and their pitches (beat layers only). */
  setLayerSound(doc: Project, a: { id: string; base?: number[]; kit?: { params: number[]; cents: number }[] }): Op[] {
    const i = layIdx(doc, a.id);
    const l = doc.layers[i];
    const ops: Op[] = [];
    const states = l.states.map((s) => s.values);
    if (a.base !== undefined) {
      checkParams(a.base, 'base sound');
      states[0] = withVocab(l.kind, states[0], 0, a.base);
    }
    if (a.kit !== undefined) {
      if (l.kind !== 'beat') fail(`layer "${l.name}" is not a beat layer and has no kit`);
      if (!Array.isArray(a.kit) || a.kit.length !== KIT_SLOTS) fail(`kit must have ${KIT_SLOTS} slots`);
      for (const s of a.kit) { checkParams(s.params, 'kit slot sound'); inRange(s.cents, 'kit slot cents', 0, MAX_CENTS); }
      for (let si = 0; si < states.length; si++) a.kit.forEach((k, slot) => { states[si] = withVocab(l.kind, states[si], slot + 1, k.params); });
      ops.push({ op: 'set', path: ['layers', i, 'kit'], value: a.kit.map((k) => ({ cents: k.cents })) });
    }
    states.forEach((v, si) => { if (v !== l.states[si].values) ops.push({ op: 'set', path: ['layers', i, 'states', si, 'values'], value: v }); });
    return ops;
  },

  /** Place or clear a whole clip. Also the path for placing a captured take (addTake first). */
  setClip(doc: Project, a: { section: string; layer: string; clip: Clip | null }): Op[] {
    const w = where(doc, a.section, a.layer);
    if (a.clip === null) {
      return hasClip(w.section, a.layer) ? [{ op: 'set', path: w.clipPath, value: undefined }] : [];
    }
    if ((a.clip.kind === 'beat') !== (w.layer.kind === 'beat')) fail(`a ${a.clip.kind} clip does not fit the ${w.layer.kind} layer "${w.layer.name}"`);
    const clip = normClip(a.clip, w.section, new Set(w.layer.states.map((x) => x.id)));
    if (hasClip(w.section, a.layer) && w.section.clips[a.layer] === clip) return [];
    return [{ op: 'set', path: w.clipPath, value: clip }];
  },

  addTake(doc: Project, a: { take: Take }): Op[] {
    if (typeof a.take?.id !== 'string' || a.take.id === '') fail('take needs an id');
    if (doc.takes[a.take.id] === a.take) return [];
    return [{ op: 'set', path: ['takes', a.take.id], value: a.take }];
  },

  /** Shrinking loopLen wraps notes and hits that fall past the new end back into the loop. */
  setClipFields(doc: Project, a: { section: string; layer: string; loopLen?: number; timeSnap?: number }): Op[] {
    const w = where(doc, a.section, a.layer);
    const clip = hasClip(w.section, a.layer) ? w.section.clips[a.layer] : fail('no clip in that section and layer');
    const c = clip as Clip;
    const grid = w.section.feel === 'grid';
    const ops: Op[] = [];
    if (a.timeSnap !== undefined) {
      if (fin(a.timeSnap, 'timeSnap') < 0) fail('timeSnap must be 0 or more');
      if (a.timeSnap !== c.timeSnap) ops.push({ op: 'set', path: [...w.clipPath, 'timeSnap'], value: a.timeSnap });
    }
    if (a.loopLen !== undefined) {
      if (fin(a.loopLen, 'loopLen') <= 0) fail('loopLen must be above 0');
      const loopLen = grid ? Math.max(1, Math.round(a.loopLen)) : a.loopLen;
      if (loopLen !== c.loopLen) {
        ops.push({ op: 'set', path: [...w.clipPath, 'loopLen'], value: loopLen });
        const min = minDur(w.section);
        const morph = (c.morph ?? []).map((m) => normMarker(m, loopLen, grid, null));
        if (!sameItems(morph, c.morph ?? [])) ops.push({ op: 'set', path: [...w.clipPath, 'morph'], value: sortByT(morph) });
        if (c.kind === 'beat') {
          const hits = c.hits.map((h) => normHit(h, loopLen, grid, min));
          if (!sameItems(hits, c.hits)) ops.push({ op: 'set', path: [...w.clipPath, 'hits'], value: sortByT(hits) });
        } else {
          const notes = c.notes.map((n) => normNote(n, loopLen, grid, min));
          if (!sameItems(notes, c.notes)) ops.push({ op: 'set', path: [...w.clipPath, 'notes'], value: sortByT(notes) });
        }
      }
    }
    return ops;
  },

  /** Adds a note; creates an empty clip as long as the section if the lane has none. Notes stay sorted by t. */
  addNote(doc: Project, a: { section: string; layer: string; note: WithId<Note> }): Op[] {
    const w = where(doc, a.section, a.layer);
    const c = melodyClip(w);
    const len = c ? c.loopLen : sectionLength(w.section);
    const note = normNote({ ...a.note, id: a.note.id ?? newId('n') } as Note, len, w.section.feel === 'grid', minDur(w.section));
    if (c?.notes.some((n) => n.id === note.id)) fail(`note id "${note.id}" already exists in this clip`);
    if (!c) {
      const clip: MelodyClip = { kind: w.layer.kind as 'melody' | 'texture', take: '', takes: [], loopLen: len, timeSnap: 0, morph: [], notes: [note], warp: [] };
      return [{ op: 'set', path: w.clipPath, value: clip }];
    }
    return [{ op: 'ins', path: [...w.clipPath, 'notes', insertSorted(c.notes, note)], value: note }];
  },

  /** dt in section units, wraps inside the loop; dcents is clamped to 0..12000 after the move. */
  moveNotes(doc: Project, a: { section: string; layer: string; ids: string[]; dt: number; dcents: number }): Op[] {
    const w = where(doc, a.section, a.layer);
    fin(a.dt, 'dt'); fin(a.dcents, 'dcents');
    const c = melodyClip(w) ?? fail('no clip in that section and layer');
    const sel = idSet(a.ids, c.notes, 'note');
    const grid = w.section.feel === 'grid';
    const next = c.notes.map((n) => {
      if (!sel.has(n.id)) return n;
      const t = wrapT(n.t + a.dt, c.loopLen, grid);
      const cents = clamp(n.cents + a.dcents, 0, MAX_CENTS);
      return t === n.t && cents === n.cents ? n : { ...n, t, cents };
    });
    if (sameItems(next, c.notes)) return [];
    return [{ op: 'set', path: [...w.clipPath, 'notes'], value: sortByT(next) }];
  },

  resizeNote(doc: Project, a: { section: string; layer: string; id: string; dur: number }): Op[] {
    const w = where(doc, a.section, a.layer);
    fin(a.dur, 'dur');
    const c = melodyClip(w) ?? fail('no clip in that section and layer');
    idSet([a.id], c.notes, 'note');
    const dur = Math.max(minDur(w.section), w.section.feel === 'grid' ? Math.round(a.dur) : a.dur);
    const i = c.notes.findIndex((n) => n.id === a.id);
    return c.notes[i].dur === dur ? [] : [{ op: 'set', path: [...w.clipPath, 'notes', i, 'dur'], value: dur }];
  },

  deleteNotes(doc: Project, a: { section: string; layer: string; ids: string[] }): Op[] {
    const w = where(doc, a.section, a.layer);
    const c = melodyClip(w) ?? fail('no clip in that section and layer');
    const sel = idSet(a.ids, c.notes, 'note');
    const ops: Op[] = [];
    for (let i = c.notes.length - 1; i >= 0; i--) if (sel.has(c.notes[i].id)) ops.push({ op: 'rm', path: [...w.clipPath, 'notes', i] });
    return ops;
  },

  addHit(doc: Project, a: { section: string; layer: string; hit: WithId<Hit> }): Op[] {
    const w = where(doc, a.section, a.layer);
    const c = beatClip(w);
    const len = c ? c.loopLen : sectionLength(w.section);
    const hit = normHit({ ...a.hit, id: a.hit.id ?? newId('h') } as Hit, len, w.section.feel === 'grid', minDur(w.section));
    if (c?.hits.some((h) => h.id === hit.id)) fail(`hit id "${hit.id}" already exists in this clip`);
    if (!c) {
      const clip: BeatClip = { kind: 'beat', take: '', takes: [], loopLen: len, timeSnap: 0, morph: [], hits: [hit] };
      return [{ op: 'set', path: w.clipPath, value: clip }];
    }
    return [{ op: 'ins', path: [...w.clipPath, 'hits', insertSorted(c.hits, hit)], value: hit }];
  },

  moveHits(doc: Project, a: { section: string; layer: string; ids: string[]; dt: number }): Op[] {
    const w = where(doc, a.section, a.layer);
    fin(a.dt, 'dt');
    const c = beatClip(w) ?? fail('no clip in that section and layer');
    const sel = idSet(a.ids, c.hits, 'hit');
    const grid = w.section.feel === 'grid';
    const next = c.hits.map((h) => {
      if (!sel.has(h.id)) return h;
      const t = wrapT(h.t + a.dt, c.loopLen, grid);
      return t === h.t ? h : { ...h, t };
    });
    if (sameItems(next, c.hits)) return [];
    return [{ op: 'set', path: [...w.clipPath, 'hits'], value: sortByT(next) }];
  },

  setHitSlot(doc: Project, a: { section: string; layer: string; ids: string[]; slot: number }): Op[] {
    const w = where(doc, a.section, a.layer);
    if (!Number.isInteger(a.slot) || a.slot < 0 || a.slot >= KIT_SLOTS) fail(`slot must be a whole number from 0 to ${KIT_SLOTS - 1}`);
    const c = beatClip(w) ?? fail('no clip in that section and layer');
    const sel = idSet(a.ids, c.hits, 'hit');
    const ops: Op[] = [];
    c.hits.forEach((h, i) => {
      if (sel.has(h.id) && h.slot !== a.slot) ops.push({ op: 'set', path: [...w.clipPath, 'hits', i, 'slot'], value: a.slot });
    });
    return ops;
  },

  deleteHits(doc: Project, a: { section: string; layer: string; ids: string[] }): Op[] {
    const w = where(doc, a.section, a.layer);
    const c = beatClip(w) ?? fail('no clip in that section and layer');
    const sel = idSet(a.ids, c.hits, 'hit');
    const ops: Op[] = [];
    for (let i = c.hits.length - 1; i >= 0; i--) if (sel.has(c.hits[i].id)) ops.push({ op: 'rm', path: [...w.clipPath, 'hits', i] });
    return ops;
  },

  /** Snap note or hit starts to the nearest multiple of `grid` (section units). `ids` limits it to some items. */
  quantize(doc: Project, a: { section: string; layer: string; grid: number; ids?: string[] }): Op[] {
    const w = where(doc, a.section, a.layer);
    if (fin(a.grid, 'grid') <= 0) fail('grid must be above 0');
    const c = hasClip(w.section, a.layer) ? (w.section.clips[a.layer] as Clip) : fail('no clip in that section and layer');
    const gridTime = w.section.feel === 'grid';
    const items: { id: string; t: number }[] = c.kind === 'beat' ? c.hits : c.notes;
    const sel = a.ids ? idSet(a.ids, items, c.kind === 'beat' ? 'hit' : 'note') : null;
    const snap = <T extends { id: string; t: number }>(x: T): T => {
      if (sel && !sel.has(x.id)) return x;
      const t = wrapT(Math.round(x.t / a.grid) * a.grid, c.loopLen, gridTime);
      return t === x.t ? x : { ...x, t };
    };
    if (c.kind === 'beat') {
      const next = c.hits.map(snap);
      return sameItems(next, c.hits) ? [] : [{ op: 'set', path: [...w.clipPath, 'hits'], value: sortByT(next) }];
    }
    const next = c.notes.map(snap);
    return sameItems(next, c.notes) ? [] : [{ op: 'set', path: [...w.clipPath, 'notes'], value: sortByT(next) }];
  },

  /** Set or clear (null) the layer's Influence. */
  setInfluence(doc: Project, a: { layer: string; influence: Influence | null }): Op[] {
    const i = layIdx(doc, a.layer);
    const l = doc.layers[i];
    if (a.influence === null) return l.influence ? [{ op: 'set', path: ['layers', i, 'influence'], value: undefined }] : [];
    const infl = checkInfluence(a.influence);
    return l.influence === infl ? [] : [{ op: 'set', path: ['layers', i, 'influence'], value: infl }];
  },

  setInfluenceAmount(doc: Project, a: { layer: string; state: string; amount: number }): Op[] {
    const i = layIdx(doc, a.layer);
    const si = stateIdx(doc.layers[i], a.state);
    inRange(a.amount, 'influence amount', 0, 1);
    return doc.layers[i].states[si].influenceAmount === a.amount ? [] : [{ op: 'set', path: ['layers', i, 'states', si, 'influenceAmount'], value: a.amount }];
  },

  setInfluenceMask(doc: Project, a: { layer: string; mask: number }): Op[] {
    const i = layIdx(doc, a.layer);
    const infl = doc.layers[i].influence ?? fail('this layer has no Influence');
    if (!Number.isInteger(a.mask) || a.mask < 0 || a.mask > ALL_GROUPS) fail(`influence mask must be a whole number from 0 to ${ALL_GROUPS}`);
    return infl.mask === a.mask ? [] : [{ op: 'set', path: ['layers', i, 'influence', 'mask'], value: a.mask }];
  },

  /** Write the blended values into the state's base, then clear the Influence and set that state's amount to 0
   *  (TDD "Commit writes the blended values into the state's base and clears the target"). Other states keep
   *  their stored amounts; with no Influence they are not sent to the engine. */
  commitInfluence(doc: Project, a: { layer: string; state: string }): Op[] {
    const i = layIdx(doc, a.layer);
    const l = doc.layers[i];
    const infl = l.influence ?? fail('this layer has no Influence');
    const si = stateIdx(l, a.state);
    const st = l.states[si];
    const blended = blendInfluence(vocabFromValues(l.kind, st.values), infl.target, infl.mask, st.influenceAmount);
    return [
      { op: 'set', path: ['layers', i, 'states', si, 'values'], value: withVocab(l.kind, st.values, 0, blended) },
      { op: 'set', path: ['layers', i, 'states', si, 'influenceAmount'], value: 0 },
      { op: 'set', path: ['layers', i, 'influence'], value: undefined },
    ];
  },

  /** A new state, copied from `from` (default: the first state). Ids come from `id` or are made. */
  addState(doc: Project, a: { layer: string; from?: string; name?: string; id?: string }): Op[] {
    const i = layIdx(doc, a.layer);
    const l = doc.layers[i];
    if (l.states.length >= MAX_STATES) fail(`too many states (${MAX_STATES} max)`);
    const src = l.states[a.from !== undefined ? stateIdx(l, a.from) : 0];
    const id = a.id !== undefined ? str(a.id, 'state id') : newId('st');
    if (l.states.some((s) => s.id === id)) fail(`state id "${id}" already exists`);
    const name = a.name !== undefined ? str(a.name, 'state name') : `State ${l.states.length + 1}`;
    const st: State = { id, name, values: { ...src.values }, macros: src.macros.slice(), influenceAmount: src.influenceAmount };
    return [{ op: 'ins', path: ['layers', i, 'states', l.states.length], value: st }];
  },

  /** `base`: 96 values of the old parameters for the state's own sound (kit slots keep theirs). */
  setState(doc: Project, a: { layer: string; id: string; name?: string; base?: number[]; macros?: number[]; influenceAmount?: number }): Op[] {
    const i = layIdx(doc, a.layer);
    const l = doc.layers[i];
    const si = stateIdx(l, a.id);
    const st = l.states[si];
    const ops: Op[] = [];
    const put = (k: keyof State, value: unknown) => { if (value !== st[k]) ops.push({ op: 'set', path: ['layers', i, 'states', si, k], value }); };
    if (a.name !== undefined) put('name', str(a.name, 'state name'));
    if (a.base !== undefined) put('values', withVocab(l.kind, st.values, 0, unitArray(a.base, PARAMS, 'state base')));
    if (a.macros !== undefined) put('macros', unitArray(a.macros, N_MACROS, 'state macros'));
    if (a.influenceAmount !== undefined) put('influenceAmount', inRange(a.influenceAmount, 'influence amount', 0, 1));
    return ops;
  },

  /** Not the last state. Morph markers that point at it are removed from every clip of the layer. */
  removeState(doc: Project, a: { layer: string; id: string }): Op[] {
    const i = layIdx(doc, a.layer);
    const l = doc.layers[i];
    const si = stateIdx(l, a.id);
    if (l.states.length === 1) fail('the last state cannot be removed');
    const ops: Op[] = [{ op: 'rm', path: ['layers', i, 'states', si] }];
    doc.sections.forEach((s, sec) => {
      const c = hasClip(s, l.id) ? s.clips[l.id] : undefined;
      if (!c || !c.morph) return;
      const keep = c.morph.filter((m) => m.state !== a.id);
      if (keep.length !== c.morph.length) ops.push({ op: 'set', path: ['sections', sec, 'clips', l.id, 'morph'], value: keep });
    });
    return ops;
  },

  /** Adds a marker; creates an empty clip as long as the section if the lane has none. Needs 0 <= t < loopLen. */
  addMorphMarker(doc: Project, a: { section: string; layer: string; marker: Omit<MarkerFields, 'lenMs' | 'shape'> & Partial<MarkerFields> }): Op[] {
    const w = where(doc, a.section, a.layer);
    const c = hasClip(w.section, a.layer) ? (w.section.clips[a.layer] as Clip) : undefined;
    const len = c ? c.loopLen : sectionLength(w.section);
    const grid = w.section.feel === 'grid';
    const m = a.marker;
    const marker = normMarkerInside({ t: m.t, state: m.state, lenMs: m.lenMs ?? 0, shape: m.shape ?? 'line' }, len, grid, w.layer);
    if (!c) {
      const base = { take: '', takes: [] as string[], loopLen: len, timeSnap: 0, morph: [marker] };
      const clip: Clip = w.layer.kind === 'beat'
        ? { kind: 'beat', ...base, hits: [] }
        : { kind: w.layer.kind, ...base, notes: [], warp: [] };
      return [{ op: 'set', path: w.clipPath, value: clip }];
    }
    const morph = c.morph ?? [];
    if (!c.morph) return [{ op: 'set', path: [...w.clipPath, 'morph'], value: [marker] }];
    return [{ op: 'ins', path: [...w.clipPath, 'morph', insertSorted(morph, marker)], value: marker }];
  },

  /** `index` is the marker's position in the clip's morph list now. Changing t re-sorts the list. */
  moveMorphMarker(doc: Project, a: { section: string; layer: string; index: number; t?: number; lenMs?: number; shape?: MorphShape; state?: string }): Op[] {
    const w = where(doc, a.section, a.layer);
    const c = hasClip(w.section, a.layer) ? (w.section.clips[a.layer] as Clip) : fail('no clip in that section and layer');
    const morph = c.morph ?? [];
    if (!Number.isInteger(a.index) || a.index < 0 || a.index >= morph.length) fail(`marker index must be 0 to ${morph.length - 1}`);
    const old = morph[a.index];
    const next = normMarkerInside({ t: a.t ?? old.t, state: a.state ?? old.state, lenMs: a.lenMs ?? old.lenMs, shape: a.shape ?? old.shape },
      c.loopLen, w.section.feel === 'grid', w.layer);
    if (next.t === old.t && next.state === old.state && next.lenMs === old.lenMs && next.shape === old.shape) return [];
    const list = morph.slice();
    list[a.index] = next;
    return [{ op: 'set', path: [...w.clipPath, 'morph'], value: next.t === old.t ? list : sortByT(list) }];
  },

  removeMorphMarker(doc: Project, a: { section: string; layer: string; index: number }): Op[] {
    const w = where(doc, a.section, a.layer);
    const c = hasClip(w.section, a.layer) ? (w.section.clips[a.layer] as Clip) : fail('no clip in that section and layer');
    const n = c.morph?.length ?? 0;
    if (!Number.isInteger(a.index) || a.index < 0 || a.index >= n) fail(`marker index must be 0 to ${n - 1}`);
    return [{ op: 'rm', path: [...w.clipPath, 'morph', a.index] }];
  },

  /** "Use my sound for a kit slot": replace one slot's sound and default pitch. */
  setKitSlot(doc: Project, a: { layer: string; slot: number; params: number[]; cents: number }): Op[] {
    const i = layIdx(doc, a.layer);
    const l = doc.layers[i];
    if (l.kind !== 'beat') fail(`layer "${l.name}" is not a beat layer and has no kit`);
    if (!Number.isInteger(a.slot) || a.slot < 0 || a.slot >= KIT_SLOTS) fail(`slot must be a whole number from 0 to ${KIT_SLOTS - 1}`);
    unitArray(a.params, PARAMS, 'kit slot sound');
    inRange(a.cents, 'kit slot cents', 0, MAX_CENTS);
    const ops: Op[] = [];
    if (l.kit[a.slot]?.cents !== a.cents) ops.push({ op: 'set', path: ['layers', i, 'kit', a.slot], value: { cents: a.cents } });
    l.states.forEach((st, si) => {
      const next = withVocab(l.kind, st.values, a.slot + 1, a.params);
      if (Object.keys(next).some((k) => next[k] !== st.values[k])) ops.push({ op: 'set', path: ['layers', i, 'states', si, 'values'], value: next });
    });
    return ops;
  },

  /** Switch to an older take in the clip's stack. The caller rebuilds notes/hits (setClip). */
  selectTake(doc: Project, a: { section: string; layer: string; take: string }): Op[] {
    const w = where(doc, a.section, a.layer);
    const c = hasClip(w.section, a.layer) ? (w.section.clips[a.layer] as Clip) : fail('no clip in that section and layer');
    if (!c.takes.includes(a.take)) fail(`take "${a.take}" is not in this clip's stack`);
    return c.take === a.take ? [] : [{ op: 'set', path: [...w.clipPath, 'take'], value: a.take }];
  },
};

export type CommandName = keyof typeof commands;
export type CommandArgs<K extends CommandName> = Parameters<(typeof commands)[K]>[1];
