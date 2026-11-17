import { describe, expect, it } from 'vitest';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import {
  CURVE_MAGIC,
  CURVE_STRIDE,
  EVENT_WORDS,
  FEEL_FREE,
  FEEL_GRID,
  TRACK_MAGIC,
  encodeCurve,
  encodeTrack,
  type TrackInput,
} from './track';

/** The fixed track behind crates/klont/tests/fixtures/track_small.bin. The
 * Rust test crates/klont/tests/track_fixture.rs reads the same file and
 * checks every field below; change both together. Values are exact in f32. */
export const smallTrack: TrackInput = {
  loop: true,
  sections: [
    { feel: 'grid', length: 3840, bpm: 120, repeats: 2, rampMs: 0 },
    { feel: 'free', length: 2.5, repeats: 1 },
  ],
  clips: [
    {
      section: 0,
      layer: 0,
      loopLen: 3840,
      curve: 3,
      events: [
        { t: 1920, dur: 960, cents: 5100, hard: 0.75 },
        { t: 0, dur: 480, cents: 4800, hard: 0.5 },
        { t: 960, dur: 480, cents: NaN, hard: 0.25 },
      ],
      warp: [
        { takeSec: 1, clipT: 960 },
        { takeSec: 0.25, clipT: 0 },
        { takeSec: 2, clipT: 2000 },
      ],
    },
    {
      section: 0,
      layer: 1,
      loopLen: 1920,
      events: [
        { t: 0, slot: 0, strength: 0.75, attack: 0.5, burst: 0.25, cents: 3600, bend: -0.5, hold: 0.125, vowel: 0.75, tone: 0.375, endStop: 0.5, endClick: 0.0625 },
        { t: 480, slot: 1, strength: 0.5 },
        { t: 960, slot: 2, strength: 1, cents: null, attack: 0.25, burst: 0.5, bend: 0.5, hold: 0.25, vowel: 0.5, tone: 0.625, endStop: 0.25, endClick: 0.125 },
        { t: 1440, slot: 3, strength: 0.25, hard: 0.5 },
      ],
    },
    {
      section: 1,
      layer: 7,
      loopLen: 2.5,
      events: [
        { t: 0, dur: 0.5, cents: 6000, hard: 1 },
        { t: 1.25, dur: 0.75, cents: 6200, hard: 0.5 },
      ],
    },
  ],
};

const fixturePath = resolve(import.meta.dirname, '../../../crates/klont/tests/fixtures/track_small.bin');

const words = (b: ArrayBuffer) => new Uint32Array(b);
const floats = (b: ArrayBuffer) => new Float32Array(b);

describe('encodeTrack layout', () => {
  const b = encodeTrack(smallTrack);
  const u = words(b);
  const f = floats(b);

  it('has the right size and header', () => {
    expect(b.byteLength).toBe((8 + 2 * 8 + 3 * 8 + 9 * 16 + 3 * 2) * 4);
    expect(Array.from(u.slice(0, 8))).toEqual([TRACK_MAGIC, 2, 3, 9, 3, 1, 0, 0]);
  });

  it('writes sections', () => {
    expect(u[8]).toBe(FEEL_GRID);
    expect([f[9], f[10], u[11], f[12]]).toEqual([3840, 120, 2, 0]);
    expect(u[16]).toBe(FEEL_FREE);
    expect([f[17], f[18], u[19], f[20]]).toEqual([2.5, 0, 1, 0]);
  });

  it('writes clip headers with event and warp runs', () => {
    const c = 8 + 16;
    expect(Array.from(u.slice(c, c + 8))).toEqual([0, 0, f32bits(3840), 0, 3, 3, 0, 3]);
    expect(Array.from(u.slice(c + 8, c + 16))).toEqual([0, 1, f32bits(1920), 3, 4, 0xffffffff, 3, 0]);
    expect(Array.from(u.slice(c + 16, c + 24))).toEqual([1, 7, f32bits(2.5), 7, 2, 0xffffffff, 3, 0]);
  });

  it('sorts events and anchors, and writes shape fields and NaN cents', () => {
    const ev = 8 + 16 + 24;
    expect([f[ev], f[ev + EVENT_WORDS], f[ev + 2 * EVENT_WORDS]]).toEqual([0, 960, 1920]);
    expect(f[ev + 2]).toBe(4800);
    expect(u[ev + EVENT_WORDS + 2]).toBe(0x7fc00000);
    const hit = ev + 3 * EVENT_WORDS;
    expect(Array.from(f.slice(hit, hit + 14)).map((x, i) => (i === 4 ? u[hit + 4] : x))).toEqual([
      0, 0, 3600, 0, 0, 0.75, 0.5, 0.25, -0.5, 0.125, 0.75, 0.375, 0.5, 0.0625,
    ]);
    expect(u[hit + EVENT_WORDS + 4]).toBe(1);
    expect(u[hit + 2 * EVENT_WORDS + 2]).toBe(0x7fc00000);
    expect(u[hit + 14]).toBe(0);
    expect(u[hit + 15]).toBe(0);
    const warp = ev + 9 * EVENT_WORDS;
    expect(Array.from(f.slice(warp, warp + 6))).toEqual([0.25, 0, 1, 960, 2, 2000]);
  });

  it('does not modify the input', () => {
    expect(smallTrack.clips[0].events[0].t).toBe(1920);
    expect(smallTrack.clips[0].warp![0].clipT).toBe(960);
  });

  it('rejects what the engine would reject', () => {
    const base = (): TrackInput => ({
      sections: [{ feel: 'grid', length: 960, bpm: 120 }],
      clips: [{ section: 0, layer: 0, loopLen: 960, events: [] }],
    });
    expect(() => encodeTrack(base())).not.toThrow();
    const bad: Array<(t: TrackInput) => void> = [
      (t) => (t.sections[0].length = 0),
      (t) => (t.sections[0].bpm = 0),
      (t) => (t.sections[0].repeats = 0),
      (t) => (t.sections[0].rampMs = -1),
      (t) => (t.clips[0].layer = 8),
      (t) => (t.clips[0].section = 1),
      (t) => (t.clips[0].loopLen = 0),
      (t) => (t.clips[0].curve = 64),
      (t) => t.clips.push({ section: 0, layer: 0, loopLen: 1, events: [] }),
      (t) => t.clips[0].events.push({ t: -1 }),
      (t) => t.clips[0].events.push({ t: NaN }),
      (t) => t.clips[0].events.push({ t: 0, dur: Infinity }),
    ];
    for (const [i, f] of bad.entries()) {
      const t = base();
      f(t);
      expect(() => encodeTrack(t), `case ${i}`).toThrow(RangeError);
    }
  });

  it('writes the empty track', () => {
    const e = encodeTrack({ sections: [], clips: [] });
    expect(Array.from(words(e))).toEqual([TRACK_MAGIC, 0, 0, 0, 0, 0, 0, 0]);
  });
});

describe('encodeCurve', () => {
  it('writes magic, count and records', () => {
    const fr = new Float32Array(2 * CURVE_STRIDE).map((_, i) => i * 0.5);
    fr[24] = NaN;
    const b = encodeCurve(fr);
    const u = words(b);
    expect(b.byteLength).toBe(8 + 2 * 25 * 4);
    expect([u[0], u[1]]).toEqual([CURVE_MAGIC, 2]);
    expect(floats(b)[2 + 1]).toBe(0.5);
    expect(u[2 + 24]).toBe(0x7fc00000);
    expect(floats(b)[2 + 49]).toBe(24.5);
  });
  it('rejects a ragged array', () => {
    expect(() => encodeCurve(new Float32Array(26))).toThrow(RangeError);
  });
  it('handles zero frames', () => {
    expect(Array.from(words(encodeCurve(new Float32Array(0))))).toEqual([CURVE_MAGIC, 0]);
  });
});

describe('cross-language fixture', () => {
  it('matches crates/klont/tests/fixtures/track_small.bin', () => {
    const got = new Uint8Array(encodeTrack(smallTrack));
    if (process.env.WRITE_FIXTURE === '1') {
      mkdirSync(dirname(fixturePath), { recursive: true });
      writeFileSync(fixturePath, got);
    }
    expect(existsSync(fixturePath), 'fixture missing: run with WRITE_FIXTURE=1').toBe(true);
    const want = new Uint8Array(readFileSync(fixturePath));
    expect(got.length).toBe(want.length);
    expect(Buffer.from(got).equals(Buffer.from(want))).toBe(true);
  });
});

function f32bits(x: number): number {
  return new Uint32Array(new Float32Array([x]).buffer)[0];
}
