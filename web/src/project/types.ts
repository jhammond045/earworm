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
 *  velocity. 5: clips may carry a flex setting, beat clips a warp map, takes their flex anchors
 *  (docs/quant.md, Q1); all optional. 6: poly takes and their notes' curves (docs/poly.md, P3).
 *  7: meters, a section's patterns and a clip's lock (docs/meter.md, R1); all optional. Older
 *  documents are upgraded by migrate.ts. */
export const SCHEMA = 7;

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
  /** Beats per bar, the same number as Meter 1's typed beats (docs/meter.md question 6). The
   *  downbeat of each bar as Meter 1 plays it is accented, and a count-in is this long. */
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
  /** Meters (docs/meter.md): bars over each section's beats that clips lock to. The first is
   *  Meter 1: it sets each section's bars and, while it plays a pattern there, its played length.
   *  Missing: one typed meter as long as the click's bar. */
  meters?: Meter[];
}

/** A meter (docs/meter.md "The model"). Stage R1 has typed bars: `beats` per bar from each
 *  section's start. */
export interface Meter {
  id: string;
  /** "Meter 1", "Meter 2", ... */
  name: string;
  bars: { from: 'typed'; beats: number };
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
  /** Meter remap (docs/meter.md "Patterns"): by meter id, the source beats (1-based) each bar of
   *  that meter plays, in order. Grid sections only. Missing: as written. */
  play?: Record<string, number[]>;
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
  /** How a take is captured into the layer (docs/poly.md, P3). Missing: mono. */
  capture?: CaptureMode;
}

/** Instruments a poly take can be read as (docs/poly.md "Instruments"): each sets the notes kept
 *  (guitar E2 to E6, piano A0 to C8, other A1 to C7) and the default channel count. */
export const INSTRUMENTS = ['guitar', 'piano', 'other'] as const;
export type Instrument = (typeof INSTRUMENTS)[number];
/** Most capture channels (notes at once) a poly take keeps. */
export const MAX_CHANNELS = 8;
export const DEFAULT_CHANNELS: Record<Instrument, number> = { guitar: 6, piano: 8, other: 8 };

/** Mono: one line of notes, as sung. Poly: several notes at once (a guitar's chords, a piano),
 *  each with its own curve, at most `channels` sounding together (1..MAX_CHANNELS, and never more
 *  than the patch's voices). */
export interface CaptureMode {
  mode: 'mono' | 'poly';
  instrument: Instrument;
  channels: number;
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
  /** A poly take's note (schema 6): its index in the note curves of the clip's take, which the
   *  voice playing it reads. Missing: a note with no curve of its own. */
  curve?: number;
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
  /** Flex quantize (docs/quant.md): applied when the track plays and exports, never written into
   *  the notes. Missing: plays as performed. */
  flex?: Flex;
  /** The meter this clip is locked to (docs/meter.md "Locking"): it plays that meter's pattern in
   *  its section, after Feel. Missing: free, it plays as written. */
  meter?: string;
}

/** How a clip's timing is bent at play time (docs/quant.md "The model"). Stage Q1 has the grid
 *  feels; a groove and another layer's timing come with Q3, the audio modes with Q2, anchor edits
 *  and sensitivity with Q4. */
export interface Flex {
  on: boolean;
  /** Slot size in section units (240 = 16ths in a grid section). */
  grid: number;
  /** Add the triplet slots of that size; each anchor goes to the nearer slot. */
  triplets: boolean;
  feel: FlexFeel;
  /** 0..1: how far each anchor moves toward its slot. A lane on 'flex.strength' replaces it, read
   *  once per anchor at its slot. */
  strength: number;
  /** Beat layers: a factor 0..1 per kit slot on the strength of the anchors of that slot. */
  slotStrength?: number[];
  /** Section units: anchors further than this from their slot are not pulled (they ride along). */
  catch: number;
  /** How the take's audio is stretched through the map (docs/quant.md Q2): slices for beats,
   *  a phase vocoder for melody and texture. Missing: by the layer's kind (FLEX_AUDIO_DEFAULT). */
  audio?: FlexAudio;
}

export type FlexAudio = 'hits' | 'smooth';

/** Straight: the section's grid. Swing: every second slot of a pair of `unit` falls at `amount`
 *  of the pair (0.5 straight, 0.66 a triplet feel), in section units. */
export type FlexFeel = { kind: 'straight' } | { kind: 'swing'; amount: number; unit: number };

/** Lane target read by the compiler, never by the engine. */
export const FLEX_STRENGTH = 'flex.strength';

export interface MelodyClip extends ClipBase {
  kind: 'melody' | 'texture';
  notes: Note[];
  /** Take seconds -> clip time; sorted by clipT. Empty: identity offset by `takeAt`. */
  warp: WarpAnchor[];
}

export interface BeatClip extends ClipBase {
  kind: 'beat';
  hits: Hit[];
  /** Take seconds -> clip time at each hit start, written when a take is placed (schema 5), so the
   *  take's flex anchors can be placed on the clip. Missing on older clips. */
  warp?: WarpAnchor[];
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
  /** Flex anchors found after the second pass: Float32 blob, three floats each (take seconds,
   *  weight, flags: bit 0 hard, bits 1-2 onset/note/hit/peak, bits 3-4 kit slot). Missing on
   *  takes from before schema 5: their clips use their note and hit starts. */
  anchors?: BlobRef;
  /** The detector version that found them (crates/analysis/src/anchors.rs VERSION). */
  anchorsVersion?: number;
  /** A poly take (schema 6, docs/poly.md P3): Float32 blob of each note's curve (layout at
   *  `encodeNoteCurves` in m1/track.ts). Its notes are in the clip, each naming its curve. */
  noteCurves?: BlobRef;
}
