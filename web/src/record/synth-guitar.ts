// A synthetic strummed guitar for the poly-take checks (record/poly.test.ts) and the poly pass
// benchmark (poly-bench.html): known notes, so what the note model finds can be counted.

/** Open-string MIDI notes, low E to high E. */
export const STRINGS = [40, 45, 50, 55, 59, 64];
/** Frets per string (-1: not played): E, Am, C, G, D, Em, A, F (barre). */
export const CHORDS = [
  [0, 2, 2, 1, 0, 0], [-1, 0, 2, 2, 1, 0], [-1, 3, 2, 0, 1, 0], [3, 2, 0, 0, 0, 3],
  [-1, -1, 0, 2, 3, 2], [0, 2, 2, 0, 0, 0], [-1, 0, 2, 2, 2, 0], [1, 3, 3, 2, 1, 1],
];
/** A strum every STRUM_S seconds, strings 15 ms apart, low to high. */
export const STRUM_S = 2;

export interface Played { t: number; midi: number }

/** A plucked-string guitar (Karplus-Strong: a noise burst through a delay line with a gentle
 *  low-pass in the loop), strummed chords, each left to ring until just after the next strum. */
export function guitar(n: number, sr: number): { audio: Float32Array; played: Played[] } {
  const out = new Float32Array(n);
  const played: Played[] = [];
  let seed = 7;
  const rnd = () => { seed = (Math.imul(seed, 1103515245) + 12345) >>> 0; return seed / 2 ** 32 - 0.5; };
  for (let k = 0; k * STRUM_S * sr < n; k++) {
    const chord = CHORDS[k % CHORDS.length];
    chord.forEach((fret, s) => {
      if (fret < 0) return;
      const midi = STRINGS[s] + fret;
      const t0 = k * STRUM_S + 0.05 + 0.015 * s;
      played.push({ t: t0, midi });
      const f = 440 * 2 ** ((midi - 69) / 12);
      const len = Math.round(sr / f);
      const line = new Float32Array(len);
      let lp = 0;
      for (let i = 0; i < len; i++) { lp += 0.5 * (rnd() - lp); line[i] = lp; }
      const start = Math.round(t0 * sr);
      const end = Math.min(n, Math.round((k + 1) * STRUM_S * sr + 0.05 * sr));
      let prev = 0;
      for (let i = start, j = 0; i < end; i++, j = (j + 1) % len) {
        const x = line[j];
        out[i] += 0.12 * x;
        line[j] = 0.996 * 0.5 * (x + prev);
        prev = x;
      }
    });
  }
  return { audio: out, played };
}

