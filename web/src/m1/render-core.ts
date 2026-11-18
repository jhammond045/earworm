// Offline rendering on top of klont.wasm exports, without any Worker or DOM
// dependency: the render worker is a thin shell around this, and Vitest runs
// it directly. Everything here is synchronous except the `yieldFn` between
// chunks, which is where a worker sees cancel messages.

import { P, toNorm } from '../gen/params';
import { BUDGET_MAX, CONTROL_CAP, LAYER_LEVEL, LOAD_CURVE, LOAD_LAYER, LOAD_OK, LOAD_TOO_BIG, LOAD_TRACK, MAX_LAYERS, MSG_BYTES,
  MSG_NOTE_ON, MSG_PLAY, MSG_SET_BUDGET, MSG_SET_LAYER, MSG_SET_PARAM, M_ACTIVE_PARTIALS, M_PEAK_L, M_PEAK_R, M_PLAYING,
  METER_LEN, SET_BASE, VOICES_PER_LAYER, encodeMessage, readBufferTable, type BufferTable, type EngineExports, type Msg } from './protocol';

export const RENDER_RATE = 48000;
export const CHUNK = 4096;
export const BLOCK = 128;
const SILENCE = 1e-4; // -80 dB
const TAIL_S = 10;

export interface SongBlobs {
  track: ArrayBuffer;
  /** Layer blobs by layer index; missing or null entries are skipped. */
  layers: (ArrayBuffer | null | undefined)[];
  curves?: { slot: number; buf: ArrayBuffer }[];
}

export interface RenderOptions {
  sampleRate?: number;
  /** Fixed partial budget (MSG_SET_BUDGET); omitted: the engine's offline default, BUDGET_MAX. */
  budget?: number;
  /** Hard stop in seconds of audio. */
  maxSeconds?: number;
  /** Render one pass of a looping track (clears the loop flag in the blob). Default true. */
  once?: boolean;
}

export interface Hooks {
  /** Awaited between chunks; lets a worker process a cancel message. */
  yieldFn?: () => Promise<void>;
  cancelled?: () => boolean;
  onProgress?: (p: { frames: number; seconds: number; ended: boolean }) => void;
}

export interface RenderResult {
  left: Float32Array;
  right: Float32Array;
  sampleRate: number;
  frames: number;
  /** Frame at which the track ended (the transport stopped), or -1 if it did not. */
  endFrame: number;
  stopReason: 'silence' | 'tail' | 'max' | 'cancelled';
  /** Wall time spent inside ew_render, ms. */
  renderMs: number;
  /** Seconds of audio per second of render time. */
  realtime: number;
  peak: number;
  partialsMean: number;
}

/** One engine inside a klont.wasm instance, with the buffer table and helpers. */
export class Core {
  readonly e: number;
  private t!: BufferTable;
  private mem: ArrayBuffer | null = null;
  constructor(readonly ex: EngineExports, readonly sampleRate: number, readonly maxBlock: number) {
    this.e = ex.ew_init(sampleRate, maxBlock);
    this.refresh();
  }
  private refresh(): void {
    if (this.mem === this.ex.memory.buffer) return;
    this.mem = this.ex.memory.buffer as ArrayBuffer;
    this.t = readBufferTable(this.mem, this.ex.ew_buffers(this.e));
  }
  /** Copy a blob into staging and ew_load it. Returns the engine's status. */
  load(kind: number, slot: number, blob: ArrayBuffer): number {
    this.refresh();
    if (blob.byteLength > this.t.stagingCap) return LOAD_TOO_BIG;
    new Uint8Array(this.mem!, this.t.staging, blob.byteLength).set(new Uint8Array(blob));
    const r = this.ex.ew_load(this.e, slot, kind, this.t.staging, blob.byteLength);
    this.refresh();
    return r;
  }
  /** Queue messages for the next render call (at most CONTROL_CAP). */
  send(msgs: Msg[]): void {
    this.refresh();
    if (msgs.length > CONTROL_CAP) throw new RangeError(`render: ${msgs.length} messages in one call, max ${CONTROL_CAP}`);
    const dv = new DataView(this.mem!, this.t.control, CONTROL_CAP * MSG_BYTES);
    msgs.forEach((m, i) => encodeMessage(dv, i * MSG_BYTES, m[0], m[1], m[2], m[3], m[4], m[5] ?? 0));
    new Uint32Array(this.mem!, this.t.controlCount, 1)[0] = msgs.length;
  }
  render(frames: number): number { return this.ex.ew_render(this.e, frames); }
  /** Views into wasm memory: valid until the next render or load. */
  out(frames: number): { l: Float32Array; r: Float32Array } {
    this.refresh();
    return { l: new Float32Array(this.mem!, this.t.outputL, frames), r: new Float32Array(this.mem!, this.t.outputR, frames) };
  }
  meter(): Float32Array { this.refresh(); return new Float32Array(this.mem!, this.t.meter, METER_LEN); }
  free(): void { this.ex.ew_free(this.e); }
}

function check(what: string, status: number): void {
  if (status !== LOAD_OK) throw new Error(`${what}: load failed with status ${status}`);
}

/** Render a song (track, layers, curves) to stereo, stopping per the TDD "Export and offline render". */
export async function renderSong(ex: EngineExports, song: SongBlobs, opts: RenderOptions = {}, hooks: Hooks = {}): Promise<RenderResult> {
  const sr = opts.sampleRate ?? RENDER_RATE;
  const core = new Core(ex, sr, CHUNK);
  try {
    song.layers.forEach((b, i) => { if (b) check(`layer ${i}`, core.load(LOAD_LAYER, i, b)); });
    for (const c of song.curves ?? []) check(`curve ${c.slot}`, core.load(LOAD_CURVE, c.slot, c.buf));
    let track = song.track;
    if (opts.once ?? true) {
      track = track.slice(0);
      new Uint32Array(track, 0, 6)[5] = 0; // header word 5: loop flag
    }
    check('track', core.load(LOAD_TRACK, 0, track));
    const first: Msg[] = [];
    if (opts.budget !== undefined) first.push([MSG_SET_BUDGET, 0, 0, 0, opts.budget]);
    first.push([MSG_PLAY, 0, 0, 0, 0]);
    core.send(first);

    const maxFrames = opts.maxSeconds !== undefined ? Math.round(opts.maxSeconds * sr) : Infinity;
    const chunksL: Float32Array[] = [], chunksR: Float32Array[] = [];
    let frames = 0, endFrame = -1, silent = 0, renderMs = 0, peak = 0, partials = 0, chunks = 0;
    let reason: RenderResult['stopReason'] = 'silence';
    let lastPost = 0;
    for (;;) {
      const n = Math.min(CHUNK, maxFrames - frames);
      const t0 = performance.now();
      core.render(n);
      renderMs += performance.now() - t0;
      const { l, r } = core.out(n);
      chunksL.push(l.slice());
      chunksR.push(r.slice());
      let cp = 0;
      for (let i = 0; i < n; i++) cp = Math.max(cp, Math.abs(l[i]), Math.abs(r[i]));
      peak = Math.max(peak, cp);
      partials += core.meter()[M_ACTIVE_PARTIALS];
      chunks++;
      frames += n;
      silent = cp < SILENCE ? silent + n : 0;
      if (endFrame < 0 && core.meter()[M_PLAYING] < 0.5) endFrame = frames;
      const now = performance.now();
      if (now - lastPost > 30) {
        lastPost = now;
        hooks.onProgress?.({ frames, seconds: frames / sr, ended: endFrame >= 0 });
      }
      if (hooks.cancelled?.()) { reason = 'cancelled'; break; }
      if (frames >= maxFrames) { reason = 'max'; break; }
      if (endFrame >= 0) {
        if (silent >= sr) { reason = 'silence'; break; }
        if (frames - endFrame >= TAIL_S * sr) { reason = 'tail'; break; }
      }
      if (hooks.yieldFn) await hooks.yieldFn();
      if (hooks.cancelled?.()) { reason = 'cancelled'; break; }
    }
    hooks.onProgress?.({ frames, seconds: frames / sr, ended: endFrame >= 0 });
    const left = new Float32Array(frames), right = new Float32Array(frames);
    let o = 0;
    for (let i = 0; i < chunksL.length; i++) { left.set(chunksL[i], o); right.set(chunksR[i], o); o += chunksL[i].length; }
    return { left, right, sampleRate: sr, frames, endFrame, stopReason: reason, renderMs,
      realtime: renderMs > 0 ? frames / sr / (renderMs / 1000) : 0, peak, partialsMean: partials / chunks };
  } finally {
    core.free();
  }
}

// ---------------------------------------------------------------- worst case

/** The native bench's worst-case scene (crates/klont/src/bin/bench.rs, worst_case): 8 layers x
 *  8 held voices with every partial audible, vibrato on every voice, delay and reverb on every
 *  layer. Two message batches: setup (65 messages), then the 64 note-ons. */
export function worstCaseMessages(budget: number): { setup: Msg[]; notes: Msg[] } {
  const setup: Msg[] = [];
  const set = (l: number, id: keyof typeof P, real: number) => {
    const i = P[id];
    setup.push([MSG_SET_PARAM, l, SET_BASE, i, toNorm(i, real)]);
  };
  for (let l = 0; l < MAX_LAYERS; l++) {
    set(l, 'src.tilt', -1.5);
    set(l, 'src.noise_mix', 0.1);
    set(l, 'env.sustain', 1.0);
    set(l, 'lfo1.rate', 5.0 + l * 0.3);
    set(l, 'lfo1.to_pitch', 20.0);
    set(l, 'delay.mix', 0.2);
    set(l, 'verb.mix', 0.2);
    setup.push([MSG_SET_LAYER, l, 0, LAYER_LEVEL, -24.0]);
  }
  setup.push([MSG_SET_BUDGET, 0, 0, 0, budget]);
  const notes: Msg[] = [];
  for (let l = 0; l < MAX_LAYERS; l++) {
    for (let v = 0; v < VOICES_PER_LAYER; v++) {
      // 55 Hz up: low notes keep all 64 harmonics under 0.45 fs.
      notes.push([MSG_NOTE_ON, l, 0, 0, 3300 + 100 * (l * 2 + v * 5)]);
    }
  }
  return { setup, notes };
}

export interface WorstResult {
  budget: number;
  seconds: number;
  wallMs: number;
  /** Seconds of audio per second of CPU. */
  realtime: number;
  partialsMean: number;
  nsPerPartialSample: number;
  peak: number;
}

const WARMUP_BLOCKS = 200;

/** Engine in the worst-case scene, warmed up past the attacks. */
function worstCore(ex: EngineExports, budget: number, sr: number): Core {
  const core = new Core(ex, sr, BLOCK);
  const { setup, notes } = worstCaseMessages(budget);
  core.send(setup);
  core.render(BLOCK);
  core.send(notes);
  for (let i = 0; i < WARMUP_BLOCKS; i++) core.render(BLOCK);
  return core;
}

/** Render `seconds` of the worst-case scene in 128-frame blocks at a fixed budget. Like the native bench:
 *  timing covers the whole block loop, a second of audio at a time (clock reads are 5 to 100 us, a block is ~200 us). */
export async function runWorstCase(ex: EngineExports, budget: number, seconds: number, hooks: Hooks = {}, sr = RENDER_RATE): Promise<WorstResult> {
  const core = worstCore(ex, budget, sr);
  try {
    const blocks = Math.floor(seconds * sr / BLOCK);
    const perBatch = Math.round(sr / BLOCK);
    let partialSamples = 0, wallMs = 0, peak = 0, done = 0;
    while (done < blocks) {
      const n = Math.min(perBatch, blocks - done);
      const m = core.meter();
      const t0 = performance.now();
      for (let i = 0; i < n; i++) {
        core.render(BLOCK);
        partialSamples += m[M_ACTIVE_PARTIALS] * BLOCK;
      }
      wallMs += performance.now() - t0;
      peak = Math.max(peak, m[M_PEAK_L], m[M_PEAK_R]);
      done += n;
      hooks.onProgress?.({ frames: done * BLOCK, seconds: done * BLOCK / sr, ended: false });
      if (hooks.cancelled?.()) break;
      if (hooks.yieldFn) await hooks.yieldFn();
    }
    const audio = done * BLOCK / sr;
    return { budget, seconds: audio, wallMs, realtime: audio / (wallMs / 1000),
      partialsMean: partialSamples / Math.max(1, done * BLOCK),
      nsPerPartialSample: wallMs * 1e6 / Math.max(1, partialSamples), peak };
  } finally {
    core.free();
  }
}

export interface BlockTimingResult {
  budget: number;
  blocks: number;
  quantumMs: number;
  p50: number;
  p99: number;
  max: number;
  mean: number;
  /** Blocks that took longer than the quantum. */
  overQuantum: number;
  partialsMean: number;
}

/** Worst-case scene in 128-frame blocks, each timed on its own. Percentiles in ms. */
export async function runBlockTiming(ex: EngineExports, budget: number, seconds: number, hooks: Hooks = {}, sr = RENDER_RATE): Promise<BlockTimingResult> {
  const core = worstCore(ex, budget, sr);
  try {
    const blocks = Math.floor(seconds * sr / BLOCK);
    const times = new Float64Array(blocks);
    const m = core.meter();
    let partials = 0;
    for (let i = 0; i < blocks; i++) {
      const t0 = performance.now();
      core.render(BLOCK);
      times[i] = performance.now() - t0;
      partials += m[M_ACTIVE_PARTIALS];
      if (i % 3750 === 3749) {
        hooks.onProgress?.({ frames: (i + 1) * BLOCK, seconds: (i + 1) * BLOCK / sr, ended: false });
        if (hooks.cancelled?.()) break;
        if (hooks.yieldFn) await hooks.yieldFn();
      }
    }
    const quantumMs = BLOCK / sr * 1000;
    const sorted = Float64Array.from(times).sort();
    const q = (p: number) => sorted[Math.min(sorted.length - 1, Math.floor(p * sorted.length))];
    let sum = 0, over = 0;
    for (const t of times) { sum += t; if (t > quantumMs) over++; }
    return { budget, blocks, quantumMs, p50: q(0.5), p99: q(0.99), max: sorted[sorted.length - 1], mean: sum / blocks,
      overQuantum: over, partialsMean: partials / blocks };
  } finally {
    core.free();
  }
}

export { BUDGET_MAX };
