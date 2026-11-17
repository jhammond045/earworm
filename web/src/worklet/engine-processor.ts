// AudioWorkletProcessor that hosts engine.wasm. The wasm module is instantiated
// synchronously from bytes passed in processorOptions (no wasm-bindgen). Each
// block: drain control messages into wasm, run ew_process, publish results.
// Shared mode writes straight into SABs; message mode writes into private
// staging rings and posts the new part to the main thread.

import { BT_CONTROL, BT_CONTROL_COUNT, BT_FIELDS, BT_FRAMES, BT_FRAMES_COUNT, BT_INPUT, BT_METER,
  BT_OUTPUT, CONTROL_CAP, METER_LEN, MSG_BYTES, RECORD_STRIDE, type EngineExports } from '../host/protocol';
import { ControlRing, MeterBlock, RecordRing } from '../host/rings';

declare class AudioWorkletProcessor {
  readonly port: MessagePort;
  constructor(options?: unknown);
}
declare const sampleRate: number;
declare function registerProcessor(name: string, ctor: new (options: any) => AudioWorkletProcessor): void;

const MAX_BLOCK = 128;
const FRAME_CAP = 6000;
const CAPTURE_S = 60;
const FLUSH_S = 0.1; // message mode: capture chunk period
const METER_S = 1 / 30;

export interface ProcessorOptions {
  bytes: ArrayBuffer;
  nBands: number;
  shared: boolean;
  control?: SharedArrayBuffer;
  /** Shared mode only; message mode stages in private buffers. */
  meter?: SharedArrayBuffer;
  frames?: SharedArrayBuffer;
  mic?: SharedArrayBuffer;
  out?: SharedArrayBuffer;
}

class EngineProcessor extends AudioWorkletProcessor {
  private readonly ex: EngineExports;
  private readonly e: number;
  private readonly shared: boolean;
  private readonly ring?: ControlRing;
  private readonly meters: MeterBlock;
  private readonly frames: RecordRing;
  private readonly mic: RecordRing;
  private readonly out: RecordRing;
  // Message-mode state
  private pending = new Uint8Array(CONTROL_CAP * MSG_BYTES);
  private pendingCount = 0;
  private frameCursor = 0;
  private capCursor = 0;
  private lastFlush = 0;
  private lastMeter = 0;
  private samples = 0;
  // Views into wasm memory; rebuilt when memory.buffer changes
  private buf: ArrayBuffer | null = null;
  private input!: Float32Array;
  private output!: Float32Array;
  private control!: Uint8Array;
  private wasmMeter!: Float32Array;
  private wasmFrames!: Float32Array;
  private ctlCount!: Uint32Array;
  private frCount!: Uint32Array;

  constructor(options: { processorOptions: ProcessorOptions }) {
    super();
    const o = options.processorOptions;
    const mod = new WebAssembly.Module(o.bytes);
    this.ex = new WebAssembly.Instance(mod, {}).exports as unknown as EngineExports;
    this.e = this.ex.ew_init(sampleRate, MAX_BLOCK, o.nBands);
    this.shared = o.shared;
    const capCap = o.shared ? CAPTURE_S * sampleRate : Math.ceil(sampleRate); // message mode stages 1 s
    this.meters = new MeterBlock(o.meter ?? new ArrayBuffer(MeterBlock.BYTES));
    this.frames = new RecordRing(o.frames ?? new ArrayBuffer(RecordRing.bytes(FRAME_CAP, RECORD_STRIDE)), FRAME_CAP, RECORD_STRIDE);
    this.mic = new RecordRing(o.mic ?? new ArrayBuffer(RecordRing.bytes(capCap, 1)), capCap, 1);
    this.out = new RecordRing(o.out ?? new ArrayBuffer(RecordRing.bytes(capCap, 1)), capCap, 1);
    if (o.shared) this.ring = new ControlRing(o.control!);
    else this.port.onmessage = (ev) => {
      const b = new Uint8Array(ev.data.buf as ArrayBuffer);
      const n = Math.min(b.length, this.pending.length - this.pendingCount * MSG_BYTES);
      this.pending.set(b.subarray(0, n), this.pendingCount * MSG_BYTES);
      this.pendingCount += n / MSG_BYTES;
    };
    this.refreshViews();
    this.port.postMessage({ t: 'ready' });
  }

  private refreshViews(): void {
    const mem = this.ex.memory.buffer as ArrayBuffer;
    if (mem === this.buf) return;
    this.buf = mem;
    const t = new Uint32Array(mem, this.ex.ew_buffers(this.e), BT_FIELDS);
    this.input = new Float32Array(mem, t[BT_INPUT], MAX_BLOCK);
    this.output = new Float32Array(mem, t[BT_OUTPUT], MAX_BLOCK);
    this.control = new Uint8Array(mem, t[BT_CONTROL], CONTROL_CAP * MSG_BYTES);
    this.wasmMeter = new Float32Array(mem, t[BT_METER], METER_LEN);
    this.wasmFrames = new Float32Array(mem, t[BT_FRAMES]);
    this.ctlCount = new Uint32Array(mem, t[BT_CONTROL_COUNT], 1);
    this.frCount = new Uint32Array(mem, t[BT_FRAMES_COUNT], 1);
  }

  private drainControl(): number {
    if (this.ring) return this.ring.drain(this.control, CONTROL_CAP);
    const n = this.pendingCount;
    this.control.set(this.pending.subarray(0, n * MSG_BYTES));
    this.pendingCount = 0;
    return n;
  }

  process(inputs: Float32Array[][], outputs: Float32Array[][]): boolean {
    const outCh = outputs[0];
    if (!outCh || outCh.length === 0) return true;
    const inp = inputs[0]?.[0];
    const total = outCh[0].length;
    for (let o = 0; o < total; o += MAX_BLOCK) {
      const m = Math.min(MAX_BLOCK, total - o);
      this.refreshViews();
      this.block(inp ? inp.subarray(o, o + m) : null, m, o === 0);
      for (const ch of outCh) ch.set(this.output.subarray(0, m), o);
    }
    return true;
  }

  private block(inp: Float32Array | null, m: number, first: boolean): void {
    this.ctlCount[0] = first ? this.drainControl() : 0;
    if (inp) this.input.set(inp); else this.input.fill(0, 0, m);
    this.ex.ew_process(this.e, m);
    // ew_process leaves the high-passed mic in `input`
    this.mic.write(this.input, m);
    this.out.write(this.output, m);
    const nf = this.frCount[0];
    if (nf > 0) this.frames.write(this.wasmFrames, nf);
    this.samples += m;
    if (this.shared) this.meters.write(this.wasmMeter);
    else this.postMessageMode();
  }

  private postMessageMode(): void {
    const sr = sampleRate;
    if (this.frames.total() !== this.frameCursor) {
      const b = this.frames.readSince(this.frameCursor);
      this.frameCursor = b.cursor;
      this.port.postMessage({ t: 'frames', records: b.records, count: b.count }, [b.records.buffer]);
    }
    if (this.samples - this.lastFlush >= FLUSH_S * sr) {
      this.lastFlush = this.samples;
      const n = this.out.total() - this.capCursor;
      const mic = new Float32Array(n), out = new Float32Array(n);
      this.mic.readRange(this.capCursor, n, mic);
      this.out.readRange(this.capCursor, n, out);
      this.capCursor += n;
      this.port.postMessage({ t: 'capture', mic, out }, [mic.buffer, out.buffer]);
    }
    if (this.samples - this.lastMeter >= METER_S * sr) {
      this.lastMeter = this.samples;
      const copy = this.wasmMeter.slice();
      this.port.postMessage({ t: 'meter', data: copy }, [copy.buffer]);
    }
  }
}

registerProcessor('earworm-engine', EngineProcessor);
