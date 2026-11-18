import { describe, expect, it } from 'vitest';
import { COUNT } from '../gen/params';
import * as pr from '../m1/protocol';
import { Compiler, layerBlob, sectionLength, sectionUnitsPerSec, type Upload } from './compile';
import type { BeatClip, Layer, MelodyClip, Project, Section, Take } from './types';

const tune = { snap: 1, speedMs: 0, slides: 0 as const, vibrato: 1, levelOut: 0, root: 0, scale: 0, refCents: 5700, octave: 0 };
const shape = { attack: 0.002, burst: 0.5, cents: null, bend: 0, hold: 0.03, vowel: 0, tone: 0.5, endStop: 0, endClick: 0 };

function layer(id: string, kind: Layer['kind'] = 'melody', extra: Partial<Layer> = {}): Layer {
  return {
    id, name: id, kind, color: 0xffffff, level: -3, pan: 0.25, mute: false,
    states: [{ id: 's', name: 's', base: Array.from({ length: COUNT }, (_, i) => i / 100), macros: [0.1, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.9], influenceAmount: 0.7 }],
    tune: { ...tune }, depth: { loud: 0.9, open: 0.4, hard: 0.3, breath: 0.2, mouth: 0.8 }, follow: 0.6,
    kit: kind === 'beat' ? [0, 1, 2, 3].map((k) => ({ params: Array.from({ length: COUNT }, () => 0.1 * (k + 1)), cents: 1000 * (k + 1) })) : [],
    ...extra,
  };
}

const take = (id: string): Take => ({ id, audio: `a-${id}`, frames: `f-${id}`, rate: 48000, latencyMs: 0, frameCount: 100, kind: 'melody',
  refCents: 5700, root: 0, scale: 0, bpm: null });

function melodyClip(takeId = '', over: Partial<MelodyClip> = {}): MelodyClip {
  return { kind: 'melody', take: takeId, takes: takeId ? [takeId] : [], loopLen: 3840, timeSnap: 0,
    notes: [{ id: 'n1', t: 0, dur: 480, cents: 5700, hard: 0.5 }, { id: 'n2', t: 960, dur: 960, cents: 6000, hard: 0.2 }],
    warp: [{ takeSec: 0, clipT: 0 }, { takeSec: 0.25, clipT: 480 }], ...over };
}

function beatClip(): BeatClip {
  return { kind: 'beat', take: '', takes: [], loopLen: 3840, timeSnap: 0,
    hits: [{ id: 'h1', t: 0, dur: 100, slot: 1, strength: 0.8, shape: { ...shape, cents: 4800, endStop: 1 } }] };
}

function project(): Project {
  const grid: Section = { id: 'sa', name: 'A', feel: 'grid', beats: 4, seconds: 0, bpm: 120, repeats: 2, rampMs: 50, clips: { m: melodyClip('t1'), b: beatClip() } };
  const free: Section = { id: 'sb', name: 'B', feel: 'free', beats: 0, seconds: 5.5, bpm: 0, repeats: 1, rampMs: 0, clips: {} };
  return {
    schema: 1, id: 'p', name: 'p', sections: [grid, free], layers: [layer('m', 'melody', { tune: { ...tune, octave: 1 } }), layer('b', 'beat')],
    takes: { t1: take('t1') }, loop: true,
  };
}

const frames = (n = 3) => new Float32Array(n * 25).fill(1);

function words(b: ArrayBuffer) { return { u: new Uint32Array(b), f: new Float32Array(b) }; }
const kinds = (u: Upload[]) => u.map((x) => `${x.kind}:${x.slot}`);

describe('helpers', () => {
  it('section units', () => {
    const p = project();
    expect(sectionUnitsPerSec(p.sections[0])).toBe(1920);
    expect(sectionLength(p.sections[0])).toBe(3840);
    expect(sectionUnitsPerSec(p.sections[1])).toBe(1);
    expect(sectionLength(p.sections[1])).toBe(5.5);
  });
});

describe('blobs', () => {
  it('layer blob carries the normalized sets and fields', () => {
    const l = layer('b', 'beat', { mute: true });
    const { u, f } = words(layerBlob(l));
    expect(u.length).toBe(pr.LAYER_WORDS);
    expect(u[0]).toBe(pr.LAYER_MAGIC);
    const fl = (k: number) => f[pr.LAYER_OFF_FIELDS + k];
    expect(fl(pr.LAYER_LEVEL)).toBe(-3);
    expect(fl(pr.LAYER_PAN)).toBeCloseTo(0.25);
    expect(fl(pr.LAYER_MUTE)).toBe(1);
    expect(fl(pr.LAYER_KIND)).toBe(pr.KIND_BEAT);
    expect(fl(pr.LAYER_DEPTH_LOUD)).toBeCloseTo(0.9);
    expect(fl(pr.LAYER_DEPTH_MOUTH)).toBeCloseTo(0.8);
    expect(fl(pr.LAYER_FOLLOW)).toBeCloseTo(0.6);
    expect(fl(pr.LAYER_INFLUENCE_AMOUNT)).toBe(0);
    expect(fl(pr.LAYER_KIT_CENTS + 2)).toBe(3000);
    expect(f[pr.LAYER_OFF_BASE + 7]).toBeCloseTo(0.07);
    expect(f[pr.LAYER_OFF_MACROS + 7]).toBeCloseTo(0.9);
    expect(f[pr.LAYER_OFF_KIT + 2 * COUNT + 5]).toBeCloseTo(0.3);
  });

  it('track blob decodes to the section, clip, event and warp records', () => {
    const p = project();
    const ups = new Compiler().compile(p, () => frames());
    const t = ups.find((x) => x.kind === pr.LOAD_TRACK)!;
    const { u, f } = words(t.blob);
    expect(u[0]).toBe(0x31545745);
    expect([u[1], u[2], u[3], u[4], u[5]]).toEqual([2, 2, 3, 2, 1]); // sections, clips, events, warp, loop
    let w = 8;
    // section 0: grid, 4 beats, 120 bpm, 2 repeats, 50 ms ramp
    expect([u[w], f[w + 1], f[w + 2], u[w + 3], f[w + 4]]).toEqual([0, 3840, 120, 2, 50]);
    w += 8; // section 1: free, 5.5 s, bpm 0
    expect([u[w], f[w + 1], f[w + 2], u[w + 3]]).toEqual([1, 5.5, 0, 1]);
    w += 8;
    // clip 0: section 0 layer 0, loop 3840, events 0..2, curve slot 0, warp 0..2
    expect([u[w], u[w + 1], f[w + 2], u[w + 3], u[w + 4], u[w + 5] | 0, u[w + 6], u[w + 7]]).toEqual([0, 0, 3840, 0, 2, 0, 0, 2]);
    w += 8; // clip 1: layer 1 (beat), no curve
    expect([u[w], u[w + 1], u[w + 3], u[w + 4], u[w + 5] | 0, u[w + 7]]).toEqual([0, 1, 2, 1, -1, 0]);
    w += 8;
    // event 0: octave 1 added to the note's cents
    expect([f[w], f[w + 1], f[w + 2], f[w + 3], f[w + 5]]).toEqual([0, 480, 6900, 0.5, 1]);
    w += 16;
    expect([f[w], f[w + 1], f[w + 2]]).toEqual([960, 960, 7200]);
    w += 16;
    // hit: slot 1, strength 0.8, cents 4800, endStop 1
    expect([f[w], f[w + 1], f[w + 2], u[w + 4], f[w + 12]]).toEqual([0, 100, 4800, 1, 1]);
    expect(f[w + 5]).toBeCloseTo(0.8);
    w += 16;
    expect([f[w], f[w + 1], f[w + 2], f[w + 3]]).toEqual([0, 0, 0.25, 480]);
  });
});

describe('diff', () => {
  it('first compile sends layers, curves, then the track; second sends nothing', () => {
    const p = project();
    const c = new Compiler();
    const arr = frames();
    const first = c.compile(p, () => arr);
    expect(kinds(first)).toEqual([`${pr.LOAD_LAYER}:0`, `${pr.LOAD_LAYER}:1`, `${pr.LOAD_CURVE}:0`, `${pr.LOAD_TRACK}:0`]);
    expect(c.compile(p, () => arr)).toEqual([]);
  });

  it('a curve array is cached by the caller, so the same object is not re-sent', () => {
    const p = project();
    const c = new Compiler();
    const arr = frames();
    c.compile(p, () => arr);
    expect(c.compile(p, () => arr)).toEqual([]);
    expect(kinds(c.compile(p, () => frames()))).toEqual([`${pr.LOAD_CURVE}:0`]);
  });

  it('changing one layer re-sends only that layer', () => {
    const p = project();
    const c = new Compiler();
    const arr = frames();
    c.compile(p, () => arr);
    const p2 = { ...p, layers: [p.layers[0], { ...p.layers[1], level: -9 }] };
    expect(kinds(c.compile(p2, () => arr))).toEqual([`${pr.LOAD_LAYER}:1`]);
  });

  it('changing a note re-sends only the track', () => {
    const p = project();
    const c = new Compiler();
    const arr = frames();
    c.compile(p, () => arr);
    const clip = p.sections[0].clips.m as MelodyClip;
    const s0 = { ...p.sections[0], clips: { ...p.sections[0].clips, m: { ...clip, notes: [{ ...clip.notes[0], t: 100 }, clip.notes[1]] } } };
    const p2 = { ...p, sections: [s0, p.sections[1]] };
    expect(kinds(c.compile(p2, () => arr))).toEqual([`${pr.LOAD_TRACK}:0`]);
  });

  it('changing a layer octave re-sends the layer and the track (notes carry the octave)', () => {
    const p = project();
    const c = new Compiler();
    const arr = frames();
    c.compile(p, () => arr);
    const p2 = { ...p, layers: [{ ...p.layers[0], tune: { ...tune, octave: 2 } }, p.layers[1]] };
    expect(kinds(c.compile(p2, () => arr))).toEqual([`${pr.LOAD_LAYER}:0`, `${pr.LOAD_TRACK}:0`]);
  });

  it('reset sends everything again', () => {
    const p = project();
    const c = new Compiler();
    const arr = frames();
    c.compile(p, () => arr);
    c.reset();
    expect(c.compile(p, () => arr).length).toBe(4);
  });
});

describe('curve slots', () => {
  function withTakes(ids: string[]): Project {
    const p = project();
    const sections: Section[] = [{ ...p.sections[0], clips: { m: melodyClip(ids[0] ?? ''), b: p.sections[0].clips.b } },
      { ...p.sections[1], clips: (ids[1] ? { m: melodyClip(ids[1], { loopLen: 5.5 }) } : {}) as Section['clips'] }];
    return { ...p, sections, takes: Object.fromEntries(ids.map((i) => [i, take(i)])) };
  }
  const slotOfTrackClip = (u: Upload[]) => {
    const { u: w } = words(u.find((x) => x.kind === pr.LOAD_TRACK)!.blob);
    const n = w[2];
    return Array.from({ length: n }, (_, i) => w[8 + 16 + i * 8 + 5] | 0);
  };

  it('allocates a slot per take, frees it, and reuses it', () => {
    const c = new Compiler();
    const a = frames(), b = frames(), d = frames();
    const arrays: Record<string, Float32Array> = { t1: a, t2: b, t3: d };
    const cf = (t: Take) => arrays[t.id];
    let ups = c.compile(withTakes(['t1', 't2']), cf);
    expect(ups.filter((x) => x.kind === pr.LOAD_CURVE).map((x) => x.slot)).toEqual([0, 1]);
    // Drop t1: its slot frees, t3 takes slot 0, t2 keeps slot 1 and is not re-sent.
    ups = c.compile(withTakes(['t3', 't2']), cf);
    expect(ups.filter((x) => x.kind === pr.LOAD_CURVE).map((x) => [x.slot, x.what])).toEqual([[0, 'curve 0 (take t3)']]);
    expect(ups.some((x) => x.kind === pr.LOAD_TRACK)).toBe(true);
  });

  it('a take with no curve, and a beat clip, get slot -1', () => {
    const c = new Compiler();
    const ups = c.compile(withTakes(['t1']), () => null);
    expect(ups.some((x) => x.kind === pr.LOAD_CURVE)).toBe(false);
    expect(slotOfTrackClip(ups)).toEqual([-1, -1]);
  });

  it('the track records the slot of each clip', () => {
    const c = new Compiler();
    const ups = c.compile(withTakes(['t1', 't2']), () => frames());
    expect(slotOfTrackClip(ups)).toEqual([0, -1, 1]);
  });
});

describe('removed layers', () => {
  it('a slot with no layer left is sent muted, once', () => {
    const p = project();
    const c = new Compiler();
    const first = frames();
    c.compile(p, () => first);
    const p2: Project = { ...p, layers: [p.layers[0]], sections: [{ ...p.sections[0], clips: { m: p.sections[0].clips.m } }, p.sections[1]] };
    const arr = frames();
    const ups = c.compile(p2, () => arr);
    const silent = ups.find((x) => x.kind === pr.LOAD_LAYER && x.slot === 1)!;
    expect(silent).toBeTruthy();
    expect(words(silent.blob).f[pr.LAYER_OFF_FIELDS + pr.LAYER_MUTE]).toBe(1);
    expect(ups.some((x) => x.kind === pr.LOAD_TRACK)).toBe(true);
    expect(c.compile(p2, () => arr)).toEqual([]);
  });
});
