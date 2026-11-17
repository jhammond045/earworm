// Sound presets for the Live follow prototype. Each is a complete set of the
// "sound" scalars (not the voice depths, tuning or level, which stay where the
// singer put them). Band shapes are always reset to flat (0 dB).

import { PARAMS, P } from '../host/protocol';

/** Parameters a preset owns. */
export const SOUND_PARAMS = [
  'harm_mix', 'noise_mix', 'tilt', 'oddeven', 'stretch', 'modal', 'damp',
  'f1_freq', 'f1_gain', 'f2_freq', 'f2_gain', 'partials',
] as const;

export interface Preset {
  name: string;
  blurb: string;
  /** Only the values that differ from the parameter defaults. */
  set: Record<string, number>;
}

export const PRESETS: Preset[] = [
  { name: 'Choir pad', blurb: 'harmonic, gentle tilt', set: { tilt: -9, noise_mix: 0.1, partials: 32, f1_gain: 6, f1_freq: 600 } },
  { name: 'Reed', blurb: 'odd-heavy, bright', set: { oddeven: 0.8, tilt: -3, noise_mix: 0.1, f1_freq: 800, f1_gain: 8, f2_freq: 2200, f2_gain: 6, partials: 40 } },
  { name: 'Glass', blurb: 'stretched, bright, some damp', set: { stretch: 0.006, tilt: 0, damp: 0.3, noise_mix: 0.05, partials: 20, f2_freq: 3000, f2_gain: 6 } },
  { name: 'Buzz', blurb: 'flat tilt, many partials', set: { tilt: 0, noise_mix: 0.1, partials: 64 } },
  { name: 'Breath', blurb: 'mostly noise', set: { harm_mix: 0.1, noise_mix: 1, tilt: -12, partials: 16 } },
  { name: 'Bell', blurb: 'bar modal', set: { modal: 1, damp: 0.3, stretch: 0.004, tilt: -6, noise_mix: 0.05, partials: 24 } },
];

/** Full value for each sound param of a preset (defaults filled in). */
export function presetValues(p: Preset): Record<string, number> {
  const out: Record<string, number> = {};
  for (const name of SOUND_PARAMS) out[name] = p.set[name] ?? PARAMS[P[name]].default;
  return out;
}
