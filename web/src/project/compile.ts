// Project -> engine blobs. The engine keeps its own copy of everything (layers by slot,
// curves by slot, one track), so the job here is to send only what changed.
//
// The diff works on object identity, not content: the document is immutable and a
// command replaces only the objects on the path it changed (patch.ts), so "same
// object as last time" means "same content". That keeps a compile cheap while playing.

import { COUNT, neutralSet } from '../gen/params';
import { encodeGrain, encodeStates, GRAIN_MAX_SAMPLES } from '../m1/blobs';
import { encodeCurve, encodeTrack, MAX_CURVES, type TrackClip, type TrackEvent, type TrackInput } from '../m1/track';
import {
  KIND_BEAT, KIND_MELODY, KIND_TEXTURE, LAYER_DEPTH_LOUD, LAYER_FIELDS, LAYER_FOLLOW, LAYER_INFLUENCE_AMOUNT,
  LAYER_INFLUENCE_MASK, LAYER_KIND, LAYER_KIT_CENTS, LAYER_LEVEL, LAYER_MAGIC, LAYER_MUTE, LAYER_OFF_BASE,
  LAYER_OFF_FIELDS, LAYER_OFF_INFLUENCE, LAYER_OFF_KIT, LAYER_OFF_MACROS, LAYER_PAN, LAYER_WORDS,
  LOAD_CURVE, LOAD_GRAIN, LOAD_LAYER, LOAD_STATES, LOAD_TRACK, MAX_LAYERS, N_MACROS,
} from '../m1/protocol';
import { KIT_SLOTS, MORPH_SHAPES, PPQ, type Hit, type Layer, type MorphMarker, type Project, type Section, type Take } from './types';

export interface Upload {
  /** LOAD_TRACK, LOAD_LAYER, LOAD_CURVE, LOAD_GRAIN or LOAD_STATES (m1/protocol.ts). */
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

/** Layer blob (crates/klont/src/protocol.rs "Layer blob") from a document layer. The document
 *  already holds normalized 96-value sets, so nothing is converted. With an Influence the blob carries its
 *  target and mask and state 0's amount; without one the amount is 0 (the engine then ignores the target).
 *  When the layer also has LOAD_STATES, those override base, macros and amount each control tick. */
export function layerBlob(l: Layer): ArrayBuffer {
  const buf = new ArrayBuffer(LAYER_WORDS * 4);
  const u = new Uint32Array(buf);
  const f = new Float32Array(buf);
  u[0] = LAYER_MAGIC;
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
  f.set(fields, LAYER_OFF_FIELDS);
  const neutral = neutralSet();
  f.set(st ? st.base.slice(0, COUNT) : neutral, LAYER_OFF_BASE);
  for (let k = 0; k < N_MACROS; k++) f[LAYER_OFF_MACROS + k] = st?.macros[k] ?? 0.5;
  f.set(infl ? infl.target.slice(0, COUNT) : neutral, LAYER_OFF_INFLUENCE);
  for (let k = 0; k < KIT_SLOTS; k++) f.set(l.kit[k] ? l.kit[k].params.slice(0, COUNT) : neutral, LAYER_OFF_KIT + k * COUNT);
  return buf;
}

/** A layer slot that no longer has a layer: neutral sound, muted, so nothing from it can be heard. */
function silentLayerBlob(): ArrayBuffer {
  const buf = new ArrayBuffer(LAYER_WORDS * 4);
  const f = new Float32Array(buf);
  new Uint32Array(buf)[0] = LAYER_MAGIC;
  const fields = new Float32Array(LAYER_FIELDS);
  fields[LAYER_MUTE] = 1;
  fields[LAYER_INFLUENCE_MASK] = 0x1f;
  fields.set([1, 0.5, 0.5, 0.5, 1], LAYER_DEPTH_LOUD);
  fields[LAYER_FOLLOW] = 1;
  fields.set(DEFAULT_KIT_CENTS, LAYER_KIT_CENTS);
  f.set(fields, LAYER_OFF_FIELDS);
  const neutral = neutralSet();
  f.set(neutral, LAYER_OFF_BASE);
  f.fill(0.5, LAYER_OFF_MACROS, LAYER_OFF_MACROS + N_MACROS);
  f.set(neutral, LAYER_OFF_INFLUENCE);
  for (let k = 0; k < KIT_SLOTS; k++) f.set(neutral, LAYER_OFF_KIT + k * COUNT);
  return buf;
}

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

/** The grain source of a layer, ready to send: the steady region of its Influence source, at the source's
 *  rate, with the region's reference pitch. null: the data is not loaded yet. The array's identity is the
 *  cache key (same array, not sent again), so return the same array until the source changes. */
export type GrainFor = (layer: Layer) => { data: Float32Array; rate: number; refCents: number | null } | null;

export interface CompileOptions { grainFor?: GrainFor }

const SHAPE_CODE: Record<string, number> = Object.fromEntries(MORPH_SHAPES.map((n, i) => [n, i]));

/** Does the layer need LOAD_STATES: two or more states, or a morph marker on any of its clips. */
function usesStates(doc: Project, l: Layer): boolean {
  if (l.states.length >= 2) return true;
  return doc.sections.some((s) => (s.clips[l.id]?.morph?.length ?? 0) > 0);
}

function statesBlob(l: Layer): ArrayBuffer {
  // Without an Influence the engine has no target to blend toward, so amounts go out as 0.
  const on = !!l.influence;
  return encodeStates(l.states.map((s) => ({ base: s.base, macros: s.macros, influenceAmount: on ? s.influenceAmount : 0 })));
}

/** A layer slot that no longer has a layer, but had states loaded: one neutral state, so a later layer
 *  in the same slot does not inherit the old states. */
function silentStatesBlob(): ArrayBuffer {
  return encodeStates([{ base: neutralSet(), macros: new Array<number>(N_MACROS).fill(0.5), influenceAmount: 0 }]);
}

const emptyGrain = (): ArrayBuffer => encodeGrain(48000, null, new Float32Array(0));

export class Compiler {
  /** Layer object last sent per engine slot; null: the slot was silenced; undefined: never sent. */
  private layers: (Layer | null | undefined)[] = [];
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

  /** Blobs that changed since the last compile, layers and curves before the track. */
  compile(doc: Project, curveFor: CurveFor, opts: CompileOptions = {}): Upload[] {
    const out: Upload[] = [];

    // Layers: a slot is the layer's index in the array. Grain and states follow their layer's blob.
    doc.layers.forEach((l, i) => {
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
        out.push({ kind: LOAD_LAYER, slot: i, blob: silentLayerBlob(), what: `layer ${i} (removed, muted)` });
        this.layers[i] = null;
      }
      if (this.grainSent[i]) {
        out.push({ kind: LOAD_GRAIN, slot: i, blob: emptyGrain(), what: `grain ${i} (cleared)` });
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

    // Track. It depends on the sections, loop flag, layer order and octave, the curve slots, and which
    // state index each morph marker points at (state edits that keep the order do not touch the track).
    const layerSig = doc.layers.map((l) => `${l.id}:${l.tune.octave}`).join(',');
    const slotSig = [...this.slotOf].map(([id, s]) => `${id}=${s}`).sort().join(',');
    const key = `${doc.loop}|${layerSig}|${slotSig}|${morphSig(doc)}`;
    if (doc.sections !== this.lastSections || key !== this.lastTrackKey) {
      out.push({ kind: LOAD_TRACK, slot: 0, blob: encodeTrack(this.trackInput(doc)), what: 'track' });
      this.lastSections = doc.sections;
      this.lastTrackKey = key;
    }
    return out;
  }

  private compileGrain(out: Upload[], l: Layer, i: number, grainFor: GrainFor | undefined): void {
    const infl = l.influence;
    const wanted = !!infl && infl.useSound && infl.grain !== null;
    if (!wanted) {
      if (this.grainSent[i]) {
        out.push({ kind: LOAD_GRAIN, slot: i, blob: emptyGrain(), what: `grain ${i} (cleared)` });
        this.grainSent[i] = null;
      }
      return;
    }
    const g = grainFor?.(l);
    if (!g || g.data === this.grainSent[i]) return;
    // The engine keeps at most GRAIN_MAX_SAMPLES; the start of the region is the part that matters.
    const data = g.data.length > GRAIN_MAX_SAMPLES ? g.data.subarray(0, GRAIN_MAX_SAMPLES) : g.data;
    out.push({ kind: LOAD_GRAIN, slot: i, blob: encodeGrain(g.rate, g.refCents, data), what: `grain ${i} "${infl!.sourceName}"` });
    this.grainSent[i] = g.data;
  }

  /** The engine restarted: everything is sent again on the next compile. */
  reset(): void {
    this.grainSent = [];
    this.statesSent = [];
    this.layers = [];
    this.slotOf.clear();
    this.curveSent = [];
    this.lastSections = null;
    this.lastTrackKey = null;
  }

  private trackInput(doc: Project): TrackInput {
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
        if (c.kind === 'beat') {
          clips.push({ section: si, layer: li, loopLen: c.loopLen, curve: -1, events: hitEvents(c.hits), morph: morphFor(c.morph, l) });
          return;
        }
        const oct = 1200 * l.tune.octave;
        const morph = morphFor(c.morph, l);
        const curve = c.take ? (this.slotOf.get(c.take) ?? -1) : -1;
        clips.push({
          section: si, layer: li, loopLen: c.loopLen, curve,
          events: c.notes.map((n) => ({ t: n.t, dur: n.dur, cents: n.cents + oct, hard: n.hard, strength: 1 })),
          warp: curve >= 0 ? c.warp : [],
          morph,
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
