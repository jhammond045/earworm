// Earworm M3 page: project bar, sections, layers, timeline, overdub capture and the gate checks.
//
// For automated runs the page exposes `window.earworm`:
//   store     the open project's Store (dispatch, undo, redo, doc, history); it changes when another project is opened
//   db        the ProjectDb (autosave journal and snapshots)
//   host      the KlontHost (null until Start)
//   compiler  the Compiler that diffs documents into engine blobs
//   gate      { runStress(seed?), reloadCheck(), crashCheck(), last, lastReload, lastCrash }
//   session, sync, ui: the pieces behind the above, for poking at in the console

import { KlontHost, wantsFallback } from '../m1/klont-host';
import { M_ACTIVE_VOICES, M_BPM, M_LOAD, M_OVERLOADS, M_PEAK_L, M_PEAK_R, M_PLAYING, M_SECTION, M_SECTION_POS } from '../m1/protocol';
import { Analysis } from '../m2/analysis';
import { idbKv, memoryKv, ProjectDb, flushOnHide, type Kv } from '../project/persist';
import { PPQ } from '../project/types';
import { $, App } from './app';
import { Capture } from './capture';
import { Gate } from './gate';
import { initLayers, initProjectBar, initSections } from './panels';
import { Session } from './session';
import { Timeline } from './timeline';

async function main(): Promise<void> {
  let kv: Kv;
  let storageNote = '';
  try {
    kv = await idbKv();
  } catch {
    kv = memoryKv();
    storageNote = 'IndexedDB is not available here, so nothing is saved after this tab closes.';
  }
  const db = new ProjectDb(kv);
  const session = new Session(kv, db);
  const app = new App(session);
  app.stopFlushOnHide = flushOnHide(db);
  if (storageNote) app.fail(storageNote);

  const timeline = new Timeline(app);
  const capture = new Capture(app);
  const gate = new Gate(app);
  const layers = initLayers(app);
  const sections = initSections(app);
  const projectBar = initProjectBar(app, () => { /* the session's change event does the rest */ });

  // ---- keep the panels in step with the document

  app.refreshAll = () => {
    const doc = app.doc;
    app.ui.fit(doc);
    projectBar.refresh(doc);
    sections.refresh(doc);
    layers.refresh(doc);
    timeline.refreshToolbar(doc);
    ($('undo') as HTMLButtonElement).disabled = !app.store.history.canUndo;
    ($('redo') as HTMLButtonElement).disabled = !app.store.history.canRedo;
    timeline.invalidate();
  };
  app.ui.subscribe(() => app.refreshAll());

  session.onChange((doc, kind) => {
    if (kind === 'swap') {
      app.ui.selection = { layerId: '', ids: [] };
      app.ui.pendingJump = -1;
      app.ui.sectionId = '';
      app.ui.layerId = '';
      if (app.host) {
        app.host.stop();
        void app.sync.request(doc).then(() => app.host?.seek(0, 0));
      }
    } else {
      app.lastRequest = app.sync.request(doc);
    }
    app.refreshAll();
  });
  app.sync.framesFor = (take) => session.framesFor(take);
  app.sync.onPass = (r) => {
    timeline.invalidate(); // a tuned curve may have arrived
    if (r.errors.length) app.fail(`Engine: ${r.errors.join('; ')}`);
    if (!r.skipped) $('engineStatus').textContent = `Engine in sync (pass ${r.pass}, compile ${r.compileMs.toFixed(1)} ms, ${r.uploads.length} blobs, ${r.loadMs.toFixed(1)} ms).`;
  };

  // ---- transport buttons

  const idx = () => app.doc.sections.findIndex((s) => s.id === app.ui.sectionId);
  $('play').addEventListener('click', async () => { if (!app.host) return; await app.host.ctx.resume(); app.host.play(); });
  $('stop').addEventListener('click', () => { app.host?.stop(); });
  $('seek').addEventListener('click', async () => {
    if (!app.host) return;
    await app.host.ctx.resume();
    app.host.seek(Math.max(0, idx()), 0);
    app.ui.pendingJump = -1;
  });
  $('undo').addEventListener('click', () => app.run(() => app.store.undo()));
  $('redo').addEventListener('click', () => app.run(() => app.store.redo()));
  window.addEventListener('keydown', (e) => {
    const t = e.target;
    if ((t instanceof HTMLInputElement && (t.type === 'text' || t.type === 'number')) || t instanceof HTMLTextAreaElement) return;
    const mod = e.metaKey || e.ctrlKey;
    if (!mod) return;
    const k = e.key.toLowerCase();
    if (k === 'z' && !e.shiftKey) { e.preventDefault(); app.run(() => app.store.undo()); }
    else if ((k === 'z' && e.shiftKey) || (k === 'y' && e.ctrlKey)) { e.preventDefault(); app.run(() => app.store.redo()); }
  });

  // ---- start

  let starting = false;
  async function start(withMic: boolean): Promise<void> {
    if (starting || app.host) return;
    starting = true;
    const buttons = ['start', 'startNoMic'].map((id) => $<HTMLButtonElement>(id));
    buttons.forEach((b) => { b.disabled = true; });
    const status = (s: string) => { $('engineStatus').textContent = s; };
    let ctx: AudioContext | null = null;
    try {
      ctx = new AudioContext({ latencyHint: 'interactive' });
      app.ctx = ctx;
      status('Loading the engine...');
      const host = await KlontHost.create(ctx, { input: true, forceFallback: wantsFallback() });
      status('Loading the analysis...');
      app.analysis = await Analysis.create();
      host.setBudget(0);
      app.host = host;
      app.sync.host = host;
      app.sync.analysis = app.analysis;
      app.sync.reset();
      await app.sync.request(app.doc);
      for (const id of ['play', 'stop', 'seek']) ($(id) as HTMLButtonElement).disabled = false;
      gate.enableStress();
      if (withMic) await capture.enableMic();
      else { $('enableMic').hidden = false; $('capStatus').textContent = 'No mic. Press Enable mic to capture.'; }
      status(`${host.mode === 'shared' ? 'Shared' : 'Fallback (MessagePort)'} transport, ${ctx.sampleRate} Hz, base latency ${((ctx.baseLatency || 0) * 1000).toFixed(1)} ms, output latency ${((ctx.outputLatency || 0) * 1000).toFixed(1)} ms.`);
    } catch (e) {
      const err = e as DOMException;
      const mic = err?.name === 'NotAllowedError' || err?.name === 'NotFoundError';
      app.fail(mic ? `No microphone: ${err.message || err.name}. Use "Start without mic" for playback and editing.` : e instanceof Error ? e.message : String(e));
      if (!app.host) { void ctx?.close(); app.ctx = null; app.stream?.getTracks().forEach((t) => t.stop()); app.stream = null; }
      status('Not started.');
      buttons.forEach((b) => { b.disabled = false; });
    } finally {
      starting = false;
    }
  }
  $('start').addEventListener('click', () => { void start(true); });
  $('startNoMic').addEventListener('click', () => { void start(false); });
  $('enableMic').addEventListener('click', async () => {
    try {
      await capture.enableMic();
      $('enableMic').hidden = true;
      app.clearError();
    } catch (e) {
      const err = e as DOMException;
      app.fail(`No microphone: ${err?.message || err?.name || String(e)}`);
    }
  });

  // ---- animation frame: transport readout, playhead, meters

  let lastMeters = 0;
  const pos = $('position');
  const tick = () => {
    requestAnimationFrame(tick);
    const m = app.readMeters();
    timeline.frame();
    if (!m) return;
    const playing = m[M_PLAYING] === 1;
    const si = Math.round(m[M_SECTION]);
    if (app.ui.pendingJump >= 0 && playing && si === app.ui.pendingJump) app.ui.pendingJump = -1;
    sections.markPlaying(playing ? si : -1);
    const sec = app.doc.sections[si];
    const p = m[M_SECTION_POS];
    const where = sec ? (sec.feel === 'grid' ? `beat ${(p / PPQ + 1).toFixed(2)}` : `${p.toFixed(2)} s`) : '';
    const jump = app.ui.pendingJump >= 0 ? ` Jump to ${app.doc.sections[app.ui.pendingJump]?.name ?? app.ui.pendingJump + 1} queued.` : '';
    pos.textContent = (playing ? `Playing ${sec?.name ?? si + 1}, ${where}${sec?.feel === 'grid' ? `, ${m[M_BPM].toFixed(0)} bpm` : ''}.` : 'Stopped.') + jump;
    const now = performance.now();
    if (now - lastMeters > 250) {
      lastMeters = now;
      $('meters').innerHTML = [
        ['CPU load', `${(m[M_LOAD] * 100).toFixed(0)}%`], ['voices', String(Math.round(m[M_ACTIVE_VOICES]))],
        ['overloads', String(Math.round(m[M_OVERLOADS]))], ['peak L/R', `${m[M_PEAK_L].toFixed(2)} / ${m[M_PEAK_R].toFixed(2)}`],
      ].map(([k, v]) => `<div><span class="muted">${k}</span> ${v}</div>`).join('');
    }
  };

  // ---- window.earworm

  Object.defineProperty(window, 'earworm', {
    configurable: true,
    value: {
      get store() { return session.store; },
      db,
      get host() { return app.host; },
      compiler: app.sync.compiler,
      gate,
      session,
      sync: app.sync,
      ui: app.ui,
    },
  });

  // ---- open the project: the one a pending check wants, else settings 'lastProject', else a new one

  await session.init(Gate.pendingProject());
  app.refreshAll();
  await gate.finishChecks();
  requestAnimationFrame(tick);
  if (session.blobError) app.fail(`Take storage is not available: ${session.blobError}`);
}

main().catch((e) => {
  const el = document.getElementById('error');
  if (el) { el.hidden = false; el.textContent = `The page could not start: ${e instanceof Error ? e.message : String(e)}`; }
});
