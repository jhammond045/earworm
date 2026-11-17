// Main-thread side of the audio engine: loads engine.wasm, starts the
// AudioWorklet that hosts it, and exposes the four shared blocks from the TDD
// (control ring, meter block, mic ring, frame ring) plus an output ring used
// by the delay page. With crossOriginIsolated false (or ?fallback=1), the same
// data travels over the worklet's MessagePort instead (TDD "Cross-origin
// isolation and the fallback").
//
// Signatures here are the contract the pages code against.

import workletUrl from '../worklet/engine-processor.ts?worker&url';
import type { ProcessorOptions } from '../worklet/engine-processor';
import { CONTROL_CAP, MSG_BYTES, MSG_NOTE_OFF, MSG_NOTE_ON, MSG_PANIC, MSG_RESET_SINGER,
  MSG_SET_LIVE, MSG_SET_NOISE_BAND, MSG_SET_PARAM, MSG_SET_TONE_BAND, RECORD_STRIDE, BT_CONTROL, BT_CONTROL_COUNT,
  BT_FIELDS, BT_OUTPUT, type EngineExports } from './protocol';
import { ControlRing, MeterBlock, RecordRing, encodeMessage } from './rings';
export { encodeMessage } from './rings';

export type TransportMode = 'shared' | 'message';

export interface EngineHostOptions {
  /** Band count for the vowel shape and band gains: 16 (TDD) or 24 (M0 trial). */
  nBands: 16 | 24;
  /** Force the MessagePort fallback even when cross-origin isolated. */
  forceFallback?: boolean;
}

/** One frame-ring record: a feature frame plus the tuned pitch (protocol.ts). */
export interface FrameBatch {
  /** Records copied out since the cursor, each RECORD_STRIDE floats, oldest first. */
  records: Float32Array;
  count: number;
  /** Pass back on the next call. */
  cursor: number;
}

const FRAME_CAP = 6000;
const CAPTURE_S = 60;
const WASM_URL = `${import.meta.env.BASE_URL}wasm/engine.wasm`;

let bytesPromise: Promise<ArrayBuffer> | null = null;

/** Engine wasm bytes, fetched once and cached. */
export function loadEngineBytes(): Promise<ArrayBuffer> {
  bytesPromise ??= fetch(WASM_URL).then((r) => {
    if (!r.ok) throw new Error(`engine.wasm: HTTP ${r.status}`);
    return r.arrayBuffer();
  }).catch((e) => { bytesPromise = null; throw e; });
  return bytesPromise;
}

/** getUserMedia constraints from the TDD: EC, NS and AGC off, one channel. */
export function micConstraints(deviceId?: string): MediaStreamConstraints {
  const audio: MediaTrackConstraints = { echoCancellation: false, noiseSuppression: false, autoGainControl: false, channelCount: 1 };
  if (deviceId) audio.deviceId = { exact: deviceId };
  return { audio };
}

/** AudioContext with the interactive latency hint. Call from a user gesture. */
export function createAudioContext(): AudioContext {
  return new AudioContext({ latencyHint: 'interactive' });
}

/** True when the page asked for the fallback (?fallback=1) or is not isolated. */
export function wantsFallback(): boolean {
  return new URLSearchParams(globalThis.location?.search ?? '').get('fallback') === '1' || !globalThis.crossOriginIsolated;
}

const moduleAdded = new WeakSet<BaseAudioContext>();

export class EngineHost {
  readonly mode: TransportMode;
  readonly sampleRate: number;
  private readonly ring?: ControlRing; // shared mode
  private queue: number[][] = []; // message mode: [type, index, param, value]
  private readonly meters: MeterBlock;
  private readonly frames: RecordRing;
  private readonly mic: RecordRing;
  private readonly out: RecordRing;

  private constructor(readonly ctx: AudioContext, readonly node: AudioWorkletNode, readonly nBands: 16 | 24,
                      mode: TransportMode, bufs: Bufs, ring?: ControlRing) {
    this.mode = mode;
    this.sampleRate = ctx.sampleRate;
    this.ring = ring;
    this.meters = new MeterBlock(bufs.meter);
    this.frames = new RecordRing(bufs.frames, FRAME_CAP, RECORD_STRIDE);
    this.mic = new RecordRing(bufs.mic, bufs.capCap, 1);
    this.out = new RecordRing(bufs.out, bufs.capCap, 1);
  }

  /** Adds the worklet module, instantiates the engine in it, sets up rings. */
  static async create(ctx: AudioContext, opts: EngineHostOptions): Promise<EngineHost> {
    const [bytes] = await Promise.all([loadEngineBytes(), addModuleOnce(ctx)]);
    const shared = !!globalThis.crossOriginIsolated && !opts.forceFallback && typeof SharedArrayBuffer !== 'undefined';
    const capCap = Math.ceil(CAPTURE_S * ctx.sampleRate);
    const mk = (n: number): ArrayBufferLike => (shared ? new SharedArrayBuffer(n) : new ArrayBuffer(n));
    const bufs: Bufs = {
      meter: mk(MeterBlock.BYTES), frames: mk(RecordRing.bytes(FRAME_CAP, RECORD_STRIDE)),
      mic: mk(RecordRing.bytes(capCap, 1)), out: mk(RecordRing.bytes(capCap, 1)), capCap,
    };
    const control = shared ? new SharedArrayBuffer(ControlRing.BYTES) : undefined;
    const po: ProcessorOptions = { bytes, nBands: opts.nBands, shared };
    if (shared) Object.assign(po, { control, meter: bufs.meter, frames: bufs.frames, mic: bufs.mic, out: bufs.out });

    const node = new AudioWorkletNode(ctx, 'earworm-engine', {
      numberOfInputs: 1, numberOfOutputs: 1, outputChannelCount: [2],
      channelCount: 1, channelCountMode: 'explicit', processorOptions: po,
    });
    const host = new EngineHost(ctx, node, opts.nBands, shared ? 'shared' : 'message', bufs,
      control && new ControlRing(control));
    await new Promise<void>((resolve, reject) => {
      node.onprocessorerror = () => reject(new Error('engine processor failed to start'));
      node.port.onmessage = (ev) => {
        if (ev.data.t === 'ready') resolve();
        else host.onWorkletMessage(ev.data);
      };
    });
    node.port.onmessage = (ev) => host.onWorkletMessage(ev.data);
    node.connect(ctx.destination);
    return host;
  }

  /** Message mode: copy what the worklet posted into the local mirrors. */
  private onWorkletMessage(m: any): void {
    if (m.t === 'frames') this.frames.write(m.records, m.count);
    else if (m.t === 'capture') { this.mic.write(m.mic, m.mic.length); this.out.write(m.out, m.out.length); }
    else if (m.t === 'meter') this.meters.write(m.data);
  }

  // Control ring (main -> audio)
  private send(type: number, index: number, param: number, value: number): void {
    if (this.ring) { this.ring.push(type, index, param, value); return; }
    if (this.queue.length === 0) queueMicrotask(() => this.flush());
    this.queue.push([type, index, param, value]);
  }
  private flush(): void {
    const q = this.queue.slice(0, CONTROL_CAP); // worklet buffer holds CONTROL_CAP per block
    this.queue = this.queue.slice(CONTROL_CAP);
    const buf = new ArrayBuffer(q.length * MSG_BYTES);
    const dv = new DataView(buf);
    q.forEach((m, i) => encodeMessage(dv, i * MSG_BYTES, m[0], m[1], m[2], m[3]));
    this.node.port.postMessage({ buf }, [buf]);
    if (this.queue.length) queueMicrotask(() => this.flush());
  }

  setParam(id: number, value: number): void { this.send(MSG_SET_PARAM, 0, id, value); }
  setToneBand(index: number, db: number): void { this.send(MSG_SET_TONE_BAND, index, 0, db); }
  setNoiseBand(index: number, db: number): void { this.send(MSG_SET_NOISE_BAND, index, 0, db); }
  noteOn(cents: number): void { this.send(MSG_NOTE_ON, 0, 0, cents); }
  noteOff(): void { this.send(MSG_NOTE_OFF, 0, 0, 0); }
  setLive(on: boolean): void { this.send(MSG_SET_LIVE, 0, 0, on ? 1 : 0); }
  resetSinger(): void { this.send(MSG_RESET_SINGER, 0, 0, 0); }
  panic(): void { this.send(MSG_PANIC, 0, 0, 0); }

  /** Connect a mic stream to the engine input. Returns the source node. */
  connectMic(stream: MediaStream): MediaStreamAudioSourceNode {
    const src = this.ctx.createMediaStreamSource(stream);
    src.connect(this.node);
    return src;
  }

  /** Copy the latest meter block into `out` (METER_LEN floats). Returns false
   *  if nothing has been published yet. Seqlock-safe in shared mode. */
  readMeters(out: Float32Array): boolean { return this.meters.read(out); }

  /** Frame-ring records written since `cursor` (start with 0). If the reader
   *  fell more than the ring capacity behind, the oldest are skipped. */
  readFrames(cursor: number): FrameBatch { return this.frames.readSince(cursor); }

  /** Total samples written so far to the mic and output rings (same clock). */
  captureCount(): number { return this.out.total(); }

  /** Copy samples [start, start + length) of the high-passed mic signal and
   *  of the engine output, by absolute sample index (see captureCount). Parts
   *  older than the 60 s ring are zero. */
  readCapture(start: number, length: number): { mic: Float32Array; out: Float32Array } {
    const mic = new Float32Array(length), out = new Float32Array(length);
    this.mic.readRange(start, length, mic);
    this.out.readRange(start, length, out);
    return { mic, out };
  }

  /** Disconnect and close the worklet node (the AudioContext is left open). */
  dispose(): void {
    this.node.disconnect();
    this.node.port.close();
  }
}

interface Bufs { meter: ArrayBufferLike; frames: ArrayBufferLike; mic: ArrayBufferLike; out: ArrayBufferLike; capCap: number }

async function addModuleOnce(ctx: AudioContext): Promise<void> {
  if (moduleAdded.has(ctx)) return;
  await ctx.audioWorklet.addModule(workletUrl);
  moduleAdded.add(ctx);
}

const OFFLINE_BLOCK = 4096;

/** Offline engine on the calling thread (Influence auditions, tests). */
export class OfflineEngine {
  private readonly ex: EngineExports;
  private readonly e: number;
  private constructor(readonly sampleRate: number, readonly nBands: 16 | 24, bytes: BufferSource) {
    this.ex = new WebAssembly.Instance(new WebAssembly.Module(bytes), {}).exports as unknown as EngineExports;
    this.e = this.ex.ew_init(sampleRate, OFFLINE_BLOCK, nBands);
  }
  static async create(sampleRate: number, nBands: 16 | 24): Promise<OfflineEngine> {
    return new OfflineEngine(sampleRate, nBands, await loadEngineBytes());
  }
  /** Same as create, with the wasm bytes supplied (tests, workers without fetch). */
  static fromBytes(bytes: BufferSource, sampleRate: number, nBands: 16 | 24): OfflineEngine {
    return new OfflineEngine(sampleRate, nBands, bytes);
  }

  /** Queue messages straight into the wasm control buffer; render drains them. */
  private send(msgs: [type: number, index: number, param: number, value: number][]): void {
    const t = new Uint32Array(this.ex.memory.buffer, this.ex.ew_buffers(this.e), BT_FIELDS);
    const dv = new DataView(this.ex.memory.buffer, t[BT_CONTROL], CONTROL_CAP * MSG_BYTES);
    const n = Math.min(msgs.length, CONTROL_CAP);
    for (let i = 0; i < n; i++) encodeMessage(dv, i * MSG_BYTES, ...msgs[i]);
    new Uint32Array(this.ex.memory.buffer, t[BT_CONTROL_COUNT], 1)[0] = n;
  }

  /** Render one note with the given parameter set. Applies every scalar and
   *  band, sends NOTE_ON, renders `holdS`, NOTE_OFF, renders `tailS`. */
  renderNote(set: { scalars: ArrayLike<number>; tone: ArrayLike<number>; noise: ArrayLike<number> },
             cents: number, holdS: number, tailS: number): Float32Array {
    const msgs: [number, number, number, number][] = [];
    for (let i = 0; i < set.scalars.length; i++) msgs.push([MSG_SET_PARAM, 0, i, set.scalars[i]]);
    for (let i = 0; i < this.nBands; i++) {
      if (i < set.tone.length) msgs.push([MSG_SET_TONE_BAND, i, 0, set.tone[i]]);
      if (i < set.noise.length) msgs.push([MSG_SET_NOISE_BAND, i, 0, set.noise[i]]);
    }
    msgs.push([MSG_NOTE_ON, 0, 0, cents]);
    this.send(msgs);
    const hold = Math.round(holdS * this.sampleRate);
    const out = new Float32Array(hold + Math.round(tailS * this.sampleRate));
    this.renderInto(out, 0, hold);
    this.send([[MSG_NOTE_OFF, 0, 0, 0]]);
    this.renderInto(out, hold, out.length);
    return out;
  }

  private renderInto(out: Float32Array, from: number, to: number): void {
    for (let pos = from; pos < to; pos += OFFLINE_BLOCK) {
      const n = Math.min(OFFLINE_BLOCK, to - pos);
      this.ex.ew_render(this.e, n);
      // memory may have grown: recreate the view each chunk
      const t = new Uint32Array(this.ex.memory.buffer, this.ex.ew_buffers(this.e), BT_FIELDS);
      out.set(new Float32Array(this.ex.memory.buffer, t[BT_OUTPUT], n), pos);
    }
  }

  dispose(): void { this.ex.ew_free(this.e); }
}
