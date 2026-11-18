// The second pass through the committed analysis.wasm, on small synthetic takes.
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import {
  AnalysisCore, FRAME_STRIDE, HIT_LEN, NOTE_LEN, OPTIONS_LEN, OUT_HEADER, decodeSecondPass, encodeOptions, teach,
  type Hit, type SecondPass, type TuneSettings,
} from './analysis';

const SR = 48000;
const core = AnalysisCore.fromBytes(readFileSync(new URL('../../public/wasm/analysis.wasm', import.meta.url)));
const cents = (hz: number) => 1200 * Math.log2(hz / 16.351598);

function rng(seed: number) {
  let s = seed >>> 0;
  return () => {
    s = (Math.imul(s, 1664525) + 1013904223) >>> 0;
    return s / 2 ** 32 - 0.5;
  };
}

interface Take { audio: Float32Array; frames: Float32Array }

/** Sine notes with a 10 ms attack and release; frames as the live tier would write them. */
function hum(notes: { t: number; dur: number; hz: number }[], total: number): Take {
  const audio = new Float32Array(Math.round(total * SR));
  const n = Math.floor(total * 100);
  const frames = new Float32Array(n * FRAME_STRIDE);
  for (let k = 0; k < n; k++) {
    const f = frames.subarray(k * FRAME_STRIDE, (k + 1) * FRAME_STRIDE);
    f[0] = NaN; f[24] = NaN; f[7] = 2;
  }
  const noise = rng(5);
  for (let i = 0; i < audio.length; i++) audio[i] = 0.0003 * noise();
  for (const { t, dur, hz } of notes) {
    let ph = 0;
    for (let i = Math.round(t * SR); i < Math.min(audio.length, Math.round((t + dur) * SR)); i++) {
      const x = i / SR - t;
      const env = Math.min(1, x / 0.01, (dur - x) / 0.01);
      ph += (2 * Math.PI * hz) / SR;
      audio[i] += 0.2 * env * Math.sin(ph);
    }
    for (let k = Math.ceil((t + 0.03) * 100); k < Math.min(n, Math.floor((t + dur - 0.01) * 100)); k++) {
      const f = frames.subarray(k * FRAME_STRIDE, (k + 1) * FRAME_STRIDE);
      f[0] = cents(hz); f[1] = 0.95; f[2] = 0.6; f[3] = 0.5; f[7] = 0;
    }
  }
  return { audio, frames };
}

/** Decaying noise bursts: `dark` ones low-passed, the others bright (second difference). Unvoiced frames. */
function bursts(times: { t: number; bright: boolean }[], total: number): Take {
  const audio = new Float32Array(Math.round(total * SR));
  const n = Math.floor(total * 100);
  const frames = new Float32Array(n * FRAME_STRIDE);
  for (let k = 0; k < n; k++) {
    const f = frames.subarray(k * FRAME_STRIDE, (k + 1) * FRAME_STRIDE);
    f[0] = NaN; f[24] = NaN; f[7] = 2;
  }
  const noise = rng(9);
  for (let i = 0; i < audio.length; i++) audio[i] = 0.0003 * noise();
  for (const { t, bright } of times) {
    let lp = 0, x1 = 0, x2 = 0;
    const a = Math.round(t * SR);
    for (let i = 0; i < 0.25 * SR && a + i < audio.length; i++) {
      const w = noise() * 2;
      let y: number;
      if (bright) {
        y = (w - 2 * x1 + x2) * 0.25;
        x2 = x1; x1 = w;
      } else {
        lp += 0.04 * (w - lp);
        y = lp * 3;
      }
      audio[a + i] += y * Math.exp(-i / (0.03 * SR));
    }
    for (let k = Math.round(t * 100); k < Math.min(n, Math.round(t * 100) + 20); k++) {
      const f = frames.subarray(k * FRAME_STRIDE, (k + 1) * FRAME_STRIDE);
      f[2] = 0.7; f[7] = 1;
      f[5] = k === Math.round(t * 100) + 2 ? 1 : 0;
      for (let b = 0; b < 16; b++) f[8 + b] = bright ? b - 8 : 8 - b;
    }
  }
  return { audio, frames };
}

const NOTES = [
  { t: 0.3, dur: 0.6, hz: 220 },
  { t: 1.1, dur: 0.6, hz: 261.63 },
  { t: 1.9, dur: 0.6, hz: 329.63 },
];

describe('second pass through analysis.wasm', () => {
  it('finds the notes of a small hum and decodes every field', () => {
    const take = hum(NOTES, 3);
    const r = core.secondPass(take.audio, SR, take.frames);
    expect(r.kind).toBe('melody');
    expect(r.notes.length).toBe(3);
    r.notes.forEach((n, i) => {
      expect(Math.abs(n.t - NOTES[i].t)).toBeLessThan(0.04);
      expect(Math.abs(n.cents - cents(NOTES[i].hz))).toBeLessThan(30);
      expect(n.dur).toBeGreaterThan(0.4);
      expect(n.hard).toBeGreaterThanOrEqual(0);
      expect(n.hard).toBeLessThanOrEqual(1);
    });
    expect(r.hits.length).toBe(0);
    expect(r.pitch.length).toBe(300);
    expect(Number.isNaN(r.pitch[5])).toBe(true);
    expect(r.pitch[60]).toBeCloseTo(cents(220), -1);
    expect(r.voicedFraction).toBeGreaterThan(0.9);
    expect(r.refCents).toBeGreaterThanOrEqual(-50);
    expect(r.refCents).toBeLessThan(50);
    expect(r.bpm).toBeNull();
    expect(r.beats).toBeNull();
    expect(r.loopSec).toBeCloseTo(3, 1);
    expect(r.ms).toBeGreaterThan(0);
    expect(r.root).toBeGreaterThanOrEqual(0);
    expect(r.scale).toBeGreaterThanOrEqual(0);
  });

  it('reads beats: hits, kit slots, tempo and the live features', () => {
    const times = [] as { t: number; bright: boolean }[];
    for (let i = 0; i < 12; i++) times.push({ t: 0.3 + i * 0.5, bright: i % 2 === 1 }); // 120 bpm quarters
    const take = bursts(times, 6.8);
    const r = core.secondPass(take.audio, SR, take.frames);
    expect(r.kind).toBe('beat');
    expect(r.hits.length).toBe(12);
    expect(r.kinds).toBe(2);
    r.hits.forEach((h, i) => {
      expect(Math.abs(h.t - times[i].t)).toBeLessThan(0.03);
      expect(h.features.length).toBe(6);
      expect(h.live.length).toBe(3);
      expect(h.strength).toBeGreaterThan(0);
      expect(h.strength).toBeLessThanOrEqual(1);
      expect(h.shape.cents).toBeNull();
      expect(h.shape.hold).toBeGreaterThan(0.05);
      expect(h.shape.burst).toBeGreaterThanOrEqual(0);
      expect(h.shape.burst).toBeLessThanOrEqual(1);
      expect(Object.values(h.shape).every((v) => v === null || Number.isFinite(v))).toBe(true);
    });
    // Dark and bright hits fall in different slots, dark first (groups are ordered by centroid).
    const dark = r.hits.filter((_, i) => i % 2 === 0), bright = r.hits.filter((_, i) => i % 2 === 1);
    expect(new Set(dark.map((h) => h.slot)).size).toBe(1);
    expect(new Set(bright.map((h) => h.slot)).size).toBe(1);
    expect(dark[0].slot).toBe(0);
    expect(bright[0].slot).toBe(1);
    // The bright hits' live brightness (from the frame bands) is above the dark ones'.
    expect(bright[0].live[0]).toBeGreaterThan(dark[0].live[0]);
    expect(r.bpm).not.toBeNull();
    expect(r.bpm! >= 60 && r.bpm! <= 180).toBe(true);
    expect(r.beats!).toBeGreaterThanOrEqual(1);
    // The loop is a whole number of beats.
    expect(r.loopSec * r.bpm! / 60).toBeCloseTo(r.beats!, 3);
  });

  it('honours the kind override and kinds', () => {
    const times = [] as { t: number; bright: boolean }[];
    for (let i = 0; i < 12; i++) times.push({ t: 0.3 + i * 0.5, bright: i % 2 === 1 });
    const take = bursts(times, 6.8);
    const one = core.secondPass(take.audio, SR, take.frames, { kind: 'beat', kinds: 1 });
    expect(one.kinds).toBe(1);
    expect(new Set(one.hits.map((h) => h.slot))).toEqual(new Set([0]));
    const asHum = core.secondPass(take.audio, SR, take.frames, { kind: 'melody' });
    expect(asHum.kind).toBe('melody');
    expect(asHum.hits.length).toBe(0);
    const tex = core.secondPass(take.audio, SR, take.frames, { kind: 'texture' });
    expect(tex.kind).toBe('texture');
    expect(tex.notes.length + tex.hits.length).toBe(0);
  });

  it('assigns hits to a taught kit and uses a spare slot for strangers', () => {
    const times = [] as { t: number; bright: boolean }[];
    for (let i = 0; i < 12; i++) times.push({ t: 0.3 + i * 0.5, bright: i % 2 === 1 });
    const take = bursts(times, 6.8);
    const first = core.secondPass(take.audio, SR, take.frames);
    const dark = first.hits.filter((_, i) => i % 2 === 0), bright = first.hits.filter((_, i) => i % 2 === 1);
    // Teach the bright sound as slot 2 only.
    const taught = teach([[], [], bright]);
    const r = core.secondPass(take.audio, SR, take.frames, { kind: 'beat', taught });
    r.hits.forEach((h, i) => expect(h.slot).toBe(i % 2 === 1 ? 2 : 0));
    // Both taught: each goes to its own.
    const both = core.secondPass(take.audio, SR, take.frames, { kind: 'beat', taught: teach([[], dark, [], bright]) });
    both.hits.forEach((h, i) => expect(h.slot).toBe(i % 2 === 1 ? 3 : 1));
  });

  it('tunes a stored take and levels it out', () => {
    const take = hum([{ t: 0.3, dur: 1.0, hz: 233 }, { t: 1.5, dur: 1.0, hz: 277 }], 2.8);
    const r = core.secondPass(take.audio, SR, take.frames);
    const s: TuneSettings = { snap: 1, speedMs: 0, slides: 0, vibrato: 0, root: 0, scale: 0, refCents: 0, levelOut: 0 };
    const t = core.tune(take.frames, r.pitch, s);
    expect(t.tuned.length).toBe(280);
    expect(t.loudness.length).toBe(280);
    const k = 70;
    expect(t.tuned[k]).toBeCloseTo(Math.round(r.pitch[k] / 100) * 100, 1);
    // Chromatic snap puts every voiced frame on a semitone; unvoiced frames before the first note are NaN.
    expect(Number.isNaN(t.tuned[5])).toBe(true);
    // Level out 0: stored loudness untouched; 1: flat where sounding.
    for (let i = 0; i < 280; i++) expect(t.loudness[i]).toBeCloseTo(take.frames[i * FRAME_STRIDE + 2], 6);
    take.frames[(80) * FRAME_STRIDE + 2] = 0.9;
    const flat = core.tune(take.frames, r.pitch, { ...s, levelOut: 1 });
    expect(flat.loudness[80]).toBeCloseTo(0.6, 3);
    expect(flat.loudness[5]).toBe(0);
    // A scale and root change the targets: G major pentatonic root 7 has no A#.
    const pent = core.tune(take.frames, r.pitch, { ...s, root: 7, scale: 3 });
    const semis = Math.round(pent.tuned[k] / 100);
    expect([7, 9, 11, 2, 4].includes(((semis % 12) + 12) % 12)).toBe(true);
  });
});

describe('decodeSecondPass', () => {
  function flat(): Float32Array {
    const f = new Float32Array(OUT_HEADER + 3 + 2 * NOTE_LEN + HIT_LEN);
    f.set([1, 0.5, 3.25, 7, -12, 3, 2, NaN, NaN, 4.8, 3, 3, 2, 1, HIT_LEN, 0]);
    f.set([100, NaN, 300], OUT_HEADER);
    f.set([0.1, 0.2, 5000, 0.4, 0.5, 0.6, 5100, 0.7], OUT_HEADER + 3);
    const h = OUT_HEADER + 3 + 2 * NOTE_LEN;
    f.set([0.5, 0.25, 2, 0.8, 0.003, 0.4, NaN, 1.5, 0.25, 0.3, 0.2, 1, 0, 1, 2, 3, 4, 5, 6, 0.1, 0.2, 0.3], h);
    return f;
  }

  it('splits the flat layout into its parts', () => {
    const r = decodeSecondPass(flat(), 9);
    expect(r.kind).toBe('beat');
    expect(r.voicedFraction).toBe(0.5);
    expect(r.onsetsPerSec).toBe(3.25);
    expect(r.repaired).toBe(7);
    expect(r.refCents).toBe(-12);
    expect([r.root, r.scale, r.kinds]).toEqual([3, 2, 3]);
    expect(r.bpm).toBeNull();
    expect(r.beats).toBeNull();
    expect(r.loopSec).toBeCloseTo(4.8);
    expect(r.ms).toBe(9);
    expect(r.pitch.length).toBe(3);
    expect(Number.isNaN(r.pitch[1])).toBe(true);
    expect(r.notes).toEqual([
      { t: expect.closeTo(0.1), dur: expect.closeTo(0.2), cents: 5000, hard: expect.closeTo(0.4) },
      { t: expect.closeTo(0.5), dur: expect.closeTo(0.6), cents: 5100, hard: expect.closeTo(0.7) },
    ]);
    const h = r.hits[0];
    expect(h.slot).toBe(2);
    expect(h.shape.cents).toBeNull();
    expect(h.shape.endStop).toBe(1);
    expect(h.shape.bend).toBeCloseTo(1.5);
    expect(h.features).toEqual([1, 2, 3, 4, 5, 6]);
    expect(h.live.map((v) => Math.round(v * 10))).toEqual([1, 2, 3]);
    const f = flat();
    f[7] = 100.5; f[8] = 16;
    expect(decodeSecondPass(f).bpm).toBeCloseTo(100.5);
    expect(decodeSecondPass(f).beats).toBe(16);
  });

  it('rejects short or inconsistent results', () => {
    expect(() => decodeSecondPass(new Float32Array(4))).toThrow();
    const f = flat();
    expect(() => decodeSecondPass(f.subarray(0, f.length - 1))).toThrow();
    f[14] = 21;
    expect(() => decodeSecondPass(f)).toThrow();
    const g = flat();
    g[0] = 5;
    expect(() => decodeSecondPass(g)).toThrow();
  });
});

describe('encodeOptions and teach', () => {
  it('lays out kind, kinds and taught slots', () => {
    const o = encodeOptions({ kind: 'beat', kinds: 3, taught: [null, { centre: [1, 2, 3, 4, 5, 6], spread: [0.1, 0.2, 0.3, 0.4, 0.5, 0.6] }, null, null] });
    expect(o.length).toBe(OPTIONS_LEN);
    expect([o[0], o[1]]).toEqual([2, 3]);
    expect(o[2]).toBe(0);
    expect(o[2 + 13]).toBe(1);
    expect(Array.from(o.slice(2 + 13 + 1, 2 + 13 + 7))).toEqual([1, 2, 3, 4, 5, 6]);
    expect(o[2 + 13 + 7 + 5]).toBeCloseTo(0.6);
    expect(Array.from(encodeOptions()).every((v) => v === 0)).toBe(true);
    expect(encodeOptions({ kind: 'melody' })[0]).toBe(1);
    expect(encodeOptions({ kind: 'texture' })[0]).toBe(3);
  });

  const hit = (features: number[]): Hit => ({
    t: 0, dur: 0.1, slot: 0, strength: 1, features, live: [0, 0, 0],
    shape: { attack: 0, burst: 0, cents: null, bend: 0, hold: 0, vowel: 0, tone: 0, endStop: 0, endClick: 0 },
  });

  it('teach: mean and spread per slot, floors on the spread, null for empty slots', () => {
    const t = teach([
      [hit([0, 0, 0, 0, 0, 0]), hit([4, 1, 1, 2, 1, 2])],
      [],
      [hit([1, 0.5, 0.2, 3, 0.5, 6])],
    ]);
    expect(t.length).toBe(4);
    expect(t[1]).toBeNull();
    expect(t[3]).toBeNull();
    expect(t[0]!.centre).toEqual([2, 0.5, 0.5, 1, 0.5, 1]);
    // std of [0, 4] is 2 (> floor 1.0); the rest are under their floors or above.
    expect(t[0]!.spread[0]).toBeCloseTo(2);
    expect(t[0]!.spread[1]).toBeCloseTo(0.5);
    expect(t[0]!.spread[5]).toBeCloseTo(1);
    // One hit: the spread is the floor.
    expect(t[2]!.centre).toEqual([1, 0.5, 0.2, 3, 0.5, 6]);
    expect(t[2]!.spread).toEqual([1.0, 0.2, 0.15, 0.7, 0.4, 0.5]);
  });
});
