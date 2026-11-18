// Hold-to-act buttons and the mic ring reader the Influence and kit panels share.

import type { KlontHost } from '../m1/klont-host';

/** Pointer, Space and Enter all hold the button down; letting go, losing focus or the window blurring lets go.
 *  Space and Enter stop at the button so the page's hold-Space capture does not start too. */
export function holdButton(btn: HTMLButtonElement, on: () => void, off: () => void): void {
  let down = false;
  const press = () => { if (!down && !btn.disabled) { down = true; btn.classList.add('down'); on(); } };
  const release = () => { if (down) { down = false; btn.classList.remove('down'); off(); } };
  btn.addEventListener('pointerdown', (e) => {
    if (e.button !== 0) return;
    try { btn.setPointerCapture(e.pointerId); } catch { /* a synthetic pointer: no capture */ }
    press();
  });
  btn.addEventListener('pointerup', release);
  btn.addEventListener('pointercancel', release);
  btn.addEventListener('blur', release);
  window.addEventListener('blur', release);
  btn.addEventListener('keydown', (e) => {
    if (e.key !== ' ' && e.key !== 'Enter') return;
    e.preventDefault();
    e.stopPropagation();
    if (!e.repeat) press();
  });
  btn.addEventListener('keyup', (e) => {
    if (e.key !== ' ' && e.key !== 'Enter') return;
    e.preventDefault();
    e.stopPropagation();
    release();
  });
}

/** True when the key event is going into a text field, a number box or a select (single-letter shortcuts stay out of the way). */
export function typingIn(t: EventTarget | null): boolean {
  return (t instanceof HTMLInputElement && !['range', 'checkbox', 'radio', 'button', 'file'].includes(t.type)) ||
    t instanceof HTMLTextAreaElement || t instanceof HTMLSelectElement || (t instanceof HTMLElement && t.isContentEditable);
}

/** The mic audio recorded from now on, read from the engine's mic ring (high-passed, at the context rate).
 *  The mic itself is only ever on because the user turned it on. */
export class MicClip {
  private readonly s0: number;
  constructor(private readonly host: KlontHost, readonly rate: number, private readonly maxSeconds: number) {
    this.s0 = host.samplesTotal();
  }
  /** Samples recorded so far, capped at the maximum. */
  get length(): number { return Math.min(this.host.samplesTotal() - this.s0, Math.floor(this.maxSeconds * this.rate)); }
  get seconds(): number { return this.length / this.rate; }
  /** Everything so far, as a fresh array. */
  read(): Float32Array { return this.host.readMic(this.s0, this.length); }
}
