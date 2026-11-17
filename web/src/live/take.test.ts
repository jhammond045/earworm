// Record mode's playback path: a take rendered through OfflineEngine.processLive
// with the page's settings. Same take, same settings, same sound; a changed
// setting changes it.
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { OfflineEngine } from '../host/engine-host';
import { MSG_SET_PARAM, PARAMS } from '../host/protocol';
import { medianF0, synthHum } from '../delay/measure';

const SR = 48000;
const bytes = readFileSync(new URL('../../public/wasm/engine.wasm', import.meta.url));
const take = synthHum(SR, 220, 2).samples.map((v) => v * 0.3);

function render(over: Record<string, number> = {}) {
  const settings: Record<string, number> = {};
  for (const d of PARAMS) settings[d.name] = d.default;
  Object.assign(settings, over);
  const eng = OfflineEngine.fromBytes(bytes, SR, 16);
  const r = eng.processLive(take, PARAMS.map((d, i): [number, number, number, number] => [MSG_SET_PARAM, 0, i, settings[d.name]]));
  eng.dispose();
  return r.out;
}

describe('take playback', () => {
  it('renders the same take the same way twice', () => {
    expect(render()).toEqual(render());
  });

  it('follows the take and re-renders with new settings', () => {
    const mid = (x: Float32Array) => medianF0(x, SR, Math.round(0.5 * SR), Math.round(0.8 * SR));
    const base = render();
    expect(Math.abs(mid(base) - 220)).toBeLessThan(3);
    const up = render({ 'tune.octave': 1 });
    expect(Math.abs(mid(up) - 440)).toBeLessThan(6);
  });
});
