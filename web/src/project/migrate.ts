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
import { SCHEMA } from './types';
import type { Project } from './types';

interface Step {
  /** The schema this step upgrades from; it produces from + 1. */
  from: number;
  /** Upgrade a whole document. Returns a new object. */
  doc(doc: Record<string, unknown>): Record<string, unknown>;
  /** Upgrade one value found in an op, anywhere in it. Returns the same object when nothing changes. */
  value(v: unknown): unknown;
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

const STEPS: Step[] = [v1to2];

function stepsFrom(schema: unknown): Step[] {
  if (typeof schema !== 'number' || !Number.isInteger(schema) || schema < 1) {
    throw new RangeError(`project has no valid schema number (got ${String(schema)})`);
  }
  if (schema > SCHEMA) {
    throw new RangeError(`project was saved by a newer version of the app (schema ${schema}, this build reads up to ${SCHEMA})`);
  }
  return STEPS.filter((s) => s.from >= schema);
}

/** Bring a stored document up to the current schema. Returns the same object if it is current.
 *  Throws RangeError for a future or malformed schema. */
export function migrate(doc: unknown): Project {
  if (!isObj(doc)) throw new RangeError('project document is not an object');
  const steps = stepsFrom(doc.schema);
  let cur: Record<string, unknown> = doc;
  for (const s of steps) cur = s.doc(cur);
  return cur as unknown as Project;
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

/** An undo group recorded under `fromSchema`, upgraded. Same object if nothing changes. */
export function migrateGroup(g: Group, fromSchema: number): Group {
  const steps = stepsFrom(fromSchema);
  const ops = migrateOps(g.ops, steps);
  const inverse = migrateOps(g.inverse, steps);
  return ops === g.ops && inverse === g.inverse ? g : { ...g, ops, inverse };
}

export function migrateEntry(e: JournalEntry, fromSchema: number): JournalEntry {
  const group = migrateGroup(e.group, fromSchema);
  return group === e.group ? e : { ...e, group };
}

export function migrateHistory(h: HistoryState, fromSchema: number): HistoryState {
  if (stepsFrom(fromSchema).length === 0) return h;
  return { undo: h.undo.map((g) => migrateGroup(g, fromSchema)), redo: h.redo.map((g) => migrateGroup(g, fromSchema)) };
}
