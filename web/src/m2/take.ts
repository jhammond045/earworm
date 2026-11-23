// Pure take helpers for the M2 capture page: second-pass result -> engine blobs,
// the follow delay measurements, and the keep tally. No DOM, no audio context.

import { CURVE_STRIDE, encodeCurve, encodeTrack, type TrackEvent, type TrackInput } from '../m1/track';
import type { Hit, Note, SecondPass } from './analysis';

/** Takes shorter than this are ignored (TDD "Input"). */
export const MIN_TAKE_SEC = 0.5;
/** A mono take stops a little short of 60 s. Its second pass holds the whole take in the analysis
 *  worker, which at 5 minutes would take 171 MB (280 MB for a beat take) against 35 MB at 60 s. */
export const MAX_TAKE_SEC = 59;
/** A poly take (docs/P3.md) can run 5 minutes: its pass streams the take in a second at a time. */
export const POLY_MAX_TAKE_SEC = 300;
/** The mic and frame rings hold a second more than the longest take, so its start is never
 *  overwritten; klont-processor.ts keeps the same figure. */
export const MIC_RING_SEC = POLY_MAX_TAKE_SEC + 1;
/** Index of the tuned pitch in a 25-float frame record. */
export const TUNED = 24;
/** Frames per second (10 ms frames). */
export const FRAME_RATE = 100;
/** Voicing slot of a frame record (crates/dsp/src/frame.rs): 2 when the live tier's gate was
 *  shut, that is the input was below the capture threshold. */
const VOICING = 7, SILENT = 2;

/** Whether a take's frames hold a voice: at least MIN_TAKE_SEC of frames with the gate open, the
 *  same bound a take's length must meet. A pass recorded with the mic on but nobody singing
 *  (playing keys, say) has none, and its take is not kept, so it cannot replace written notes. */
export function heardVoice(frames: Float32Array): boolean {
  let open = 0;
  for (let i = VOICING; i < frames.length; i += CURVE_STRIDE) if (frames[i] !== SILENT) open++;
  return open >= MIN_TAKE_SEC * FRAME_RATE;
}

/** Cents above C0 of the lowest representable pitch for the follow check, and C0 in Hz. */
export const C0_HZ = 16.351597831287414;

export type PageKind = 'melody' | 'beat';

// ---------------------------------------------------------------- blobs

/** Copy of `frames` with the tuned slot of every record replaced. `octave` whole octaves are added to finite values. */
export function withTuned(frames: Float32Array, tuned: ArrayLike<number>, octave = 0): Float32Array {
  const n = Math.floor(frames.length / CURVE_STRIDE);
  const out = new Float32Array(n * CURVE_STRIDE);
  out.set(frames.subarray(0, n * CURVE_STRIDE));
  for (let i = 0; i < n; i++) {
    const t = i < tuned.length ? tuned[i] : NaN;
    out[i * CURVE_STRIDE + TUNED] = Number.isFinite(t) ? t + 1200 * octave : NaN;
  }
  return out;
}

export function buildCurveBlob(frames: Float32Array, tuned: ArrayLike<number>, octave = 0): ArrayBuffer {
  return encodeCurve(withTuned(frames, tuned, octave));
}

/** Melody notes as track events. Take seconds equal clip seconds in a free section, so no warp anchors. */
export function notesToEvents(notes: Note[], octave = 0): TrackEvent[] {
  return notes.map((n) => ({ t: n.t, dur: n.dur, cents: n.cents + 1200 * octave, hard: n.hard, strength: 1 }));
}

/** Beat hits as track events: slot, strength and the shape fields (crates/klont/src/seq.rs Event, layer.rs hit_start). */
export function hitsToEvents(hits: Hit[]): TrackEvent[] {
  return hits.map((h) => ({
    t: h.t, dur: h.dur, slot: Math.max(0, Math.min(3, Math.round(h.slot))), strength: h.strength,
    cents: h.shape.cents ?? NaN,
    attack: h.shape.attack, burst: h.shape.burst, bend: h.shape.bend, hold: h.shape.hold,
    vowel: h.shape.vowel, tone: h.shape.tone, endStop: h.shape.endStop ? 1 : 0, endClick: h.shape.endClick,
  }));
}

/** Loop length in seconds: the second pass's whole-beat loop, else the take length; never zero. */
export function loopSeconds(sp: Pick<SecondPass, 'loopSec'>, frameCount: number): number {
  const take = frameCount / FRAME_RATE;
  return Number.isFinite(sp.loopSec) && sp.loopSec > 0 ? sp.loopSec : Math.max(take, 0.1);
}

export interface TrackSpec {
  kind: PageKind;
  layer: number;
  curveSlot: number;
  sp: Pick<SecondPass, 'notes' | 'hits' | 'loopSec'>;
  frameCount: number;
  octave?: number;
}

export function trackInput(s: TrackSpec): TrackInput {
  const loopSec = loopSeconds(s.sp, s.frameCount);
  const events = (s.kind === 'beat' ? hitsToEvents(s.sp.hits) : notesToEvents(s.sp.notes, s.octave ?? 0))
    .filter((e) => e.t < loopSec);
  return {
    sections: [{ feel: 'free', length: loopSec, repeats: 1 }],
    clips: [{ section: 0, layer: s.layer, loopLen: loopSec, curve: s.kind === 'melody' ? s.curveSlot : -1, events }],
    loop: true,
  };
}

export function buildTrackBlob(s: TrackSpec): ArrayBuffer {
  return encodeTrack(trackInput(s));
}

/** Mean live_hit_features (bright, low, noise) of a taught slot's hits, or null with none. */
export function meanLive(hits: Hit[]): [number, number, number] | null {
  const live = hits.filter((h) => h.live && h.live.length >= 3);
  if (!live.length) return null;
  const m: [number, number, number] = [0, 0, 0];
  for (const h of live) for (let k = 0; k < 3; k++) m[k] += h.live[k] / live.length;
  return m;
}

export function median(v: ArrayLike<number>): number {
  const a = Array.from(v).filter(Number.isFinite).sort((x, y) => x - y);
  if (!a.length) return NaN;
  const m = a.length >> 1;
  return a.length & 1 ? a[m] : (a[m - 1] + a[m]) / 2;
}

/** What the engine should be singing for each note: the median tuned pitch (octave applied) shortly after the start. */
export function noteTargets(frames: Float32Array, notes: Note[], octave = 0): { startSec: number; cents: number }[] {
  const n = Math.floor(frames.length / CURVE_STRIDE);
  const out: { startSec: number; cents: number }[] = [];
  for (const note of notes) {
    const grab = (from: number, to: number) => {
      const v: number[] = [];
      for (let i = Math.max(0, Math.floor(from * FRAME_RATE)); i < Math.min(n, Math.ceil(to * FRAME_RATE)); i++) {
        v.push(frames[i * CURVE_STRIDE + TUNED]);
      }
      return median(v);
    };
    let c = grab(note.t + 0.1, note.t + Math.min(note.dur, 0.3));
    if (!Number.isFinite(c)) c = grab(note.t, note.t + note.dur);
    if (Number.isFinite(c)) out.push({ startSec: note.t, cents: c + 1200 * octave });
  }
  return out;
}

// ---------------------------------------------------------------- follow delay

/** RMS envelope, one value per `hopMs`, over a window of `winMs` ending at the hop. */
export function envelope(x: Float32Array, sr: number, hopMs = 1, winMs = 4): Float32Array {
  const hop = Math.max(1, Math.round((sr * hopMs) / 1000));
  const win = Math.max(hop, Math.round((sr * winMs) / 1000));
  const n = Math.floor(x.length / hop);
  const e = new Float32Array(n);
  for (let i = 0; i < n; i++) {
    const end = (i + 1) * hop;
    let s = 0;
    for (let k = Math.max(0, end - win); k < end; k++) s += x[k] * x[k];
    e[i] = Math.sqrt(s / win);
  }
  return e;
}

/** Delay of the engine output envelope behind the mic envelope, ms (cross-correlation peak, parabolic refinement).
 *  Null when either side is nearly silent or the match is weak. */
export function levelDelayMs(mic: Float32Array, out: Float32Array, sr: number, maxMs = 250, hopMs = 0.5): number | null {
  const a = envelope(mic, sr, hopMs, 2 * hopMs + 2);
  const b = envelope(out, sr, hopMs, 2 * hopMs + 2);
  const n = Math.min(a.length, b.length);
  if (n < 10) return null;
  const mean = (v: Float32Array) => { let s = 0; for (let i = 0; i < n; i++) s += v[i]; return s / n; };
  const ma = mean(a), mb = mean(b);
  let ea = 0, eb = 0;
  for (let i = 0; i < n; i++) { ea += (a[i] - ma) ** 2; eb += (b[i] - mb) ** 2; }
  if (!(ea > 1e-12) || !(eb > 1e-12)) return null;
  const maxLag = Math.min(n - 1, Math.round(maxMs / hopMs));
  const minLag = -Math.round(5 / hopMs); // the output should not lead, but allow a little for noise
  const corr = new Float64Array(maxLag - minLag + 1);
  let best = -Infinity, bi = 0;
  for (let lag = minLag; lag <= maxLag; lag++) {
    let s = 0;
    const from = Math.max(0, -lag), to = Math.min(n, n - lag);
    for (let i = from; i < to; i++) s += (a[i] - ma) * (b[i + lag] - mb);
    const c = s / Math.sqrt(ea * eb);
    corr[lag - minLag] = c;
    if (c > best) { best = c; bi = lag - minLag; }
  }
  if (best < 0.3) return null;
  let off = 0;
  if (bi > 0 && bi < corr.length - 1) {
    const y0 = corr[bi - 1], y1 = corr[bi], y2 = corr[bi + 1];
    const d = y0 - 2 * y1 + y2;
    if (d < 0) off = Math.max(-0.5, Math.min(0.5, (0.5 * (y0 - y2)) / d));
  }
  return (bi + minLag + off) * hopMs;
}

/** Pitch of x around `s` by normalised autocorrelation, searched near `period` samples. Returns cents above C0 and the
 *  correlation, or null. The estimate uses x[s .. s + win + lag). */
function pitchAt(x: Float32Array, sr: number, s: number, win: number, minLag: number, maxLag: number):
    { cents: number; corr: number; span: number } | null {
  if (s < 0 || s + win + maxLag + 1 > x.length) return null;
  let e0 = 0;
  for (let i = 0; i < win; i++) e0 += x[s + i] * x[s + i];
  if (e0 < 1e-9 * win) return null;
  const r = new Float64Array(maxLag + 2);
  let bestLag = -1, best = 0;
  for (let lag = minLag - 1; lag <= maxLag + 1; lag++) {
    let c = 0, e1 = 0;
    for (let i = 0; i < win; i++) { c += x[s + i] * x[s + i + lag]; e1 += x[s + i + lag] * x[s + i + lag]; }
    const v = c / Math.sqrt(e0 * e1 + 1e-30);
    r[lag] = v;
    if (lag >= minLag && lag <= maxLag && v > best) { best = v; bestLag = lag; }
  }
  if (bestLag < 0 || best < 0.6) return null;
  let lag = bestLag;
  if (bestLag > minLag - 1 && bestLag < maxLag + 1) {
    const y0 = r[bestLag - 1], y1 = r[bestLag], y2 = r[bestLag + 1];
    const d = y0 - 2 * y1 + y2;
    if (d < 0) lag += Math.max(-0.5, Math.min(0.5, (0.5 * (y0 - y2)) / d));
  }
  const hz = sr / lag;
  return { cents: 1200 * Math.log2(hz / C0_HZ), corr: best, span: win + lag };
}

/** For each target (note start in seconds from the start of `out`, wanted pitch in cents), the time in ms from that start
 *  until the output pitch is within `tol` cents, on two windows in a row. Null when not reached within `maxMs`.
 *  The estimator looks at about three periods, so it reads a few ms late on low notes. */
export function pitchLockDelaysMs(out: Float32Array, sr: number, targets: { startSec: number; cents: number }[],
                                  tol = 50, maxMs = 400): (number | null)[] {
  const step = Math.max(1, Math.round(sr * 0.0025));
  return targets.map((t) => {
    const hz = C0_HZ * 2 ** (t.cents / 1200);
    const period = sr / hz;
    const win = Math.max(256, Math.min(2048, Math.round(2 * period)));
    const minLag = Math.max(8, Math.floor(period / 1.25));
    const maxLag = Math.ceil(period * 1.25);
    const first = Math.round(t.startSec * sr);
    const last = first + Math.round((maxMs / 1000) * sr);
    let prevOk = false;
    for (let s = first - win; s <= last; s += step) {
      const p = pitchAt(out, sr, s, win, minLag, maxLag);
      const ok = !!p && Math.abs(p.cents - t.cents) <= tol;
      if (ok && prevOk) {
        // report the time of the earlier of the two windows, centred
        const centre = s - step + (win + period) / 2;
        return Math.max(0, ((centre - first) / sr) * 1000);
      }
      prevOk = ok;
    }
    return null;
  });
}

export interface FollowResult { levelMs: number | null; pitchMs: number | null; pitchN: number; pitchMissed: number }

export function measureFollow(mic: Float32Array, out: Float32Array, sr: number, targets: { startSec: number; cents: number }[]): FollowResult {
  const levelMs = levelDelayMs(mic, out, sr);
  const per = pitchLockDelaysMs(out, sr, targets);
  const got = per.filter((v): v is number => v !== null);
  return { levelMs, pitchMs: got.length ? median(got) : null, pitchN: got.length, pitchMissed: per.length - got.length };
}

// ---------------------------------------------------------------- tally

export interface TakeScore { kept: boolean | null }

/** Keep tally: rated takes only. The target is 80%. */
export function tally(scores: TakeScore[], target = 0.8): { kept: number; rated: number; pct: number; meets: boolean } {
  const rated = scores.filter((s) => s.kept !== null);
  const kept = rated.filter((s) => s.kept).length;
  const pct = rated.length ? (100 * kept) / rated.length : 0;
  return { kept, rated: rated.length, pct, meets: rated.length > 0 && kept / rated.length >= target };
}

export function tallyText(scores: TakeScore[]): string {
  const t = tally(scores);
  return `kept ${t.kept} of ${t.rated} (${t.pct.toFixed(0)}%) against the 80% target`;
}
