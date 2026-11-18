import { describe, expect, it } from 'vitest';
import { CURVE_STRIDE } from '../m1/track';
import {
  TUNED, LOUD, centsToY, dragCents, dragDt, fitPpu, gridLines, hitGlyph, hitTestHits, hitTestNotes, newItemDur, noteRect,
  pitchRange, readFrame, resizeDur, ribbonRuns, slotRow, slotY, snapChoices, snapTo, timeToX, warpToTake, warpSlope, xToTime, yToCents,
  type Viewport,
} from './geometry';

const vp: Viewport = { ppu: 0.1, x0: 50 };
const range = { lo: 4800, hi: 7200 };

function frames(n: number, pitch: (i: number) => number, loud = 0.5): Float32Array {
  const f = new Float32Array(n * CURVE_STRIDE);
  for (let i = 0; i < n; i++) { f[i * CURVE_STRIDE + TUNED] = pitch(i); f[i * CURVE_STRIDE + LOUD] = loud; }
  return f;
}

describe('axes', () => {
  it('time and x are inverses', () => {
    for (const t of [0, 100, 960, 3840.5]) expect(xToTime(timeToX(t, vp), vp)).toBeCloseTo(t, 9);
    expect(timeToX(0, vp)).toBe(50);
  });
  it('cents and y are inverses, higher pitch is higher on screen (smaller y)', () => {
    expect(centsToY(7200, range, 10, 100)).toBe(10);
    expect(centsToY(4800, range, 10, 100)).toBe(110);
    for (const c of [4800, 5500, 7200]) expect(yToCents(centsToY(c, range, 10, 100), range, 10, 100)).toBeCloseTo(c, 9);
  });
  it('fitPpu fits the length into the width', () => {
    expect(fitPpu(962, 3840, 50, 1, 12)).toBeCloseTo(900 / 3840, 9);
    expect(fitPpu(962, 3840, 50, 2, 12)).toBeCloseTo(1800 / 3840, 9);
  });
  it('pitchRange is at least an octave and holds the values', () => {
    const r = pitchRange([6000, 6100]);
    expect(r.hi - r.lo).toBeGreaterThanOrEqual(1200);
    expect(r.lo).toBeLessThan(6000);
    expect(r.hi).toBeGreaterThan(6100);
    expect(pitchRange([NaN], range)).toBe(range);
  });
  it('snapTo rounds to the grid and does nothing when off', () => {
    expect(snapTo(130, 120)).toBe(120);
    expect(snapTo(190, 120)).toBe(240);
    expect(snapTo(130, 0)).toBe(130);
  });
});

describe('grid and snap', () => {
  it('grid lines: a beat each 960 ticks, a bar every 4', () => {
    const l = gridLines('grid', 3840);
    expect(l).toHaveLength(5);
    expect(l.filter((x) => x.strong).map((x) => x.t)).toEqual([0, 3840]);
    expect(gridLines('free', 2).map((x) => x.t)).toEqual([0, 1, 2]);
  });
  it('snap choices start with off and differ by feel', () => {
    expect(snapChoices('grid')[0]).toEqual({ label: 'Off', value: 0 });
    expect(snapChoices('grid').map((c) => c.value)).toEqual([0, 60, 120, 240, 960, 3840]);
    expect(snapChoices('free').every((c) => c.value < 2)).toBe(true);
  });
  it('new item length follows the snap', () => {
    expect(newItemDur('grid', 240)).toBe(240);
    expect(newItemDur('grid', 0)).toBe(480);
    expect(newItemDur('free', 0)).toBe(0.25);
  });
});

describe('warpToTake (same cases as curve.rs)', () => {
  const a = (takeSec: number, clipT: number) => ({ takeSec, clipT });
  it('no anchors, one anchor', () => {
    expect(warpToTake([], 480, 0.001)).toBeCloseTo(0.48, 12);
    expect(warpToTake([a(1, 960)], 1440, 0.002)).toBeCloseTo(1 + 480 * 0.002, 12);
    expect(warpToTake([a(1, 960)], 0, 0.002)).toBeCloseTo(1 - 960 * 0.002, 12);
  });
  it('piecewise linear, extends the end segments', () => {
    const v = [a(0, 0), a(1, 1000), a(2, 2000)];
    expect(warpToTake(v, 500, 9)).toBe(0.5);
    expect(warpToTake(v, 1000, 9)).toBe(1);
    const w = [a(0, 0), a(1, 1000), a(1.5, 2000)];
    expect(warpToTake(w, 1500, 9)).toBe(1.25);
    expect(warpToTake(w, -1000, 9)).toBe(-1);
    expect(warpToTake(w, 3000, 9)).toBe(2);
  });
  it('a zero-width segment uses the default slope', () => {
    expect(warpToTake([a(0, 10), a(1, 10)], 20, 0.5)).toBe(5);
    expect(warpSlope('grid', 120)).toBeCloseTo(60 / (120 * 960), 12);
    expect(warpSlope('free', 120)).toBe(1);
  });
});

describe('readFrame', () => {
  it('interpolates, and holds NaN gaps by the nearer frame', () => {
    const f = frames(4, (i) => 6000 + i * 100);
    expect(readFrame(f, 0.015, TUNED)).toBeCloseTo(6150, 3);
    expect(readFrame(f, -0.1, TUNED)).toBeNaN();
    expect(readFrame(f, 5, TUNED)).toBeNaN();
    const g = frames(3, (i) => (i === 1 ? NaN : 6000));
    expect(readFrame(g, 0.004, TUNED)).toBe(6000);
    expect(readFrame(g, 0.006, TUNED)).toBeNaN();
  });
});

describe('ribbonRuns', () => {
  const base = { vp: { ppu: 1, x0: 0 }, range, top: 0, h: 120, maxHalf: 10, slope: 0.01 };
  it('follows the pitch curve through an identity warp, width from loudness', () => {
    // 0.5 s of constant 6000 cents at loudness 0.5; one clip unit is 10 ms.
    const runs = ribbonRuns({ ...base, t: 0, dur: 40, anchors: [], frames: frames(50, () => 6000, 0.5) });
    expect(runs).toHaveLength(1);
    for (const p of runs[0]) {
      expect(p.y).toBeCloseTo(centsToY(6000, range, 0, 120), 6);
      expect(p.hw).toBeCloseTo(5, 6);
    }
    expect(runs[0][0].x).toBe(0);
    expect(runs[0].at(-1)!.x).toBe(40);
  });
  it('reads through warp anchors: the second half of the note plays the take at half speed', () => {
    // Take pitch rises 100 cents per frame. Anchor: clip 0 -> take 0 s, clip 100 -> take 0.1 s (slope 0.001 s per unit).
    const f = frames(30, (i) => 5000 + i * 100);
    const runs = ribbonRuns({ ...base, t: 0, dur: 100, anchors: [{ takeSec: 0, clipT: 0 }, { takeSec: 0.1, clipT: 100 }], frames: f });
    const last = runs[0].at(-1)!;
    expect(last.y).toBeCloseTo(centsToY(6000, range, 0, 120), 4); // take 0.1 s = frame 10 = 6000 cents
    const mid = runs[0].find((p) => p.x === 50)!;
    expect(mid.y).toBeCloseTo(centsToY(5500, range, 0, 120), 4);
  });
  it('an unvoiced gap splits the ribbon', () => {
    const f = frames(50, (i) => (i >= 20 && i < 30 ? NaN : 6000));
    const runs = ribbonRuns({ ...base, t: 0, dur: 40, anchors: [], frames: f });
    expect(runs).toHaveLength(2);
  });
  it('no frames give no ribbon', () => {
    expect(ribbonRuns({ ...base, t: 0, dur: 40, anchors: [], frames: new Float32Array(0) })).toEqual([]);
  });
});

describe('hit testing', () => {
  const notes = [
    { id: 'a', t: 0, dur: 1000, cents: 6000 },
    { id: 'b', t: 500, dur: 1000, cents: 6000 },
  ];
  const top = 0, h = 120;
  it('finds a note, topmost first, and its right edge', () => {
    const r = noteRect(notes[0], vp, range, top, h);
    const y = r.y + r.h / 2;
    expect(hitTestNotes(notes, r.x + 5, y, vp, range, top, h)).toEqual({ id: 'a', part: 'body' });
    // x of t=700 is inside both; b is later in the list so it is on top.
    expect(hitTestNotes(notes, timeToX(700, vp), y, vp, range, top, h)).toEqual({ id: 'b', part: 'body' });
    expect(hitTestNotes([notes[0]], r.x + r.w - 2, y, vp, range, top, h)).toEqual({ id: 'a', part: 'end' });
    expect(hitTestNotes(notes, r.x + 5, y - 40, vp, range, top, h)).toBeNull();
  });
  it('rows are four bands; clamps outside the lane', () => {
    expect(slotRow(5, 0, 120)).toBe(0);
    expect(slotRow(35, 0, 120)).toBe(1);
    expect(slotRow(119, 0, 120)).toBe(3);
    expect(slotRow(-50, 0, 120)).toBe(0);
    expect(slotRow(500, 0, 120)).toBe(3);
    expect(slotY(2, 0, 120)).toBe(75);
  });
  it('hit glyph: size from strength, tail from hold', () => {
    const mk = (strength: number, hold: number) => ({ id: 'x', t: 100, slot: 1, strength, shape: { hold } });
    const weak = hitGlyph(mk(0.1, 0), vp, 0, 120, 1600);
    const strong = hitGlyph(mk(1, 0.1), vp, 0, 120, 1600);
    expect(strong.r).toBeGreaterThan(weak.r);
    expect(weak.tailW).toBe(0);
    expect(strong.tailW).toBeCloseTo(0.1 * 1600 * vp.ppu, 9);
    expect(strong.cy).toBe(slotY(1, 0, 120));
  });
  it('hits are found on the disc and on the tail', () => {
    const hits = [{ id: 'h1', t: 100, slot: 1, strength: 0.8, shape: { hold: 0.1 } }];
    const g = hitGlyph(hits[0], vp, 0, 120, 1600);
    expect(hitTestHits(hits, g.cx, g.cy, vp, 0, 120, 1600)).toBe('h1');
    expect(hitTestHits(hits, g.cx + g.tailW - 1, g.cy, vp, 0, 120, 1600)).toBe('h1');
    expect(hitTestHits(hits, g.cx, g.cy + 40, vp, 0, 120, 1600)).toBeNull();
    expect(hitTestHits(hits, g.cx + g.tailW + 40, g.cy, vp, 0, 120, 1600)).toBeNull();
  });
});

describe('drag maths', () => {
  it('the primary item lands on the grid, free with Alt', () => {
    expect(dragDt(100, 130, 240)).toBe(140); // 230 -> 240
    expect(dragDt(100, 130, 240, true)).toBe(130);
    expect(dragDt(100, 130, 0)).toBe(130);
  });
  it('pitch lands on a semitone unless free', () => {
    expect(dragCents(6000, 130)).toBe(100);
    expect(dragCents(6040, 130)).toBe(160); // 6170 -> 6200
    expect(dragCents(6000, 130, true)).toBe(130);
  });
  it('resize snaps the end and keeps a minimum', () => {
    expect(resizeDur(0, 500, 130, 240, 1)).toBe(720); // end 630 -> 720
    expect(resizeDur(0, 500, -900, 240, 1)).toBe(1);
    expect(resizeDur(0, 500, 33, 240, 1, true)).toBe(533);
  });
});
