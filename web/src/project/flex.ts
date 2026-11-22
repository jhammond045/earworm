// Flex quantize at play time (docs/quant.md, stage Q1). A clip's flex setting turns its anchors
// (points of its take, or its note and hit starts) into a time map: each anchor is pulled toward
// the nearest slot of the bent grid by the strength, and everything the clip plays goes through
// the map. The stored notes and the take never change. Pure functions of the document, so the
// same document always compiles to the same track.
//
// Times are section units (ticks in a grid section, seconds in a free one) inside the clip's
// loop. The map is periodic with the loop: an anchor played just before the loop end can be
// pulled to the next pass's first slot, and its note then starts at the top of the loop.

import type { AutoLane, BeatClip, Clip, Flex, FlexFeel, Hit, MelodyClip, Note, WarpAnchor } from './types';
import { FLEX_STRENGTH } from './types';

/** Anchors closer than this to the one before them ride along instead of moving, so anchors never
 *  cross and every stretch between two of them is long enough for Q2's crossfade. */
export const MIN_GAP_SEC = 0.012;
/** Flag bits of a stored anchor (crates/analysis/src/anchors.rs). */
export const ANCHOR_HARD = 1;
export const ANCHOR_KIND_HIT = 2;
export const ANCHOR_LEN = 3;

export interface FlexAnchor {
  /** Performed clip time, section units, in [0, loopLen). */
  t: number;
  /** Of two anchors on one slot, the heavier takes it. */
  w: number;
  hard: boolean;
  /** Kit slot of a beat anchor, -1 none. */
  kit: number;
}

/** One anchor after quantizing: where it was played and where it now plays (section units). */
export interface Moved {
  src: number;
  dst: number;
  /** The slot it was pulled to (bent time), NaN for a rider. */
  slot: number;
  hard: boolean;
}

export interface Target {
  /** Slot size, section units. */
  grid: number;
  triplets: boolean;
  feel: FlexFeel;
  /** 0..1 at a slot (section units, inside the loop). */
  strength: (slot: number) => number;
  /** Kit slot factors (beat layers). */
  slotStrength?: number[];
  /** Anchors further than this from their slot ride along. */
  catch: number;
  /** Section units, see MIN_GAP_SEC. */
  minGap: number;
  loopLen: number;
}

/** Where a straight grid position `x` falls once the feel bends it. */
export function feelTime(feel: FlexFeel, x: number): number {
  if (feel.kind !== 'swing' || !(feel.unit > 0)) return x;
  const u = feel.unit;
  const slot = (k: number) => k * u + (((k % 2) + 2) % 2 === 1 ? (feel.amount - 0.5) * 2 * u : 0);
  const k = Math.floor(x / u);
  const f = x / u - k;
  return slot(k) + (slot(k + 1) - slot(k)) * f;
}

/** The candidate slots (bent times) nearest `t`: the straight slots around it, bent by the feel,
 *  and with triplets on, the triplet slots of the same size (not bent). */
function candidates(t: number, tg: Target): number[] {
  const out: number[] = [];
  const near = (step: number, bend: boolean) => {
    const k0 = Math.round(t / step);
    for (const k of [k0 - 1, k0, k0 + 1]) out.push(bend ? feelTime(tg.feel, k * step) : k * step);
  };
  near(tg.grid, true);
  if (tg.triplets) near((tg.grid * 2) / 3, false);
  return out;
}

const wrap = (x: number, len: number) => ((x % len) + len) % len;
/** A slot's identity within the loop, so a slot at the loop end is the same as the one at 0. */
const slotKey = (slot: number, len: number) => Math.round(wrap(slot, len) * 1e6);

/** Pull each anchor toward the nearest slot of the bent grid by the strength at that slot
 *  (docs/quant.md "Strength, catch and order"). Returns the moved anchors and the riders that fit
 *  between them, sorted by src, with dst increasing and every two at least `minGap` apart, also
 *  across the loop end. */
export function quantize(anchors: FlexAnchor[], tg: Target): Moved[] {
  const len = tg.loopLen;
  const picks = anchors
    .filter((a) => a.t >= 0 && a.t < len)
    .map((a) => {
      let best = Infinity;
      let slot = NaN;
      for (const c of candidates(a.t, tg)) {
        const d = Math.abs(c - a.t);
        if (d < best) { best = d; slot = c; }
      }
      return { a, slot: best <= tg.catch ? slot : NaN };
    })
    .sort((x, y) => x.a.t - y.a.t);
  // One anchor per slot: the heaviest keeps it (the earlier one on a tie).
  const owner = new Map<number, number>();
  picks.forEach((p, i) => {
    if (Number.isNaN(p.slot)) return;
    const k = slotKey(p.slot, len);
    const j = owner.get(k);
    if (j === undefined || p.a.w > picks[j].a.w) owner.set(k, i);
  });
  const out: Moved[] = [];
  const riders: FlexAnchor[] = [];
  picks.forEach((p, i) => {
    const { a, slot } = p;
    if (Number.isNaN(slot) || owner.get(slotKey(slot, len)) !== i) { riders.push(a); return; }
    const factor = a.kit >= 0 ? (tg.slotStrength?.[a.kit] ?? 1) : 1;
    const s = Math.min(1, Math.max(0, tg.strength(wrap(slot, len)) * factor));
    const dst = a.t + s * (slot - a.t);
    const prev = out[out.length - 1];
    if (prev && (dst < prev.dst + tg.minGap || a.t < prev.src + tg.minGap)) { riders.push(a); return; }
    out.push({ src: a.t, dst, slot, hard: a.hard });
  });
  // Across the loop end: the last must stay before the first of the next pass.
  while (out.length > 1 && (out[out.length - 1].dst + tg.minGap > out[0].dst + len || out[out.length - 1].src + tg.minGap > out[0].src + len)) {
    const m = out.pop()!;
    riders.push({ t: m.src, w: 0, hard: m.hard, kit: -1 });
  }
  // Riders keep their transient as a point of the map, on the line the moved anchors draw.
  const line = out.slice();
  for (const a of riders) {
    const dst = mapTime(line, len, a.t);
    let i = 0;
    while (i < out.length && out[i].src < a.t) i++;
    const prev = i > 0 ? out[i - 1] : out.length ? { src: out[out.length - 1].src - len, dst: out[out.length - 1].dst - len } : null;
    const next = i < out.length ? out[i] : out.length ? { src: out[0].src + len, dst: out[0].dst + len } : null;
    const okPrev = !prev || (a.t - prev.src >= tg.minGap && dst - prev.dst >= tg.minGap);
    const okNext = !next || (next.src - a.t >= tg.minGap && next.dst - dst >= tg.minGap);
    if (okPrev && okNext) out.splice(i, 0, { src: a.t, dst, slot: NaN, hard: a.hard });
  }
  return out;
}

/** Where performed time `t` plays: piecewise linear through the moved points, repeated every loop
 *  (point (s, d) also stands at (s + k len, d + k len)). No points: t itself. Increasing in t. */
export function mapTime(moved: Moved[], len: number, t: number): number {
  const n = moved.length;
  if (!n) return t;
  const k = Math.floor(t / len);
  const r = t - k * len;
  let i = 0;
  while (i < n && moved[i].src <= r) i++;
  const a = i > 0 ? moved[i - 1] : { src: moved[n - 1].src - len, dst: moved[n - 1].dst - len };
  const b = i < n ? moved[i] : { src: moved[0].src + len, dst: moved[0].dst + len };
  const span = b.src - a.src;
  const d = span > 0 ? a.dst + ((b.dst - a.dst) * (r - a.src)) / span : a.dst;
  return k * len + d;
}

/** The value of a lane at `t` (section units inside the loop), as the engine reads it
 *  (crates/klont/src/seq.rs `lane_value`). null: the lane has no point inside the loop. */
export function laneValue(lane: AutoLane, loopLen: number, t: number): number | null {
  const pts = lane.points.filter((p) => p.t < loopLen);
  if (!pts.length) return null;
  let k = 0;
  while (k < pts.length && pts[k].t <= t) k++;
  if (k === 0) return pts[0].v;
  if (k >= pts.length) return pts[pts.length - 1].v;
  const a = pts[k - 1], b = pts[k];
  const span = b.t - a.t;
  let x = span > 0 ? Math.min(1, Math.max(0, (t - a.t) / span)) : 1;
  if (a.shape === 'step') x = 0;
  else if (a.shape === 'ease') x = x * x * (3 - 2 * x);
  return x <= 0 ? a.v : a.v + (b.v - a.v) * x;
}

/** The target a clip's flex setting gives. `ups`: section units per second. */
export function targetOf(clip: Clip, flex: Flex, ups: number): Target {
  const lane = clip.auto?.find((l) => l.target === FLEX_STRENGTH);
  const strength = (slot: number) => (lane ? laneValue(lane, clip.loopLen, slot) : null) ?? flex.strength;
  return {
    grid: flex.grid, triplets: flex.triplets, feel: flex.feel, strength, slotStrength: flex.slotStrength,
    catch: flex.catch, minGap: MIN_GAP_SEC * ups, loopLen: clip.loopLen,
  };
}

/** Clip time of take second `sec`: through the warp anchor nearest in take time (each note keeps
 *  the placement it was given, even after the note was moved by hand), at the section's rate. */
function takeToClip(warp: WarpAnchor[], ups: number, len: number, sec: number): number {
  let best = warp[0];
  for (const a of warp) if (Math.abs(a.takeSec - sec) < Math.abs(best.takeSec - sec)) best = a;
  return wrap(best.clipT + (sec - best.takeSec) * ups, len);
}

/** The clip's anchors: the take's stored anchors placed on the clip, or, for a clip without a
 *  take, without stored anchors or without a placement, its note or hit starts. */
export function clipAnchors(clip: Clip, takeAnchors: Float32Array | null, ups: number): FlexAnchor[] {
  const warp = clip.kind === 'beat' ? (clip.warp ?? []) : clip.warp;
  if (clip.take && takeAnchors && warp.length) {
    const out: FlexAnchor[] = [];
    for (let i = 0; i + ANCHOR_LEN <= takeAnchors.length; i += ANCHOR_LEN) {
      const flags = takeAnchors[i + 2];
      const kind = (flags >> 1) & 3;
      out.push({
        t: takeToClip(warp, ups, clip.loopLen, takeAnchors[i]), w: takeAnchors[i + 1], hard: (flags & ANCHOR_HARD) !== 0,
        kit: clip.kind === 'beat' && kind === ANCHOR_KIND_HIT ? (flags >> 3) & 3 : -1,
      });
    }
    return out;
  }
  if (clip.kind === 'beat') return clip.hits.map((h) => ({ t: h.t, w: h.strength, hard: true, kit: h.slot }));
  return clip.notes.map((n) => ({ t: n.t, w: 0.5 + n.hard, hard: true, kit: -1 }));
}

/** Whether the clip's flex setting does anything in this section. Free sections have no grid; the
 *  feel that works there, another layer's timing, comes with Q3. */
export function flexActive(clip: Clip, grid: boolean): clip is Clip & { flex: Flex } {
  return !!clip.flex?.on && grid && clip.flex.grid > 0 && clip.loopLen > 0;
}

/** A clip as it plays: notes or hits through the time map, and (melody and texture) the warp map
 *  the curves are read through, so each note still reads its take from where it was sung. */
export interface Flexed {
  notes?: Note[];
  hits?: Hit[];
  warp?: WarpAnchor[];
  moved: Moved[];
}

/** The time map of a clip, and what it plays through it. `ups`: section units per second. */
export function flexClip(clip: Clip & { flex: Flex }, takeAnchors: Float32Array | null, ups: number): Flexed {
  const len = clip.loopLen;
  const moved = quantize(clipAnchors(clip, takeAnchors, ups), targetOf(clip, clip.flex, ups));
  const m = (t: number) => mapTime(moved, len, t);
  const place = <T extends { t: number; dur: number }>(x: T): T => {
    const a = m(x.t);
    return { ...x, t: wrap(a, len), dur: Math.max(0, m(x.t + x.dur) - a) };
  };
  if (clip.kind === 'beat') return { moved, hits: (clip as BeatClip).hits.map(place) };
  const c = clip as MelodyClip;
  return { moved, notes: c.notes.map(place), warp: moved.length ? flexWarp(c, moved, ups) : c.warp };
}

/** clip.warp read at clip time `t` (crates/klont/src/curve.rs `warp_to_take`). */
export function warpToTake(warp: WarpAnchor[], t: number, slope: number): number {
  const n = warp.length;
  if (n === 0) return t * slope;
  if (n === 1) return warp[0].takeSec + (t - warp[0].clipT) * slope;
  let i = 0;
  while (i < n && warp[i].clipT <= t) i++;
  i = Math.min(Math.max(i - 1, 0), n - 2);
  const a = warp[i], b = warp[i + 1];
  const w = b.clipT - a.clipT;
  const s = w > 0 ? (b.takeSec - a.takeSec) / w : slope;
  return a.takeSec + (t - a.clipT) * s;
}

/** The warp map of a flexed melody or texture clip: the old map read through the inverse of the
 *  time map, so a curve plays at the moved note what it played at the performed one. When the warp
 *  has two anchors per note (a take placed by its notes), it is rebuilt note by note, so a note
 *  pulled across the loop end reads its own take at the top of the loop; otherwise it is the old
 *  anchors and the time map's points, mapped. */
function flexWarp(c: MelodyClip, moved: Moved[], ups: number): WarpAnchor[] {
  const len = c.loopLen;
  const slope = 1 / ups;
  const m = (t: number) => mapTime(moved, len, t);
  const w = (t: number) => warpToTake(c.warp, t, slope);
  const out: WarpAnchor[] = [];
  /** The map's points with src in (a, b), in order. */
  const inside = (a: number, b: number) => {
    const pts: number[] = [];
    for (let k = Math.floor(a / len); k * len < b; k++) {
      for (const p of moved) {
        const s = p.src + k * len;
        if (s > a && s < b) pts.push(s);
      }
    }
    return pts;
  };
  if (c.notes.length && c.warp.length === 2 * c.notes.length) {
    for (const n of c.notes) {
      const a = m(n.t);
      const shift = wrap(a, len) - a;
      out.push({ takeSec: w(n.t), clipT: a + shift });
      for (const s of inside(n.t, n.t + n.dur)) out.push({ takeSec: w(s), clipT: m(s) + shift });
      out.push({ takeSec: w(n.t + n.dur), clipT: m(n.t + n.dur) + shift });
    }
  } else {
    for (const a of c.warp) out.push({ takeSec: a.takeSec, clipT: m(a.clipT) });
    for (const p of moved) out.push({ takeSec: w(p.src), clipT: p.dst });
  }
  return out.sort((a, b) => a.clipT - b.clipT || a.takeSec - b.takeSec);
}

/** Flex as the Quantize button turns it on: the grid at full strength, catching anchors half a
 *  slot away, keeping the clip's other choices when it already has some. */
export function quantizeFlex(prev: Flex | undefined, grid: number): Flex {
  return { triplets: false, feel: { kind: 'straight' }, ...prev, on: true, grid, strength: 1, catch: grid / 2 };
}

/** Check a flex setting from outside (a command's argument). Throws RangeError. */
export function checkFlex(f: Flex): Flex {
  const fin = (v: unknown, what: string) => {
    if (typeof v !== 'number' || !Number.isFinite(v)) throw new RangeError(`flex ${what} must be a number`);
    return v;
  };
  if (typeof f.on !== 'boolean' || typeof f.triplets !== 'boolean') throw new RangeError('flex on and triplets must be true or false');
  if (!(fin(f.grid, 'grid') > 0)) throw new RangeError('flex grid must be above 0');
  const s = fin(f.strength, 'strength');
  if (s < 0 || s > 1) throw new RangeError('flex strength must be from 0 to 1');
  if (fin(f.catch, 'catch') < 0) throw new RangeError('flex catch must be 0 or more');
  if (f.feel?.kind === 'swing') {
    const a = fin(f.feel.amount, 'swing amount');
    if (a < 0.5 || a > 0.75) throw new RangeError('swing amount must be from 0.5 to 0.75');
    if (!(fin(f.feel.unit, 'swing unit') > 0)) throw new RangeError('swing unit must be above 0');
  } else if (f.feel?.kind !== 'straight') throw new RangeError('flex feel must be straight or swing');
  if (f.slotStrength !== undefined) {
    if (!Array.isArray(f.slotStrength) || f.slotStrength.length > 4) throw new RangeError('flex slot strengths must be up to four numbers');
    for (const v of f.slotStrength) if (fin(v, 'slot strength') < 0 || v > 1) throw new RangeError('flex slot strengths must be from 0 to 1');
  }
  return f;
}
