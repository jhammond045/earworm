// The Influence match through the committed analysis.wasm, on small synthetic sounds.
import { readFileSync } from 'node:fs';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { COUNT, DESCS, P, toReal } from '../gen/params';
import {
  FLAG_DECAYING, FLAG_NO_SOUND, FLAG_USE_SOUND, FULL_EXTRA, FULL_MATCH_LEN, MatchCore, decodeMatch, liveMatch, matchFlags,
  type Match, type MatchSource,
} from './match';

const SR = 48000;
const core = MatchCore.fromBytes(readFileSync(new URL('../../public/wasm/analysis.wasm', import.meta.url)));
const cents = (hz: number) => 1200 * Math.log2(hz / 16.351598);
const real = (m: Match, id: keyof typeof P) => toReal(P[id], m.target[P[id]]);

function rng(seed: number) {
  let s = seed >>> 0;
  return () => {
    s = (Math.imul(s, 1664525) + 1013904223) >>> 0;
    return s / 2 ** 32 - 0.5;
  };
}

/** A 220 Hz tone with a few harmonics, `secs` long, 20 ms fades. */
function tone(secs: number, hz = 220): Float32Array {
  const n = Math.round(secs * SR);
  const x = new Float32Array(n);
  for (let i = 0; i < n; i++) {
    const t = i / SR;
    const env = Math.min(1, t / 0.02, (secs - t) / 0.02);
    let v = 0;
    for (let k = 1; k <= 6; k++) v += Math.sin(2 * Math.PI * hz * k * t) / k;
    x[i] = 0.2 * env * v;
  }
  return x;
}

function hiss(secs: number): Float32Array {
  const r = rng(7);
  const x = new Float32Array(Math.round(secs * SR));
  for (let i = 0; i < x.length; i++) x[i] = 0.3 * r();
  return x;
}

describe('layout', () => {
  it('matches the Rust constants', () => {
    const src = readFileSync(new URL('../../../crates/analysis/src/full.rs', import.meta.url), 'utf8');
    expect(Number(/pub const FULL_EXTRA: usize = (\d+);/.exec(src)![1])).toBe(FULL_EXTRA);
    expect(FULL_MATCH_LEN).toBe(COUNT + FULL_EXTRA);
    expect(Number(/pub const FLAG_DECAYING: u32 = (\d+);/.exec(src)![1])).toBe(FLAG_DECAYING);
    expect(Number(/pub const FLAG_USE_SOUND: u32 = (\d+);/.exec(src)![1])).toBe(FLAG_USE_SOUND);
    expect(Number(/pub const FLAG_NO_SOUND: u32 = (\d+);/.exec(src)![1])).toBe(FLAG_NO_SOUND);
  });

  it('decodes by hand', () => {
    const f = new Float32Array(FULL_MATCH_LEN);
    f.fill(0.5, 0, COUNT);
    f.set([1, 4500, 1.25, 0.9, 1, 0, 480, 24000, NaN, 1], COUNT);
    const m = decodeMatch(f);
    expect(m.target.length).toBe(COUNT);
    expect(m).toMatchObject({ pitched: true, pitchCents: 4500, durationS: 1.25, provisional: true, useSound: false });
    expect(m.grain).toEqual({ start: 480, end: 24000, refCents: null });
    f[COUNT + 8] = 4500;
    f[COUNT + 9] = 0;
    expect(decodeMatch(f).grain).toBeNull();
    expect(() => decodeMatch(new Float32Array(10))).toThrow();
  });

  it('turns options into flags', () => {
    expect(matchFlags()).toBe(0);
    expect(matchFlags({ decaying: true })).toBe(FLAG_DECAYING);
    expect(matchFlags({ useSound: true })).toBe(FLAG_USE_SOUND);
    expect(matchFlags({ useSound: false })).toBe(FLAG_NO_SOUND);
    expect(matchFlags({ decaying: true, useSound: false })).toBe(FLAG_DECAYING | FLAG_NO_SOUND);
  });
});

describe('MatchCore', () => {
  it('matches a pitched tone', () => {
    const m = core.match(tone(1.2), SR);
    expect(m.target.length).toBe(COUNT);
    for (const v of m.target) expect(v >= 0 && v <= 1).toBe(true);
    expect(m.pitched).toBe(true);
    expect(Math.abs(m.pitchCents - cents(220))).toBeLessThan(10);
    expect(m.useSound).toBe(false);
    expect(real(m, 'src.grain_mix')).toBe(0);
    // Harmonics 1/k: a tilt of about -6 dB/oct.
    expect(Math.abs(real(m, 'src.tilt') + 6)).toBeLessThan(1.5);
    expect(m.grain).not.toBeNull();
    expect(Math.abs(m.grain!.refCents! - cents(220))).toBeLessThan(10);
    expect(m.grain!.end).toBeGreaterThan(m.grain!.start);
    expect(m.ms).toBeGreaterThan(0);
  });

  it('matches noise as unpitched and uses the sound by default', () => {
    const m = core.match(hiss(1.2), SR);
    expect(m.pitched).toBe(false);
    expect(m.useSound).toBe(true);
    expect(real(m, 'src.grain_mix')).toBeCloseTo(0.75, 2);
    expect(real(m, 'src.harm_mix')).toBe(0);
    expect(m.grain!.refCents).toBeNull();
    expect(core.match(hiss(1.2), SR, { useSound: false }).useSound).toBe(false);
  });

  it('can force use_sound on a pitched sound', () => {
    const m = core.match(tone(1.2), SR, { useSound: true });
    expect(m.useSound).toBe(true);
    expect(real(m, 'src.grain_mix')).toBeCloseTo(0.75, 2);
    expect(real(m, 'src.harm_mix')).toBeCloseTo(0.25, 2);
  });

  it('survives silence and very short input', () => {
    for (const x of [new Float32Array(0), new Float32Array(1000), new Float32Array(48000)]) {
      const m = core.match(x, SR);
      expect(m.target.length).toBe(COUNT);
      expect(m.target.every((v) => Number.isFinite(v))).toBe(true);
    }
    const neutral = core.match(new Float32Array(48000), SR);
    for (let i = 0; i < COUNT; i++) expect(toReal(i, neutral.target[i])).toBeCloseTo(DESCS[i].neutral, 3);
  });

  it('finds a vibrato in a sustained tone', () => {
    const n = 2 * SR;
    const x = new Float32Array(n);
    let ph = 0;
    for (let i = 0; i < n; i++) {
      const t = i / SR;
      ph += (2 * Math.PI * 200 * 2 ** ((30 / 1200) * Math.sin(2 * Math.PI * 5 * t))) / SR;
      x[i] = 0.3 * Math.min(1, t / 0.05, (2 - t) / 0.05) * (Math.sin(ph) + 0.4 * Math.sin(2 * ph));
    }
    const m = core.match(x, SR);
    expect(Math.abs(real(m, 'lfo1.rate') - 5)).toBeLessThan(0.7);
    expect(real(m, 'lfo1.to_pitch')).toBeGreaterThan(15);
    expect(real(m, 'lfo1.to_pitch')).toBeLessThan(45);
    expect(m.provisional).toBe(false);
  });
});

describe('liveMatch', () => {
  afterEach(() => vi.useRealTimers());

  /** A match source that answers after `delay` ms of fake time and records its inputs. */
  function fakeMatcher(delay: number, calls: number[]): MatchSource {
    return {
      match: (signal) =>
        new Promise((resolve) => {
          calls.push(signal.length);
          setTimeout(() => resolve(decodeMatch(new Float32Array(FULL_MATCH_LEN), 0)), delay);
        }),
    };
  }

  it('reruns every 250 ms on what has been captured so far', async () => {
    vi.useFakeTimers();
    let len = 0;
    const calls: number[] = [];
    const got: Match[] = [];
    const live = liveMatch(() => new Float32Array(len), fakeMatcher(0, calls), (m) => got.push(m), { sampleRate: SR });
    for (let k = 0; k < 4; k++) {
      len += 12000; // 250 ms of audio per tick
      await vi.advanceTimersByTimeAsync(250);
    }
    await vi.advanceTimersByTimeAsync(1); // the last answer arrives
    expect(calls).toEqual([12000, 24000, 36000, 48000]);
    expect(got.length).toBe(4);
    live.stop();
    await vi.advanceTimersByTimeAsync(1000);
    expect(calls.length).toBe(4);
  });

  it('does not overlap runs and skips ticks with nothing new', async () => {
    vi.useFakeTimers();
    let len = 24000;
    const calls: number[] = [];
    const live = liveMatch(() => new Float32Array(len), fakeMatcher(600, calls), () => {}, { sampleRate: SR });
    await vi.advanceTimersByTimeAsync(250); // first run starts, takes 600 ms
    await vi.advanceTimersByTimeAsync(500); // ticks at 500 and 750 are skipped while it runs
    expect(calls).toEqual([24000]);
    await vi.advanceTimersByTimeAsync(250); // done at 850; tick at 1000 has no new audio
    await vi.advanceTimersByTimeAsync(250);
    expect(calls).toEqual([24000]);
    len = 36000;
    await vi.advanceTimersByTimeAsync(250);
    expect(calls).toEqual([24000, 36000]);
    live.stop();
  });

  it('waits for 100 ms of sound, and finish runs once over everything', async () => {
    vi.useFakeTimers();
    let len = 2000;
    const calls: number[] = [];
    const got: Match[] = [];
    const live = liveMatch(() => new Float32Array(len), fakeMatcher(0, calls), (m) => got.push(m), { sampleRate: SR });
    await vi.advanceTimersByTimeAsync(250);
    expect(calls).toEqual([]);
    len = 5000;
    const fin = live.finish();
    await vi.advanceTimersByTimeAsync(0);
    const m = await fin;
    expect(calls).toEqual([5000]);
    expect(got.at(-1)).toBe(m);
    await vi.advanceTimersByTimeAsync(1000);
    expect(calls).toEqual([5000]);
  });

  it('keeps going after a failed run', async () => {
    vi.useFakeTimers();
    let len = 12000;
    let fail = true;
    const errors: Error[] = [];
    const got: Match[] = [];
    const m: MatchSource = {
      match: async () => {
        if (fail) throw new Error('boom');
        return decodeMatch(new Float32Array(FULL_MATCH_LEN));
      },
    };
    const live = liveMatch(() => new Float32Array(len), m, (x) => got.push(x), { sampleRate: SR, onError: (e) => errors.push(e) });
    await vi.advanceTimersByTimeAsync(250);
    expect(errors.map((e) => e.message)).toEqual(['boom']);
    fail = false;
    len = 24000;
    await vi.advanceTimersByTimeAsync(250);
    expect(got.length).toBe(1);
    live.stop();
  });

  it('works end to end on the real wasm over a growing capture', async () => {
    vi.useFakeTimers();
    const full = tone(1.5);
    let len = 0;
    const real: MatchSource = { match: async (s, sr, o) => core.match(s, sr, o) };
    const got: Match[] = [];
    const live = liveMatch(() => full.subarray(0, len), real, (m) => got.push(m), { sampleRate: SR });
    for (let k = 0; k < 6; k++) {
      len = Math.min(full.length, len + 12000);
      await vi.advanceTimersByTimeAsync(250);
    }
    expect(got.length).toBe(6);
    const final = await live.finish();
    expect(final.pitched).toBe(true);
    expect(Math.abs(final.pitchCents - cents(220))).toBeLessThan(10);
  });
});
