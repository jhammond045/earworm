// Matches two synthetic sounds with analysis.wasm and renders them through klont.wasm
// with the same job the blind test page builds (planMatchRender).
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { instantiateEngine } from '../m1/protocol';
import { renderSong } from '../m1/render-core';
import { MatchCore, type Match } from './match';
import { C4_CENTS, RENDER_RATE, matchLayerBlob, planMatchRender, toMono } from './test-render';
import { LAYER_HEADER_WORDS, LAYER_INFLUENCE_AMOUNT, LAYER_OFF_FIELDS, N_MACROS, layerWords } from '../m1/protocol';
import { DESCS } from '../gen/params';
import { classicPatch, vocabKeys } from '../synth/classic';
import { layoutOf } from '../synth/patch';

const SR = 48000;
const pub = (f: string) => readFileSync(new URL(`../../public/wasm/${f}`, import.meta.url));
const analysis = MatchCore.fromBytes(pub('analysis.wasm'));
const klont = pub('klont.wasm');
const cents = (hz: number) => 1200 * Math.log2(hz / 16.351598);

function pluck(hz: number, secs: number): Float32Array {
  const x = new Float32Array(Math.round(secs * SR));
  for (let i = 0; i < x.length; i++) {
    const t = i / SR;
    let v = 0;
    for (let k = 1; k <= 8; k++) v += (Math.sin(2 * Math.PI * hz * k * t) / k) * Math.exp(-t * (2 + k));
    x[i] = 0.4 * v * Math.min(1, t / 0.003);
  }
  return x;
}

function noiseBurst(secs: number): Float32Array {
  let s = 12345;
  const x = new Float32Array(Math.round(secs * SR));
  for (let i = 0; i < x.length; i++) {
    s = (Math.imul(s, 1664525) + 1013904223) >>> 0;
    const t = i / SR;
    x[i] = 0.5 * (s / 2 ** 32 - 0.5) * Math.exp(-t * 6);
  }
  return x;
}

/** Autocorrelation pitch over `len` samples from `from`, searched 100 to 800 Hz. */
function pitchHz(x: Float32Array, from: number, len: number): number {
  let best = 0, bestLag = 0;
  const lo = Math.floor(SR / 800), hi = Math.floor(SR / 100);
  for (let lag = lo; lag <= hi; lag++) {
    let sum = 0;
    for (let i = from; i < from + len; i++) sum += x[i] * x[i + lag];
    if (sum > best) { best = sum; bestLag = lag; }
  }
  return SR / bestLag;
}

const rms = (x: Float32Array) => Math.sqrt(x.reduce((a, v) => a + v * v, 0) / Math.max(1, x.length));

async function renderOf(signal: Float32Array, m: Match) {
  const plan = planMatchRender({ signal, sampleRate: SR }, m);
  const { ex } = instantiateEngine(klont, RENDER_RATE, 128);
  const r = await renderSong(ex, plan.song, plan.options);
  return { plan, r, mono: toMono(r.left, r.right) };
}

describe('match layer blob', () => {
  it('holds the target in the Influence slot at amount 1', () => {
    const t = Float32Array.from({ length: 96 }, (_, i) => i / 100);
    const b = matchLayerBlob(t);
    const lay = layoutOf(classicPatch('melody'));
    const n = lay.keys.length;
    expect(b.byteLength).toBe(layerWords(n) * 4);
    const f = new Float32Array(b);
    expect(f[LAYER_OFF_FIELDS + LAYER_INFLUENCE_AMOUNT]).toBe(1);
    const target = f.subarray(LAYER_HEADER_WORDS + n + N_MACROS, LAYER_HEADER_WORDS + 2 * n + N_MACROS);
    // Each old parameter's target sits in every value it is bound to.
    DESCS.forEach((d, i) => {
      for (const k of vocabKeys('melody', 0, d.id)) expect(target[lay.index.get(k)!], d.id).toBe(Math.fround(t[i]));
    });
  });
  it('rejects a wrong length', () => expect(() => matchLayerBlob(new Float32Array(5))).toThrow(RangeError));
});

describe('blind test render', () => {
  it('renders a plucked string near its pitch, at the source length plus release', async () => {
    const src = pluck(220, 1.2);
    const m = analysis.match(src, SR);
    expect(m.pitched).toBe(true);
    const { plan, r, mono } = await renderOf(src, m);
    expect(plan.cents).toBe(m.pitchCents);
    expect(r.frames).toBe(plan.frames);
    expect(r.frames / RENDER_RATE).toBeCloseTo(1.2 + 1, 2);
    expect(mono.every(Number.isFinite)).toBe(true);
    expect(r.peak).toBeGreaterThan(0.01);
    expect(rms(mono.subarray(0, SR / 2))).toBeGreaterThan(1e-3);
    // Pitch of the first part of the note, after the attack, within a quarter tone of the source.
    const got = pitchHz(mono, Math.round(0.08 * SR), 4096);
    expect(Math.abs(cents(got) - cents(220))).toBeLessThan(50);
  }, 60_000);

  it('renders a noise burst, unpitched at C4, with the grain source when the match uses it', async () => {
    const src = noiseBurst(0.6);
    const m = analysis.match(src, SR);
    expect(m.pitched).toBe(false);
    const { plan, r, mono } = await renderOf(src, m);
    expect(plan.cents).toBe(C4_CENTS);
    expect(plan.grain).toBe(m.useSound && m.grain !== null);
    expect(plan.grain).toBe(true); // unpitched sources default to use_sound on
    expect(plan.song.samples).toHaveLength(1);
    expect(r.frames).toBe(Math.round(1.6 * RENDER_RATE));
    expect(mono.every(Number.isFinite)).toBe(true);
    expect(r.peak).toBeGreaterThan(0.01);
    expect(rms(mono.subarray(0, SR / 4))).toBeGreaterThan(1e-3);
  }, 60_000);

  it('leaves the grain out when use_sound is off', async () => {
    const src = noiseBurst(0.6);
    const m = analysis.match(src, SR, { useSound: false });
    const { plan, r } = await renderOf(src, m);
    expect(plan.grain).toBe(false);
    expect(plan.song.samples).toBeUndefined();
    expect(r.peak).toBeGreaterThan(0.001);
  }, 60_000);
});
