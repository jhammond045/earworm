import { describe, expect, it } from 'vitest';
import { applyOps } from '../project/patch';
import { commands, newProject } from '../project/commands';
import { exportProject, importProject } from '../project/archive';
import { memoryKv, ProjectDb } from '../project/persist';
import { Store } from '../project/store';
import type { Layer, Project } from '../project/types';
import { layoutOf } from '../synth/patch';
import { channelLayer, mappedTarget, notePlays, parseMidi, percentile, toDefault } from './midi';

const keys = (l: Layer) => layoutOf(l.patch).index;
const run = <K extends keyof typeof commands>(doc: Project, name: K, args: Parameters<(typeof commands)[K]>[1]) =>
  applyOps(doc, (commands[name] as (d: Project, a: unknown) => ReturnType<(typeof commands)[K]>)(doc, args)).doc;

describe('MIDI messages', () => {
  it('reads notes, controllers, bend and pressure on any channel', () => {
    expect(parseMidi([0x90, 60, 127])).toEqual({ kind: 'on', channel: 0, note: 60, velocity: 1 });
    expect(parseMidi([0x93, 60, 0])).toEqual({ kind: 'off', channel: 3, note: 60 });
    expect(parseMidi([0x8f, 61, 40])).toEqual({ kind: 'off', channel: 15, note: 61 });
    expect(parseMidi([0xb1, 74, 127])).toEqual({ kind: 'cc', channel: 1, cc: 74, value: 1 });
    expect(parseMidi([0xe0, 0, 64])).toEqual({ kind: 'bend', channel: 0, value: 0 });
    expect(parseMidi([0xe0, 0, 0])).toEqual({ kind: 'bend', channel: 0, value: -1 });
    expect(parseMidi([0xe0, 127, 127])).toEqual({ kind: 'bend', channel: 0, value: 1 });
    expect(parseMidi([0xd2, 127])).toEqual({ kind: 'pressure', channel: 2, value: 1 });
    expect(parseMidi([0xf8])).toBeNull();
    expect(parseMidi([0xc0, 5])).toBeNull();
    expect(parseMidi([0xa0, 60, 10])).toBeNull();
  });

  it('plays the pitch on a melody layer and the GM kit notes on a beat layer', () => {
    const doc = newProject('Song');
    const [melody, beat] = doc.layers;
    expect(notePlays(melody, 60)).toEqual({ slot: 0, cents: 4800 });
    expect(notePlays(melody, 69)).toEqual({ slot: 0, cents: 5700 });
    expect([36, 38, 42, 46].map((n) => notePlays(beat, n)?.slot)).toEqual([0, 1, 2, 3]);
    expect(notePlays(beat, 38)?.cents).toBe(beat.kit[1].cents);
    expect(notePlays(beat, 60)).toBeNull();
  });

  it('sends a channel to the layer the project names for it, else to the selected layer', () => {
    let doc = newProject('Song');
    const [melody, beat] = doc.layers;
    expect(channelLayer(doc, 9, melody.id)?.id).toBe(melody.id);
    doc = run(doc, 'setPerform', { channel: 9, layer: beat.id });
    expect(channelLayer(doc, 9, melody.id)?.id).toBe(beat.id);
    expect(channelLayer(doc, 0, melody.id)?.id).toBe(melody.id);
    doc = run(doc, 'removeLayer', { id: beat.id });
    expect(doc.perform?.channels[9]).toBe('');
    expect(channelLayer(doc, 9, melody.id)?.id).toBe(melody.id);
  });

  it('percentile by nearest rank', () => {
    expect(percentile([5, 1, 4, 2, 3], 50)).toBe(3);
    expect(percentile(Array.from({ length: 100 }, (_, i) => i + 1), 95)).toBe(95);
    expect(percentile([], 95)).toBeNaN();
  });
});

describe('Learn', () => {
  const cutoff = 'filter1.cutoff';

  it('maps a controller to a value or a macro, one controller to one target, and checks both', () => {
    let doc = newProject('Song');
    const l = doc.layers[0];
    expect(layoutOf(l.patch).index.has(cutoff)).toBe(true);
    doc = run(doc, 'learnMidi', { device: 'Keys', channel: 0, cc: 74, target: { layer: l.id, key: cutoff } });
    doc = run(doc, 'learnMidi', { device: 'Keys', channel: 0, cc: 71, target: { layer: l.id, key: 'macro2' } });
    expect(mappedTarget(doc, undefined, 'Keys', 0, 74, keys)).toEqual({ layer: l.id, key: cutoff });
    expect(mappedTarget(doc, undefined, 'Keys', 0, 71, keys)).toEqual({ layer: l.id, key: 'macro2' });
    expect(mappedTarget(doc, undefined, 'Keys', 1, 74, keys)).toBeNull();
    expect(mappedTarget(doc, undefined, 'Pads', 0, 74, keys)).toBeNull();
    // Learning the same controller again moves it.
    doc = run(doc, 'learnMidi', { device: 'Keys', channel: 0, cc: 74, target: { layer: l.id, key: 'macro3' } });
    expect(doc.perform?.map.length).toBe(2);
    expect(mappedTarget(doc, undefined, 'Keys', 0, 74, keys)?.key).toBe('macro3');
    doc = run(doc, 'forgetMidi', { device: 'Keys', channel: 0, cc: 74 });
    expect(mappedTarget(doc, undefined, 'Keys', 0, 74, keys)).toBeNull();
    expect(() => commands.learnMidi(doc, { device: 'Keys', channel: 0, cc: 74, target: { layer: l.id, key: 'nope.x' } })).toThrow(/no value/);
    expect(() => commands.learnMidi(doc, { device: 'Keys', channel: 16, cc: 74, target: { layer: l.id, key: cutoff } })).toThrow(/channel/);
    expect(() => commands.learnMidi(doc, { device: 'Keys', channel: 0, cc: 128, target: { layer: l.id, key: cutoff } })).toThrow(/controller/);
  });

  it('uses the device default when the project has no mapping for that device', () => {
    let saved = newProject('Saved');
    saved = run(saved, 'learnMidi', { device: 'Keys', channel: 0, cc: 74, target: { layer: saved.layers[1].id, key: 'macro1' } });
    const defaults = toDefault(saved, 'Keys');
    expect(defaults).toEqual([{ channel: 0, cc: 74, layer: 1, key: 'macro1' }]);
    let other = newProject('Other');
    expect(mappedTarget(other, defaults, 'Keys', 0, 74, keys)).toEqual({ layer: other.layers[1].id, key: 'macro1' });
    // Once the project maps that device itself, its own map is the whole story.
    other = run(other, 'learnMidi', { device: 'Keys', channel: 0, cc: 10, target: { layer: other.layers[0].id, key: 'macro4' } });
    expect(mappedTarget(other, defaults, 'Keys', 0, 74, keys)).toBeNull();
  });

  it('survives a reload (journal replay) and a project export and import', async () => {
    const kv = memoryKv();
    const db = new ProjectDb(kv);
    const doc = newProject('Song');
    await db.create(doc);
    const store = new Store(doc);
    store.subscribe((d, entry) => db.append(d.id, entry, d, store.history.toJSON()));
    const target = { layer: doc.layers[0].id, key: cutoff };
    store.dispatch('learnMidi', { device: 'Keys', channel: 2, cc: 74, target });
    store.dispatch('setPerform', { channel: 2, layer: doc.layers[0].id });
    store.dispatch('setLayer', { id: doc.layers[0].id, sceneMs: 900 });
    await db.flush();

    const reopened = await new ProjectDb(kv).open(doc.id);
    expect(reopened?.doc.perform?.map).toEqual([{ device: 'Keys', channel: 2, cc: 74, target }]);
    expect(reopened?.doc.perform?.channels[2]).toBe(doc.layers[0].id);
    expect(reopened?.doc.layers[0].sceneMs).toBe(900);
    // Undo removes them one step at a time, the scene time first.
    expect(reopened!.history.undo.map((g) => g.name)).toEqual(['learnMidi', 'setPerform', 'setLayer']);

    const { bytes } = await exportProject(store.doc, async () => null);
    const back = await importProject(bytes);
    expect(back.doc.perform).toEqual(store.doc.perform);
    expect(mappedTarget(back.doc, undefined, 'Keys', 2, 74, keys)).toEqual(target);
    expect(back.doc.layers[0].sceneMs).toBe(900);
  });
});
