// One klont frame (25 floats, pitch in slot 0) as the record the Live ribbon already draws.

import { F_PITCH, MAX_FRAME_LEN, RECORD_TUNED } from '../host/protocol';

export function recordFromFrame(frame: Float32Array): Float32Array {
  const rec = new Float32Array(MAX_FRAME_LEN + 1);
  rec.set(frame.subarray(0, Math.min(frame.length, MAX_FRAME_LEN)));
  const pitch = frame[F_PITCH];
  rec[RECORD_TUNED] = Number.isFinite(pitch) ? pitch : Number.NaN;
  return rec;
}
