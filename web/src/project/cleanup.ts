// Which takes and blobs a project still needs. Cleanup drops the rest: a take left
// behind when a clip was replaced, and a grain source whose Influence was cleared.
// Undo and redo are included, so stepping backwards can still find the audio.

import type { Group, HistoryState } from './history';
import type { Project } from './types';

export interface CleanupPlan {
  /** Take ids in `doc.takes` that no clip and no undo entry points at. */
  takeIds: string[];
  /** Blob refs the document and the history still need. Everything else in the store can go. */
  keep: string[];
}

function walk(v: unknown, knownTakes: Set<string>, takes: Set<string>, grains: Set<string>): void {
  if (typeof v === 'string') {
    if (knownTakes.has(v)) takes.add(v);
    if (v.startsWith('grain-')) grains.add(v);
    if (v.startsWith('audio-')) takes.add(v.slice('audio-'.length));
    if (v.startsWith('frames-')) takes.add(v.slice('frames-'.length));
    if (v.startsWith('anchors-')) takes.add(v.slice('anchors-'.length));
    if (v.startsWith('notecurves-')) takes.add(v.slice('notecurves-'.length));
    return;
  }
  if (Array.isArray(v)) {
    for (const x of v) walk(x, knownTakes, takes, grains);
    return;
  }
  if (v && typeof v === 'object') {
    for (const x of Object.values(v)) walk(x, knownTakes, takes, grains);
  }
}

function walkGroups(groups: Group[], knownTakes: Set<string>, takes: Set<string>, grains: Set<string>): void {
  for (const g of groups) {
    walk(g.ops, knownTakes, takes, grains);
    walk(g.inverse, knownTakes, takes, grains);
  }
}

/** Takes and blob refs still reachable from the document or the undo history. */
export function cleanupPlan(doc: Project, history: HistoryState | { undo: Group[]; redo: Group[] }): CleanupPlan {
  const knownTakes = new Set(Object.keys(doc.takes));
  const takes = new Set<string>();
  const grains = new Set<string>();
  // The takes map itself names every take, so it is not a reason to keep one.
  walk(doc.sections, knownTakes, takes, grains);
  walk(doc.layers, knownTakes, takes, grains);
  walkGroups(history.undo, knownTakes, takes, grains);
  walkGroups(history.redo, knownTakes, takes, grains);
  const keep = new Set<string>(grains);
  for (const id of takes) {
    const t = doc.takes[id];
    if (t?.audio) keep.add(t.audio);
    if (t?.frames) keep.add(t.frames);
    if (t?.anchors) keep.add(t.anchors);
    if (t?.noteCurves) keep.add(t.noteCurves);
    keep.add(`audio-${id}`);
    keep.add(`frames-${id}`);
    keep.add(`anchors-${id}`);
    keep.add(`notecurves-${id}`);
  }
  return { takeIds: [...knownTakes].filter((id) => !takes.has(id)).sort(), keep: [...keep].sort() };
}

/** Blob refs in `listed` that the plan does not keep. */
export function unusedRefs(listed: string[], plan: CleanupPlan): string[] {
  const keep = new Set(plan.keep);
  return listed.filter((ref) => !keep.has(ref)).sort();
}
