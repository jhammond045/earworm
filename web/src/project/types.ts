// The project document (TDD "Data model"). One immutable JSON value, changed only
// through commands (commands.ts). Binary data (take audio and frames) lives outside
// the document in the blob store (persist.ts) and is referred to by BlobRef.
//
// Units:
// - Grid sections: note and hit times in ticks (960 per beat), so a tempo change
//   re-times them with no data change.
// - Free sections: seconds.
// - Curves (a take's frames) stay in take seconds; a clip's warp map maps take
//   seconds to clip time.
// - Parameter sets are 96 normalized values (gen/params.ts order).

export const SCHEMA = 1;
export const PPQ = 960;
export const MAX_LAYERS = 8;
export const KIT_SLOTS = 4;

export type Ticks = number;
export type Seconds = number;
/** Id of a binary blob in the blob store. */
export type BlobRef = string;

export type Feel = 'grid' | 'free';
export type LayerKind = 'melody' | 'beat' | 'texture';

export interface Project {
  schema: number;
  id: string;
  name: string;
  /** Play order. */
  sections: Section[];
  /** At most MAX_LAYERS. A layer's engine slot is its index in this array. */
  layers: Layer[];
  /** Every take referenced by a clip (current or stacked), by id. */
  takes: Record<string, Take>;
  /** Whether the track loops back to the first section after the last. */
  loop: boolean;
}

export interface Section {
  id: string;
  name: string;
  feel: Feel;
  /** Grid: length in beats. */
  beats: number;
  /** Free: length in seconds. */
  seconds: number;
  /** Grid only. */
  bpm: number;
  repeats: number;
  /** Tempo ramp from the previous section's bpm, ms. */
  rampMs: number;
  /** Clips by layer id. At most one clip per (section, layer). */
  clips: Record<string, Clip>;
}

export interface GestureDepth { loud: number; open: number; hard: number; breath: number; mouth: number }

export interface Tune {
  snap: number;
  speedMs: number;
  slides: 0 | 1 | 2; // glide, gliss, jump
  vibrato: number;
  levelOut: number;
  root: number;
  scale: number;
  refCents: number;
  octave: number;
}

export interface KitSlot {
  /** 96 normalized values. */
  params: number[];
  /** Default pitch, cents above C0. */
  cents: number;
}

/** A sound state (TDD State). M3 uses states[0] only; M4 adds the morph lane. */
export interface State {
  id: string;
  name: string;
  /** 96 normalized values. */
  base: number[];
  /** 8 macro positions, 0.5 neutral. */
  macros: number[];
  influenceAmount: number;
}

export interface Layer {
  id: string;
  name: string;
  kind: LayerKind;
  color: number; // 0xRRGGBB
  /** dB, -60..12. */
  level: number;
  pan: number;
  mute: boolean;
  states: State[];
  tune: Tune;
  depth: GestureDepth;
  /** Hit-shape follow 0..1 (beat layers). */
  follow: number;
  /** Beat layers: four slots. Empty for other kinds. */
  kit: KitSlot[];
}

export interface Note {
  id: string;
  /** Section units: ticks (grid) or seconds (free). */
  t: number;
  dur: number;
  /** Cents above C0, octave not applied (Tune.octave is added at compile time). */
  cents: number;
  hard: number;
}

export interface HitShape {
  attack: number; burst: number; cents: number | null; bend: number; hold: number;
  vowel: number; tone: number; endStop: number; endClick: number;
}

export interface Hit {
  id: string;
  t: number;
  dur: number;
  slot: number;
  strength: number;
  shape: HitShape;
}

export interface WarpAnchor { takeSec: Seconds; clipT: number }

export interface ClipBase {
  /** Current take id, or '' for a clip with no take (drawn notes only). */
  take: string;
  /** Take stack, newest last; includes `take`. */
  takes: string[];
  /** Loop length in section units. */
  loopLen: number;
  /** Grid snap for edits in section units (0 = off). */
  timeSnap: number;
}

export interface MelodyClip extends ClipBase {
  kind: 'melody' | 'texture';
  notes: Note[];
  /** Take seconds -> clip time; sorted by clipT. Empty: identity offset by `takeAt`. */
  warp: WarpAnchor[];
}

export interface BeatClip extends ClipBase {
  kind: 'beat';
  hits: Hit[];
}

export type Clip = MelodyClip | BeatClip;

export interface Take {
  id: string;
  /** Mono Float32 mic audio (high-passed) at `rate`. */
  audio: BlobRef;
  /** 25-float frame records per 10 ms; slot 0 holds the second pass's repaired pitch. */
  frames: BlobRef;
  rate: number;
  /** Round trip used when the take was placed, ms. */
  latencyMs: number;
  /** Frame count (10 ms each). */
  frameCount: number;
  /** Second-pass results kept for display and re-tuning. */
  kind: LayerKind;
  refCents: number;
  root: number;
  scale: number;
  bpm: number | null;
}
