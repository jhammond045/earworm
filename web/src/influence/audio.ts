// Getting source sounds in: file decode, silence trim, mic recording.
// Also the small WAV encoder and IndexedDB storage the two pages share.

import { micConstraints } from '../host/engine-host';

export const MAX_SOURCE_S = 10;
const SILENCE = Math.pow(10, -50 / 20);

/** Trim leading/trailing samples below -50 dBFS and cap at 10 s. */
export function trimAndCap(x: Float32Array, sampleRate: number): Float32Array {
  let a = 0;
  let b = x.length;
  while (a < b && Math.abs(x[a]) < SILENCE) a++;
  while (b > a && Math.abs(x[b - 1]) < SILENCE) b--;
  b = Math.min(b, a + Math.floor(MAX_SOURCE_S * sampleRate));
  return x.slice(a, b);
}

function toMono(buf: AudioBuffer): Float32Array {
  const n = buf.length;
  const out = new Float32Array(n);
  for (let c = 0; c < buf.numberOfChannels; c++) {
    const ch = buf.getChannelData(c);
    for (let i = 0; i < n; i++) out[i] += ch[i] / buf.numberOfChannels;
  }
  return out;
}

/** Decode encoded audio bytes to mono at the context's rate, trimmed and capped. */
export async function decodeBytes(ctx: BaseAudioContext, bytes: ArrayBuffer): Promise<Float32Array> {
  const buf = await ctx.decodeAudioData(bytes);
  const x = trimAndCap(toMono(buf), ctx.sampleRate);
  if (x.length < ctx.sampleRate * 0.05) throw new Error('sound is silent or shorter than 50 ms');
  return x;
}

export function decodeFile(ctx: BaseAudioContext, file: File): Promise<Float32Array> {
  return file.arrayBuffer().then((b) => decodeBytes(ctx, b));
}

/** Record `seconds` from the mic with MediaRecorder, then decode the result. */
export async function recordMic(ctx: BaseAudioContext, seconds: number): Promise<Float32Array> {
  const stream = await navigator.mediaDevices.getUserMedia(micConstraints());
  try {
    const rec = new MediaRecorder(stream);
    const chunks: Blob[] = [];
    rec.ondataavailable = (e) => { if (e.data.size) chunks.push(e.data); };
    const stopped = new Promise<void>((res) => { rec.onstop = () => res(); });
    rec.start();
    await new Promise((r) => setTimeout(r, seconds * 1000));
    rec.stop();
    await stopped;
    const blob = new Blob(chunks, { type: rec.mimeType });
    return await decodeBytes(ctx, await blob.arrayBuffer());
  } finally {
    stream.getTracks().forEach((t) => t.stop());
  }
}

/** Linear-interpolation resample; good enough for auditioning. */
export function resample(x: Float32Array, from: number, to: number): Float32Array {
  if (from === to) return x;
  const n = Math.floor((x.length * to) / from);
  const out = new Float32Array(n);
  const step = from / to;
  for (let i = 0; i < n; i++) {
    const p = i * step;
    const k = Math.floor(p);
    const f = p - k;
    out[i] = x[k] * (1 - f) + (x[Math.min(k + 1, x.length - 1)] ?? 0) * f;
  }
  return out;
}

/** 16-bit mono PCM WAV. */
export function wavBlob(x: Float32Array, sampleRate: number): Blob {
  const buf = new ArrayBuffer(44 + x.length * 2);
  const v = new DataView(buf);
  const str = (o: number, s: string) => { for (let i = 0; i < s.length; i++) v.setUint8(o + i, s.charCodeAt(i)); };
  str(0, 'RIFF'); v.setUint32(4, 36 + x.length * 2, true); str(8, 'WAVE');
  str(12, 'fmt '); v.setUint32(16, 16, true); v.setUint16(20, 1, true); v.setUint16(22, 1, true);
  v.setUint32(24, sampleRate, true); v.setUint32(28, sampleRate * 2, true);
  v.setUint16(32, 2, true); v.setUint16(34, 16, true);
  str(36, 'data'); v.setUint32(40, x.length * 2, true);
  for (let i = 0; i < x.length; i++) {
    const s = Math.max(-1, Math.min(1, x[i]));
    v.setInt16(44 + i * 2, Math.round(s * 32767), true);
  }
  return new Blob([buf], { type: 'audio/wav' });
}

export function downloadBlob(blob: Blob, filename: string): void {
  const a = document.createElement('a');
  a.href = URL.createObjectURL(blob);
  a.download = filename;
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 10000);
}

// ---- IndexedDB: saved sources for the blind-test page ----

export interface SavedSource {
  name: string;
  sampleRate: number;
  signal: Float32Array;
}

const DB = 'earworm-influence';
const STORE = 'sources';

function openDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(DB, 1);
    req.onupgradeneeded = () => req.result.createObjectStore(STORE, { autoIncrement: true });
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error ?? new Error('IndexedDB open failed'));
  });
}

function done(tx: IDBTransaction): Promise<void> {
  return new Promise((resolve, reject) => {
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error);
    tx.onabort = () => reject(tx.error ?? new Error('IndexedDB transaction aborted'));
  });
}

/** Replace the saved set. Throws if storage is unavailable. */
export async function saveSources(list: SavedSource[]): Promise<void> {
  const db = await openDb();
  try {
    const tx = db.transaction(STORE, 'readwrite');
    const s = tx.objectStore(STORE);
    s.clear();
    for (const x of list) s.add(x);
    await done(tx);
  } finally {
    db.close();
  }
}

export async function loadSources(): Promise<SavedSource[]> {
  const db = await openDb();
  try {
    const req = db.transaction(STORE).objectStore(STORE).getAll();
    return await new Promise((resolve, reject) => {
      req.onsuccess = () => resolve(req.result as SavedSource[]);
      req.onerror = () => reject(req.error);
    });
  } finally {
    db.close();
  }
}

export const clearSources = () => saveSources([]);
