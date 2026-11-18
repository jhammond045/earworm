import { describe, expect, it } from 'vitest';
import { applyOps } from './patch';
import type { Op } from './patch';

function deepFreeze<T>(v: T): T {
  if (typeof v === 'object' && v !== null && !Object.isFrozen(v)) {
    Object.freeze(v);
    for (const x of Object.values(v)) deepFreeze(x);
  }
  return v;
}

const start = () => ({ a: { x: 1, y: [1, 2, 3] }, b: { z: { deep: true } }, list: [{ id: 'p' }, { id: 'q' }] });

describe('applyOps', () => {
  it('sets, adds and deletes object keys', () => {
    const r = applyOps(start(), [
      { op: 'set', path: ['a', 'x'], value: 9 },
      { op: 'set', path: ['a', 'new'], value: 'n' },
      { op: 'set', path: ['b', 'z'], value: undefined },
    ]);
    expect(r.doc.a).toEqual({ x: 9, y: [1, 2, 3], new: 'n' });
    expect('z' in r.doc.b).toBe(false);
  });

  it('inserts and removes array elements', () => {
    const r = applyOps(start(), [
      { op: 'ins', path: ['a', 'y', 1], value: 7 },
      { op: 'rm', path: ['a', 'y', 3] },
      { op: 'ins', path: ['a', 'y', 3], value: 8 },
    ]);
    expect(r.doc.a.y).toEqual([1, 7, 2, 8]);
  });

  it('inverse gives back a deep-equal original', () => {
    const doc = start();
    const ops: Op[] = [
      { op: 'set', path: ['a', 'x'], value: 9 },
      { op: 'set', path: ['a', 'fresh'], value: 1 },
      { op: 'set', path: ['b'], value: undefined },
      { op: 'ins', path: ['list', 0], value: { id: 'r' } },
      { op: 'rm', path: ['list', 2] },
      { op: 'set', path: ['list', 0, 'id'], value: 'changed' },
    ];
    const r = applyOps(doc, ops);
    expect(r.doc).not.toEqual(doc);
    expect(applyOps(r.doc, r.inverse).doc).toStrictEqual(doc);
  });

  it('the inverse survives a JSON round trip, including key deletion', () => {
    const doc = start();
    const r = applyOps(doc, [{ op: 'set', path: ['a', 'fresh'], value: 1 }, { op: 'set', path: ['b'], value: undefined }]);
    const inv = JSON.parse(JSON.stringify(r.inverse)) as Op[];
    expect(applyOps(r.doc, inv).doc).toStrictEqual(doc);
  });

  it('shares untouched subtrees and copies the changed path', () => {
    const doc = start();
    const r = applyOps(doc, [{ op: 'set', path: ['a', 'y', 0], value: 5 }]).doc;
    expect(r).not.toBe(doc);
    expect(r.a).not.toBe(doc.a);
    expect(r.a.y).not.toBe(doc.a.y);
    expect(r.b).toBe(doc.b);
    expect(r.list).toBe(doc.list);
    expect(r.list[1]).toBe(doc.list[1]);
  });

  it('returns the same object when nothing changes', () => {
    const doc = start();
    expect(applyOps(doc, [{ op: 'set', path: ['a', 'x'], value: 1 }]).doc).toBe(doc);
    expect(applyOps(doc, [{ op: 'set', path: ['nope'], value: undefined }]).doc).toBe(doc);
    expect(applyOps(doc, []).doc).toBe(doc);
  });

  it('works on a deep-frozen input and leaves it alone', () => {
    const doc = deepFreeze(start());
    const snapshot = JSON.stringify(doc);
    const r = applyOps(doc, [
      { op: 'set', path: ['a', 'x'], value: 2 },
      { op: 'ins', path: ['list', 2], value: { id: 'w' } },
      { op: 'rm', path: ['list', 0] },
      { op: 'set', path: ['b', 'z'], value: undefined },
    ]);
    expect(JSON.stringify(doc)).toBe(snapshot);
    expect(applyOps(r.doc, r.inverse).doc).toStrictEqual(doc);
  });

  it('can replace the root', () => {
    const r = applyOps<unknown>({ a: 1 }, [{ op: 'set', path: [], value: { b: 2 } }]);
    expect(r.doc).toEqual({ b: 2 });
    expect(applyOps(r.doc, r.inverse).doc).toEqual({ a: 1 });
  });

  it('throws on a bad path', () => {
    const doc = start();
    expect(() => applyOps(doc, [{ op: 'set', path: ['nope', 'x'], value: 1 }])).toThrow(RangeError);
    expect(() => applyOps(doc, [{ op: 'set', path: ['a', 'x', 'deeper'], value: 1 }])).toThrow(RangeError);
    expect(() => applyOps(doc, [{ op: 'ins', path: ['a', 'y', 9], value: 1 }])).toThrow(RangeError);
    expect(() => applyOps(doc, [{ op: 'rm', path: ['a', 'y', 3] }])).toThrow(RangeError);
    expect(() => applyOps(doc, [{ op: 'rm', path: ['a', 'x'] }])).toThrow(RangeError);
    expect(() => applyOps(doc, [{ op: 'set', path: ['a', 'y', 5], value: 1 }])).toThrow(RangeError);
  });
});
