import { describe, expect, it } from 'vitest';
import { newProject } from './commands';
import { cleanupPlan, unusedRefs } from './cleanup';
import { Store } from './store';
import type { MelodyClip, Take } from './types';

const take = (id: string): Take => ({
  id, audio: `audio-${id}`, frames: `frames-${id}`, rate: 48000, latencyMs: 0, frameCount: 1,
  kind: 'melody', refCents: 0, root: 0, scale: 0, bpm: null,
});

describe('cleanup', () => {
  it('keeps a take a clip or the undo stack still points at', () => {
    const store = new Store(newProject('x'));
    const sec = store.doc.sections[0].id;
    const mel = store.doc.layers[0].id;
    store.dispatch('addTake', { take: take('keep') });
    store.dispatch('addTake', { take: take('old') });
    const clip: MelodyClip = { kind: 'melody', take: 'keep', takes: ['keep'], loopLen: 7680, timeSnap: 0, morph: [], notes: [], warp: [] };
    store.dispatch('setClip', { section: sec, layer: mel, clip });
    store.dispatch('setClip', { section: sec, layer: mel, clip: { ...clip, take: 'old', takes: ['old', 'keep'] } });
    store.dispatch('setClip', { section: sec, layer: mel, clip: null });
    // The current document uses neither take. Undo still restores the clips that do.
    const plan = cleanupPlan(store.doc, store.history.toJSON());
    expect(plan.takeIds).toEqual([]);
    const gone = cleanupPlan(store.doc, { undo: [], redo: [] });
    expect(gone.takeIds).toEqual(['keep', 'old']);
    expect(unusedRefs(['audio-keep', 'frames-old', 'grain-x', 'audio-other'], gone)).toEqual(['audio-keep', 'audio-other', 'frames-old', 'grain-x']);
  });

  it('keeps a grain source the layer or the history still names', () => {
    const store = new Store(newProject('x'));
    const layer = store.doc.layers[0].id;
    const influence = {
      target: new Array(96).fill(0.2), mask: 31, source: 'grain-a', sourceName: 'a', sourceRate: 48000,
      grain: null, useSound: false,
    };
    store.dispatch('setInfluence', { layer, influence });
    store.dispatch('setInfluence', { layer, influence: null });
    expect(cleanupPlan(store.doc, store.history.toJSON()).keep).toContain('grain-a');
    expect(cleanupPlan(store.doc, { undo: [], redo: [] }).keep).not.toContain('grain-a');
  });
});
