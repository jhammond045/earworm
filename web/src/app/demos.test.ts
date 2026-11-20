import { describe, expect, it } from 'vitest';
import { DEMOS, demoById } from './demos';

describe('demos', () => {
  it('builds a melody, a beat and a states project', () => {
    expect(DEMOS.map((d) => d.id)).toEqual(['melody', 'beat', 'states']);
    const melody = demoById('melody')!;
    const notes = Object.values(melody.doc.sections[0].clips).flatMap((c) => c.kind === 'beat' ? [] : c.notes);
    expect(notes.length).toBe(8);
    expect(melody.doc.metro?.on).toBe(true);
    const beat = demoById('beat')!;
    const hits = Object.values(beat.doc.sections[0].clips).flatMap((c) => c.kind === 'beat' ? c.hits : []);
    expect(hits.length).toBeGreaterThan(8);
    expect(beat.doc.metro?.voice).toBe('kit');
    const states = demoById('states')!;
    const pad = states.doc.layers.find((l) => l.kind === 'texture')!;
    expect(pad.states).toHaveLength(2);
    expect(pad.influence?.source).toBe('grain-demo-glass');
    const morph = states.doc.sections[0].clips[pad.id].morph;
    expect(morph).toHaveLength(2);
    expect(states.blobs['grain-demo-glass']?.length).toBe(4800);
  });
});