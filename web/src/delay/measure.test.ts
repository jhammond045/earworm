import { describe, expect, it } from 'vitest';
import {
  chirp, crossCorrelationLag, estimateF0, median, medianF0, onsetIndex,
  pitchLockIndex, segmentBursts, spread, synthHum,
} from './measure';

const SR = 48000;

// Deterministic noise.
function rng(seed: number) {
  let s = seed >>> 0;
  return () => ((s = (Math.imul(s, 1664525) + 1013904223) >>> 0) / 2 ** 32) * 2 - 1;
}

function saw(n: number, f: number, sr = SR, from = 0): Float32Array {
  const x = new Float32Array(n);
  for (let i = from; i < n; i++) x[i] = 2 * (((i * f) / sr) % 1) - 1;
  return x;
}

describe('median and spread', () => {
  it('handles odd, even and empty', () => {
    expect(median([3, 1, 2])).toBe(2);
    expect(median([4, 1, 2, 3])).toBe(2.5);
    expect(median([])).toBeNaN();
    const s = spread([1, 2, 3]);
    expect([s.min, s.max, s.n]).toEqual([1, 3, 3]);
    expect(s.stdev).toBeCloseTo(Math.sqrt(2 / 3), 6);
  });
});

describe('crossCorrelationLag', () => {
  it('recovers known integer delays exactly', () => {
    const c = chirp(SR);
    for (const d of [0, 1, 137, 2400, 9999]) {
      const sig = new Float32Array(30000);
      sig.set(c, 500 + d);
      const r = crossCorrelationLag(sig, c, 14400, 500);
      expect(Math.abs(r.lag - d)).toBeLessThan(1);
      expect(r.norm).toBeGreaterThan(0.99);
    }
  });

  it('recovers the delay under noise and a quieter copy', () => {
    const c = chirp(SR);
    const rand = rng(7);
    const sig = new Float32Array(30000);
    for (let i = 0; i < sig.length; i++) sig[i] = 0.05 * rand();
    for (let i = 0; i < c.length; i++) sig[3000 + i] += 0.2 * c[i];
    const r = crossCorrelationLag(sig, c, 14400, 0);
    expect(Math.abs(r.lag - 3000)).toBeLessThan(1);
  });

  it('reports low norm when the click is absent', () => {
    const rand = rng(3);
    const sig = new Float32Array(30000);
    for (let i = 0; i < sig.length; i++) sig[i] = 0.1 * rand();
    expect(crossCorrelationLag(sig, chirp(SR), 14400, 0).norm).toBeLessThan(0.3);
  });
});

describe('onsetIndex', () => {
  it('finds a burst onset within 2 ms, with noise', () => {
    const rand = rng(11);
    const on = 20000;
    const x = saw(60000, 220);
    for (let i = 0; i < x.length; i++) x[i] = (i >= on ? 0.4 * x[i] : 0) + 0.002 * rand();
    const got = onsetIndex(x, SR, 10000, 40000);
    expect(Math.abs(got - on) / SR).toBeLessThan(0.002);
  });

  it('works with a 10 ms attack ramp', () => {
    const on = 15000;
    const x = saw(40000, 220);
    for (let i = 0; i < x.length; i++) x[i] *= i < on ? 0 : Math.min(1, (i - on) / (0.01 * SR)) * 0.5;
    const got = onsetIndex(x, SR, 5000, 30000);
    expect(got).toBeGreaterThanOrEqual(on);
    expect((got - on) / SR).toBeLessThan(0.004);
  });

  it('returns -1 for silence', () => {
    expect(onsetIndex(new Float32Array(10000), SR, 0, 10000)).toBe(-1);
  });
});

describe('segmentBursts', () => {
  it('finds each hum burst and ignores short blips', () => {
    const hum = synthHum(SR, 220, 4);
    const x = hum.samples;
    for (let i = 0; i < 200; i++) x[100 + i] = 0.5; // blip before the first burst
    const b = segmentBursts(x, SR);
    expect(b.length).toBe(4);
    b.forEach((burst, k) => {
      expect(Math.abs(burst.start - hum.burstStarts[k]) / SR).toBeLessThan(0.02);
      expect(Math.abs((burst.end - burst.start) / SR - 0.6)).toBeLessThan(0.03);
    });
  });
});

describe('estimateF0', () => {
  it('is within 1% on 220 Hz sawtooth', () => {
    const x = saw(10000, 220);
    for (const s of [0, 137, 1000, 3333]) {
      const e = estimateF0(x, s, SR);
      expect(Math.abs(e.f0 / 220 - 1)).toBeLessThan(0.01);
      expect(e.clarity).toBeGreaterThan(0.9);
    }
  });

  it('is within 1% on the vowel hum, and across other pitches', () => {
    const hum = synthHum(SR, 220, 1);
    const e = estimateF0(hum.samples, hum.burstStarts[0] + 5000, SR);
    expect(Math.abs(e.f0 / 220 - 1)).toBeLessThan(0.01);
    for (const f of [110, 175, 330, 440]) {
      const r = estimateF0(saw(8000, f), 500, SR);
      expect(Math.abs(r.f0 / f - 1)).toBeLessThan(0.01);
    }
  });

  it('returns 0 for silence and for noise', () => {
    expect(estimateF0(new Float32Array(8000), 0, SR).f0).toBe(0);
    const rand = rng(5);
    const n = new Float32Array(8000).map(() => rand());
    expect(estimateF0(n, 0, SR).clarity).toBeLessThan(0.6);
  });

  it('medianF0 reads the burst pitch', () => {
    const hum = synthHum(SR, 220, 1);
    const f = medianF0(hum.samples, SR, hum.burstStarts[0] + 4800, hum.burstStarts[0] + 24000);
    expect(Math.abs(f / 220 - 1)).toBeLessThan(0.01);
  });
});

describe('pitchLockIndex', () => {
  it('finds when the output pitch settles to the target, within a few ms', () => {
    // 262 Hz for 60 ms, then 220 Hz.
    const switchAt = 15000;
    const n = 30000;
    const x = new Float32Array(n);
    let ph = 0;
    for (let i = 5000; i < n; i++) {
      ph += (i < switchAt ? 262 : 220) / SR;
      x[i] = 2 * (ph % 1) - 1;
    }
    const got = pitchLockIndex(x, SR, 5000, 25000, 220);
    expect(got).toBeGreaterThan(0);
    const errMs = ((got - switchAt) / SR) * 1000;
    expect(errMs).toBeGreaterThan(-4);
    expect(errMs).toBeLessThan(8);
  });

  it('returns -1 when the pitch never gets there', () => {
    expect(pitchLockIndex(saw(20000, 300), SR, 0, 8000, 220)).toBe(-1);
  });
});
