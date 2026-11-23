// The Meter row (docs/meter.md "Interface sketch", stage R1), beside the Feel row for the
// selected layer: which meter the layer's clips lock to, that meter's beats per bar, and the
// section's pattern as numbered beat chips. Click a chip to repeat its beat, drag it (mouse or
// finger) to move it, × to drop it; the keyboard moves with the arrows, Shift with an arrow moves
// a chip, Delete drops and Enter repeats. A text box takes the pattern typed ("1 3 4 2"), and a
// menu has the common ones. Every edit is one command, so one undo step.

import type { App } from '../m3/app';
import { MAX_METER_BEATS, MAX_PATTERN, MIN_METER_BEATS, METER1, metersOf, parsePattern, patternOf, playOrder, presets } from '../project/meter';
import type { Layer, Meter, Project, Section } from '../project/types';
import { PPQ } from '../project/types';

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;

/** What a layer's clips are locked to: one meter id, '' for free, or 'mixed'. */
export function trackLock(doc: Project, layerId: string): string {
  const locks = new Set<string>();
  for (const s of doc.sections) {
    const c = s.clips[layerId];
    if (c) locks.add(c.meter && metersOf(doc).some((m) => m.id === c.meter) ? c.meter : '');
  }
  if (locks.size > 1) return 'mixed';
  return [...locks][0] ?? '';
}

/** A short tag for a layer's lock: "M1", "M2", "mixed", or '' when free. */
export function lockTag(doc: Project, layerId: string): string {
  const lock = trackLock(doc, layerId);
  if (lock === 'mixed') return 'M mixed';
  const i = metersOf(doc).findIndex((m) => m.id === lock);
  return i >= 0 ? `M${i + 1}` : '';
}

/** "4 bars of 3 (written 4 bars of 4)": how the section plays under Meter 1. */
export function sectionPlays(doc: Project, s: Section): string {
  if (s.feel !== 'grid') return 'A free section has no beats, so no meter acts on it.';
  const m1 = metersOf(doc)[0];
  const bars = (beats: number, per: number) => {
    const whole = Math.floor(beats / per), rest = beats - whole * per;
    const n = (k: number) => (k === 1 ? '1 bar' : `${k} bars`);
    return rest ? `${n(whole)} of ${per} and ${rest} beat${rest === 1 ? '' : 's'}` : `${n(whole)} of ${per}`;
  };
  const p = patternOf(s, m1.id);
  if (!p) return `${s.name} plays as written: ${bars(s.beats, m1.bars.beats)}.`;
  const played = playOrder(s.beats * PPQ, m1.bars.beats, p).reduce((n, x) => n + x.len, 0) / PPQ;
  return `${s.name} plays ${bars(played, p.length)} (written ${bars(s.beats, m1.bars.beats)}).`;
}

export class MeterRow {
  private dragFrom = -1;

  constructor(private app: App, private section: () => Section | undefined, private layer: () => Layer | undefined, private acted: () => void) {
    $<HTMLSelectElement>('meterLock').addEventListener('change', () => this.lock($<HTMLSelectElement>('meterLock').value));
    $<HTMLInputElement>('meterBeats').addEventListener('change', () => {
      const v = Number($<HTMLInputElement>('meterBeats').value);
      const m = this.meter();
      if (!m || !Number.isInteger(v) || v < MIN_METER_BEATS || v > MAX_METER_BEATS) { this.render(); return; }
      this.app.run(() => this.app.store.dispatch('setMeter', { id: m.id, beats: v }));
    });
    $<HTMLInputElement>('meterPattern').addEventListener('change', () => {
      const text = $<HTMLInputElement>('meterPattern').value;
      this.setPlay(text.trim() === '' ? null : parsePattern(text) ?? []);
    });
    $<HTMLSelectElement>('meterPreset').addEventListener('change', () => {
      const sel = $<HTMLSelectElement>('meterPreset');
      const m = this.meter();
      const pick = m ? presets(m.bars.beats)[Number(sel.value)] : undefined;
      sel.value = '';
      if (pick) this.setPlay(pick.play);
    });
    $('meterAdd').addEventListener('click', () => {
      const m = this.meter();
      const cur = this.pattern();
      if (!m) return;
      const missing = Array.from({ length: m.bars.beats }, (_, i) => i + 1).find((b) => !cur.includes(b));
      this.setPlay([...cur, missing ?? cur[cur.length - 1] ?? 1]);
    });
    $('meterAll').addEventListener('click', () => {
      const s = this.section(), m = this.meter();
      if (!s || !m) return;
      const play = patternOf(s, m.id) ?? null;
      this.app.run(() => this.app.store.dispatch('setMeterPlay', { section: s.id, meter: m.id, play, all: true }));
    });
    const chips = $('meterChips');
    chips.addEventListener('keydown', (e) => this.onKey(e));
    chips.addEventListener('pointerup', (e) => this.drop(e));
    chips.addEventListener('pointercancel', () => { this.dragFrom = -1; this.paintDrag(-1); });
  }

  /** The meter the row edits: the selected layer's, or Meter 1 when it is free or mixed. */
  private meter(): Meter | undefined {
    const doc = this.app.doc;
    const l = this.layer();
    const lock = l ? trackLock(doc, l.id) : '';
    return metersOf(doc).find((m) => m.id === lock) ?? metersOf(doc)[0];
  }

  private pattern(): number[] {
    const s = this.section(), m = this.meter();
    const p = s && m ? patternOf(s, m.id) : undefined;
    return p ? [...p] : m ? Array.from({ length: m.bars.beats }, (_, i) => i + 1) : [];
  }

  private lock(value: string): void {
    const l = this.layer();
    if (!l || value === 'mixed') return;
    if (this.app.run(() => this.app.store.dispatch('lockClips', { layer: l.id, meter: value || null }))) this.acted();
  }

  /** Set the section's pattern; one that keeps every beat in order is "as written". */
  private setPlay(play: number[] | null): void {
    const s = this.section(), m = this.meter();
    if (!s || !m) return;
    const plain = play && play.length === m.bars.beats && play.every((b, i) => b === i + 1);
    if (this.app.run(() => this.app.store.dispatch('setMeterPlay', { section: s.id, meter: m.id, play: plain ? null : play }))) this.acted();
  }

  private edit(change: (p: number[]) => void, focus: number): void {
    const p = this.pattern();
    change(p);
    if (!p.length || p.length > MAX_PATTERN) return;
    this.focusAfter = focus;
    this.setPlay(p);
  }

  private focusAfter = -1;

  private onKey(e: KeyboardEvent): void {
    const t = e.target;
    if (!(t instanceof HTMLElement) || !t.classList.contains('beat')) return;
    const i = Number(t.dataset.i);
    const n = this.pattern().length;
    const move = (to: number) => this.focusChip(Math.max(0, Math.min(n - 1, to)));
    if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
      const d = e.key === 'ArrowLeft' ? -1 : 1;
      e.preventDefault();
      e.stopPropagation();
      if (e.shiftKey) {
        const j = i + d;
        if (j < 0 || j >= n) return;
        this.edit((p) => { [p[i], p[j]] = [p[j], p[i]]; }, j);
      } else move(i + d);
    } else if (e.key === 'Home' || e.key === 'End') {
      e.preventDefault();
      move(e.key === 'Home' ? 0 : n - 1);
    } else if (e.key === 'Delete' || e.key === 'Backspace') {
      e.preventDefault();
      e.stopPropagation();
      if (n > 1) this.edit((p) => p.splice(i, 1), Math.min(i, n - 2));
    }
  }

  private focusChip(i: number): void {
    const chips = [...$('meterChips').querySelectorAll<HTMLButtonElement>('button.beat')];
    chips.forEach((c, k) => { c.tabIndex = k === i ? 0 : -1; });
    chips[i]?.focus();
  }

  private paintDrag(over: number): void {
    $('meterChips').querySelectorAll<HTMLElement>('.beat-chip').forEach((c, k) => {
      c.classList.toggle('dragging', k === this.dragFrom);
      c.classList.toggle('drop', over >= 0 && k === over && k !== this.dragFrom);
    });
  }

  private chipAt(e: PointerEvent): number {
    const el = document.elementFromPoint(e.clientX, e.clientY)?.closest<HTMLElement>('.beat-chip');
    return el ? Number(el.dataset.i) : -1;
  }

  private drop(e: PointerEvent): void {
    const from = this.dragFrom;
    this.dragFrom = -1;
    this.paintDrag(-1);
    if (from < 0) return;
    const to = this.chipAt(e);
    if (to < 0 || to === from) return;
    this.edit((p) => { const [b] = p.splice(from, 1); p.splice(to, 0, b); }, to);
  }

  render(): void {
    const doc = this.app.doc;
    const s = this.section();
    const l = this.layer();
    const meters = metersOf(doc);
    const grid = s?.feel === 'grid';
    const lock = l ? trackLock(doc, l.id) : '';
    const m = this.meter();
    const sel = $<HTMLSelectElement>('meterLock');
    const opts: [string, string][] = [['', 'Free'], ...meters.map((x) => [x.id, x.name] as [string, string])];
    if (lock === 'mixed') opts.push(['mixed', 'Mixed: clips differ']);
    const sig = opts.map((o) => o.join('=')).join('|');
    if (sel.dataset.sig !== sig) {
      sel.dataset.sig = sig;
      sel.replaceChildren(...opts.map(([v, t]) => { const o = new Option(t, v); o.disabled = v === 'mixed'; return o; }));
    }
    const hasClips = !!l && doc.sections.some((x) => x.clips[l.id]);
    sel.disabled = !hasClips;
    if (document.activeElement !== sel) sel.value = lock;
    const beats = $<HTMLInputElement>('meterBeats');
    if (document.activeElement !== beats) beats.value = String(m?.bars.beats ?? 4);
    $('meterBeatsName').textContent = m?.id === METER1 || !m ? 'Bar' : `${m.name} bar`;
    // The pattern: chips, the typed text, presets.
    const p = this.pattern();
    const set = !!(s && m && patternOf(s, m.id));
    const box = $('meterChips');
    const focused = box.contains(document.activeElement) ? Number((document.activeElement as HTMLElement).dataset.i) : -1;
    const keep = this.focusAfter >= 0 ? this.focusAfter : focused;
    this.focusAfter = -1;
    box.replaceChildren(...p.map((b, i) => this.chip(b, i, p.length, grid, keep < 0 ? i === 0 : i === Math.min(keep, p.length - 1))));
    if (keep >= 0) this.focusChip(Math.min(keep, p.length - 1));
    const text = $<HTMLInputElement>('meterPattern');
    if (document.activeElement !== text) text.value = set ? p.join(' ') : '';
    text.placeholder = m ? `${p.join(' ')} (as written)` : '';
    const pre = $<HTMLSelectElement>('meterPreset');
    const psig = String(m?.bars.beats ?? 0);
    if (pre.dataset.sig !== psig && m) {
      pre.dataset.sig = psig;
      pre.replaceChildren(new Option('Presets…', ''), ...presets(m.bars.beats).map((x, i) => new Option(x.name, String(i))));
    }
    for (const id of ['meterBeats', 'meterPattern', 'meterPreset', 'meterAdd', 'meterAll']) ($(id) as HTMLInputElement).disabled = !grid;
    ($('meterAdd') as HTMLButtonElement).disabled = !grid || p.length >= MAX_PATTERN;
    // Live status: how the section plays, and what the layer does in it.
    const c = s && l ? s.clips[l.id] : undefined;
    const mine = c?.meter && meters.some((x) => x.id === c.meter) ? meters.find((x) => x.id === c.meter)! : null;
    let note = s ? sectionPlays(doc, s) : '';
    if (s && l && grid) {
      note += !c ? ` ${l.name} has no clip here.`
        : !mine ? ` ${l.name} is free: it plays as written.`
        : patternOf(s, mine.id) ? ` ${l.name} follows ${mine.name}.` : ` ${l.name} is locked to ${mine.name}, which plays as written here.`;
    }
    $('meterNote').textContent = note;
    $('meterSummary').textContent = !l ? '' : !grid ? 'a free section has no beats'
      : `${lock === 'mixed' ? 'clips differ' : mine ? mine.name : 'free'}${set ? ` · plays ${p.join(' ')}` : ' · as written'}`;
  }

  private chip(beat: number, i: number, n: number, enabled: boolean, tabbable: boolean): HTMLElement {
    const wrap = document.createElement('span');
    wrap.className = 'beat-chip';
    wrap.dataset.i = String(i);
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'beat';
    b.dataset.i = String(i);
    b.textContent = String(beat);
    b.tabIndex = tabbable ? 0 : -1;
    b.disabled = !enabled;
    b.setAttribute('aria-label', `Beat ${i + 1} plays source beat ${beat}. Press to repeat it; Shift and an arrow moves it; Delete drops it.`);
    b.title = 'Press to repeat this beat; drag to move it';
    b.addEventListener('click', () => {
      if (this.pattern().length < MAX_PATTERN) this.edit((p) => p.splice(i + 1, 0, p[i]), i + 1);
    });
    b.addEventListener('pointerdown', (e) => {
      if (e.button !== 0) return;
      this.dragFrom = i;
      // Without the implicit capture a finger's pointerup lands on the chip it was lifted over, and
      // a drag that ends on another chip fires no click on this one (so it is a move, not a repeat).
      if (b.hasPointerCapture(e.pointerId)) b.releasePointerCapture(e.pointerId);
    });
    b.addEventListener('pointermove', () => { if (this.dragFrom >= 0) this.paintDrag(i); });
    wrap.addEventListener('pointerenter', () => { if (this.dragFrom >= 0) this.paintDrag(i); });
    const x = document.createElement('button');
    x.type = 'button';
    x.className = 'beat-x';
    x.textContent = '×';
    x.tabIndex = -1;
    x.disabled = !enabled || n < 2;
    x.setAttribute('aria-label', `Drop beat ${i + 1} (source beat ${beat})`);
    x.title = 'Drop this beat';
    x.addEventListener('click', () => this.edit((p) => p.splice(i, 1), Math.min(i, n - 2)));
    wrap.append(b, x);
    return wrap;
  }
}
