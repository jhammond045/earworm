// Poly takes (docs/poly.md, stage P3): what a layer armed in Poly captures. The take's notes come
// from the note model in the analysis worker (crates/analysis/src/poly), each with its own curve;
// this file turns the layer's capture setting into the pass's options.

import { classicPatch } from '../synth/classic';
import { layerBudget, voiceCount } from '../synth/patch';
import { DEFAULT_CHANNELS, MAX_CHANNELS, type CaptureMode, type Instrument, type Layer } from '../project/types';
import type { PolyOptions } from '../m2/analysis';

let budget = 0;
const layerBudgetNow = () => budget || (budget = layerBudget([classicPatch('melody'), classicPatch('beat')]));

/** The layer's capture setting; mono when it has none. */
export function captureOf(l: Layer): CaptureMode {
  return l.capture ?? { mode: 'mono', instrument: 'guitar', channels: DEFAULT_CHANNELS.guitar };
}

/** Does a take into this layer come back as a poly take. Beat layers keep their hits. */
export function isPoly(l: Layer): boolean {
  return l.kind !== 'beat' && captureOf(l).mode === 'poly';
}

/** The voices the layer's patch may sound at once. */
export function patchVoices(l: Layer): number {
  return voiceCount(l.patch, layerBudgetNow());
}

/** Capture channels a poly take keeps: the setting, 1..8, never more than the patch's voices. */
export function polyChannels(l: Layer): number {
  const c = captureOf(l);
  const want = Math.round(c.channels) || DEFAULT_CHANNELS[c.instrument];
  return Math.max(1, Math.min(MAX_CHANNELS, want, patchVoices(l)));
}

export function polyOptions(l: Layer): PolyOptions {
  return { instrument: captureOf(l).instrument, channels: polyChannels(l) };
}

/** A capture setting with the instrument changed: the channel count follows the instrument's
 *  default unless it had been set away from the old one's. */
export function withInstrument(c: CaptureMode, instrument: Instrument): CaptureMode {
  const channels = c.channels === DEFAULT_CHANNELS[c.instrument] ? DEFAULT_CHANNELS[instrument] : c.channels;
  return { ...c, instrument, channels };
}
