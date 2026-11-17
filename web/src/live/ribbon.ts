// Canvas drawing for the Live view: pitch ribbon and band bars.

import { F_ONSET, F_PITCH, F_LOUDNESS, F_OPENNESS, F_VOICING, RECORD_TUNED, SILENT } from '../host/protocol';

export const SCALE_DEGREES: number[][] = [
  [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
  [0, 2, 4, 5, 7, 9, 11],
  [0, 2, 3, 5, 7, 8, 10],
  [0, 2, 4, 7, 9],
  [0, 3, 5, 7, 10],
  [0, 3, 5, 6, 7, 10],
];

const NOTES = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];
export const noteName = (midi: number) => NOTES[((Math.round(midi) % 12) + 12) % 12] + (Math.floor(Math.round(midi) / 12) - 1);

/** Size a canvas to its CSS box at device pixel ratio. Returns a ctx in CSS pixels. */
export function fitCanvas(c: HTMLCanvasElement): { g: CanvasRenderingContext2D; w: number; h: number } {
  const dpr = window.devicePixelRatio || 1;
  const w = c.clientWidth, h = c.clientHeight;
  const pw = Math.max(1, Math.round(w * dpr)), ph = Math.max(1, Math.round(h * dpr));
  if (c.width !== pw || c.height !== ph) {
    c.width = pw;
    c.height = ph;
  }
  const g = c.getContext('2d')!;
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  return { g, w, h };
}

export interface Colors { fg: string; muted: string; line: string; accent: string }
export function readColors(): Colors {
  const s = getComputedStyle(document.documentElement);
  const v = (n: string) => s.getPropertyValue(n).trim();
  return { fg: v('--fg'), muted: v('--muted'), line: v('--line'), accent: v('--accent') };
}

/** Records are read from a page-side ring: get(i) returns the record i frames ago, or null. */
export interface RecordSource { count: number; get(back: number): Float32Array | null }

export class Ribbon {
  private center = NaN; // smoothed pitch centre in cents
  constructor(private canvas: HTMLCanvasElement, readonly seconds = 8, readonly spanSemis = 36) {}

  draw(src: RecordSource, scaleIdx: number, root: number, col: Colors, isDark: boolean) {
    const { g, w, h } = fitCanvas(this.canvas);
    g.clearRect(0, 0, w, h);
    const n = this.seconds * 100;
    const dx = w / n;
    // Follow the newest tuned pitch slowly so the view re-centres on the singer's range.
    for (let i = 0; i < Math.min(n, src.count); i += 20) {
      const r = src.get(i);
      const t = r && r[RECORD_TUNED];
      if (t && Number.isFinite(t)) {
        this.center = Number.isFinite(this.center) ? this.center + (t - this.center) * 0.002 : t;
        break;
      }
    }
    if (!Number.isFinite(this.center)) this.center = 4800; // C4 region before any voice
    const spanC = this.spanSemis * 100;
    const lo = this.center - spanC / 2;
    const y = (cents: number) => h - ((cents - lo) / spanC) * h;

    // Scale degree guides.
    const degs = SCALE_DEGREES[scaleIdx] ?? SCALE_DEGREES[0];
    g.font = '11px system-ui, sans-serif';
    g.textBaseline = 'middle';
    for (let m = Math.ceil(lo / 100); m * 100 <= lo + spanC; m++) {
      const rel = (((m - root) % 12) + 12) % 12;
      if (!degs.includes(rel)) continue;
      const yy = y(m * 100);
      const isRoot = rel === 0;
      g.strokeStyle = col.line;
      g.globalAlpha = isRoot ? 1 : 0.6;
      g.lineWidth = isRoot ? 1.5 : 1;
      g.beginPath();
      g.moveTo(0, yy);
      g.lineTo(w, yy);
      g.stroke();
      g.globalAlpha = 1;
      if (isRoot || degs.length <= 7) {
        g.fillStyle = col.muted;
        g.fillText(noteName(m + 12), 3, yy - 6); // cents above C0: C0 is MIDI 12
      }
    }

    // Raw pitch dots, tuned line, onset ticks.
    let prevX = 0, prevY = 0, prevOk = false;
    g.lineCap = 'round';
    for (let i = Math.min(n, src.count) - 1; i >= 0; i--) {
      const r = src.get(i);
      if (!r) continue;
      const x = w - i * dx;
      const raw = r[F_PITCH];
      if (Number.isFinite(raw) && r[F_VOICING] !== SILENT) {
        g.globalAlpha = 0.3;
        g.fillStyle = col.fg;
        g.fillRect(x - 1, y(raw) - 1, 2, 2);
        g.globalAlpha = 1;
      }
      const t = r[RECORD_TUNED];
      const ok = Number.isFinite(t) && t > 0 && r[F_VOICING] !== SILENT;
      if (ok) {
        const yy = y(t);
        if (prevOk) {
          const loud = Math.max(0, Math.min(1, r[F_LOUDNESS]));
          const open = Math.max(0, Math.min(1, r[F_OPENNESS]));
          const hue = 230 - open * 190; // dark blue closed, amber open
          const light = isDark ? 45 + open * 25 : 50 - open * 8;
          g.strokeStyle = `hsl(${hue} 80% ${light}%)`;
          g.lineWidth = 1 + loud * 12;
          g.beginPath();
          g.moveTo(prevX, prevY);
          g.lineTo(x, yy);
          g.stroke();
        }
        prevX = x; prevY = yy;
      }
      prevOk = ok;
      if (r[F_ONSET] > 0.5) {
        const p = src.get(i + 1);
        if (!p || p[F_ONSET] <= 0.5) {
          g.strokeStyle = col.accent;
          g.lineWidth = 2;
          g.beginPath();
          g.moveTo(x, h);
          g.lineTo(x, h - 12);
          g.stroke();
        }
      }
    }
  }
}

/** Bars from a centre line, +-range dB. */
export function drawBands(canvas: HTMLCanvasElement, vals: ArrayLike<number>, n: number, range: number, col: Colors) {
  const { g, w, h } = fitCanvas(canvas);
  g.clearRect(0, 0, w, h);
  const mid = h / 2;
  g.strokeStyle = col.line;
  g.lineWidth = 1;
  g.beginPath();
  g.moveTo(0, mid);
  g.lineTo(w, mid);
  g.stroke();
  const bw = w / n;
  g.fillStyle = col.accent;
  for (let i = 0; i < n; i++) {
    const v = Math.max(-range, Math.min(range, vals[i] || 0));
    const bh = (v / range) * (h / 2 - 2);
    g.fillRect(i * bw + 1, bh >= 0 ? mid - bh : mid, Math.max(1, bw - 2), Math.max(1, Math.abs(bh)));
  }
}
