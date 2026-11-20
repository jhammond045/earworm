import { describe, expect, it } from 'vitest';
import { exportProject, importProject, zipStore, unzip } from './archive';
import { newProject } from './commands';
import type { Project } from './types';

describe('project zip', () => {
  it('round-trips a document and its blobs, and migrates an older document on the way in', async () => {
    const doc = newProject('Saved');
    const audio = new Float32Array([0.25, -0.5, 0.125]);
    doc.takes.t1 = {
      id: 't1', audio: 'audio-t1', frames: 'frames-t1', rate: 48000, latencyMs: 1, frameCount: 1,
      kind: 'melody', refCents: 0, root: 0, scale: 0, bpm: null,
    };
    const frames = new Float32Array([1, 2, 3, 4]);
    const blobs = new Map<string, Float32Array>([['audio-t1', audio], ['frames-t1', frames]]);
    const { bytes, missing } = await exportProject(doc, async (ref) => blobs.get(ref) ?? null);
    expect(missing).toEqual([]);
    const back = await importProject(bytes);
    expect(back.doc.name).toBe('Saved');
    expect(back.doc.schema).toBe(doc.schema);
    expect(Array.from(back.blobs.get('audio-t1')!)).toEqual([0.25, -0.5, 0.125]);
    expect(Array.from(back.blobs.get('frames-t1')!)).toEqual([1, 2, 3, 4]);

    const old = { ...doc, schema: 1 } as Project;
    for (const s of old.sections) for (const c of Object.values(s.clips)) delete (c as { morph?: unknown }).morph;
    const packed = zipStore([{ name: 'project.json', data: new TextEncoder().encode(JSON.stringify(old)) }]);
    const migrated = await importProject(packed);
    expect(migrated.doc.schema).toBe(3);
    expect(migrated.doc.metro?.bar).toBe(4);
  });

  it('rejects a path that leaves the archive', async () => {
    const evil = zipStore([{ name: '../secret.txt', data: new Uint8Array([1]) }]);
    await expect(unzip(evil)).rejects.toThrow(/Unsafe path/);
    await expect(importProject(zipStore([{ name: 'readme.txt', data: new Uint8Array() }]))).rejects.toThrow(/project.json/);
  });
});
