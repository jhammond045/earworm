import { describe, expect, it } from 'vitest';
import { TutorialRunner } from './tutorial';
import { LESSONS, lessonById } from './tutorials';

describe('tutorials', () => {
  it('covers a control for every feature area and only advances on the matching action', () => {
    const targets = new Set(LESSONS.flatMap((l) => l.steps.map((s) => s.target)));
    for (const name of ['start', 'transport', 'metro', 'pattern', 'layers', 'capture', 'influence', 'states', 'morph', 'file', 'undo', 'help', 'timeline', 'macros', 'gestures', 'kit']) {
      expect(targets.has(name), name).toBe(true);
    }
    const runner = new TutorialRunner();
    const lesson = lessonById('around')!;
    runner.start(lesson);
    expect(runner.event('play')).toBe(false);
    expect(runner.step?.id).toBe('start');
    expect(runner.event('start')).toBe(true);
    expect(runner.step?.advance).toBe('play');
    runner.event('play');
    expect(runner.step?.advance).toBe('metro');
  });
});
