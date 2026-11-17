import { describe, expect, it } from 'vitest';
import { CONTROL_CAP, METER_LEN, MSG_BYTES } from './protocol';
import { ControlRing, MeterBlock, RecordRing, encodeMessage } from './rings';

describe('encodeMessage', () => {
  it('matches the protocol byte layout', () => {
    const dv = new DataView(new ArrayBuffer(32));
    encodeMessage(dv, 16, 0x0304, 7, 0x0102, 1.5);
    expect(dv.getUint16(16, true)).toBe(0x0304);
    expect(dv.getUint8(18)).toBe(0); // layer
    expect(dv.getUint8(19)).toBe(7);
    expect(dv.getUint16(20, true)).toBe(0x0102);
    expect(dv.getUint16(22, true)).toBe(0);
    expect(dv.getFloat32(24, true)).toBe(1.5);
    expect(dv.getUint32(28, true)).toBe(0);
    expect(dv.getUint8(16)).toBe(0x04); // little endian
  });
});

describe('ControlRing', () => {
  const make = () => new ControlRing(new SharedArrayBuffer(ControlRing.BYTES));
  it('delivers messages in order across wrap-around', () => {
    const r = make();
    const dst = new Uint8Array(CONTROL_CAP * MSG_BYTES);
    const dv = new DataView(dst.buffer);
    let next = 0, seen = 0;
    for (let round = 0; round < 5; round++) {
      for (let i = 0; i < 200; i++) expect(r.push(1, i & 255, 0, next++)).toBe(true);
      const n = r.drain(dst, CONTROL_CAP);
      expect(n).toBe(200);
      for (let i = 0; i < n; i++) expect(dv.getFloat32(i * MSG_BYTES + 8, true)).toBe(seen++);
    }
  });
  it('rejects pushes when full and recovers after a drain', () => {
    const r = make();
    for (let i = 0; i < CONTROL_CAP; i++) expect(r.push(1, 0, 0, i)).toBe(true);
    expect(r.push(1, 0, 0, 999)).toBe(false);
    const dst = new Uint8Array(CONTROL_CAP * MSG_BYTES);
    expect(r.drain(dst, 10)).toBe(10);
    for (let i = 0; i < 10; i++) expect(r.push(1, 0, 0, 1000 + i)).toBe(true);
    expect(r.push(1, 0, 0, 2000)).toBe(false);
    expect(r.drain(dst, CONTROL_CAP)).toBe(CONTROL_CAP);
  });
});

describe('MeterBlock', () => {
  it('returns false before the first write', () => {
    const m = new MeterBlock(new SharedArrayBuffer(MeterBlock.BYTES));
    expect(m.read(new Float32Array(METER_LEN))).toBe(false);
  });
  it('never returns a torn read when a write lands mid-copy', () => {
    const m = new MeterBlock(new SharedArrayBuffer(MeterBlock.BYTES));
    const gen = (v: number) => new Float32Array(METER_LEN).fill(v);
    m.write(gen(1));
    const out = new Float32Array(METER_LEN);
    let g = 1;
    // A write happens during the first two copies; the third try is clean.
    let hits = 0;
    const ok = m.read(out, 8, () => { if (hits++ < 2) m.write(gen(++g)); });
    expect(ok).toBe(true);
    expect(new Set(out).size).toBe(1);
    expect(out[0]).toBe(g);
  });
  it('gives up (false) if every try is interrupted', () => {
    const m = new MeterBlock(new SharedArrayBuffer(MeterBlock.BYTES));
    m.write(new Float32Array(METER_LEN));
    let g = 0;
    expect(m.read(new Float32Array(METER_LEN), 3, () => m.write(new Float32Array(METER_LEN).fill(++g)))).toBe(false);
  });
  it('skips a write that is in progress (odd version)', () => {
    const buf = new SharedArrayBuffer(MeterBlock.BYTES);
    const m = new MeterBlock(buf);
    m.write(new Float32Array(METER_LEN).fill(5));
    new Int32Array(buf, 0, 1)[0] += 1; // writer stuck inside
    expect(m.read(new Float32Array(METER_LEN))).toBe(false);
  });
});

describe('RecordRing', () => {
  const make = (cap: number, stride: number) => new RecordRing(new ArrayBuffer(RecordRing.bytes(cap, stride)), cap, stride);
  const recs = (from: number, n: number, stride: number) =>
    Float32Array.from({ length: n * stride }, (_, i) => from + Math.floor(i / stride));

  it('reads back in order across wrap-around', () => {
    const r = make(8, 2);
    r.write(recs(0, 6, 2), 6);
    r.write(recs(6, 5, 2), 5); // wraps
    const b = r.readSince(3);
    expect(b.count).toBe(8);
    expect(b.cursor).toBe(11);
    expect(Array.from(b.records.filter((_, i) => i % 2 === 0))).toEqual([3, 4, 5, 6, 7, 8, 9, 10]);
  });
  it('skips the oldest when the reader is more than a capacity behind', () => {
    const r = make(4, 1);
    r.write(recs(0, 10, 1), 10);
    const b = r.readSince(0);
    expect(Array.from(b.records)).toEqual([6, 7, 8, 9]);
    expect(b.cursor).toBe(10);
    expect(r.readSince(b.cursor).count).toBe(0);
  });
  it('keeps only the last cap records when one write exceeds capacity', () => {
    const r = make(4, 1);
    r.write(recs(0, 7, 1), 7);
    expect(r.total()).toBe(7);
    expect(Array.from(r.readSince(0).records)).toEqual([3, 4, 5, 6]);
  });
  it('readRange zero-fills parts that are too old or not written yet', () => {
    const r = make(4, 1);
    r.write(recs(1, 6, 1), 6); // values 1..6, ring holds 3..6
    const out = new Float32Array(10);
    r.readRange(0, 10, out);
    expect(Array.from(out)).toEqual([0, 0, 0, 0, 0, 0, 0, 0, 0, 0].map((_, i) => (i >= 2 && i <= 5 ? i + 1 : 0)));
  });
});
