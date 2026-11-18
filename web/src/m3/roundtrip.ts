// Round trip measurement for the capture page: five chirps out of the speakers (or headphones),
// recorded by the mic, the median cross-correlation lag. Same method as delay.html (the TDD's
// "Devices and calibration"), run on the page's own audio context and mic stream.

import loopbackUrl from '../delay/loopback-processor.ts?worker&url';
import { chirp, crossCorrelationLag, median } from '../delay/measure';

const CLICKS = 5;
const SPACING_S = 0.4;
const LEAD_S = 0.5;
const SEARCH_S = 0.3;
const MIN_NORM = 0.2; // below this a chirp was not really found

const loaded = new WeakSet<BaseAudioContext>();

export interface RoundTripResult { medianMs: number | null; lagsMs: (number | null)[]; found: number; error?: string }

export async function measureRoundTrip(ctx: AudioContext, stream: MediaStream): Promise<RoundTripResult> {
  if (ctx.state !== 'running') await ctx.resume();
  if (!loaded.has(ctx)) { await ctx.audioWorklet.addModule(loopbackUrl); loaded.add(ctx); }
  const sr = ctx.sampleRate;
  const click = chirp(sr);
  const spacing = Math.round(SPACING_S * sr);
  const lead = Math.round(LEAD_S * sr);
  const total = lead + CLICKS * spacing + Math.round(SEARCH_S * sr) + click.length;
  const node = new AudioWorkletNode(ctx, 'loopback', {
    numberOfInputs: 1, numberOfOutputs: 1, outputChannelCount: [1],
    processorOptions: { click, spacing, count: CLICKS, lead, total },
  });
  const src = ctx.createMediaStreamSource(stream);
  const done = new Promise<Float32Array>((resolve) => { node.port.onmessage = (e) => resolve(e.data.rec as Float32Array); });
  src.connect(node);
  node.connect(ctx.destination);
  const rec = await done;
  src.disconnect();
  node.disconnect();

  const maxLag = Math.round(SEARCH_S * sr);
  const lagsMs: (number | null)[] = [];
  const good: number[] = [];
  for (let k = 0; k < CLICKS; k++) {
    const r = crossCorrelationLag(rec, click, maxLag, lead + k * spacing);
    if (r.norm >= MIN_NORM && Number.isFinite(r.lag)) { const v = (r.lag / sr) * 1000; good.push(v); lagsMs.push(v); } else lagsMs.push(null);
  }
  if (!good.length) {
    return { medianMs: null, lagsMs, found: 0, error: 'No chirp found in the recording. Turn the output up, or give the mic a path from the output.' };
  }
  return { medianMs: median(good), lagsMs, found: good.length };
}
