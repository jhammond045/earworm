// Live follow through the committed klont.wasm: a synthetic hum goes into
// ew_process in 128-sample blocks with layer 0 armed, and the output must
// follow its level and pitch within the budget. Mirrors
// crates/klont/tests/live_follow.rs.
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import * as pr from './protocol';
import { encodeMessage, instantiateEngine, readBufferTable } from './protocol';

const SR = 48000;
const BLOCK = 128;
const STRIDE = 25;
const bytes = readFileSync(new URL('../../public/wasm/klont.wasm', import.meta.url));

/** Harmonic voice with two formant bells, a stand-in for a sung "ah". */
function hum(hz: number, n: number, amp: number): Float32Array {
  const out = new Float32Array(n);
  const kmax = Math.floor((SR * 0.45) / hz);
  for (let k = 1; k <= kmax; k++) {
    const f = hz * k;
    const bell = (fc: number, bw: number) => Math.exp(-((f - fc) ** 2) / (2 * bw * bw));
    const a = (1 / k) * (0.2 + bell(700, 150) + 0.6 * bell(1100, 200));
    const w = (2 * Math.PI * f) / SR;
    for (let i = 0; i < n; i++) out[i] += a * Math.sin(w * i);
  }
  let peak = 0;
  for (const v of out) peak = Math.max(peak, Math.abs(v));
  for (let i = 0; i < n; i++) out[i] *= amp / peak;
  return out;
}

/** Autocorrelation f0, 70..1000 Hz; 0 for silence or no clear pitch. */
function f0(x: Float32Array): number {
  let energy = 0;
  for (const v of x) energy += v * v;
  if (energy < 1e-6) return 0;
  const minLag = Math.floor(SR / 1000), maxLag = Math.min(Math.floor(SR / 70), x.length >> 1);
  let bestLag = 0, best = 0;
  for (let lag = minLag; lag < maxLag; lag++) {
    let c = 0;
    for (let i = 0; i < x.length - lag; i++) c += x[i] * x[i + lag];
    c /= energy;
    if (c > best) { best = c; bestLag = lag; }
  }
  return best < 0.5 ? 0 : SR / bestLag;
}

const cents = (a: number, b: number) => 1200 * Math.log2(a / b);

function makeEngine() {
  const { ex, e } = instantiateEngine(bytes, SR, BLOCK);
  const t = readBufferTable(ex.memory.buffer, ex.ew_buffers(e));
  const send = (type: number, layer: number, index: number, param: number, value: number) => {
    const n = new Uint32Array(ex.memory.buffer, t.controlCount, 1);
    const dv = new DataView(ex.memory.buffer, t.control);
    encodeMessage(dv, n[0] * pr.MSG_BYTES, type, layer, index, param, value, 0);
    n[0]++;
  };
  /** Run `input` through ew_process in blocks; returns the output and frame records. */
  const run = (input: Float32Array) => {
    const out = new Float32Array(input.length);
    const frames: number[] = [];
    for (let o = 0; o < input.length; o += BLOCK) {
      const m = Math.min(BLOCK, input.length - o);
      new Float32Array(ex.memory.buffer, t.input, BLOCK).set(input.subarray(o, o + m));
      ex.ew_process(e, m);
      out.set(new Float32Array(ex.memory.buffer, t.outputL, m), o);
      const nf = new Uint32Array(ex.memory.buffer, t.framesCount, 1)[0];
      frames.push(...new Float32Array(ex.memory.buffer, t.frames, nf * STRIDE));
    }
    return { out, frames: Float32Array.from(frames) };
  };
  const meter = () => new Float32Array(ex.memory.buffer, t.meter, pr.METER_LEN);
  return { send, run, meter, t };
}

describe('klont Live follow (wasm)', () => {
  it('lays out the frame buffer and meters', () => {
    const { t, meter } = makeEngine();
    expect(t.framesCap).toBe(Math.ceil(BLOCK / (SR / 100)) + 2);
    expect(meter()[pr.M_ARMED]).toBe(-1);
    expect(meter()[pr.M_LIVE_SLOT]).toBe(-1);
  });

  it('follows level and pitch of a hum on the armed layer', () => {
    const { send, run, meter } = makeEngine();
    send(pr.MSG_ARM, 0, 0, 0, 1);
    const sil = Math.round(0.5 * SR), note = Math.round(0.6 * SR);
    const input = new Float32Array(sil * 3 + note * 2);
    input.set(hum(220, note, 0.3), sil);
    input.set(hum(330, note, 0.3), sil * 2 + note);
    const { out, frames } = run(input);
    expect(out.every((v) => Number.isFinite(v) && Math.abs(v) <= 1)).toBe(true);
    expect(frames.length / STRIDE).toBe(Math.floor(input.length / 480));

    const win = Math.round(0.02 * SR), hop = Math.round(0.002 * SR);
    const level: number[] = [], pitch: number[] = [];
    for (const [i, hz] of [[0, 220], [1, 330]] as const) {
      const start = sil + i * (note + sil);
      let on = start - 1000;
      while (on < out.length && Math.abs(out[on]) <= 0.01) on++;
      level.push(((on - start) / SR) * 1000);
      let t = start;
      const ok = (at: number) => { const e = f0(out.subarray(at, at + win)); return e > 0 && Math.abs(cents(e, hz)) < 50; };
      while (!(ok(t) && ok(t + win) && ok(t + 2 * win))) {
        t += hop;
        if (t + 4 * win >= start + note) throw new Error(`never locked to ${hz} Hz`);
      }
      pitch.push(((t - start) / SR) * 1000);
      const mid = start + (note >> 1);
      expect(Math.abs(cents(f0(out.subarray(mid, mid + win)), hz))).toBeLessThan(20);
    }
    console.log(`wasm live follow: level delay ${level.map((v) => v.toFixed(2))} ms, pitch lock ${pitch.map((v) => v.toFixed(1))} ms`);
    for (const l of level) expect(l).toBeLessThanOrEqual(3);
    for (const p of pitch) expect(p).toBeLessThanOrEqual(40);
    // The gate closes: silence after the second note.
    const tail = out.subarray(out.length - sil / 2);
    expect(tail.every((v) => Math.abs(v) < 1e-3)).toBe(true);
    expect(meter()[pr.M_ARMED]).toBe(0);
  });

  it('is silent when nothing is armed', () => {
    const { run, meter } = makeEngine();
    const input = new Float32Array(SR);
    input.set(hum(220, SR / 2, 0.3), SR / 4);
    const { out, frames } = run(input);
    expect(out.every((v) => Math.abs(v) < 1e-6)).toBe(true);
    expect(frames.length / STRIDE).toBeGreaterThan(50);
    expect(meter()[pr.M_GATE]).toBeGreaterThanOrEqual(0);
  });
});
