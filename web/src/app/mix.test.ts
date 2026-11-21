import { describe, expect, it } from 'vitest';
import { LOAD_CURVE, LOAD_LAYER, LOAD_PATCH, LOAD_SAMPLE, LOAD_STATES, LOAD_TRACK } from '../m1/protocol';
import { newProject } from '../project/commands';
import { songSeconds, uploadsToSong } from './mix';

describe('uploadsToSong', () => {
  it('groups a compiler upload by kind and rejects a song with no track', () => {
    const track = new ArrayBuffer(8);
    const layer = new ArrayBuffer(4);
    const patch = new ArrayBuffer(4);
    const song = uploadsToSong([
      { kind: LOAD_PATCH, slot: 1, blob: patch, what: 'patch' },
      { kind: LOAD_LAYER, slot: 1, blob: layer, what: 'layer' },
      { kind: LOAD_TRACK, slot: 0, blob: track, what: 'track' },
      { kind: LOAD_CURVE, slot: 0, blob: new ArrayBuffer(2), what: 'curve' },
      { kind: LOAD_SAMPLE, slot: 1, blob: new ArrayBuffer(2), what: 'sample' },
      { kind: LOAD_STATES, slot: 1, blob: new ArrayBuffer(2), what: 'states' },
    ]);
    expect(song.track).toBe(track);
    expect(song.layers[1]).toBe(layer);
    expect(song.patches?.[1]).toBe(patch);
    expect(song.curves).toHaveLength(1);
    expect(song.samples?.[0].slot).toBe(1);
    expect(song.states?.[0].slot).toBe(1);
    expect(() => uploadsToSong([])).toThrow(/no track/);
  });
});

describe('songSeconds', () => {
  it('adds grid and free sections, including repeats', () => {
    const doc = newProject('s');
    doc.sections[0].beats = 8;
    doc.sections[0].bpm = 120;
    doc.sections[0].repeats = 2;
    expect(songSeconds(doc)).toBeCloseTo(8);
  });
});
