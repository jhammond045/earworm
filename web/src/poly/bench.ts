// The poly pass benchmark page (poly-bench.html, docs/P3.md check 5): a fresh analysis worker per
// take, the take fed a second at a time as the app feeds it, then the time and the worker's
// memory. `window.polyBench` holds the results for scripted runs.

import { Analysis } from '../m2/analysis';
import { guitar } from '../record/synth-guitar';

const SR = 48000;

interface Row { seconds: number; ms: number; realtime: number; notes: number; wasmMB: number }

const $ = (id: string) => document.getElementById(id)!;
const results: Row[] = [];
(window as unknown as { polyBench: { results: Row[]; done: boolean } }).polyBench = { results, done: false };

async function one(seconds: number): Promise<Row> {
  $('status').textContent = `Making ${seconds} s of guitar…`;
  const { audio } = guitar(seconds * SR, SR);
  const a = await Analysis.create();
  try {
    $('status').textContent = `Reading ${seconds} s…`;
    const t0 = performance.now();
    const r = await a.polyPass(audio, SR, { instrument: 'guitar', channels: 6 });
    const ms = performance.now() - t0;
    // The worker's memory is its wasm memory (which only grows, so this is the peak) plus the one
    // second of audio it holds at a time.
    const wasm = await a.memoryBytes();
    return { seconds, ms, realtime: (seconds * 1000) / ms, notes: r.notes.length, wasmMB: wasm / 2 ** 20 };
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
      p.textContent = `${s} s: ${(r.ms / 1000).toFixed(2)} s, ${r.realtime.toFixed(1)}x real time, ${r.notes} notes; the worker's wasm memory ${r.wasmMB.toFixed(1)} MB`;
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
