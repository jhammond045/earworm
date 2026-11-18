// Sound Influence for the selected layer (TDD "Sound Influence"): pick or drop a source, or hold Listen and
// let the sound move while the match is rerun on what the mic has heard; blend by amount and by aspect;
// play the source itself through the grain cloud; commit the blend into the editing state.
//
// The mic is only ever on because the user turned it on (Enable mic). Nothing here checks for speaker bleed
// or mutes anything.

import { decodeBytes, MAX_SOURCE_S, loadSources, trimAndCap, type SavedSource } from '../influence/audio';
import { LAYER_INFLUENCE_AMOUNT, LAYER_INFLUENCE_MASK, M_LAYER_PEAK, SET_INFLUENCE } from '../m1/protocol';
import { liveMatch, type LiveMatch, type Match } from '../m4/match';
import { commands, newId } from '../project/commands';
import { applyOps } from '../project/patch';
import type { Influence, Layer, Project } from '../project/types';
import { $, type App } from './app';
import { holdButton, MicClip, typingIn } from './hold';
import { focused, h, nextId, rangeCtl } from './panels';
import { ASPECTS, readoutOf, maskCount } from './readout';

const C4_CENTS = 4800;
/** Listening and recording stop by themselves here; the engine's mic ring holds a minute. */
const MIN_LISTEN_S = 0.1;

/** Does the layer need LOAD_STATES in the engine (compile.ts): two or more states, or a morph marker. */
export function layerUsesStates(doc: Project, l: Layer): boolean {
  return l.states.length >= 2 || doc.sections.some((s) => (s.clips[l.id]?.morph?.length ?? 0) > 0);
}

/** The grain region to store for a match: the suggested one, else the whole source when the sound is meant to play. */
export function grainFor(m: Match, length: number): Influence['grain'] {
  if (m.grain) return { start: m.grain.start, end: m.grain.end, refCents: m.grain.refCents };
  return m.useSound ? { start: 0, end: length, refCents: m.pitched ? m.pitchCents : null } : null;
}

export interface InfluencePanel { refresh(doc: Project): void }

export function initInfluence(app: App): InfluencePanel {
  const root = $('influencePanel');
  const layer = (doc = app.doc): Layer | undefined => doc.layers.find((l) => l.id === app.ui.layerId);
  const layerIdx = (id: string) => app.doc.layers.findIndex((l) => l.id === id);

  // ---------------------------------------------------------------- DOM

  const status = h('p', { class: 'small mono', id: 'infStatus' });
  status.setAttribute('role', 'status');
  const sourceInfo = h('p', { class: 'small muted', id: 'infSource' });
  const heading = h('p', { class: 'small', id: 'infFor' });

  const fileId = nextId('inf-file');
  const file = h('input', { type: 'file', id: fileId, accept: 'audio/*' });
  const drop = h('div', { class: 'drop', tabIndex: 0 }, 'Drop an audio file here');
  drop.setAttribute('role', 'button');
  drop.setAttribute('aria-label', 'Drop an audio file here, or press Enter to choose one');
  const savedId = nextId('inf-saved');
  const saved = h('select', { id: savedId });
  const savedUse = h('button', { type: 'button' }, 'Use saved source');
  const savedRefresh = h('button', { type: 'button' }, 'Refresh list');
  const listenBtn = h('button', { type: 'button', id: 'infListen', class: 'hold' }, 'Listen (hold, or hold L)');
  listenBtn.setAttribute('aria-keyshortcuts', 'L');
  listenBtn.title = 'Hold to record from the mic. The sound moves while it listens; let go to keep the match.';

  const sourceGroup = h('fieldset', { class: 'sub' }, h('legend', {}, 'Source'),
    h('div', { class: 'row' },
      h('label', { htmlFor: fileId }, 'Audio file ', file), drop,
      h('label', { htmlFor: savedId }, 'Saved source (from the Influence page) ', saved), savedUse, savedRefresh),
    h('div', { class: 'bar' }, listenBtn), status, sourceInfo);

  // Readout: five aspect bars and five named parameters.
  const bars = ASPECTS.map((a) => {
    const fill = h('span', { class: 'fill' });
    const track = h('span', { class: 'track' }, fill);
    const num = h('span', { class: 'mono small' });
    const row = h('div', { class: 'meter', role: 'img' }, h('span', { class: 'small lbl' }, a.label), track, num);
    return { row, fill, num };
  });
  const named = h('div', { class: 'named mono small' });
  const provisional = h('span', { class: 'tag', hidden: true }, 'provisional');
  provisional.title = 'The motion rows rest on under two cycles, or under 2 s of steady sound; they sharpen with more audio.';
  const readoutGroup = h('fieldset', { class: 'sub' }, h('legend', {}, 'Target, compared with the editing state'),
    h('div', { class: 'meters' }, ...bars.map((b) => b.row)), named, provisional);
  readoutGroup.setAttribute('aria-live', 'off');

  // Blend controls.
  const blendGrid = h('div', { class: 'grid' });
  const amountUp = rangeCtl(blendGrid, {
    label: 'Amount (of the editing state)', min: 0, max: 100, step: 1, digits: 0, unit: '%',
    get: (d) => { const l = layer(d); return l ? Math.round(app.ui.editState(l).influenceAmount * 100) : 0; },
    preview: (v) => previewAmount(v / 100),
    commit: (v) => {
      const l = layer();
      if (l) app.runRestoring(() => app.store.dispatch('setInfluenceAmount', { layer: l.id, state: app.ui.editState(l).id, amount: v / 100 }));
    },
  });
  const amountInput = blendGrid.querySelector('input') as HTMLInputElement;
  amountInput.setAttribute('aria-label', 'Influence amount of the editing state, percent');

  const aspectBoxes = ASPECTS.map((a) => {
    const id = nextId('inf-asp');
    const box = h('input', { type: 'checkbox', id });
    box.addEventListener('change', () => setMask());
    return { a, box, label: h('label', { htmlFor: id, class: 'small' }, box, ` ${a.label}`) };
  });
  const aspects = h('div', { class: 'bar', role: 'group' }, ...aspectBoxes.map((x) => x.label));
  aspects.setAttribute('aria-label', 'Aspects the Influence blends: tone, shape, motion, grit, space');
  const soundId = nextId('inf-sound');
  const useSound = h('input', { type: 'checkbox', id: soundId });
  const useSoundLabel = h('label', { htmlFor: soundId, class: 'small' }, useSound, ' Use the sound itself (grain cloud from the source\'s steady region)');
  const commitBtn = h('button', { type: 'button', id: 'infCommit' }, 'Commit');
  commitBtn.title = 'Write the blended sound into the editing state and clear the target';
  const clearBtn = h('button', { type: 'button', id: 'infClear' }, 'Clear');
  clearBtn.title = 'Remove the target and the source';
  const blendGroup = h('fieldset', { class: 'sub' }, h('legend', {}, 'Blend'), blendGrid, aspects, h('div', {}, useSoundLabel),
    h('div', { class: 'bar' }, commitBtn, clearBtn));

  // Audition.
  const noteBtn = h('button', { type: 'button', id: 'infNote', class: 'hold' }, 'Hold to play a note');
  const srcBtn = h('button', { type: 'button', id: 'infPlaySrc' }, 'Play source');
  const peak = h('span', { class: 'mono small muted', id: 'infPeak' });
  const auditionGroup = h('fieldset', { class: 'sub' }, h('legend', {}, 'Audition'), h('div', { class: 'bar' }, noteBtn, srcBtn, peak));

  root.append(heading, sourceGroup, readoutGroup, blendGroup, auditionGroup);

  // ---------------------------------------------------------------- readout

  let liveTarget: ArrayLike<number> | null = null;
  let liveProvisional = false;
  let lastProvisional = false;

  function drawReadout(target: ArrayLike<number> | null, base: ArrayLike<number>, prov: boolean): void {
    if (!target) {
      for (const b of bars) { b.fill.style.left = '50%'; b.fill.style.width = '0%'; b.num.textContent = ''; b.row.setAttribute('aria-label', `${b.row.firstChild?.textContent}: no target`); }
      named.textContent = 'No target yet.';
      provisional.hidden = true;
      return;
    }
    const r = readoutOf(target, base);
    r.groups.forEach((g, i) => {
      const b = bars[i];
      const half = Math.min(1, Math.abs(g.delta) / 0.25) * 50; // half the track is a mean change of 0.25 of the range
      b.fill.style.left = g.delta >= 0 ? '50%' : `${50 - half}%`;
      b.fill.style.width = `${half}%`;
      b.fill.dataset.sign = g.delta >= 0 ? 'up' : 'down';
      const txt = `${g.delta >= 0 ? '+' : ''}${(g.delta * 100).toFixed(0)}`;
      b.num.textContent = txt;
      b.row.setAttribute('aria-label', `${g.label}: mean change ${txt} percent of range from the editing state`);
    });
    named.replaceChildren(...r.named.map((n) => h('div', {}, `${n.label}: ${n.value} `, h('span', { class: 'muted' }, `(was ${n.base})`))));
    provisional.hidden = !prov;
  }

  // ---------------------------------------------------------------- sending to the engine

  /** Preview a new amount: the engine's layer field at once; when the layer's states are loaded they own the amount, so the document is compiled too. */
  function previewAmount(a: number): void {
    const l = layer();
    if (!l) return;
    const i = layerIdx(l.id);
    app.host?.setLayer(i, LAYER_INFLUENCE_AMOUNT, a);
    if (l.influence && layerUsesStates(app.doc, l)) {
      app.previewDoc((d) => applyOps(d, commands.setInfluenceAmount(d, { layer: l.id, state: app.ui.editState(l).id, amount: a })).doc);
    }
  }

  /** A new target straight into the engine: all 96 values, the amount and the mask. */
  function sendTarget(l: Layer, target: ArrayLike<number>, amount: number, mask: number): void {
    const host = app.host;
    if (!host) return;
    const i = layerIdx(l.id);
    if (i < 0) return;
    for (let k = 0; k < 96; k++) host.setParam(i, k, target[k], SET_INFLUENCE);
    host.setLayer(i, LAYER_INFLUENCE_AMOUNT, amount);
    host.setLayer(i, LAYER_INFLUENCE_MASK, mask);
    if (layerUsesStates(app.doc, l)) {
      // Loaded states override the layer's own base, macros and amount, so give the compiler a document that carries the target too.
      const st = app.ui.editState(l);
      app.previewDoc((d) => {
        const infl: Influence = { target: Array.from(target), mask, source: 'grain-live', sourceName: 'Listening', sourceRate: 48000, grain: null, useSound: false };
        let out = applyOps(d, commands.setInfluence(d, { layer: l.id, influence: infl })).doc;
        out = applyOps(out, commands.setInfluenceAmount(out, { layer: l.id, state: st.id, amount })).doc;
        return out;
      });
    }
  }

  // ---------------------------------------------------------------- applying a match

  let busy = false;
  const say = (s: string) => { status.textContent = s; };

  /** Store the source, commit the Influence for the layer. `m` is the whole-source match. */
  async function commitMatch(layerId: string, signal: Float32Array, rate: number, name: string, m: Match): Promise<void> {
    const ref = `grain-${newId('s').slice(2)}`;
    try {
      await app.session.putGrain(ref, signal);
    } catch (e) {
      app.fail(`The source audio could not be saved (${e instanceof Error ? e.message : String(e)}). It stays in memory until the page reloads.`);
    }
    const l = app.doc.layers.find((x) => x.id === layerId);
    if (!l) return;
    const st = app.ui.editState(l);
    const influence: Influence = {
      target: Array.from(m.target), mask: 0x1f, source: ref, sourceName: name, sourceRate: rate,
      grain: grainFor(m, signal.length), useSound: m.useSound,
    };
    lastProvisional = m.provisional;
    app.runRestoring(() => app.store.batch('Set Influence', (run) => {
      run('setInfluence', { layer: layerId, influence });
      // A target at 0% would change nothing the user can hear, so a new one starts at 100%.
      if (st.influenceAmount === 0) run('setInfluenceAmount', { layer: layerId, state: st.id, amount: 1 });
    }));
    say(`${name}: ${(signal.length / rate).toFixed(2)} s, ${m.pitched ? 'pitched' : 'unpitched'}, matched in ${m.ms.toFixed(0)} ms.${m.provisional ? ' Motion is provisional.' : ''}`);
  }

  async function useSignal(signal: Float32Array, rate: number, name: string): Promise<void> {
    const l = layer();
    if (!l || busy) return;
    busy = true;
    try {
      say(`Matching ${name}...`);
      const matcher = await app.matcher();
      const m = await matcher.match(signal, rate);
      await commitMatch(l.id, signal, rate, name, m);
    } catch (e) {
      say('');
      app.fail(`The match failed: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      busy = false;
    }
  }

  async function useBytes(bytes: ArrayBuffer, name: string): Promise<void> {
    try {
      const ctx: BaseAudioContext = app.ctx ?? new OfflineAudioContext(1, 1, 48000);
      const signal = await decodeBytes(ctx, bytes);
      await useSignal(signal, ctx.sampleRate, name);
    } catch (e) {
      say('');
      app.fail(`That file could not be used: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  async function useFile(f: File | undefined): Promise<void> {
    if (!f) return;
    if (!layer()) { app.fail('Pick a layer first.'); return; }
    await useBytes(await f.arrayBuffer(), f.name);
  }
  file.addEventListener('change', () => { void useFile(file.files?.[0]).then(() => { file.value = ''; }); });

  drop.addEventListener('dragover', (e) => { e.preventDefault(); drop.classList.add('over'); });
  drop.addEventListener('dragleave', () => drop.classList.remove('over'));
  drop.addEventListener('drop', (e) => {
    e.preventDefault();
    drop.classList.remove('over');
    void useFile(e.dataTransfer?.files[0]);
  });
  drop.addEventListener('click', () => file.click());
  drop.addEventListener('keydown', (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); file.click(); } });

  // Saved sources from the M0 Influence page (IndexedDB).
  let savedList: SavedSource[] = [];
  async function fillSaved(): Promise<void> {
    try {
      savedList = await loadSources();
    } catch {
      savedList = [];
    }
    saved.replaceChildren(...(savedList.length
      ? savedList.map((s, i) => new Option(`${s.name} (${(s.signal.length / s.sampleRate).toFixed(1)} s)`, String(i)))
      : [new Option('None saved', '')]));
    savedUse.disabled = savedList.length === 0;
  }
  savedRefresh.addEventListener('click', () => { void fillSaved(); });
  saved.addEventListener('focus', () => { void fillSaved(); });
  savedUse.addEventListener('click', () => {
    const s = savedList[Number(saved.value)];
    if (s) void useSignal(trimAndCap(s.signal, s.sampleRate), s.sampleRate, s.name);
  });
  void fillSaved();

  // ---------------------------------------------------------------- Listen

  interface Listening { layerId: string; clip: MicClip; live: LiveMatch; timer: number }
  let listening: Listening | null = null;
  let listenWanted = false;

  async function startListen(): Promise<void> {
    if (listenWanted || busy) return;
    const l = layer();
    const { host, ctx } = app;
    if (!l) { app.fail('Pick a layer first.'); return; }
    if (!host || !ctx) { say('Press Start first.'); return; }
    if (!app.stream) { say('The mic is off. Press Enable mic first; nothing turns it on for you.'); return; }
    listenWanted = true;
    let matcher;
    try {
      matcher = await app.matcher();
    } catch (e) {
      listenWanted = false;
      app.fail(`The match could not start: ${e instanceof Error ? e.message : String(e)}`);
      return;
    }
    if (!listenWanted) return; // let go before the worker was ready
    const clip = new MicClip(host, ctx.sampleRate, MAX_SOURCE_S);
    const st = app.ui.editState(l);
    const amount = st.influenceAmount > 0 ? st.influenceAmount : 1;
    const mask = l.influence ? l.influence.mask : 0x1f;
    const live = liveMatch(() => clip.read(), matcher, (m) => {
      liveTarget = m.target;
      liveProvisional = m.provisional;
      const cur = app.doc.layers.find((x) => x.id === l.id);
      if (cur && listening) sendTarget(cur, m.target, amount, mask);
      refreshReadout();
    }, { sampleRate: ctx.sampleRate, onError: (e) => say(`Match failed: ${e.message}`) });
    const timer = window.setInterval(() => say(`Listening to ${l.name}: ${clip.seconds.toFixed(1)} s`), 200);
    window.setTimeout(() => { if (listening?.live === live) void endListen(); }, MAX_SOURCE_S * 1000);
    listening = { layerId: l.id, clip, live, timer };
    listenBtn.classList.add('down');
    say(`Listening to ${l.name}...`);
  }

  async function endListen(): Promise<void> {
    listenWanted = false;
    const a = listening;
    if (!a) return;
    listening = null;
    window.clearInterval(a.timer);
    listenBtn.classList.remove('down');
    const signal = a.clip.read();
    const rate = a.clip.rate;
    if (signal.length < MIN_LISTEN_S * rate) {
      a.live.stop();
      liveTarget = null;
      void app.sync.request(app.doc);
      say('Too short to match. Hold a little longer.');
      refreshReadout();
      return;
    }
    busy = true;
    try {
      // "On release the same code runs once over the whole capture and the result replaces the running estimate."
      const m = await a.live.finish();
      liveTarget = null;
      // The capture is stored as heard (not trimmed), so the grain region from the match indexes it directly.
      await commitMatch(a.layerId, signal, rate, `Listen ${new Date().toLocaleTimeString()}`, m);
    } catch (e) {
      liveTarget = null;
      void app.sync.request(app.doc);
      app.fail(`The match failed: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      busy = false;
    }
  }

  holdButton(listenBtn, () => { void startListen(); }, () => { void endListen(); });
  window.addEventListener('keydown', (e) => {
    if (e.key.toLowerCase() !== 'l' || e.ctrlKey || e.metaKey || e.altKey || typingIn(e.target)) return;
    e.preventDefault();
    if (!e.repeat) void startListen();
  });
  window.addEventListener('keyup', (e) => {
    if (e.key.toLowerCase() !== 'l' || typingIn(e.target)) return;
    void endListen();
  });

  // ---------------------------------------------------------------- blend controls

  function setMask(): void {
    const l = layer();
    if (!l?.influence) return;
    const mask = aspectBoxes.reduce((m, x) => m | (x.box.checked ? x.a.bit : 0), 0);
    app.run(() => app.store.dispatch('setInfluenceMask', { layer: l.id, mask }));
  }

  useSound.addEventListener('change', async () => {
    const l = layer();
    const infl = l?.influence;
    if (!l || !infl || busy) return;
    const want = useSound.checked;
    busy = true;
    try {
      // The match raises src.grain_mix in the target when the sound is used, so it is rerun with the choice.
      await app.session.loadGrain(infl.source);
      const data = app.session.grains.get(infl.source);
      if (!data) throw new Error('the source audio is not available');
      const m = await (await app.matcher()).match(data, infl.sourceRate, { useSound: want });
      const next: Influence = { ...infl, target: Array.from(m.target), useSound: want, grain: want ? grainFor(m, data.length) : m.grain ? { ...m.grain } : infl.grain };
      lastProvisional = m.provisional;
      app.run(() => app.store.dispatch('setInfluence', { layer: l.id, influence: next }));
    } catch (e) {
      app.fail(`The match failed: ${e instanceof Error ? e.message : String(e)}`);
      app.refreshAll();
    } finally {
      busy = false;
    }
  });

  commitBtn.addEventListener('click', () => {
    const l = layer();
    if (l?.influence) app.run(() => app.store.dispatch('commitInfluence', { layer: l.id, state: app.ui.editState(l).id }));
  });
  clearBtn.addEventListener('click', () => {
    const l = layer();
    if (l?.influence && app.run(() => app.store.dispatch('setInfluence', { layer: l.id, influence: null }))) say('Cleared.');
  });

  // ---------------------------------------------------------------- audition

  let noteOn: { layerIdx: number; cents: number; timer: number } | null = null;
  holdButton(noteBtn, () => {
    const l = layer();
    const host = app.host;
    if (!l || !host) { say('Press Start first.'); return; }
    const i = layerIdx(l.id);
    // The source's own pitch when it has one; C4 otherwise (the grain cloud then plays an unpitched source at its own speed).
    const cents = l.influence?.grain?.refCents ?? C4_CENTS;
    void host.ctx.resume();
    host.noteOn(i, cents, 0);
    noteOn = { layerIdx: i, cents, timer: window.setInterval(showPeak, 100) };
  }, () => {
    const n = noteOn;
    if (!n) return;
    noteOn = null;
    window.clearInterval(n.timer);
    app.host?.noteOff(n.layerIdx, n.cents);
    window.setTimeout(showPeak, 300);
  });
  function showPeak(): void {
    const m = app.readMeters();
    const l = layer();
    if (!m || !l) return;
    peak.textContent = `Layer peak ${m[M_LAYER_PEAK + layerIdx(l.id)].toFixed(3)}`;
  }

  let srcNode: AudioBufferSourceNode | null = null;
  function stopSource(): void {
    const n = srcNode;
    srcNode = null;
    if (n) { n.onended = null; try { n.stop(); } catch { /* already stopped */ } n.disconnect(); }
    srcBtn.textContent = 'Play source';
  }
  srcBtn.addEventListener('click', async () => {
    if (srcNode) { stopSource(); return; }
    const l = layer();
    const infl = l?.influence;
    const ctx = app.ctx;
    if (!infl || !ctx) { say(ctx ? 'This layer has no source.' : 'Press Start first.'); return; }
    await app.session.loadGrain(infl.source);
    const data = app.session.grains.get(infl.source);
    if (!data) { say('The source audio is not available.'); return; }
    await ctx.resume();
    const buf = ctx.createBuffer(1, data.length, infl.sourceRate);
    buf.copyToChannel(data as Float32Array<ArrayBuffer>, 0);
    const node = ctx.createBufferSource();
    node.buffer = buf;
    node.connect(ctx.destination);
    node.onended = () => { if (srcNode === node) stopSource(); };
    node.start();
    srcNode = node;
    srcBtn.textContent = 'Stop source';
  });

  // ---------------------------------------------------------------- refresh

  function refreshReadout(): void {
    const l = layer();
    if (!l) return;
    const base = app.ui.editState(l).base;
    if (liveTarget) drawReadout(liveTarget, base, liveProvisional);
    else drawReadout(l.influence?.target ?? null, base, !!l.influence && lastProvisional);
  }

  let warmed = false;
  return {
    refresh(doc) {
      const l = layer(doc);
      root.hidden = !l;
      if (!l) return;
      if (!warmed && app.host) { warmed = true; void app.matcher().catch(() => { warmed = false; }); }
      const st = app.ui.editState(l);
      const infl = l.influence;
      heading.textContent = `Layer ${l.name}, editing state ${st.name}.`;
      const on = !!infl;
      for (const x of aspectBoxes) {
        if (!focused(x.box)) x.box.checked = !!infl && (infl.mask & x.a.bit) !== 0;
        x.box.disabled = !on;
      }
      useSound.disabled = !on;
      if (!focused(useSound)) useSound.checked = !!infl?.useSound;
      amountInput.disabled = !on;
      amountUp(doc);
      commitBtn.disabled = !on;
      clearBtn.disabled = !on;
      srcBtn.disabled = !on;
      const srcLen = infl ? app.session.grains.get(infl.source)?.length : undefined;
      sourceInfo.textContent = infl
        ? `Source: ${infl.sourceName}${srcLen ? `, ${(srcLen / infl.sourceRate).toFixed(2)} s` : ''} at ${infl.sourceRate} Hz. ` +
          `Grain region ${infl.grain ? `${(infl.grain.start / infl.sourceRate).toFixed(2)} to ${(infl.grain.end / infl.sourceRate).toFixed(2)} s` : 'none'}` +
          `${infl.grain?.refCents != null ? `, pitched at ${infl.grain.refCents.toFixed(0)} cents above C0` : ''}. ${maskCount(infl.mask)} of 5 aspects blend.`
        : 'No source yet: choose a file, a saved source, or hold Listen.';
      refreshReadout();
    },
  };
}
