// The delay page's engine test, run offline through the committed
// engine.wasm with a crude speaker-in-a-room model of the hum.
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { OfflineEngine } from '../host/engine-host';
import { MSG_SET_PARAM, P } from '../host/protocol';
import { ENGINE_TEST_PARAMS, analyseEngineCapture, roomHum } from './measure';

const SR = 48000;
const bytes = readFileSync(new URL('../../public/wasm/engine.wasm', import.meta.url));

function run(params: [string, number][]) {
  const eng = OfflineEngine.fromBytes(bytes, SR, 16);
  const r = eng.processLive(roomHum(SR), params.map(([name, v]) => [MSG_SET_PARAM, 0, P[name], v]));
  eng.dispose();
  return { ...r, bursts: analyseEngineCapture(r.mic, r.out, SR) };
}

describe('engine delay test', () => {
  it('finds every burst and a pitch lock near the native figure', () => {
    const { bursts } = run(ENGINE_TEST_PARAMS);
    expect(bursts.length).toBe(6);
    for (const b of bursts) {
      expect(b.pitchDelayMs).not.toBeNull();
      expect(b.pitchDelayMs!).toBeLessThan(35);
      expect(Math.abs(b.levelDelayMs!)).toBeLessThan(5);
    }
  });
});
