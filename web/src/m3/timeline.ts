// The timeline: one canvas for the selected section, one lane per layer, and the pointer and
// keyboard editing on it.
//
// Drawing is Canvas 2D. The TDD asks for one WebGL2 renderer with retained geometry; M3 uses
// Canvas 2D because a section has at most 8 lanes and a few hundred items, and the point of this
// milestone is the edit path, not the renderer. The retained-geometry part is kept: ribbon
// outlines are computed once per clip, zoom and pitch range and reused until one of them changes.
//
// Dragging never touches the document. Each move builds the changed document with applyOps and
// sends only the track to the engine (through Sync); the single undo group is committed on release.

import { readColors, fitCanvas, type Colors } from '../live/ribbon';
import { M_PLAYING, M_SECTION, M_SECTION_POS } from '../m1/protocol';
import { SLOT_COLORS, SLOT_NAMES } from '../m2/sounds';
import { commands, newId } from '../project/commands';
import type { CommandArgs, CommandName } from '../project/commands';
import { applyOps } from '../project/patch';
import { sectionLength, sectionUnitsPerSec } from '../project/compile';
import { KIT_SLOTS, PPQ, type Clip, type Hit, type Layer, type Project, type Section } from '../project/types';
import { $, type App } from './app';
import {
  centsToY, dragCents, dragDt, dragMarkerT, fitPpu, gridLines, hitGlyph, hitTestHits, hitTestMarkers, hitTestNotes, mixColor, morphAt, MORPH_H,
  newItemDur, noteRect, pitchRange, resizeDur, ribbonRuns, shapeProgress, slotRow, slotRowH, slotY, snapChoices, snapTo, stateColor, timeToX,
  warpSlope, xToTime, yToCents, TUNED,
  type Range, type RibbonPoint, type Viewport,
} from './geometry';

const RULER_H = 22;
const LANE_H = 96;
/** One lane and the morph strip under it. */
const STRIDE = LANE_H + MORPH_H;
const X0 = 88;
const DRAG_PX = 3;

interface Step { name: CommandName; args: unknown }

interface Drag {
  kind: 'move' | 'resize' | 'marker';
  /** Marker drags: the marker's place in the clip's list in the previewed document (the list re-sorts as it passes others). */
  markerIndex: number;
  /** Marker drags: the marker's place in the list of the document the drag started from. */
  markerIndex0: number;
  /** Marker drags: the dragged marker's state, to find it again after the list re-sorts. */
  markerState: string;
  layerId: string;
  layer: Layer;
  startDoc: Project;
  section: Section;
  clip: Clip;
  primary: string;
  ids: string[];
  startX: number;
  startY: number;
  moved: boolean;
  range: Range;
  laneTop: number;
  octave: number;
  label: string;
}

interface RibbonCache { key: string; runs: RibbonPoint[][][] }

const hex = (c: number) => `#${c.toString(16).padStart(6, '0')}`;
const rgba = (c: number, a: number) => `rgba(${(c >> 16) & 255},${(c >> 8) & 255},${c & 255},${a})`;
const DEFAULT_SHAPE = { attack: 0.004, burst: 0.5, cents: null, bend: 0, hold: 0.06, vowel: 0.5, tone: 0.5, endStop: 0, endClick: 0 };

export class Timeline {
  private readonly canvas = $<HTMLCanvasElement>('timeline');
  private readonly scroller = $('tlscroll');
  private col: Colors = { fg: '#000', muted: '#666', line: '#ccc', accent: '#27c' };
  private colAt = 0;
  private dirty = true;
  private drag: Drag | null = null;
  /** The document with the drag applied (never committed), shown and played while dragging. */
  private preview: Project | null = null;
  private ranges = new Map<string, Range>();
  private rangeSection = '';
  private ribbons = new WeakMap<Clip, RibbonCache>();
  private arrayIds = new WeakMap<object, number>();
  private nextArrayId = 1;
  /** Snap choice index for new clips; a clip with its own timeSnap shows that one. */
  private snapIdx = 0;
  private vp: Viewport = { ppu: 1, x0: X0 };

  constructor(private readonly app: App) {
    const c = this.canvas;
    c.addEventListener('pointerdown', (e) => this.onDown(e));
    c.addEventListener('pointermove', (e) => this.onMove(e));
    c.addEventListener('pointerup', (e) => this.onUp(e));
    c.addEventListener('pointercancel', () => this.cancelDrag());
    c.addEventListener('dblclick', (e) => this.onDouble(e));
    c.addEventListener('keydown', (e) => this.onKeyCanvas(e));
    window.addEventListener('keydown', (e) => this.onKeyGlobal(e));
    window.addEventListener('resize', () => { this.dirty = true; });
    new ResizeObserver(() => { this.dirty = true; }).observe(this.scroller);
    app.ui.subscribe(() => { this.dirty = true; });

    $<HTMLSelectElement>('zoom').addEventListener('change', (e) => {
      app.ui.zoom = Number((e.target as HTMLSelectElement).value);
      this.dirty = true;
    });
    $<HTMLSelectElement>('snap').addEventListener('change', (e) => this.onSnap(Number((e.target as HTMLSelectElement).value)));
    $('quantize').addEventListener('click', () => this.quantize());
    $<HTMLInputElement>('loopLen').addEventListener('change', (e) => this.onLoopLen(Number((e.target as HTMLInputElement).value)));
  }

  invalidate(): void { this.dirty = true; }

  // ---------------------------------------------------------------- the document being shown

  private get doc(): Project { return this.preview ?? this.app.doc; }
  private get section(): Section | undefined { return this.doc.sections.find((s) => s.id === this.app.ui.sectionId); }
  private activeLayer(doc = this.doc): Layer | undefined { return doc.layers.find((l) => l.id === this.app.ui.layerId); }

  private viewLen(s: Section, doc: Project): number {
    let len = sectionLength(s);
    for (const l of doc.layers) len = Math.max(len, s.clips[l.id]?.loopLen ?? 0);
    return len;
  }

  private laneTop(i: number): number { return RULER_H + i * STRIDE; }
  /** The lane under y, or -1 above the lanes. `morph` is true in the strip under the lane's notes. */
  private zoneAt(y: number, doc: Project): { li: number; morph: boolean } {
    const i = Math.floor((y - RULER_H) / STRIDE);
    if (y < RULER_H || i >= doc.layers.length) return { li: -1, morph: false };
    return { li: i, morph: y - this.laneTop(i) >= LANE_H };
  }

  // ---------------------------------------------------------------- toolbar

  /** Update the snap, loop length and quantize controls for the active lane. */
  refreshToolbar(doc: Project): void {
    const s = doc.sections.find((x) => x.id === this.app.ui.sectionId);
    const sel = $<HTMLSelectElement>('snap');
    const feel = s?.feel ?? 'grid';
    const choices = snapChoices(feel);
    if (sel.dataset.feel !== feel) {
      sel.dataset.feel = feel;
      sel.replaceChildren(...choices.map((c, i) => new Option(c.label, String(i))));
    }
    const lay = this.activeLayer(doc);
    const clip = s && lay ? s.clips[lay.id] : undefined;
    if (document.activeElement !== sel) {
      const at = clip ? choices.findIndex((c) => c.value === clip.timeSnap) : this.snapIdx;
      sel.value = String(at >= 0 ? at : 0);
    }
    const len = $<HTMLInputElement>('loopLen');
    len.disabled = !clip;
    if (document.activeElement !== len) {
      len.value = clip ? String(feel === 'grid' ? Math.round((clip.loopLen / PPQ) * 1000) / 1000 : clip.loopLen) : '';
    }
    $('loopLen').setAttribute('aria-label', `Loop length of the active clip, in ${feel === 'grid' ? 'beats' : 'seconds'}`);
    ($('quantize') as HTMLButtonElement).disabled = !clip;
    this.dirty = true;
  }

  private onSnap(idx: number): void {
    const s = this.section;
    const lay = this.activeLayer();
    if (!s) return;
    this.snapIdx = idx;
    const value = snapChoices(s.feel)[idx]?.value ?? 0;
    if (lay && s.clips[lay.id]) this.app.run(() => this.app.store.dispatch('setClipFields', { section: s.id, layer: lay.id, timeSnap: value }));
  }

  private onLoopLen(v: number): void {
    const s = this.section;
    const lay = this.activeLayer();
    if (!s || !lay || !Number.isFinite(v) || v <= 0) { this.app.refreshAll(); return; }
    this.app.run(() => this.app.store.dispatch('setClipFields', { section: s.id, layer: lay.id, loopLen: s.feel === 'grid' ? v * PPQ : v }));
  }

  private quantize(): void {
    const s = this.section;
    const lay = this.activeLayer();
    if (!s || !lay) return;
    const clip = s.clips[lay.id];
    if (!clip) { this.app.fail('That lane has no clip to quantize.'); return; }
    const grid = clip.timeSnap > 0 ? clip.timeSnap : s.feel === 'grid' ? PPQ / 4 : 0.25;
    const ids = this.app.ui.selection.layerId === lay.id && this.app.ui.selection.ids.length ? this.app.ui.selection.ids : undefined;
    this.app.run(() => this.app.store.dispatch('quantize', { section: s.id, layer: lay.id, grid, ids }));
  }

  // ---------------------------------------------------------------- pointer

  private pos(e: PointerEvent | MouseEvent): { x: number; y: number } {
    const r = this.canvas.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  }

  private rangeFor(layer: Layer, clip: Clip | undefined, doc: Project): Range {
    let r = this.ranges.get(layer.id);
    if (clip && clip.kind !== 'beat' && !this.drag) {
      const oct = 1200 * layer.tune.octave;
      const need: number[] = clip.notes.map((n) => n.cents + oct);
      const frames = this.curveOf(clip, layer, doc);
      if (frames) for (let i = 0; i < frames.length; i += 25 * 5) need.push(frames[i + TUNED]);
      const inside = (c: number) => !!r && c >= r.lo + 100 && c <= r.hi - 100;
      if (!r || !need.every((c) => !Number.isFinite(c) || inside(c))) r = pitchRange(need, r);
      this.ranges.set(layer.id, r);
    }
    return r ?? { lo: 4800, hi: 7200 };
  }

  private curveOf(clip: Clip, layer: Layer, doc: Project): Float32Array | null {
    if (clip.kind === 'beat' || !clip.take) return null;
    const take = doc.takes[clip.take];
    return take ? this.app.sync.cachedCurve(take, layer) : null;
  }

  /** Where the dragged marker sits in `doc`'s list: the one with the dragged state at the time the last step gave it. */
  private markerIndexIn(doc: Project, d: Drag, steps: Step[]): number {
    const t = (steps[0]?.args as { t?: number } | undefined)?.t;
    const list = doc.sections.find((x) => x.id === d.section.id)?.clips[d.layerId]?.morph ?? [];
    const i = list.findIndex((m) => m.t === t && m.state === d.markerState);
    return i >= 0 ? i : d.markerIndex0;
  }

  /** A press in the morph strip under a lane: pick a marker and start dragging it, or clear the marker selection. */
  private onDownMorph(e: PointerEvent, layer: Layer, clip: Clip | undefined, top: number, x: number, y: number): void {
    const s = this.section;
    if (!s) return;
    const ui = this.app.ui;
    const i = clip ? hitTestMarkers(clip.morph, x, y, this.vp, top + LANE_H) : -1;
    if (i < 0) { ui.marker = null; ui.select(layer.id, []); return; }
    ui.selectMarker({ layerId: layer.id, index: i });
    try { this.canvas.setPointerCapture(e.pointerId); } catch { /* a synthetic pointer: no capture */ }
    this.drag = {
      kind: 'marker', markerIndex: i, markerIndex0: i, markerState: clip!.morph[i].state, layerId: layer.id, layer, startDoc: this.doc, section: s,
      clip: clip!, primary: '', ids: [], startX: x, startY: y, moved: false, range: { lo: 4800, hi: 7200 }, laneTop: top, octave: 0, label: 'Move morph marker',
    };
  }

  private onDown(e: PointerEvent): void {
    if (e.button !== 0) return;
    const { x, y } = this.pos(e);
    const doc = this.doc;
    const s = this.section;
    if (!s) return;
    this.canvas.focus();
    const { li, morph } = this.zoneAt(y, doc);
    if (li < 0) {
      // The ruler: move the transport there.
      const t = Math.max(0, xToTime(x, this.vp));
      const si = doc.sections.findIndex((q) => q.id === s.id);
      if (this.app.host && si >= 0) this.app.host.seek(si, t);
      return;
    }
    const layer = doc.layers[li];
    if (this.app.ui.layerId !== layer.id) { this.app.ui.layerId = layer.id; this.app.ui.changed(); this.app.refreshAll(); }
    const clip = s.clips[layer.id];
    const top = this.laneTop(li);
    if (morph) { this.onDownMorph(e, layer, clip, top, x, y); return; }
    let id: string | null = null;
    let part: 'body' | 'end' = 'body';
    if (clip) {
      if (clip.kind === 'beat') {
        id = hitTestHits(clip.hits, x, y, this.vp, top, LANE_H, sectionUnitsPerSec(s));
      } else {
        const h = hitTestNotes(this.displayNotes(clip, layer), x, y, this.vp, this.rangeFor(layer, clip, doc), top, LANE_H);
        if (h) { id = h.id; part = h.part; }
      }
    }
    const sel = this.app.ui.selection;
    const cur = sel.layerId === layer.id ? sel.ids : [];
    if (!id) {
      if (!e.shiftKey) { this.app.ui.marker = null; this.app.ui.select(layer.id, []); }
      return;
    }
    let ids: string[];
    if (e.shiftKey) {
      if (cur.includes(id)) { this.app.ui.select(layer.id, cur.filter((i) => i !== id)); return; }
      ids = [...cur, id];
    } else ids = cur.includes(id) ? cur : [id];
    this.app.ui.select(layer.id, ids);
    try { this.canvas.setPointerCapture(e.pointerId); } catch { /* a pointer that is not active (synthetic events): drag without capture */ }
    this.drag = {
      markerIndex: -1, markerIndex0: -1, markerState: '',
      kind: part === 'end' && clip!.kind !== 'beat' ? 'resize' : 'move', layerId: layer.id, layer, startDoc: doc, section: s, clip: clip!, primary: id, ids,
      startX: x, startY: y, moved: false, range: this.rangeFor(layer, clip, doc), laneTop: top, octave: layer.tune.octave,
      label: clip!.kind === 'beat' ? 'Move hits' : part === 'end' ? 'Resize note' : 'Move notes',
    };
  }

  private displayNotes(clip: Clip, layer: Layer): { id: string; t: number; dur: number; cents: number }[] {
    if (clip.kind === 'beat') return [];
    const oct = 1200 * layer.tune.octave;
    return oct === 0 ? clip.notes : clip.notes.map((n) => ({ id: n.id, t: n.t, dur: n.dur, cents: n.cents + oct }));
  }

  private stepsFor(d: Drag, x: number, y: number, alt: boolean): Step[] {
    const s = d.section;
    if (d.kind === 'marker') {
      const m = d.clip.morph[d.markerIndex0];
      if (!m) return [];
      const t = dragMarkerT(m.t, (x - d.startX) / this.vp.ppu, d.clip.timeSnap, d.clip.loopLen, s.feel === 'grid', alt);
      return [{ name: 'moveMorphMarker', args: { section: s.id, layer: d.layerId, index: d.markerIndex0, t } }];
    }
    const rawDt = (x - d.startX) / this.vp.ppu;
    const snap = d.clip.timeSnap;
    const grid = s.feel === 'grid';
    const rnd = (v: number) => (grid ? Math.round(v) : v);
    if (d.clip.kind === 'beat') {
      const p = d.clip.hits.find((h) => h.id === d.primary);
      if (!p) return [];
      const steps: Step[] = [{ name: 'moveHits', args: { section: s.id, layer: d.layerId, ids: d.ids, dt: rnd(dragDt(p.t, rawDt, snap, alt)) } }];
      const row = slotRow(y, d.laneTop, LANE_H);
      if (row !== p.slot && Math.abs(y - d.startY) >= DRAG_PX) steps.push({ name: 'setHitSlot', args: { section: s.id, layer: d.layerId, ids: d.ids, slot: row } });
      return steps;
    }
    const p = d.clip.notes.find((n) => n.id === d.primary);
    if (!p) return [];
    if (d.kind === 'resize') {
      return [{ name: 'resizeNote', args: { section: s.id, layer: d.layerId, id: p.id, dur: rnd(resizeDur(p.t, p.dur, rawDt, snap, grid ? 1 : 0.01, alt)) } }];
    }
    const rawDc = yToCents(y, d.range, d.laneTop, LANE_H) - yToCents(d.startY, d.range, d.laneTop, LANE_H);
    return [{ name: 'moveNotes', args: { section: s.id, layer: d.layerId, ids: d.ids, dt: rnd(dragDt(p.t, rawDt, snap, alt)), dcents: dragCents(p.cents, rawDc, alt) } }];
  }

  private build(doc: Project, steps: Step[]): Project {
    let d = doc;
    for (const st of steps) d = applyOps(d, (commands[st.name] as (x: Project, a: unknown) => ReturnType<typeof commands.addNote>)(d, st.args)).doc;
    return d;
  }

  private onMove(e: PointerEvent): void {
    const d = this.drag;
    if (!d) return;
    const { x, y } = this.pos(e);
    if (!d.moved && Math.hypot(x - d.startX, y - d.startY) < DRAG_PX) return;
    d.moved = true;
    try {
      const steps = this.stepsFor(d, x, y, e.altKey);
      this.preview = this.build(d.startDoc, steps);
      if (d.kind === 'marker') d.markerIndex = this.markerIndexIn(this.preview, d, steps);
    } catch (err) {
      this.app.fail(err instanceof Error ? err.message : String(err));
      return;
    }
    void this.app.sync.request(this.preview);
    this.dirty = true;
  }

  private onUp(e: PointerEvent): void {
    const d = this.drag;
    if (!d) return;
    this.drag = null;
    if (this.canvas.hasPointerCapture(e.pointerId)) this.canvas.releasePointerCapture(e.pointerId);
    if (!d.moved) return;
    const { x, y } = this.pos(e);
    const steps = this.stepsFor(d, x, y, e.altKey);
    this.preview = null;
    const store = this.app.store;
    let changed = false;
    this.app.run(() => { changed = store.batch(d.label, (run) => { for (const st of steps) run(st.name, st.args as never); }); });
    if (d.kind === 'marker') this.app.ui.selectMarker({ layerId: d.layerId, index: this.markerIndexIn(this.app.doc, d, steps) });
    // Nothing committed: put the engine back to the real document.
    if (!changed) void this.app.sync.request(this.app.doc);
    this.dirty = true;
  }

  private cancelDrag(): void {
    if (!this.drag) return;
    const moved = this.drag.moved;
    this.drag = null;
    this.preview = null;
    if (moved) void this.app.sync.request(this.app.doc);
    this.dirty = true;
  }

  private onDouble(e: MouseEvent): void {
    const { x, y } = this.pos(e);
    const doc = this.app.doc;
    const s = this.section;
    const { li, morph } = this.zoneAt(y, doc);
    if (!s || li < 0) return;
    const layer = doc.layers[li];
    const clip = s.clips[layer.id];
    const top = this.laneTop(li);
    if (morph) { this.addMarkerAtX(s, layer, clip, top, x, e.altKey); return; }
    if (clip) {
      const onItem = clip.kind === 'beat'
        ? hitTestHits(clip.hits, x, y, this.vp, top, LANE_H, sectionUnitsPerSec(s))
        : hitTestNotes(this.displayNotes(clip, layer), x, y, this.vp, this.rangeFor(layer, clip, doc), top, LANE_H);
      if (onItem) return;
    }
    const snap = clip ? clip.timeSnap : (snapChoices(s.feel)[this.snapIdx]?.value ?? 0);
    const t = Math.max(0, snapTo(xToTime(x, this.vp), e.altKey ? 0 : snap));
    const cents = layer.kind === 'beat' ? 0 : Math.max(0, Math.round((yToCents(y, this.rangeFor(layer, clip, doc), top, LANE_H) - 1200 * layer.tune.octave) / 100) * 100);
    this.addAt(s, layer, clip, t, cents, slotRow(y, top, LANE_H), snap);
  }

  /** Add a morph marker for the layer's editing state at the pointer, snapped like a note, and select it. */
  private addMarkerAtX(s: Section, layer: Layer, clip: Clip | undefined, top: number, x: number, free: boolean): void {
    if (clip && hitTestMarkers(clip.morph, x, top + LANE_H + 1, this.vp, top + LANE_H) >= 0) return;
    const raw = xToTime(x, this.vp);
    const len = clip ? clip.loopLen : sectionLength(s);
    if (raw < 0 || raw >= len) return;
    const snap = clip ? clip.timeSnap : (snapChoices(s.feel)[this.snapIdx]?.value ?? 0);
    this.addMarker(s, layer, dragMarkerT(0, raw, snap, len, s.feel === 'grid', free));
  }

  /** The new marker's state is the layer's editing state; it eases in over half a second. */
  addMarker(s: Section, layer: Layer, t: number): void {
    const state = this.app.ui.editState(layer).id;
    const ok = this.app.run(() => this.app.store.dispatch('addMorphMarker', { section: s.id, layer: layer.id, marker: { t, state, lenMs: 500, shape: 'ease' } }));
    if (!ok) return;
    const list = this.app.doc.sections.find((q) => q.id === s.id)?.clips[layer.id]?.morph ?? [];
    this.app.ui.selectMarker({ layerId: layer.id, index: Math.max(0, list.findIndex((m) => m.t === t && m.state === state)) });
  }

  /** Add a note (melody, texture) or a hit (beat) and select it. A new clip gets the snap shown in the toolbar. */
  private addAt(s: Section, layer: Layer, clip: Clip | undefined, t: number, cents: number, slot: number, snap: number): void {
    const dur = newItemDur(s.feel, snap);
    const id = newId(layer.kind === 'beat' ? 'hit' : 'note');
    const tt = s.feel === 'grid' ? Math.round(t) : t;
    const ok = this.app.run(() => this.app.store.batch(layer.kind === 'beat' ? 'Add hit' : 'Add note', (run) => {
      if (layer.kind === 'beat') {
        run('addHit', { section: s.id, layer: layer.id, hit: { id, t: tt, dur, slot, strength: 0.8, shape: { ...DEFAULT_SHAPE } } });
      } else {
        run('addNote', { section: s.id, layer: layer.id, note: { id, t: tt, dur, cents, hard: 0.5 } });
      }
      if (!clip && snap > 0) run('setClipFields', { section: s.id, layer: layer.id, timeSnap: snap });
    }));
    if (ok) this.app.ui.select(layer.id, [id]);
  }

  // ---------------------------------------------------------------- keyboard

  private onKeyGlobal(e: KeyboardEvent): void {
    if (e.key !== 'Delete' && e.key !== 'Backspace') return;
    const t = e.target as HTMLElement | null;
    if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.tagName === 'SELECT' || t.isContentEditable)) return;
    if (this.deleteSelection()) e.preventDefault();
  }

  private deleteSelection(): boolean {
    const mk = this.app.ui.marker;
    const sec = this.section;
    if (mk && sec) {
      this.app.run(() => this.app.store.dispatch('removeMorphMarker', { section: sec.id, layer: mk.layerId, index: mk.index }));
      this.app.ui.selectMarker(null);
      return true;
    }
    const sel = this.app.ui.selection;
    const s = this.section;
    if (!s || !sel.ids.length) return false;
    const clip = s.clips[sel.layerId];
    if (!clip) return false;
    const args = { section: s.id, layer: sel.layerId, ids: sel.ids };
    this.app.run(() => (clip.kind === 'beat' ? this.app.store.dispatch('deleteHits', args) : this.app.store.dispatch('deleteNotes', args)));
    this.app.ui.select(sel.layerId, []);
    return true;
  }

  private onKeyCanvas(e: KeyboardEvent): void {
    const s = this.section;
    const lay = this.activeLayer();
    if (!s || !lay) return;
    const clip = s.clips[lay.id];
    const sel = this.app.ui.selection;
    const ids = sel.layerId === lay.id ? sel.ids : [];
    const step = clip && clip.timeSnap > 0 ? clip.timeSnap : s.feel === 'grid' ? PPQ / 4 : 0.1;
    const items = clip ? (clip.kind === 'beat' ? clip.hits : clip.notes).slice().sort((a, b) => a.t - b.t) : [];

    // Morph markers: M adds one for the editing state at the playhead, [ and ] pick the previous or next, arrows move the picked one.
    if ((e.key === 'm' || e.key === 'M') && !e.ctrlKey && !e.metaKey) {
      const m = this.app.readMeters();
      const si = this.app.doc.sections.findIndex((q) => q.id === s.id);
      const len = clip?.loopLen ?? sectionLength(s);
      const raw = m && m[M_PLAYING] === 1 && m[M_SECTION] === si ? m[M_SECTION_POS] % len : 0;
      const snap = clip ? clip.timeSnap : (snapChoices(s.feel)[this.snapIdx]?.value ?? 0);
      this.addMarker(s, lay, dragMarkerT(0, raw, snap, len, s.feel === 'grid'));
      e.preventDefault();
      return;
    }
    const marks = clip?.morph ?? [];
    if ((e.key === '[' || e.key === ']') && marks.length) {
      const cur = this.app.ui.marker?.layerId === lay.id ? this.app.ui.marker.index : -1;
      const next = e.key === ']' ? (cur + 1) % marks.length : cur <= 0 ? marks.length - 1 : cur - 1;
      this.app.ui.selectMarker({ layerId: lay.id, index: next });
      e.preventDefault();
      return;
    }
    const mk = this.app.ui.marker;
    if (mk && mk.layerId === lay.id && clip && (e.key === 'ArrowLeft' || e.key === 'ArrowRight')) {
      const old = marks[mk.index];
      if (!old) return;
      e.preventDefault();
      const t = dragMarkerT(old.t, (e.key === 'ArrowLeft' ? -1 : 1) * step, 0, clip.loopLen, s.feel === 'grid', true);
      if (this.app.run(() => this.app.store.dispatch('moveMorphMarker', { section: s.id, layer: lay.id, index: mk.index, t }))) {
        const at = this.app.doc.sections.find((q) => q.id === s.id)?.clips[lay.id]?.morph.findIndex((x) => x.t === t && x.state === old.state) ?? -1;
        this.app.ui.selectMarker({ layerId: lay.id, index: Math.max(0, at) });
      }
      return;
    }

    if (e.key === ',' || e.key === '.') {
      if (!items.length) return;
      const at = items.findIndex((i) => i.id === ids[ids.length - 1]);
      const next = e.key === '.' ? (at + 1) % items.length : (at <= 0 ? items.length - 1 : at - 1);
      this.app.ui.select(lay.id, [items[next].id]);
      e.preventDefault();
      return;
    }
    if (e.key === 'a' && !e.ctrlKey && !e.metaKey) {
      const m = this.app.readMeters();
      const si = this.app.doc.sections.findIndex((q) => q.id === s.id);
      const t = m && m[M_PLAYING] === 1 && m[M_SECTION] === si ? m[M_SECTION_POS] % (clip?.loopLen ?? sectionLength(s)) : 0;
      const snap = clip ? clip.timeSnap : (snapChoices(s.feel)[this.snapIdx]?.value ?? 0);
      this.addAt(s, lay, clip, snapTo(t, snap), 6000, 0, snap);
      e.preventDefault();
      return;
    }
    if (!clip || !ids.length || !e.key.startsWith('Arrow')) return;
    e.preventDefault();
    const dir = e.key === 'ArrowLeft' ? -1 : e.key === 'ArrowRight' ? 1 : e.key === 'ArrowUp' ? 1 : -1;
    const horizontal = e.key === 'ArrowLeft' || e.key === 'ArrowRight';
    const base = { section: s.id, layer: lay.id };
    this.app.run(() => {
      if (clip.kind === 'beat') {
        if (horizontal) this.app.store.dispatch('moveHits', { ...base, ids, dt: dir * step });
        else {
          const first = clip.hits.find((h) => h.id === ids[0]);
          if (first) this.app.store.dispatch('setHitSlot', { ...base, ids, slot: Math.max(0, Math.min(KIT_SLOTS - 1, first.slot - dir)) });
        }
      } else if (horizontal && e.shiftKey) {
        const min = s.feel === 'grid' ? 1 : 0.01;
        this.app.store.batch('Resize notes', (run) => {
          for (const n of clip.notes) if (ids.includes(n.id)) run('resizeNote', { ...base, id: n.id, dur: Math.max(min, n.dur + dir * step) });
        });
      } else if (horizontal) this.app.store.dispatch('moveNotes', { ...base, ids, dt: dir * step, dcents: 0 });
      else this.app.store.dispatch('moveNotes', { ...base, ids, dt: 0, dcents: dir * 100 });
    });
  }

  // ---------------------------------------------------------------- drawing

  /** Called every animation frame. Draws when something changed or the transport is moving. */
  frame(): void {
    const m = this.app.readMeters();
    const playing = !!m && m[M_PLAYING] === 1;
    if (!this.dirty && !playing) return;
    this.dirty = false;
    this.draw(m);
  }

  private geometryKey(clip: Clip, curve: Float32Array | null, range: Range, ppu: number, top: number): string {
    let id = 0;
    if (curve) {
      id = this.arrayIds.get(curve) ?? 0;
      if (!id) { id = this.nextArrayId++; this.arrayIds.set(curve, id); }
    }
    return `${top}|${ppu.toFixed(5)}|${range.lo}|${range.hi}|${id}|${clip.kind === 'beat' ? '' : clip.warp.length}`;
  }

  private draw(m: Float32Array | null): void {
    const doc = this.doc;
    const s = this.section;
    const c = this.canvas;
    if (!s) return;
    if (performance.now() - this.colAt > 1000) { this.col = readColors(); this.colAt = performance.now(); }
    const col = this.col;
    if (this.rangeSection !== s.id) { this.ranges.clear(); this.rangeSection = s.id; }

    const len = this.viewLen(s, doc);
    const avail = this.scroller.clientWidth || 600;
    const ppu = fitPpu(avail, len, X0, this.app.ui.zoom);
    this.vp = { ppu, x0: X0 };
    const vp = this.vp;
    const width = Math.ceil(X0 + len * ppu + 12);
    const height = RULER_H + Math.max(1, doc.layers.length) * STRIDE;
    c.style.width = `${width}px`;
    c.style.height = `${height}px`;
    const { g } = fitCanvas(c);
    g.clearRect(0, 0, width, height);
    g.font = '11px ui-monospace, Menlo, monospace';
    g.textBaseline = 'middle';

    // Ruler and grid.
    const lines = gridLines(s.feel, len);
    g.strokeStyle = col.line;
    g.fillStyle = col.muted;
    lines.forEach((l, i) => {
      const x = Math.round(timeToX(l.t, vp)) + 0.5;
      g.globalAlpha = l.strong ? 1 : 0.5;
      g.beginPath(); g.moveTo(x, l.strong ? 0 : RULER_H - 6); g.lineTo(x, height); g.stroke();
      g.globalAlpha = 1;
      if (l.strong && ppu * (s.feel === 'grid' ? PPQ : 1) * 4 > 28) g.fillText(s.feel === 'grid' ? String(i / 4 + 1) : `${l.t}s`, x + 3, 9);
    });
    g.beginPath(); g.moveTo(0, RULER_H + 0.5); g.lineTo(width, RULER_H + 0.5); g.stroke();

    const upsec = sectionUnitsPerSec(s);
    const si = doc.sections.findIndex((q) => q.id === s.id);
    doc.layers.forEach((layer, li) => {
      const top = this.laneTop(li);
      const clip = s.clips[layer.id];
      const active = layer.id === this.app.ui.layerId;
      g.strokeStyle = col.line;
      g.beginPath(); g.moveTo(0, top + STRIDE + 0.5); g.lineTo(width, top + STRIDE + 0.5); g.stroke();
      g.globalAlpha = 0.5;
      g.beginPath(); g.moveTo(0, top + LANE_H + 0.5); g.lineTo(width, top + LANE_H + 0.5); g.stroke();
      g.globalAlpha = 1;
      this.drawMorph(g, layer, clip, top, s, vp, width, col);
      if (active) { g.fillStyle = col.accent; g.fillRect(0, top, 3, STRIDE); }
      g.fillStyle = layer.mute ? col.muted : col.fg;
      g.fillText(layer.name.slice(0, 11), 8, top + 11);
      g.fillStyle = col.muted;
      g.fillText(layer.kind, 8, top + 25);

      g.fillStyle = col.muted;
      g.fillText('morph', 8, top + LANE_H + MORPH_H / 2);
      if (!clip) {
        g.fillStyle = col.muted;
        g.fillText('empty: double-click to add', X0 + 8, top + LANE_H / 2);
        return;
      }
      g.save();
      g.beginPath(); g.rect(X0, top + 1, width - X0, LANE_H - 1); g.clip();
      // The clip's own loop end.
      const endX = timeToX(clip.loopLen, vp);
      g.fillStyle = col.line;
      g.globalAlpha = 0.35;
      if (endX < width) g.fillRect(endX, top, width - endX, LANE_H);
      g.globalAlpha = 1;
      const selIds = this.app.ui.selection.layerId === layer.id ? new Set(this.app.ui.selection.ids) : new Set<string>();

      if (clip.kind === 'beat') {
        for (let k = 0; k < KIT_SLOTS; k++) {
          const y = slotY(k, top, LANE_H);
          g.strokeStyle = col.line; g.globalAlpha = 0.6;
          g.beginPath(); g.moveTo(X0, y + 0.5); g.lineTo(width, y + 0.5); g.stroke();
          g.globalAlpha = 1;
          g.fillStyle = col.muted;
          g.fillText(SLOT_NAMES[k], X0 + 3, y - slotRowH(LANE_H) / 2 + 7);
        }
        for (const h of clip.hits) this.drawHit(g, h, vp, top, upsec, selIds.has(h.id), col);
      } else {
        const range = this.rangeFor(layer, clip, doc);
        const curve = this.curveOf(clip, layer, doc);
        const key = this.geometryKey(clip, curve, range, ppu, top);
        let cache = this.ribbons.get(clip);
        if (!cache || cache.key !== key) {
          const slope = warpSlope(s.feel, s.bpm);
          const runs = curve
            ? clip.notes.map((n) => ribbonRuns({ t: n.t, dur: n.dur, anchors: clip.warp, slope, frames: curve, vp, range, top, h: LANE_H, maxHalf: 14 }))
            : [];
          cache = { key, runs };
          this.ribbons.set(clip, cache);
        }
        g.fillStyle = rgba(layer.color, 0.42);
        for (const noteRuns of cache.runs) {
          for (const run of noteRuns) {
            if (run.length < 2) continue;
            g.beginPath();
            g.moveTo(run[0].x, run[0].y - run[0].hw);
            for (const p of run) g.lineTo(p.x, p.y - p.hw);
            for (let i = run.length - 1; i >= 0; i--) g.lineTo(run[i].x, run[i].y + run[i].hw);
            g.closePath();
            g.fill();
          }
        }
        // Pitch grid: C lines.
        g.fillStyle = col.muted;
        for (let mi = Math.ceil(range.lo / 1200); mi * 1200 <= range.hi; mi++) {
          const y = centsToY(mi * 1200, range, top, LANE_H);
          g.strokeStyle = col.line; g.globalAlpha = 0.6;
          g.beginPath(); g.moveTo(X0, y + 0.5); g.lineTo(width, y + 0.5); g.stroke();
          g.globalAlpha = 1;
          g.fillText(`C${mi}`, X0 + 3, y - 6);
        }
        for (const n of this.displayNotes(clip, layer)) {
          const r = noteRect(n, vp, range, top, LANE_H);
          g.fillStyle = hex(layer.color);
          g.fillRect(r.x, r.y, r.w, r.h);
          if (selIds.has(n.id)) {
            g.strokeStyle = col.fg; g.lineWidth = 2;
            g.strokeRect(r.x - 1, r.y - 1, r.w + 2, r.h + 2);
            g.lineWidth = 1;
            g.fillStyle = col.fg;
            g.fillRect(r.x + r.w - 3, r.y - 2, 3, r.h + 4);
          }
        }
      }
      g.restore();

      // Playhead: section position modulo the clip's loop, only for the section that is playing.
      if (m && m[M_PLAYING] === 1 && m[M_SECTION] === si) {
        const x = timeToX(m[M_SECTION_POS] % clip.loopLen, vp);
        g.strokeStyle = col.accent; g.lineWidth = 2;
        g.beginPath(); g.moveTo(x, top); g.lineTo(x, top + LANE_H); g.stroke();
        g.lineWidth = 1;
      }
    });

    // Ruler playhead.
    if (m && m[M_PLAYING] === 1 && m[M_SECTION] === si) {
      const x = timeToX(m[M_SECTION_POS], vp);
      g.fillStyle = col.accent;
      g.beginPath(); g.moveTo(x - 5, 0); g.lineTo(x + 5, 0); g.lineTo(x, 8); g.closePath(); g.fill();
    }
  }

  /** The morph strip under a lane: the state colour in force along the clip (blending over each marker's length, in the marker's
   *  shape), a line that shows the shape of each transition, and a glyph per marker in its state's colour. */
  private drawMorph(g: CanvasRenderingContext2D, layer: Layer, clip: Clip | undefined, top: number, s: Section, vp: Viewport, width: number, col: Colors): void {
    const y0 = top + LANE_H;
    const bandTop = y0 + 3;
    const bandH = MORPH_H - 6;
    g.save();
    g.beginPath(); g.rect(X0, y0 + 1, width - X0, MORPH_H - 1); g.clip();
    const marks = clip?.morph ?? [];
    if (!clip || !marks.length) {
      g.fillStyle = col.muted;
      g.fillText('double-click to add a morph marker (M adds one at the playhead)', X0 + 8, y0 + MORPH_H / 2);
      g.restore();
      return;
    }
    const upsec = sectionUnitsPerSec(s);
    const colorOf = (id: string) => stateColor(Math.max(0, layer.states.findIndex((x) => x.id === id)));
    const colors = marks.map((m) => colorOf(m.state));
    const x0 = timeToX(0, vp);
    const x1 = Math.min(width, timeToX(clip.loopLen, vp));
    g.globalAlpha = 0.55;
    for (let x = x0; x < x1; x += 2) {
      const m = morphAt(marks, xToTime(x + 1, vp), upsec);
      if (!m) continue;
      g.fillStyle = mixColor(colors[m.prev], colors[m.k], m.x);
      g.fillRect(x, bandTop, Math.min(2, x1 - x), bandH);
    }
    g.globalAlpha = 1;
    // The shape of each transition: a line rising from the old state's edge to the new one's over the marker's length.
    if (marks.length > 1) {
      g.strokeStyle = col.fg; g.lineWidth = 1.5;
      marks.forEach((m) => {
        const lenU = (m.lenMs / 1000) * upsec;
        const xa = timeToX(m.t, vp), xb = timeToX(m.t + lenU, vp);
        g.beginPath();
        if (m.shape === 'step' || m.lenMs <= 0 || xb - xa < 2) { g.moveTo(xa, bandTop + bandH); g.lineTo(xa, bandTop); }
        else {
          const n = Math.max(4, Math.min(40, Math.round((xb - xa) / 3)));
          for (let i = 0; i <= n; i++) {
            const u = i / n;
            const x = xa + (xb - xa) * u, y = bandTop + bandH - shapeProgress(m.shape, u) * bandH;
            if (i === 0) g.moveTo(x, y); else g.lineTo(x, y);
          }
        }
        g.stroke();
      });
      g.lineWidth = 1;
    }
    const sel = this.app.ui.marker;
    const selIdx = this.drag?.kind === 'marker' && this.drag.layerId === layer.id ? this.drag.markerIndex : sel && sel.layerId === layer.id ? sel.index : -1;
    marks.forEach((m, i) => {
      const cx = timeToX(m.t, vp), cy = y0 + MORPH_H / 2;
      this.drawGlyph(g, cx, cy, colors[i], i === selIdx, col);
    });
    g.restore();
  }

  /** A small Klont: a disc with a ring of short spikes. */
  private drawGlyph(g: CanvasRenderingContext2D, cx: number, cy: number, color: string, selected: boolean, col: Colors): void {
    g.fillStyle = color;
    g.strokeStyle = col.fg;
    g.beginPath();
    const n = 8;
    for (let i = 0; i < n * 2; i++) {
      const a = (i / (n * 2)) * Math.PI * 2;
      const r = i % 2 === 0 ? 7 : 4.5;
      const x = cx + Math.cos(a) * r, y = cy + Math.sin(a) * r;
      if (i === 0) g.moveTo(x, y); else g.lineTo(x, y);
    }
    g.closePath();
    g.fill();
    g.lineWidth = selected ? 2 : 1;
    g.stroke();
    g.lineWidth = 1;
  }

  private drawHit(g: CanvasRenderingContext2D, h: Hit, vp: Viewport, top: number, upsec: number, selected: boolean, col: Colors): void {
    const gl = hitGlyph(h, vp, top, LANE_H, upsec);
    const color = SLOT_COLORS[h.slot] ?? col.fg;
    if (gl.tailW > 0) {
      g.strokeStyle = color; g.globalAlpha = 0.55; g.lineWidth = 3;
      g.beginPath(); g.moveTo(gl.cx, gl.cy); g.lineTo(gl.cx + gl.tailW, gl.cy); g.stroke();
      g.globalAlpha = 1; g.lineWidth = 1;
    }
    g.fillStyle = color;
    g.beginPath(); g.arc(gl.cx, gl.cy, gl.r, 0, Math.PI * 2); g.fill();
    if (selected) {
      g.strokeStyle = col.fg; g.lineWidth = 2;
      g.beginPath(); g.arc(gl.cx, gl.cy, gl.r + 3, 0, Math.PI * 2); g.stroke();
      g.lineWidth = 1;
    }
  }
}
