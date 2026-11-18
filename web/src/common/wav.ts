// Minimal WAV writer for downloads (hum corpus, delay measurements, M1 export).

export type WavFormat = 'pcm16' | 'float32';

/** Encode samples as a WAV file: mono, or one array per channel (equal lengths, interleaved here).
 *  16-bit PCM by default; 'float32' keeps exact values. */
export function encodeWav(input: Float32Array | Float32Array[], sampleRate: number, format: WavFormat = 'pcm16'): Blob {
  const chans = Array.isArray(input) ? input : [input];
  const nch = chans.length;
  const samples = nch === 1 ? chans[0] : interleave(chans);
  const float = format === 'float32';
  const bytesPer = float ? 4 : 2;
  const dataLen = samples.length * bytesPer;
  const buf = new ArrayBuffer(44 + dataLen);
  const v = new DataView(buf);
  const str = (off: number, s: string) => {
    for (let i = 0; i < s.length; i++) v.setUint8(off + i, s.charCodeAt(i));
  };
  str(0, 'RIFF');
  v.setUint32(4, 36 + dataLen, true);
  str(8, 'WAVE');
  str(12, 'fmt ');
  v.setUint32(16, 16, true);
  v.setUint16(20, float ? 3 : 1, true); // 3 = IEEE float, 1 = PCM
  v.setUint16(22, nch, true);
  v.setUint32(24, Math.round(sampleRate), true);
  v.setUint32(28, Math.round(sampleRate) * bytesPer * nch, true);
  v.setUint16(32, bytesPer * nch, true);
  v.setUint16(34, bytesPer * 8, true);
  str(36, 'data');
  v.setUint32(40, dataLen, true);
  let off = 44;
  for (let i = 0; i < samples.length; i++, off += bytesPer) {
    const x = samples[i];
    if (float) {
      v.setFloat32(off, Number.isFinite(x) ? x : 0, true);
    } else {
      const c = Number.isFinite(x) ? Math.max(-1, Math.min(1, x)) : 0;
      v.setInt16(off, Math.round(c < 0 ? c * 32768 : c * 32767), true);
    }
  }
  return new Blob([buf], { type: 'audio/wav' });
}

function interleave(chans: Float32Array[]): Float32Array {
  const n = chans[0].length;
  const out = new Float32Array(n * chans.length);
  chans.forEach((c, k) => { if (c.length !== n) throw new RangeError('wav: channels differ in length'); for (let i = 0; i < n; i++) out[i * chans.length + k] = c[i]; });
  return out;
}

/** Save a blob through a temporary link. */
export function download(blob: Blob, name: string): void {
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}
