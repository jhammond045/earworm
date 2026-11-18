// Canvas drawing for the M2 page: the live ribbon and the finished take.

import { fitCanvas, noteName, type Colors } from '../live/ribbon';
import { SLOT_COLORS } from './sounds';
import type { Hit, Note } from './analysis';
import { CURVE_STRIDE } from '../m1/track';
import { TUNED } from './take';

const F_VOICING = 7, SILENT = 2, F_LOUD = 2, F_ONSET = 5;

interface Range { lo: number; hi: number }

/** Cents range that holds the finite values, at least an octave tall, with a margin. */
function rangeOf(vals: Iterable<number>, prev?: Range): Range {
  let lo = Infinity, hi = -Infinity;
  for (const v of vals) if (Number.isFinite(v) && v > 0) { lo = Math.min(lo, v); hi = Math.max(hi, v); }
  if (!(lo <= hi)) return prev ?? { lo: 4200, hi: 6000 };
  const mid = (lo + hi) / 2, span = Math.max(1200, hi - lo + 400);
  return { lo: mid - span / 2, hi: mid + span / 2 };
}

function grid(g: CanvasRenderingContext2D, w: number, h: number, r: Range, col: Colors) {
  g.font = '11px ui-monospace, Menlo, monospace';
  g.textBaseline = 'middle';
  const y = (c: number) => h - ((c - r.lo) / (r.hi - r.lo)) * h;
  for (let m = Math.ceil(r.lo / 100); m * 100 <= r.hi; m++) {
    if (m % 12 !== 0 && m % 12 !== 7) continue;
    const yy = y(m * 100);
    g.strokeStyle = col.line;
    g.globalAlpha = m % 12 === 0 ? 1 : 0.5;
    g.beginPath(); g.moveTo(0, yy); g.lineTo(w, yy); g.stroke();
    g.globalAlpha = 1;
    g.fillStyle = col.muted;
    g.fillText(noteName(m + 12), 3, yy - 7);
  }
  return y;
}

/** Newest frames, oldest first: `recs` holds n records of 25 floats. Raw pitch dots, tuned line (width = loudness). */
export class LiveRibbon {
  private range?: Range;
  constructor(private canvas: HTMLCanvasElement) {}
  draw(recs: Float32Array, col: Colors, armed: boolean) {
    const { g, w, h } = fitCanvas(this.canvas);
    g.clearRect(0, 0, w, h);
    const n = Math.floor(recs.length / CURVE_STRIDE);
    const tuned: number[] = [];
    for (let i = 0; i < n; i += 5) tuned.push(recs[i * CURVE_STRIDE + TUNED]);
    const target = rangeOf(tuned, this.range);
    this.range = this.range ? { lo: this.range.lo + (target.lo - this.range.lo) * 0.05, hi: this.range.hi + (target.hi - this.range.hi) * 0.05 } : target;
    const y = grid(g, w, h, this.range, col);
    const dx = w / Math.max(n, 800);
    let px = 0, py = 0, pok = false;
    for (let i = 0; i < n; i++) {
      const r = i * CURVE_STRIDE;
      const x = w - (n - i) * dx;
      const voiced = recs[r + F_VOICING] !== SILENT;
      const raw = recs[r];
      if (voiced && Number.isFinite(raw)) { g.globalAlpha = 0.3; g.fillStyle = col.fg; g.fillRect(x - 1, y(raw) - 1, 2, 2); g.globalAlpha = 1; }
      const t = recs[r + TUNED];
      const ok = voiced && Number.isFinite(t) && t > 0;
      if (ok) {
        const yy = y(t);
        if (pok) {
          g.strokeStyle = col.accent;
          g.lineWidth = 1 + Math.max(0, Math.min(1, recs[r + F_LOUD])) * 10;
          g.lineCap = 'round';
          g.beginPath(); g.moveTo(px, py); g.lineTo(x, yy); g.stroke();
        }
        px = x; py = yy;
      }
      pok = ok;
      if (recs[r + F_ONSET] > 0.5 && !(i > 0 && recs[r - CURVE_STRIDE + F_ONSET] > 0.5)) {
        g.strokeStyle = col.accent; g.lineWidth = 2;
        g.beginPath(); g.moveTo(x, h); g.lineTo(x, h - 12); g.stroke();
      }
    }
    if (armed) {
      g.fillStyle = '#d9402b';
      g.beginPath(); g.arc(w - 12, 12, 6, 0, 2 * Math.PI); g.fill();
    }
  }
}

export interface TakeView {
  seconds: number;
  /** Repaired raw pitch per frame, cents, NaN unvoiced. */
  pitch: ArrayLike<number>;
  /** Tuned pitch per frame (octave applied). */
  tuned: ArrayLike<number>;
  notes: Note[];
  hits: Hit[];
  octave: number;
  loopSec: number;
}

/** The take: raw pitch (grey), tuned (accent), note bars, hit markers by slot, playhead. */
export function drawTake(canvas: HTMLCanvasElement, v: TakeView | null, playheadSec: number | null, col: Colors) {
  const { g, w, h } = fitCanvas(canvas);
  g.clearRect(0, 0, w, h);
  if (!v) {
    g.fillStyle = col.muted; g.font = '14px system-ui'; g.textBaseline = 'middle';
    g.fillText('Hold Space (or the button) and sing or beatbox. The take shows here.', 12, h / 2);
    return;
  }
  const vals: number[] = [];
  for (let i = 0; i < v.pitch.length; i += 2) vals.push(v.pitch[i], v.tuned[i]);
  for (const n of v.notes) vals.push(n.cents + 1200 * v.octave);
  for (const hh of v.hits) if (hh.shape.cents !== null) vals.push(hh.shape.cents);
  const r = rangeOf(vals);
  const y = grid(g, w, h, r, col);
  const total = Math.max(v.seconds, v.loopSec, 0.1);
  const x = (s: number) => (s / total) * w;
  // Loop end.
  if (v.loopSec < total) {
    g.fillStyle = col.line; g.globalAlpha = 0.4; g.fillRect(x(v.loopSec), 0, w - x(v.loopSec), h); g.globalAlpha = 1;
  }
  // Note bars.
  g.fillStyle = col.accent; g.globalAlpha = 0.25;
  for (const n of v.notes) {
    const yy = y(n.cents + 1200 * v.octave);
    g.fillRect(x(n.t), yy - 6, Math.max(2, x(n.dur)), 12);
  }
  g.globalAlpha = 1;
  // Pitch lines.
  const line = (a: ArrayLike<number>, color: string, width: number, alpha: number) => {
    g.strokeStyle = color; g.lineWidth = width; g.globalAlpha = alpha;
    g.beginPath();
    let pen = false;
    for (let i = 0; i < a.length; i++) {
      const c = a[i];
      if (!Number.isFinite(c) || c <= 0) { pen = false; continue; }
      const px = x(i / 100), py = y(c);
      if (pen) g.lineTo(px, py); else g.moveTo(px, py);
      pen = true;
    }
    g.stroke();
    g.globalAlpha = 1;
  };
  line(v.pitch, col.fg, 1, 0.45);
  line(v.tuned, col.accent, 2, 1);
  // Hits.
  for (const hh of v.hits) {
    const c = SLOT_COLORS[Math.max(0, Math.min(3, hh.slot))];
    const hx = x(hh.t);
    g.strokeStyle = c; g.fillStyle = c; g.lineWidth = 2;
    const top = h - 6 - Math.max(0.15, Math.min(1, hh.strength)) * (h - 20);
    g.beginPath(); g.moveTo(hx, h - 4); g.lineTo(hx, top); g.stroke();
    g.beginPath(); g.arc(hx, top, 4, 0, 2 * Math.PI); g.fill();
  }
  if (playheadSec !== null && playheadSec >= 0) {
    g.strokeStyle = col.fg; g.lineWidth = 1;
    g.beginPath(); g.moveTo(x(playheadSec), 0); g.lineTo(x(playheadSec), h); g.stroke();
  }
}

/** Level meter bar helper for the gate. */
export function levelBar(el: HTMLElement, frac: number) {
  el.style.width = `${Math.max(0, Math.min(1, frac)) * 100}%`;
}
