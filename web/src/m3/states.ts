// Sound states of the selected layer: the list (up to 8), which one the sound controls edit, and its macros.
// A state is a full sound (96 values), eight macro positions and an Influence amount; the morph lane blends them.

import { MACRO_NAMES, N_MACROS } from '../gen/params';
import { commands, newId } from '../project/commands';
import { applyOps } from '../project/patch';
import { MAX_STATES, type Layer, type Project } from '../project/types';
import { stateColor } from './geometry';
import { $, type App } from './app';
import { focused, h, nextId, rangeCtl, type Updater } from './panels';

export interface StatesPanel { refresh(doc: Project): void }

export function initStates(app: App): StatesPanel {
  const root = $('statesPanel');
  const heading = h('p', { class: 'small', id: 'statesFor' });
  const list = h('div', { class: 'states', role: 'radiogroup' });
  list.setAttribute('aria-label', 'Sound states of the selected layer; the chosen one is the state the sound, macros and Influence amount edit');
  const addBtn = h('button', { type: 'button', id: 'stateAdd' }, 'Add state');
  addBtn.title = 'A new state, copied from the one being edited';
  const bar = h('div', { class: 'bar' }, addBtn, h('span', { class: 'small muted', id: 'stateCount' }));
  const macros = h('details', { class: 'tune' }, h('summary', {}, 'Macros of the editing state'));
  const macroGrid = h('div', { class: 'grid' });
  macros.append(macroGrid);
  root.append(heading, list, bar, macros);

  const layer = (doc = app.doc): Layer | undefined => doc.layers.find((l) => l.id === app.ui.layerId);

  addBtn.addEventListener('click', () => {
    const l = layer();
    if (!l) return;
    const from = app.ui.editState(l).id;
    const id = newId('st');
    if (app.run(() => app.store.dispatch('addState', { layer: l.id, from, id }))) app.ui.setEditState(l, id);
  });

  // Macros: a drag previews the state through the compiler (the engine reads macros from the loaded states), one command on release.
  const macroUps: Updater[] = [];
  for (let k = 0; k < N_MACROS; k++) {
    macroUps.push(rangeCtl(macroGrid, {
      label: `Macro ${MACRO_NAMES[k]}`, min: 0, max: 1, step: 0.01, digits: 2,
      get: (d) => { const l = layer(d); return l ? app.ui.editState(l).macros[k] ?? 0.5 : 0.5; },
      preview: (v) => {
        const l = layer();
        if (l) app.previewDoc((d) => applyOps(d, commands.setState(d, { layer: l.id, id: app.ui.editState(l).id, macros: withMacro(app.ui.editState(l).macros, k, v) })).doc);
      },
      commit: (v) => {
        const l = layer();
        if (l) app.runRestoring(() => app.store.dispatch('setState', { layer: l.id, id: app.ui.editState(l).id, macros: withMacro(app.ui.editState(l).macros, k, v) }));
      },
    }));
  }

  let sig = '';
  const rows = new Map<string, { radio: HTMLInputElement; name: HTMLInputElement; dup: HTMLButtonElement; del: HTMLButtonElement }>();

  function build(l: Layer): void {
    rows.clear();
    list.replaceChildren();
    l.states.forEach((st, i) => {
      const rid = nextId('st');
      const radio = h('input', { type: 'radio', id: rid, value: st.id });
      radio.name = 'editState';
      radio.addEventListener('change', () => {
        const cur = layer();
        if (radio.checked && cur) app.ui.setEditState(cur, st.id);
      });
      const dot = h('span', { class: 'swatch' });
      dot.style.background = stateColor(i);
      const name = h('input', { type: 'text', size: 14 });
      name.setAttribute('aria-label', `Name of state ${i + 1}`);
      name.addEventListener('change', () => {
        const cur = layer();
        const v = name.value.trim();
        if (!cur) return;
        if (!v) { app.refreshAll(); return; }
        app.run(() => app.store.dispatch('setState', { layer: cur.id, id: st.id, name: v }));
      });
      const dup = h('button', { type: 'button' }, 'Duplicate');
      dup.setAttribute('aria-label', `Duplicate state ${i + 1}`);
      dup.addEventListener('click', () => {
        const cur = layer();
        const src = cur?.states.find((x) => x.id === st.id);
        if (!cur || !src) return;
        const id = newId('st');
        if (app.run(() => app.store.dispatch('addState', { layer: cur.id, from: st.id, id, name: `${src.name} copy` }))) app.ui.setEditState(cur, id);
      });
      const del = h('button', { type: 'button' }, 'Remove');
      del.setAttribute('aria-label', `Remove state ${i + 1}`);
      del.addEventListener('click', () => {
        const cur = layer();
        if (cur) app.run(() => app.store.dispatch('removeState', { layer: cur.id, id: st.id }));
      });
      const row = h('div', { class: 'row state' }, h('label', { htmlFor: rid, class: 'small' }, radio, ` ${i + 1} `), dot, name, dup, del);
      rows.set(st.id, { radio, name, dup, del });
      list.append(row);
    });
  }

  return {
    refresh(doc) {
      const l = layer(doc);
      root.hidden = !l;
      if (!l) return;
      heading.textContent = `Layer ${l.name}: pick the state that the Sound, Macros and Influence amount below edit.`;
      const s = `${l.id}|${l.states.map((x) => x.id).join(',')}`;
      if (s !== sig) { sig = s; build(l); }
      const cur = app.ui.editState(l);
      l.states.forEach((st) => {
        const r = rows.get(st.id);
        if (!r) return;
        r.radio.checked = st.id === cur.id;
        if (!focused(r.name)) r.name.value = st.name;
        r.del.disabled = l.states.length <= 1;
        r.dup.disabled = l.states.length >= MAX_STATES;
      });
      addBtn.disabled = l.states.length >= MAX_STATES;
      $('stateCount').textContent = `${l.states.length} of ${MAX_STATES} states`;
      macroUps.forEach((u) => u(doc));
    },
  };
}

function withMacro(macros: number[], k: number, v: number): number[] {
  const out = macros.slice();
  out[k] = v;
  return out;
}
