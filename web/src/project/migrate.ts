// Schema migration (TDD "Persistence"). Each step is a pure function from schema n to n+1;
// `migrate` runs them in order. A document from a newer app is refused rather than guessed at.
//
// A stored project is more than the document: the undo history and the crash journal hold patch
// ops whose values can contain pieces of the old shape (a whole clip, a whole section). Those
// are upgraded with the same step, so replaying an old journal entry or undoing an old action
// cannot put an old-shape clip back into a new document. Each step therefore has two parts:
// `doc` for the document and `value` for any value inside an op.

import type { Group, HistoryState, JournalEntry } from './history';
import type { Op } from './patch';
import { classicPatch, valuesFromVocab } from '../synth/classic';
import { SCHEMA, defaultMetro } from './types';
import type { LayerKind, Metro, Project } from './types';

interface Step {
  /** The schema this step upgrades from; it produces from + 1. */
  from: number;
  /** Upgrade a whole document. Returns a new object. */
  doc(doc: Record<string, unknown>): Record<string, unknown>;
  /** Upgrade one value found in an op, anywhere in it. Returns the same object when nothing changes. */
  value(v: unknown): unknown;
  /** The step cannot upgrade old ops: the undo history starts fresh, and crash-journal entries
   *  written before it are replayed on the document before this step runs. */
  resetsHistory?: boolean;
}

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);

/** Apply `fn` to every object inside `v` (children first); untouched parts keep their identity. */
function mapObjects(v: unknown, fn: (o: Record<string, unknown>) => Record<string, unknown>): unknown {
  if (Array.isArray(v)) {
    let out: unknown[] | null = null;
    v.forEach((x, i) => {
      const y = mapObjects(x, fn);
      if (y !== x) (out ??= v.slice())[i] = y;
    });
    return out ?? v;
  }
  if (!isObj(v)) return v;
  let out: Record<string, unknown> | null = null;
  for (const [k, x] of Object.entries(v)) {
    const y = mapObjects(x, fn);
    if (y !== x) (out ??= { ...v })[k] = y;
  }
  return fn(out ?? v);
}

const looksLikeClip = (o: Record<string, unknown>) =>
  (o.kind === 'melody' || o.kind === 'texture' || o.kind === 'beat') && typeof o.loopLen === 'number';

/** 1 -> 2: every clip gets an empty morph lane. */
const v1to2: Step = {
  from: 1,
  doc(doc) {
    const sections = Array.isArray(doc.sections) ? doc.sections : [];
    return { ...doc, schema: 2, sections: this.value(sections) };
  },
  value(v) {
    return mapObjects(v, (o) => (looksLikeClip(o) && !Array.isArray(o.morph) ? { ...o, morph: [] } : o));
  },
};

function metroOf(v: unknown): Metro {
  if (!isObj(v)) return defaultMetro();
  const voice = v.voice === 'kit' ? 'kit' : 'tick';
  const level = typeof v.level === 'number' && Number.isFinite(v.level) ? Math.min(1, Math.max(0, v.level)) : 0.7;
  const bar = typeof v.bar === 'number' && Number.isFinite(v.bar) ? Math.min(32, Math.max(1, Math.round(v.bar))) : 4;
  const slot = typeof v.slot === 'number' && Number.isInteger(v.slot) ? Math.min(3, Math.max(0, v.slot)) : 0;
  const layerId = typeof v.layerId === 'string' ? v.layerId : '';
  return { on: v.on === true, level, voice, layerId, slot, bar };
}

/** 2 -> 3: the document carries metronome settings. A project-shaped value inside an old op gets them too. */
const v2to3: Step = {
  from: 2,
  doc(doc) {
    const metro = isObj(doc.metro) ? doc.metro : metroOf(doc.metro);
    return { ...doc, schema: 3, metro };
  },
  value(v) {
    return mapObjects(v, (o) => {
      const projectLike = Array.isArray(o.sections) && Array.isArray(o.layers) && typeof o.schema === 'number';
      return projectLike && !isObj(o.metro) ? { ...o, metro: defaultMetro() } : o;
    });
  },
};

const num = (v: unknown, d: number) => (typeof v === 'number' && Number.isFinite(v) ? v : d);

/** 3 -> 4 (docs/synth.md "Existing projects and presets"): each layer plays the classic patch;
 *  a state's 96 values and the layer's kit slot sets become values by key (the kit's in every
 *  state); kit slots keep their pitch; notes get velocity 1. Old ops name paths that no longer
 *  exist (states[0].base), so the undo history starts fresh. */
const v3to4: Step = {
  from: 3,
  resetsHistory: true,
  doc(doc) {
    const layers = Array.isArray(doc.layers) ? doc.layers : [];
    const sections = Array.isArray(doc.sections) ? doc.sections : [];
    return {
      ...doc,
      schema: 4,
      layers: layers.map((l) => {
        if (!isObj(l)) return l;
        const kind = (l.kind === 'beat' || l.kind === 'texture' ? l.kind : 'melody') as LayerKind;
        const kitIn = Array.isArray(l.kit) ? l.kit : [];
        const kitSets = kitIn.map((k) => (isObj(k) && Array.isArray(k.params) ? (k.params as number[]) : []));
        const states = (Array.isArray(l.states) ? l.states : []).map((st) => {
          if (!isObj(st)) return st;
          const { base, ...rest } = st;
          return { ...rest, values: valuesFromVocab(kind, Array.isArray(base) ? (base as number[]) : [], kitSets) };
        });
        const kit = kitIn.map((k) => ({ cents: num(isObj(k) ? k.cents : undefined, 4800) }));
        return { ...l, patch: classicPatch(kind), states, kit };
      }),
      sections: mapObjects(sections, (o) =>
        looksLikeClip(o) && Array.isArray(o.notes) ? { ...o, notes: o.notes.map((n) => (isObj(n) && n.vel === undefined ? { ...n, vel: 1 } : n)) } : o),
    };
  },
  value(v) {
    return v;
  },
};

/** 4 -> 5 (docs/quant.md, Q1): flex settings, beat clip warp maps and take anchors are optional
 *  fields, so a document without them plays as before. Only the number changes. */
const v4to5: Step = {
  from: 4,
  doc(doc) {
    return { ...doc, schema: 5 };
  },
  value(v) {
    return v;
  },
};

/** 5 -> 6 (docs/poly.md, P3): a layer's capture mode, a note's curve and a take's note curves
 *  are optional fields; a document without them plays as before. Only the number changes. */
const v5to6: Step = {
  from: 5,
  doc(doc) {
    return { ...doc, schema: 6 };
  },
  value(v) {
    return v;
  },
};

const STEPS: Step[] = [v1to2, v2to3, v3to4, v4to5, v5to6];

function stepsFrom(schema: unknown): Step[] {
  if (typeof schema !== 'number' || !Number.isInteger(schema) || schema < 1) {
    throw new RangeError(`project has no valid schema number (got ${String(schema)})`);
  }
  if (schema > SCHEMA) {
    throw new RangeError(`project was saved by a newer version of the app (schema ${schema}, this build reads up to ${SCHEMA})`);
  }
  return STEPS.filter((s) => s.from >= schema);
}

/** Bring a stored document up to the current schema (or to `upTo`). Returns the same object if
 *  it is current. Throws RangeError for a future or malformed schema. */
export function migrate(doc: unknown, upTo = SCHEMA): Project {
  if (!isObj(doc)) throw new RangeError('project document is not an object');
  const steps = stepsFrom(doc.schema).filter((s) => s.from < upTo);
  let cur: Record<string, unknown> = doc;
  for (const s of steps) cur = s.doc(cur);
  return cur as unknown as Project;
}

/** The last schema from which old ops can still be upgraded when coming from `fromSchema`:
 *  the `from` of the first step that resets the history, or SCHEMA when none does. */
export function replaySchema(fromSchema: number): number {
  return stepsFrom(fromSchema).find((s) => s.resetsHistory)?.from ?? SCHEMA;
}

/** Does upgrading from `fromSchema` start a fresh undo history. */
export function historyResets(fromSchema: number): boolean {
  return replaySchema(fromSchema) < SCHEMA;
}

function migrateOps(ops: Op[], steps: Step[]): Op[] {
  let changed = false;
  const out = ops.map((op) => {
    if (op.op === 'rm') return op;
    let value = op.value;
    for (const s of steps) value = s.value(value);
    if (value === op.value) return op;
    changed = true;
    return { ...op, value } as Op;
  });
  return changed ? out : ops;
}

/** An undo group recorded under `fromSchema`, upgraded (to `upTo`). Same object if nothing changes. */
export function migrateGroup(g: Group, fromSchema: number, upTo = SCHEMA): Group {
  const steps = stepsFrom(fromSchema).filter((s) => s.from < upTo);
  const ops = migrateOps(g.ops, steps);
  const inverse = migrateOps(g.inverse, steps);
  return ops === g.ops && inverse === g.inverse ? g : { ...g, ops, inverse };
}

export function migrateEntry(e: JournalEntry, fromSchema: number, upTo = SCHEMA): JournalEntry {
  const group = migrateGroup(e.group, fromSchema, upTo);
  return group === e.group ? e : { ...e, group };
}

/** The undo history upgraded; empty when a step on the way cannot upgrade old ops. */
export function migrateHistory(h: HistoryState, fromSchema: number): HistoryState {
  if (stepsFrom(fromSchema).length === 0) return h;
  if (historyResets(fromSchema)) return { undo: [], redo: [] };
  return { undo: h.undo.map((g) => migrateGroup(g, fromSchema)), redo: h.redo.map((g) => migrateGroup(g, fromSchema)) };
}
