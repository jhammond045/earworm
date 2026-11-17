// The delay page's engine test, run offline through the committed
// engine.wasm with a crude speaker-in-a-room model of the hum. Before
// ENGINE_TEST_PARAMS set Level to its minimum, the engine's bleed check muted
// every other burst here, as in the owner's run (3 of 6 bursts found).
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { OfflineEngine } from '../host/engine-host';
import { MSG_SET_PARAM, P } from '../host/protocol';
import { ENGINE_TEST_PARAMS, analyseEngineCapture, synthHum } from './measure';

const SR = 48000;
const bytes = readFileSync(new URL('../../public/wasm/engine.wasm', import.meta.url));

/** Hum at -26 dB plus sparse decaying reflections (deterministic). */
function roomHum(): Float32Array {
  const dry = synthHum(SR).samples.map((v) => v * 0.05);
  const y = Float32Array.from(dry);
  let seed = 1;
  const rnd = () => ((seed = (seed * 1664525 + 1013904223) >>> 0) / 2 ** 32);
  const n = Math.round(0.3 * SR);
  for (let k = Math.round(0.003 * SR); k < n; k++) {
    const g = (rnd() * 2 - 1) * Math.exp((-6.9 * k) / n);
    if (rnd() >= 0.02) continue;
    for (let i = k; i < dry.length; i++) y[i] += 0.3 * g * dry[i - k];
  }
  return y;
}

function run(params: [string, number][]) {
  const eng = OfflineEngine.fromBytes(bytes, SR, 16);
  const r = eng.processLive(roomHum(), params.map(([name, v]) => [MSG_SET_PARAM, 0, P[name], v]));
  eng.dispose();
  return { ...r, bursts: analyseEngineCapture(r.mic, r.out, SR) };
}

describe('engine delay test', () => {
  it('finds every burst and a pitch lock near the native figure', () => {
    const { bursts, bleedSamples } = run(ENGINE_TEST_PARAMS);
    expect(bleedSamples).toBe(0);
    expect(bursts.length).toBe(6);
    for (const b of bursts) {
      expect(b.pitchDelayMs).not.toBeNull();
      expect(b.pitchDelayMs!).toBeLessThan(35);
      expect(Math.abs(b.levelDelayMs!)).toBeLessThan(5);
    }
  });

  it('at the default Level the bleed check mutes the engine', () => {
    const { bleedSamples } = run(ENGINE_TEST_PARAMS.filter(([n]) => n !== 'level'));
    expect(bleedSamples).toBeGreaterThan(0);
  });
});
