import { describe, expect, it } from 'vitest';
import { GATE_PCT, buildReport, makeTrials, mulberry32, outcomeFor, score, shuffle, useSoundFor } from './test-trials';

describe('trials', () => {
  it('gives every source one trial with the true source among four distinct candidates', () => {
    const trials = makeTrials(20, 42);
    expect(trials).toHaveLength(20);
    expect(new Set(trials.map((t) => t.target)).size).toBe(20);
    for (const t of trials) {
      expect(t.candidates).toHaveLength(4);
      expect(new Set(t.candidates).size).toBe(4);
      expect(t.candidates).toContain(t.target);
    }
  });

  it('repeats for the same seed and differs for another', () => {
    expect(makeTrials(8, 7)).toEqual(makeTrials(8, 7));
    expect(makeTrials(8, 7)).not.toEqual(makeTrials(8, 8));
  });

  it('works with exactly four sources and refuses fewer', () => {
    const t = makeTrials(4, 1);
    for (const x of t) expect([...x.candidates].sort()).toEqual([0, 1, 2, 3]);
    expect(() => makeTrials(3, 1)).toThrow(RangeError);
  });

  it('puts the true source in every position about equally', () => {
    const counts = [0, 0, 0, 0];
    for (let seed = 0; seed < 50; seed++) for (const t of makeTrials(20, seed)) counts[t.candidates.indexOf(t.target)]++;
    for (const c of counts) expect(c / 1000).toBeGreaterThan(0.2);
  });

  it('shuffles without losing items and does not touch the input', () => {
    const a = [1, 2, 3, 4, 5];
    const s = shuffle(a, mulberry32(3));
    expect([...s].sort()).toEqual(a);
    expect(a).toEqual([1, 2, 3, 4, 5]);
  });
});

describe('scoring', () => {
  const items = ['a', 'b', 'c', 'd', 'e'].map((name) => ({ name }));
  const trial = { target: 2, candidates: [0, 2, 3, 4] };

  it('marks a pick right or wrong', () => {
    expect(outcomeFor(items, trial, 1)).toEqual({ source: 'c', chosen: 'c', correct: true, position: 'B' });
    expect(outcomeFor(items, trial, 3)).toEqual({ source: 'c', chosen: 'e', correct: false, position: 'D' });
  });

  it('applies the 70% line', () => {
    const mk = (right: number, total: number) => Array.from({ length: total }, (_, i) => ({ source: 's', chosen: 's', correct: i < right, position: 'A' }));
    expect(score(mk(14, 20), 20)).toMatchObject({ right: 14, total: 20, percent: 70, atGate: true, short: false });
    expect(score(mk(13, 20), 20).atGate).toBe(false);
    expect(score(mk(5, 5), 5)).toMatchObject({ atGate: true, short: true });
    expect(score([], 0)).toMatchObject({ total: 0, percent: 0, atGate: false });
    expect(GATE_PCT).toBe(70);
  });

  it('maps the sound toggle', () => {
    expect(useSoundFor('default')).toBeUndefined();
    expect(useSoundFor('off')).toBe(false);
    expect(useSoundFor('on')).toBe(true);
  });

  it('builds the report', () => {
    const src = ['a', 'b', 'c', 'd'].map((name) => ({ name, sampleRate: 48000, seconds: 1, pitched: true, pitchCents: 4800, useSound: false, matchMs: 5, renderMs: 50 }));
    const trials = makeTrials(4, 9);
    const outcomes = trials.map((t) => outcomeFor(src, t, t.candidates.indexOf(t.target)));
    const r = buildReport({ seed: 9, soundMode: 'default', sources: src, trials, outcomes, renderRate: 48000, releaseS: 1, when: new Date(0) });
    expect(r).toMatchObject({ engine: 'klont M4', seed: 9, right: 4, total: 4, percent: 100, atGate: true, gateRun: false, finishedAt: '1970-01-01T00:00:00.000Z' });
    expect(r.trials[0].candidates).toHaveLength(4);
  });
});
