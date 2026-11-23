// Meter remap at play time (docs/meter.md, stage R1). A clip locked to a meter is cut into beat
// slices along that meter's bars and played in the order its section's pattern gives: `1 2 3`
// drops beat 4 of every 4/4 bar, `1 2 3 3` repeats beat 3 of a 3/4 bar, `1 3 4 2` shuffles.
// Everything runs after Feel (flex.ts): the cuts fall at the clip's hard anchors as Feel plays
// them, and notes, hits, the warp map the curves are read through and the take audio's stretch
// map are all built from the same runs. The document never changes, and a project without a
// pattern compiles exactly as before. Pure functions of the document.
//
// Times are section units (ticks; meters only act in grid sections). "Played time" is the clip's
// time after Feel, before the remap; "output time" is where the remapped clip plays it.

import type { Clip, Meter, Project, Section, WarpAnchor } from './types';
import { PPQ } from './types';
import { POINT_HARD, POINT_RUN, type TrackWarpAnchor } from '../m1/track';
import { warpToTake, type FlexAnchor } from './flex';

/** The id of the first meter, also when the document has no meters list yet. */
export const METER1 = 'meter1';
export const MIN_METER_BEATS = 2;
export const MAX_METER_BEATS = 16;
/** Longest pattern: a 16-beat bar played twice through. */
export const MAX_PATTERN = 32;
/** A cut moves to a hard anchor this close to its beat, at most (docs/meter.md "What moves"). */
export const CUT_BEATS = 1 / 8;
export const CUT_MAX_SEC = 0.06;
/** Of the anchors near a beat, those at least this heavy against the heaviest can take the cut. */
export const CUT_WEIGHT = 0.3;
/** A held note a run starts in the middle of plays again if this much of it is left (beats). */
export const RESTART_BEATS = 1 / 4;

/** The project's meters. Without a list: one typed meter, as many beats per bar as the click
 *  counts (4 by default), as the grid was before meters. The first one is Meter 1. */
export function metersOf(doc: Pick<Project, 'meters' | 'metro'>): Meter[] {
  if (doc.meters?.length) return doc.meters;
  return [{ id: METER1, name: 'Meter 1', bars: { from: 'typed', beats: doc.metro?.bar ?? 4 } }];
}

/** A meter by id, or undefined. */
export function meterById(doc: Pick<Project, 'meters' | 'metro'>, id: string | undefined): Meter | undefined {
  return id ? metersOf(doc).find((m) => m.id === id) : undefined;
}

/** The section's pattern for a meter, when it plays one: only grid sections have beats. */
export function patternOf(s: Section, meterId: string): number[] | undefined {
  if (s.feel !== 'grid') return undefined;
  const p = s.play?.[meterId];
  return p && p.length ? p : undefined;
}

/** One output beat: the source beat it plays (0-based from the clip or section start) and how
 *  long it is in section units (a whole beat, or the last part of one). */
export interface Slot { src: number; len: number }

/** The output beats of `len` section units cut into bars of `perBar` beats from the start, each
 *  bar played by the pattern (1-based source beats). A last part bar plays the pattern's beats
 *  that exist in it. */
export function playOrder(len: number, perBar: number, pattern: readonly number[], beat = PPQ): Slot[] {
  const nSrc = Math.max(0, Math.ceil(len / beat - 1e-9));
  const out: Slot[] = [];
  for (let bar = 0; bar * perBar < nSrc; bar++) {
    const inBar = Math.min(perBar, nSrc - bar * perBar);
    for (const p of pattern) {
      if (p < 1 || p > inBar) continue;
      const j = bar * perBar + p - 1;
      out.push({ src: j, len: Math.min(beat, len - j * beat) });
    }
  }
  return out;
}

/** How long `len` written section units play under the pattern. */
export function playedLength(len: number, perBar: number, pattern: readonly number[] | undefined, beat = PPQ): number {
  if (!pattern) return len;
  return playOrder(len, perBar, pattern, beat).reduce((n, s) => n + s.len, 0);
}

/** The section's played length in its own units: Meter 1's pattern sets it (docs/meter.md
 *  "Locking"); other meters' clips loop inside it. */
export function sectionPlayedLength(doc: Pick<Project, 'meters' | 'metro'>, s: Section): number {
  if (s.feel !== 'grid') return s.seconds;
  const m1 = metersOf(doc)[0];
  return playedLength(s.beats * PPQ, m1.bars.beats, patternOf(s, m1.id));
}

/** Beats in one of Meter 1's bars as the section plays them: the pattern's length, or the
 *  meter's beats per bar. The click accents these and the transport counts them. */
export function playedBar(doc: Pick<Project, 'meters' | 'metro'>, s: Section | undefined): number {
  const m1 = metersOf(doc)[0];
  const p = s ? patternOf(s, m1.id) : undefined;
  return p ? p.length : m1.bars.beats;
}

/** The written position (section units) that plays at played position `pos` of a section under
 *  Meter 1's pattern, for the playhead: it skips dropped beats and goes back for repeated ones. */
export function writtenAt(doc: Pick<Project, 'meters' | 'metro'>, s: Section, pos: number): number {
  const m1 = metersOf(doc)[0];
  const p = patternOf(s, m1.id);
  if (!p) return pos;
  const order = playOrder(s.beats * PPQ, m1.bars.beats, p);
  let o = 0;
  for (const sl of order) {
    if (pos < o + sl.len) return sl.src * PPQ + (pos - o);
    o += sl.len;
  }
  return s.beats * PPQ;
}

/** Check a pattern from outside (a command's argument). Throws RangeError. */
export function checkPattern(p: unknown, beats: number): number[] {
  if (!Array.isArray(p) || p.length < 1 || p.length > MAX_PATTERN) throw new RangeError(`a pattern is 1 to ${MAX_PATTERN} beats`);
  for (const x of p) {
    if (!Number.isInteger(x) || x < 1 || x > beats) throw new RangeError(`a pattern names beats 1 to ${beats}`);
  }
  return p as number[];
}

/** A pattern written as text, "1 3 4 2" or "1,3,4,2". null: not a pattern. */
export function parsePattern(text: string): number[] | null {
  const parts = text.trim().split(/[\s,]+/).filter(Boolean);
  if (!parts.length) return null;
  const out = parts.map((x) => Number(x));
  return out.every((x) => Number.isInteger(x)) ? out : null;
}

/** The common patterns for a bar of `beats` (docs/meter.md "Interface sketch"). */
export function presets(beats: number): { name: string; play: number[] | null }[] {
  const all = Array.from({ length: beats }, (_, i) => i + 1);
  const out: { name: string; play: number[] | null }[] = [
    { name: 'As written', play: null },
    { name: 'Drop last', play: all.slice(0, -1) },
    { name: 'Repeat last', play: [...all, beats] },
  ];
  if (beats >= 3) out.push({ name: `Shuffle ${[1, ...all.slice(2), 2].join(' ')}`, play: [1, ...all.slice(2), 2] });
  return out;
}

/** True when the pattern plays every beat once, in order: nothing to remap. */
export function asWritten(order: readonly Slot[], len: number, beat = PPQ): boolean {
  const nSrc = Math.ceil(len / beat - 1e-9);
  return order.length === nSrc && order.every((s, i) => s.src === i);
}

// ---------------------------------------------------------------- runs

/** One stretch of output that plays one stretch of the played clip, moved by `shift`, both
 *  folded into their loops. `run` numbers the run it belongs to; `join` is true where the piece
 *  starts a run (a cut), false where it only continues one across a loop end. */
export interface Piece { o0: number; o1: number; p0: number; p1: number; run: number; join: boolean }

/** A run in output time, unwrapped: output [a, b) plays played time [a - shift, b - shift). */
export interface Run { a: number; b: number; shift: number }

export interface Remap {
  /** The clip's loop as it plays. */
  loopLen: number;
  runs: Run[];
  pieces: Piece[];
}

export interface RemapInput {
  /** The clip's written loop, section units. */
  loopLen: number;
  /** Beats per bar of the meter. */
  perBar: number;
  pattern: readonly number[];
  /** Section units per second (for the 60 ms limit on a cut's move). */
  ups: number;
  /** The clip's anchors in played time (after Feel), inside [0, loopLen). Only hard ones cut. */
  anchors: readonly { t: number; w: number; hard: boolean }[];
}

/** Where the clip is cut at beat boundary `x` (played time): the earliest hard anchor within an
 *  eighth of a beat (60 ms at most) that weighs at least 0.3 of the heaviest there; else x. */
export function cutAt(anchors: RemapInput['anchors'], x: number, tol: number, len: number): number {
  let top = 0;
  const near: { t: number; w: number }[] = [];
  for (const a of anchors) {
    if (!a.hard) continue;
    for (const t of [a.t - len, a.t, a.t + len]) {
      if (Math.abs(t - x) <= tol) {
        near.push({ t, w: a.w });
        top = Math.max(top, a.w);
      }
    }
  }
  let best = Infinity;
  for (const a of near) if (a.w >= CUT_WEIGHT * top && a.t < best) best = a.t;
  return Number.isFinite(best) ? best : x;
}

/** The runs of a locked clip (docs/meter.md "What moves, and how"), or null when the pattern
 *  plays the clip as written. */
export function remapRuns(inp: RemapInput, beat = PPQ): Remap | null {
  const L = inp.loopLen;
  if (!(L > 0)) return null;
  const order = playOrder(L, inp.perBar, inp.pattern, beat);
  if (!order.length || asWritten(order, L, beat)) return null;
  const nSrc = Math.ceil(L / beat - 1e-9);
  const OL = order.reduce((n, s) => n + s.len, 0);
  const tol = Math.min(beat * CUT_BEATS, CUT_MAX_SEC * inp.ups);
  // Beat boundaries and their cuts in played time, unwrapped across loop passes: boundary j of
  // pass m is j + m * nSrc.
  const b0 = (j: number) => Math.min(j * beat, L);
  const cuts: number[] = [];
  for (let j = 0; j < nSrc; j++) cuts.push(cutAt(inp.anchors, b0(j), tol, L));
  // Keep the cuts in order (a last part beat shorter than two tolerances could swap them).
  for (let j = 1; j < nSrc; j++) if (cuts[j] <= cuts[j - 1]) cuts[j] = b0(j);
  const pass = (j: number) => Math.floor(j / nSrc);
  const B = (j: number) => b0(j - pass(j) * nSrc) + pass(j) * L;
  const cut = (j: number) => cuts[j - pass(j) * nSrc] + pass(j) * L;
  // Output beats, starting where a run starts (rotated when the loop end does not cut).
  const starts = order.map((s, k) => {
    const prev = order[(k - 1 + order.length) % order.length];
    return k === 0 ? !(prev.src === nSrc - 1 && s.src === 0) : prev.src + 1 !== s.src;
  });
  const first = starts.indexOf(true);
  if (first < 0) return null;
  const o: number[] = [];
  let acc = 0;
  for (const s of order) { o.push(acc); acc += s.len; }
  // Each run: its first output beat (unwrapped output position) and its unwrapped source beats.
  const runs: { k: number; o: number; j0: number; j1: number }[] = [];
  for (let i = 0; i < order.length; i++) {
    const k = (first + i) % order.length;
    const oo = o[k] + (first + i >= order.length ? OL : 0);
    if (starts[k]) runs.push({ k, o: oo, j0: order[k].src, j1: order[k].src + 1 });
    else {
      const r = runs[runs.length - 1];
      r.j1 += 1;
    }
  }
  // Nominal start and end of each run in output time, then the joins: where two runs meet,
  // the earlier ends where the later starts, or the later starts where the earlier ends,
  // whichever comes first (a late hit after an early one, or an early hit after a late one).
  const shifts = runs.map((r) => r.o - B(r.j0));
  const start = runs.map((r, i) => cut(r.j0) + shifts[i]);
  const end = runs.map((r, i) => cut(r.j1) + shifts[i]);
  const n = runs.length;
  const J: number[] = [];
  for (let i = 0; i < n; i++) {
    const nextStart = i + 1 < n ? start[i + 1] : start[0] + OL;
    J.push(Math.min(end[i], nextStart));
  }
  const out: Run[] = runs.map((_, i) => ({ a: i === 0 ? J[n - 1] - OL : J[i - 1], b: J[i], shift: shifts[i] }));
  return { loopLen: OL, runs: out, pieces: piecesOf(out, OL, L) };
}

/** Runs folded into the output loop and split where the played time crosses its loop end. */
function piecesOf(runs: Run[], OL: number, L: number): Piece[] {
  const out: Piece[] = [];
  runs.forEach((r, ri) => {
    // Split points: output multiples of OL and played multiples of L inside (a, b).
    const cutsAt = new Set<number>([r.a, r.b]);
    for (let k = Math.ceil(r.a / OL); k * OL < r.b; k++) if (k * OL > r.a) cutsAt.add(k * OL);
    for (let k = Math.ceil((r.a - r.shift) / L); k * L < r.b - r.shift; k++) if (k * L > r.a - r.shift) cutsAt.add(k * L + r.shift);
    const xs = [...cutsAt].sort((x, y) => x - y);
    for (let i = 0; i + 1 < xs.length; i++) {
      const a = xs[i], b = xs[i + 1];
      if (!(b - a > 1e-9)) continue;
      const mo = Math.floor((a + 1e-9) / OL);
      const p = a - r.shift;
      const mp = Math.floor((p + 1e-9) / L);
      out.push({ o0: a - mo * OL, o1: b - mo * OL, p0: p - mp * L, p1: p - mp * L + (b - a), run: ri, join: i === 0 });
    }
  });
  return out.sort((x, y) => x.o0 - y.o0);
}

// ---------------------------------------------------------------- what plays

const fold = (x: number, len: number) => {
  const r = x - Math.floor(x / len) * len;
  return r >= len ? 0 : r;
};

/** Notes or hits through the runs (docs/meter.md "Notes and hits"): each belongs to the run its
 *  played start falls in, is copied into every run that plays it (twice in a repeat), dropped with
 *  a dropped beat, and ends where its run ends. With `restart`, a held note that a run starts in
 *  the middle of plays again from the run's start if a quarter beat or more of it is left. */
export function remapEvents<T extends { t: number; dur: number }>(items: readonly T[], r: Remap, loopLen: number, restart: boolean, beat = PPQ): T[] {
  const L = loopLen;
  const out: T[] = [];
  for (const run of r.runs) {
    const s0 = run.a - run.shift, s1 = run.b - run.shift;
    for (const x of items) {
      for (let m = Math.ceil((s0 - x.t) / L); x.t + m * L < s1; m++) {
        const t = x.t + m * L + run.shift;
        if (t < run.a - 1e-9) continue;
        out.push({ ...x, t: fold(t, r.loopLen), dur: Math.max(0, Math.min(x.dur, run.b - t)) });
      }
      if (!restart) continue;
      // Copies of x that started before this run and are still sounding at its start.
      for (let m = Math.ceil((s0 - x.t - x.dur) / L); x.t + m * L < s0; m++) {
        const left = x.t + m * L + x.dur - s0;
        if (left >= beat * RESTART_BEATS) out.push({ ...x, t: fold(run.a, r.loopLen), dur: Math.min(left, run.b - run.a) });
      }
    }
  }
  return out.sort((a, b) => a.t - b.t);
}

/** The warp map the curves are read through, rebuilt per piece: each piece's anchors moved by its
 *  shift, and at each join two anchors at one clip time, the piece ending and the piece starting,
 *  so a note reads its take where that slice was sung (docs/meter.md "Curves"). `warp` maps played
 *  time to take seconds; `slope` is seconds per section unit. */
export function remapWarp(warp: readonly WarpAnchor[], r: Remap, slope: number): WarpAnchor[] {
  const w = (t: number) => warpToTake(warp as WarpAnchor[], t, slope);
  const out: WarpAnchor[] = [];
  for (const p of r.pieces) {
    out.push({ clipT: p.o0, takeSec: w(p.p0) });
    for (const a of warp) if (a.clipT > p.p0 && a.clipT < p.p1) out.push({ clipT: p.o0 + a.clipT - p.p0, takeSec: a.takeSec });
    out.push({ clipT: p.o1, takeSec: w(p.p1) });
  }
  return out;
}

/** The take audio's stretch map cut into the pieces (docs/meter.md "Take audio"): a hard run point
 *  where each piece starts, the base map's hard and soft points inside it, a hard point where it
 *  ends. `base` is the clip's stretch map in played time (stretchMap in flex.ts): runs of points
 *  folded into the written loop. Parts of a piece the base map does not cover stay silent, and
 *  parts shorter than 6 ms are left out. `minGap`: interior points this close to an end are
 *  dropped, as stretchMap does. */
export function remapStretch(base: readonly TrackWarpAnchor[], r: Remap, ups: number, minGap: number): TrackWarpAnchor[] {
  const runs: TrackWarpAnchor[][] = [];
  for (const p of base) {
    if ((p.flags ?? 0) & POINT_RUN || !runs.length) runs.push([p]);
    else runs[runs.length - 1].push(p);
  }
  const secAt = (run: TrackWarpAnchor[], t: number) => {
    let i = 0;
    while (i + 2 < run.length && run[i + 1].clipT <= t) i++;
    const a = run[i], b = run[i + 1] ?? run[i];
    return b.clipT > a.clipT ? a.takeSec + ((b.takeSec - a.takeSec) * (t - a.clipT)) / (b.clipT - a.clipT) : a.takeSec;
  };
  const out: TrackWarpAnchor[][] = [];
  for (const pc of r.pieces) {
    for (const run of runs) {
      if (run.length < 2) continue;
      const x0 = Math.max(pc.p0, run[0].clipT), x1 = Math.min(pc.p1, run[run.length - 1].clipT);
      if (x1 - x0 < 0.006 * ups) continue;
      const at = (x: number) => pc.o0 + (x - pc.p0);
      const pts: TrackWarpAnchor[] = [{ clipT: at(x0), takeSec: secAt(run, x0), flags: POINT_HARD | POINT_RUN }];
      for (const q of run) {
        if (q.clipT >= x0 + minGap && q.clipT <= x1 - minGap) pts.push({ clipT: at(q.clipT), takeSec: q.takeSec, flags: (q.flags ?? 0) & POINT_HARD });
      }
      pts.push({ clipT: at(x1), takeSec: secAt(run, x1), flags: POINT_HARD });
      out.push(pts);
    }
  }
  return out.sort((x, y) => x[0].clipT - y[0].clipT).flat();
}

/** Anchors in played time: the clip's anchors through Feel's time map, folded into the loop. */
export function playedAnchors(anchors: readonly FlexAnchor[], map: ((t: number) => number) | null, len: number): { t: number; w: number; hard: boolean }[] {
  return anchors.map((a) => ({ t: fold(map ? map(a.t) : a.t, len), w: a.w, hard: a.hard }));
}

/** A locked clip's meter and pattern in this section, or null when it plays as written. */
export function lockOf(doc: Pick<Project, 'meters' | 'metro'>, s: Section, c: Clip): { meter: Meter; pattern: number[] } | null {
  const meter = meterById(doc, c.meter);
  const pattern = meter ? patternOf(s, meter.id) : undefined;
  return meter && pattern ? { meter, pattern } : null;
}

