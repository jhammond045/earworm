// The delay page's engine test, run through the committed klont.wasm (the M2
// engine) with the same room-hum input as engine-delay.test.ts.
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { MSG_BYTES, encodeMessage, instantiateEngine, readBufferTable } from '../m1/protocol';
import { KLONT_TEST_SETTINGS, analyseEngineCapture, roomHum } from './measure';

const SR = 48000;
const BLOCK = 128;
const bytes = readFileSync(new URL('../../public/wasm/klont.wasm', import.meta.url));

function run() {
  const { ex, e } = instantiateEngine(bytes, SR, BLOCK);
  const t = readBufferTable(ex.memory.buffer, ex.ew_buffers(e));
  const n = new Uint32Array(ex.memory.buffer, t.controlCount, 1);
  for (const m of KLONT_TEST_SETTINGS) {
    encodeMessage(new DataView(ex.memory.buffer, t.control), n[0] * MSG_BYTES, m[0], m[1], m[2], m[3], m[4], 0);
    n[0]++;
  }
  const input = roomHum(SR);
  const mic = new Float32Array(input.length);
  const out = new Float32Array(input.length);
  for (let o = 0; o < input.length; o += BLOCK) {
    const m = Math.min(BLOCK, input.length - o);
    new Float32Array(ex.memory.buffer, t.input, BLOCK).set(input.subarray(o, o + m));
    ex.ew_process(e, m);
    // The engine high-passes the input in place.
    mic.set(new Float32Array(ex.memory.buffer, t.input, m), o);
    out.set(new Float32Array(ex.memory.buffer, t.outputL, m), o);
  }
  ex.ew_free(e);
  return { mic, out, bursts: analyseEngineCapture(mic, out, SR, { clarity: 0.8 }) };
}

describe('klont delay test (wasm)', () => {
  it('finds every burst and a pitch lock under 35 ms at -60 dB layer level', () => {
    const { bursts, out } = run();
    let peak = 0;
    for (const v of out) peak = Math.max(peak, Math.abs(v));
    console.log(`klont delay: output peak ${peak.toExponential(2)}`);
    bursts.forEach((b, i) => console.log(
      `klont delay burst ${i + 1}: mic onset ${b.micOnsetS.toFixed(3)} s, f0 ${b.inputF0?.toFixed(1)} Hz, ` +
      `level ${b.levelDelayMs?.toFixed(2)} ms, pitch ${b.pitchDelayMs?.toFixed(1)} ms`));
    expect(bursts.length).toBe(6);
    for (const b of bursts) {
      expect(b.pitchDelayMs).not.toBeNull();
      expect(b.pitchDelayMs!).toBeLessThan(35);
      expect(Math.abs(b.levelDelayMs!)).toBeLessThan(5);
    }
  });

});
