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

/** 3: the project carries metronome settings (M5). Older documents are upgraded by migrate.ts. */
export const SCHEMA = 3;

/** Click track. The engine plays it live only; exports and the mic never contain it. */
export interface Metro {
  on: boolean;
  /** 0..1. */
  level: number;
  /** `tick` is a plain click. `kit` uses `slot` of the layer `layerId`. */
  voice: 'tick' | 'kit';
  layerId: string;
  /** Kit slot 0..3. */
  slot: number;
  /** Beats per bar. The downbeat is accented, and a count-in is this long. */
  bar: number;
}

export function defaultMetro(layerId = ''): Metro {
  return { on: false, level: 0.7, voice: 'tick', layerId, slot: 0, bar: 4 };
}
export const PPQ = 960;
export const MAX_LAYERS = 8;
export const KIT_SLOTS = 4;
/** Sound states per layer (matches the engine's MAX_STATES). */
export const MAX_STATES = 8;

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
  /** Click settings. Missing on a document that has not been migrated yet. */
  metro?: Metro;
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

/** A sound state (TDD State). More than one state is blended by the morph lane (M4). */
export interface State {
  id: string;
  name: string;
  /** 96 normalized values. */
  base: number[];
  /** 8 macro positions, 0.5 neutral. */
  macros: number[];
  influenceAmount: number;
}

/** Sound Influence (TDD "Sound Influence"): a target parameter set the layer's base is blended toward. */
export interface Influence {
  /** 96 normalized values. */
  target: number[];
  /** One bit per aspect group (tone 1, shape 2, motion 4, grit 8, space 16); 0x1f is all. */
  mask: number;
  /** The captured sound: mono Float32 blob, id starts with "grain-". */
  source: BlobRef;
  sourceName: string;
  /** Sample rate of the source blob. */
  sourceRate: number;
  /** Steady region of the source (sample indices) and its pitch; null: no grain source. */
  grain: { start: number; end: number; refCents: number | null } | null;
  /** Play the source itself through the grain cloud. */
  useSound: boolean;
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
  /** Set while a match is applied and not yet committed. */
  influence?: Influence;
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

export type MorphShape = 'line' | 'ease' | 'late' | 'step' | 'wobble';
export const MORPH_SHAPES: readonly MorphShape[] = ['line', 'ease', 'late', 'step', 'wobble'];

/** From position `t` in the clip the layer moves to `state` over `lenMs`. Sorted by t. */
export interface MorphMarker {
  /** Section units, inside [0, loopLen). */
  t: number;
  /** State id of the layer. */
  state: string;
  lenMs: number;
  shape: MorphShape;
}

export interface ClipBase {
  /** Current take id, or '' for a clip with no take (drawn notes only). */
  take: string;
  /** Take stack, newest last; includes `take`. */
  takes: string[];
  /** Loop length in section units. */
  loopLen: number;
  /** Grid snap for edits in section units (0 = off). */
  timeSnap: number;
  /** The morph lane, sorted by t. */
  morph: MorphMarker[];
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
