import { describe, expect, it } from 'vitest';
import { tuneFromTake } from './tune';

describe('tuneFromTake', () => {
  it('keeps a root, a scale and a reference that already fit', () => {
    expect(tuneFromTake({ root: 7, scale: 3, refCents: 12 })).toEqual({ root: 7, scale: 3, refCents: 12 });
  });

  it('clamps a wild second-pass result onto the tune fields', () => {
    expect(tuneFromTake({ root: 14.6, scale: -2, refCents: 80 })).toEqual({ root: 11, scale: 0, refCents: 50 });
    expect(tuneFromTake({ root: -1, scale: 1.2, refCents: -90 })).toEqual({ root: 0, scale: 1, refCents: -50 });
  });
});
