import { describe, expect, it } from 'vitest';
import { decodeMatch } from './match';
import { MATCH_DURATION, MATCH_LEN, MATCH_NOISE, MATCH_PITCH, MATCH_PITCHED, MATCH_TONE, MATCH_VOICED, MAX_BANDS, PARAMS } from '../host/protocol';

describe('decodeMatch', () => {
  it('splits the flat layout into its parts', () => {
    const f = new Float32Array(MATCH_LEN);
    for (let i = 0; i < PARAMS.length; i++) f[i] = i + 1;
    for (let i = 0; i < MAX_BANDS; i++) {
      f[MATCH_TONE + i] = 100 + i;
      f[MATCH_NOISE + i] = 200 + i;
    }
    f[MATCH_PITCH] = 6000;
    f[MATCH_DURATION] = 1.5;
    f[MATCH_VOICED] = 0.75;
    f[MATCH_PITCHED] = 1;
    const m = decodeMatch(f, 12);
    expect(m.scalars.length).toBe(PARAMS.length);
    expect(m.scalars[0]).toBe(1);
    expect(m.scalars[PARAMS.length - 1]).toBe(PARAMS.length);
    expect(m.tone.length).toBe(MAX_BANDS);
    expect(m.tone[0]).toBe(100);
    expect(m.tone[MAX_BANDS - 1]).toBe(100 + MAX_BANDS - 1);
    expect(m.noise[0]).toBe(200);
    expect(m.noise[MAX_BANDS - 1]).toBe(200 + MAX_BANDS - 1);
    expect(m.pitchCents).toBe(6000);
    expect(m.durationS).toBe(1.5);
    expect(m.voicedFraction).toBe(0.75);
    expect(m.pitched).toBe(true);
    expect(m.ms).toBe(12);
  });

  it('reads pitched as false when the flag is 0', () => {
    expect(decodeMatch(new Float32Array(MATCH_LEN)).pitched).toBe(false);
  });

  it('rejects a short result', () => {
    expect(() => decodeMatch(new Float32Array(MATCH_LEN - 1))).toThrow();
  });
});
