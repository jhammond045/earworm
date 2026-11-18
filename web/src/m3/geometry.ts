// Pure geometry for the M3 timeline: time <-> x, cents <-> y, hit testing, warp reading and
// ribbon sampling. No DOM and no canvas, so all of it is tested in geometry.test.ts.
//
// Units: section units are ticks (960 per beat) in a grid section and seconds in a free one.
// Pixels are CSS pixels inside one canvas. A lane is a horizontal band with a top and a height.

import { CURVE_STRIDE } from '../m1/track';
import { KIT_SLOTS, PPQ, type Feel } from '../project/types';

/** Index of the tuned pitch and of the loudness in a 25-float frame record. */
export const TUNED = 24;
export const LOUD = 2;
/** Frames per second in a take's frame blob (10 ms frames). */
export const FRAME_RATE = 100;

// ---------------------------------------------------------------- time and pitch axes

export interface Viewport {
  /** Pixels per section unit. */
  ppu: number;
  /** X of section time 0 (leaves room for lane labels). */
  x0: number;
}

export const timeToX = (t: number, v: Viewport): number => v.x0 + t * v.ppu;
export const xToTime = (x: number, v: Viewport): number => (x - v.x0) / v.ppu;

/** Pixels per unit that fit `len` units into `width` px (after `x0` and a right margin), times a zoom factor. */
export function fitPpu(width: number, len: number, x0: number, zoom = 1, margin = 12): number {
  return (Math.max(40, width - x0 - margin) * zoom) / Math.max(1e-9, len);
}

export interface Range { lo: number; hi: number }

export const centsToY = (c: number, r: Range, top: number, h: number): number => top + h - ((c - r.lo) / (r.hi - r.lo)) * h;
export const yToCents = (y: number, r: Range, top: number, h: number): number => r.lo + ((top + h - y) / h) * (r.hi - r.lo);

/** Cents range that holds the finite values with a margin, at least an octave tall. `prev` is returned when there are none. */
export function pitchRange(vals: Iterable<number>, prev?: Range): Range {
  let lo = Infinity, hi = -Infinity;
  for (const v of vals) if (Number.isFinite(v)) { lo = Math.min(lo, v); hi = Math.max(hi, v); }
  if (!(lo <= hi)) return prev ?? { lo: 4800, hi: 7200 };
  const mid = (lo + hi) / 2;
  const span = Math.max(1200, hi - lo + 600);
  return { lo: mid - span / 2, hi: mid + span / 2 };
}

/** Snap a time to a multiple of `snap` (0 or less: no snapping). */
export const snapTo = (t: number, snap: number): number => (snap > 0 ? Math.round(t / snap) * snap : t);

// ---------------------------------------------------------------- grid and snap choices

export interface GridLine { t: number; strong: boolean }

/** Beat and bar lines for a section (bars are 4 beats; a free section gets a line a second, strong every 4). */
export function gridLines(feel: Feel, len: number): GridLine[] {
  const step = feel === 'grid' ? PPQ : 1;
  const out: GridLine[] = [];
  for (let i = 0; i * step <= len + 1e-9; i++) out.push({ t: i * step, strong: i % 4 === 0 });
  return out;
}

export interface SnapChoice { label: string; value: number }

export function snapChoices(feel: Feel): SnapChoice[] {
  if (feel === 'grid') {
    return [
      { label: 'Off', value: 0 }, { label: '1/16 beat', value: PPQ / 16 }, { label: '1/8 beat', value: PPQ / 8 },
      { label: '1/4 beat', value: PPQ / 4 }, { label: 'Beat', value: PPQ }, { label: 'Bar', value: PPQ * 4 },
    ];
  }
  return [
    { label: 'Off', value: 0 }, { label: '0.05 s', value: 0.05 }, { label: '0.1 s', value: 0.1 },
    { label: '0.25 s', value: 0.25 }, { label: '0.5 s', value: 0.5 }, { label: '1 s', value: 1 },
  ];
}

/** Length of a new note or hit: one snap step if snapping, else half a beat or a quarter second. */
export function newItemDur(feel: Feel, snap: number): number {
  return snap > 0 ? snap : feel === 'grid' ? PPQ / 2 : 0.25;
}

// ---------------------------------------------------------------- warp and curve reading

export interface Anchor { takeSec: number; clipT: number }

/** Clip time -> take seconds (crates/klont/src/curve.rs warp_to_take). Piecewise linear between anchors sorted by
 *  clip time, nearest segment's slope outside them; one anchor uses `slope`; none returns clipT * slope.
 *  `slope` is take seconds per clip unit: 1 in a free section, 60 / (bpm * 960) in a grid one. */
export function warpToTake(anchors: readonly Anchor[], clipT: number, slope: number): number {
  const n = anchors.length;
  if (n === 0) return clipT * slope;
  if (n === 1) return anchors[0].takeSec + (clipT - anchors[0].clipT) * slope;
  // Last anchor at or before clipT, kept inside 0..n-2 so (i, i + 1) is a segment.
  let lo = 0, hi = n; // partition_point: first index whose clipT > clipT
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (anchors[mid].clipT <= clipT) lo = mid + 1; else hi = mid;
  }
  const i = Math.min(Math.max(lo - 1, 0), n - 2);
  const a = anchors[i], b = anchors[i + 1];
  const w = b.clipT - a.clipT;
  const s = w > 0 ? (b.takeSec - a.takeSec) / w : slope;
  return a.takeSec + (clipT - a.clipT) * s;
}

export const warpSlope = (feel: Feel, bpm: number): number => (feel === 'grid' ? 60 / (Math.max(1, bpm) * PPQ) : 1);

/** One field of a frame-record array at take second `sec`, linearly interpolated. Where one of the two neighbouring
 *  frames is NaN the nearer one decides (pitch gaps stay gaps). NaN outside the take. */
export function readFrame(frames: Float32Array, sec: number, slot: number): number {
  const n = Math.floor(frames.length / CURVE_STRIDE);
  if (n === 0 || !Number.isFinite(sec)) return NaN;
  const pos = sec * FRAME_RATE;
  if (pos < 0 || pos > n - 1) return NaN;
  const i = Math.min(n - 2, Math.floor(pos));
  if (n === 1) return frames[slot];
  const f = pos - i;
  const a = frames[i * CURVE_STRIDE + slot], b = frames[(i + 1) * CURVE_STRIDE + slot];
  if (Number.isFinite(a) && Number.isFinite(b)) return a + (b - a) * f;
  return f < 0.5 ? a : b;
}

export interface RibbonPoint { x: number; y: number; hw: number }

export interface RibbonArgs {
  /** The note's clip time span. */
  t: number; dur: number;
  anchors: readonly Anchor[];
  slope: number;
  /** Frame records with the tuned slot filled (and the octave applied). */
  frames: Float32Array;
  vp: Viewport;
  range: Range;
  top: number; h: number;
  /** Half-width in px at loudness 1. */
  maxHalf: number;
  /** Sampling step in px. */
  stepPx?: number;
}

/** The take's tuned pitch curve read through the warp anchors over one note: centre = pitch, half-width = loudness.
 *  Each run is a stretch of voiced frames; an unvoiced gap ends a run. */
export function ribbonRuns(a: RibbonArgs): RibbonPoint[][] {
  const step = a.stepPx ?? 2;
  const x1 = timeToX(a.t + a.dur, a.vp);
  const runs: RibbonPoint[][] = [];
  let run: RibbonPoint[] = [];
  const x0 = timeToX(a.t, a.vp);
  for (let x = x0; ; x = Math.min(x + step, x1)) {
    const t = xToTime(x, a.vp);
    const sec = warpToTake(a.anchors, t, a.slope);
    const c = readFrame(a.frames, sec, TUNED);
    if (Number.isFinite(c)) {
      const loud = readFrame(a.frames, sec, LOUD);
      const l = Number.isFinite(loud) ? Math.max(0, Math.min(1, loud)) : 0;
      run.push({ x, y: centsToY(c, a.range, a.top, a.h), hw: Math.max(0.75, a.maxHalf * l) });
    } else if (run.length) {
      runs.push(run);
      run = [];
    }
    if (x >= x1) break;
  }
  if (run.length) runs.push(run);
  return runs;
}

// ---------------------------------------------------------------- notes: boxes and hit testing

export interface Rect { x: number; y: number; w: number; h: number }

export const NOTE_BAR_H = 8;
/** Width of the right-edge resize zone, px. */
export const EDGE_PX = 6;

export function noteRect(n: { t: number; dur: number; cents: number }, vp: Viewport, range: Range, top: number, h: number): Rect {
  const x = timeToX(n.t, vp);
  const w = Math.max(3, n.dur * vp.ppu);
  return { x, y: centsToY(n.cents, range, top, h) - NOTE_BAR_H / 2, w, h: NOTE_BAR_H };
}

export type NoteHit = { id: string; part: 'body' | 'end' };

/** The note under (px, py), topmost (last drawn) first. A few pixels of slack make the thin bars easy to grab. */
export function hitTestNotes(notes: readonly { id: string; t: number; dur: number; cents: number }[], px: number, py: number,
                             vp: Viewport, range: Range, top: number, h: number, slack = 4): NoteHit | null {
  for (let i = notes.length - 1; i >= 0; i--) {
    const r = noteRect(notes[i], vp, range, top, h);
    if (px < r.x - slack || px > r.x + r.w + slack || py < r.y - slack || py > r.y + r.h + slack) continue;
    const edge = Math.min(EDGE_PX, r.w / 2);
    return { id: notes[i].id, part: px >= r.x + r.w - edge ? 'end' : 'body' };
  }
  return null;
}

// ---------------------------------------------------------------- hits: slot rows and glyphs

export const slotRowH = (laneH: number): number => laneH / KIT_SLOTS;
export const slotY = (slot: number, top: number, h: number): number => top + (slot + 0.5) * slotRowH(h);
/** Row (0 to 3) under py, clamped. */
export function slotRow(py: number, top: number, h: number): number {
  return Math.max(0, Math.min(KIT_SLOTS - 1, Math.floor((py - top) / slotRowH(h))));
}

export interface HitGlyph { cx: number; cy: number; r: number; tailW: number }

/** A hit is a disc (radius from strength) with a tail to the right (length from hold, in seconds). */
export function hitGlyph(hit: { t: number; slot: number; strength: number; shape: { hold: number } }, vp: Viewport, top: number, h: number,
                        unitsPerSec: number): HitGlyph {
  const rowH = slotRowH(h);
  const r = Math.max(3, Math.min(rowH * 0.45, 3 + 8 * Math.max(0, Math.min(1, hit.strength))));
  const hold = Number.isFinite(hit.shape.hold) ? Math.max(0, hit.shape.hold) : 0;
  return { cx: timeToX(hit.t, vp), cy: slotY(hit.slot, top, h), r, tailW: hold * unitsPerSec * vp.ppu };
}

export function hitTestHits(hits: readonly { id: string; t: number; slot: number; strength: number; shape: { hold: number } }[],
                            px: number, py: number, vp: Viewport, top: number, h: number, unitsPerSec: number, slack = 3): string | null {
  for (let i = hits.length - 1; i >= 0; i--) {
    const g = hitGlyph(hits[i], vp, top, h, unitsPerSec);
    const inDisc = Math.hypot(px - g.cx, py - g.cy) <= g.r + slack;
    const inTail = px >= g.cx && px <= g.cx + g.tailW && Math.abs(py - g.cy) <= 3 + slack;
    if (inDisc || inTail) return hits[i].id;
  }
  return null;
}

// ---------------------------------------------------------------- drag maths

/** Time shift for a drag: the primary item lands on the snap grid (unless snap is 0 or `free`), the rest follow. */
export function dragDt(primaryT: number, rawDt: number, snap: number, free = false): number {
  return free || snap <= 0 ? rawDt : snapTo(primaryT + rawDt, snap) - primaryT;
}

/** Pitch shift for a drag: the primary note lands on a whole semitone (100 cents) unless `free`. */
export function dragCents(primaryCents: number, rawDc: number, free = false): number {
  return free ? rawDc : Math.round((primaryCents + rawDc) / 100) * 100 - primaryCents;
}

/** New duration for a right-edge drag: the end lands on the snap grid, never below `minDur`. */
export function resizeDur(t: number, dur: number, rawDt: number, snap: number, minDur: number, free = false): number {
  const end = t + dur + rawDt;
  const snapped = free || snap <= 0 ? end : snapTo(end, snap);
  return Math.max(minDur, snapped - t);
}

// ---------------------------------------------------------------- morph lane

/** Height of the morph strip under each lane, px. */
export const MORPH_H = 18;
/** Radius of a marker glyph, px. */
export const MARKER_R = 6;

/** One colour per state index (a layer has at most 8 states). Chosen to stay apart on both light and dark pages. */
export const STATE_COLORS = ['#7a6df0', '#e2874a', '#34a98f', '#d84f7e', '#6aa84f', '#c9a227', '#3d8fd1', '#9a6b4f'];
export const stateColor = (index: number): string => STATE_COLORS[((index % STATE_COLORS.length) + STATE_COLORS.length) % STATE_COLORS.length];

export type MorphShapeName = 'line' | 'ease' | 'late' | 'step' | 'wobble';

/** The shaped progress of a transition, x in 0..1 (crates/klont/src/seq.rs, "Morph lane"). Step is 1 at once,
 *  also at x = 0 (a step marker has no ramp). */
export function shapeProgress(shape: MorphShapeName, x: number): number {
  const c = Math.max(0, Math.min(1, x));
  switch (shape) {
    case 'ease': return c * c * (3 - 2 * c);
    case 'late': return c * c * c;
    case 'step': return 1;
    case 'wobble': return Math.max(0, Math.min(1, c + 0.12 * Math.sin(6 * Math.PI * c) * (1 - c)));
    default: return c;
  }
}

export interface MorphMark { t: number; lenMs: number; shape: MorphShapeName }

/** Where a clip's morph lane is at position `pos` (section units, inside the loop): the marker in force,
 *  the one before it (the lane wraps with the loop), and the shaped progress from the older state to the newer.
 *  Same rules as the engine, so the strip draws what is heard. `unitsPerSec` turns the marker's length in ms
 *  into section units. No markers: null. One marker: that marker with progress 1. */
export function morphAt(markers: readonly MorphMark[], pos: number, unitsPerSec: number): { k: number; prev: number; x: number } | null {
  const n = markers.length;
  if (n === 0) return null;
  let k = -1;
  for (let i = 0; i < n; i++) if (markers[i].t <= pos) k = i;
  const wrapped = k < 0;
  if (wrapped) k = n - 1;
  const prev = n === 1 ? k : (k + n - 1) % n;
  if (n === 1) return { k, prev, x: 1 };
  const m = markers[k];
  // Before the first marker the last one is still in force: the loop is a whole lap behind it, so its transition is done.
  const since = wrapped ? Infinity : ((pos - m.t) / unitsPerSec) * 1000;
  const raw = m.lenMs <= 0 || m.shape === 'step' ? 1 : Math.max(0, Math.min(1, since / m.lenMs));
  return { k, prev, x: shapeProgress(m.shape, raw) };
}

/** The marker whose glyph is under px (and inside the strip at py), nearest first, or -1. */
export function hitTestMarkers(markers: readonly { t: number }[], px: number, py: number, vp: Viewport, top: number, h = MORPH_H, slack = 3): number {
  if (py < top || py > top + h) return -1;
  let best = -1, bestD = Infinity;
  markers.forEach((m, i) => {
    const d = Math.abs(px - timeToX(m.t, vp));
    if (d <= MARKER_R + slack && d < bestD) { best = i; bestD = d; }
  });
  return best;
}

/** A dragged marker's new time: snapped (unless `free`), whole ticks in a grid section, and inside the loop
 *  (0 up to just under `loopLen`, as the document requires). */
export function dragMarkerT(t: number, rawDt: number, snap: number, loopLen: number, grid: boolean, free = false): number {
  const want = free || snap <= 0 ? t + rawDt : snapTo(t + rawDt, snap);
  const top = grid ? loopLen - 1 : loopLen - 0.001;
  const v = Math.max(0, Math.min(top, want));
  return grid ? Math.round(v) : Math.round(v * 1000) / 1000;
}

/** Linear blend of two #rrggbb colours, w in 0..1. */
export function mixColor(a: string, b: string, w: number): string {
  const pa = parseInt(a.slice(1), 16), pb = parseInt(b.slice(1), 16);
  const ch = (s: number) => Math.round(((pa >> s) & 255) * (1 - w) + ((pb >> s) & 255) * w);
  return `rgb(${ch(16)},${ch(8)},${ch(0)})`;
}
