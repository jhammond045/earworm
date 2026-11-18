// Shared page state: the session, the engine sync, the host, and what the editor has selected.

import type { KlontHost } from '../m1/klont-host';
import { METER_LEN } from '../m1/protocol';
import type { Analysis } from '../m2/analysis';
import { Matcher } from '../m4/match';
import type { Layer, Project, State } from '../project/types';
import { Session } from './session';
import { Sync, type SyncResult } from './sync';

export const $ = <T extends HTMLElement>(id: string): T => {
  const e = document.getElementById(id);
  if (!e) throw new Error(`missing #${id}`);
  return e as T;
};

/** Items picked in one lane. Selection is per layer: picking in another lane replaces it. */
export interface Selection { layerId: string; ids: string[] }

/** A morph marker picked on the timeline: the lane's layer (in the selected section) and its index in the clip's morph list. */
export interface MarkerSel { layerId: string; index: number }

export class UiState {
  sectionId = '';
  /** The lane being edited, and the layer a capture goes to. */
  layerId = '';
  selection: Selection = { layerId: '', ids: [] };
  /** The picked morph marker, if any. Picking a marker clears the note selection and the other way round. */
  marker: MarkerSel | null = null;
  /** The state each layer's sound controls edit (layer id -> state id). A missing or stale entry means the first state. */
  private editing: Record<string, string> = {};
  /** Horizontal zoom of the timeline. */
  zoom = 1;
  /** Section index of a queued jump (shown until the transport gets there), or -1. */
  pendingJump = -1;
  private fns = new Set<() => void>();

  subscribe(fn: () => void): () => void { this.fns.add(fn); return () => { this.fns.delete(fn); }; }
  changed(): void { for (const f of [...this.fns]) f(); }

  select(layerId: string, ids: string[]): void {
    this.selection = { layerId, ids };
    if (ids.length) this.marker = null;
    this.changed();
  }

  selectMarker(m: MarkerSel | null): void {
    this.marker = m;
    if (m) this.selection = { layerId: m.layerId, ids: [] };
    this.changed();
  }

  /** The state of `layer` that the sound preset, macros and Influence amount edit. */
  editState(layer: Layer): State {
    return layer.states.find((s) => s.id === this.editing[layer.id]) ?? layer.states[0];
  }
  setEditState(layer: Layer, stateId: string): void {
    this.editing[layer.id] = stateId;
    this.changed();
  }
  clearSelection(): void {
    if (this.selection.ids.length) this.select(this.selection.layerId, []);
  }

  /** Keep the selected section and layer valid for `doc`. */
  fit(doc: Project): void {
    if (!doc.sections.some((s) => s.id === this.sectionId)) this.sectionId = doc.sections[0]?.id ?? '';
    if (!doc.layers.some((l) => l.id === this.layerId)) this.layerId = doc.layers[0]?.id ?? '';
    if (this.marker) {
      const m = doc.sections.find((x) => x.id === this.sectionId)?.clips[this.marker.layerId]?.morph;
      if (!m || this.marker.index >= m.length) this.marker = null;
    }
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
  private matcherP: Promise<Matcher> | null = null;
  /** The match worker, started on first use. A failure is not cached, so the next try starts it again. */
  matcher(): Promise<Matcher> {
    this.matcherP ??= Matcher.create().catch((e) => { this.matcherP = null; throw e; });
    return this.matcherP;
  }
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

  /** Run an edit that a slider or drag was previewing. When it changed nothing the document is the same object, so the
   *  engine (which heard the preview) is put back on it by hand. */
  runRestoring(fn: () => unknown): boolean {
    const before = this.doc;
    const ok = this.run(fn);
    if (this.doc === before) void this.sync.request(this.doc);
    return ok;
  }

  /** Let the engine hear `make(doc)` without touching the document. A bad value is left for the commit to report. */
  previewDoc(make: (doc: Project) => Project): void {
    try { void this.sync.request(make(this.doc)); } catch { /* the commit on release reports it */ }
  }

  /** The newest meters, or null before the engine has published any. */
  readMeters(): Float32Array | null {
    if (!this.host) return null;
    this.haveMeters = this.host.readMeters(this.meters) || this.haveMeters;
    return this.haveMeters ? this.meters : null;
  }
}
