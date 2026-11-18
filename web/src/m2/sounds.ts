// Sound choices for the M2 capture page: the M0 Live presets as klont sounds, a few demo
// melody sounds, and the demo drum kits as the default 4-slot kit.

import type { Sound } from '../m1/layer';
import { layerDefs } from '../m1/demo';

/** Envelope for a voice that follows the singer: quick start, holds while sung, short tail. */
const follow: Sound = { 'env.attack': 0.01, 'env.decay': 0.3, 'env.sustain': 1, 'env.release': 0.12 };

export interface MelodySound { name: string; blurb: string; sound: Sound }

export const MELODY_SOUNDS: MelodySound[] = [
  { name: 'Choir pad', blurb: 'harmonic, gentle tilt', sound: { ...follow, 'src.tilt': -9, 'src.noise_mix': 0.1, 'src.f1_gain': 6, 'src.f1_freq': 600 } },
  { name: 'Reed', blurb: 'odd-heavy, bright', sound: { ...follow, 'src.oddeven': 0.8, 'src.tilt': -3, 'src.noise_mix': 0.1, 'src.f1_freq': 800,
    'src.f1_gain': 8, 'src.f2_freq': 2200, 'src.f2_gain': 6 } },
  { name: 'Glass', blurb: 'stretched, bright, some damp', sound: { ...follow, 'src.stretch': 0.006, 'src.tilt': 0, 'src.damp': 0.3,
    'src.noise_mix': 0.05, 'src.f2_freq': 3000, 'src.f2_gain': 6 } },
  { name: 'Buzz', blurb: 'flat tilt', sound: { ...follow, 'src.tilt': 0, 'src.noise_mix': 0.1 } },
  { name: 'Breath', blurb: 'mostly noise', sound: { ...follow, 'src.harm_mix': 0.1, 'src.noise_mix': 1, 'src.tilt': -12 } },
  { name: 'Bell', blurb: 'bar modal', sound: { ...follow, 'src.modal': 1, 'src.damp': 0.3, 'src.stretch': 0.004, 'src.tilt': -6, 'src.noise_mix': 0.05 } },
  { name: 'Demo bass', blurb: 'the M1 demo bass', sound: layerDefs[2].base ?? {} },
  { name: 'Demo lead', blurb: 'the M1 demo lead', sound: { ...layerDefs[4].base, 'env.sustain': 1 } },
];

export interface BeatKit { name: string; kit: Sound[]; kitCents: number[]; base: Sound }

export const BEAT_KITS: BeatKit[] = [
  { name: 'Drums: kick, snare, hats', kit: layerDefs[0].kit as Sound[], kitCents: layerDefs[0].kitCents!, base: layerDefs[0].base ?? {} },
  { name: 'Percussion: toms, clap, rim', kit: layerDefs[1].kit as Sound[], kitCents: layerDefs[1].kitCents!, base: layerDefs[1].base ?? {} },
];

/** Slot colours for hit markers and the live slot readout. */
export const SLOT_COLORS = ['#d9402b', '#2b8ad9', '#2bb35a', '#c98a12'];
export const SLOT_NAMES = ['kick', 'snare', 'hat', 'open'];
