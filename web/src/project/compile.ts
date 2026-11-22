// Project -> engine blobs. The engine keeps its own copy of everything (layers by slot,
// curves by slot, one track), so the job here is to send only what changed.
//
// The diff works on object identity, not content: the document is immutable and a
// command replaces only the objects on the path it changed (patch.ts), so "same
// object as last time" means "same content". That keeps a compile cheap while playing.

import { encodeLayerBlob, encodeSample, encodeStates } from '../m1/blobs';
import {
  encodeCurve, encodeTrack, LANE_LEVEL, LANE_MACRO, LANE_PAN, LANE_PERF, LANE_VALUE, MAX_CURVES, MAX_SAMPLES,
  type TrackClip, type TrackEvent, type TrackInput, type TrackLane,
} from '../m1/track';
import {
  KIND_BEAT, KIND_MELODY, KIND_TEXTURE, LAYER_DEPTH_LOUD, LAYER_FIELDS, LAYER_FOLLOW, LAYER_INFLUENCE_AMOUNT,
  LAYER_INFLUENCE_MASK, LAYER_KIND, LAYER_KIT_CENTS, LAYER_LEVEL, LAYER_MUTE, LAYER_PAN,
  LOAD_CURVE, LOAD_LAYER, LOAD_PATCH, LOAD_SAMPLE, LOAD_STATES, LOAD_TRACK, MAX_LAYERS, N_MACROS, SAMPLE_POOL_FLOATS,
} from '../m1/protocol';
import { classicPatch, classicVectors, isClassic } from '../synth/classic';
import { inputAt, layoutOf, patchBlob, type Patch } from '../synth/patch';
import { KIT_SLOTS, LANE_SHAPES, MORPH_SHAPES, PPQ, type AutoLane, type Hit, type Layer, type MorphMarker, type Project, type Section, type Take } from './types';
import { flexActive, flexClip } from './flex';

export interface Upload {
  /** LOAD_TRACK, LOAD_PATCH, LOAD_LAYER, LOAD_CURVE, LOAD_SAMPLE or LOAD_STATES (m1/protocol.ts). */
  kind: number;
  slot: number;
  blob: ArrayBuffer;
  /** Short plain description for logs and the load-status readout. */
  what: string;
}

/** Transport units per second: ticks (bpm * 960 / 60) in a grid section, seconds in a free one. */
export function sectionUnitsPerSec(s: Section): number {
  return s.feel === 'grid' ? (s.bpm * PPQ) / 60 : 1;
}

/** Section length in its own units: beats * 960 ticks, or seconds. */
export function sectionLength(s: Section): number {
  return s.feel === 'grid' ? s.beats * PPQ : s.seconds;
}

const KIND_CODE = { melody: KIND_MELODY, beat: KIND_BEAT, texture: KIND_TEXTURE } as const;
const DEFAULT_KIT_CENTS = [2400, 4800, 6000, 7200];

/** The patch's values in layout order from a state's values (missing keys take the patch's start). */
function vector(p: Patch, values: Record<string, number>): number[] {
  const lay = layoutOf(p);
  return lay.keys.map((k, i) => values[k] ?? lay.init[i]);
}

/** Layer blob v2 (crates/klont/src/protocol.rs "Layer blob v2") from a document layer: state 0's
 *  values, macros and Influence. With an Influence the blob carries its target (the old parameters,
 *  bound to the classic patch's values) and mask and state 0's amount; without one the amount is 0.
 *  When the layer also has LOAD_STATES, those override values, macros and amount each control tick. */
export function layerBlob(l: Layer): ArrayBuffer {
  const st = l.states[0];
  const fields = new Float32Array(LAYER_FIELDS);
  fields[LAYER_LEVEL] = l.level;
  fields[LAYER_PAN] = l.pan;
  fields[LAYER_MUTE] = l.mute ? 1 : 0;
  fields[LAYER_KIND] = KIND_CODE[l.kind];
  const d = l.depth;
  fields.set([d.loud, d.open, d.hard, d.breath, d.mouth], LAYER_DEPTH_LOUD);
  fields[LAYER_FOLLOW] = l.follow;
  const infl = l.influence;
  fields[LAYER_INFLUENCE_AMOUNT] = infl && st ? st.influenceAmount : 0;
  fields[LAYER_INFLUENCE_MASK] = infl ? infl.mask : 0x1f;
  for (let k = 0; k < KIT_SLOTS; k++) fields[LAYER_KIT_CENTS + k] = l.kit[k]?.cents ?? DEFAULT_KIT_CENTS[k];
  const macros = Array.from({ length: N_MACROS }, (_, k) => st?.macros[k] ?? 0.5);
  if (isClassic(l.patch)) {
    const v = classicVectors(l.kind, st?.values ?? {}, infl ? infl.target : null, l.patch);
    return encodeLayerBlob({ fields, values: v.values, macros, target: v.target, groups: v.groups });
  }
  const values = vector(l.patch, st?.values ?? {});
  return encodeLayerBlob({ fields, values, macros, target: values, groups: values.map(() => 0) });
}

/** A layer slot that no longer has a layer: the classic melody patch's starting values, muted. */
function silentLayerBlob(): ArrayBuffer {
  const fields = new Float32Array(LAYER_FIELDS);
  fields[LAYER_MUTE] = 1;
  fields[LAYER_INFLUENCE_MASK] = 0x1f;
  fields.set([1, 0.5, 0.5, 0.5, 1], LAYER_DEPTH_LOUD);
  fields[LAYER_FOLLOW] = 1;
  fields.set(DEFAULT_KIT_CENTS, LAYER_KIT_CENTS);
  const init = layoutOf(SILENT_PATCH).init;
  return encodeLayerBlob({ fields, values: init, macros: new Array<number>(N_MACROS).fill(0.5), target: init, groups: init.map(() => 0) });
}

const SILENT_PATCH = classicPatch('melody');
/** Longest grain source sent for one layer. */
export const GRAIN_SAMPLES = 480_000;

/** Same mapping as hitsToEvents in m2/take.ts (the document's Hit has no analysis-only fields). */
function hitEvents(hits: Hit[]): TrackEvent[] {
  return hits.map((h) => ({
    t: h.t, dur: h.dur, slot: Math.max(0, Math.min(KIT_SLOTS - 1, Math.round(h.slot))), strength: h.strength,
    cents: h.shape.cents ?? NaN,
    attack: h.shape.attack, burst: h.shape.burst, bend: h.shape.bend, hold: h.shape.hold,
    vowel: h.shape.vowel, tone: h.shape.tone, endStop: h.shape.endStop ? 1 : 0, endClick: h.shape.endClick,
  }));
}

export type CurveFor = (take: Take, layer: Layer) => Float32Array | null;

/** The grain source of a layer, ready to send as its sample (layer i's grains and sampler read sample i): the steady
 *  region of its Influence source, at the source's rate, with the region's reference pitch. null: the data
 *  is not loaded yet. The array's identity is the cache key (same array, not sent again), so return the
 *  same array until the source changes. */
export type GrainFor = (layer: Layer) => { data: Float32Array; rate: number; refCents: number | null } | null;

/** A take's audio (mono, at the take's rate), for a layer whose patch has an `input` element: its
 *  raw input on playback. null: not loaded yet. Same array until the take changes. */
export type AudioFor = (take: Take) => Float32Array | null;

/** A take's flex anchors (docs/quant.md), or null: not loaded yet, or the take has none. A clip
 *  then quantizes its note or hit starts. Same array until the take's anchors change. */
export type AnchorsFor = (take: Take) => Float32Array | null;

export interface CompileOptions { grainFor?: GrainFor; audioFor?: AudioFor; anchorsFor?: AnchorsFor }

/** Take audio goes in the sample pool from this id up (ids below are the layers' grain samples). */
export const FIRST_AUDIO_SAMPLE = 8;

const SHAPE_CODE: Record<string, number> = Object.fromEntries(MORPH_SHAPES.map((n, i) => [n, i]));

/** Does the layer need LOAD_STATES: two or more states, or a morph marker on any of its clips. */
function usesStates(doc: Project, l: Layer): boolean {
  if (l.states.length >= 2) return true;
  return doc.sections.some((s) => (s.clips[l.id]?.morph?.length ?? 0) > 0);
}

function statesBlob(l: Layer): ArrayBuffer {
  // Without an Influence the engine has no target to blend toward, so amounts go out as 0.
  const on = !!l.influence;
  const n = layoutOf(l.patch).keys.length;
  return encodeStates(n, l.states.map((s) => ({ values: vector(l.patch, s.values), macros: s.macros, influenceAmount: on ? s.influenceAmount : 0 })));
}

/** A layer slot that no longer has a layer, but had states loaded: one state at the starting values,
 *  so a later layer in the same slot does not inherit the old states. */
function silentStatesBlob(): ArrayBuffer {
  const init = layoutOf(SILENT_PATCH).init;
  return encodeStates(init.length, [{ values: init, macros: new Array<number>(N_MACROS).fill(0.5), influenceAmount: 0 }]);
}

const emptySample = (): ArrayBuffer => encodeSample(48000, null, new Float32Array(0));

export class Compiler {
  /** Layer object last sent per engine slot; null: the slot was silenced; undefined: never sent. */
  private layers: (Layer | null | undefined)[] = [];
  /** Patch object last sent per engine slot; undefined: the engine's own (classic melody). */
  private patches: (Patch | undefined)[] = [];
  /** Take id -> curve slot. */
  private slotOf = new Map<string, number>();
  /** Curve array last sent per slot. */
  private curveSent: (Float32Array | undefined)[] = [];
  /** Grain array last sent per slot; null: the source was cleared; undefined: never sent. */
  private grainSent: (Float32Array | null | undefined)[] = [];
  /** What LOAD_STATES last carried per slot: the states array and whether an Influence was on; 'silent' for a removed layer; undefined: never sent. */
  private statesSent: ({ states: Layer['states']; infl: boolean } | 'silent' | undefined)[] = [];
  private lastSections: Section[] | null = null;
  private lastTrackKey: string | null = null;
  /** Take id -> sample id of its audio, and the array last sent there. */
  private audioOf = new Map<string, number>();
  private audioSent = new Map<number, Float32Array>();

  /** Blobs that changed since the last compile, layers and curves before the track. */
  compile(doc: Project, curveFor: CurveFor, opts: CompileOptions = {}): Upload[] {
    const out: Upload[] = [];

    // Layers: a slot is the layer's index in the array. Grain and states follow their layer's blob.
    doc.layers.forEach((l, i) => {
      this.compilePatch(out, i, l.patch, `"${l.name}"`);
      if (this.layers[i] !== l) {
        out.push({ kind: LOAD_LAYER, slot: i, blob: layerBlob(l), what: `layer ${i} "${l.name}"` });
        this.layers[i] = l;
      }
      this.compileGrain(out, l, i, opts.grainFor);
      // Once states are loaded the engine cannot unload them, so a layer that drops back to one state
      // keeps getting its (single) state blob.
      const wasLoaded = this.statesSent[i] !== undefined;
      if (usesStates(doc, l) || wasLoaded) {
        const sent = this.statesSent[i];
        const infl = !!l.influence;
        if (sent === undefined || sent === 'silent' || sent.states !== l.states || sent.infl !== infl) {
          out.push({ kind: LOAD_STATES, slot: i, blob: statesBlob(l), what: `states ${i} "${l.name}" (${l.states.length})` });
          this.statesSent[i] = { states: l.states, infl };
        }
      }
    });
    for (let i = doc.layers.length; i < MAX_LAYERS; i++) {
      if (this.layers[i] !== undefined && this.layers[i] !== null) {
        this.compilePatch(out, i, SILENT_PATCH, '(removed)');
        out.push({ kind: LOAD_LAYER, slot: i, blob: silentLayerBlob(), what: `layer ${i} (removed, muted)` });
        this.layers[i] = null;
      }
      if (this.grainSent[i]) {
        out.push({ kind: LOAD_SAMPLE, slot: i, blob: emptySample(), what: `sample ${i} (cleared)` });
        this.grainSent[i] = null;
      }
      if (this.statesSent[i] !== undefined && this.statesSent[i] !== 'silent') {
        out.push({ kind: LOAD_STATES, slot: i, blob: silentStatesBlob(), what: `states ${i} (removed layer)` });
        this.statesSent[i] = 'silent';
      }
    }

    // Curves: one slot per take that a melody or texture clip currently plays.
    const wanted = new Map<string, { take: Take; layer: Layer }>();
    doc.sections.forEach((s) => {
      doc.layers.forEach((l) => {
        const c = s.clips[l.id];
        if (!c || c.kind === 'beat' || !c.take || wanted.has(c.take)) return;
        const take = doc.takes[c.take];
        if (take) wanted.set(c.take, { take, layer: l });
      });
    });
    const frames = new Map<string, Float32Array>();
    for (const [id, { take, layer }] of wanted) {
      const arr = curveFor(take, layer);
      if (arr) frames.set(id, arr);
    }
    for (const id of [...this.slotOf.keys()]) {
      if (!frames.has(id)) {
        const slot = this.slotOf.get(id)!;
        this.slotOf.delete(id);
        this.curveSent[slot] = undefined;
      }
    }
    const used = new Set(this.slotOf.values());
    for (const [id, arr] of frames) {
      let slot = this.slotOf.get(id);
      if (slot === undefined) {
        slot = 0;
        while (used.has(slot)) slot++;
        if (slot >= MAX_CURVES) throw new RangeError(`more than ${MAX_CURVES} takes are in use at once`);
        used.add(slot);
        this.slotOf.set(id, slot);
      }
      if (this.curveSent[slot] !== arr) {
        out.push({ kind: LOAD_CURVE, slot, blob: encodeCurve(arr), what: `curve ${slot} (take ${id})` });
        this.curveSent[slot] = arr;
      }
    }

    this.compileAudio(out, doc, opts.audioFor);

    // Track. It depends on the sections, loop flag, layer order and octave, the curve slots, the take
    // audio's samples, which state index each morph marker points at (state edits that keep the order
    // do not touch the track), and the layout of every patch that a lane names a value of.
    const layerSig = doc.layers.map((l) => `${l.id}:${l.tune.octave}`).join(',');
    const slotSig = [...this.slotOf].map(([id, s]) => `${id}=${s}`).sort().join(',');
    const audioSig = [...this.audioOf].map(([id, s]) => `${id}=${s}`).sort().join(',');
    // A flexed clip's take anchors arrive after the document does.
    const anchors = (t: Take) => opts.anchorsFor?.(t) ?? null;
    const flexSig = doc.sections.flatMap((s) => Object.values(s.clips).filter((c) => c.flex?.on && c.take).map((c) => `${c.take}:${anchors(doc.takes[c.take]) ? 1 : 0}`)).join(',');
    const key = `${doc.loop}|${layerSig}|${slotSig}|${audioSig}|${morphSig(doc)}|${laneSig(doc)}|${flexSig}`;
    if (doc.sections !== this.lastSections || key !== this.lastTrackKey) {
      out.push({ kind: LOAD_TRACK, slot: 0, blob: encodeTrack(this.trackInput(doc, anchors)), what: 'track' });
      this.lastSections = doc.sections;
      this.lastTrackKey = key;
    }
    return out;
  }

  /** A new patch for a slot. The engine then starts its values over and drops its states, so the
   *  layer and its states go again too. */
  private compilePatch(out: Upload[], i: number, p: Patch, name: string): void {
    if (this.patches[i] === p) return;
    out.push({ kind: LOAD_PATCH, slot: i, blob: patchBlob(p, i), what: `patch ${i} ${name}` });
    this.patches[i] = p;
    this.layers[i] = undefined;
    this.statesSent[i] = undefined;
  }

  private compileGrain(out: Upload[], l: Layer, i: number, grainFor: GrainFor | undefined): void {
    const infl = l.influence;
    // The grains play the source when "Use the sound itself" is on; a sampler always plays it.
    const sampler = l.patch.voices.some((v) => v.modules.some((m) => m.type === 'sampler'));
    const wanted = !!infl && (infl.useSound || sampler) && infl.grain !== null;
    if (!wanted) {
      if (this.grainSent[i]) {
        out.push({ kind: LOAD_SAMPLE, slot: i, blob: emptySample(), what: `sample ${i} (cleared)` });
        this.grainSent[i] = null;
      }
      return;
    }
    const g = grainFor?.(l);
    if (!g || g.data === this.grainSent[i]) return;
    // Ten seconds at 48 kHz per layer, as before the shared pool, so a long source sounds as it did.
    // The start of the region is the part that matters.
    const max = Math.min(GRAIN_SAMPLES, SAMPLE_POOL_FLOATS / MAX_LAYERS);
    const data = g.data.length > max ? g.data.subarray(0, max) : g.data;
    out.push({ kind: LOAD_SAMPLE, slot: i, blob: encodeSample(g.rate, g.refCents, data), what: `sample ${i} "${infl!.sourceName}"` });
    this.grainSent[i] = g.data;
  }

  /** Take audio for every clip whose layer has an `input` element, in sample ids from
   *  FIRST_AUDIO_SAMPLE. A take no clip needs any more has its sample cleared. */
  private compileAudio(out: Upload[], doc: Project, audioFor: AudioFor | undefined): void {
    const wanted = new Map<string, Take>();
    doc.sections.forEach((s) => doc.layers.forEach((l) => {
      const c = s.clips[l.id];
      if (!c?.take || inputAt(l.patch) < 0) return;
      const take = doc.takes[c.take];
      if (take) wanted.set(take.id, take);
    }));
    for (const [id, slot] of [...this.audioOf]) {
      if (wanted.has(id)) continue;
      this.audioOf.delete(id);
      this.audioSent.delete(slot);
      out.push({ kind: LOAD_SAMPLE, slot, blob: emptySample(), what: `sample ${slot} (take ${id} no longer used)` });
    }
    const used = new Set(this.audioOf.values());
    for (const [id, take] of wanted) {
      const data = audioFor?.(take);
      if (!data) continue;
      let slot = this.audioOf.get(id);
      if (slot === undefined) {
        slot = FIRST_AUDIO_SAMPLE;
        while (used.has(slot)) slot++;
        if (slot >= MAX_SAMPLES) continue;
        used.add(slot);
        this.audioOf.set(id, slot);
      }
      if (this.audioSent.get(slot) === data) continue;
      out.push({ kind: LOAD_SAMPLE, slot, blob: encodeSample(take.rate, null, data), what: `sample ${slot} (take ${id} audio)` });
      this.audioSent.set(slot, data);
    }
  }

  /** The engine restarted: everything is sent again on the next compile. */
  reset(): void {
    this.audioOf.clear();
    this.audioSent.clear();
    this.patches = [];
    this.grainSent = [];
    this.statesSent = [];
    this.layers = [];
    this.slotOf.clear();
    this.curveSent = [];
    this.lastSections = null;
    this.lastTrackKey = null;
  }

  private trackInput(doc: Project, anchorsFor: AnchorsFor): TrackInput {
    const sections = doc.sections.map((s) => ({
      feel: s.feel,
      length: sectionLength(s),
      bpm: s.feel === 'grid' ? s.bpm : undefined,
      repeats: Math.max(1, Math.round(s.repeats)),
      rampMs: s.rampMs,
      uid: sectionUid(s.id),
    }));
    const clips: TrackClip[] = [];
    doc.sections.forEach((s, si) => {
      doc.layers.forEach((l, li) => {
        const c = s.clips[l.id];
        if (!c) return;
        const audio = c.take ? (this.audioOf.get(c.take) ?? -1) : -1;
        const lanes = lanesFor(c.auto, l);
        // Flex (docs/quant.md, Q1): notes, hits and the warp map the curves are read through go
        // through the clip's time map. The take's audio keeps its performed timing until Q2.
        const take = c.take ? doc.takes[c.take] : undefined;
        const flexed = flexActive(c, s.feel === 'grid') ? flexClip(c, take ? anchorsFor(take) : null, sectionUnitsPerSec(s)) : null;
        if (c.kind === 'beat') {
          clips.push({ section: si, layer: li, loopLen: c.loopLen, curve: -1, events: hitEvents(flexed?.hits ?? c.hits), morph: morphFor(c.morph, l), audio, lanes });
          return;
        }
        const oct = 1200 * l.tune.octave;
        const morph = morphFor(c.morph, l);
        const curve = c.take ? (this.slotOf.get(c.take) ?? -1) : -1;
        const warp = flexed?.warp ?? c.warp;
        // The audio keeps the clip's own map; an empty one reads the take at clip time, as the engine does.
        const audioWarp = audio >= 0 && warp !== c.warp
          ? (c.warp.length ? c.warp : [{ takeSec: 0, clipT: 0 }, { takeSec: c.loopLen / sectionUnitsPerSec(s), clipT: c.loopLen }])
          : undefined;
        clips.push({
          section: si, layer: li, loopLen: c.loopLen, curve,
          events: (flexed?.notes ?? c.notes).map((n) => ({ t: n.t, dur: n.dur, cents: n.cents + oct, hard: n.hard, strength: n.vel ?? 1 })),
          warp: curve >= 0 || audio >= 0 ? warp : [],
          morph,
          audio,
          lanes,
          ...(audioWarp ? { audioWarp } : {}),
        });
      });
    });
    return { sections, clips, loop: doc.loop };
  }
}

/** Morph markers for the engine: state ids become indices into the layer's states, shape names become codes.
 *  A marker whose state is gone (should not happen: removeState drops them) is skipped.
 *  No markers: undefined, so a project without any still compiles to the v1 track blob. */
function morphFor(markers: MorphMarker[] | undefined, l: Layer): { t: number; state: number; lenMs: number; shape: number }[] | undefined {
  const out: { t: number; state: number; lenMs: number; shape: number }[] = [];
  for (const m of markers ?? []) {
    const state = l.states.findIndex((s) => s.id === m.state);
    if (state >= 0) out.push({ t: m.t, state, lenMs: m.lenMs, shape: SHAPE_CODE[m.shape] ?? 0 });
  }
  return out.length ? out : undefined;
}

const PERF_INDEX: Record<string, number> = { wheel: 0, bend: 1, pressure: 2 };
const LANE_SHAPE_CODE: Record<string, number> = Object.fromEntries(LANE_SHAPES.map((n, i) => [n, i]));

/** A lane's target for the engine: a value of the layer's patch (by its place in the layout), a
 *  macro, the level, the pan or a performance source. null: the patch has no such value. */
export function laneTarget(target: string, l: Layer): { kind: number; index: number } | null {
  const m = /^macro([1-8])$/.exec(target);
  if (m) return { kind: LANE_MACRO, index: Number(m[1]) - 1 };
  if (target === 'layer.level') return { kind: LANE_LEVEL, index: 0 };
  if (target === 'layer.pan') return { kind: LANE_PAN, index: 0 };
  if (target in PERF_INDEX) return { kind: LANE_PERF, index: PERF_INDEX[target] };
  const i = layoutOf(l.patch).index.get(target);
  return i === undefined ? null : { kind: LANE_VALUE, index: i };
}

/** Automation lanes for the engine. A lane whose value the patch no longer has is skipped. */
function lanesFor(auto: AutoLane[] | undefined, l: Layer): TrackLane[] | undefined {
  const out: TrackLane[] = [];
  for (const lane of auto ?? []) {
    const t = laneTarget(lane.target, l);
    if (!t || !lane.points.length) continue;
    out.push({ ...t, points: lane.points.map((p) => ({ t: p.t, v: p.v, shape: LANE_SHAPE_CODE[p.shape] ?? 0 })) });
  }
  return out.length ? out : undefined;
}

/** The value indices lanes name change when a patch's layout does. */
function laneSig(doc: Project): string {
  const parts: string[] = [];
  doc.layers.forEach((l) => {
    const keys = new Set<string>();
    for (const s of doc.sections) for (const lane of s.clips[l.id]?.auto ?? []) keys.add(lane.target);
    if (keys.size) parts.push(`${l.id}:${[...keys].map((k) => laneTarget(k, l)?.index ?? 'x').join('.')}`);
  });
  return parts.join(';');
}

function morphSig(doc: Project): string {
  const parts: string[] = [];
  doc.layers.forEach((l, li) => {
    doc.sections.forEach((s, si) => {
      const m = s.clips[l.id]?.morph;
      if (m?.length) parts.push(`${si}.${li}:${m.map((x) => l.states.findIndex((st) => st.id === x.state)).join('')}`);
    });
  });
  return parts.join(';');
}

/** Stable nonzero 32-bit id of a section for the engine (FNV-1a of its id), so the transport
 *  stays on the same section when sections before it are added, removed or moved while playing.
 *  A collision only means the engine may pick the other section of the pair after a reload. */
export function sectionUid(id: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < id.length; i++) {
    h ^= id.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h || 1;
}
