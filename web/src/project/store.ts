// The document holder. Commands go in, patches and undo groups come out. No audio or
// storage in here: subscribers (compiler, autosave journal) hang off `subscribe`.

import { commands } from './commands';
import type { CommandArgs, CommandName } from './commands';
import { History } from './history';
import type { Group, HistoryState, JournalEntry } from './history';
import { applyOps } from './patch';
import type { Op } from './patch';
import type { Project } from './types';

type Listener = (doc: Project, entry: JournalEntry) => void;
type Run = <K extends CommandName>(name: K, args: CommandArgs<K>) => void;

export class Store {
  private current: Project;
  readonly history: History;
  private listeners = new Set<Listener>();

  constructor(doc: Project, history?: HistoryState) {
    this.current = doc;
    this.history = new History(history);
  }

  get doc(): Project { return this.current; }

  /** Run a named command as one undo group. Returns false if it changed nothing. Throws on bad args. */
  dispatch<K extends CommandName>(name: K, args: CommandArgs<K>): boolean {
    return this.batch(name, (run) => run(name, args));
  }

  /** Several commands as one undo group (a drag, a take placement). Each command sees the result of the
   *  one before. If any throws, nothing changes. */
  batch(name: string, fn: (run: Run) => void): boolean {
    let working = this.current;
    const ops: Op[] = [];
    const inverses: Op[][] = [];
    const run: Run = (cmd, args) => {
      const make = commands[cmd] as (d: Project, a: unknown) => Op[];
      const o = make(working, args);
      const r = applyOps(working, o);
      if (r.doc === working) return;
      working = r.doc;
      ops.push(...o);
      inverses.push(r.inverse);
    };
    fn(run);
    if (working === this.current) return false;
    const group: Group = { name, ops, inverse: inverses.reverse().flat() };
    this.current = working;
    this.history.push(group);
    this.emit({ kind: 'do', group });
    return true;
  }

  undo(): boolean {
    const g = this.history.popUndo();
    if (!g) return false;
    try {
      this.current = applyOps(this.current, g.inverse).doc;
    } catch (e) {
      this.history.popRedo();
      throw e;
    }
    this.emit({ kind: 'undo', group: g });
    return true;
  }

  redo(): boolean {
    const g = this.history.popRedo();
    if (!g) return false;
    try {
      this.current = applyOps(this.current, g.ops).doc;
    } catch (e) {
      this.history.popUndo();
      throw e;
    }
    this.emit({ kind: 'redo', group: g });
    return true;
  }

  /** Called after every change with the new doc and the journal entry that produced it. */
  subscribe(fn: Listener): () => void {
    this.listeners.add(fn);
    return () => { this.listeners.delete(fn); };
  }

  /** Re-apply a journal entry during crash recovery. Same doc and history as the original action; no subscribers. */
  replay(entry: JournalEntry): void {
    const g = entry.group;
    if (entry.kind === 'do') {
      this.current = applyOps(this.current, g.ops).doc;
      this.history.push(g);
    } else if (entry.kind === 'undo') {
      this.current = applyOps(this.current, g.inverse).doc;
      this.history.popUndo();
    } else {
      this.current = applyOps(this.current, g.ops).doc;
      this.history.popRedo();
    }
  }

  private emit(entry: JournalEntry): void {
    for (const fn of [...this.listeners]) fn(this.current, entry);
  }
}
