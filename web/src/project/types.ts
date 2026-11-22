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
// - A layer's sound is a patch (synth/patch.ts) and, per state, a value for every key of it,
//   normalized 0..1. The classic patch's values map to the old 96 parameters (synth/classic.ts).

/** 4: layers carry a patch and states carry values by key (docs/synth.md, stage S1); notes carry a
 *  velocity. Older documents are upgraded by migrate.ts. */
export const SCHEMA = 4;

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

import type { Patch } from '../synth/patch';
export type { Patch };

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
  /** Live play (docs/synth.md "Live use"): the MIDI controller map, which layer each MIDI
   *  channel plays, and what the XY pad moves. Missing means none of them is set. */
  perform?: Perform;
}

/** What a MIDI controller or the XY pad sets: a value of a layer's patch by key, or one of the
 *  layer's macros ("macro1" to "macro8"), in the state being edited. */
export interface PerfTarget { layer: string; key: string }

/** One learned controller: control change `cc` on MIDI `channel` (0..15) of the input named `device`. */
export interface MidiMapping { device: string; channel: number; cc: number; target: PerfTarget }

export interface Perform {
  map: MidiMapping[];
  /** Layer id each MIDI channel (index 0..15) plays; '' or missing: the selected layer. */
  channels: string[];
  /** The XY pad's two values, across and up. */
  xy: [PerfTarget | null, PerfTarget | null];
}

export const MIDI_CHANNELS = 16;
export const defaultPerform = (): Perform => ({ map: [], channels: [], xy: [null, null] });
/** Scene time when a layer has none, ms. */
export const SCENE_MS = 400;

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

/** A kit slot's sound is its voice patch (patch.voices[slot]) and values `k<n>.*` in every state. */
export interface KitSlot {
  /** Default pitch, cents above C0. */
  cents: number;
}

/** A sound state (TDD State). More than one state is blended by the morph lane (M4). */
export interface State {
  id: string;
  name: string;
  /** A normalized value for every key of the layer's patch. */
  values: Record<string, number>;
  /** 8 macro positions, 0.5 neutral. */
  macros: number[];
  influenceAmount: number;
}

/** Sound Influence (TDD "Sound Influence"): a target parameter set the layer's values are blended toward. */
export interface Influence {
  /** 96 normalized values in the Klont vocabulary (the old parameters, gen/params.ts). */
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
  /** The sound's structure: modules, routes, channel. The same for every state. */
  patch: Patch;
  states: State[];
  tune: Tune;
  depth: GestureDepth;
  /** Hit-shape follow 0..1 (beat layers). */
  follow: number;
  /** Beat layers: four slots. Empty for other kinds. */
  kit: KitSlot[];
  /** Set while a match is applied and not yet committed. */
  influence?: Influence;
  /** How long a scene takes to morph to this layer's state, ms. Missing: SCENE_MS. */
  sceneMs?: number;
}

export interface Note {
  id: string;
  /** Section units: ticks (grid) or seconds (free). */
  t: number;
  dur: number;
  /** Cents above C0, octave not applied (Tune.octave is added at compile time). */
  cents: number;
  hard: number;
  /** Velocity 0..1; missing means 1 (documents from before schema 4 get 1). */
  vel?: number;
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

export type LaneShape = 'line' | 'ease' | 'step';
export const LANE_SHAPES: readonly LaneShape[] = ['line', 'ease', 'step'];

/** One point of an automation lane: from `t` (section units, inside [0, loopLen)) the target is
 *  `v`, moving to the next point's value by `shape`. */
export interface LanePoint { t: number; v: number; shape: LaneShape }

/** An automation lane (docs/synth.md "DAW use"). `target` is a value key of the layer's patch, a
 *  macro (`macro1` to `macro8`), `layer.level`, `layer.pan`, or one of the `wheel`, `bend` and
 *  `pressure` sources. `v` is in the target's units: 0..1 for a value, a macro, the wheel and
 *  pressure; dB (-60..12) for the level; -1..1 for the pan and the bend. Points sorted by t. */
export interface AutoLane { target: string; points: LanePoint[] }

/** Lane targets that are not value keys, with their range. */
export const LANE_FIXED: Record<string, [number, number]> = {
  'layer.level': [-60, 12], 'layer.pan': [-1, 1], wheel: [0, 1], bend: [-1, 1], pressure: [0, 1],
  ...Object.fromEntries(Array.from({ length: 8 }, (_, k) => [`macro${k + 1}`, [0, 1] as [number, number]])),
};

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
  /** Automation lanes, one per target (stage S5). Missing means none. */
  auto?: AutoLane[];
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
