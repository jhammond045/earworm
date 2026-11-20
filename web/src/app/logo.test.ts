import { describe, expect, it } from 'vitest';
import { LOGO_WORDS, SOURCE, logoPlan } from './logo';

const bag = (s: string) => [...s].sort().join('');

describe('logo words', () => {
  it('spells each word from six letters of EARWORM and leaves one behind', () => {
    expect(new Set(LOGO_WORDS).size).toBe(7);
    for (const word of LOGO_WORDS) {
      const plan = logoPlan(word);
      expect(plan, word).not.toBeNull();
      expect(plan!.from.map((i) => SOURCE[i]).join('')).toBe(word);
      expect(new Set(plan!.from).size).toBe(word.length);
      expect(plan!.from).not.toContain(plan!.spare);
      expect(bag(word + SOURCE[plan!.spare])).toBe(bag(SOURCE));
    }
  });

  it('rejects a word EARWORM cannot spell', () => {
    expect(logoPlan('XXXXXX')).toBeNull();
    expect(logoPlan('RRRRRR')).toBeNull();
    expect(logoPlan('EARWORM')).toBeNull();
    expect(logoPlan('MARROWX')).toBeNull();
  });
});
