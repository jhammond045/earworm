// The pattern grid: rows are subdivisions of the section, columns are layers.
// This is Earworm's track (sections, notes, hits, the morph lane), not a tracker note column.

import { SLOT_NAMES } from '../m2/sounds';
import { PPQ, type Clip, type Layer, type Section } from '../project/types';

export const STEPS_PER_BEAT = 4;

const NAMES = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];

export function centsToName(cents: number): string {
  const midi = Math.round(cents / 100);
  const name = NAMES[((midi % 12) + 12) % 12];
  return `${name}${Math.floor(midi / 12)}`;
}

export function sectionLength(section: Section): number {
  return section.feel === 'grid' ? section.beats * PPQ : section.seconds;
}

/** How many pattern rows the section has. Free time uses four rows per second. */
export function rowCount(section: Section): number {
  if (section.feel === 'free') return Math.max(1, Math.round(section.seconds * STEPS_PER_BEAT));
  return Math.max(1, Math.round(section.beats * STEPS_PER_BEAT));
}

export function rowStart(section: Section, row: number): number {
  const n = rowCount(section);
  return (Math.max(0, row) / n) * sectionLength(section);
}

/** The row the playhead is in, clamped into the section. */
export function playRow(section: Section, pos: number): number {
  const n = rowCount(section);
  const len = sectionLength(section);
  if (!(len > 0) || !Number.isFinite(pos)) return 0;
  const row = Math.floor((Math.max(0, pos) / len) * n);
  return Math.max(0, Math.min(n - 1, row));
}

export interface CellView {
  label: string;
  title: string;
  eventId: string | null;
  /** Note pitch, when the cell holds a note. */
  cents: number | null;
  slot: number | null;
}

function inRow(section: Section, row: number, t: number): boolean {
  const a = rowStart(section, row);
  const b = rowStart(section, row + 1);
  return t >= a && t < b;
}

/** What one pattern cell shows. The earliest event in the row wins the label. */
export function cellView(section: Section, layer: Layer, row: number): CellView {
  const clip: Clip | undefined = section.clips[layer.id];
  if (!clip) return { label: '·', title: 'Empty. Double-click or press Enter to add.', eventId: null, cents: null, slot: null };
  if (clip.kind === 'beat') {
    const hit = clip.hits.find((h) => inRow(section, row, h.t));
    if (!hit) return { label: '·', title: 'Empty. Double-click or press Enter to add a hit.', eventId: null, cents: null, slot: null };
    const name = SLOT_NAMES[hit.slot] ?? `slot ${hit.slot + 1}`;
    return { label: name.slice(0, 4), title: `${name}, strength ${hit.strength.toFixed(2)}`, eventId: hit.id, cents: null, slot: hit.slot };
  }
  const note = clip.notes.find((n) => inRow(section, row, n.t));
  if (!note) return { label: '·', title: 'Empty. Double-click or press Enter to add a note.', eventId: null, cents: null, slot: null };
  const name = centsToName(note.cents);
  return { label: name, title: `${name}, ${note.dur} ${section.feel === 'grid' ? 'ticks' : 'seconds'}`, eventId: note.id, cents: note.cents, slot: null };
}

/** Beat.step readout, 1-based, for the transport strip. */
export function positionLabel(section: Section | undefined, pos: number): string {
  if (!section) return '—';
  if (section.feel === 'free') return `${pos.toFixed(2)}s`;
  const beat = Math.floor(pos / PPQ);
  const step = Math.floor((pos % PPQ) / (PPQ / STEPS_PER_BEAT));
  return `${beat + 1}.${step + 1}`;
}
