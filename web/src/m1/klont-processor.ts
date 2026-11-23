// AudioWorkletProcessor that hosts klont.wasm (the M1 engine). Same shape as
// engine-processor.ts: the module is instantiated synchronously from bytes in
// processorOptions; each block drains the control ring into wasm, reports the
// CPU load, runs ew_process(128) and publishes the meter block.
//
// CPU load: the engine adapts its partial budget to `host_load`, the last
// block's ew_process time over the quantum (128 / sampleRate). Chrome has no
// performance.now() in AudioWorkletGlobalScope (checked on Chrome 14x; the
// `perf` flag in the 'ready' message says what this browser has). Without it
// the fallback times a window of WINDOW blocks with Date.now() (1 ms steps):
// each call adds (after - before) to a sum, which is right on average because
// the call start is spread over the millisecond grid, and the window average is
// reported once, on the first block after the window; other blocks report 0,
// which the engine treats as "no adaptation". Limits: the load figure is the
// mean of WINDOW blocks, so a single slow block is not seen, and the budget
// reacts every WINDOW blocks, not every block.

import { CONTROL_CAP, LOAD_TOO_BIG, METER_LEN, M_LOAD, MSG_BYTES, readBufferTable, type BufferTable,
  type EngineExports } from './protocol';
import { ControlRing, MeterBlock, RecordRing } from '../host/rings';

declare class AudioWorkletProcessor {
  readonly port: MessagePort;
  constructor(options?: unknown);
}
declare const sampleRate: number;
declare function registerProcessor(name: string, ctor: new (options: any) => AudioWorkletProcessor): void;

const MAX_BLOCK = 128;
const METER_S = 1 / 30;
const WINDOW = 16;
const FLUSH_S = 0.1; // message mode: live chunk period
// Same values as in klont-host.ts.
/** Frame record: 24-float feature frame plus the tuned pitch (CURVE_STRIDE in the engine). */
const FRAME_STRIDE = 25;
const MIC_SECONDS = 301; // MIC_RING_SEC in m2/take.ts: a 5-minute poly take and a second
const FRAME_RING = MIC_SECONDS * 100;
const OUT_SECONDS = 60;
/** Message mode ships the rings every 100 ms, so the worklet's own copies need only a few seconds. */
const LOCAL_SECONDS = 4;

export interface KlontProcessorOptions {
  bytes: ArrayBuffer;
  shared: boolean;
  /** Shared mode only. */
  control?: SharedArrayBuffer;
  meter?: SharedArrayBuffer;
  /** Mono mic input: Live follow. Without it the M1 player path runs unchanged. */
  input?: boolean;
  /** Shared mode with `input`: the mic, output and frame rings. */
  mic?: SharedArrayBuffer;
  out?: SharedArrayBuffer;
  frames?: SharedArrayBuffer;
}

const perf: { now(): number } | null = typeof performance !== 'undefined' && typeof performance.now === 'function'
  ? performance : null;

class KlontProcessor extends AudioWorkletProcessor {
  private readonly ex: EngineExports;
  private readonly e: number;
  private readonly ring?: ControlRing;
  private readonly meters?: MeterBlock;
  private pending = new Uint8Array(CONTROL_CAP * MSG_BYTES);
  private pendingCount = 0;
  private samples = 0;
  private lastMeter = 0;
  private readonly quantumMs = (MAX_BLOCK / sampleRate) * 1000;
  // Fallback timing window
  private winSum = 0;
  private winBlocks = 0;
  private winLoad = 0;
  // Views into wasm memory; rebuilt when memory.buffer changes
  private buf: ArrayBuffer | null = null;
  private t!: BufferTable;
  private outL!: Float32Array;
  private outR!: Float32Array;
  private control!: Uint8Array;
  private wasmMeter!: Float32Array;
  private ctlCount!: Uint32Array;
  private hostLoad!: Float32Array;
  private input!: Float32Array;
  private wasmFrames!: Float32Array;
  private frCount!: Uint32Array;
  // Live follow rings (input mode only)
  private readonly live: boolean;
  private mic?: RecordRing;
  private outRing?: RecordRing;
  private frames?: RecordRing;
  private capCursor = 0;
  private frameCursor = 0;
  private lastFlush = 0;

  constructor(options: { processorOptions: KlontProcessorOptions }) {
    super();
    const o = options.processorOptions;
    this.ex = new WebAssembly.Instance(new WebAssembly.Module(o.bytes), {}).exports as unknown as EngineExports;
    this.e = this.ex.ew_init(sampleRate, MAX_BLOCK);
    if (o.shared) {
      this.ring = new ControlRing(o.control!);
      this.meters = new MeterBlock(o.meter!, METER_LEN);
    }
    this.live = !!o.input;
    if (this.live) {
      const shared = !!o.mic;
      const cap = Math.ceil((shared ? MIC_SECONDS : LOCAL_SECONDS) * sampleRate);
      const outCap = Math.ceil((shared ? OUT_SECONDS : LOCAL_SECONDS) * sampleRate);
      const frameCap = shared ? FRAME_RING : LOCAL_SECONDS * 100;
      this.mic = new RecordRing(o.mic ?? new ArrayBuffer(RecordRing.bytes(cap, 1)), cap, 1);
      this.outRing = new RecordRing(o.out ?? new ArrayBuffer(RecordRing.bytes(outCap, 1)), outCap, 1);
      this.frames = new RecordRing(o.frames ?? new ArrayBuffer(RecordRing.bytes(frameCap, FRAME_STRIDE)), frameCap, FRAME_STRIDE);
    }
    this.port.onmessage = (ev) => this.onMessage(ev.data);
    this.refreshViews();
    this.port.postMessage({ t: 'ready', perf: perf !== null, sampleRate });
  }

  private refreshViews(): void {
    const mem = this.ex.memory.buffer as ArrayBuffer;
    if (mem === this.buf) return;
    this.buf = mem;
    const t = this.t = readBufferTable(mem, this.ex.ew_buffers(this.e));
    this.input = new Float32Array(mem, t.input, MAX_BLOCK);
    this.wasmFrames = new Float32Array(mem, t.frames, t.framesCap * FRAME_STRIDE);
    this.frCount = new Uint32Array(mem, t.framesCount, 1);
    this.outL = new Float32Array(mem, t.outputL, MAX_BLOCK);
    this.outR = new Float32Array(mem, t.outputR, MAX_BLOCK);
    this.control = new Uint8Array(mem, t.control, CONTROL_CAP * MSG_BYTES);
    this.wasmMeter = new Float32Array(mem, t.meter, METER_LEN);
    this.ctlCount = new Uint32Array(mem, t.controlCount, 1);
    this.hostLoad = new Float32Array(mem, t.hostLoad, 1);
  }

  private onMessage(m: any): void {
    if (m.t === 'ctl') {
      const b = new Uint8Array(m.buf as ArrayBuffer);
      const n = Math.min(b.length, this.pending.length - this.pendingCount * MSG_BYTES);
      this.pending.set(b.subarray(0, n), this.pendingCount * MSG_BYTES);
      this.pendingCount += n / MSG_BYTES;
    } else if (m.t === 'load') {
      this.refreshViews();
      const b = new Uint8Array(m.buf as ArrayBuffer);
      let status = LOAD_TOO_BIG;
      if (b.length <= this.t.stagingCap) {
        new Uint8Array(this.ex.memory.buffer, this.t.staging, b.length).set(b);
        status = this.ex.ew_load(this.e, m.slot, m.kind, this.t.staging, b.length);
        this.refreshViews(); // the load may have grown memory
      }
      this.port.postMessage({ t: 'loaded', id: m.id, status });
    } else if (m.t === 'seek') {
      this.ex.ew_seek(this.e, m.section, m.ticks);
    }
  }

  private drainControl(): number {
    if (this.ring) return this.ring.drain(this.control, CONTROL_CAP);
    const n = this.pendingCount;
    this.control.set(this.pending.subarray(0, n * MSG_BYTES));
    this.pendingCount = 0;
    return n;
  }

  process(inputs: Float32Array[][], outputs: Float32Array[][]): boolean {
    const out = outputs[0];
    if (!out || out.length === 0) return true;
    const inp = this.live ? inputs[0]?.[0] : undefined;
    const total = out[0].length;
    for (let o = 0; o < total; o += MAX_BLOCK) {
      const m = Math.min(MAX_BLOCK, total - o);
      this.refreshViews();
      if (this.live) {
        if (inp) this.input.set(inp.subarray(o, o + m)); else this.input.fill(0, 0, m);
      }
      this.block(m);
      out[0].set(this.outL.subarray(0, m), o);
      (out[1] ?? out[0]).set(this.outR.subarray(0, m), o);
    }
    return true;
  }

  private block(m: number): void {
    this.ctlCount[0] = this.drainControl();
    if (perf) {
      const t0 = perf.now();
      this.hostLoad[0] = this.lastLoad;
      this.ex.ew_process(this.e, m);
      this.lastLoad = (perf.now() - t0) / this.quantumMs;
    } else {
      // Report the finished window once; 0 on the other blocks means "no adaptation".
      this.hostLoad[0] = this.winLoad;
      this.winLoad = 0;
      const t0 = Date.now();
      this.ex.ew_process(this.e, m);
      this.winSum += Date.now() - t0;
      if (++this.winBlocks >= WINDOW) {
        this.winLoad = this.shownLoad = this.winSum / (WINDOW * this.quantumMs);
        this.winSum = 0;
        this.winBlocks = 0;
      }
    }
    // The engine copies host_load into the meter, which is 0 on most blocks in the
    // window fallback; show the last real figure instead.
    this.wasmMeter[M_LOAD] = perf ? this.lastLoad : this.shownLoad;
    this.samples += m;
    if (this.live) this.publishLive(m);
    if (this.meters) this.meters.write(this.wasmMeter);
    else if (this.samples - this.lastMeter >= METER_S * sampleRate) {
      this.lastMeter = this.samples;
      const copy = this.wasmMeter.slice();
      this.port.postMessage({ t: 'meter', data: copy }, [copy.buffer]);
    }
  }

  /** ew_process left the high-passed mic in `input`; log it, the output and the frames. */
  private publishLive(m: number): void {
    this.mic!.write(this.input, m);
    this.outRing!.write(this.outL, m);
    const nf = this.frCount[0];
    if (nf > 0) this.frames!.write(this.wasmFrames, nf);
    if (this.meters) return;
    // Message mode: ship what is new every 100 ms, frames and samples together.
    if (this.samples - this.lastFlush >= FLUSH_S * sampleRate) {
      this.lastFlush = this.samples;
      const n = this.outRing!.total() - this.capCursor;
      const mic = new Float32Array(n), out = new Float32Array(n);
      this.mic!.readRange(this.capCursor, n, mic);
      this.outRing!.readRange(this.capCursor, n, out);
      this.capCursor += n;
      const f = this.frames!.readSince(this.frameCursor);
      this.frameCursor = f.cursor;
      this.port.postMessage({ t: 'live', mic, out, frames: f.records, nFrames: f.count },
        [mic.buffer, out.buffer, f.records.buffer]);
    }
  }

  private lastLoad = 0;
  private shownLoad = 0;
}

registerProcessor('earworm-klont', KlontProcessor);
