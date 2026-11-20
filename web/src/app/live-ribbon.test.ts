import { describe, expect, it } from 'vitest';
import { F_LOUDNESS, F_PITCH, RECORD_TUNED } from '../host/protocol';
import { recordFromFrame } from './live-ribbon';

describe('recordFromFrame', () => {
  it('copies the live pitch into the slot the ribbon draws', () => {
    const frame = new Float32Array(25);
    frame[F_PITCH] = 4800;
    frame[F_LOUDNESS] = 0.4;
    const rec = recordFromFrame(frame);
    expect(rec[F_PITCH]).toBe(4800);
    expect(rec[F_LOUDNESS]).toBeCloseTo(0.4);
    expect(rec[RECORD_TUNED]).toBe(4800);
  });

  it('leaves the tuned slot empty when the frame has no pitch', () => {
    const frame = new Float32Array(25);
    frame[F_PITCH] = Number.NaN;
    expect(Number.isFinite(recordFromFrame(frame)[RECORD_TUNED])).toBe(false);
  });
});
