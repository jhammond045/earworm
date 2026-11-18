// What the Influence panel shows about a target: how far each aspect group moved from the base, and a few
// parameters by name with their real-unit value. Pure, so it is tested without a page.

import { COUNT, DESCS, P, toReal, type Group } from '../gen/params';

/** Aspect groups in the order of the mask bits (commands.ts GROUP_BIT). */
export const ASPECTS: { group: Group; label: string; bit: number }[] = [
  { group: 'tone', label: 'Tone', bit: 1 },
  { group: 'shape', label: 'Shape', bit: 2 },
  { group: 'motion', label: 'Motion', bit: 4 },
  { group: 'grit', label: 'Grit', bit: 8 },
  { group: 'space', label: 'Space', bit: 16 },
];

/** Mean of (target - base) over the parameters of each aspect group, each -1..1 (normalized units). */
export function groupDeltas(target: ArrayLike<number>, base: ArrayLike<number>): number[] {
  const sum = ASPECTS.map(() => 0);
  const n = ASPECTS.map(() => 0);
  for (let i = 0; i < COUNT; i++) {
    const a = ASPECTS.findIndex((x) => x.group === DESCS[i].group);
    if (a < 0) continue;
    sum[a] += target[i] - base[i];
    n[a]++;
  }
  return sum.map((s, a) => (n[a] ? s / n[a] : 0));
}

/** The parameters the readout names, with the plain label to show. */
export const NAMED: { id: keyof typeof P; label: string }[] = [
  { id: 'src.tilt', label: 'Tilt' },
  { id: 'src.noise_mix', label: 'Noise mix' },
  { id: 'env.attack', label: 'Attack' },
  { id: 'env.decay', label: 'Decay' },
  { id: 'filt.cutoff', label: 'Filter cutoff' },
];

/** A normalized value as a short real-unit string, for example "-9.5 dB/oct" or "1.2 kHz". */
export function formatParam(index: number, norm: number): string {
  const d = DESCS[index];
  const v = toReal(index, norm);
  if (d.unit === 'Hz' && Math.abs(v) >= 1000) return `${(v / 1000).toFixed(2)} kHz`;
  if (d.unit === 's' && Math.abs(v) < 1) return `${(v * 1000).toFixed(0)} ms`;
  const digits = Math.abs(v) >= 100 ? 0 : Math.abs(v) >= 10 ? 1 : 2;
  return `${v.toFixed(digits)}${d.unit ? ` ${d.unit}` : ''}`;
}

export interface Readout {
  groups: { label: string; delta: number }[];
  named: { label: string; value: string; base: string; moved: number }[];
}

/** Everything the readout draws, from a target and the base it is measured against. */
export function readoutOf(target: ArrayLike<number>, base: ArrayLike<number>): Readout {
  const d = groupDeltas(target, base);
  return {
    groups: ASPECTS.map((a, i) => ({ label: a.label, delta: d[i] })),
    named: NAMED.map((n) => {
      const i = P[n.id];
      return { label: n.label, value: formatParam(i, target[i]), base: formatParam(i, base[i]), moved: target[i] - base[i] };
    }),
  };
}

/** Number of mask bits set, for the status line. */
export const maskCount = (mask: number): number => ASPECTS.filter((a) => mask & a.bit).length;
