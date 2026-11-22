import { describe, expect, it } from 'vitest';
import { checkFlex, clipAnchors, feelTime, flexClip, laneValue, mapTime, quantize, quantizeFlex, targetOf, warpToTake, type FlexAnchor, type Target } from './flex';
import type { BeatClip, Flex, MelodyClip } from './types';

// A grid section at 120 bpm: 960 ticks a beat, 1920 ticks a second; 16ths are 240 ticks.
const UPS = 1920;
const BAR = 3840;
const a = (t: number, w = 1, kit = -1): FlexAnchor => ({ t, w, hard: true, kit });
const target = (o: Partial<Target> = {}): Target => ({
  grid: 240, triplets: false, feel: { kind: 'straight' }, strength: () => 1, catch: 120, minGap: 0.012 * UPS, loopLen: BAR, ...o,
});
const dsts = (anchors: FlexAnchor[], tg: Target) => quantize(anchors, tg).map((m) => [m.src, m.dst]);

describe('quantize', () => {
  it('pulls each anchor to the nearest slot of the bent grid', () => {
    expect(dsts([a(250), a(470), a(1000)], target())).toEqual([[250, 240], [470, 480], [1000, 960]]);
    // 16ths swung 66%: the second 16th of each 8th sits at 0.66 of the pair (317 ticks, not 240).
    const swing = target({ feel: { kind: 'swing', amount: 0.66, unit: 240 } });
    expect(feelTime(swing.feel, 240)).toBeCloseTo(316.8, 6);
    // An anchor at 300 goes to the swung slot, not the straight 16th it is also near.
    expect(dsts([a(300)], swing)[0][1]).toBeCloseTo(316.8, 6);
    // Played swung and quantized to the same swing: it stays put.
    expect(dsts([a(316.8)], swing)[0][1]).toBeCloseTo(316.8, 6);
  });

  it('adds the triplet slots of the same size when asked, and the nearer slot wins', () => {
    // 8th triplets of a 480-tick grid fall every 320 ticks.
    expect(dsts([a(330), a(470)], target({ grid: 480, catch: 240, triplets: true }))).toEqual([[330, 320], [470, 480]]);
    expect(dsts([a(330)], target({ grid: 480, catch: 240 }))).toEqual([[330, 480]]);
  });

  it('leaves anchors outside the catch where they are, as riders', () => {
    const m = quantize([a(240 + 10), a(600), a(960 - 5)], target({ catch: 60 }));
    expect(m.map((x) => [x.src, Number.isNaN(x.slot)])).toEqual([[250, false], [600, true], [955, false]]);
    // The rider moves with the line its neighbours draw.
    expect(m[1].dst).toBeCloseTo(600 + (-10 + ((5 - -10) * (600 - 250)) / (955 - 250)), 6);
  });

  it('gives each slot to its heaviest anchor; the other rides', () => {
    const m = quantize([a(470, 0.4), a(500, 0.9)], target({ catch: 240 }));
    const moved = m.filter((x) => !Number.isNaN(x.slot));
    expect(moved.map((x) => [x.src, x.dst])).toEqual([[500, 480]]);
    expect(m.length).toBe(2);
  });

  it('never lets anchors land closer than the minimum gap or cross', () => {
    // At strength 0.9 the two anchors would land 6 ticks apart (3 ms): the second rides.
    const tg = target({ strength: () => 0.9, catch: 240 });
    const m = quantize([a(230), a(250), a(470)], tg);
    for (let i = 1; i < m.length; i++) {
      expect(m[i].src - m[i - 1].src).toBeGreaterThanOrEqual(tg.minGap);
      expect(m[i].dst - m[i - 1].dst).toBeGreaterThanOrEqual(tg.minGap);
    }
    expect(m.find((x) => x.src === 230)!.slot).toBe(240);
  });

  it('reads the strength once per anchor, at its slot', () => {
    const seen: number[] = [];
    const tg = target({ strength: (slot) => { seen.push(slot); return slot < 1920 ? 0.5 : 1; } });
    const m = quantize([a(260), a(2000)], tg);
    expect(seen).toEqual([240, 1920]);
    expect(m.map((x) => x.dst)).toEqual([250, 1920]);
  });

  it('scales the strength by the anchor\'s kit slot', () => {
    const tg = target({ slotStrength: [1, 0, 0.5, 1] });
    expect(dsts([a(250, 1, 0), a(500, 1, 1), a(740, 1, 2), a(1000, 1, -1)], tg)).toEqual([[250, 240], [500, 500], [740, 730], [1000, 960]]);
  });

  it('pulls an anchor across the loop end to the next pass\'s first slot', () => {
    const m = quantize([a(BAR - 20), a(950)], target());
    expect(m.map((x) => [x.src, x.dst])).toEqual([[950, 960], [BAR - 20, BAR]]);
    expect(mapTime(m, BAR, BAR - 20)).toBe(BAR);
    // The next pass starts where the last one put it.
    expect(mapTime(m, BAR, -20)).toBe(0);
    expect(mapTime(m, BAR, BAR + 950)).toBe(BAR + 960);
  });

  it('keeps the map increasing for any strength lane', () => {
    let seed = 7;
    const rnd = () => ((seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648);
    for (let trial = 0; trial < 200; trial++) {
      const anchors = Array.from({ length: 2 + Math.floor(rnd() * 40) }, () => a(rnd() * BAR, rnd()));
      const pts = Array.from({ length: 6 }, () => rnd());
      const tg = target({
        strength: (slot) => pts[Math.floor((slot / BAR) * pts.length) % pts.length],
        triplets: rnd() < 0.5,
        feel: rnd() < 0.5 ? { kind: 'straight' } : { kind: 'swing', amount: 0.5 + rnd() * 0.25, unit: 240 },
        catch: rnd() * 240,
      });
      const m = quantize(anchors, tg);
      for (let i = 1; i < m.length; i++) {
        expect(m[i].src).toBeGreaterThan(m[i - 1].src);
        expect(m[i].dst).toBeGreaterThan(m[i - 1].dst);
      }
      if (m.length) expect(m[m.length - 1].dst).toBeLessThan(m[0].dst + BAR);
      let last = -Infinity;
      let increasing = true;
      for (let t = -BAR; t < 2 * BAR; t += 13) {
        const v = mapTime(m, BAR, t);
        increasing &&= v > last;
        last = v;
      }
      expect(increasing).toBe(true);
    }
  });
});

describe('lanes and targets', () => {
  it('reads a strength lane the way the engine reads a lane', () => {
    const lane = { target: 'flex.strength', points: [{ t: 0, v: 0, shape: 'line' as const }, { t: 1000, v: 1, shape: 'step' as const }, { t: 2000, v: 0.5, shape: 'line' as const }] };
    expect(laneValue(lane, BAR, 500)).toBe(0.5);
    expect(laneValue(lane, BAR, 1500)).toBe(1);
    expect(laneValue(lane, BAR, 3000)).toBe(0.5);
    const clip = melody({ auto: [lane] });
    expect(targetOf(clip, quantizeFlex(undefined, 240), UPS).strength(500)).toBe(0.5);
    expect(targetOf(melody(), { ...quantizeFlex(undefined, 240), strength: 0.3 }, UPS).strength(500)).toBe(0.3);
  });

  it('checks a flex setting', () => {
    const f = quantizeFlex(undefined, 240);
    expect(checkFlex(f)).toBe(f);
    const bad: Partial<Flex>[] = [{ grid: 0 }, { grid: NaN }, { strength: 1.5 }, { catch: -1 }, { feel: { kind: 'swing', amount: 0.9, unit: 240 } }, { slotStrength: [2] }];
    for (const b of bad) expect(() => checkFlex({ ...f, ...b } as Flex)).toThrow(RangeError);
  });
});

function melody(o: Partial<MelodyClip> = {}): MelodyClip {
  return { kind: 'melody', take: '', takes: [], loopLen: BAR, timeSnap: 0, morph: [], notes: [], warp: [], ...o };
}

describe('flexClip', () => {
  it('moves notes and keeps their lengths through the map; a clip without a take uses its note starts', () => {
    const c = melody({
      notes: [{ id: 'a', t: 250, dur: 470, cents: 5000, hard: 0.5 }, { id: 'b', t: 1940, dur: 100, cents: 5100, hard: 0.5 }],
      flex: quantizeFlex(undefined, 240),
    });
    const f = flexClip(c as MelodyClip & { flex: Flex }, null, UPS);
    // Between anchors the map is a straight line: 250 -> 240, 1940 -> 1920, 4090 -> 4080 (the next pass).
    const line = (t: number, s0: number, d0: number, s1: number, d1: number) => d0 + ((d1 - d0) * (t - s0)) / (s1 - s0);
    expect(f.notes!.map((n) => n.t)).toEqual([240, 1920]);
    expect(f.notes![0].dur).toBeCloseTo(line(720, 250, 240, 1940, 1920) - 240, 9);
    expect(f.notes![1].dur).toBeCloseTo(line(2040, 1940, 1920, BAR + 250, BAR + 240) - 1920, 9);
    expect(c.notes[0].t).toBe(250);
  });

  it('places the take\'s anchors through the warp anchor nearest in take time, and a curve follows its note', () => {
    // Two notes placed from take seconds 0.1 and 0.6 at clip times 250 and 1210 (0.5 s later plus 2 ticks).
    const notes = [{ id: 'a', t: 250, dur: 900, cents: 5000, hard: 0.5 }, { id: 'b', t: 1210, dur: 600, cents: 5100, hard: 0.5 }];
    const warp = [{ takeSec: 0.1, clipT: 250 }, { takeSec: 0.1 + 900 / UPS, clipT: 1150 }, { takeSec: 0.6, clipT: 1210 }, { takeSec: 0.6 + 600 / UPS, clipT: 1810 }];
    const c = melody({ take: 'k', takes: ['k'], notes, warp, flex: quantizeFlex(undefined, 240) });
    const anchors = new Float32Array([0.1, 1, 1, 0.6, 1, 1]);
    clipAnchors(c, anchors, UPS).forEach((x, i) => expect(x.t).toBeCloseTo([250, 1210][i], 3));
    const f = flexClip(c as MelodyClip & { flex: Flex }, anchors, UPS);
    f.notes!.forEach((n, i) => expect(n.t).toBeCloseTo([240, 1200][i], 3));
    for (const [i, n] of f.notes!.entries()) {
      expect(warpToTake(f.warp!, n.t, 1 / UPS)).toBeCloseTo(warpToTake(warp, notes[i].t, 1 / UPS), 9);
    }
  });

  it('moves hits and scales by kit slot; a note pulled across the loop end starts at the top and reads its own take', () => {
    const hits = [0, 1].map((k) => ({ id: `h${k}`, t: k ? 970 : BAR - 15, dur: 50, slot: k, strength: 1, shape: { attack: 0, burst: 0, cents: null, bend: 0, hold: 0, vowel: 0, tone: 0, endStop: 0, endClick: 0 } }));
    const b: BeatClip = { kind: 'beat', take: '', takes: [], loopLen: BAR, timeSnap: 0, morph: [], hits, flex: { ...quantizeFlex(undefined, 240), slotStrength: [1, 0.5] } };
    const f = flexClip(b as BeatClip & { flex: Flex }, null, UPS);
    expect(f.hits!.map((h) => h.t)).toEqual([0, 965]);
    const notes = [{ id: 'a', t: BAR - 15, dur: 300, cents: 5000, hard: 0.5 }];
    const warp = [{ takeSec: 2, clipT: BAR - 15 }, { takeSec: 2 + 300 / UPS, clipT: BAR + 285 }];
    const m = flexClip(melody({ take: 'k', takes: ['k'], notes, warp, flex: quantizeFlex(undefined, 240) }) as MelodyClip & { flex: Flex }, new Float32Array([2, 1, 1]), UPS);
    expect(m.notes![0].t).toBe(0);
    expect(warpToTake(m.warp!, 0, 1 / UPS)).toBeCloseTo(2, 9);
  });
});

describe('cost', () => {
  it('builds a 4-bar clip\'s time map and moves its notes in well under a millisecond', () => {
    // 64 notes on 16ths of four bars, each played up to 30 ms off, with a take placed by its notes.
    const len = 4 * BAR;
    let seed = 3;
    const rnd = () => ((seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648);
    const notes = Array.from({ length: 64 }, (_, k) => ({ id: `n${k}`, t: k * 240 + 20 + (rnd() - 0.5) * 100, dur: 200, cents: 5000, hard: 0.5 }));
    const warp = notes.flatMap((n) => [{ takeSec: n.t / UPS, clipT: n.t }, { takeSec: (n.t + n.dur) / UPS, clipT: n.t + n.dur }]);
    const anchors = new Float32Array(notes.flatMap((n) => [n.t / UPS, 1, 1]));
    const c = melody({ take: 'k', takes: ['k'], loopLen: len, notes, warp, flex: quantizeFlex(undefined, 240) }) as MelodyClip & { flex: Flex };
    for (let i = 0; i < 200; i++) flexClip(c, anchors, UPS);
    const runs = 2000;
    const t0 = performance.now();
    for (let i = 0; i < runs; i++) flexClip(c, anchors, UPS);
    const us = ((performance.now() - t0) * 1000) / runs;
    console.log(`flexClip: 64 notes and anchors over 4 bars, ${us.toFixed(1)} us per clip`);
    expect(us).toBeLessThan(1000);
  });
});
