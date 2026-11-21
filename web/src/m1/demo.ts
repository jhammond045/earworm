// The 8-layer demo song as data. A minor, two grid sections (120 bpm, then
// 132 bpm after a 4 s tempo ramp), four repeats each, track loop on. It keeps
// pad chords, bells and texture ringing over drums, bass, lead and arpeggio,
// so it doubles as the "8 layers playing" gate for the benchmark.
// `pnpm gen:demo` writes the encoded blobs to web/public/m1/demo for the
// native bench (crates/klont/src/bin/bench.rs).

import { PPQ, encodeTrack, type TrackClip, type TrackEvent, type TrackInput } from './track';
import { encodeLayer, type LayerDef, type Sound } from './layer';

export const LAYER_NAMES = ['Drums', 'Percussion', 'Bass', 'Pad', 'Lead', 'Arpeggio', 'Bells', 'Texture'] as const;

const BAR = 4 * PPQ;
const SECTION_BARS = 4;
const SECTION_LEN = SECTION_BARS * BAR;

/** Cents above C0 for a MIDI note. */
const cents = (midi: number) => (midi - 12) * 100;

/** Note at `beat` (from the clip start) lasting `dur` beats. */
function note(beat: number, dur: number, midi: number, strength = 1): TrackEvent {
  return { t: beat * PPQ, dur: dur * PPQ, cents: cents(midi), strength };
}
function hit(beat: number, slot: number, strength = 1): TrackEvent {
  return { t: beat * PPQ, dur: PPQ / 4, slot, strength };
}

// Chords as MIDI notes, one per bar.
const Am = [57, 60, 64, 69], F = [53, 57, 60, 65], C = [60, 64, 67, 72], G = [55, 59, 62, 67], Em = [52, 59, 64, 67];
const PROG_A = [Am, F, C, G];
const PROG_B = [F, G, Am, Em];
const rootOf = (c: number[]) => c[0] - 24; // bass: two octaves under the chord's lowest note

// ---------------------------------------------------------------- sounds

const noiseBand = (lo: number, hi: number, dbLow: number, dbHigh: number): Sound => {
  const s: Sound = {};
  for (let i = 0; i < 16; i++) s[`noise_bands.${i}`] = i < lo ? dbLow : i >= hi ? dbHigh : 0;
  return s;
};

const kick: Sound = { 'src.tilt': -14, 'src.noise_mix': 0.02, 'env.decay': 0.28, 'env.sustain': 0, 'env.release': 0.05,
  'env.pitch_amt': 30, 'env.pitch_time': 0.035, 'strike.click': 0.25, 'filt.cutoff': 1500 };
const snare: Sound = { 'src.tilt': -8, 'src.modal': 1, 'src.harm_mix': 0.55, 'src.noise_mix': 0.9, 'env.decay': 0.11,
  'env.sustain': 0, 'env.release': 0.05, 'env.noise_decay': 0.22, 'strike.click': 0.3, ...noiseBand(3, 16, -12, 0) };
const hatClosed: Sound = { 'src.harm_mix': 0.05, 'src.noise_mix': 1, 'env.decay': 0.04, 'env.sustain': 0, 'env.release': 0.02,
  'env.noise_decay': 0.045, ...noiseBand(9, 16, -30, 6) };
const hatOpen: Sound = { ...hatClosed, 'env.decay': 0.3, 'env.noise_decay': 0.32, 'env.release': 0.1 };
const tom = (decay: number): Sound => ({ 'src.tilt': -10, 'src.modal': 1, 'src.noise_mix': 0.05, 'env.decay': decay,
  'env.sustain': 0, 'env.release': 0.05, 'env.pitch_amt': 7, 'env.pitch_time': 0.06, 'strike.click': 0.15 });
const clap: Sound = { 'src.harm_mix': 0.05, 'src.noise_mix': 1, 'env.attack': 0.004, 'env.decay': 0.14, 'env.sustain': 0,
  'env.release': 0.05, 'env.noise_decay': 0.16, 'strike.click': 0.6, ...noiseBand(4, 14, -20, -6) };
const rim: Sound = { 'src.modal': 2, 'src.tilt': -4, 'src.noise_mix': 0.1, 'clang.amt': 0.5, 'env.decay': 0.05,
  'env.sustain': 0, 'env.release': 0.03, 'strike.click': 0.5 };

export const layerDefs: LayerDef[] = [
  { kind: 'beat', level: -6, kitCents: [2100, 5500, 9000, 9000], kit: [kick, snare, hatClosed, hatOpen],
    base: { 'verb.mix': 0.08, 'verb.size': 0.3 } },
  { kind: 'beat', level: -10, pan: 0.2, kitCents: [4500, 5400, 8000, 7800], kit: [tom(0.35), tom(0.25), clap, rim],
    base: { 'verb.mix': 0.15, 'verb.size': 0.4 } },
  { kind: 'melody', level: -7, base: { 'src.tilt': -9, 'src.noise_mix': 0.02, 'env.attack': 0.006, 'env.decay': 0.3, 'env.sustain': 0.7,
    'env.release': 0.12, 'fold.amt': 0.25, 'filt.cutoff': 1100, 'filt.res': 0.2, 'filt.track': 0.3 } },
  { kind: 'melody', level: -11, base: { 'src.tilt': -9, 'src.oddeven': 0.2, 'src.jitter': 8, 'src.noise_mix': 0.03, 'env.attack': 0.6,
    'env.decay': 1.5, 'env.sustain': 0.8, 'env.release': 0.15, 'filt.cutoff': 2800, 'lfo2.rate': 0.3, 'lfo2.to_pitch': 6,
    'verb.mix': 0.45, 'verb.size': 0.85, 'verb.damp': 0.6, width: 1.6 } },
  { kind: 'melody', level: -7, pan: 0.1, base: { 'src.tilt': -5, 'src.noise_mix': 0.03, 'src.f1_gain': 6, 'env.attack': 0.02,
    'env.decay': 0.4, 'env.sustain': 0.6, 'env.release': 0.25, 'lfo1.rate': 5.5, 'lfo1.to_pitch': 18, 'delay.time': 0.375,
    'delay.fb': 0.35, 'delay.mix': 0.28, 'verb.mix': 0.12 } },
  { kind: 'melody', level: -5, pan: -0.3, base: { 'src.tilt': -6, 'src.noise_mix': 0.02, 'comb.amt': 0.4, 'comb.tune': 2, 'comb.decay': 0.15,
    'env.attack': 0.002, 'env.decay': 0.12, 'env.sustain': 0, 'env.release': 0.04, 'delay.mix': 0.12, 'delay.time': 0.25 } },
  { kind: 'melody', level: -10, pan: 0.35, base: { 'src.modal': 2, 'src.damp': 0.4, 'src.tilt': -4, 'src.noise_mix': 0.02,
    'env.decay': 1.5, 'env.sustain': 0, 'env.release': 0.3, 'strike.click': 0.1, 'verb.mix': 0.5, 'verb.size': 0.8 } },
  { kind: 'texture', level: -11, pan: -0.2, base: { 'src.harm_mix': 0.5, 'src.noise_mix': 0.8, 'src.tilt': -8, 'env.attack': 1.5,
    'env.decay': 2, 'env.sustain': 0.8, 'env.release': 2.5, 'env.noise_decay': 4, 'tape.amt': 0.5, 'crush.bits': 10,
    'crush.rate': 12000, 'verb.mix': 0.3, ...noiseBand(8, 16, 0, -24) } },
];

// ---------------------------------------------------------------- patterns

const at = (bar: number, evs: TrackEvent[]): TrackEvent[] => evs.map((e) => ({ ...e, t: e.t + bar * BAR }));

function drums(): TrackEvent[] {
  const out: TrackEvent[] = [];
  for (let b = 0; b < SECTION_BARS; b++) {
    const fill = b === SECTION_BARS - 1;
    const e: TrackEvent[] = [hit(0, 0), hit(1, 1), hit(1.5, 0, 0.8), hit(2.5, 0, 0.9)];
    for (let h = 0; h < 8; h++) e.push(hit(h * 0.5, 2, h % 2 === 0 ? 0.8 : 0.5));
    if (fill) e.push(hit(3, 1), hit(3.25, 1, 0.6), hit(3.5, 1, 0.7), hit(3.75, 1, 0.9));
    else e.push(hit(3, 1), hit(3.5, 3, 0.7));
    out.push(...at(b, e));
  }
  return out;
}

function percussion(sectionB: boolean): TrackEvent[] {
  const out: TrackEvent[] = [];
  for (let b = 0; b < SECTION_BARS; b++) {
    const e: TrackEvent[] = [hit(1, 2, 0.8), hit(3, 2, 0.8), hit(0.75, 3, 0.5), hit(2.75, 3, 0.4)];
    if (b === 3) e.push(hit(2, 0, 0.8), hit(2.5, 1, 0.8), hit(3.25, 0, 0.7), hit(3.5, 1, 0.8));
    if (sectionB && b % 2 === 1) e.push(hit(1.75, 3, 0.5));
    out.push(...at(b, e));
  }
  return out;
}

function bass(prog: number[][]): TrackEvent[] {
  return prog.flatMap((c, b) => {
    const r = rootOf(c);
    return at(b, [note(0, 1.4, r, 1), note(1.5, 0.45, r + 12, 0.7), note(2, 1.4, r, 0.9), note(3.5, 0.45, r + 7, 0.7)]);
  });
}

function pad(prog: number[][]): TrackEvent[] {
  return prog.flatMap((c, b) => at(b, c.map((m) => note(0, 3.8, m, 0.8))));
}

function arpeggio(prog: number[][]): TrackEvent[] {
  const order = [0, 1, 2, 3, 2, 1, 2, 3];
  return prog.flatMap((c, b) => at(b, Array.from({ length: 16 }, (_, i) => note(i * 0.25, 0.22, c[order[i % 8]] + 12, i % 4 === 0 ? 0.8 : 0.55))));
}

const LEAD_A: TrackEvent[] = [
  note(0, 1.5, 76), note(1.5, 0.5, 74), note(2, 1, 72), note(3, 0.9, 69),
  note(4, 1, 72), note(5, 1, 69), note(6, 1, 72), note(7, 1, 77),
  note(8, 1.5, 76), note(9.5, 0.5, 79), note(10, 1, 76), note(11, 0.9, 74),
  note(12, 1.5, 74), note(13.5, 0.5, 71), note(14, 1, 67), note(15, 1, 71),
].map((e) => ({ ...e, strength: 0.85 }));
const LEAD_B: TrackEvent[] = [
  note(0, 1, 81), note(1, 0.5, 79), note(1.5, 0.5, 77), note(2, 2, 76),
  note(4, 1, 79), note(5, 1, 74), note(6, 1.5, 71), note(7.5, 0.5, 74),
  note(8, 1.5, 81), note(9.5, 0.5, 76), note(10, 1, 72), note(11, 1, 76),
  note(12, 1, 71), note(13, 1, 76), note(14, 2, 79),
].map((e) => ({ ...e, strength: 0.85 }));

const BELLS_A = [note(0, 1.5, 81), note(1.5, 1.5, 88), note(3, 1.5, 84), note(4.5, 1.5, 79)];
const BELLS_B = [note(0, 1.5, 84), note(1, 1.5, 79), note(2.5, 1.5, 88), note(4, 1.5, 81), note(5.5, 1.5, 91)];

const TEXTURE_A = [note(0, 7.8, 57, 0.9), note(8, 7.8, 64, 0.9)];
const TEXTURE_B = [note(0, 7.8, 53, 0.9), note(8, 7.8, 55, 0.9)];

function clips(section: number, b: boolean): TrackClip[] {
  const prog = b ? PROG_B : PROG_A;
  return [
    { section, layer: 0, loopLen: SECTION_LEN, events: drums() },
    { section, layer: 1, loopLen: SECTION_LEN, events: percussion(b) },
    { section, layer: 2, loopLen: SECTION_LEN, events: bass(prog) },
    { section, layer: 3, loopLen: SECTION_LEN, events: pad(prog) },
    { section, layer: 4, loopLen: SECTION_LEN, events: b ? LEAD_B : LEAD_A },
    { section, layer: 5, loopLen: SECTION_LEN, events: arpeggio(prog) },
    // Uneven loops: the bell phrase is 6 (or 7) beats in a 16-beat section, so it re-fires across the chord changes.
    { section, layer: 6, loopLen: (b ? 7 : 6) * PPQ, events: b ? BELLS_B : BELLS_A },
    { section, layer: 7, loopLen: SECTION_LEN, events: b ? TEXTURE_B : TEXTURE_A },
  ];
}

export const demoTrack: TrackInput = {
  loop: true,
  sections: [
    { feel: 'grid', length: SECTION_LEN, bpm: 120, repeats: 4 },
    { feel: 'grid', length: SECTION_LEN, bpm: 132, repeats: 4, rampMs: 4000 },
  ],
  clips: [...clips(0, false), ...clips(1, true)],
};

/** Seconds one pass of the track lasts, ramp ignored (about 61 s). */
export const DEMO_SECONDS = (4 * SECTION_BARS * 4 * 60) / 120 + (4 * SECTION_BARS * 4 * 60) / 132;

export interface DemoSong { track: ArrayBuffer; patches: ArrayBuffer[]; layers: ArrayBuffer[] }

export function encodeDemo(): DemoSong {
  const enc = layerDefs.map((d, i) => encodeLayer(d, i));
  return { track: encodeTrack(demoTrack), patches: enc.map((e) => e.patch), layers: enc.map((e) => e.layer) };
}
