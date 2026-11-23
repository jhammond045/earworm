// Main-thread side of the M1 engine: starts the klont AudioWorklet and talks
// to it. Shared mode uses a SharedArrayBuffer control ring and meter block
// (page is crossOriginIsolated); otherwise the same data goes over the
// worklet's MessagePort (meters 30 times a second). ?fallback=1 forces it.

import workletUrl from './klont-processor.ts?worker&url';
import type { KlontProcessorOptions } from './klont-processor';
import { CONTROL_CAP, LOAD_BAD_KIND, LOAD_NOTE_CURVES, METER_LEN, MSG_BYTES, MSG_JUMP, MSG_NOTE_OFF, MSG_NOTE_ON, MSG_PANIC, MSG_PLAY,
  MSG_ARM, MSG_COUNT_IN, MSG_RESET_SINGER, MSG_SELECT_LAYER, MSG_SET_BUDGET, MSG_SET_LAYER, MSG_SET_LIVE_KIT, MSG_SET_MACRO,
  MSG_SET_METRO, MSG_SET_PARAM, MSG_SET_TUNE, MSG_STOP, METRO_BAR, METRO_LAYER, METRO_LEVEL, METRO_ON, METRO_SLOT,
  METRO_VOICE, SET_BASE, encodeMessage } from './protocol';
import { ControlRing, MeterBlock, RecordRing } from '../host/rings';
import { micConstraints } from '../host/engine-host';
import { MIC_RING_SEC } from '../m2/take';

export { micConstraints };

// Same values as in klont-processor.ts (which cannot be imported outside the worklet).
const FRAME_STRIDE = 25; // 24-float feature frame plus the tuned pitch
const MIC_SECONDS = MIC_RING_SEC;
const FRAME_RING = MIC_RING_SEC * 100;
/** The output ring is only read over the last few seconds (the follow delay checks). */
const OUT_SECONDS = 60;

export type TransportMode = 'shared' | 'message';

export interface KlontHostOptions {
  /** Force the MessagePort fallback even when cross-origin isolated. */
  forceFallback?: boolean;
  /** Give the node one mono input (Live follow). */
  input?: boolean;
}

const WASM_URL = `${import.meta.env.BASE_URL}wasm/klont.wasm`;
let bytesPromise: Promise<ArrayBuffer> | null = null;

/** klont.wasm bytes, fetched once and cached. */
export function loadKlontBytes(): Promise<ArrayBuffer> {
  bytesPromise ??= fetch(WASM_URL).then((r) => {
    if (!r.ok) throw new Error(`klont.wasm: HTTP ${r.status}`);
    return r.arrayBuffer();
  }).catch((e) => { bytesPromise = null; throw e; });
  return bytesPromise;
}

/** True when the page asked for the fallback (?fallback=1) or is not isolated. */
export function wantsFallback(): boolean {
  return new URLSearchParams(globalThis.location?.search ?? '').get('fallback') === '1' || !globalThis.crossOriginIsolated;
}

interface LiveBufs { cap: number; outCap: number; frames: ArrayBufferLike; mic: ArrayBufferLike; out: ArrayBufferLike }

const moduleAdded = new WeakSet<BaseAudioContext>();

export class KlontHost {
  readonly mode: TransportMode;
  readonly sampleRate: number;
  /** Whether the worklet scope has performance.now() (else CPU load is a coarse Date.now() window average). */
  perfNow = false;
  private readonly ring?: ControlRing;
  private queue: number[][] = [];
  private readonly meters: MeterBlock;
  private nextId = 1;
  private readonly waiting = new Map<number, (status: number) => void>();
  private readonly frames?: RecordRing;
  private readonly mic?: RecordRing;
  private readonly out?: RecordRing;
  private readonly L: number;

  private constructor(readonly ctx: AudioContext, readonly node: AudioWorkletNode, mode: TransportMode,
                      meter: ArrayBufferLike, ring?: ControlRing, live?: LiveBufs) {
    this.mode = mode;
    this.sampleRate = ctx.sampleRate;
    this.ring = ring;
    this.meters = new MeterBlock(meter, METER_LEN);
    this.L = Math.round(ctx.sampleRate / 100);
    if (live) {
      this.frames = new RecordRing(live.frames, FRAME_RING, FRAME_STRIDE);
      this.mic = new RecordRing(live.mic, live.cap, 1);
      this.out = new RecordRing(live.out, live.outCap, 1);
    }
  }

  /** Adds the worklet module, instantiates the engine in it, connects it to the destination. */
  static async create(ctx: AudioContext, opts: KlontHostOptions = {}): Promise<KlontHost> {
    const [bytes] = await Promise.all([loadKlontBytes(), addModuleOnce(ctx)]);
    const shared = !!globalThis.crossOriginIsolated && !opts.forceFallback && typeof SharedArrayBuffer !== 'undefined';
    const mk = (n: number): ArrayBufferLike => (shared ? new SharedArrayBuffer(n) : new ArrayBuffer(n));
    const meter = mk(MeterBlock.bytes(METER_LEN));
    const control = shared ? new SharedArrayBuffer(ControlRing.BYTES) : undefined;
    const po: KlontProcessorOptions = { bytes, shared, input: !!opts.input };
    if (shared) Object.assign(po, { control, meter });
    let live: LiveBufs | undefined;
    if (opts.input) {
      const cap = Math.ceil(MIC_SECONDS * ctx.sampleRate);
      const outCap = Math.ceil(OUT_SECONDS * ctx.sampleRate);
      live = { cap, outCap, frames: mk(RecordRing.bytes(FRAME_RING, FRAME_STRIDE)), mic: mk(RecordRing.bytes(cap, 1)), out: mk(RecordRing.bytes(outCap, 1)) };
      if (shared) Object.assign(po, { frames: live.frames, mic: live.mic, out: live.out });
    }
    const node = new AudioWorkletNode(ctx, 'earworm-klont', {
      numberOfInputs: opts.input ? 1 : 0, numberOfOutputs: 1, outputChannelCount: [2], processorOptions: po,
      ...(opts.input ? { channelCount: 1, channelCountMode: 'explicit' as const } : {}),
    });
    const host = new KlontHost(ctx, node, shared ? 'shared' : 'message', meter, control && new ControlRing(control), live);
    await new Promise<void>((resolve, reject) => {
      node.onprocessorerror = () => reject(new Error('klont processor failed to start'));
      node.port.onmessage = (ev) => {
        if (ev.data.t === 'ready') { host.perfNow = !!ev.data.perf; resolve(); }
        else host.onWorkletMessage(ev.data);
      };
    });
    node.port.onmessage = (ev) => host.onWorkletMessage(ev.data);
    node.connect(ctx.destination);
    return host;
  }

  private onWorkletMessage(m: any): void {
    if (m.t === 'meter') this.meters.write(m.data);
    else if (m.t === 'live') {
      this.mic?.write(m.mic, m.mic.length);
      this.out?.write(m.out, m.out.length);
      this.frames?.write(m.frames, m.nFrames);
    } else if (m.t === 'loaded') {
      this.waiting.get(m.id)?.(m.status);
      this.waiting.delete(m.id);
    }
  }

  /** One control message. `time` is a sample offset inside the next block. */
  send(type: number, layer: number, index: number, param: number, value: number, time = 0): void {
    if (this.ring) {
      if (!this.ring.pushWith((v, o) => encodeMessage(v, o, type, layer, index, param, value, time))) {
        console.warn('klont: control ring full, message dropped');
      }
      return;
    }
    if (this.queue.length === 0) queueMicrotask(() => this.flush());
    this.queue.push([type, layer, index, param, value, time]);
  }
  private flush(): void {
    const q = this.queue.slice(0, CONTROL_CAP);
    this.queue = this.queue.slice(CONTROL_CAP);
    const buf = new ArrayBuffer(q.length * MSG_BYTES);
    const dv = new DataView(buf);
    q.forEach((m, i) => encodeMessage(dv, i * MSG_BYTES, m[0], m[1], m[2], m[3], m[4], m[5]));
    this.node.port.postMessage({ t: 'ctl', buf }, [buf]);
    if (this.queue.length) queueMicrotask(() => this.flush());
  }

  /** Normalized value into a stored set (SET_BASE, SET_KIT + k, SET_INFLUENCE). */
  setParam(layer: number, param: number, norm: number, set = SET_BASE): void { this.send(MSG_SET_PARAM, layer, set, param, norm); }
  setMacro(layer: number, macro: number, v: number): void { this.send(MSG_SET_MACRO, layer, macro, 0, v); }
  setLayer(layer: number, field: number, value: number): void { this.send(MSG_SET_LAYER, layer, 0, field, value); }
  noteOn(layer: number, cents: number, kitSlot = 0, velocity = 0): void {
    this.send(MSG_NOTE_ON, layer, kitSlot, Math.round(velocity * 1000), cents);
  }
  noteOff(layer: number, cents: number): void { this.send(MSG_NOTE_OFF, layer, 0, 0, cents); }
  play(): void { this.send(MSG_PLAY, 0, 0, 0, 0); }
  stop(): void { this.send(MSG_STOP, 0, 0, 0, 0); }
  jump(section: number): void { this.send(MSG_JUMP, 0, section, 0, 0); }
  panic(): void { this.send(MSG_PANIC, 0, 0, 0, 0); }
  /** Partial budget for all layers; 0 = automatic (adapts to the CPU load). */
  setBudget(n: number): void { this.send(MSG_SET_BUDGET, 0, 0, 0, n); }
  selectLayer(layer: number): void { this.send(MSG_SELECT_LAYER, layer, 0, 0, 0); }

  /** Copy a blob into the engine (kind: LOAD_TRACK .. LOAD_NOTE_CURVES). The buffer is copied, not transferred. */
  load(kind: number, slot: number, blob: ArrayBuffer): Promise<number> {
    if (kind < 1 || kind > LOAD_NOTE_CURVES) return Promise.resolve(LOAD_BAD_KIND);
    const id = this.nextId++;
    const copy = blob.slice(0);
    return new Promise((resolve) => {
      this.waiting.set(id, resolve);
      this.node.port.postMessage({ t: 'load', id, slot, kind, buf: copy }, [copy]);
    });
  }

  /** Move the transport to `ticks` (grid) or seconds (free) inside `section`. */
  seek(section: number, ticks: number): void { this.node.port.postMessage({ t: 'seek', section, ticks }); }

  // ---- Live follow (M2). Needs `input: true` at create. ----

  /** Connect a mic stream to the engine's input. Returns the source node. */
  connectMic(stream: MediaStream): MediaStreamAudioSourceNode {
    const src = this.ctx.createMediaStreamSource(stream);
    src.connect(this.node);
    return src;
  }
  /** Arm a layer: the mic drives it. Arming one layer disarms the other. */
  arm(layer: number, on: boolean): void { this.send(MSG_ARM, layer, 0, 0, on ? 1 : 0); }
  /** TUNE_* field of the layer's live tuner (the armed layer's settings are used). */
  setTune(layer: number, field: number, value: number): void { this.send(MSG_SET_TUNE, layer, 0, field, value); }
  /** LIVE_KIT_* field of a kit slot for live slot choice on beat layers. */
  setLiveKit(layer: number, slot: number, field: number, value: number): void {
    this.send(MSG_SET_LIVE_KIT, layer, slot, field, value);
  }
  /** Forget the learned singer average, openness range and noise floor. */
  resetSinger(): void { this.send(MSG_RESET_SINGER, 0, 0, 0, 0); }

  /** Live metronome. `voice` `kit` uses `layer`'s kit `slot`; `tick` is a plain click.
   *  The click is mixed in the worklet only, so exports and the mic buffer stay free of it. */
  setMetro(on: boolean, level: number, voice: 'tick' | 'kit', layer: number, slot: number, bar: number): void {
    this.send(MSG_SET_METRO, 0, 0, METRO_ON, on ? 1 : 0);
    this.send(MSG_SET_METRO, 0, 0, METRO_LEVEL, level);
    this.send(MSG_SET_METRO, 0, 0, METRO_VOICE, voice === 'kit' ? 1 : 0);
    this.send(MSG_SET_METRO, 0, 0, METRO_LAYER, layer);
    this.send(MSG_SET_METRO, 0, 0, METRO_SLOT, slot);
    this.send(MSG_SET_METRO, 0, 0, METRO_BAR, bar);
  }

  /** One-bar count-in at `bpm`, then the engine starts the transport. Ignored while already playing. */
  countIn(bpm: number): void { this.send(MSG_COUNT_IN, 0, 0, 0, bpm); }

  /** Frames written to the frame ring since the engine started (the global frame count). */
  framesTotal(): number { return this.frames?.total() ?? 0; }
  /** Records [start, start + count) by global frame index, 25 floats each. Frames
   *  not in the ring (older than 301 s or not written yet) read as zeros. */
  readFrames(start: number, count: number): Float32Array {
    const out = new Float32Array(Math.max(0, count) * FRAME_STRIDE);
    this.frames?.readRange(start, count, out);
    return out;
  }
  /** Mic samples written to the mic ring since the engine started. */
  samplesTotal(): number { return this.mic?.total() ?? 0; }
  /** High-passed mic samples [start, start + count) by global sample index (zeros outside the 301 s ring). */
  readMic(start: number, count: number): Float32Array {
    const out = new Float32Array(Math.max(0, count));
    this.mic?.readRange(start, count, out);
    return out;
  }
  /** Engine output (left channel) on the same sample clock as the mic. */
  readOut(start: number, count: number): Float32Array {
    const out = new Float32Array(Math.max(0, count));
    this.out?.readRange(start, count, out);
    return out;
  }
  /** Samples per feature frame: sampleRate / 100. Global frame n covers mic samples [n L, (n + 1) L). */
  frameLen(): number { return this.L; }

  /** Copy the latest meter block into `out` (METER_LEN floats). False if nothing was published yet. */
  readMeters(out: Float32Array): boolean { return this.meters.read(out); }

  /** Disconnect the node (the AudioContext is left open). */
  disconnect(): void {
    this.node.disconnect();
    this.node.port.close();
  }
}

async function addModuleOnce(ctx: AudioContext): Promise<void> {
  if (moduleAdded.has(ctx)) return;
  await ctx.audioWorklet.addModule(workletUrl);
  moduleAdded.add(ctx);
}
