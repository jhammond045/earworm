// Steps a lesson forward. The app highlights `step.target` and calls `event` when the user acts.

import type { Advance, Lesson, LessonStep } from './tutorials';

export class TutorialRunner {
  lesson: Lesson | null = null;
  index = 0;

  get step(): LessonStep | null {
    return this.lesson?.steps[this.index] ?? null;
  }

  get done(): boolean {
    return !!this.lesson && this.index >= this.lesson.steps.length;
  }

  start(lesson: Lesson): void {
    this.lesson = lesson;
    this.index = 0;
  }

  stop(): void {
    this.lesson = null;
    this.index = 0;
  }

  /** Advance if the step is waiting for `kind`. A `next` step only moves when kind is `next`. */
  event(kind: Advance): boolean {
    const step = this.step;
    if (!step || step.advance !== kind) return false;
    this.index += 1;
    if (this.lesson && this.index >= this.lesson.steps.length) {
      this.lesson = null;
      this.index = 0;
    }
    return true;
  }
}
