import { describe, expect, it } from 'vitest';
import { encodeWav } from './wav';

async function view(b: Blob) {
  return new DataView(await b.arrayBuffer());
}
const tag = (v: DataView, o: number) => String.fromCharCode(v.getUint8(o), v.getUint8(o + 1), v.getUint8(o + 2), v.getUint8(o + 3));

describe('encodeWav', () => {
  it('writes a 16-bit PCM header and clamped samples', async () => {
    const v = await view(encodeWav(new Float32Array([0, 0.5, -1, 2, NaN]), 48000));
    expect(tag(v, 0)).toBe('RIFF');
    expect(tag(v, 8)).toBe('WAVE');
    expect(v.getUint16(20, true)).toBe(1);
    expect(v.getUint16(22, true)).toBe(1);
    expect(v.getUint32(24, true)).toBe(48000);
    expect(v.getUint32(28, true)).toBe(96000);
    expect(v.getUint16(34, true)).toBe(16);
    expect(v.getUint32(40, true)).toBe(10);
    expect(v.byteLength).toBe(54);
    expect(v.getUint32(4, true)).toBe(v.byteLength - 8);
    expect(v.getInt16(44, true)).toBe(0);
    expect(v.getInt16(46, true)).toBe(16384);
    expect(v.getInt16(48, true)).toBe(-32768);
    expect(v.getInt16(50, true)).toBe(32767);
    expect(v.getInt16(52, true)).toBe(0);
  });

  it('writes float32 exactly', async () => {
    const x = new Float32Array([0.123456, -0.75, 1.5]);
    const v = await view(encodeWav(x, 44100, 'float32'));
    expect(v.getUint16(20, true)).toBe(3);
    expect(v.getUint16(34, true)).toBe(32);
    expect(v.getUint32(28, true)).toBe(44100 * 4);
    expect(v.byteLength).toBe(44 + 12);
    for (let i = 0; i < 3; i++) expect(v.getFloat32(44 + 4 * i, true)).toBe(x[i]);
  });

  it('handles an empty buffer', async () => {
    const v = await view(encodeWav(new Float32Array(0), 48000));
    expect(v.byteLength).toBe(44);
    expect(v.getUint32(40, true)).toBe(0);
  });

  it('interleaves two channels', async () => {
    const v = await view(encodeWav([new Float32Array([0.5, 0]), new Float32Array([-0.5, 1])], 44100));
    expect(v.getUint16(22, true)).toBe(2);
    expect(v.getUint32(28, true)).toBe(44100 * 4);
    expect(v.getUint16(32, true)).toBe(4);
    expect(v.getUint32(40, true)).toBe(8);
    expect([44, 46, 48, 50].map((o) => v.getInt16(o, true))).toEqual([16384, -16384, 0, 32767]);
  });
});
