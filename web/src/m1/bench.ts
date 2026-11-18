// Benchmark page for the M1 gate ("8 layers playing": no dropouts, under 50%
// of one core, offline render above 4x realtime). Offline tests run in the
// render worker; the live test plays the demo in the worklet.

import { download } from '../common/wav';
import { encodeDemo } from './demo';
import { KlontHost } from './klont-host';
import { OfflineRenderer, type BlockTimingResult, type RenderResult, type WorstResult } from './offline';
import { LOAD_LAYER, LOAD_OK, LOAD_TRACK, METER_LEN, M_ACTIVE_PARTIALS, M_BUDGET, M_LOAD, M_OVERLOADS, M_STEALS, MAX_LAYERS } from './protocol';

const BUDGETS = [4096, 3072, 2048, 1536, 1024, 768, 512];
const TARGET = 4; // x realtime
const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

interface DemoRun { budget: number | 'default'; seconds: number; realtime: number; peak: number; partialsMean: number }
interface LiveSample { t: number; load: number; budget: number; partials: number; overloads: number; steals: number; playout?: unknown }
interface Results {
  env: Record<string, unknown>;
  worst?: (WorstResult & { coreLoadPct: number; meetsTarget: boolean })[];
  recommendedBudget?: number | null;
  demo?: DemoRun[];
  blocks?: BlockTimingResult;
  live?: { samples: LiveSample[]; transport: string; perfNow: boolean; playoutStatsAvailable: boolean };
}

const results: Results = {
  env: {
    userAgent: navigator.userAgent,
    hardwareConcurrency: navigator.hardwareConcurrency,
    crossOriginIsolated: globalThis.crossOriginIsolated,
    date: new Date().toISOString(),
  },
};
let renderer: OfflineRenderer | null = null;
const getRenderer = () => (renderer ??= new OfflineRenderer());

$('env').textContent = Object.entries(results.env).map(([k, v]) => `${k}: ${v}`).join('\n');
$('env').style.whiteSpace = 'pre-wrap';

const status = (s: string) => { $('status').textContent = s; };
function table(head: string[], rows: string[][]): string {
  const th = head.map((h, i) => `<th${i ? ' class="num"' : ''}>${h}</th>`).join('');
  const body = rows.map((r) => `<tr>${r.map((c, i) => `<td${i ? ' class="num"' : ''}>${c}</td>`).join('')}</tr>`).join('');
  return `<table class="mono small"><thead><tr>${th}</tr></thead><tbody>${body}</tbody></table>`;
}

async function guarded(fn: () => Promise<void>): Promise<void> {
  const btns = document.querySelectorAll<HTMLButtonElement>('.bar button');
  btns.forEach((b) => { b.disabled = true; });
  $('error').hidden = true;
  try {
    await fn();
    status('Done.');
  } catch (e) {
    $('error').hidden = false;
    $('error').textContent = e instanceof Error ? e.message : String(e);
    status('Failed.');
  } finally {
    btns.forEach((b) => { b.disabled = false; });
    renderGate();
  }
}

function recommended(): number | null {
  const ok = (results.worst ?? []).filter((r) => r.meetsTarget).map((r) => r.budget);
  return ok.length ? Math.max(...ok) : null;
}

// ---------------------------------------------------------------- 1
$('b1').addEventListener('click', () => guarded(async () => {
  const rows: NonNullable<Results['worst']> = [];
  for (const budget of BUDGETS) {
    status(`Worst case at budget ${budget}...`);
    const r = await getRenderer().worstCase(budget, 10, (p) => status(`Worst case at budget ${budget}: ${p.seconds.toFixed(0)} of 10 s`)).promise;
    rows.push({ ...r, coreLoadPct: 100 / r.realtime, meetsTarget: r.realtime > TARGET });
    results.worst = rows;
    results.recommendedBudget = recommended();
    renderWorst();
  }
}));

function renderWorst(): void {
  const w = results.worst ?? [];
  $('r1').innerHTML = table(['budget', 'mean partials', 'x realtime', 'ns / partial-sample', 'core load', `> ${TARGET}x`],
    w.map((r) => [String(r.budget), r.partialsMean.toFixed(0), r.realtime.toFixed(2), r.nsPerPartialSample.toFixed(3),
      `${r.coreLoadPct.toFixed(1)}%`, r.meetsTarget ? '<span class="pass">yes</span>' : '<span class="fail">no</span>']))
    + `<p>Recommended partial budget: <strong>${results.recommendedBudget ?? 'none (even 512 is below target)'}</strong></p>`;
}

// ---------------------------------------------------------------- 2
$('b2').addEventListener('click', () => guarded(async () => {
  const rec = recommended();
  const budgets = [4096, ...(rec && rec !== 4096 ? [rec] : [])];
  const runs: DemoRun[] = [];
  for (const budget of budgets) {
    status(`Demo song at budget ${budget}...`);
    const r: RenderResult = await getRenderer().render(encodeDemo(), { budget, maxSeconds: 60, once: false },
      (p) => status(`Demo song at budget ${budget}: ${p.seconds.toFixed(0)} of 60 s`)).promise;
    runs.push({ budget, seconds: r.frames / r.sampleRate, realtime: r.realtime, peak: r.peak, partialsMean: r.partialsMean });
    results.demo = runs;
    renderDemo();
  }
}));

function renderDemo(): void {
  $('r2').innerHTML = table(['budget', 'audio (s)', 'x realtime', 'core load', 'peak', 'mean partials'],
    (results.demo ?? []).map((r) => [String(r.budget), r.seconds.toFixed(0), r.realtime.toFixed(2), `${(100 / r.realtime).toFixed(1)}%`,
      r.peak.toFixed(2), r.partialsMean.toFixed(0)]))
    + (recommended() === null ? '<p class="small muted">Run test 1 first to get a recommended budget.</p>' : '');
}

// ---------------------------------------------------------------- 3
$('b3').addEventListener('click', () => guarded(async () => {
  const budget = recommended() ?? 512;
  status(`Block timing at budget ${budget}...`);
  const r = await getRenderer().blockTiming(budget, 20, (p) => status(`Block timing at budget ${budget}: ${p.seconds.toFixed(0)} of 20 s`)).promise;
  results.blocks = r;
  $('r3').innerHTML = table(['budget', 'blocks', 'mean ms', 'p50 ms', 'p99 ms', 'max ms', 'over quantum'],
    [[String(r.budget), String(r.blocks), r.mean.toFixed(3), r.p50.toFixed(3), r.p99.toFixed(3), r.max.toFixed(3), String(r.overQuantum)]])
    + `<p class="small muted">The quantum is ${r.quantumMs.toFixed(2)} ms. A block over it is a dropout in live playback if nothing buffers it. `
    + 'The browser clock is coarse (5 to 100 us), so tiny times are rounded.</p>';
}));

// ---------------------------------------------------------------- 4
$('b4').addEventListener('click', () => guarded(async () => {
  const ctx = new AudioContext({ latencyHint: 'interactive' });
  let host: KlontHost | null = null;
  try {
    host = await KlontHost.create(ctx, { forceFallback: new URLSearchParams(location.search).get('fallback') === '1' });
    const song = encodeDemo();
    for (let l = 0; l < MAX_LAYERS; l++) {
      if (await host.load(LOAD_LAYER, l, song.layers[l]) !== LOAD_OK) throw new Error(`layer ${l} failed to load`);
    }
    if (await host.load(LOAD_TRACK, 0, song.track) !== LOAD_OK) throw new Error('track failed to load');
    host.setBudget(0);
    await ctx.resume();
    host.play();
    const samples: LiveSample[] = [];
    const m = new Float32Array(METER_LEN);
    const playout = (ctx as unknown as { playoutStats?: { toJSON?: () => unknown } }).playoutStats;
    const t0 = performance.now();
    for (let s = 1; s <= 60; s++) {
      await new Promise((r) => setTimeout(r, 1000 - ((performance.now() - t0) % 1000)));
      if (!host.readMeters(m)) continue;
      samples.push({ t: s, load: m[M_LOAD], budget: m[M_BUDGET], partials: m[M_ACTIVE_PARTIALS], overloads: m[M_OVERLOADS],
        steals: m[M_STEALS], playout: playout?.toJSON?.() });
      status(`Live: ${s} of 60 s, load ${(m[M_LOAD] * 100).toFixed(0)}%, budget ${m[M_BUDGET]}`);
      results.live = { samples, transport: host.mode, perfNow: host.perfNow, playoutStatsAvailable: playout !== undefined };
      renderLive();
    }
    host.stop();
  } finally {
    host?.disconnect();
    await ctx.close();
  }
}));

function renderLive(): void {
  const l = results.live;
  if (!l || !l.samples.length) return;
  const s = l.samples;
  const last = s[s.length - 1];
  const maxLoad = Math.max(...s.map((x) => x.load));
  const minBudget = Math.min(...s.map((x) => x.budget));
  const maxParts = Math.max(...s.map((x) => x.partials));
  const rows = s.filter((_, i) => i % 5 === 4 || i === s.length - 1).map((x) => [`${x.t} s`, `${(x.load * 100).toFixed(0)}%`, String(x.budget),
    String(x.partials), String(x.overloads), String(x.steals)]);
  const po = last.playout as { fallbackFramesEvents?: number; fallbackFramesDuration?: number } | undefined;
  $('r4').innerHTML = `<p>Transport: ${l.transport}. Worklet CPU timer: ${l.perfNow ? 'performance.now()' : 'Date.now() window average (coarse)'}. `
    + `Max load ${(maxLoad * 100).toFixed(0)}%, lowest budget ${minBudget}, most partials ${maxParts}, overloads ${last.overloads}, steals ${last.steals}.</p>`
    + table(['time', 'host CPU load', 'budget', 'active partials', 'overloads', 'steals'], rows)
    + (l.playoutStatsAvailable
      ? `<p>Playout stats: ${po?.fallbackFramesEvents ?? '?'} underrun events, ${((po?.fallbackFramesDuration ?? 0) * 1000).toFixed(0)} ms of silence inserted.</p>`
      : '<p class="small muted">This Chrome does not expose AudioContext.playoutStats, so dropouts must be judged by ear.</p>');
}

// ---------------------------------------------------------------- gate
function renderGate(): void {
  const rec = recommended();
  const w4096 = results.worst?.find((r) => r.budget === 4096);
  const demoRec = results.demo?.find((r) => r.budget === (rec ?? 4096)) ?? results.demo?.[results.demo.length - 1];
  const lines: string[] = [];
  lines.push(`Recommended partial budget: ${results.worst ? (rec ?? 'none') : 'run test 1'}`);
  if (w4096) lines.push(`Worst case at 4096: ${w4096.realtime.toFixed(2)}x realtime (${w4096.coreLoadPct.toFixed(0)}% of a core)`);
  if (demoRec) lines.push(`Demo song at budget ${demoRec.budget}: ${demoRec.realtime.toFixed(2)}x realtime`);
  if (results.blocks) lines.push(`Block timing at ${results.blocks.budget}: p99 ${results.blocks.p99.toFixed(2)} ms, max ${results.blocks.max.toFixed(2)} ms, ${results.blocks.overQuantum} blocks over ${results.blocks.quantumMs.toFixed(2)} ms`);
  let verdict = '';
  if (results.worst && results.demo) {
    const pass = rec !== null && !!demoRec && demoRec.realtime > TARGET;
    verdict = pass
      ? `<span class="pass">GATE PASS (offline)</span>: the worst case runs above ${TARGET}x realtime at budget ${rec}, and so does the demo song.`
      : `<span class="fail">GATE FAIL (offline)</span>: ${rec === null ? 'no tested budget reaches' : 'the demo song does not reach'} ${TARGET}x realtime.`;
  } else {
    verdict = 'Run tests 1 and 2 for the pass or fail line.';
  }
  const live = results.live?.samples.length ? ' Dropouts in the live test cannot be proven from here unless playout stats are shown above; listen for clicks.' : '';
  $('gate').innerHTML = `${lines.map((x) => `<div class="mono small">${x}</div>`).join('')}<p>${verdict}${live}</p>`;
}

$('dl').addEventListener('click', () => {
  download(new Blob([JSON.stringify(results, null, 2)], { type: 'application/json' }), `earworm-m1-bench-${Date.now()}.json`);
});
renderGate();
