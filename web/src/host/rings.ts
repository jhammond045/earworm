// Ring and seqlock helpers shared by the main thread and the worklet. Every
// class works on a SharedArrayBuffer or a plain ArrayBuffer (the message-mode
// mirrors), and uses Atomics for the counters either way.
//
// Counters are free-running Int32 values. The frame and capture rings would
// wrap after 2^31 records (about 12 hours of 48 kHz audio); fine for M0.

import { CONTROL_CAP, MSG_BYTES, METER_LEN } from './protocol';

const HDR = 16; // bytes before the data: keeps the data 16-byte aligned

/** Write one control message (protocol.rs layout, little endian). */
export function encodeMessage(view: DataView, offset: number, type: number, index: number, param: number, value: number): void {
  view.setUint16(offset, type, true);
  view.setUint8(offset + 2, 0); // layer
  view.setUint8(offset + 3, index);
  view.setUint16(offset + 4, param, true);
  view.setUint16(offset + 6, 0, true);
  view.setFloat32(offset + 8, value, true);
  view.setUint32(offset + 12, 0, true); // time
}

/** Single-producer single-consumer ring of 16-byte messages. */
export class ControlRing {
  static readonly BYTES = HDR + CONTROL_CAP * MSG_BYTES;
  private readonly idx: Int32Array; // [write, read]
  private readonly bytes: Uint8Array;
  private readonly view: DataView;
  constructor(buf: ArrayBufferLike) {
    this.idx = new Int32Array(buf, 0, 2);
    this.bytes = new Uint8Array(buf, HDR);
    this.view = new DataView(buf, HDR);
  }
  /** Producer. False when the ring is full (message dropped). */
  push(type: number, index: number, param: number, value: number): boolean {
    const w = Atomics.load(this.idx, 0);
    if (((w - Atomics.load(this.idx, 1)) | 0) >= CONTROL_CAP) return false;
    encodeMessage(this.view, (w & (CONTROL_CAP - 1)) * MSG_BYTES, type, index, param, value);
    Atomics.store(this.idx, 0, (w + 1) | 0);
    return true;
  }
  /** Consumer. Copies up to `max` messages into `dst` (wasm control buffer), returns the count. */
  drain(dst: Uint8Array, max: number): number {
    const r = Atomics.load(this.idx, 1);
    const n = Math.min(((Atomics.load(this.idx, 0) - r) | 0), max);
    for (let i = 0; i < n; i++) {
      const o = ((r + i) & (CONTROL_CAP - 1)) * MSG_BYTES;
      dst.set(this.bytes.subarray(o, o + MSG_BYTES), i * MSG_BYTES);
    }
    if (n > 0) Atomics.store(this.idx, 1, (r + n) | 0);
    return n;
  }
}

/** Meter block guarded by a version counter: odd while the writer is inside. */
export class MeterBlock {
  static readonly BYTES = HDR + METER_LEN * 4;
  private readonly ver: Int32Array;
  private readonly data: Float32Array;
  constructor(buf: ArrayBufferLike) {
    this.ver = new Int32Array(buf, 0, 1);
    this.data = new Float32Array(buf, HDR, METER_LEN);
  }
  write(src: Float32Array): void {
    const v = Atomics.load(this.ver, 0);
    Atomics.store(this.ver, 0, (v + 1) | 0);
    this.data.set(src.subarray(0, METER_LEN));
    Atomics.store(this.ver, 0, (v + 2) | 0);
  }
  /** False if nothing was ever published, or every try hit a write. `mid` is a test hook. */
  read(out: Float32Array, tries = 8, mid?: () => void): boolean {
    for (let t = 0; t < tries; t++) {
      const v1 = Atomics.load(this.ver, 0);
      if (v1 === 0) return false;
      if (v1 & 1) continue;
      out.set(this.data);
      mid?.();
      if (Atomics.load(this.ver, 0) === v1) return true;
    }
    return false;
  }
}

/** Overwriting ring of fixed-size records (frames: stride 25, capture: stride 1). */
export class RecordRing {
  static bytes(cap: number, stride: number): number { return HDR + cap * stride * 4; }
  private readonly head: Int32Array;
  private readonly data: Float32Array;
  constructor(buf: ArrayBufferLike, readonly cap: number, readonly stride: number) {
    this.head = new Int32Array(buf, 0, 1);
    this.data = new Float32Array(buf, HDR, cap * stride);
  }
  /** Total records ever written. */
  total(): number { return Atomics.load(this.head, 0); }
  /** Single writer. Appends the first `n` records of `src`; if n > cap only the last cap survive. */
  write(src: Float32Array, n: number): void {
    const { cap, stride, data } = this;
    const t = Atomics.load(this.head, 0);
    const skip = Math.max(0, n - cap);
    const k = n - skip;
    const pos = (t + skip) % cap;
    const first = Math.min(k, cap - pos);
    data.set(src.subarray(skip * stride, (skip + first) * stride), pos * stride);
    if (k > first) data.set(src.subarray((skip + first) * stride, (skip + k) * stride), 0);
    Atomics.store(this.head, 0, (t + n) | 0);
  }
  /** Copy records [start, start+n) by absolute index into `out`. Records not in
   *  the ring (too old or not written yet) are left as they are in `out`. */
  readRange(start: number, n: number, out: Float32Array): void {
    const { cap, stride, data } = this;
    const t = Atomics.load(this.head, 0);
    const lo = Math.max(start, t - cap, 0);
    const hi = Math.min(start + n, t);
    for (let i = lo; i < hi;) {
      const pos = i % cap;
      const run = Math.min(hi - i, cap - pos);
      out.set(data.subarray(pos * stride, (pos + run) * stride), (i - start) * stride);
      i += run;
    }
    // The writer may have lapped us while we copied; zero what it overwrote.
    const lost = Atomics.load(this.head, 0) - cap;
    for (let i = lo; i < Math.min(lost, hi); i++) out.fill(0, (i - start) * stride, (i - start + 1) * stride);
  }
  /** Records since `cursor`, oldest first; skips ahead if more than cap behind. */
  readSince(cursor: number): { records: Float32Array; count: number; cursor: number } {
    const t = this.total();
    const start = Math.max(cursor, t - this.cap);
    const count = Math.max(0, t - start);
    const records = new Float32Array(count * this.stride);
    this.readRange(start, count, records);
    return { records, count, cursor: t };
  }
}
