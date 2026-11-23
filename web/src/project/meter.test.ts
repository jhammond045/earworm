// Meter remap (docs/meter.md, R1): the order beats play in, where the clip is cut, the runs, and
// what notes and hits do through them.
import { describe, expect, it } from 'vitest';
import { POINT_HARD, POINT_RUN } from '../m1/track';
import {
  METER1, checkPattern, cutAt, metersOf, parsePattern, playOrder, playedBar, playedLength, presets, remapEvents, remapRuns,
  remapStretch, remapWarp, sectionLabel, sectionPlayedLength, writtenAt, type Remap,
} from './meter';
import { warpToTake } from './flex';
import { PPQ, type Section } from './types';

const B = PPQ;
const UPS = (100 * PPQ) / 60; // 100 bpm
const none: { t: number; w: number; hard: boolean }[] = [];
const runs = (len: number, perBar: number, pattern: number[], anchors = none) =>
  remapRuns({ loopLen: len, perBar, pattern, ups: UPS, anchors })!;
const note = (t: number, dur: number, id = 'n') => ({ id, t, dur, cents: 4800, hard: 0.5 });

describe('play order', () => {
  it('drops, repeats, shuffles and plays past the bar', () => {
    expect(playOrder(8 * B, 4, [1, 2, 3]).map((s) => s.src)).toEqual([0, 1, 2, 4, 5, 6]);
    expect(playOrder(6 * B, 3, [1, 2, 3, 3]).map((s) => s.src)).toEqual([0, 1, 2, 2, 3, 4, 5, 5]);
    expect(playOrder(4 * B, 4, [1, 3, 4, 2]).map((s) => s.src)).toEqual([0, 2, 3, 1]);
    expect(playOrder(4 * B, 4, [1, 2, 3, 4, 4]).map((s) => s.src)).toEqual([0, 1, 2, 3, 3]);
  });

  it('a last part bar plays the pattern beats that exist in it', () => {
    // 10 beats of 4/4: bars of 4, 4 and 2.
    expect(playOrder(10 * B, 4, [1, 3, 4, 2]).map((s) => s.src)).toEqual([0, 2, 3, 1, 4, 6, 7, 5, 8, 9]);
    expect(playedLength(10 * B, 4, [1, 2, 3])).toBe(8 * B);
    // A part beat at the end keeps its length.
    expect(playOrder(4.5 * B, 4, [1, 2, 3]).map((s) => [s.src, s.len])).toEqual([[0, B], [1, B], [2, B], [4, B / 2]]);
  });

  it('Meter 1 sets the section played length and the played bar; others do not', () => {
    const doc = { meters: undefined, metro: undefined };
    const s = { feel: 'grid', beats: 16, seconds: 8, play: { [METER1]: [1, 2, 3] } } as unknown as Section;
    expect(metersOf(doc)[0]).toEqual({ id: METER1, name: 'Meter 1', bars: { from: 'typed', beats: 4 } });
    expect(sectionPlayedLength(doc, s)).toBe(12 * B);
    expect(playedBar(doc, s)).toBe(3);
    expect(sectionPlayedLength(doc, { ...s, play: { other: [1, 2, 3] } })).toBe(16 * B);
    expect(sectionPlayedLength(doc, { ...s, feel: 'free' })).toBe(8);
    // The section stepper shows what plays, and the written length when they differ.
    expect(sectionLabel(doc, { ...s, name: 'A', beats: 4 })).toBe('A 3 beats (4 written)');
    expect(sectionLabel(doc, { ...s, name: 'A', play: {} })).toBe('A 16 beats');
    expect(sectionLabel(doc, { ...s, name: 'A', feel: 'free' })).toBe('A 8s');
    // The playhead: played beat 3 (0-based) is written beat 4, the next bar's first.
    expect(writtenAt(doc, s, 3 * B + 10)).toBe(4 * B + 10);
  });

  it('checks and reads patterns', () => {
    expect(checkPattern([1, 3, 4, 2], 4)).toEqual([1, 3, 4, 2]);
    expect(() => checkPattern([1, 5], 4)).toThrow(/1 to 4/);
    expect(() => checkPattern([], 4)).toThrow(RangeError);
    expect(() => checkPattern([1.5], 4)).toThrow(RangeError);
    expect(parsePattern(' 1 3,4  2 ')).toEqual([1, 3, 4, 2]);
    expect(parsePattern('1 x')).toBeNull();
    expect(presets(4).map((p) => p.play)).toEqual([null, [1, 2, 3], [1, 2, 3, 4, 4], [1, 3, 4, 2]]);
  });
});

describe('cuts', () => {
  const tol = B / 8;
  it('at the earliest hard anchor near the beat that weighs at least 0.3 of the heaviest', () => {
    const an = [
      { t: 4 * B - 60, w: 0.1, hard: true }, // too light against 1.0
      { t: 4 * B - 40, w: 0.5, hard: true }, // the cut
      { t: 4 * B + 10, w: 1.0, hard: true },
      { t: 4 * B - 100, w: 1.0, hard: false }, // soft anchors never cut
    ];
    expect(cutAt(an, 4 * B, tol, 8 * B)).toBe(4 * B - 40);
    // Nothing near: the beat itself.
    expect(cutAt(an, 2 * B, tol, 8 * B)).toBe(2 * B);
    // An anchor just before the loop end cuts beat 1 of the next pass.
    expect(cutAt([{ t: 8 * B - 20, w: 1, hard: true }], 0, tol, 8 * B)).toBe(-20);
  });

  it('no further than 60 ms at a slow tempo', () => {
    const ups = (40 * PPQ) / 60; // 40 bpm: an eighth of a beat is 188 ms
    const an = [{ t: 4 * B - 0.1 * ups, w: 1, hard: true }];
    expect(cutAt(an, 4 * B, Math.min(B / 8, 0.06 * ups), 8 * B)).toBe(4 * B);
  });
});

/** Every run in output order, folded: [output start, output end, played start]. */
const shape = (r: Remap) => r.pieces.map((p) => [p.o0, p.o1, p.p0]);

describe('runs', () => {
  it('as written is no remap', () => {
    expect(remapRuns({ loopLen: 8 * B, perBar: 4, pattern: [1, 2, 3, 4], ups: UPS, anchors: none })).toBeNull();
  });

  it('a drop: one run per bar, beat 4 never played', () => {
    const r = runs(8 * B, 4, [1, 2, 3]);
    expect(r.loopLen).toBe(6 * B);
    expect(shape(r)).toEqual([[0, 3 * B, 0], [3 * B, 6 * B, 4 * B]]);
  });

  it('a repeat and a pattern longer than the bar', () => {
    const r = runs(3 * B, 3, [1, 2, 3, 3]);
    expect(r.loopLen).toBe(4 * B);
    expect(shape(r)).toEqual([[0, 3 * B, 0], [3 * B, 4 * B, 2 * B]]);
    expect(shape(runs(4 * B, 4, [1, 2, 3, 4, 4]))).toEqual([[0, 4 * B, 0], [4 * B, 5 * B, 3 * B]]);
  });

  it('a shuffle: three runs per bar', () => {
    const r = runs(4 * B, 4, [1, 3, 4, 2]);
    expect(shape(r)).toEqual([[0, B, 0], [B, 3 * B, 2 * B], [3 * B, 4 * B, B]]);
    expect(r.pieces.every((p) => p.join)).toBe(true);
  });

  it('cuts at anchors keep an early hit with its beat', () => {
    // Beat 5 (index 4) is hit 30 ticks early; beat 4 (index 3) is dropped.
    const r = runs(8 * B, 4, [1, 2, 3], [{ t: 4 * B - 30, w: 1, hard: true }]);
    expect(shape(r)).toEqual([[0, 3 * B - 30, 0], [3 * B - 30, 6 * B, 4 * B - 30]]);
  });

  it('a late run start trims the run before; an early one after a late end starts the next earlier', () => {
    // Shuffle 1 3 4 2 over two bars. Index 4 is hit 50 ticks late and index 1 30 ticks early.
    const an = [{ t: 4 * B + 50, w: 1, hard: true }, { t: B - 30, w: 1, hard: true }];
    const r = runs(8 * B, 4, [1, 3, 4, 2], an);
    // Run "1" (index 0) ends at index 1's early cut, before run "3 4" (index 2, no anchor) would
    // start: rather than leave a gap, the later run starts where the earlier one ends.
    const first = r.pieces.find((p) => p.p0 === 0)!;
    expect(first.o1).toBe(B - 30);
    const second = r.pieces.find((p) => p.o0 === B - 30)!;
    expect(second.p0).toBe(2 * B - 30);
    // Run "3 4" would end at index 4's late cut, after run "2" starts at index 1's early one:
    // the earlier run ends where the later starts.
    expect(second.o1).toBe(3 * B - 30);
    // Runs tile the loop: no gaps, no overlaps.
    const sorted = [...r.pieces].sort((a, b) => a.o0 - b.o0);
    sorted.forEach((p, i) => { if (i) expect(p.o0).toBeCloseTo(sorted[i - 1].o1, 9); });
    expect(sorted[0].o0).toBe(0);
    expect(sorted[sorted.length - 1].o1).toBe(r.loopLen);
  });

  it('a cut before the loop start folds into the loop end', () => {
    // Beat 1 hit 20 ticks early: the first run starts in the previous pass.
    const r = runs(4 * B, 4, [1, 2, 3], [{ t: 4 * B - 20, w: 1, hard: true }]);
    const pieces = [...r.pieces].sort((a, b) => a.o0 - b.o0);
    const total = pieces.reduce((n, p) => n + p.o1 - p.o0, 0);
    expect(total).toBeCloseTo(3 * B, 9);
    // The last 20 ticks of the output loop play the last 20 ticks of the clip: the early hit.
    expect(pieces[pieces.length - 1]).toMatchObject({ o1: 3 * B, p0: 4 * B - 20 });
  });
});

describe('notes and hits through the runs', () => {
  it('copied in a repeat, dropped with a dropped beat, ended at their run', () => {
    // 3/4, pattern 1 2 3 3: a note in beat 3 plays twice.
    const r = runs(3 * B, 3, [1, 2, 3, 3]);
    const out = remapEvents([note(2 * B + 100, 200)], r, 3 * B, true);
    expect(out.map((n) => n.t)).toEqual([2 * B + 100, 3 * B + 100]);
    // 4/4, 1 2 3: a note on beat 4 is dropped; one held from beat 3 into beat 4 ends at the join.
    const d = runs(4 * B, 4, [1, 2, 3]);
    expect(remapEvents([note(3 * B + 10, 100)], d, 4 * B, true)).toEqual([]);
    expect(remapEvents([note(2 * B, 2 * B)], d, 4 * B, true)).toEqual([{ ...note(2 * B, B) }]);
  });

  it('a held note a run starts in the middle of starts again if a quarter beat is left', () => {
    // 1 3 4 2: a note from beat 1 held for a beat and a half. Run "3 4" starts after it has
    // ended; run "2" (index 1) starts in its middle, with half a beat of it left.
    const r = runs(4 * B, 4, [1, 3, 4, 2]);
    const out = remapEvents([note(0, 1.5 * B)], r, 4 * B, true);
    // Its own copy, ended at run "1"'s end, and a restart where run "2" (index 1) begins, half a beat long.
    expect(out.map((n) => [n.t, n.dur])).toEqual([[0, B], [3 * B, 0.5 * B]]);
    // Less than a quarter beat left: not started again.
    expect(remapEvents([note(0, 1.2 * B)], r, 4 * B, true).map((n) => n.t)).toEqual([0]);
    // Hits never start again.
    expect(remapEvents([note(0, 1.5 * B)], r, 4 * B, false).map((n) => n.t)).toEqual([0]);
  });

  it('a note played just before its beat moves with that beat', () => {
    const an = [{ t: 2 * B - 25, w: 1, hard: true }];
    const r = runs(4 * B, 4, [1, 3, 4, 2], an);
    // The note 25 ticks before beat 3 (index 2) plays 25 ticks before output beat 2 (index 1).
    expect(remapEvents([note(2 * B - 25, 50)], r, 4 * B, true).map((n) => n.t)).toEqual([B - 25]);
  });
});

describe('maps', () => {
  it('the warp map reads each copy where it was sung, with two anchors at each join', () => {
    const r = runs(3 * B, 3, [1, 2, 3, 3]);
    const warp = [{ clipT: 0, takeSec: 1 }, { clipT: 3 * B, takeSec: 1 + (3 * B) / UPS }];
    const out = remapWarp(warp, r, 1 / UPS);
    const at = (t: number) => warpToTake(out, t, 1 / UPS);
    expect(at(2 * B + 100)).toBeCloseTo(at(3 * B + 100), 9);
    expect(at(3 * B)).toBeCloseTo(1 + (2 * B) / UPS, 9);
    expect(out.filter((a) => a.clipT === 3 * B)).toHaveLength(2);
  });

  it('the stretch map: a hard run point at each piece, the inside points moved, uncovered parts left out', () => {
    const r = runs(4 * B, 4, [1, 3, 4, 2]);
    const s = (sec: number) => sec;
    const base = [
      { clipT: 0, takeSec: s(0), flags: POINT_HARD | POINT_RUN },
      { clipT: 2.5 * B, takeSec: s(2.5 * B / UPS), flags: 0 },
      { clipT: 4 * B, takeSec: s(4 * B / UPS), flags: POINT_HARD },
    ];
    const out = remapStretch(base, r, UPS, 0.012 * UPS);
    const runStarts = out.filter((p) => p.flags! & POINT_RUN).map((p) => p.clipT);
    expect(runStarts).toEqual([0, B, 3 * B]);
    // The soft point at 2.5 beats plays at 1.5 beats (run "3 4" starts at output beat 1).
    expect(out.find((p) => p.flags === 0)).toMatchObject({ clipT: 1.5 * B });
    // A base map that covers only the first beat leaves the rest silent.
    const short = remapStretch([base[0], { clipT: B, takeSec: B / UPS, flags: POINT_HARD }], r, UPS, 0.012 * UPS);
    expect(short.map((p) => p.clipT)).toEqual([0, B]);
  });
});
