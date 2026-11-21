// M4 through the committed klont.wasm: a grain source on a layer, offline
// through ew_render, and states with a morph lane. Mirrors
// crates/klont/tests/m4.rs on the native side.
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { classicPatch } from '../synth/classic';
import { layoutOf } from '../synth/patch';
import { encodeSample, encodeStates } from './blobs';
import { encodeLayer } from './layer';
import * as pr from './protocol';
import { Core } from './render-core';
import { MORPH_LINE, MORPH_STEP, encodeTrack, type TrackMorph } from './track';

const SR = 48000;
const BLOCK = 480;
const wasm = readFileSync(new URL('../../public/wasm/klont.wasm', import.meta.url));
const A4 = 5700;

function makeCore(): Core {
  const ex = new WebAssembly.Instance(new WebAssembly.Module(wasm), {}).exports as unknown as pr.EngineExports;
  return new Core(ex, SR, BLOCK);
}

function sine(hz: number, secs: number, amp: number): Float32Array {
  return Float32Array.from({ length: Math.round(secs * SR) }, (_, i) => amp * Math.sin((2 * Math.PI * hz * i) / SR));
}

function render(core: Core, frames: number): Float32Array {
  const out = new Float32Array(frames);
  for (let o = 0; o < frames; o += BLOCK) {
    const n = Math.min(BLOCK, frames - o);
    core.render(n);
    out.set(core.out(n).l, o);
  }
  return out;
}

const rms = (x: Float32Array) => Math.sqrt(x.reduce((s, v) => s + v * v, 0) / Math.max(1, x.length));

function noteTrack(morph?: TrackMorph[]) {
  return encodeTrack({
    sections: [{ feel: 'grid', length: 3840, bpm: 120 }],
    clips: [{ section: 0, layer: 0, loopLen: 3840, events: [{ t: 0, dur: 1920, cents: A4 }], morph }],
  });
}

/** A layer whose only sound is the grain cloud, at the given grain mix. */
function grainRender(mix: number, source: Float32Array | null): Float32Array {
  const core = makeCore();
  try {
    const layer = encodeLayer({ kind: 'melody', base: { 'src.harm_mix': 0, 'src.noise_mix': 0, 'src.grain_mix': mix } }, 0);
    expect(core.load(pr.LOAD_PATCH, 0, layer.patch)).toBe(pr.LOAD_OK);
    expect(core.load(pr.LOAD_LAYER, 0, layer.layer)).toBe(pr.LOAD_OK);
    if (source) expect(core.load(pr.LOAD_SAMPLE, 0, encodeSample(SR, A4, source))).toBe(pr.LOAD_OK);
    render(core, SR / 2); // let the 15 ms parameter glides settle
    expect(core.load(pr.LOAD_TRACK, 0, noteTrack())).toBe(pr.LOAD_OK);
    core.send([[pr.MSG_PLAY, 0, 0, 0, 0]]);
    return render(core, SR);
  } finally {
    core.free();
  }
}

describe('klont grain cloud (wasm)', () => {
  it('is silent with no source or no mix, and sounds with both', () => {
    const src = sine(440, 1, 0.5);
    expect(grainRender(1, null).every((v) => v === 0)).toBe(true);
    expect(grainRender(0, src).every((v) => v === 0)).toBe(true);
    const full = grainRender(1, src);
    expect(rms(full.subarray(9600))).toBeGreaterThan(0.05);
    expect(full.every((v) => Number.isFinite(v) && Math.abs(v) <= 1)).toBe(true);
  });

  it('follows src.grain_mix', () => {
    const src = sine(440, 1, 0.5);
    const half = rms(grainRender(0.5, src).subarray(9600));
    const full = rms(grainRender(1, src).subarray(9600));
    expect(half).toBeGreaterThan(0.01);
    expect(full / half).toBeGreaterThan(1.7);
    expect(full / half).toBeLessThan(2.3);
  });

  it('renders the same twice', () => {
    const src = sine(440, 1, 0.5);
    expect(grainRender(1, src)).toEqual(grainRender(1, src));
  });

  it('rejects bad sample blobs and takes a long source', () => {
    const core = makeCore();
    const good = encodeSample(SR, A4, sine(440, 0.1, 0.5));
    expect(core.load(pr.LOAD_SAMPLE, pr.MAX_SAMPLES, good)).toBe(pr.LOAD_BAD_SLOT);
    expect(core.load(pr.LOAD_SAMPLE, 0, good.slice(0, good.byteLength - 4))).toBe(pr.LOAD_MALFORMED);
    expect(core.load(pr.LOAD_SAMPLE, 0, new ArrayBuffer(16))).toBe(pr.LOAD_BAD_MAGIC);
    expect(core.load(pr.LOAD_SAMPLE, 0, encodeSample(SR, A4, new Float32Array(1_000_000)))).toBe(pr.LOAD_OK);
    core.free();
  });
});

describe('klont states and morph (wasm)', () => {
  // Every engine layer starts on the classic melody patch.
  const lay = layoutOf(classicPatch('melody'));
  const CUT = lay.index.get('filter1.cutoff')!;
  function states(cutoffs: number[]) {
    return encodeStates(
      lay.keys.length,
      cutoffs.map((c) => {
        const values = lay.init.slice();
        values[CUT] = c;
        return { values, macros: new Array(8).fill(0.5), influenceAmount: 0 };
      }),
    );
  }

  it('loads a states blob and a v2 track, and the stored cutoff follows the lane', () => {
    const core = makeCore();
    try {
      expect(core.load(pr.LOAD_LAYER, 0, encodeLayer({ kind: 'melody' }).layer)).toBe(pr.LOAD_OK);
      expect(core.load(pr.LOAD_STATES, 0, states([0.2, 0.8]))).toBe(pr.LOAD_OK);
      const markers: TrackMorph[] = [
        { t: 0, state: 0, lenMs: 0, shape: MORPH_LINE },
        { t: 1920, state: 1, lenMs: 500, shape: MORPH_LINE },
      ];
      const track = noteTrack(markers);
      expect(new Uint32Array(track)[0]).toBe(0x32545745);
      expect(core.load(pr.LOAD_TRACK, 0, track)).toBe(pr.LOAD_OK);
      expect(core.meter()[pr.M_LOAD_STATUS]).toBe(pr.LOAD_OK);
      core.send([[pr.MSG_PLAY, 0, 0, 0, 0]]);
      const cutoff: number[] = [];
      for (let i = 0; i < 190; i++) {
        core.render(BLOCK);
        cutoff.push(core.meter()[pr.M_STORED + CUT]);
      }
      expect(Math.abs(cutoff[60] - 0.2)).toBeLessThan(1e-4);
      expect(cutoff[125]).toBeGreaterThan(0.4);
      expect(cutoff[125]).toBeLessThan(0.6);
      expect(Math.abs(cutoff[189] - 0.8)).toBeLessThan(1e-4);
    } finally {
      core.free();
    }
  });

  it('a step marker switches at once, and a bad v2 track keeps the previous one', () => {
    const core = makeCore();
    try {
      expect(core.load(pr.LOAD_STATES, 0, states([0.2, 0.8]))).toBe(pr.LOAD_OK);
      const markers: TrackMorph[] = [
        { t: 0, state: 0, lenMs: 0, shape: MORPH_LINE },
        { t: 1920, state: 1, lenMs: 500, shape: MORPH_STEP },
      ];
      expect(core.load(pr.LOAD_TRACK, 0, noteTrack(markers))).toBe(pr.LOAD_OK);
      const bad = noteTrack(markers);
      new Uint32Array(bad)[8 + 8 + 12 + 16 + 4 + 1] = 9; // second marker's state, past MAX_STATES
      expect(core.load(pr.LOAD_TRACK, 0, bad)).toBe(pr.LOAD_MALFORMED);
      core.send([[pr.MSG_PLAY, 0, 0, 0, 0]]);
      const cutoff: number[] = [];
      for (let i = 0; i < 150; i++) {
        core.render(BLOCK);
        cutoff.push(core.meter()[pr.M_STORED + CUT]);
      }
      expect(Math.abs(cutoff[98] - 0.2)).toBeLessThan(1e-4);
      expect(cutoff[105]).toBeGreaterThan(0.7);
      expect(Math.abs(cutoff[140] - 0.8)).toBeLessThan(1e-4);
    } finally {
      core.free();
    }
  });

  it('rejects bad states blobs', () => {
    const core = makeCore();
    const good = states([0.5]);
    expect(core.load(pr.LOAD_STATES, 8, good)).toBe(pr.LOAD_BAD_SLOT);
    expect(core.load(pr.LOAD_STATES, 0, good.slice(0, good.byteLength - 4))).toBe(pr.LOAD_MALFORMED);
    expect(core.load(pr.LOAD_STATES, 0, new ArrayBuffer(16))).toBe(pr.LOAD_BAD_MAGIC);
    core.free();
  });
});
