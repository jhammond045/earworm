// The M3 gate, "edit while playing, no loss": three checks on the page.
//
// - Edit stress test: a scratch project plays while 300 seeded random edits go in at 10 a second.
//   Each edit is timed from the command to the last host.load that carries it.
// - Reload check: flush, reload, reopen, compare the document.
// - Crash check: 20 edits 50 ms apart, then a reload that does not flush. The journal is written
//   within 300 ms, so at most the edits of the last 300 ms may be missing afterwards.

import { download } from '../common/wav';
import { M_LOAD, M_OVERLOADS, M_PLAYING, M_SECTION, M_SECTION_POS } from '../m1/protocol';
import { newProject } from '../project/commands';
import { median } from '../m2/take';
import { $, type App } from './app';
import { firstDifference, hashText, stableStringify } from './check';
import { actionLabel, applyAction, generateProject, nextAction, seeded } from './stress';

const EDITS = 300;
const EDIT_INTERVAL_MS = 100;
const STRESS_NAME = 'M3 gate test';
const CRASH_NAME = 'M3 crash test';
const CRASH_EDITS = 20;
const CRASH_GAP_MS = 50;
const RELOAD_KEY = 'earworm-m3-reload';
const CRASH_KEY = 'earworm-m3-crash';
const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

export interface EditRecord {
  i: number;
  action: string;
  applied: boolean;
  error?: string;
  compileMs: number;
  /** Compile start until the last host.load of the pass resolved. */
  loadMs: number;
  /** Command until that pass finished (what the editor waits for). */
  editToEngineMs: number;
  uploads: number;
  loadStatuses: string[];
  playing: number | null;
  cpuLoad: number | null;
  section: number | null;
  position: number | null;
}

export interface StressSummary {
  editsApplied: number;
  editsNoChange: number;
  commandErrors: number;
  loadErrors: number;
  maxEditToEngineMs: number;
  medianEditToEngineMs: number;
  maxCpuLoad: number;
  playbackStopped: boolean;
  overloadsDuringRun: number;
}

export interface StressResult { summary: StressSummary; records: EditRecord[]; seed: number; meta: Record<string, unknown> }

export class Gate {
  last: StressResult | null = null;
  private running = false;
  private cancel = false;

  constructor(private readonly app: App) {
    $('gStress').addEventListener('click', () => { void this.runStress(); });
    $('gStressStop').addEventListener('click', () => { this.cancel = true; });
    $('gStressDl').addEventListener('click', () => this.downloadJson());
    $('gReload').addEventListener('click', () => { void this.reloadCheck(); });
    $('gCrash').addEventListener('click', () => { void this.crashCheck(); });
  }

  /** The project a check wants reopened after a reload, if one is pending. */
  static pendingProject(): string | undefined {
    for (const key of [CRASH_KEY, RELOAD_KEY]) {
      try {
        const v = sessionStorage.getItem(key);
        if (v) return (JSON.parse(v) as { id: string }).id;
      } catch { /* ignore a bad entry */ }
    }
    return undefined;
  }

  enableStress(): void { ($('gStress') as HTMLButtonElement).disabled = false; }

  // ---------------------------------------------------------------- stress

  async runStress(seed = 1): Promise<StressResult | null> {
    const { app } = this;
    const host = app.host;
    if (!host) { app.fail('Press Start first: the stress test needs the engine.'); return null; }
    if (this.running) return null;
    this.running = true;
    this.cancel = false;
    const btn = $<HTMLButtonElement>('gStress');
    btn.disabled = true;
    ($('gStressStop') as HTMLButtonElement).disabled = false;
    ($('gStressDl') as HTMLButtonElement).disabled = true;
    const status = (s: string) => { $('gStressStatus').textContent = s; };
    try {
      status('Creating the scratch project...');
      const old = (await app.session.db.list()).filter((r) => r.name === STRESS_NAME).map((r) => r.id);
      await app.session.adopt(generateProject(seed, STRESS_NAME), false);
      for (const id of old) if (id !== app.doc.id) await app.session.remove(id);
      await host.ctx.resume();
      const first = await app.sync.request(app.doc);
      host.seek(0, 0);
      host.play();
      for (let i = 0; i < 100 && app.readMeters()?.[M_PLAYING] !== 1; i++) await sleep(20);
      const m0 = app.readMeters();
      const overloads0 = m0 ? m0[M_OVERLOADS] : 0;

      // Sample the transport between edits too, so a stop that no edit's reading caught is still seen.
      let stopped = false;
      let maxCpu = 0;
      let samples = 0;
      const sampler = window.setInterval(() => {
        const m = app.readMeters();
        if (!m) return;
        samples++;
        if (m[M_PLAYING] !== 1) stopped = true;
        maxCpu = Math.max(maxCpu, m[M_LOAD]);
      }, 40);

      const rnd = seeded(seed * 7919 + 13);
      const records: EditRecord[] = [];
      const pending: Promise<void>[] = [];
      const t0 = performance.now();
      for (let i = 0; i < EDITS && !this.cancel; i++) {
        const wait = t0 + i * EDIT_INTERVAL_MS - performance.now();
        if (wait > 0) await sleep(wait);
        const store = app.store;
        const action = nextAction(store.doc, rnd, store.history.canUndo, store.history.canRedo);
        const rec: EditRecord = {
          i, action: actionLabel(action), applied: false, compileMs: 0, loadMs: 0, editToEngineMs: 0, uploads: 0, loadStatuses: [],
          playing: null, cpuLoad: null, section: null, position: null,
        };
        records.push(rec);
        const t = performance.now();
        app.lastRequest = null;
        try {
          rec.applied = applyAction(store, action);
        } catch (e) {
          rec.error = e instanceof Error ? e.message : String(e);
        }
        if (rec.applied) {
          // The change listener asked for the pass that carries this edit; its result has the compile and load numbers.
          pending.push((app.lastRequest ?? app.sync.request(store.doc)).then((r) => {
            rec.compileMs = r.compileMs;
            rec.loadMs = r.loadMs;
            rec.editToEngineMs = performance.now() - t;
            rec.uploads = r.uploads.length;
            rec.loadStatuses = r.uploads.map((u) => u.name);
            if (r.errors.length) rec.error = r.errors.join('; ');
            const m = app.readMeters();
            if (m) { rec.playing = m[M_PLAYING]; rec.cpuLoad = m[M_LOAD]; rec.section = m[M_SECTION]; rec.position = m[M_SECTION_POS]; }
          }));
        }
        status(`Edit ${i + 1} of ${EDITS}: ${rec.action}`);
      }
      await Promise.all(pending);
      await sleep(300);
      window.clearInterval(sampler);
      const m1 = app.readMeters();
      host.stop();
      for (const r of records) if (r.playing !== null && r.playing !== 1) stopped = true;

      const applied = records.filter((r) => r.applied);
      const times = applied.map((r) => r.editToEngineMs);
      const summary: StressSummary = {
        editsApplied: applied.length,
        editsNoChange: records.filter((r) => !r.applied && !r.error).length,
        commandErrors: records.filter((r) => !r.applied && !!r.error).length,
        loadErrors: records.filter((r) => r.applied && r.loadStatuses.some((s) => s !== 'ok')).length + (first.errors.length ? 1 : 0),
        maxEditToEngineMs: times.length ? Math.max(...times) : 0,
        medianEditToEngineMs: times.length ? median(times) : 0,
        maxCpuLoad: Math.max(maxCpu, ...records.map((r) => r.cpuLoad ?? 0)),
        playbackStopped: stopped,
        overloadsDuringRun: (m1 ? m1[M_OVERLOADS] : 0) - overloads0,
      };
      const ctx = host.ctx;
      const result: StressResult = {
        summary, records, seed,
        meta: {
          userAgent: navigator.userAgent, date: new Date().toISOString(), transport: host.mode, sampleRate: ctx.sampleRate, contextState: ctx.state,
          baseLatencyMs: (ctx.baseLatency || 0) * 1000, meterSamples: samples, editsRequested: EDITS, intervalMs: EDIT_INTERVAL_MS, cancelled: this.cancel,
          project: STRESS_NAME,
        },
      };
      this.last = result;
      this.show(result);
      status(this.cancel ? 'Cancelled.' : 'Done. The scratch project is open; pick your own from Saved projects.');
      ($('gStressDl') as HTMLButtonElement).disabled = false;
      return result;
    } catch (e) {
      app.fail(e instanceof Error ? e.message : String(e));
      status('Failed.');
      return null;
    } finally {
      this.running = false;
      btn.disabled = false;
      ($('gStressStop') as HTMLButtonElement).disabled = true;
    }
  }

  private show(r: StressResult): void {
    const s = r.summary;
    const out = $('gStressOut');
    out.hidden = false;
    const ok = s.loadErrors === 0 && s.commandErrors === 0 && !s.playbackStopped;
    out.replaceChildren();
    const verdict = document.createElement('b');
    verdict.className = ok ? 'pass' : 'failed';
    verdict.textContent = ok ? 'NO LOSS' : 'PROBLEM';
    out.append(verdict, '\n',
      `edits applied: ${s.editsApplied} of ${r.records.length} (${s.editsNoChange} changed nothing, ${s.commandErrors} rejected)\n` +
      `load errors: ${s.loadErrors}\n` +
      `edit to engine: max ${s.maxEditToEngineMs.toFixed(1)} ms, median ${s.medianEditToEngineMs.toFixed(1)} ms\n` +
      `max CPU load: ${(s.maxCpuLoad * 100).toFixed(0)}% of a render block\n` +
      `playback ever stopped: ${s.playbackStopped ? 'YES' : 'no'}\n` +
      `engine overloads during the run: ${s.overloadsDuringRun}`);
  }

  private downloadJson(): void {
    if (!this.last) return;
    download(new Blob([JSON.stringify(this.last, null, 2)], { type: 'application/json' }), 'earworm-m3-stress.json');
  }

  // ---------------------------------------------------------------- reload check

  async reloadCheck(): Promise<void> {
    const { app } = this;
    $('gReloadOut').textContent = 'Saving and reloading...';
    const doc = app.doc;
    await app.session.db.flush();
    const text = stableStringify(doc);
    const journal = (await app.session.kv.keys('journal', `${doc.id}/`)).length;
    const h = app.store.history.toJSON();
    sessionStorage.setItem(RELOAD_KEY, JSON.stringify({
      id: doc.id, name: doc.name, hash: await hashText(text), json: text, journal, undo: h.undo.length, redo: h.redo.length,
    }));
    location.reload();
  }

  // ---------------------------------------------------------------- crash check

  async crashCheck(): Promise<void> {
    const { app } = this;
    const out = $('gCrashOut');
    for (const b of ['gCrash', 'gReload', 'gStress']) ($(b) as HTMLButtonElement).disabled = true;
    try {
      out.textContent = 'Making 20 edits 50 ms apart...';
      const old = (await app.session.db.list()).filter((r) => r.name === CRASH_NAME).map((r) => r.id);
      await app.session.adopt(newProject(CRASH_NAME), false);
      for (const id of old) if (id !== app.doc.id) await app.session.remove(id);
      const id = app.doc.id;
      const layer = app.doc.layers[0].id;
      const times: number[] = [];
      for (let k = 1; k <= CRASH_EDITS; k++) {
        if (k > 1) await sleep(CRASH_GAP_MS);
        // Edit k sets the level to -k dB, so the level after a reload says how many edits survived.
        app.store.dispatch('setLayer', { id: layer, level: -k });
        times.push(Date.now());
      }
      // No flush, and no save-on-hide either: this is the page going away without a chance to write.
      app.stopFlushOnHide();
      sessionStorage.setItem(CRASH_KEY, JSON.stringify({ id, times, crashAt: Date.now() }));
      location.reload();
    } catch (e) {
      app.fail(e instanceof Error ? e.message : String(e));
      for (const b of ['gCrash', 'gReload']) ($(b) as HTMLButtonElement).disabled = false;
    }
  }

  // ---------------------------------------------------------------- after a reload

  /** Read what the checks saved before the reload and show the result. Call once the project is open. */
  async finishChecks(): Promise<void> {
    const { app } = this;
    const reload = sessionStorage.getItem(RELOAD_KEY);
    if (reload) {
      sessionStorage.removeItem(RELOAD_KEY);
      const out = $('gReloadOut');
      try {
        const r = JSON.parse(reload) as { id: string; name: string; hash: string; json: string; journal: number; undo: number; redo: number };
        const doc = app.doc;
        const text = stableStringify(doc);
        const hash = await hashText(text);
        const journal = (await app.session.kv.keys('journal', `${doc.id}/`)).length;
        const h = app.store.history.toJSON();
        const diff = doc.id === r.id ? firstDifference(JSON.parse(r.json), JSON.parse(text)) : `opened a different project (${doc.id} instead of ${r.id})`;
        const same = hash === r.hash && !diff;
        const info = `document hash ${hash.slice(0, 12)} (was ${r.hash.slice(0, 12)}); journal entries ${journal} (was ${r.journal}); ` +
          `undo ${h.undo.length}/redo ${h.redo.length} (was ${r.undo}/${r.redo})`;
        out.replaceChildren();
        const v = document.createElement('b');
        v.className = same ? 'pass' : 'failed';
        v.textContent = same ? 'PASS' : 'FAIL';
        out.append(`Reload check "${r.name}": `, v, same ? '. ' : `. First difference: ${diff ?? 'hash differs'}. `, info);
        this.lastReload = { pass: same, diff, info };
      } catch (e) {
        out.textContent = `Reload check could not be read: ${e instanceof Error ? e.message : String(e)}`;
      }
    }
    const crash = sessionStorage.getItem(CRASH_KEY);
    if (crash) {
      sessionStorage.removeItem(CRASH_KEY);
      const out = $('gCrashOut');
      try {
        const r = JSON.parse(crash) as { id: string; times: number[]; crashAt: number };
        const level = app.doc.layers[0]?.level ?? 0;
        const kept = Math.max(0, Math.min(CRASH_EDITS, Math.round(-level)));
        const lost = CRASH_EDITS - kept;
        // The oldest lost edit is the one the 300 ms rule is about: everything after it is lost too.
        const oldestAge = lost ? r.crashAt - r.times[kept] : 0;
        const newestAge = lost ? r.crashAt - r.times[CRASH_EDITS - 1] : 0;
        const pass = lost === 0 || oldestAge <= 300;
        out.replaceChildren();
        const v = document.createElement('b');
        v.className = pass ? 'pass' : 'failed';
        v.textContent = pass ? 'PASS' : 'FAIL';
        out.append('Crash check: ', v,
          `. ${kept} of ${CRASH_EDITS} edits survived, ${lost} lost. ` +
          (lost ? `Oldest lost edit was ${oldestAge} ms before the reload, newest ${newestAge} ms (allowed: 300 ms).` : 'Nothing was lost.'));
        this.lastCrash = { pass, kept, lost, oldestAgeMs: oldestAge, newestAgeMs: newestAge };
      } catch (e) {
        out.textContent = `Crash check could not be read: ${e instanceof Error ? e.message : String(e)}`;
      }
    }
  }

  lastReload: { pass: boolean; diff: string | null; info: string } | null = null;
  lastCrash: { pass: boolean; kept: number; lost: number; oldestAgeMs: number; newestAgeMs: number } | null = null;
}
