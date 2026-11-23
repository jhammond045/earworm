// The poly pass benchmark page (poly-bench.html, docs/P3.md check 5): a fresh analysis worker per
// take, the take fed a second at a time as the app feeds it, then the time and the worker's
// memory. `window.polyBench` holds the results for scripted runs.

import { Analysis } from '../m2/analysis';
import { guitar } from '../record/synth-guitar';

const SR = 48000;

interface Row { seconds: number; ms: number; realtime: number; notes: number; wasmMB: number; workerMB: number | null }

const $ = (id: string) => document.getElementById(id)!;
const results: Row[] = [];
(window as unknown as { polyBench: { results: Row[]; done: boolean } }).polyBench = { results, done: false };

/** The worker's memory from the browser, when the page is cross-origin isolated (Chrome). */
async function workerMemory(): Promise<number | null> {
  const p = performance as Performance & { measureUserAgentSpecificMemory?: () => Promise<{ breakdown: { bytes: number; types: string[]; attribution: { scope?: string }[] }[] }> };
  if (!p.measureUserAgentSpecificMemory || !crossOriginIsolated) return null;
  const m = await p.measureUserAgentSpecificMemory();
  return m.breakdown.filter((b) => b.attribution.some((a) => a.scope === 'DedicatedWorkerGlobalScope')).reduce((s, b) => s + b.bytes, 0);
}

async function one(seconds: number): Promise<Row> {
  $('status').textContent = `Making ${seconds} s of guitar…`;
  const { audio } = guitar(seconds * SR, SR);
  const a = await Analysis.create();
  try {
    $('status').textContent = `Reading ${seconds} s…`;
    const t0 = performance.now();
    const r = await a.polyPass(audio, SR, { instrument: 'guitar', channels: 6 });
    const ms = performance.now() - t0;
    const wasm = await a.memoryBytes();
    const worker = await workerMemory();
    return { seconds, ms, realtime: (seconds * 1000) / ms, notes: r.notes.length, wasmMB: wasm / 2 ** 20, workerMB: worker === null ? null : worker / 2 ** 20 };
  } finally {
    a.dispose();
  }
}

$('run').addEventListener('click', async () => {
  ($('run') as HTMLButtonElement).disabled = true;
  try {
    for (const s of [60, 300]) {
      const r = await one(s);
      results.push(r);
      const p = document.createElement('p');
      p.className = 'mono';
      p.textContent = `${s} s: ${(r.ms / 1000).toFixed(2)} s, ${r.realtime.toFixed(1)}x real time, ${r.notes} notes; wasm memory ${r.wasmMB.toFixed(1)} MB${r.workerMB === null ? '' : `, worker ${r.workerMB.toFixed(1)} MB`}`;
      $('out').append(p);
    }
    $('status').textContent = 'Done.';
  } catch (e) {
    $('status').textContent = `Failed: ${e instanceof Error ? e.message : String(e)}`;
  } finally {
    (window as unknown as { polyBench: { done: boolean } }).polyBench.done = true;
    ($('run') as HTMLButtonElement).disabled = false;
  }
});
