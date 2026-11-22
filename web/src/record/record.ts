// Recording a pass (docs/synth.md "DAW use", stage S5). While Record is on, what is played live is
// kept as data: notes from MIDI and the on-screen keys, moves of values, macros, level and pan and
// of the wheel, bend and pressure, and scene taps. When the pass ends it becomes clip edits that
// the caller runs as one undo group, with the pass's take if the mic recorded one:
//
// - Notes and hits go into the clip of the layer they played, at the transport position where
//   they sounded minus the output latency (the player heard the song that much late), wrapped
//   into the clip's loop, with their velocity. A melody note keeps the pitch that sounded: the
//   layer's Tune octave, which the compiler adds, is taken off.
// - Moves become automation lanes, one per target. A recorded lane overwrites the lane's old
//   points from the first move to the end of the pass. A new lane starts with a point at the
//   start of the pass holding (a step) the value the target had then, so playback starts as the
//   pass did.
// - Scene taps become morph markers (the tap's state, over the scene time). When the clip had no
//   markers, one at the start of the pass holds the state the layer was in, as heard.
// - Where the hand left each value, macro, level and pan becomes the layer's own (in the state
//   being edited), in the same undo group: the engine already plays it there.
//
// Times are engine samples (the engine's clock, which the meter block reports); `marks` turn them
// into transport positions. Nothing here talks to the engine or the page, so it runs in tests.

import { commands, newId } from '../project/commands';
import { applyOps } from '../project/patch';
import { sectionLength } from '../project/compile';
import { positionAt, type ClockMark } from '../project/overdub';
import type { AutoLane, BeatClip, Clip, Hit, LanePoint, Layer, MelodyClip, MorphMarker, Note, Project, Take, WarpAnchor } from '../project/types';
import { LANE_FIXED, PPQ } from '../project/types';
import { layoutOf } from '../synth/patch';

/** Lane points closer together than this (seconds) are thinned to the last of them. */
export const LANE_STEP_S = 0.01;
/** Shortest note a pass keeps, in seconds of the section's rate. */
const MIN_NOTE_S = 0.02;

interface PassNote { layer: string; start: number; end: number; cents: number; slot: number; vel: number }
export interface PassMove { layer: string; target: string; v: number; sample: number }

/** What the page reports while a pass records (AppCapture implements it; idle otherwise). */
export interface RecordHooks {
  readonly recording: boolean;
  /** A note or hit started; returns a handle for noteOff. */
  noteOn(layer: string, cents: number, slot: number, vel: number): number;
  noteOff(handle: number): void;
  /** A hand moved a target (types.ts AutoLane units). */
  move(layer: string, target: string, v: number): void;
  scene(layer: string, state: string, ms: number): void;
  /** `doc` with where the hand has left things so far in this pass, for previews. */
  overlay(doc: Project): Project;
}

export const idleRecord: RecordHooks = {
  recording: false, noteOn: () => 0, noteOff: () => {}, move: () => {}, scene: () => {}, overlay: (d) => d,
};
interface PassScene { layer: string; state: string; ms: number; sample: number }

export class Pass {
  readonly notes: PassNote[] = [];
  readonly moves: PassMove[] = [];
  readonly scenes: PassScene[] = [];
  /** The state each layer's scene held when the pass started (layers that had a scene tapped). */
  readonly sceneAtStart = new Map<string, string>();
  private open = new Map<number, PassNote>();
  private seq = 0;
  end = -1;

  constructor(readonly start: number) {}

  /** A note started sounding at engine sample `sample`. Returns a handle for `noteOff`. */
  noteOn(layer: string, cents: number, slot: number, vel: number, sample: number): number {
    const n: PassNote = { layer, start: sample, end: -1, cents, slot, vel };
    this.notes.push(n);
    this.open.set(++this.seq, n);
    return this.seq;
  }

  noteOff(handle: number, sample: number): void {
    const n = this.open.get(handle);
    if (!n) return;
    n.end = sample;
    this.open.delete(handle);
  }

  /** A value (key of the layer's patch), macro (`macro1`..), `layer.level`, `layer.pan`, `wheel`,
   *  `bend` or `pressure` was set to `v` (its lane units, types.ts AutoLane). */
  move(layer: string, target: string, v: number, sample: number): void {
    this.moves.push({ layer, target, v, sample });
  }

  scene(layer: string, state: string, ms: number, sample: number): void {
    this.scenes.push({ layer, state, ms, sample });
  }

  /** End the pass: notes still held end here. */
  finish(sample: number): void {
    this.end = sample;
    for (const n of this.open.values()) n.end = sample;
    this.open.clear();
  }

  get empty(): boolean {
    return !this.notes.length && !this.moves.length && !this.scenes.length;
  }
}

/** How a pass's samples map to the song. */
export interface Placement {
  /** Transport readings taken during the pass, oldest first (at least one, while playing). */
  marks: ClockMark[];
  /** Output latency in samples: what the player heard at sample s, the engine played at s - this. */
  outLatency: number;
}

/** A command for Store.batch. */
export interface Cmd { name: string; args: Record<string, unknown> }

/** `x` wrapped into [0, len). A rounding error just under a loop start counts as the start. */
const wrap = (x: number, len: number) => {
  const w = ((x % len) + len) % len;
  return len - w < 1e-9 * Math.max(1, len) ? 0 : w;
};

/** The newest mark at or before sample `s` (the first one before any). */
function markAt(marks: ClockMark[], s: number): ClockMark {
  let m = marks[0];
  for (const k of marks) if (k.sample <= s) m = k;
  return m;
}

/** The value `target` of `layer` had in `doc` (the state being edited), in lane units. */
export type ValueOf = (layer: Layer, target: string) => number;

/** A target's value in a document's state `stateId` (else the layer's first state). */
export function stateValue(layer: Layer, target: string, stateId?: string): number {
  const st = layer.states.find((s) => s.id === stateId) ?? layer.states[0];
  const m = /^macro([1-8])$/.exec(target);
  if (m) return st?.macros[Number(m[1]) - 1] ?? 0.5;
  if (target === 'layer.level') return layer.level;
  if (target === 'layer.pan') return layer.pan;
  if (target in LANE_FIXED) return 0;
  const lay = layoutOf(layer.patch);
  return st?.values[target] ?? lay.init[lay.index.get(target) ?? -1] ?? 0;
}

function emptyClip(layer: Layer, len: number): Clip {
  const base = { take: '', takes: [] as string[], loopLen: len, timeSnap: 0, morph: [] as MorphMarker[] };
  return layer.kind === 'beat' ? { kind: 'beat', ...base, hits: [] } : { kind: layer.kind, ...base, notes: [], warp: [] };
}

/** Keep one point per LANE_STEP_S of samples (the last in each step), and the last point. */
function thin(points: { sample: number; v: number }[], sr: number): { sample: number; v: number }[] {
  const step = LANE_STEP_S * sr;
  const out: { sample: number; v: number }[] = [];
  for (const p of points) {
    const last = out[out.length - 1];
    if (last && p.sample - last.sample < step && out.length > 1) out[out.length - 1] = p;
    else out.push(p);
  }
  return out;
}

/**
 * The clip edits that put `pass` into `doc`: one `setClip` per clip it touched. `clips` gives clips
 * to build on instead of the document's (the pass's take clip, placed in the same undo group).
 * `valueOf` gives a target's value when the pass started (new lanes start with it).
 */
export function passCommands(doc: Project, pass: Pass, place: Placement, sr: number, valueOf: ValueOf, clips: Map<string, Clip> = new Map(), stateOf: (l: Layer) => string = (l) => l.states[0].id): Cmd[] {
  if (!place.marks.length) throw new RangeError('the transport gave no position during the pass');
  const out = new Map<string, { section: string; layer: Layer; clip: Clip }>();
  const touch = (si: number, layerId: string) => {
    const s = doc.sections[si];
    const l = doc.layers.find((x) => x.id === layerId);
    if (!s || !l) return null;
    const key = `${s.id}/${l.id}`;
    let e = out.get(key);
    if (!e) {
      const have = clips.get(key) ?? s.clips[l.id];
      e = { section: s.id, layer: l, clip: have ?? emptyClip(l, sectionLength(s)) };
      out.set(key, e);
    }
    return e;
  };
  /** Section, clip position and units per sample of the sound at engine sample `s`. */
  const where = (s: number) => {
    const m = markAt(place.marks, s);
    const at = positionAt(m, s - place.outLatency);
    return { si: at.section, pos: at.pos, ups: m.unitsPerSample };
  };

  // Notes and hits.
  for (const n of pass.notes) {
    const w = where(n.start);
    const e = touch(w.si, n.layer);
    if (!e) continue;
    const grid = doc.sections[w.si].feel === 'grid';
    const len = e.clip.loopLen;
    const minDur = grid ? (MIN_NOTE_S * doc.sections[w.si].bpm * PPQ) / 60 : MIN_NOTE_S;
    let t = wrap(w.pos, len);
    let dur = Math.max(minDur, (Math.max(n.end, n.start) - n.start) * w.ups);
    if (grid) { t = Math.min(len - 1, Math.round(t)); dur = Math.max(1, Math.round(dur)); }
    if (e.clip.kind === 'beat') {
      const hit: Hit = {
        id: newId('hit'), t, dur, slot: n.slot, strength: n.vel,
        shape: { attack: 0, burst: 0, cents: null, bend: 0, hold: 0, vowel: 0, tone: 0, endStop: 0, endClick: 0 },
      };
      const c = e.clip as BeatClip;
      e.clip = { ...c, hits: [...c.hits, hit].sort((a, b) => a.t - b.t) };
    } else {
      const note: Note = { id: newId('note'), t, dur, cents: n.cents - 1200 * e.layer.tune.octave, hard: 0, vel: n.vel };
      const c = e.clip as MelodyClip;
      e.clip = { ...c, notes: [...c.notes, note].sort((a, b) => a.t - b.t) };
    }
  }

  // Lanes: group the moves by clip and target, in time order.
  const groups = new Map<string, { si: number; layer: string; target: string; pts: { sample: number; v: number }[] }>();
  for (const mv of [...pass.moves].sort((a, b) => a.sample - b.sample)) {
    const w = where(mv.sample);
    const key = `${w.si}/${mv.layer}/${mv.target}`;
    const g = groups.get(key) ?? { si: w.si, layer: mv.layer, target: mv.target, pts: [] };
    g.pts.push({ sample: mv.sample, v: mv.v });
    groups.set(key, g);
  }
  const startW = where(pass.start);
  for (const g of groups.values()) {
    const e = touch(g.si, g.layer);
    if (!e) continue;
    const len = e.clip.loopLen;
    const pts = thin(g.pts, sr).map((p): LanePoint => ({ t: wrap(where(p.sample).pos, len), v: p.v, shape: 'line' }));
    const lanes = e.clip.auto ?? [];
    const old = lanes.find((x) => x.target === g.target);
    // The old points from the first move to the end of the pass are replaced (wrapping with the loop).
    const t0 = pts[0].t;
    const t1 = wrap(where(pass.end >= 0 ? pass.end : g.pts[g.pts.length - 1].sample).pos, len);
    const passLen = ((pass.end >= 0 ? pass.end : g.pts[g.pts.length - 1].sample) - g.pts[0].sample) * where(g.pts[0].sample).ups;
    const inside = (t: number) => (passLen >= len ? true : t0 <= t1 ? t >= t0 && t <= t1 : t >= t0 || t <= t1);
    const kept = (old?.points ?? []).filter((p) => !inside(p.t));
    const first: LanePoint[] = [];
    if (!old && g.si === startW.si) {
      const t = wrap(startW.pos, len);
      // Held (a step) until the first move, as it was heard.
      if (t < t0) first.push({ t, v: valueOf(e.layer, g.target), shape: 'step' });
    }
    const merged = [...kept, ...first, ...pts].sort((a, b) => a.t - b.t);
    // Points recorded later in the pass win where the loop wrapped onto earlier ones.
    const points = merged.filter((p, i) => i === merged.length - 1 || merged[i + 1].t !== p.t);
    const lane: AutoLane = { target: g.target, points };
    e.clip = { ...e.clip, auto: old ? lanes.map((x) => (x === old ? lane : x)) : [...lanes, lane] };
  }

  // Scene taps.
  for (const sc of [...pass.scenes].sort((a, b) => a.sample - b.sample)) {
    const w = where(sc.sample);
    const e = touch(w.si, sc.layer);
    if (!e || !e.layer.states.some((s) => s.id === sc.state)) continue;
    const len = e.clip.loopLen;
    const grid = doc.sections[w.si].feel === 'grid';
    const had = e.clip.morph ?? [];
    let morph = had;
    if (!had.length && w.si === startW.si) {
      const st = pass.sceneAtStart.get(sc.layer) ?? e.layer.states[0].id;
      const t = wrap(startW.pos, len);
      morph = [{ t: grid ? Math.min(len - 1, Math.round(t)) : t, state: st, lenMs: 0, shape: 'step' }];
    }
    const t = wrap(w.pos, len);
    const m: MorphMarker = { t: grid ? Math.min(len - 1, Math.round(t)) : t, state: sc.state, lenMs: Math.max(0, sc.ms), shape: 'line' };
    e.clip = { ...e.clip, morph: [...morph.filter((x) => x.t !== m.t), m].sort((a, b) => a.t - b.t) };
  }

  const cmds: Cmd[] = [...out.values()].map((e) => ({ name: 'setClip', args: { section: e.section, layer: e.layer.id, clip: e.clip } }));
  return [...cmds, ...handCommands(doc, pass.moves, stateOf)];
}

/** `doc` with where the hand left each target in `moves`. */
export function withHand(doc: Project, moves: PassMove[], stateOf: (l: Layer) => string): Project {
  let d = doc;
  for (const c of handCommands(doc, moves, stateOf)) {
    const make = commands[c.name as keyof typeof commands] as (doc: Project, a: unknown) => import('../project/patch').Op[];
    d = applyOps(d, make(d, c.args)).doc;
  }
  return d;
}

/** Where the hand left each value, macro, level and pan: commands for the state being edited. */
function handCommands(doc: Project, moves: PassMove[], stateOf: (l: Layer) => string): Cmd[] {
  const cmds: Cmd[] = [];
  const last = new Map<string, PassMove>();
  for (const mv of [...moves].sort((a, b) => a.sample - b.sample)) last.set(`${mv.layer}/${mv.target}`, mv);
  for (const l of doc.layers) {
    const mine = [...last.values()].filter((m) => m.layer === l.id);
    if (!mine.length) continue;
    const state = stateOf(l);
    const st = l.states.find((x) => x.id === state) ?? l.states[0];
    const values: Record<string, number> = {};
    const macros = [...st.macros];
    let macroMoved = false;
    const fields: Record<string, number> = {};
    for (const m of mine) {
      const k = /^macro([1-8])$/.exec(m.target);
      if (k) { macros[Number(k[1]) - 1] = m.v; macroMoved = true; }
      else if (m.target === 'layer.level') fields.level = m.v;
      else if (m.target === 'layer.pan') fields.pan = m.v;
      else if (!(m.target in LANE_FIXED)) values[m.target] = m.v;
    }
    if (Object.keys(values).length) cmds.push({ name: 'setValues', args: { layer: l.id, state: st.id, values } });
    if (macroMoved) cmds.push({ name: 'setState', args: { layer: l.id, id: st.id, macros } });
    if (Object.keys(fields).length) cmds.push({ name: 'setLayer', args: { id: l.id, ...fields } });
  }
  return cmds;
}

/** The clip for a take recorded into a listening layer: the take's audio and features, no notes of
 *  its own (the layer's notes come from MIDI or the keys). Warp anchors put take second 0 at the
 *  position the take started, minus the round trip, at the section's nominal rate. */
export function listeningClip(doc: Project, sectionId: string, layerId: string, take: Take, startSample: number, mark: ClockMark, roundTripSamples: number, seconds: number): Clip {
  const si = doc.sections.findIndex((s) => s.id === sectionId);
  const section = doc.sections[si] ?? (() => { throw new RangeError(`no section ${sectionId}`); })();
  const layer = doc.layers.find((l) => l.id === layerId) ?? (() => { throw new RangeError(`no layer ${layerId}`); })();
  const existing = section.clips[layerId];
  const loopLen = existing?.loopLen ?? sectionLength(section);
  const ups = section.feel === 'grid' ? (section.bpm * PPQ) / 60 : 1;
  const at = positionAt(mark, startSample - roundTripSamples);
  const p0 = wrap(at.section === si ? at.pos : 0, loopLen);
  const warp: WarpAnchor[] = [{ takeSec: 0, clipT: p0 }, { takeSec: seconds, clipT: p0 + seconds * ups }];
  const base = {
    take: take.id,
    takes: [...(existing?.takes ?? []).filter((id) => id !== take.id), take.id],
    loopLen,
    timeSnap: existing?.timeSnap ?? 0,
    morph: existing?.morph ?? [],
    ...(existing?.auto ? { auto: existing.auto } : {}),
  };
  if (layer.kind === 'beat') return { kind: 'beat', ...base, hits: existing?.kind === 'beat' ? existing.hits : [] };
  return { kind: layer.kind, ...base, notes: existing && existing.kind !== 'beat' ? existing.notes : [], warp };
}
