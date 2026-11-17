// Main-thread side of the audio engine: loads engine.wasm, starts the
// AudioWorklet that hosts it, and exposes the four shared blocks from the TDD
// (control ring, meter block, mic ring, frame ring) plus an output ring used
// by the delay page. With crossOriginIsolated false (or ?fallback=1), the same
// data travels over the worklet's MessagePort instead (TDD "Cross-origin
// isolation and the fallback").
//
// THIS FILE IS THE CONTRACT the pages code against. Signatures are fixed;
// bodies are filled in by the host implementation.

export type TransportMode = 'shared' | 'message';

export interface EngineHostOptions {
  /** Band count for the vowel shape and band gains: 16 (TDD) or 24 (M0 trial). */
  nBands: 16 | 24;
  /** Force the MessagePort fallback even when cross-origin isolated. */
  forceFallback?: boolean;
}

/** One frame-ring record: a feature frame plus the tuned pitch (protocol.ts). */
export interface FrameBatch {
  /** Records copied out since the cursor, each RECORD_STRIDE floats, oldest first. */
  records: Float32Array;
  count: number;
  /** Pass back on the next call. */
  cursor: number;
}

/** Engine wasm bytes, fetched once and cached. */
export declare function loadEngineBytes(): Promise<ArrayBuffer>;

/** getUserMedia constraints from the TDD: EC, NS and AGC off, one channel. */
export declare function micConstraints(deviceId?: string): MediaStreamConstraints;

/** AudioContext with the interactive latency hint. Call from a user gesture. */
export declare function createAudioContext(): AudioContext;

/** True when the page asked for the fallback (?fallback=1) or is not isolated. */
export declare function wantsFallback(): boolean;

export declare class EngineHost {
  readonly ctx: AudioContext;
  readonly node: AudioWorkletNode;
  readonly mode: TransportMode;
  readonly nBands: 16 | 24;
  readonly sampleRate: number;

  /** Adds the worklet module, instantiates the engine in it, sets up rings. */
  static create(ctx: AudioContext, opts: EngineHostOptions): Promise<EngineHost>;

  // Control ring (main -> audio)
  setParam(id: number, value: number): void;
  setToneBand(index: number, db: number): void;
  setNoiseBand(index: number, db: number): void;
  noteOn(cents: number): void;
  noteOff(): void;
  setLive(on: boolean): void;
  resetSinger(): void;
  panic(): void;

  /** Connect a mic stream to the engine input. Returns the source node. */
  connectMic(stream: MediaStream): MediaStreamAudioSourceNode;

  /** Copy the latest meter block into `out` (METER_LEN floats). Returns false
   *  if nothing has been published yet. Seqlock-safe in shared mode. */
  readMeters(out: Float32Array): boolean;

  /** Frame-ring records written since `cursor` (start with 0). If the reader
   *  fell more than the ring capacity behind, the oldest are skipped. */
  readFrames(cursor: number): FrameBatch;

  /** Total samples written so far to the mic and output rings (same clock). */
  captureCount(): number;

  /** Copy samples [start, start + length) of the high-passed mic signal and
   *  of the engine output, by absolute sample index (see captureCount). Parts
   *  older than the 60 s ring are zero. */
  readCapture(start: number, length: number): { mic: Float32Array; out: Float32Array };

  /** Disconnect and close the worklet node (the AudioContext is left open). */
  dispose(): void;
}

/** Offline engine on the calling thread (Influence auditions, tests). */
export declare class OfflineEngine {
  static create(sampleRate: number, nBands: 16 | 24): Promise<OfflineEngine>;
  readonly sampleRate: number;
  readonly nBands: 16 | 24;
  /** Render one note with the given parameter set. Applies every scalar and
   *  band, sends NOTE_ON, renders `holdS`, NOTE_OFF, renders `tailS`. */
  renderNote(set: { scalars: ArrayLike<number>; tone: ArrayLike<number>; noise: ArrayLike<number> },
             cents: number, holdS: number, tailS: number): Float32Array;
  dispose(): void;
}
