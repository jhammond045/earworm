import { describe, expect, it } from 'vitest';
import { CLIP_WORDS, CURVE_MAGIC, CURVE_STRIDE, EVENT_WORDS, FEEL_FREE, SECTION_WORDS, TRACK_HEADER_WORDS, TRACK_MAGIC } from '../m1/track';
import type { Hit, Note } from './analysis';
import { MAX_TAKE_SEC, MIC_RING_SEC, POLY_MAX_TAKE_SEC, buildCurveBlob, buildTrackBlob, hitsToEvents, levelDelayMs, loopSeconds, meanLive, measureFollow,
  noteTargets, notesToEvents, pitchLockDelaysMs, tally, withTuned } from './take';

const SR = 48000;
const hz = (cents: number) => 16.351597831287414 * 2 ** (cents / 1200);

describe('curve blob', () => {
  it('replaces only the tuned slot and keeps the header layout', () => {
    const n = 3;
    const frames = new Float32Array(n * CURVE_STRIDE).map((_, i) => i);
    const tuned = new Float32Array([5000, NaN, 5200]);
    const w = withTuned(frames, tuned, 1);
    expect(w[24]).toBe(6200);
    expect(w[25 + 24]).toBeNaN();
    expect(w[2 * 25 + 24]).toBe(6400);
    expect(w[3]).toBe(3);
    expect(w[25 + 7]).toBe(32);
    const u = new Uint32Array(buildCurveBlob(frames, tuned));
    expect(u[0]).toBe(CURVE_MAGIC);
    expect(u[1]).toBe(n);
    expect(u.length).toBe(2 + n * CURVE_STRIDE);
  });
});

describe('track blob', () => {
  const notes: Note[] = [{ t: 0.1, dur: 0.5, cents: 4800, hard: 0.4 }, { t: 0.8, dur: 0.4, cents: 5000, hard: 0.2 }];
  it('melody: one free section, one clip, curve slot, notes as events', () => {
    const buf = buildTrackBlob({ kind: 'melody', layer: 0, curveSlot: 0, sp: { notes, hits: [], loopSec: 2 }, frameCount: 150 });
    const u = new Uint32Array(buf), f = new Float32Array(buf), i = new Int32Array(buf);
    expect(u[0]).toBe(TRACK_MAGIC);
    expect([u[1], u[2], u[3], u[4], u[5]]).toEqual([1, 1, 2, 0, 1]); // sections, clips, events, warp, loop
    const s = TRACK_HEADER_WORDS;
    expect(u[s]).toBe(FEEL_FREE);
    expect(f[s + 1]).toBe(2);
    expect(u[s + 3]).toBe(1);
    const c = s + SECTION_WORDS;
    expect(u[c]).toBe(0); // section
    expect(u[c + 1]).toBe(0); // layer
    expect(f[c + 2]).toBe(2); // loopLen
    expect(u[c + 3]).toBe(0);
    expect(u[c + 4]).toBe(2);
    expect(i[c + 5]).toBe(0); // curve slot
    const e = c + CLIP_WORDS;
    expect(f[e]).toBeCloseTo(0.1);
    expect(f[e + 1]).toBeCloseTo(0.5);
    expect(f[e + 2]).toBe(4800);
    expect(f[e + 3]).toBeCloseTo(0.4);
    expect(f[e + 5]).toBe(1);
    expect(f[e + EVENT_WORDS + 2]).toBe(5000);
  });

  it('applies the octave to note cents', () => {
    expect(notesToEvents(notes, -1)[0].cents).toBe(3600);
  });

  it('beat: hits with slot, strength and shape fields, no curve', () => {
    const hit = (t: number, slot: number): Hit => ({
      t, dur: 0.1, slot, strength: 0.7,
      shape: { attack: 0.01, burst: 0.6, cents: null, bend: -2, hold: 0.15, vowel: 0.3, tone: 0.2, endStop: 1, endClick: 0.4 },
      features: [0, 0, 0, 0, 0, 0], live: [0.5, 0.2, 0.1],
    });
    const buf = buildTrackBlob({ kind: 'beat', layer: 1, curveSlot: 1, sp: { notes: [], hits: [hit(0.5, 2), hit(0.1, 0)], loopSec: 1 }, frameCount: 100 });
    const u = new Uint32Array(buf), f = new Float32Array(buf), i = new Int32Array(buf);
    const c = TRACK_HEADER_WORDS + SECTION_WORDS;
    expect(u[c + 1]).toBe(1);
    expect(i[c + 5]).toBe(-1);
    const e = c + CLIP_WORDS;
    // sorted by time: the t=0.1 slot 0 hit first
    expect(f[e]).toBeCloseTo(0.1);
    expect(u[e + 4]).toBe(0);
    expect(f[e + 2]).toBeNaN();
    expect(f[e + 5]).toBeCloseTo(0.7);
    expect([f[e + 6], f[e + 7], f[e + 8], f[e + 9], f[e + 10], f[e + 11], f[e + 12], f[e + 13]].map((v) => +v.toFixed(3)))
      .toEqual([0.01, 0.6, -2, 0.15, 0.3, 0.2, 1, 0.4]);
    expect(u[e + EVENT_WORDS + 4]).toBe(2);
    expect(hitsToEvents([hit(0, 9)])[0].slot).toBe(3);
  });

  it('drops events past the loop and falls back to the take length', () => {
    const b = buildTrackBlob({ kind: 'melody', layer: 0, curveSlot: 0, sp: { notes, hits: [], loopSec: NaN }, frameCount: 70 });
    const u = new Uint32Array(b);
    expect(u[3]).toBe(1); // the note at 0.8 s is past a 0.7 s loop
    expect(loopSeconds({ loopSec: 0 }, 90)).toBeCloseTo(0.9);
  });
});

describe('mean live features', () => {
  it('averages per slot', () => {
    const h = (l: number[]) => ({ live: l }) as Hit;
    expect(meanLive([h([1, 0, 0]), h([0, 1, 0.5])])).toEqual([0.5, 0.5, 0.25]);
    expect(meanLive([])).toBeNull();
  });
});

/** Bursts of a 200 Hz tone, 80 ms on, 200 ms off, with a fast attack. */
function bursts(seconds: number, hzv: number, delayMs: number, attackMs = 2): Float32Array {
  const x = new Float32Array(Math.round(seconds * SR));
  const d = Math.round((delayMs / 1000) * SR);
  for (let i = 0; i < x.length; i++) {
    const t = (i - d) / SR;
    if (t < 0) continue;
    const ph = t % 0.28;
    if (ph < 0.08) x[i] = Math.sin(2 * Math.PI * hzv * t) * Math.min(1, ph / (attackMs / 1000));
  }
  return x;
}

describe('follow delay', () => {
  it('level delay recovers a known shift', () => {
    const mic = bursts(3, 200, 0);
    for (const d of [3, 12, 40]) {
      const out = bursts(3, 200, d);
      expect(levelDelayMs(mic, out, SR)!).toBeCloseTo(d, 0);
    }
  });

  it('level delay is null for silence', () => {
    expect(levelDelayMs(new Float32Array(SR), new Float32Array(SR), SR)).toBeNull();
  });

  it('pitch lock: time from note start to being within 50 cents', () => {
    const cents = 4800 + 400; // some pitch near 330 Hz... 
    const f = hz(cents);
    const out = new Float32Array(SR * 2);
    // output sits 300 cents flat for 60 ms, then locks
    const start = 0.5, lock = 0.06;
    for (let i = 0; i < out.length; i++) {
      const t = i / SR - start;
      if (t < 0) continue;
      const fr = t < lock ? f * 2 ** (-300 / 1200) : f;
      out[i] = 0.5 * Math.sin(2 * Math.PI * fr * t);
    }
    const [d] = pitchLockDelaysMs(out, SR, [{ startSec: start, cents }]);
    expect(d).not.toBeNull();
    expect(d!).toBeGreaterThan(40);
    expect(d!).toBeLessThan(60 + 15);
  });

  it('pitch lock is null for a note that never arrives', () => {
    const out = new Float32Array(SR);
    expect(pitchLockDelaysMs(out, SR, [{ startSec: 0.2, cents: 5000 }])).toEqual([null]);
  });

  it('measureFollow combines both', () => {
    const f = 220;
    const mic = bursts(3, f, 0);
    const out = bursts(3, f, 15);
    const cents = 1200 * Math.log2(f / 16.351597831287414);
    const targets = [0, 1, 2].map((k) => ({ startSec: k * 0.28 * 3 + 0.0, cents }));
    const r = measureFollow(mic, out, SR, targets);
    expect(r.levelMs!).toBeCloseTo(15, 0);
    expect(r.pitchN).toBeGreaterThan(0);
    expect(r.pitchMs!).toBeGreaterThan(10);
    expect(r.pitchMs!).toBeLessThan(40);
  });

  it('noteTargets reads the tuned slot after the start', () => {
    const n = 100;
    const fr = new Float32Array(n * CURVE_STRIDE);
    for (let i = 0; i < n; i++) fr[i * CURVE_STRIDE + 24] = i < 20 ? 100 : 4900;
    const t = noteTargets(fr, [{ t: 0.1, dur: 0.6, cents: 4800, hard: 0 }], 1);
    expect(t).toEqual([{ startSec: 0.1, cents: 6100 }]);
  });
});

describe('tally', () => {
  it('counts rated takes only', () => {
    const t = tally([{ kept: true }, { kept: true }, { kept: true }, { kept: true }, { kept: false }, { kept: null }]);
    expect(t).toMatchObject({ kept: 4, rated: 5, pct: 80, meets: true });
    expect(tally([{ kept: true }, { kept: false }]).meets).toBe(false);
    expect(tally([]).meets).toBe(false);
  });
  it('takes cap below the mic and frame rings', () => {
    expect(MAX_TAKE_SEC).toBeLessThan(MIC_RING_SEC);
    expect(POLY_MAX_TAKE_SEC).toBe(300);
    expect(POLY_MAX_TAKE_SEC).toBeLessThan(MIC_RING_SEC);
  });
});
