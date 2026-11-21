// Normalized <-> real units for any value descriptor (the Klont vocabulary and the module table's
// own), as crates/params/src/lib.rs `to_real` and `to_norm` do.

import { ALL_DESCS, STEPS } from '../gen/modules';

const clamp01 = (x: number): number => (x < 0 ? 0 : x > 1 ? 1 : x);

export function toReal(desc: number, n: number): number {
  const d = ALL_DESCS[desc];
  if (Number.isNaN(n)) return d.neutral;
  n = clamp01(n);
  let r = d.scale === 'log' ? d.min * Math.pow(d.max / d.min, n) : d.min + (d.max - d.min) * n;
  const steps = STEPS[desc];
  if (steps >= 2) {
    const step = (d.max - d.min) / (steps - 1);
    r = d.min + Math.round((r - d.min) / step) * step;
  }
  return Math.min(d.max, Math.max(d.min, r));
}

export function toNorm(desc: number, real: number): number {
  const d = ALL_DESCS[desc];
  if (Number.isNaN(real)) real = d.neutral;
  if (d.scale === 'log') return real > 0 ? clamp01(Math.log(real / d.min) / Math.log(d.max / d.min)) : 0;
  return clamp01((real - d.min) / (d.max - d.min));
}
