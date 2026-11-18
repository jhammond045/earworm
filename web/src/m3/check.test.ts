import { describe, expect, it } from 'vitest';
import { firstDifference, hashText, stableStringify } from './check';

describe('check helpers', () => {
  it('stableStringify ignores key order', () => {
    expect(stableStringify({ b: 1, a: { d: 2, c: [3, { z: 1, y: 2 }] } })).toBe(stableStringify({ a: { c: [3, { y: 2, z: 1 }], d: 2 }, b: 1 }));
  });
  it('hash is stable and differs for different text', async () => {
    expect(await hashText('a')).toBe(await hashText('a'));
    expect(await hashText('a')).not.toBe(await hashText('b'));
  });
  it('finds the first difference with a path', () => {
    expect(firstDifference({ a: [1, { t: 120 }] }, { a: [1, { t: 120 }] })).toBeNull();
    expect(firstDifference({ a: [1, { t: 120 }] }, { a: [1, { t: 121 }] })).toBe('a[1].t: 120 vs 121');
    expect(firstDifference({ a: [1] }, { a: [1, 2] })).toBe('a[1]: undefined vs 2');
    expect(firstDifference({ x: 1 }, { y: 1 })).toBe('x: 1 vs undefined');
  });
});
