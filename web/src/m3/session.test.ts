import { describe, expect, it } from 'vitest';
import { newProject } from '../project/commands';
import { memoryBlobs, memoryKv, ProjectDb } from '../project/persist';
import type { Influence, Project } from '../project/types';
import { Session } from './session';

const infl = (over: Partial<Influence> = {}): Influence => ({
  target: new Array(96).fill(0.5), mask: 0x1f, source: 'grain-abc', sourceName: 'x', sourceRate: 48000,
  grain: { start: 100, end: 400, refCents: null }, useSound: true, ...over,
});

async function open(doc: Project): Promise<Session> {
  const kv = memoryKv();
  const blobs = memoryBlobs();
  const s = new Session(kv, new ProjectDb(kv), () => blobs);
  await s.adopt(doc);
  return s;
}

describe('Session grains', () => {
  it('grainFor is null until the source is loaded, then slices the region and keeps the same array', async () => {
    const s = await open(newProject('t'));
    s.store.dispatch('setInfluence', { layer: s.doc.layers[0].id, influence: infl() });
    const layer = s.doc.layers[0];
    expect(s.grainFor(layer)).toBeNull();
    const data = Float32Array.from({ length: 1000 }, (_, i) => i);
    await s.putGrain('grain-abc', data);
    const g = s.grainFor(s.doc.layers[0])!;
    expect(g.data).toHaveLength(300);
    expect(g.data[0]).toBe(100);
    expect(g.rate).toBe(48000);
    expect(g.refCents).toBeNull();
    // Same blob and region: the identical array, so the compiler does not send it again.
    expect(s.grainFor(s.doc.layers[0])!.data).toBe(g.data);
  });

  it('a different region gives a different array; a layer without a grain region gives null', async () => {
    const s = await open(newProject('t'));
    await s.putGrain('grain-abc', new Float32Array(1000));
    s.store.dispatch('setInfluence', { layer: s.doc.layers[0].id, influence: infl() });
    const a = s.grainFor(s.doc.layers[0])!;
    s.store.dispatch('setInfluence', { layer: s.doc.layers[0].id, influence: infl({ grain: { start: 0, end: 50, refCents: 4800 } }) });
    const b = s.grainFor(s.doc.layers[0])!;
    expect(b.data).not.toBe(a.data);
    expect(b.data).toHaveLength(50);
    expect(b.refCents).toBe(4800);
    s.store.dispatch('setInfluence', { layer: s.doc.layers[0].id, influence: infl({ grain: null, useSound: false }) });
    expect(s.grainFor(s.doc.layers[0])).toBeNull();
  });

  it('clamps a region that runs past the audio', async () => {
    const s = await open(newProject('t'));
    await s.putGrain('grain-abc', new Float32Array(200));
    s.store.dispatch('setInfluence', { layer: s.doc.layers[0].id, influence: infl({ grain: { start: 150, end: 9999, refCents: null } }) });
    expect(s.grainFor(s.doc.layers[0])!.data).toHaveLength(50);
  });

  it('loads a stored source when the project opens', async () => {
    const kv = memoryKv();
    const blobs = memoryBlobs();
    const db = new ProjectDb(kv);
    const s = new Session(kv, db, () => blobs);
    await s.adopt(newProject('t'));
    await s.putGrain('grain-abc', Float32Array.of(1, 2, 3));
    s.store.dispatch('setInfluence', { layer: s.doc.layers[0].id, influence: infl({ grain: { start: 0, end: 3, refCents: null } }) });
    await db.flush();
    const s2 = new Session(kv, db, () => blobs);
    expect(await s2.open(s.doc.id)).toBe(true);
    expect(s2.grains.get('grain-abc')).toEqual(Float32Array.of(1, 2, 3));
  });
});
