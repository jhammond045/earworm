import { describe, expect, it } from 'vitest';
import { newProject } from './commands';
import type { JournalEntry } from './history';
import { historyResets, migrate, migrateEntry, migrateHistory, replaySchema } from './migrate';
import { applyOps } from './patch';
import { SCHEMA } from './types';
import type { Project } from './types';
import { classicPatch, vocabFromValues } from '../synth/classic';

/** A schema 4 document written back in the schema 3 shape: no patches, 96-value states and kit slot sets. */
function toV3(p: Project): Record<string, any> {
  const doc = JSON.parse(JSON.stringify(p)) as Record<string, any>;
  doc.schema = 3;
  doc.layers = p.layers.map((l) => {
    const { patch, ...rest } = JSON.parse(JSON.stringify(l)) as Record<string, any>;
    void patch;
    return {
      ...rest,
      states: l.states.map(({ values, ...st }) => ({ ...st, base: vocabFromValues(l.kind, values) })),
      kit: l.kit.map((k, i) => ({ cents: k.cents, params: vocabFromValues(l.kind, l.states[0].values, i + 1) })),
    };
  });
  return doc;
}

/** A schema 1 document: no morph lanes. */
function v1(): Record<string, unknown> {
  const p = newProject('Old');
  const mel = p.layers[0].id;
  const clip = { kind: 'melody', take: '', takes: [], loopLen: 3840, timeSnap: 0, notes: [], warp: [] };
  const beatClip = { kind: 'beat', take: '', takes: [], loopLen: 3840, timeSnap: 0, hits: [] };
  const doc: Record<string, any> = { ...toV3(p), schema: 1 };
  doc.sections[0].clips = { [mel]: clip, [p.layers[1].id]: beatClip };
  return doc;
}

describe('migrate', () => {
  it('4 -> 5 changes only the schema number: flex fields are optional', () => {
    const doc = { ...newProject('x'), schema: 4 };
    const out = migrate(doc, 5);
    expect(out.schema).toBe(5);
    expect({ ...out, schema: 4 }).toEqual(doc);
    expect(out.sections).toBe(doc.sections);
  });

  it('5 -> 6 changes only the schema number: poly capture fields are optional', () => {
    const doc = { ...newProject('x'), schema: 5 };
    const out = migrate(doc);
    expect(out.schema).toBe(6);
    expect({ ...out, schema: 5 }).toEqual(doc);
    expect(out.layers).toBe(doc.layers);
  });

  it('1 -> 2 adds an empty morph lane to every clip and sets the schema', () => {
    const doc = v1();
    const frozen = JSON.stringify(doc);
    const out = migrate(doc);
    expect(JSON.stringify(doc)).toBe(frozen); // input untouched
    expect(out.schema).toBe(SCHEMA);
    expect(out.metro?.voice).toBe('tick');
    for (const c of Object.values(out.sections[0].clips)) expect(c.morph).toEqual([]);
    expect(out.takes).toBe((doc as { takes: unknown }).takes); // untouched parts are shared
  });

  it('leaves a current document alone, by identity', () => {
    const p = newProject('now');
    expect(p.schema).toBe(SCHEMA);
    expect(migrate(p)).toBe(p);
  });

  it('keeps morph lanes that already exist', () => {
    const doc = v1() as { sections: { clips: Record<string, { morph?: unknown }> }[] };
    const first = Object.values(doc.sections[0].clips)[0];
    first.morph = [{ t: 1, state: 's', lenMs: 0, shape: 'line' }];
    expect(Object.values(migrate(doc).sections[0].clips)[0].morph).toHaveLength(1);
  });

  it('throws on a future or malformed schema', () => {
    expect(() => migrate({ ...newProject('x'), schema: SCHEMA + 1 })).toThrow(/newer version/);
    expect(() => migrate({ ...newProject('x'), schema: 0 })).toThrow(RangeError);
    expect(() => migrate({ ...newProject('x'), schema: 'two' })).toThrow(RangeError);
    expect(() => migrate({ name: 'no schema' })).toThrow(RangeError);
    expect(() => migrate(null)).toThrow(RangeError);
  });

  it('upgrades clips inside journal ops up to schema 3, so old entries replay to a valid document', () => {
    const doc = migrate(v1(), 3);
    const sec = doc.sections[0].id;
    const lay = doc.layers[0].id;
    const oldClip = { kind: 'melody', take: '', takes: [], loopLen: 1920, timeSnap: 0, notes: [], warp: [] };
    const entry: JournalEntry = {
      kind: 'do',
      group: {
        name: 'setClip',
        ops: [{ op: 'set', path: ['sections', 0, 'clips', lay], value: oldClip }],
        inverse: [{ op: 'set', path: ['sections', 0, 'clips', lay], value: v1().sections as never }],
      },
    };
    const up = migrateEntry(entry, 1, 3);
    const after = applyOps(doc, up.group.ops).doc as Project;
    expect(after.sections[0].clips[lay].morph).toEqual([]);
    expect(oldClip).not.toHaveProperty('morph');
    // A whole old section inside an op is upgraded too (duplicateSection, setSection feel change).
    const secOld = JSON.parse(JSON.stringify(v1().sections))[0];
    const dup = migrateEntry({ kind: 'do', group: { name: 'x', ops: [{ op: 'ins', path: ['sections', 1], value: secOld }], inverse: [{ op: 'rm', path: ['sections', 1] }] } }, 1, 3);
    const withDup = applyOps(doc, dup.group.ops).doc as Project;
    for (const c of Object.values(withDup.sections[1].clips)) expect(c.morph).toEqual([]);
    expect(sec).toBeTruthy();
    // Ops before schema 4 cannot come along (they name states[].base): the undo history starts fresh.
    expect(historyResets(1)).toBe(true);
    expect(replaySchema(1)).toBe(3);
    expect(migrateHistory({ undo: [entry.group], redo: [] }, 1)).toEqual({ undo: [], redo: [] });
    expect(historyResets(SCHEMA)).toBe(false);
    expect(migrateHistory({ undo: [entry.group], redo: [] }, SCHEMA).undo[0]).toBe(entry.group);
  });

  it('3 -> 4 gives each layer the classic patch, values by key, kit pitches, and notes velocity 1', () => {
    const now = newProject('Kit');
    const mel = now.layers[0].id;
    const old = toV3(now);
    old.sections[0].clips = { [mel]: { kind: 'melody', take: '', takes: [], loopLen: 3840, timeSnap: 0, morph: [], warp: [],
      notes: [{ id: 'n', t: 0, dur: 480, cents: 4800, hard: 0.5 }] } };
    old.layers[0].states[0].base[3] = 0.123; // src.tilt
    old.layers[1].kit[2].params[3] = 0.456; // kit slot 3's src.tilt
    const out = migrate(old, 4);
    expect(out.schema).toBe(4);
    expect(out.layers[0].patch).toEqual(classicPatch('melody'));
    expect(out.layers[1].patch).toEqual(classicPatch('beat'));
    expect(out.layers[0].states[0].values['partials1.tilt']).toBe(0.123);
    expect(out.layers[1].states[0].values['k3.partials1.tilt']).toBe(0.456);
    expect(out.layers[1].kit).toEqual(now.layers[1].kit);
    expect(out.layers[1].kit[0]).not.toHaveProperty('params');
    expect(out.layers[0].states[0]).not.toHaveProperty('base');
    expect((out.sections[0].clips[mel] as { notes: { vel?: number }[] }).notes[0].vel).toBe(1);
    // Everything else comes back as it was.
    const fresh = migrate(toV3(now));
    expect(fresh.layers).toEqual(now.layers);
  });
});
