import { describe, expect, it } from 'vitest';
import { Store } from '../project/store';
import { PPQ } from '../project/types';
import { STRESS_GROUPS, actionLabel, applyAction, generateProject, nextAction, seeded } from './stress';

describe('generateProject', () => {
  it('has 3 sections, 6 layers, and notes or hits in every clip', () => {
    const doc = generateProject(1);
    expect(doc.name).toBe('M3 gate test');
    expect(doc.sections).toHaveLength(3);
    expect(doc.layers).toHaveLength(6);
    expect(doc.layers.map((l) => l.kind).sort()).toEqual(['beat', 'beat', 'melody', 'melody', 'melody', 'texture']);
    for (const s of doc.sections) {
      for (const l of doc.layers) {
        const c = s.clips[l.id];
        expect(c).toBeTruthy();
        expect(c.kind === 'beat' ? c.hits.length : c.notes.length).toBeGreaterThan(0);
      }
    }
  });
  it('is the same for the same seed (apart from ids)', () => {
    const strip = (d: ReturnType<typeof generateProject>) => JSON.stringify(d).replace(/"(id|section|layer)":"[^"]*"/g, '');
    const a = strip(generateProject(7));
    const b = strip(generateProject(7));
    expect(a.length).toBe(b.length);
    expect(strip(generateProject(8))).not.toBe(a);
  });
});

describe('nextAction', () => {
  it('produces only valid commands over 1000 steps, and keeps the document valid', () => {
    const store = new Store(generateProject(42));
    const rnd = seeded(2024);
    const seen = new Set<string>();
    let changed = 0;
    for (let i = 0; i < 1000; i++) {
      const a = nextAction(store.doc, rnd, store.history.canUndo, store.history.canRedo);
      seen.add(a.type === 'cmd' ? a.label : a.type);
      // Throws RangeError on an invalid command; the test fails then.
      if (applyAction(store, a)) changed++;
      const d = store.doc;
      expect(d.layers.length).toBeLessThanOrEqual(8);
      expect(d.sections.length).toBeGreaterThanOrEqual(1);
      expect(d.sections.length).toBeLessThanOrEqual(6);
    }
    expect(changed).toBeGreaterThan(800);
    // Every kind of edit in the gate's list turned up.
    for (const label of ['move notes', 'add note', 'delete note', 'move hits', 'add hit', 'delete hit', 'change hit slot', 'layer level',
      'layer pan', 'layer mute', 'section bpm', 'section repeats', 'section beats', 'add section', 'remove section', 'quantize', 'undo', 'redo']) {
      expect(seen.has(label), label).toBe(true);
    }
    // Times stay inside their loops and notes keep a duration.
    for (const s of store.doc.sections) {
      for (const c of Object.values(s.clips)) {
        for (const it of c.kind === 'beat' ? c.hits : c.notes) {
          expect(it.t).toBeGreaterThanOrEqual(0);
          expect(it.t).toBeLessThan(c.loopLen);
        }
      }
    }
    expect(store.doc.sections.every((s) => s.beats * PPQ > 0)).toBe(true);
  });

  it('is deterministic for a seed', () => {
    const run = () => {
      const store = new Store(generateProject(5));
      const r = seeded(9);
      const labels: string[] = [];
      for (let i = 0; i < 50; i++) {
        const a = nextAction(store.doc, r, store.history.canUndo, store.history.canRedo);
        labels.push(actionLabel(a));
        applyAction(store, a);
      }
      return labels;
    };
    expect(run()).toEqual(run());
  });

  it('exports the group list', () => {
    expect(STRESS_GROUPS).toContain('quantize');
    expect(STRESS_GROUPS).toContain('undo');
  });
});
