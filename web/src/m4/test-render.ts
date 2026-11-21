// Turns a source and its match into a render job for the offline klont engine, the
// way the Rust blind-match proxy does (crates/analysis/tests/blind_match.rs,
// render_match): neutral base, the match as the Influence target at amount 1 with
// every group on, the source's steady region as the grain source when the match
// asks for it, one note at the matched pitch (C4 when unpitched) for the source's
// length, then the release. The page renders this in a worker; the Node test
// renders the same job with renderSong.

import { LAYER_FIELDS, LAYER_OFF_FIELDS, N_MACROS } from '../m1/protocol';
import { COUNT, neutralSet } from '../gen/params';
import { encodeLayerBlob, encodeSample } from '../m1/blobs';
import { encodeLayer } from '../m1/layer';
import { GRAIN_SAMPLES } from '../project/compile';
import { classicVectors, valuesFromVocab } from '../synth/classic';
import { encodeTrack } from '../m1/track';
import type { RenderOptions, SongBlobs } from '../m1/render-core';
import type { Match } from './match';

export const RENDER_RATE = 48000;
/** Seconds rendered after the source's length, for the note's release. */
export const RELEASE_S = 1;
/** Cents above C0 of C4, where an unpitched match is played. */
export const C4_CENTS = 4800;
const MIN_NOTE_S = 0.05;

export interface Source {
  signal: Float32Array;
  sampleRate: number;
}

export interface MatchPlan {
  song: SongBlobs;
  options: RenderOptions;
  /** Seconds of source; the note's section length. */
  totalS: number;
  /** Seconds the note is held before its release. */
  holdS: number;
  cents: number;
  grain: boolean;
  /** Frames the render is expected to have. */
  frames: number;
}

/** Layer blob (for the engine's classic melody patch) with a neutral base and `target` (the old
 *  parameters, normalized) as the Influence target at amount 1, all groups. */
export function matchLayerBlob(target: ArrayLike<number>): ArrayBuffer {
  if (target.length !== COUNT) throw new RangeError(`match target has ${target.length} values, wanted ${COUNT}`);
  const fields = new Float32Array(decodeFields(encodeLayer({ kind: 'melody', influence: { amount: 1 } }).layer));
  const v = classicVectors('melody', valuesFromVocab('melody', neutralSet()), target);
  return encodeLayerBlob({ fields, values: v.values, macros: new Array<number>(N_MACROS).fill(0.5), target: v.target, groups: v.groups });
}

function decodeFields(blob: ArrayBuffer): Float32Array {
  return new Float32Array(blob, LAYER_OFF_FIELDS * 4, LAYER_FIELDS).slice();
}

export function planMatchRender(src: Source, m: Match): MatchPlan {
  const totalS = Math.max(MIN_NOTE_S, src.signal.length / src.sampleRate);
  const holdS = Math.min(Math.max(MIN_NOTE_S, m.durationS), totalS);
  const cents = m.pitched ? m.pitchCents : C4_CENTS;
  const track = encodeTrack({
    sections: [{ feel: 'free', length: totalS }],
    clips: [{ section: 0, layer: 0, loopLen: totalS, events: [{ t: 0, dur: holdS, cents, strength: 1 }] }],
    loop: false,
  });
  const song: SongBlobs = { track, layers: [matchLayerBlob(m.target)] };
  let grain = false;
  if (m.useSound && m.grain) {
    const g = m.grain;
    const end = Math.min(g.end, g.start + GRAIN_SAMPLES, src.signal.length);
    if (end > g.start) {
      song.samples = [{ slot: 0, buf: encodeSample(src.sampleRate, g.refCents, src.signal.slice(g.start, end)) }];
      grain = true;
    }
  }
  const options: RenderOptions = { sampleRate: RENDER_RATE, maxSeconds: totalS + RELEASE_S, once: true };
  return { song, options, totalS, holdS, cents, grain, frames: Math.round((totalS + RELEASE_S) * RENDER_RATE) };
}

/** Left and right averaged to one channel, for playback and the test. */
export function toMono(left: Float32Array, right: Float32Array): Float32Array {
  const out = new Float32Array(left.length);
  for (let i = 0; i < out.length; i++) out[i] = 0.5 * (left[i] + right[i]);
  return out;
}
