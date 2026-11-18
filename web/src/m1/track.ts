// Encoders for the engine's track blob and curve blob. The Rust side is
// crates/klont/src/seq.rs (track) and curve.rs (curve); keep them in step.
// All words are 32-bit little endian.

export const TRACK_MAGIC = 0x31545745; // "EWT1"
export const CURVE_MAGIC = 0x31435745; // "EWC1"
export const FEEL_GRID = 0;
export const FEEL_FREE = 1;
export const PPQ = 960;

export const TRACK_HEADER_WORDS = 8;
export const SECTION_WORDS = 8;
export const CLIP_WORDS = 8;
export const EVENT_WORDS = 16;
export const WARP_WORDS = 2;
export const CURVE_STRIDE = 25;
export const CURVE_HEADER_WORDS = 2;

export const MAX_SECTIONS = 64;
export const MAX_CLIPS = 512;
export const MAX_EVENTS = 16384;
export const MAX_WARP = 16384;
export const MAX_LAYERS = 8;
export const MAX_CURVES = 64;

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
}

export interface TrackWarpAnchor {
  takeSec: number;
  clipT: number;
}

export interface TrackClip {
  section: number;
  layer: number;
  loopLen: number;
  /** Curve slot, or -1 / undefined for none. */
  curve?: number;
  events: TrackEvent[];
  warp?: TrackWarpAnchor[];
}

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
    const warp = [...(c.warp ?? [])].map((a, k) => {
      finite(a.takeSec, `clip ${i} anchor ${k} takeSec`);
      finite(a.clipT, `clip ${i} anchor ${k} clipT`);
      return a;
    });
    warp.sort((a, b) => a.clipT - b.clipT);
    return { section: c.section, layer: c.layer, loopLen, curve, events: sorted, warp };
  });

  const nEvents = clips.reduce((n, c) => n + c.events.length, 0);
  const nWarp = clips.reduce((n, c) => n + c.warp.length, 0);
  if (nEvents > MAX_EVENTS) fail(`more than ${MAX_EVENTS} events`);
  if (nWarp > MAX_WARP) fail(`more than ${MAX_WARP} warp anchors`);

  const w = new Words(
    TRACK_HEADER_WORDS +
      sections.length * SECTION_WORDS +
      clips.length * CLIP_WORDS +
      nEvents * EVENT_WORDS +
      nWarp * WARP_WORDS,
  );
  w.u(TRACK_MAGIC);
  w.u(sections.length);
  w.u(clips.length);
  w.u(nEvents);
  w.u(nWarp);
  w.u(t.loop ? 1 : 0);
  w.zeros(2);
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
  for (const c of clips) {
    w.u(c.section);
    w.u(c.layer);
    w.f(c.loopLen);
    w.u(fe);
    w.u(c.events.length);
    w.i(c.curve);
    w.u(fw);
    w.u(c.warp.length);
    fe += c.events.length;
    fw += c.warp.length;
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
      w.zeros(2);
    }
  }
  for (const c of clips) {
    for (const a of c.warp) {
      w.f(a.takeSec);
      w.f(a.clipT);
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
