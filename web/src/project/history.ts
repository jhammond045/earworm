// Undo and redo stacks of patch groups. JSON-serializable so undo survives a reload.

import type { Op } from './patch';

export interface Group { name: string; ops: Op[]; inverse: Op[] }
export interface JournalEntry { kind: 'do' | 'undo' | 'redo'; group: Group }
export interface HistoryState { undo: Group[]; redo: Group[] }

export class History {
  private undoStack: Group[];
  private redoStack: Group[];

  constructor(state?: HistoryState, private readonly limit = 500) {
    this.undoStack = state ? state.undo.slice() : [];
    this.redoStack = state ? state.redo.slice() : [];
  }

  get canUndo(): boolean { return this.undoStack.length > 0; }
  get canRedo(): boolean { return this.redoStack.length > 0; }

  /** A new change: clears redo, drops the oldest group past the limit. */
  push(g: Group): void {
    this.undoStack.push(g);
    if (this.undoStack.length > this.limit) this.undoStack.splice(0, this.undoStack.length - this.limit);
    this.redoStack = [];
  }

  popUndo(): Group | undefined {
    const g = this.undoStack.pop();
    if (g) this.redoStack.push(g);
    return g;
  }

  popRedo(): Group | undefined {
    const g = this.redoStack.pop();
    if (g) this.undoStack.push(g);
    return g;
  }

  toJSON(): HistoryState {
    return { undo: this.undoStack.slice(), redo: this.redoStack.slice() };
  }
}
