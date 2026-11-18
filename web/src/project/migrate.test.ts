import { describe, expect, it } from 'vitest';
import { newProject } from './commands';
import type { JournalEntry } from './history';
import { migrate, migrateEntry, migrateHistory } from './migrate';
import { applyOps } from './patch';
import { SCHEMA } from './types';
import type { Project } from './types';

/** A schema 1 document: no morph lanes. */
function v1(): Record<string, unknown> {
  const p = newProject('Old');
  const mel = p.layers[0].id;
  const clip = { kind: 'melody', take: '', takes: [], loopLen: 3840, timeSnap: 0, notes: [], warp: [] };
  const beatClip = { kind: 'beat', take: '', takes: [], loopLen: 3840, timeSnap: 0, hits: [] };
  const doc = JSON.parse(JSON.stringify({ ...p, schema: 1 }));
  doc.sections[0].clips = { [mel]: clip, [p.layers[1].id]: beatClip };
  return doc;
}

describe('migrate', () => {
  it('1 -> 2 adds an empty morph lane to every clip and sets the schema', () => {
    const doc = v1();
    const frozen = JSON.stringify(doc);
    const out = migrate(doc);
    expect(JSON.stringify(doc)).toBe(frozen); // input untouched
    expect(out.schema).toBe(2);
    for (const c of Object.values(out.sections[0].clips)) expect(c.morph).toEqual([]);
    expect(out.layers).toBe((doc as { layers: unknown }).layers); // untouched parts are shared
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

  it('upgrades clips inside journal ops and undo history, so old entries replay to a valid document', () => {
    const doc = migrate(v1());
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
    const up = migrateEntry(entry, 1);
    const after = applyOps(doc, up.group.ops).doc as Project;
    expect(after.sections[0].clips[lay].morph).toEqual([]);
    expect(oldClip).not.toHaveProperty('morph');
    // A whole old section inside an op is upgraded too (duplicateSection, setSection feel change).
    const secOld = JSON.parse(JSON.stringify(v1().sections))[0];
    const dup = migrateEntry({ kind: 'do', group: { name: 'x', ops: [{ op: 'ins', path: ['sections', 1], value: secOld }], inverse: [{ op: 'rm', path: ['sections', 1] }] } }, 1);
    const withDup = applyOps(doc, dup.group.ops).doc as Project;
    for (const c of Object.values(withDup.sections[1].clips)) expect(c.morph).toEqual([]);
    expect(sec).toBeTruthy();
    const h = migrateHistory({ undo: [entry.group], redo: [] }, 1);
    expect(h.undo[0].ops[0]).toMatchObject({ value: { morph: [] } });
    expect(migrateHistory({ undo: [entry.group], redo: [] }, SCHEMA).undo[0]).toBe(entry.group);
  });
});
