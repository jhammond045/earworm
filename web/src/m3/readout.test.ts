import { describe, expect, it } from 'vitest';
import { COUNT, DESCS, P, neutralSet } from '../gen/params';
import { ASPECTS, formatParam, groupDeltas, maskCount, readoutOf } from './readout';

describe('readout', () => {
  it('has no change when the target is the base', () => {
    const b = Array.from(neutralSet());
    expect(groupDeltas(b, b)).toEqual([0, 0, 0, 0, 0]);
  });

  it('a group moves only when its own parameters move, as a mean', () => {
    const b = Array.from(neutralSet());
    const t = b.slice();
    let nGrit = 0;
    DESCS.forEach((d, i) => { if (d.group === 'grit') { t[i] = b[i] + 0.2; nGrit++; } });
    const d = groupDeltas(t, b);
    expect(d[ASPECTS.findIndex((a) => a.group === 'grit')]).toBeCloseTo(0.2, 6);
    expect(d.filter((x) => Math.abs(x) > 1e-9)).toHaveLength(1);
    expect(nGrit).toBeGreaterThan(0);
  });

  it('a lower target gives a negative change', () => {
    const b = new Array(COUNT).fill(0.5);
    const t = new Array(COUNT).fill(0.25);
    for (const x of groupDeltas(t, b)) expect(x).toBeCloseTo(-0.25, 6);
  });

  it('names five parameters with real units', () => {
    const b = Array.from(neutralSet());
    const r = readoutOf(b, b);
    expect(r.named.map((n) => n.label)).toEqual(['Tilt', 'Noise mix', 'Attack', 'Decay', 'Filter cutoff']);
    expect(r.named[0].value).toBe('-6.00 dB/oct');
    expect(r.named.every((n) => n.moved === 0)).toBe(true);
  });

  it('formats Hz and seconds in friendlier units', () => {
    expect(formatParam(P['filt.cutoff'], 1)).toMatch(/kHz$/);
    expect(formatParam(P['env.attack'], 0)).toMatch(/ms$/);
  });

  it('counts mask bits', () => {
    expect(maskCount(0x1f)).toBe(5);
    expect(maskCount(0)).toBe(0);
    expect(maskCount(5)).toBe(2);
  });
});
