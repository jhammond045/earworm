// WebGL2 timeline for one section: a lane per layer, note ribbons from the take, written notes,
// hits, and the morph strip. The pattern grid stays beside it. Dragging commits one undo on release.

import { M_PLAYING, M_SECTION, M_SECTION_POS } from '../m1/protocol';
import { SLOT_COLORS } from '../m2/sounds';
import type { App } from '../m3/app';
import {
  MORPH_H, centsToY, dragCents, dragDt, dragMarkerT, draggedHitSlot, fitPpu, gridLines, hitTestHits, hitTestMarkers, hitTestNotes,
  morphAt, pitchRange, resizeDur, ribbonRuns, slotRow, slotY, stateColor, timeToX, warpSlope, xToTime, yToCents,
  type Range,
} from '../m3/geometry';
import { sectionLength, sectionUnitsPerSec } from '../project/compile';
import { LANE_FIXED, PPQ, type AutoLane, type Clip, type LanePoint, type Layer, type MorphShape, type Note, type Section } from '../project/types';

const RULER = 18;
const LANE = 88;
/** The automation strip under the morph strip (stage S5): one lane of the clip, chosen per layer. */
const AUTO_H = 34;
const STRIDE = LANE + MORPH_H + AUTO_H;
const X0 = 4;

/** A lane target's range (types.ts AutoLane units). */
export const laneRange = (target: string): [number, number] => LANE_FIXED[target] ?? [0, 1];

const VERT = `#version 300 es
layout(location=0) in vec2 a_pos;
layout(location=1) in vec4 a_color;
uniform vec2 u_res;
out vec4 v_color;
void main() {
  vec2 c = (a_pos / u_res) * 2.0 - 1.0;
  gl_Position = vec4(c.x, -c.y, 0.0, 1.0);
  v_color = a_color;
}`;

const FRAG = `#version 300 es
precision mediump float;
in vec4 v_color;
out vec4 o;
void main() { o = v_color; }`;

type Rgba = [number, number, number, number];

interface Drag {
  kind: 'move' | 'resize' | 'marker' | 'point';
  /** A lane point being dragged: its value. */
  v?: number;
  layerId: string;
  ids: string[];
  index: number;
  primaryT: number;
  primaryCents: number;
  primaryDur: number;
  startX: number;
  startY: number;
  dt: number;
  dc: number;
  dur: number;
  t: number;
  /** Hit slot while a beat hit is dragged. Notes leave this at 0. */
  slot: number;
  primarySlot: number;
}

class Mesh {
  readonly data: number[] = [];
  vert(x: number, y: number, c: Rgba): void { this.data.push(x, y, c[0], c[1], c[2], c[3]); }
  quad(x: number, y: number, w: number, h: number, c: Rgba): void {
    this.vert(x, y, c); this.vert(x + w, y, c); this.vert(x, y + h, c);
    this.vert(x, y + h, c); this.vert(x + w, y, c); this.vert(x + w, y + h, c);
  }
  /** A ribbon run: centre line plus half-width, as a filled strip. */
  ribbon(run: { x: number; y: number; hw: number }[], c: Rgba): void {
    for (let i = 0; i < run.length - 1; i++) {
      const a = run[i];
      const b = run[i + 1];
      this.vert(a.x, a.y - a.hw, c); this.vert(b.x, b.y - b.hw, c); this.vert(a.x, a.y + a.hw, c);
      this.vert(a.x, a.y + a.hw, c); this.vert(b.x, b.y - b.hw, c); this.vert(b.x, b.y + b.hw, c);
    }
  }
}

function parseHex(hex: string, a: number): Rgba {
  const n = parseInt(hex.replace('#', ''), 16);
  return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255, a];
}

function fromInt(n: number, a: number): Rgba {
  return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255, a];
}

function compile(gl: WebGL2RenderingContext, type: number, src: string): WebGLShader {
  const sh = gl.createShader(type);
  if (!sh) throw new Error('no shader');
  gl.shaderSource(sh, src);
  gl.compileShader(sh);
  if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(sh) || 'shader');
  return sh;
}

export class GlTimeline {
  private gl: WebGL2RenderingContext | null = null;
  private program: WebGLProgram | null = null;
  private buf: WebGLBuffer | null = null;
  private drag: Drag | null = null;
  private dirty = true;
  private ranges = new Map<string, Range>();
  private nameKey = '';
  private readonly canvas: HTMLCanvasElement;
  private readonly names: HTMLElement;
  private readonly scroll: HTMLElement;
  private readonly note: HTMLElement;
  /** The lane target the strip shows, per layer ('' or missing: the clip's first lane). */
  readonly laneShown = new Map<string, string>();

  constructor(private readonly app: App, private readonly onUse: () => void) {
    const canvas = document.getElementById('timeline');
    const names = document.getElementById('tlNames');
    const scroll = document.getElementById('tlScroll');
    const note = document.getElementById('tlNote');
    if (!(canvas instanceof HTMLCanvasElement) || !names || !scroll || !note) throw new Error('missing timeline');
    this.canvas = canvas;
    this.names = names;
    this.scroll = scroll;
    this.note = note;
    const gl = canvas.getContext('webgl2', { alpha: false, antialias: true });
    this.gl = gl;
    if (!gl) { note.textContent = 'This browser has no WebGL2, so the timeline cannot draw. The pattern grid still edits the track.'; return; }
    const prog = gl.createProgram();
    if (!prog) return;
    gl.attachShader(prog, compile(gl, gl.VERTEX_SHADER, VERT));
    gl.attachShader(prog, compile(gl, gl.FRAGMENT_SHADER, FRAG));
    gl.linkProgram(prog);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(prog) || 'link');
    this.program = prog;
    this.buf = gl.createBuffer();
    canvas.addEventListener('pointerdown', (e) => this.onDown(e));
    canvas.addEventListener('pointermove', (e) => this.onMove(e));
    canvas.addEventListener('pointerup', (e) => this.onUp(e));
    canvas.addEventListener('pointercancel', () => { this.drag = null; });
    canvas.addEventListener('dblclick', (e) => this.onDouble(e));
    new ResizeObserver(() => { this.dirty = true; }).observe(scroll);
    app.ui.subscribe(() => { this.dirty = true; });
  }

  invalidate(): void { this.dirty = true; }

  /** Redraw while the playhead moves, and whenever the document or the size changed. */
  frame(): void {
    const playing = this.app.readMeters()?.[M_PLAYING] === 1;
    if (this.dirty || playing || this.drag) this.draw();
  }

  private section(): Section | undefined {
    return this.app.doc.sections.find((s) => s.id === this.app.ui.sectionId) ?? this.app.doc.sections[0];
  }

  private ppu(sec: Section, width: number): number {
    return fitPpu(width, Math.max(1, sectionLength(sec)), X0, this.app.ui.zoom);
  }

  private shown(layer: Layer, notes: Note[]): Note[] {
    const oct = 1200 * layer.tune.octave;
    return notes.map((n) => ({ ...n, cents: n.cents + oct }));
  }

  private rangeFor(layer: Layer, notes: Note[]): Range {
    const prev = this.ranges.get(layer.id);
    const r = pitchRange(notes.map((n) => n.cents), prev);
    this.ranges.set(layer.id, r);
    return r;
  }

  private pos(e: MouseEvent): { x: number; y: number } {
    const r = this.canvas.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  }

  private zone(y: number): { li: number; morph: boolean; auto: boolean } {
    const i = Math.floor((y - RULER) / STRIDE);
    if (y < RULER || i < 0 || i >= this.app.doc.layers.length) return { li: -1, morph: false, auto: false };
    const dy = y - (RULER + i * STRIDE);
    return { li: i, morph: dy >= LANE && dy < LANE + MORPH_H, auto: dy >= LANE + MORPH_H };
  }

  /** The target the strip of `layer` shows in `clip`. */
  laneTarget(layer: Layer, clip: Clip | undefined): string {
    return this.laneShown.get(layer.id) || clip?.auto?.[0]?.target || '';
  }

  private laneOf(layer: Layer, clip: Clip | undefined): AutoLane | undefined {
    const t = this.laneTarget(layer, clip);
    return clip?.auto?.find((x) => x.target === t);
  }

  /** y of lane value `v` in the strip whose top is `top`, and back. */
  private vToY(v: number, target: string, top: number): number {
    const [lo, hi] = laneRange(target);
    return top + 3 + (1 - (v - lo) / (hi - lo)) * (AUTO_H - 6);
  }
  private yToV(y: number, target: string, top: number): number {
    const [lo, hi] = laneRange(target);
    const u = 1 - (y - top - 3) / (AUTO_H - 6);
    return lo + Math.min(1, Math.max(0, u)) * (hi - lo);
  }

  /** Index of the lane point under (x, y), or -1. */
  private hitPoint(points: LanePoint[], x: number, y: number, target: string, top: number, vp: { ppu: number; x0: number }): number {
    let best = -1, bd = 7;
    points.forEach((p, i) => {
      const d = Math.hypot(timeToX(p.t, vp) - x, this.vToY(p.v, target, top) - y);
      if (d < bd) { bd = d; best = i; }
    });
    return best;
  }

  private snapOf(sec: Section, clip: Clip | undefined): number {
    if (clip && clip.timeSnap > 0) return clip.timeSnap;
    return sec.feel === 'grid' ? PPQ / 4 : 0.25;
  }

  private onDown(e: PointerEvent): void {
    if (e.button !== 0 || !this.gl) return;
    this.onUse();
    const sec = this.section();
    if (!sec) return;
    const { x, y } = this.pos(e);
    const { li, morph, auto } = this.zone(y);
    if (li < 0) {
      const si = this.app.doc.sections.findIndex((s) => s.id === sec.id);
      const t = Math.max(0, xToTime(x, { ppu: this.ppu(sec, this.canvas.clientWidth), x0: X0 }));
      if (si >= 0) this.app.host?.seek(si, t);
      return;
    }
    const layer = this.app.doc.layers[li];
    this.app.ui.layerId = layer.id;
    const clip = sec.clips[layer.id];
    const top = RULER + li * STRIDE;
    const vp = { ppu: this.ppu(sec, this.canvas.clientWidth), x0: X0 };
    if (auto) {
      const lane = this.laneOf(layer, clip);
      const i = lane ? this.hitPoint(lane.points, x, y, lane.target, top + LANE + MORPH_H, vp) : -1;
      if (lane && i >= 0) {
        const p = lane.points[i];
        this.drag = { kind: 'point', layerId: layer.id, ids: [], index: i, primaryT: p.t, primaryCents: 0, primaryDur: 0, startX: x, startY: y, dt: 0, dc: 0, dur: 0, t: p.t, slot: 0, primarySlot: 0, v: p.v };
        try { this.canvas.setPointerCapture(e.pointerId); } catch { /* synthetic */ }
      }
      this.app.ui.changed();
      return;
    }
    if (morph) {
      const i = clip ? hitTestMarkers(clip.morph ?? [], x, y, vp, top + LANE) : -1;
      if (i >= 0 && clip) {
        this.app.ui.selectMarker({ layerId: layer.id, index: i });
        const m = clip.morph[i];
        this.drag = { kind: 'marker', layerId: layer.id, ids: [], index: i, primaryT: m.t, primaryCents: 0, primaryDur: 0, startX: x, startY: y, dt: 0, dc: 0, dur: 0, t: m.t, slot: 0, primarySlot: 0 };
        try { this.canvas.setPointerCapture(e.pointerId); } catch { /* synthetic */ }
      }
      return;
    }
    if (!clip) { this.app.ui.select(layer.id, []); return; }
    if (clip.kind === 'beat') {
      const id = hitTestHits(clip.hits, x, y, vp, top, LANE, sectionUnitsPerSec(sec));
      if (!id) { this.app.ui.select(layer.id, []); return; }
      const hit = clip.hits.find((h) => h.id === id);
      this.app.ui.select(layer.id, [id]);
      this.drag = { kind: 'move', layerId: layer.id, ids: [id], index: -1, primaryT: hit?.t ?? 0, primaryCents: 0, primaryDur: 0, startX: x, startY: y, dt: 0, dc: 0, dur: 0, t: 0, slot: hit?.slot ?? 0, primarySlot: hit?.slot ?? 0 };
      try { this.canvas.setPointerCapture(e.pointerId); } catch { /* synthetic */ }
      return;
    }
    const notes = this.shown(layer, clip.notes);
    const hit = hitTestNotes(notes, x, y, vp, this.rangeFor(layer, notes), top, LANE);
    if (!hit) { this.app.ui.select(layer.id, []); return; }
    const note = notes.find((n) => n.id === hit.id);
    this.app.ui.select(layer.id, [hit.id]);
    this.drag = {
      kind: hit.part === 'end' ? 'resize' : 'move', layerId: layer.id, ids: [hit.id], index: -1,
      primaryT: note?.t ?? 0, primaryCents: note?.cents ?? 4800, primaryDur: note?.dur ?? 1,
      startX: x, startY: y, dt: 0, dc: 0, dur: note?.dur ?? 1, t: 0, slot: 0, primarySlot: 0,
    };
    try { this.canvas.setPointerCapture(e.pointerId); } catch { /* synthetic */ }
  }

  private onMove(e: PointerEvent): void {
    const d = this.drag;
    const sec = this.section();
    if (!d || !sec) return;
    const { x, y } = this.pos(e);
    const ppu = this.ppu(sec, this.canvas.clientWidth);
    const clip = sec.clips[d.layerId];
    const snap = this.snapOf(sec, clip);
    const rawDt = (x - d.startX) / ppu;
    if (d.kind === 'point') {
      const layer = this.app.doc.layers.find((l) => l.id === d.layerId);
      const top = RULER + this.app.doc.layers.findIndex((l) => l.id === d.layerId) * STRIDE + LANE + MORPH_H;
      d.t = dragMarkerT(d.primaryT, rawDt, e.shiftKey ? 0 : snap, clip?.loopLen ?? sectionLength(sec), sec.feel === 'grid');
      if (layer) d.v = this.yToV(y, this.laneTarget(layer, clip), top);
    } else if (d.kind === 'marker') d.t = dragMarkerT(d.primaryT, rawDt, snap, clip?.loopLen ?? sectionLength(sec), sec.feel === 'grid');
    else if (d.kind === 'resize') d.dur = resizeDur(d.primaryT, d.primaryDur, rawDt, snap, sec.feel === 'grid' ? 1 : 0.05);
    else {
      d.dt = dragDt(d.primaryT, rawDt, snap);
      const layer = this.app.doc.layers.find((l) => l.id === d.layerId);
      if (layer?.kind === 'beat') {
        const top = RULER + this.app.doc.layers.findIndex((l) => l.id === d.layerId) * STRIDE;
        d.slot = draggedHitSlot(d.primarySlot, y, d.startY, top, LANE);
      }
      const top = RULER + this.app.doc.layers.findIndex((l) => l.id === d.layerId) * STRIDE;
      const range = layer ? this.rangeFor(layer, this.shown(layer, clip && clip.kind !== 'beat' ? clip.notes : [])) : { lo: 4800, hi: 7200 };
      const c0 = yToCents(d.startY, range, top, LANE);
      const c1 = yToCents(y, range, top, LANE);
      d.dc = dragCents(d.primaryCents, c1 - c0);
    }
    this.dirty = true;
  }

  private onUp(e: PointerEvent): void {
    const d = this.drag;
    this.drag = null;
    if (!d) return;
    const sec = this.section();
    if (!sec) return;
    const layer = d.layerId;
    if (d.kind === 'point') {
      const l = this.app.doc.layers.find((x) => x.id === layer);
      const lane = l ? this.laneOf(l, sec.clips[layer]) : undefined;
      if (lane && (d.t !== d.primaryT || d.v !== lane.points[d.index]?.v)) {
        const points = lane.points.map((p, i) => (i === d.index ? { ...p, t: d.t, v: d.v ?? p.v } : p));
        this.app.run(() => this.app.store.dispatch('setLane', { section: sec.id, layer, target: lane.target, points }));
      }
    } else if (d.kind === 'marker') {
      if (d.t !== d.primaryT) this.app.run(() => this.app.store.dispatch('moveMorphMarker', { section: sec.id, layer, index: d.index, t: d.t }));
    } else if (d.kind === 'resize') {
      if (d.dur !== d.primaryDur) this.app.run(() => this.app.store.dispatch('resizeNote', { section: sec.id, layer, id: d.ids[0], dur: d.dur }));
    } else if (d.dt !== 0 || d.dc !== 0 || d.slot !== d.primarySlot) {
      const clip = sec.clips[layer];
      if (clip?.kind === 'beat') {
        if (d.dt !== 0 || d.slot !== d.primarySlot) {
          this.app.run(() => this.app.store.batch('Move hit', (run) => {
            if (d.dt !== 0) run('moveHits', { section: sec.id, layer, ids: d.ids, dt: d.dt });
            if (d.slot !== d.primarySlot) run('setHitSlot', { section: sec.id, layer, ids: d.ids, slot: d.slot });
          }));
        }
      } else if (d.dt !== 0 || d.dc !== 0) this.app.run(() => this.app.store.dispatch('moveNotes', { section: sec.id, layer, ids: d.ids, dt: d.dt, dcents: d.dc }));
    }
    this.dirty = true;
    void e;
  }

  private onDouble(e: MouseEvent): void {
    const sec = this.section();
    if (!sec || !this.gl) return;
    const { x, y } = this.pos(e);
    const { li, morph, auto } = this.zone(y);
    if (li < 0) return;
    const layer = this.app.doc.layers[li];
    const top = RULER + li * STRIDE;
    const vp = { ppu: this.ppu(sec, this.canvas.clientWidth), x0: X0 };
    const t = Math.max(0, xToTime(x, vp));
    if (auto) {
      // Double-click a point to remove it; empty space adds one (line shape to the next).
      const clip = sec.clips[layer.id];
      const target = this.laneTarget(layer, clip);
      if (!target) { this.app.fail('Pick what the lane shows under the timeline (Lane), then double-click the strip.'); return; }
      const lane = clip?.auto?.find((a) => a.target === target);
      const at = top + LANE + MORPH_H;
      const hit = lane ? this.hitPoint(lane.points, x, y, target, at, vp) : -1;
      const points = hit >= 0 ? lane!.points.filter((_, i) => i !== hit) : [...(lane?.points ?? []), { t, v: this.yToV(y, target, at), shape: 'line' as const }];
      this.app.run(() => this.app.store.dispatch('setLane', { section: sec.id, layer: layer.id, target, points }));
      return;
    }
    if (morph) {
      const shape = (document.getElementById('morphShape') as HTMLSelectElement | null)?.value ?? 'line';
      const lenMs = Number((document.getElementById('morphLen') as HTMLInputElement | null)?.value ?? 400);
      this.app.run(() => this.app.store.dispatch('addMorphMarker', {
        section: sec.id, layer: layer.id, marker: { t, state: this.app.ui.editState(layer).id, lenMs, shape: shape as MorphShape },
      }));
      return;
    }
    if (layer.kind === 'beat') {
      const slot = slotRow(y, top, LANE);
      this.app.run(() => this.app.store.dispatch('addHit', {
        section: sec.id, layer: layer.id,
        hit: { t, dur: 200, slot, strength: 0.8, shape: { attack: 0.004, burst: 0.4, cents: null, bend: 0, hold: 0.05, vowel: 0.4, tone: 0.6, endStop: 0, endClick: 0 } },
      }));
      return;
    }
    const clip = sec.clips[layer.id];
    const notes = clip && clip.kind !== 'beat' ? clip.notes : [];
    const range = this.rangeFor(layer, this.shown(layer, notes));
    const cents = Math.round(yToCents(y, range, top, LANE) - 1200 * layer.tune.octave);
    const dur = sec.feel === 'grid' ? PPQ / 4 : 0.25;
    this.app.run(() => this.app.store.dispatch('addNote', { section: sec.id, layer: layer.id, note: { t, dur, cents, hard: 0.35 } }));
  }

  private draw(): void {
    const gl = this.gl;
    const prog = this.program;
    if (!gl || !prog || !this.buf) return;
    const doc = this.app.doc;
    const sec = this.section();
    const n = doc.layers.length;
    const cssH = RULER + Math.max(1, n) * STRIDE;
    const viewW = Math.max(280, this.scroll.clientWidth);
    const ppu = sec ? this.ppu(sec, viewW) : 1;
    const len = sec ? sectionLength(sec) : 1;
    const cssW = Math.max(viewW, X0 + len * ppu + 24);
    const dpr = window.devicePixelRatio || 1;
    this.canvas.style.width = `${cssW}px`;
    this.canvas.style.height = `${cssH}px`;
    const pw = Math.max(1, Math.round(cssW * dpr));
    const ph = Math.max(1, Math.round(cssH * dpr));
    if (this.canvas.width !== pw || this.canvas.height !== ph) { this.canvas.width = pw; this.canvas.height = ph; }
    this.names.style.height = `${cssH}px`;
    this.layoutNames(doc.layers);
    if (!sec) { this.dirty = false; return; }

    const mesh = new Mesh();
    const bg: Rgba = [0.039, 0.051, 0.071, 1];
    const line: Rgba = [0.24, 0.32, 0.40, 1];
    const accent: Rgba = [0.941, 0.706, 0.161, 1];
    const dim: Rgba = [0.54, 0.63, 0.71, 0.7];
    mesh.quad(0, 0, cssW, cssH, bg);
    const vp = { ppu, x0: X0 };
    for (const g of gridLines(sec.feel, len)) {
      const x = timeToX(g.t, vp);
      mesh.quad(x, 0, g.strong ? 1.5 : 1, RULER, g.strong ? accent : line);
    }
    const meters = this.app.readMeters();
    const playing = !!meters && meters[M_PLAYING] === 1 && Math.round(meters[M_SECTION]) === doc.sections.findIndex((s) => s.id === sec.id);
    const playT = playing && meters ? meters[M_SECTION_POS] : -1;

    doc.layers.forEach((layer, i) => {
      const top = RULER + i * STRIDE;
      const active = layer.id === this.app.ui.layerId;
      mesh.quad(0, top, cssW, LANE, active ? [0.12, 0.16, 0.22, 1] : [0.07, 0.09, 0.13, 1]);
      mesh.quad(0, top + LANE, cssW, MORPH_H, [0.09, 0.11, 0.15, 1]);
      mesh.quad(0, top + LANE, cssW, 1, line);
      mesh.quad(0, top + LANE + MORPH_H, cssW, AUTO_H, [0.06, 0.08, 0.11, 1]);
      mesh.quad(0, top + LANE + MORPH_H, cssW, 1, line);
      if (active) mesh.quad(0, top, 3, STRIDE, accent);
      const clip = sec.clips[layer.id];
      const sel = this.app.ui.selection.layerId === layer.id ? new Set(this.app.ui.selection.ids) : new Set<string>();
      const drag = this.drag && this.drag.layerId === layer.id ? this.drag : null;
      if (clip?.kind === 'beat') {
        for (let k = 0; k < 4; k++) mesh.quad(X0, slotY(k, top, LANE), cssW - X0, 1, line);
        for (const h of clip.hits) {
          const moving = drag?.kind === 'move' && drag.ids.includes(h.id);
          const t = h.t + (moving ? drag.dt : 0);
          const slot = moving ? drag.slot : h.slot;
          const x = timeToX(t, vp);
          const y = slotY(slot, top, LANE);
          const c = parseHex(SLOT_COLORS[slot] ?? '#d5e2ee', sel.has(h.id) ? 1 : 0.85);
          mesh.quad(x - 4, y - 5, 8, 10, c);
        }
      } else if (clip) {
        const notes = this.shown(layer, clip.notes).map((note) => {
          if (!drag || !drag.ids.includes(note.id)) return note;
          if (drag.kind === 'resize') return { ...note, dur: drag.dur };
          return { ...note, t: note.t + drag.dt, cents: note.cents + drag.dc };
        });
        const range = this.rangeFor(layer, notes);
        for (let mi = Math.ceil(range.lo / 1200); mi * 1200 <= range.hi; mi++) {
          mesh.quad(X0, centsToY(mi * 1200, range, top, LANE), cssW - X0, 1, dim);
        }
        const take = clip.take ? doc.takes[clip.take] : undefined;
        const frames = take ? this.app.sync.cachedCurve(take, layer) : null;
        const ink = fromInt(layer.color, 0.45);
        if (frames) {
          const slope = warpSlope(sec.feel, sec.bpm);
          for (const n of notes) {
            for (const run of ribbonRuns({ t: n.t, dur: n.dur, anchors: clip.warp, slope, frames, vp, range, top, h: LANE, maxHalf: 12 })) {
              mesh.ribbon(run, ink);
            }
          }
        }
        for (const n of notes) {
          const x = timeToX(n.t, vp);
          const w = Math.max(3, n.dur * ppu);
          const y = centsToY(n.cents, range, top, LANE) - 4;
          mesh.quad(x, y, w, 8, fromInt(layer.color, sel.has(n.id) ? 1 : 0.9));
          if (sel.has(n.id)) mesh.quad(x + w - 3, y - 2, 3, 12, accent);
        }
      }
      const marks = clip?.morph ?? [];
      if (clip && marks.length) {
        const up = sectionUnitsPerSec(sec);
        const colors = marks.map((m) => stateColor(Math.max(0, layer.states.findIndex((s) => s.id === m.state))));
        const x1 = Math.min(cssW, timeToX(clip.loopLen, vp));
        for (let x = X0; x < x1; x += 3) {
          const at = morphAt(marks, xToTime(x + 1, vp), up);
          if (!at) continue;
          mesh.quad(x, top + LANE + 4, 3, MORPH_H - 8, parseHex(colors[at.k] ?? colors[0], 0.85));
        }
        marks.forEach((m, mi) => {
          const t = drag?.kind === 'marker' && drag.index === mi ? drag.t : m.t;
          const x = timeToX(t, vp);
          mesh.quad(x - 4, top + LANE + 4, 8, MORPH_H - 8, parseHex(colors[mi] ?? '#e6b15a', 1));
        });
      }
      // The automation strip: the chosen lane as a line through its points.
      const lane = this.laneOf(layer, clip);
      if (clip && lane && lane.points.length) {
        const at = top + LANE + MORPH_H;
        const pts = lane.points.map((p, pi) => (drag?.kind === 'point' && drag.index === pi ? { ...p, t: drag.t, v: drag.v ?? p.v } : p)).sort((a, b) => a.t - b.t);
        const ink2 = fromInt(layer.color, 0.9);
        const x1 = Math.min(cssW, timeToX(clip.loopLen, vp));
        const valueAt = (t: number) => {
          let k = 0;
          while (k < pts.length && pts[k].t <= t) k++;
          if (k === 0) return pts[0].v;
          if (k >= pts.length) return pts[pts.length - 1].v;
          const a = pts[k - 1], b = pts[k];
          let u = (t - a.t) / Math.max(1e-9, b.t - a.t);
          if (a.shape === 'step') u = 0;
          else if (a.shape === 'ease') u = u * u * (3 - 2 * u);
          return a.v + (b.v - a.v) * u;
        };
        for (let px = X0; px < x1; px += 2) mesh.quad(px, this.vToY(valueAt(xToTime(px, vp)), lane.target, at) - 1, 2, 2, ink2);
        for (const p of pts) mesh.quad(timeToX(p.t, vp) - 3, this.vToY(p.v, lane.target, at) - 3, 6, 6, accent);
      }
      if (playT >= 0) mesh.quad(timeToX(clip ? playT % (clip.loopLen || len) : playT, vp), top, 2, LANE, accent);
    });
    if (playT >= 0) mesh.quad(timeToX(playT, vp) - 4, 0, 8, RULER, accent);

    gl.viewport(0, 0, this.canvas.width, this.canvas.height);
    gl.disable(gl.DEPTH_TEST);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
    gl.useProgram(prog);
    gl.uniform2f(gl.getUniformLocation(prog, 'u_res'), cssW, cssH);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.buf);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(mesh.data), gl.DYNAMIC_DRAW);
    gl.enableVertexAttribArray(0);
    gl.enableVertexAttribArray(1);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 24, 0);
    gl.vertexAttribPointer(1, 4, gl.FLOAT, false, 24, 8);
    gl.drawArrays(gl.TRIANGLES, 0, mesh.data.length / 6);
    this.dirty = false;
    if (doc.layers.some((l) => { const c = sec.clips[l.id]; return c && c.kind !== 'beat' && c.take && !this.app.sync.cachedCurve(doc.takes[c.take], l); })) {
      this.note.textContent = 'Ribbons appear once the take has been tuned. The bars are the written notes.';
    }
  }

  private layoutNames(layers: Layer[]): void {
    const key = layers.map((l) => `${l.id}:${l.name}:${l.id === this.app.ui.layerId ? 1 : 0}`).join('|');
    if (key === this.nameKey) return;
    this.nameKey = key;
    this.names.replaceChildren();
    const ruler = document.createElement('div');
    ruler.style.height = `${RULER}px`;
    this.names.append(ruler);
    for (const l of layers) {
      const d = document.createElement('button');
      d.type = 'button';
      d.className = 'tl-name' + (l.id === this.app.ui.layerId ? ' on' : '');
      d.style.height = `${STRIDE}px`;
      d.textContent = l.name;
      d.title = l.kind;
      d.addEventListener('click', () => { this.app.ui.layerId = l.id; this.app.ui.changed(); });
      this.names.append(d);
    }
  }
}
