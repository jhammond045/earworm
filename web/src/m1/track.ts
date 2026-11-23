// Encoders for the engine's track blob and curve blob. The Rust side is
// crates/klont/src/seq.rs (track) and curve.rs (curve); keep them in step.
// All words are 32-bit little endian.

export const TRACK_MAGIC = 0x31545745; // "EWT1"
export const TRACK_MAGIC2 = 0x32545745; // "EWT2": v1 plus morph markers
export const TRACK_MAGIC3 = 0x33545745; // "EWT3": v2 plus take audio and automation lanes (S5)
export const TRACK_MAGIC4 = 0x34545745; // "EWT4": v3 plus a note curve per note (docs/poly.md P3)
export const TRACK_MAGIC5 = 0x35545745; // "EWT5": v4 plus warp anchor flags and a clip audio mode (Q2)
export const CURVE_MAGIC = 0x31435745; // "EWC1"
export const NOTE_CURVE_MAGIC = 0x314e5745; // "EWN1"
/** Floats per note-curve record: pitch offset (cents), volume, intensity, hardness, onset. */
export const NOTE_REC = 5;
export const FEEL_GRID = 0;
export const FEEL_FREE = 1;
export const PPQ = 960;

export const TRACK_HEADER_WORDS = 8;
export const SECTION_WORDS = 8;
export const CLIP_WORDS = 8;
export const CLIP_WORDS2 = 12;
export const CLIP_WORDS3 = 16;
export const LANE_WORDS = 4;
export const POINT_WORDS = 3;
/** Lane target kinds and point shapes (crates/klont/src/seq.rs). */
export const LANE_VALUE = 0;
export const LANE_MACRO = 1;
export const LANE_LEVEL = 2;
export const LANE_PAN = 3;
export const LANE_PERF = 4;
export const LANE_LINE = 0;
export const LANE_EASE = 1;
export const LANE_STEP = 2;
export const MAX_LANES = 1024;
export const MAX_LANE_POINTS = 65536;
export const MORPH_WORDS = 4;
export const MORPH_LINE = 0;
export const MORPH_EASE = 1;
export const MORPH_LATE = 2;
export const MORPH_STEP = 3;
export const MORPH_WOBBLE = 4;
export const MAX_MORPH = 4096;
export const MAX_STATES = 8;
export const EVENT_WORDS = 16;
export const WARP_WORDS = 2;
export const WARP_WORDS5 = 3;
/** Clip audio modes and warp anchor flags (crates/klont/src/stretch.rs). */
export const AUDIO_READ = 0;
export const AUDIO_HITS = 1;
export const AUDIO_SMOOTH = 2;
export const POINT_HARD = 1;
export const POINT_RUN = 2;
/** Most points of one clip's stretch map the engine holds (crates/klont/src/stretch.rs MAX_POINTS). */
export const MAX_STRETCH_POINTS = 1024;
export const CURVE_STRIDE = 25;
export const CURVE_HEADER_WORDS = 2;

export const MAX_SECTIONS = 64;
export const MAX_CLIPS = 512;
export const MAX_EVENTS = 16384;
export const MAX_WARP = 16384;
export const MAX_LAYERS = 8;
export const MAX_CURVES = 64;
export const MAX_SAMPLES = 64;

const QUIET_NAN = 0x7fc00000;

export interface TrackSection {
  feel: 'grid' | 'free';
  /** Ticks (960 per beat) for grid, seconds for free. */
  length: number;
  /** Grid only; written as 0 for free sections. */
  bpm?: number;
  /** At least 1. Default 1. */
  repeats?: number;
  /** Tempo ramp from the previous section's bpm. Default 0. */
  rampMs?: number;
  /** Stable id (1..2^32-1) so the engine stays on this section when others are added, removed or moved. 0 or missing: none. */
  uid?: number;
}

/** A note or a hit. Unused fields are written as 0 (cents as NaN). */
export interface TrackEvent {
  t: number;
  dur?: number;
  /** Pitch in cents above C0. NaN or undefined: the kit slot's own pitch. */
  cents?: number | null;
  hard?: number;
  slot?: number;
  /** Default 1. */
  strength?: number;
  // Hit shape.
  attack?: number;
  burst?: number;
  bend?: number;
  hold?: number;
  vowel?: number;
  tone?: number;
  endStop?: number;
  endClick?: number;
  /** A poly take's note: the note-curve slot (LOAD_NOTE_CURVES) and the note's index in it.
   *  Any note with one makes the blob v4. */
  noteCurve?: number;
  noteIndex?: number;
}

export interface TrackWarpAnchor {
  takeSec: number;
  clipT: number;
  /** POINT_HARD, POINT_RUN. Any flag in the track makes the blob v5. */
  flags?: number;
}

/** A morph marker (resolution step 1, crates/klont/src/seq.rs "Morph lane"). */
export interface TrackMorph {
  /** Section units; markers are sorted by t here. */
  t: number;
  /** State index 0..MAX_STATES-1. */
  state: number;
  /** Blend time in ms, >= 0. 0 or shape step: at once. */
  lenMs: number;
  /** MORPH_LINE, _EASE, _LATE, _STEP or _WOBBLE. */
  shape: number;
}

export interface TrackClip {
  section: number;
  layer: number;
  loopLen: number;
  /** Curve slot, or -1 / undefined for none. */
  curve?: number;
  events: TrackEvent[];
  warp?: TrackWarpAnchor[];
  /** Morph markers for the layer's states. Any marker in the track makes the blob v2. */
  morph?: TrackMorph[];
  /** Sample id of the take's audio (the raw input on playback), -1 / undefined for none. Any audio
   *  or lane in the track makes the blob v3. */
  audio?: number;
  /** Automation lanes (resolution step 2). */
  lanes?: TrackLane[];
  /** The warp map the take's audio is read through instead of `warp` (a flexed clip keeps its
   *  audio at the performed timing, docs/quant.md Q1). Empty / undefined: `warp`. Makes the blob v3. */
  audioWarp?: TrackWarpAnchor[];
  /** AUDIO_READ (default), AUDIO_HITS or AUDIO_SMOOTH: how the take audio plays through
   *  `audioWarp`, which is then the clip's time map folded into the loop (docs/quant.md Q2).
   *  Any mode but AUDIO_READ makes the blob v5. */
  audioMode?: number;
}

/** An automation lane: LANE_* kind, index (value, macro or perf source), points sorted by t with
 *  v in the target's units and a LANE_LINE, _EASE or _STEP shape. */
export interface TrackLane { kind: number; index: number; points: { t: number; v: number; shape: number }[] }

export interface TrackInput {
  sections: TrackSection[];
  clips: TrackClip[];
  loop?: boolean;
}

function fail(msg: string): never {
  throw new RangeError(`track: ${msg}`);
}

function finite(v: number, what: string): number {
  if (!Number.isFinite(v)) fail(`${what} must be finite, got ${v}`);
  return v;
}

function int(v: number, lo: number, hi: number, what: string): number {
  if (!Number.isInteger(v) || v < lo || v > hi) fail(`${what} must be an integer in ${lo}..${hi}, got ${v}`);
  return v;
}

class Words {
  readonly buf: ArrayBuffer;
  private v: DataView;
  private at = 0;
  constructor(words: number) {
    this.buf = new ArrayBuffer(words * 4);
    this.v = new DataView(this.buf);
  }
  u(x: number) {
    this.v.setUint32(this.at, x >>> 0, true);
    this.at += 4;
  }
  i(x: number) {
    this.v.setInt32(this.at, x | 0, true);
    this.at += 4;
  }
  f(x: number) {
    if (Number.isNaN(x)) this.v.setUint32(this.at, QUIET_NAN, true);
    else this.v.setFloat32(this.at, x, true);
    this.at += 4;
  }
  zeros(n: number) {
    this.at += n * 4;
  }
}

/** Encode a track. Inputs are not modified; events and anchors are sorted
 * (stable) here, and anything the engine would reject throws a RangeError. */
export function encodeTrack(t: TrackInput): ArrayBuffer {
  if (t.sections.length > MAX_SECTIONS) fail(`more than ${MAX_SECTIONS} sections`);
  if (t.clips.length > MAX_CLIPS) fail(`more than ${MAX_CLIPS} clips`);

  const sections = t.sections.map((s, i) => {
    const grid = s.feel === 'grid';
    const repeats = int(s.repeats ?? 1, 1, 0xffffffff, `section ${i} repeats`);
    const length = finite(s.length, `section ${i} length`);
    if (!(length > 0)) fail(`section ${i} length must be > 0`);
    const bpm = grid ? finite(s.bpm ?? NaN, `section ${i} bpm`) : 0;
    if (grid && !(bpm > 0)) fail(`section ${i} bpm must be > 0`);
    const rampMs = finite(s.rampMs ?? 0, `section ${i} rampMs`);
    if (rampMs < 0) fail(`section ${i} rampMs must be >= 0`);
    const uid = int(s.uid ?? 0, 0, 0xffffffff, `section ${i} uid`);
    return { grid, length, bpm, repeats, rampMs, uid };
  });

  const seen = new Set<number>();
  const clips = t.clips.map((c, i) => {
    int(c.section, 0, sections.length - 1, `clip ${i} section`);
    int(c.layer, 0, MAX_LAYERS - 1, `clip ${i} layer`);
    const key = c.section * MAX_LAYERS + c.layer;
    if (seen.has(key)) fail(`clip ${i} repeats section ${c.section} layer ${c.layer}`);
    seen.add(key);
    const loopLen = finite(c.loopLen, `clip ${i} loopLen`);
    if (!(loopLen > 0)) fail(`clip ${i} loopLen must be > 0`);
    const curve = int(c.curve ?? -1, -1, MAX_CURVES - 1, `clip ${i} curve`);
    const events = c.events.map((e, k) => {
      finite(e.t, `clip ${i} event ${k} t`);
      if (e.t < 0) fail(`clip ${i} event ${k} t must be >= 0`);
      const dur = finite(e.dur ?? 0, `clip ${i} event ${k} dur`);
      if (dur < 0) fail(`clip ${i} event ${k} dur must be >= 0`);
      int(e.slot ?? 0, 0, 0xffffffff, `clip ${i} event ${k} slot`);
      return e;
    });
    const sorted = [...events].sort((a, b) => a.t - b.t);
    const anchors = (list: TrackWarpAnchor[] | undefined, what: string) => {
      const out = [...(list ?? [])].map((a, k) => {
        finite(a.takeSec, `clip ${i} ${what} ${k} takeSec`);
        finite(a.clipT, `clip ${i} ${what} ${k} clipT`);
        int(a.flags ?? 0, 0, POINT_HARD | POINT_RUN, `clip ${i} ${what} ${k} flags`);
        return a;
      });
      return out.sort((a, b) => a.clipT - b.clipT);
    };
    const warp = anchors(c.warp, 'anchor');
    const audioWarp = anchors(c.audioWarp, 'audio anchor');
    const morph = (c.morph ?? []).map((m, k) => {
      const t = finite(m.t, `clip ${i} morph ${k} t`);
      if (t < 0) fail(`clip ${i} morph ${k} t must be >= 0`);
      const lenMs = finite(m.lenMs, `clip ${i} morph ${k} lenMs`);
      if (lenMs < 0) fail(`clip ${i} morph ${k} lenMs must be >= 0`);
      return {
        t,
        state: int(m.state, 0, MAX_STATES - 1, `clip ${i} morph ${k} state`),
        lenMs,
        shape: int(m.shape, MORPH_LINE, MORPH_WOBBLE, `clip ${i} morph ${k} shape`),
      };
    });
    morph.sort((a, b) => a.t - b.t);
    const audio = int(c.audio ?? -1, -1, MAX_SAMPLES - 1, `clip ${i} audio`);
    const lanes = (c.lanes ?? []).map((l, k) => {
      int(l.kind, LANE_VALUE, LANE_PERF, `clip ${i} lane ${k} kind`);
      const max = l.kind === LANE_VALUE ? 1023 : l.kind === LANE_MACRO ? 7 : l.kind === LANE_PERF ? 2 : 0;
      int(l.index, 0, max, `clip ${i} lane ${k} index`);
      const points = l.points.map((p, j) => {
        if (!(finite(p.t, `clip ${i} lane ${k} point ${j} t`) >= 0)) fail(`clip ${i} lane ${k} point ${j} t must be >= 0`);
        finite(p.v, `clip ${i} lane ${k} point ${j} v`);
        int(p.shape, LANE_LINE, LANE_STEP, `clip ${i} lane ${k} point ${j} shape`);
        return p;
      });
      return { kind: l.kind, index: l.index, points: [...points].sort((a, b) => a.t - b.t) };
    });
    const audioMode = int(c.audioMode ?? AUDIO_READ, AUDIO_READ, AUDIO_SMOOTH, `clip ${i} audioMode`);
    return { section: c.section, layer: c.layer, loopLen, curve, events: sorted, warp, audioWarp, morph, audio, lanes, audioMode };
  });

  const nEvents = clips.reduce((n, c) => n + c.events.length, 0);
  const nWarp = clips.reduce((n, c) => n + c.warp.length + c.audioWarp.length, 0);
  if (nEvents > MAX_EVENTS) fail(`more than ${MAX_EVENTS} events`);
  if (nWarp > MAX_WARP) fail(`more than ${MAX_WARP} warp anchors`);
  const nMorph = clips.reduce((n, c) => n + c.morph.length, 0);
  if (nMorph > MAX_MORPH) fail(`more than ${MAX_MORPH} morph markers`);
  const nLanes = clips.reduce((n, c) => n + c.lanes.length, 0);
  const nPoints = clips.reduce((n, c) => n + c.lanes.reduce((m, l) => m + l.points.length, 0), 0);
  if (nLanes > MAX_LANES) fail(`more than ${MAX_LANES} automation lanes`);
  if (nPoints > MAX_LANE_POINTS) fail(`more than ${MAX_LANE_POINTS} automation points`);
  // The oldest version that holds everything, so existing tracks and fixtures stay byte-identical.
  const v4n = clips.some((c) => c.events.some((e) => (e.noteCurve ?? -1) >= 0));
  if (clips.some((c) => c.events.some((e) => (e.noteCurve ?? -1) >= MAX_CURVES))) fail(`note-curve slot past ${MAX_CURVES - 1}`);
  const v5 = clips.some((c) => c.audioMode !== AUDIO_READ || [...c.warp, ...c.audioWarp].some((a) => (a.flags ?? 0) !== 0));
  const v4 = v5 || v4n;
  const v3 = v4 || nLanes > 0 || clips.some((c) => c.audio >= 0 || c.audioWarp.length > 0);
  const v2 = v3 || nMorph > 0;

  const w = new Words(
    TRACK_HEADER_WORDS +
      sections.length * SECTION_WORDS +
      clips.length * (v3 ? CLIP_WORDS3 : v2 ? CLIP_WORDS2 : CLIP_WORDS) +
      nEvents * EVENT_WORDS +
      nWarp * (v5 ? WARP_WORDS5 : WARP_WORDS) +
      nMorph * MORPH_WORDS +
      (v3 ? 1 + nLanes * LANE_WORDS + nPoints * POINT_WORDS : 0),
  );
  w.u(v5 ? TRACK_MAGIC5 : v4 ? TRACK_MAGIC4 : v3 ? TRACK_MAGIC3 : v2 ? TRACK_MAGIC2 : TRACK_MAGIC);
  w.u(sections.length);
  w.u(clips.length);
  w.u(nEvents);
  w.u(nWarp);
  w.u(t.loop ? 1 : 0);
  w.u(nMorph);
  w.u(v3 ? nLanes : 0);
  for (const s of sections) {
    w.u(s.grid ? FEEL_GRID : FEEL_FREE);
    w.f(s.length);
    w.f(s.bpm);
    w.u(s.repeats);
    w.f(s.rampMs);
    w.u(s.uid);
    w.zeros(2);
  }
  let fe = 0;
  let fw = 0;
  let fm = 0;
  let fl = 0;
  for (const c of clips) {
    w.u(c.section);
    w.u(c.layer);
    w.f(c.loopLen);
    w.u(fe);
    w.u(c.events.length);
    w.i(c.curve);
    w.u(fw);
    w.u(c.warp.length);
    if (v3) {
      w.u(fm);
      w.u(c.morph.length);
      w.i(c.audio);
      w.u(fl);
      w.u(c.lanes.length);
      // The audio warp run follows the clip's own warp run in the warp array.
      w.u(c.audioWarp.length ? fw + c.warp.length : 0);
      w.u(c.audioWarp.length);
      w.u(c.audioMode);
    } else if (v2) {
      w.u(fm);
      w.u(c.morph.length);
      w.zeros(2);
    }
    fl += c.lanes.length;
    fe += c.events.length;
    fw += c.warp.length + c.audioWarp.length;
    fm += c.morph.length;
  }
  for (const c of clips) {
    for (const e of c.events) {
      w.f(e.t);
      w.f(e.dur ?? 0);
      w.f(e.cents ?? NaN);
      w.f(e.hard ?? 0);
      w.u(e.slot ?? 0);
      w.f(e.strength ?? 1);
      w.f(e.attack ?? 0);
      w.f(e.burst ?? 0);
      w.f(e.bend ?? 0);
      w.f(e.hold ?? 0);
      w.f(e.vowel ?? 0);
      w.f(e.tone ?? 0);
      w.f(e.endStop ?? 0);
      w.f(e.endClick ?? 0);
      if (v4) {
        w.i(e.noteCurve ?? -1);
        w.u(e.noteIndex ?? 0);
      } else {
        w.zeros(2);
      }
    }
  }
  for (const c of clips) {
    for (const a of [...c.warp, ...c.audioWarp]) {
      w.f(a.takeSec);
      w.f(a.clipT);
      if (v5) w.u(a.flags ?? 0);
    }
  }
  for (const c of clips) {
    for (const m of c.morph) {
      w.f(m.t);
      w.u(m.state);
      w.f(m.lenMs);
      w.u(m.shape);
    }
  }
  if (v3) {
    w.u(nPoints);
    let fp = 0;
    for (const c of clips) {
      for (const l of c.lanes) {
        w.u(l.kind);
        w.u(l.index);
        w.u(fp);
        w.u(l.points.length);
        fp += l.points.length;
      }
    }
    for (const c of clips) {
      for (const l of c.lanes) {
        for (const p of l.points) {
          w.f(p.t);
          w.f(p.v);
          w.u(p.shape);
        }
      }
    }
  }
  return w.buf;
}

/** Encode a curve: `frames` holds n records of CURVE_STRIDE floats (24 frame
 * floats then the tuned pitch in cents, NaN when unvoiced). */
export function encodeCurve(frames: Float32Array): ArrayBuffer {
  if (frames.length % CURVE_STRIDE !== 0) fail(`curve length ${frames.length} is not a multiple of ${CURVE_STRIDE}`);
  const n = frames.length / CURVE_STRIDE;
  const w = new Words(CURVE_HEADER_WORDS + frames.length);
  w.u(CURVE_MAGIC);
  w.u(n);
  for (let i = 0; i < frames.length; i++) w.f(frames[i]);
  return w.buf;
}

/**
 * The note curves of a poly take as stored with it (docs/poly.md P3, `decodePolyPass` in
 * m2/analysis.ts): n notes, n records, then per note its start (take seconds), first record and
 * record count, then the records, NOTE_REC floats each.
 */
export function encodeNoteCurves(c: Float32Array): ArrayBuffer {
  const nNotes = c[0], nRecs = c[1];
  if (c.length !== 2 + nNotes * 3 + nRecs * NOTE_REC) fail(`note curves: ${c.length} floats for ${nNotes} notes and ${nRecs} records`);
  const w = new Words(3 + nNotes * 3 + nRecs * NOTE_REC);
  w.u(NOTE_CURVE_MAGIC);
  w.u(nNotes);
  w.u(nRecs);
  for (let i = 0; i < nNotes; i++) {
    w.f(c[2 + 3 * i]);
    w.u(c[3 + 3 * i]);
    w.u(c[4 + 3 * i]);
  }
  for (let i = 2 + 3 * nNotes; i < c.length; i++) w.f(c[i]);
  return w.buf;
}
