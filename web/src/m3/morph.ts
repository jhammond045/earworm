// The small panel for the morph marker picked on the timeline: its state, shape, length and time. It also lists the
// selected lane's markers, so a marker can be picked and edited without the pointer.

import { MORPH_SHAPES, PPQ, type Layer, type MorphShape, type Project } from '../project/types';
import { $, type App } from './app';
import { focused, h, nextId } from './panels';

export interface MorphPanel { refresh(doc: Project): void }

const SHAPE_LABELS: Record<MorphShape, string> = {
  line: 'Line', ease: 'Ease (smooth)', late: 'Late (slow start)', step: 'Step (at once)', wobble: 'Wobble (overshoots)',
};

export function initMorph(app: App): MorphPanel {
  const root = $('morphPanel');
  const hint = h('p', { class: 'small muted' },
    'Morph lane: the strip under each lane. Double-click it (or press M in the timeline) to add a marker for the state being edited; drag to move; click to pick. ' +
    'The layer goes to a marker\'s state over its length, in its shape. Delete removes the picked marker; [ and ] pick the previous and next; arrows nudge it.');

  const pickId = nextId('mk-pick'), stateId = nextId('mk-state'), shapeId = nextId('mk-shape'), lenId = nextId('mk-len'), timeId = nextId('mk-time');
  const pick = h('select', { id: pickId });
  const state = h('select', { id: stateId });
  const shape = h('select', { id: shapeId });
  MORPH_SHAPES.forEach((sh) => shape.append(new Option(SHAPE_LABELS[sh], sh)));
  const len = h('input', { type: 'number', id: lenId, min: '0', step: '10' });
  const time = h('input', { type: 'number', id: timeId, min: '0', step: 'any' });
  const remove = h('button', { type: 'button', id: 'mkRemove' }, 'Remove marker');
  const fields = h('div', { class: 'row' },
    h('label', { htmlFor: pickId }, 'Marker ', pick), h('label', { htmlFor: stateId }, 'State ', state),
    h('label', { htmlFor: shapeId }, 'Shape ', shape), h('label', { htmlFor: lenId }, 'Length (ms) ', len),
    h('label', { htmlFor: timeId }, h('span', { id: 'mkTimeLbl' }, 'Time'), time), remove);
  root.append(hint, fields);

  const ctx = () => {
    const sec = app.doc.sections.find((s) => s.id === app.ui.sectionId);
    const layer = app.doc.layers.find((l) => l.id === app.ui.layerId);
    const clip = sec && layer ? sec.clips[layer.id] : undefined;
    return { sec, layer, clip };
  };
  const label = (layer: Layer, m: { t: number; state: string; shape: string; lenMs: number }, i: number, grid: boolean) =>
    `${i + 1}: ${layer.states.find((x) => x.id === m.state)?.name ?? '?'} at ${grid ? `beat ${(m.t / PPQ + 1).toFixed(2)}` : `${m.t.toFixed(2)} s`}`;

  /** Edit the picked marker; the list re-sorts when the time changes, so the pick follows the marker. */
  function edit(change: { state?: string; shape?: MorphShape; lenMs?: number; t?: number }): void {
    const { sec, layer, clip } = ctx();
    const sel = app.ui.marker;
    if (!sec || !layer || !clip || !sel || sel.layerId !== layer.id) return;
    const old = clip.morph[sel.index];
    if (!old) return;
    const ok = app.run(() => app.store.dispatch('moveMorphMarker', { section: sec.id, layer: layer.id, index: sel.index, ...change }));
    if (!ok) return;
    const next = { ...old, ...change };
    const list = app.doc.sections.find((q) => q.id === sec.id)?.clips[layer.id]?.morph ?? [];
    const at = list.findIndex((m) => m.t === next.t && m.state === next.state);
    app.ui.selectMarker({ layerId: layer.id, index: Math.max(0, at) });
  }

  pick.addEventListener('change', () => {
    const { layer } = ctx();
    if (layer && pick.value !== '') app.ui.selectMarker({ layerId: layer.id, index: Number(pick.value) });
  });
  state.addEventListener('change', () => edit({ state: state.value }));
  shape.addEventListener('change', () => edit({ shape: shape.value as MorphShape }));
  len.addEventListener('change', () => {
    const v = Number(len.value);
    if (!Number.isFinite(v) || v < 0) { app.refreshAll(); return; }
    edit({ lenMs: v });
  });
  time.addEventListener('change', () => {
    const { sec } = ctx();
    const v = Number(time.value);
    if (!sec || !Number.isFinite(v) || v < 0) { app.refreshAll(); return; }
    edit({ t: sec.feel === 'grid' ? Math.round((v - 1) * PPQ) : v });
  });
  remove.addEventListener('click', () => {
    const { sec, layer } = ctx();
    const sel = app.ui.marker;
    if (!sec || !layer || !sel) return;
    app.run(() => app.store.dispatch('removeMorphMarker', { section: sec.id, layer: layer.id, index: sel.index }));
    app.ui.selectMarker(null);
  });

  let sig = '';
  return {
    refresh(doc) {
      const { sec, layer, clip } = ctx();
      void doc;
      const marks = clip?.morph ?? [];
      const grid = sec?.feel !== 'free';
      const sel = app.ui.marker && layer && app.ui.marker.layerId === layer.id ? app.ui.marker.index : -1;
      const m = sel >= 0 ? marks[sel] : undefined;
      const s = `${layer?.id}|${marks.map((x) => `${x.t},${x.state},${x.lenMs},${x.shape}`).join(';')}|${layer?.states.map((x) => `${x.id}:${x.name}`).join(',')}`;
      if (s !== sig && layer) {
        sig = s;
        pick.replaceChildren(new Option(marks.length ? 'Pick a marker' : 'No markers in this lane', ''), ...marks.map((x, i) => new Option(label(layer, x, i, grid), String(i))));
        state.replaceChildren(...layer.states.map((x) => new Option(x.name, x.id)));
      }
      if (!focused(pick)) pick.value = sel >= 0 ? String(sel) : '';
      state.disabled = shape.disabled = len.disabled = time.disabled = remove.disabled = !m;
      if (m) {
        if (!focused(state)) state.value = m.state;
        if (!focused(shape)) shape.value = m.shape;
        if (!focused(len)) len.value = String(m.lenMs);
        if (!focused(time)) time.value = grid ? String(Math.round((m.t / PPQ + 1) * 1000) / 1000) : String(m.t);
      }
      $('mkTimeLbl').textContent = grid ? 'Time (beat) ' : 'Time (s) ';
    },
  };
}
