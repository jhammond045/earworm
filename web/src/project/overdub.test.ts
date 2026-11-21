import { describe, expect, it } from 'vitest';
import * as pr from '../m1/protocol';
import { classicPatch } from '../synth/classic';
import { clockMark, placeTake, positionAt, type ClockMark } from './overdub';
import type { BeatClip, Layer, MelodyClip, Project, Section, Take } from './types';

const shape = { attack: 0.002, burst: 0.5, cents: null, bend: 0, hold: 0.03, vowel: 0, tone: 0.5, endStop: 0, endClick: 0 };
const tune = { snap: 1, speedMs: 0, slides: 0 as const, vibrato: 1, levelOut: 0, root: 0, scale: 0, refCents: 5700, octave: 0 };
const layer = (id: string, kind: Layer['kind']): Layer => ({
  id, name: id, kind, color: 0, level: 0, pan: 0, mute: false, patch: classicPatch(kind), states: [], tune, follow: 1, kit: [],
  depth: { loud: 1, open: 0.5, hard: 0.5, breath: 0.5, mouth: 1 },
});
const take: Take = { id: 'tk', audio: 'a', frames: 'f', rate: 48000, latencyMs: 0, frameCount: 100, kind: 'melody', refCents: 5700, root: 0, scale: 0, bpm: null };
const note = (t: number, dur: number) => ({ t, dur, cents: 6000, hard: 0.5 });

function doc(feel: 'grid' | 'free', clips: Section['clips'] = {}): Project {
  const s: Section = { id: 's', name: 'A', feel, beats: 4, seconds: 4, bpm: 120, repeats: 1, rampMs: 0, clips };
  return { schema: 1, id: 'p', name: 'p', sections: [s], layers: [layer('m', 'melody'), layer('b', 'beat')], takes: {}, loop: true };
}
// 120 bpm at 48 kHz: 1920 ticks/s, 0.04 ticks/sample.
const mark = (over: Partial<ClockMark> = {}): ClockMark => ({ sample: 48000, section: 0, pos: 1920, unitsPerSample: 0.04, playing: true, ...over });

describe('positionAt', () => {
  it('extrapolates forwards and backwards inside the section', () => {
    expect(positionAt(mark(), 48000)).toEqual({ section: 0, pos: 1920 });
    expect(positionAt(mark(), 48000 + 24000).pos).toBeCloseTo(1920 + 960);
    expect(positionAt(mark(), 48000 - 24000).pos).toBeCloseTo(1920 - 960);
  });
  it('does not wrap past a section end (the caller wraps)', () => {
    expect(positionAt(mark({ pos: 3800 }), 48000 + 4000).pos).toBeCloseTo(3800 + 160);
    expect(positionAt(mark({ pos: 10 }), 48000 - 4000).pos).toBeCloseTo(10 - 160);
  });
  it('holds still when stopped', () => {
    expect(positionAt(mark({ playing: false, unitsPerSample: 0 }), 99999).pos).toBe(1920);
  });
  it('free sections count seconds', () => {
    expect(positionAt(mark({ unitsPerSample: 1 / 48000, pos: 2 }), 48000 + 24000).pos).toBeCloseTo(2.5);
  });
});

describe('clockMark', () => {
  it('reads the meter slots', () => {
    const m = new Float32Array(pr.METER_LEN);
    m[pr.M_CLOCK_HI] = 3;
    m[pr.M_CLOCK_LO] = 1000;
    m[pr.M_SECTION] = 2;
    m[pr.M_SECTION_POS] = 480;
    m[pr.M_UNITS_PER_SAMPLE] = 0.04;
    m[pr.M_PLAYING] = 1;
    const c = clockMark(m);
    expect(c.sample).toBe(3 * 1048576 + 1000);
    expect([c.section, c.pos, c.playing]).toEqual([2, 480, true]);
    expect(c.unitsPerSample).toBeCloseTo(0.04);
    expect(clockMark(new Float32Array(pr.METER_LEN)).playing).toBe(false);
  });
});

describe('placeTake', () => {
  const args = (d: Project, extra = {}) => ({ doc: d, sectionId: 's', layerId: 'm', take, startSample: 48000, mark: mark(), roundTripSamples: 0,
    notes: [note(0, 0.25), note(0.5, 0.25)], hits: [], ...extra });

  it('grid: take seconds become ticks from the position at start', () => {
    const c = placeTake(args(doc('grid'))) as MelodyClip;
    expect(c.kind).toBe('melody');
    expect(c.loopLen).toBe(3840);
    expect(c.notes.map((n) => [n.t, n.dur])).toEqual([[1920, 480], [2880, 480]]);
    expect(c.take).toBe('tk');
    expect(c.takes).toEqual(['tk']);
    expect(c.notes[0].id).not.toBe(c.notes[1].id);
  });

  it('subtracts the round trip', () => {
    // 30 ms = 1440 samples = 57.6 ticks earlier.
    const c = placeTake(args(doc('grid'), { roundTripSamples: 1440 })) as MelodyClip;
    expect(c.notes[0].t).toBeCloseTo(1920 - 57.6, 6);
    expect(c.notes[1].t).toBeCloseTo(2880 - 57.6, 6);
  });

  it('wraps into the loop, also backwards', () => {
    const c = placeTake(args(doc('grid'), { notes: [note(1.5, 0.25), note(0.1, 0.1)], startSample: 48000 + 24000 })) as MelodyClip;
    // p0 = 1920 + 960 = 2880; note at 1.5 s -> 2880 + 2880 = 5760 -> 1920
    expect(c.notes.map((n) => n.t)).toEqual([1920, 2880 + 192]); // notes come back sorted by time
    const back = placeTake(args(doc('grid'), { roundTripSamples: 48000 * 2, startSample: 48000 })) as MelodyClip;
    // p0 = 1920 - 3840 = -1920 -> 1920
    expect(back.notes[0].t).toBeCloseTo(1920);
  });

  it('free: seconds, and the loop length is the section length', () => {
    const m = mark({ unitsPerSample: 1 / 48000, pos: 1 });
    const c = placeTake(args(doc('free'), { mark: m, notes: [note(0, 0.5), note(3.5, 0.2)] })) as MelodyClip;
    expect(c.loopLen).toBe(4);
    expect(c.notes[0].t).toBeCloseTo(0.5); // 1 + 3.5 = 4.5 -> 0.5
    expect(c.notes[1].t).toBeCloseTo(1);
    expect(c.notes[1].dur).toBeCloseTo(0.5);
  });

  it('keeps the existing loop length, snap and take stack', () => {
    const old: MelodyClip = { kind: 'melody', take: 'old', takes: ['old'], loopLen: 1920, timeSnap: 120, morph: [], notes: [], warp: [] };
    const c = placeTake(args(doc('grid', { m: old }), { notes: [note(0.5, 0.1)] })) as MelodyClip;
    expect(c.loopLen).toBe(1920);
    expect(c.timeSnap).toBe(120);
    expect(c.takes).toEqual(['old', 'tk']);
    expect(c.take).toBe('tk');
    expect(c.notes[0].t).toBe(1920 % 1920 + 960); // p0 1920 -> 0 mod 1920, plus 960
  });

  it('warp anchors at every note start and end, in take seconds and clip time', () => {
    const c = placeTake(args(doc('grid'))) as MelodyClip;
    expect(c.warp).toEqual([
      { takeSec: 0, clipT: 1920 }, { takeSec: 0.25, clipT: 2400 }, { takeSec: 0.5, clipT: 2880 }, { takeSec: 0.75, clipT: 3360 },
    ]);
  });

  it('beat layer: hits with shape copied, no warp', () => {
    const hits = [{ t: 0.25, dur: 0.05, slot: 2, strength: 0.7, shape: { ...shape, cents: 3000 }, features: [], live: [] }];
    const c = placeTake({ ...args(doc('grid')), layerId: 'b', hits }) as BeatClip;
    expect(c.kind).toBe('beat');
    expect(c.hits[0]).toMatchObject({ t: 1920 + 480, dur: 96, slot: 2, strength: 0.7 });
    expect(c.hits[0].shape).toEqual({ ...shape, cents: 3000 });
    expect(c.hits[0].shape).not.toBe(hits[0].shape);
    expect('warp' in c).toBe(false);
  });
});
