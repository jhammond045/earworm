// Pure signal-processing helpers for the delay page. No DOM, no audio nodes,
// so they can be tested against synthetic signals.

export type Arr = ArrayLike<number>;

export function median(xs: Arr): number {
  const n = xs.length;
  if (n === 0) return NaN;
  const s = Array.from(xs).sort((a, b) => a - b);
  return n % 2 ? s[(n - 1) >> 1] : (s[n / 2 - 1] + s[n / 2]) / 2;
}

export interface Spread { n: number; min: number; max: number; stdev: number }

export function spread(xs: Arr): Spread {
  const n = xs.length;
  if (n === 0) return { n: 0, min: NaN, max: NaN, stdev: NaN };
  let min = Infinity, max = -Infinity, sum = 0;
  for (let i = 0; i < n; i++) { min = Math.min(min, xs[i]); max = Math.max(max, xs[i]); sum += xs[i]; }
  const mean = sum / n;
  let v = 0;
  for (let i = 0; i < n; i++) v += (xs[i] - mean) ** 2;
  return { n, min, max, stdev: Math.sqrt(v / n) };
}

/** Short Hann-windowed linear chirp. It has a very sharp autocorrelation peak,
 *  carries more energy than a one-sample click, and a small speaker can play it. */
export function chirp(sr: number, ms = 8, f0 = 500, f1 = 8000, amp = 0.8): Float32Array {
  const n = Math.round((ms / 1000) * sr);
  const out = new Float32Array(n);
  const T = n / sr;
  const hi = Math.min(f1, 0.45 * sr);
  for (let i = 0; i < n; i++) {
    const t = i / sr;
    const phase = 2 * Math.PI * (f0 * t + ((hi - f0) / (2 * T)) * t * t);
    const w = 0.5 - 0.5 * Math.cos((2 * Math.PI * (i + 0.5)) / n);
    out[i] = amp * w * Math.sin(phase);
  }
  return out;
}

export interface LagResult {
  /** Lag in samples (fractional, from a parabola through the peak), relative to `start`. */
  lag: number;
  /** Raw correlation at the best integer lag. */
  peak: number;
  /** Peak divided by sqrt(template energy * signal energy under it); 1 is a perfect match. */
  norm: number;
}

/** Direct cross-correlation of `template` against `signal` at offsets
 *  start .. start+maxLag. Returns the best lag. */
export function crossCorrelationLag(signal: Arr, template: Arr, maxLag: number, start = 0): LagResult {
  const m = template.length;
  const last = Math.min(maxLag, signal.length - start - m);
  if (last < 0) return { lag: NaN, peak: 0, norm: 0 };
  const corr = new Float64Array(last + 1);
  let best = 0, bestV = -Infinity;
  for (let L = 0; L <= last; L++) {
    let s = 0;
    const base = start + L;
    for (let i = 0; i < m; i++) s += template[i] * signal[base + i];
    corr[L] = s;
    if (s > bestV) { bestV = s; best = L; }
  }
  let lag = best;
  if (best > 0 && best < last) {
    const a = corr[best - 1], b = corr[best], c = corr[best + 1];
    const d = a - 2 * b + c;
    if (d < 0) lag = best + (0.5 * (a - c)) / d;
  }
  let et = 0, es = 0;
  for (let i = 0; i < m; i++) { et += template[i] ** 2; es += signal[start + best + i] ** 2; }
  const norm = et > 0 && es > 0 ? bestV / Math.sqrt(et * es) : 0;
  return { lag, peak: bestV, norm };
}

/** Trailing moving average of |x| over [from, to), as a Float32Array indexed from `from`. */
function envelope(x: Arr, from: number, to: number, win: number): Float32Array {
  const out = new Float32Array(Math.max(0, to - from));
  let sum = 0;
  for (let i = Math.max(0, from - win); i < to; i++) {
    sum += Math.abs(x[i]);
    if (i - win >= 0) sum -= Math.abs(x[i - win]);
    if (i >= from) out[i - from] = sum / win;
  }
  return out;
}

function percentile(a: Float32Array, p: number): number {
  if (a.length === 0) return 0;
  const s = Float32Array.from(a).sort();
  return s[Math.min(s.length - 1, Math.floor(p * s.length))];
}

export interface OnsetOptions {
  /** Fraction of the way from the noise floor to the peak envelope. */
  frac?: number;
  smoothMs?: number;
  /** Below this peak envelope the window is treated as silent. */
  minPeak?: number;
}

/** Index of the first sample where the smoothed envelope of x in [from, to)
 *  rises through floor + frac * (peak - floor); -1 if the window is silent.
 *  The window should start before the onset, in quiet. */
export function onsetIndex(x: Arr, sr: number, from: number, to: number, opts: OnsetOptions = {}): number {
  const { frac = 0.1, smoothMs = 2, minPeak = 1e-3 } = opts;
  from = Math.max(0, from);
  to = Math.min(x.length, to);
  const win = Math.max(1, Math.round((smoothMs / 1000) * sr));
  const env = envelope(x, from, to, win);
  let peak = 0;
  for (let i = 0; i < env.length; i++) if (env[i] > peak) peak = env[i];
  if (peak < minPeak) return -1;
  const floor = percentile(env, 0.05);
  const thr = floor + frac * (peak - floor);
  for (let i = 0; i < env.length; i++) if (env[i] >= thr) return from + i;
  return -1;
}

export interface Burst { start: number; end: number }

/** Rough burst segmentation over a whole recording: runs where the 5 ms
 *  envelope is above a fraction of the global peak, short gaps merged, short
 *  runs dropped. Starts are approximate; refine with onsetIndex. */
export function segmentBursts(
  x: Arr, sr: number,
  opts: { smoothMs?: number; frac?: number; minMs?: number; mergeMs?: number } = {},
): Burst[] {
  const { smoothMs = 5, frac = 0.2, minMs = 200, mergeMs = 100 } = opts;
  const win = Math.max(1, Math.round((smoothMs / 1000) * sr));
  const env = envelope(x, 0, x.length, win);
  let peak = 0;
  for (let i = 0; i < env.length; i++) if (env[i] > peak) peak = env[i];
  if (peak <= 0) return [];
  const thr = frac * peak;
  const runs: Burst[] = [];
  let cur: Burst | null = null;
  const merge = Math.round((mergeMs / 1000) * sr);
  for (let i = 0; i < env.length; i++) {
    if (env[i] >= thr) {
      if (cur && i - cur.end <= merge) cur.end = i + 1;
      else { cur = { start: i, end: i + 1 }; runs.push(cur); }
    }
  }
  const minLen = Math.round((minMs / 1000) * sr);
  return runs.filter((r) => r.end - r.start >= minLen);
}

export interface F0Result { f0: number; clarity: number }

/** Fundamental from the normalised autocorrelation of one window (default
 *  20 ms) starting at `start`. Needs winMs + 1/fmin of samples after `start`.
 *  Picks the smallest lag whose peak is within 10% of the best, which avoids
 *  octave-down errors. f0 is 0 when nothing periodic is found. */
export function estimateF0(
  x: Arr, start: number, sr: number,
  opts: { winMs?: number; fmin?: number; fmax?: number } = {},
): F0Result {
  const { winMs = 20, fmin = 100, fmax = 500 } = opts;
  const W = Math.round((winMs / 1000) * sr);
  const minTau = Math.max(2, Math.floor(sr / fmax) - 1);
  const maxTau = Math.ceil(sr / fmin) + 1;
  if (start < 0 || start + W + maxTau > x.length) return { f0: 0, clarity: 0 };
  let e0 = 0;
  for (let i = 0; i < W; i++) e0 += x[start + i] ** 2;
  if (e0 < 1e-12) return { f0: 0, clarity: 0 };
  const r = new Float64Array(maxTau + 2);
  let best = 0;
  for (let tau = minTau; tau <= maxTau + 1; tau++) {
    let s = 0, et = 0;
    for (let i = 0; i < W; i++) {
      const v = x[start + i + tau];
      s += x[start + i] * v;
      et += v * v;
    }
    r[tau] = et > 0 ? s / Math.sqrt(e0 * et) : 0;
    if (tau <= maxTau && r[tau] > best) best = r[tau];
  }
  if (best < 0.3) return { f0: 0, clarity: best };
  for (let tau = minTau + 1; tau <= maxTau; tau++) {
    if (r[tau] >= 0.9 * best && r[tau] >= r[tau - 1] && r[tau] >= r[tau + 1]) {
      const a = r[tau - 1], b = r[tau], c = r[tau + 1];
      const d = a - 2 * b + c;
      const t = d < 0 ? tau + (0.5 * (a - c)) / d : tau;
      return { f0: sr / t, clarity: b };
    }
  }
  return { f0: 0, clarity: best };
}

/** Median f0 over sliding windows in [from, to), ignoring unpitched ones. */
export function medianF0(x: Arr, sr: number, from: number, to: number, hopMs = 5): number {
  const hop = Math.max(1, Math.round((hopMs / 1000) * sr));
  const vals: number[] = [];
  for (let s = from; s < to; s += hop) {
    const e = estimateF0(x, s, sr);
    if (e.f0 > 0 && e.clarity >= 0.8) vals.push(e.f0);
  }
  return median(vals);
}

export const centsBetween = (a: number, b: number) => 1200 * Math.log2(a / b);

/** First window start in [from, to) from which `need` consecutive 20 ms
 *  windows (2 ms hop) have f0 within tolCents of targetHz; -1 if none. A
 *  window that straddles the change can pass a few ms early. */
export function pitchLockIndex(
  x: Arr, sr: number, from: number, to: number, targetHz: number,
  tolCents = 50, need = 3, hopMs = 2, minClarity = 0.95,
): number {
  const hop = Math.max(1, Math.round((hopMs / 1000) * sr));
  let run = 0, first = -1;
  for (let s = Math.max(0, from); s < to; s += hop) {
    const e = estimateF0(x, s, sr);
    if (e.f0 > 0 && e.clarity >= minClarity && Math.abs(centsBetween(e.f0, targetHz)) <= tolCents) {
      if (run === 0) first = s;
      if (++run >= need) return first;
    } else run = 0;
  }
  return -1;
}

export interface Hum { samples: Float32Array; burstStarts: number[]; f0: number }

/** Voice-like test signal: band-limited sawtooth with two vowel formants
 *  (about "ah"), in bursts with silence between, 10 ms fades. Built
 *  additively so nothing aliases and no filter state is needed. */
export function synthHum(
  sr: number, f0 = 220, bursts = 6, burstMs = 600, gapMs = 600, leadMs = 300,
): Hum {
  const burstN = Math.round((burstMs / 1000) * sr);
  const gapN = Math.round((gapMs / 1000) * sr);
  const lead = Math.round((leadMs / 1000) * sr);
  const total = lead + bursts * (burstN + gapN);
  const samples = new Float32Array(total);
  const kmax = Math.floor((0.45 * sr) / f0);
  const formant = (f: number) =>
    0.05 + 1 / (1 + ((f - 700) / 130) ** 2) + 0.7 / (1 + ((f - 1220) / 90) ** 2);
  const amps: number[] = [];
  for (let k = 1; k <= kmax; k++) amps.push(formant(k * f0) / k);
  const one = new Float32Array(burstN);
  for (let k = 1; k <= kmax; k++) {
    const w = (2 * Math.PI * k * f0) / sr, a = amps[k - 1];
    for (let i = 0; i < burstN; i++) one[i] += a * Math.sin(w * i);
  }
  let peak = 0;
  for (let i = 0; i < burstN; i++) peak = Math.max(peak, Math.abs(one[i]));
  const fade = Math.round(0.01 * sr);
  for (let i = 0; i < burstN; i++) {
    let g = 0.5 / peak;
    if (i < fade) g *= 0.5 - 0.5 * Math.cos((Math.PI * i) / fade);
    if (i >= burstN - fade) g *= 0.5 - 0.5 * Math.cos((Math.PI * (burstN - 1 - i)) / fade);
    one[i] *= g;
  }
  const burstStarts: number[] = [];
  for (let b = 0; b < bursts; b++) {
    const s = lead + b * (burstN + gapN);
    burstStarts.push(s);
    samples.set(one, s);
  }
  return { samples, burstStarts, f0 };
}

/** Engine settings for the internal delay test. Level is at its minimum
 *  because the engine itself is never played here. Delay is read from onsets
 *  and pitch, which do not depend on level. */
export const ENGINE_TEST_PARAMS: [name: string, value: number][] = [
  ['depth.mouth', 0], ['tune.snap', 0], ['depth.loud', 1], ['harm_mix', 1], ['noise_mix', 0], ['level', -60],
];

export interface BurstDelay {
  micOnsetS: number;
  levelDelayMs: number | null;
  pitchDelayMs: number | null;
  inputF0: number | null;
}

/** Level and pitch delay per hum burst, from the engine's view of the mic
 *  and its output on the same sample clock. */
export function analyseEngineCapture(mic: Arr, out: Arr, sr: number): BurstDelay[] {
  const w = (s: number) => Math.round(s * sr);
  const ms = (samples: number) => (samples / sr) * 1000;
  const fin = (x: number) => (Number.isFinite(x) ? x : null);
  const res: BurstDelay[] = [];
  for (const b of segmentBursts(mic, sr)) {
    const r: BurstDelay = { micOnsetS: b.start / sr, levelDelayMs: null, pitchDelayMs: null, inputF0: null };
    // A low threshold on both: the synth reaches full level faster than the
    // faded-in test hum, so a 10% threshold would read as negative delay.
    // The output is digital and quiet (see ENGINE_TEST_PARAMS), so its
    // silence floor is far below the default minPeak.
    const micOn = onsetIndex(mic, sr, b.start - w(0.05), b.start + w(0.1), { frac: 0.02 });
    if (micOn >= 0) {
      r.micOnsetS = micOn / sr;
      const outOn = onsetIndex(out, sr, micOn - w(0.03), micOn + w(0.4), { frac: 0.02, minPeak: 1e-7 });
      if (outOn >= 0) r.levelDelayMs = ms(outOn - micOn);
      const f0 = medianF0(mic, sr, micOn + w(0.2), Math.min(b.end - w(0.06), micOn + w(0.5)));
      r.inputF0 = fin(f0);
      if (Number.isFinite(f0)) {
        const from = micOn - w(0.01), to = micOn + w(0.6);
        let lock = pitchLockIndex(out, sr, from, to, f0);
        if (lock < 0) lock = pitchLockIndex(out, sr, from, to, f0, 50, 3, 2, 0.8);
        if (lock >= 0) r.pitchDelayMs = ms(lock - micOn);
      }
    }
    res.push(r);
  }
  return res;
}
