import { describe, expect, it } from 'vitest';
import { COUNT } from '../gen/params';
import * as pr from '../m1/protocol';
import { Compiler, layerBlob, sectionLength, sectionUnitsPerSec, type Upload } from './compile';
import type { BeatClip, Influence, Layer, MelodyClip, MorphMarker, Project, Section, State, Take } from './types';

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
  return { kind: 'melody', take: takeId, takes: takeId ? [takeId] : [], loopLen: 3840, timeSnap: 0, morph: [],
    notes: [{ id: 'n1', t: 0, dur: 480, cents: 5700, hard: 0.5 }, { id: 'n2', t: 960, dur: 960, cents: 6000, hard: 0.2 }],
    warp: [{ takeSec: 0, clipT: 0 }, { takeSec: 0.25, clipT: 480 }], ...over };
}

function beatClip(): BeatClip {
  return { kind: 'beat', take: '', takes: [], loopLen: 3840, timeSnap: 0, morph: [],
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

// ---------- M4: Influence, grain, states, morph ----------


const GRAIN_MAGIC = 0x31475745;
const STATES_MAGIC = 0x31535745;

const ramp = (f: (i: number) => number) => Array.from({ length: COUNT }, (_, i) => f(i));
const influence = (over: Partial<Influence> = {}): Influence => ({
  target: ramp((i) => (i % 7) / 7), mask: 0b01101, source: 'grain-a', sourceName: 'kettle', sourceRate: 16000,
  grain: { start: 0, end: 4, refCents: 6000 }, useSound: true, ...over,
});
const stateOf = (id: string, v: number, amount = 0): State => ({ id, name: id, base: ramp(() => v), macros: new Array(8).fill(v), influenceAmount: amount });
const withLayer0 = (p: Project, patch: Partial<Layer>): Project => ({ ...p, layers: [{ ...p.layers[0], ...patch }, p.layers[1]] });
const withMorph = (p: Project, morph: MorphMarker[]): Project => {
  const clip = p.sections[0].clips.m as MelodyClip;
  return { ...p, sections: [{ ...p.sections[0], clips: { ...p.sections[0].clips, m: { ...clip, morph } } }, p.sections[1]] };
};
const samples = new Float32Array([0.1, 0.2, 0.3, 0.4]);
const grainFor = () => ({ data: samples, rate: 16000, refCents: 6000 as number | null });

describe('layer blob with Influence', () => {
  it('carries the target, the mask and state 0 amount; without an Influence the amount is 0', () => {
    const l = layer('m', 'melody', { states: [stateOf('s0', 0.25, 0.6)], influence: influence() });
    const { f } = words(layerBlob(l));
    expect(f[pr.LAYER_OFF_FIELDS + pr.LAYER_INFLUENCE_AMOUNT]).toBeCloseTo(0.6);
    expect(f[pr.LAYER_OFF_FIELDS + pr.LAYER_INFLUENCE_MASK]).toBe(0b01101);
    for (let i = 0; i < COUNT; i++) expect(f[pr.LAYER_OFF_INFLUENCE + i]).toBeCloseTo(l.influence!.target[i], 6);
    const bare = words(layerBlob({ ...l, influence: undefined })).f;
    expect(bare[pr.LAYER_OFF_FIELDS + pr.LAYER_INFLUENCE_AMOUNT]).toBe(0);
    expect(bare[pr.LAYER_OFF_FIELDS + pr.LAYER_INFLUENCE_MASK]).toBe(0x1f);
  });
});

describe('grain upload', () => {
  it('sends LOAD_GRAIN for an Influence with a grain region and useSound, decodes, and is cached by array identity', () => {
    const p = withLayer0(project(), { influence: influence() });
    const c = new Compiler();
    const arr = frames();
    const first = c.compile(p, () => arr, { grainFor });
    expect(kinds(first)).toEqual([`${pr.LOAD_LAYER}:0`, `${pr.LOAD_GRAIN}:0`, `${pr.LOAD_LAYER}:1`, `${pr.LOAD_CURVE}:0`, `${pr.LOAD_TRACK}:0`]);
    const { u, f } = words(first[1].blob);
    expect(u[0]).toBe(GRAIN_MAGIC);
    expect(f[1]).toBe(16000);
    expect(f[2]).toBe(6000);
    expect(u[3]).toBe(4);
    expect(Array.from(f.slice(4, 8))).toEqual(Array.from(samples));
    expect(c.compile(p, () => arr, { grainFor })).toEqual([]);
    // A new array for the same source is sent again; the unpitched case encodes NaN.
    const other = () => ({ data: new Float32Array(samples), rate: 16000, refCents: null });
    const again = c.compile(p, () => arr, { grainFor: other });
    expect(kinds(again)).toEqual([`${pr.LOAD_GRAIN}:0`]);
    expect(Number.isNaN(words(again[0].blob).f[2])).toBe(true);
  });

  it('sends nothing without useSound, without a region, without a callback, or while the data is not loaded', () => {
    const arr = frames();
    for (const p of [
      withLayer0(project(), { influence: influence({ useSound: false }) }),
      withLayer0(project(), { influence: influence({ grain: null }) }),
    ]) expect(new Compiler().compile(p, () => arr, { grainFor }).some((x) => x.kind === pr.LOAD_GRAIN)).toBe(false);
    const p = withLayer0(project(), { influence: influence() });
    expect(new Compiler().compile(p, () => arr).some((x) => x.kind === pr.LOAD_GRAIN)).toBe(false);
    const c = new Compiler();
    expect(c.compile(p, () => arr, { grainFor: () => null }).some((x) => x.kind === pr.LOAD_GRAIN)).toBe(false);
    expect(kinds(c.compile(p, () => arr, { grainFor }))).toEqual([`${pr.LOAD_GRAIN}:0`]);
  });

  it('a layer whose grain source went away gets an empty grain blob, once', () => {
    const p = withLayer0(project(), { influence: influence() });
    const c = new Compiler();
    const arr = frames();
    c.compile(p, () => arr, { grainFor });
    const p2 = withLayer0(p, { influence: undefined });
    const ups = c.compile(p2, () => arr, { grainFor });
    expect(kinds(ups)).toEqual([`${pr.LOAD_LAYER}:0`, `${pr.LOAD_GRAIN}:0`]);
    expect(words(ups[1].blob).u[3]).toBe(0);
    expect(c.compile(p2, () => arr, { grainFor })).toEqual([]);
    // Turning useSound off does the same.
    const c2 = new Compiler();
    c2.compile(p, () => arr, { grainFor });
    const off = c2.compile(withLayer0(p, { influence: influence({ useSound: false }) }), () => arr, { grainFor });
    expect(words(off.find((x) => x.kind === pr.LOAD_GRAIN)!.blob).u[3]).toBe(0);
  });

  it('a removed layer slot with a grain source is cleared', () => {
    const p0 = project();
    const p: Project = { ...p0, layers: [p0.layers[0], { ...p0.layers[1], influence: influence() }] };
    const c = new Compiler();
    const arr = frames();
    expect(c.compile(p, () => arr, { grainFor }).some((x) => x.kind === pr.LOAD_GRAIN && x.slot === 1)).toBe(true);
    const gone: Project = { ...p, layers: [p.layers[0]], sections: [{ ...p.sections[0], clips: { m: p.sections[0].clips.m } }, p.sections[1]] };
    const ups = c.compile(gone, () => arr, { grainFor });
    expect(ups.filter((x) => x.kind === pr.LOAD_GRAIN).map((x) => [x.slot, words(x.blob).u[3]])).toEqual([[1, 0]]);
  });
});

describe('states upload', () => {
  const two = (p: Project) => withLayer0(p, { states: [stateOf('s0', 0.25, 0.5), stateOf('s1', 0.75, 0.9)], influence: influence() });

  it('is not sent for a single state with no markers', () => {
    const ups = new Compiler().compile(project(), () => frames());
    expect(ups.some((x) => x.kind === pr.LOAD_STATES)).toBe(false);
  });

  it('is sent for two or more states, with base, macros and amounts decoded', () => {
    const c = new Compiler();
    const arr = frames();
    const ups = c.compile(two(project()), () => arr, { grainFor });
    const st = ups.find((x) => x.kind === pr.LOAD_STATES)!;
    expect(st.slot).toBe(0);
    const { u, f } = words(st.blob);
    expect(u[0]).toBe(STATES_MAGIC);
    expect(u[1]).toBe(2);
    expect(u.length).toBe(2 + 2 * 108);
    expect(f[2]).toBeCloseTo(0.25);
    expect(f[2 + 95]).toBeCloseTo(0.25);
    expect(f[2 + 96]).toBeCloseTo(0.25); // macro 0
    expect(f[2 + 104]).toBeCloseTo(0.5); // amount
    expect(f[2 + 108]).toBeCloseTo(0.75);
    expect(f[2 + 108 + 104]).toBeCloseTo(0.9);
    expect(f[2 + 108 + 105]).toBe(0);
  });

  it('amounts go out as 0 when the layer has no Influence', () => {
    const p = withLayer0(project(), { states: [stateOf('s0', 0.25, 0.5), stateOf('s1', 0.75, 0.9)] });
    const st = new Compiler().compile(p, () => frames()).find((x) => x.kind === pr.LOAD_STATES)!;
    const { f } = words(st.blob);
    expect(f[2 + 104]).toBe(0);
    expect(f[2 + 108 + 104]).toBe(0);
  });

  it('is sent for one state when a clip has a morph marker', () => {
    const p = withMorph(project(), [{ t: 0, state: 's0', lenMs: 0, shape: 'line' }]);
    const ups = new Compiler().compile(p, () => frames());
    const st = ups.find((x) => x.kind === pr.LOAD_STATES)!;
    expect(words(st.blob).u[1]).toBe(1);
  });

  it('changing a state re-sends only that layer (its layer blob and states blob)', () => {
    const c = new Compiler();
    const arr = frames();
    const p = two(project());
    c.compile(p, () => arr, { grainFor });
    const states = [p.layers[0].states[0], { ...p.layers[0].states[1], base: ramp(() => 0.1) }];
    const ups = c.compile(withLayer0(p, { states }), () => arr, { grainFor });
    expect(kinds(ups)).toEqual([`${pr.LOAD_LAYER}:0`, `${pr.LOAD_STATES}:0`]);
  });

  it('a layer that drops back to one state keeps a one-state blob, and a removed slot is neutralised', () => {
    const c = new Compiler();
    const arr = frames();
    const p = two(project());
    c.compile(p, () => arr, { grainFor });
    const one = withLayer0(p, { states: [p.layers[0].states[0]] });
    const ups = c.compile(one, () => arr, { grainFor });
    const st = ups.find((x) => x.kind === pr.LOAD_STATES)!;
    expect(words(st.blob).u[1]).toBe(1);
    const gone: Project = { ...one, layers: [one.layers[1]], sections: [{ ...one.sections[0], clips: { b: one.sections[0].clips.b } }, { ...one.sections[1], clips: {} }] };
    const ups2 = c.compile(gone, () => arr, { grainFor });
    expect(ups2.filter((x) => x.kind === pr.LOAD_STATES).map((x) => x.slot)).toContain(0);
  });

  it('reset sends states and grain again', () => {
    const c = new Compiler();
    const arr = frames();
    const p = two(project());
    c.compile(p, () => arr, { grainFor });
    c.reset();
    const ups = c.compile(p, () => arr, { grainFor });
    expect(kinds(ups)).toEqual([`${pr.LOAD_LAYER}:0`, `${pr.LOAD_GRAIN}:0`, `${pr.LOAD_STATES}:0`, `${pr.LOAD_LAYER}:1`, `${pr.LOAD_CURVE}:0`, `${pr.LOAD_TRACK}:0`]);
  });
});

describe('track v2 morph markers', () => {
  const markers: MorphMarker[] = [
    { t: 0, state: 's0', lenMs: 0, shape: 'line' },
    { t: 960, state: 's1', lenMs: 400, shape: 'late' },
    { t: 2880, state: 's0', lenMs: 120, shape: 'wobble' },
  ];
  const base = () => withLayer0(project(), { states: [stateOf('s0', 0.25), stateOf('s1', 0.75)] });

  it('a project without markers still compiles to the v1 track', () => {
    const t = new Compiler().compile(base(), () => frames()).find((x) => x.kind === pr.LOAD_TRACK)!;
    expect(words(t.blob).u[0]).toBe(0x31545745);
  });

  it('writes v2: state ids become indices, shape names become codes', () => {
    const p = withMorph(base(), markers);
    const t = new Compiler().compile(p, () => frames()).find((x) => x.kind === pr.LOAD_TRACK)!;
    const { u, f } = words(t.blob);
    expect(u[0]).toBe(0x32545745);
    expect(u[6]).toBe(3); // n_morph
    const nSec = u[1], nClip = u[2], nEv = u[3], nWarp = u[4];
    const clip0 = 8 + nSec * 8;
    expect([u[clip0 + 8], u[clip0 + 9]]).toEqual([0, 3]); // first_morph, n_morph of clip 0
    expect(u[clip0 + 12 + 9]).toBe(0); // the beat clip has none
    const m = 8 + nSec * 8 + nClip * 12 + nEv * 16 + nWarp * 2;
    const rec = (k: number) => [f[m + k * 4], u[m + k * 4 + 1], f[m + k * 4 + 2], u[m + k * 4 + 3]];
    expect(rec(0)).toEqual([0, 0, 0, 0]);
    expect(rec(1)).toEqual([960, 1, 400, 2]);
    expect(rec(2)).toEqual([2880, 0, 120, 4]);
  });

  it('changing a morph marker re-sends only the track; adding the first marker sends states too', () => {
    const c = new Compiler();
    const arr = frames();
    const p = withMorph(base(), markers);
    c.compile(p, () => arr);
    const moved = withMorph(p, [markers[0], { ...markers[1], t: 1200 }, markers[2]]);
    expect(kinds(c.compile(moved, () => arr))).toEqual([`${pr.LOAD_TRACK}:0`]);
    const c2 = new Compiler();
    const plain = project();
    c2.compile(plain, () => arr);
    const ups = c2.compile(withMorph(plain, [{ t: 0, state: plain.layers[0].states[0].id, lenMs: 0, shape: 'line' }]), () => arr);
    expect(kinds(ups)).toEqual([`${pr.LOAD_STATES}:0`, `${pr.LOAD_TRACK}:0`]);
  });

  it('editing a state does not re-send the track', () => {
    const c = new Compiler();
    const arr = frames();
    const p = withMorph(base(), markers);
    c.compile(p, () => arr);
    const states = [p.layers[0].states[0], { ...p.layers[0].states[1], base: ramp(() => 0.9) }];
    expect(kinds(c.compile(withLayer0(p, { states }), () => arr))).toEqual([`${pr.LOAD_LAYER}:0`, `${pr.LOAD_STATES}:0`]);
  });
});
