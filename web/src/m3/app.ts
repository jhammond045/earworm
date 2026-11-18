// Shared page state: the session, the engine sync, the host, and what the editor has selected.

import type { KlontHost } from '../m1/klont-host';
import { METER_LEN } from '../m1/protocol';
import type { Analysis } from '../m2/analysis';
import type { Project } from '../project/types';
import { Session } from './session';
import { Sync, type SyncResult } from './sync';

export const $ = <T extends HTMLElement>(id: string): T => {
  const e = document.getElementById(id);
  if (!e) throw new Error(`missing #${id}`);
  return e as T;
};

/** Items picked in one lane. Selection is per layer: picking in another lane replaces it. */
export interface Selection { layerId: string; ids: string[] }

export class UiState {
  sectionId = '';
  /** The lane being edited, and the layer a capture goes to. */
  layerId = '';
  selection: Selection = { layerId: '', ids: [] };
  /** Horizontal zoom of the timeline. */
  zoom = 1;
  /** Section index of a queued jump (shown until the transport gets there), or -1. */
  pendingJump = -1;
  private fns = new Set<() => void>();

  subscribe(fn: () => void): () => void { this.fns.add(fn); return () => { this.fns.delete(fn); }; }
  changed(): void { for (const f of [...this.fns]) f(); }

  select(layerId: string, ids: string[]): void {
    this.selection = { layerId, ids };
    this.changed();
  }
  clearSelection(): void {
    if (this.selection.ids.length) this.select(this.selection.layerId, []);
  }

  /** Keep the selected section and layer valid for `doc`. */
  fit(doc: Project): void {
    if (!doc.sections.some((s) => s.id === this.sectionId)) this.sectionId = doc.sections[0]?.id ?? '';
    if (!doc.layers.some((l) => l.id === this.layerId)) this.layerId = doc.layers[0]?.id ?? '';
    if (this.selection.ids.length) {
      const sec = doc.sections.find((s) => s.id === this.sectionId);
      const clip = sec?.clips[this.selection.layerId];
      const have = new Set<string>(clip ? (clip.kind === 'beat' ? clip.hits : clip.notes).map((x) => x.id) : []);
      const ids = this.selection.ids.filter((i) => have.has(i));
      if (ids.length !== this.selection.ids.length) this.selection = { layerId: this.selection.layerId, ids };
    }
  }
}

export class App {
  readonly ui = new UiState();
  readonly sync = new Sync();
  readonly meters = new Float32Array(METER_LEN);
  haveMeters = false;
  /** The engine sync that the newest store change started (the gate reads its timings). */
  lastRequest: Promise<SyncResult> | null = null;
  host: KlontHost | null = null;
  analysis: Analysis | null = null;
  ctx: AudioContext | null = null;
  stream: MediaStream | null = null;
  /** Re-reads the document into every panel. Set by main. */
  refreshAll: () => void = () => {};
  /** Turns the periodic save-on-hide off (the crash check needs a page that does not flush on its way out). */
  stopFlushOnHide: () => void = () => {};

  constructor(readonly session: Session) {}

  get doc(): Project { return this.session.store.doc; }
  get store() { return this.session.store; }

  fail(msg: string): void {
    const e = $('error');
    e.hidden = false;
    e.textContent = msg;
  }
  clearError(): void { $('error').hidden = true; }

  /** Run an edit; a command that rejects its arguments shows its plain message and the panels re-read the document. */
  run(fn: () => unknown): boolean {
    try {
      fn();
      this.clearError();
      return true;
    } catch (e) {
      this.fail(e instanceof Error ? e.message : String(e));
      this.refreshAll();
      return false;
    }
  }

  /** The newest meters, or null before the engine has published any. */
  readMeters(): Float32Array | null {
    if (!this.host) return null;
    this.haveMeters = this.host.readMeters(this.meters) || this.haveMeters;
    return this.haveMeters ? this.meters : null;
  }
}
