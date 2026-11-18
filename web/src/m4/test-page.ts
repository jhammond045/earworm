// The M4 gate page: blind four-way test of the Influence match (match_full + grain source
// + Klont render) on saved sources. Sources live in the same IndexedDB store as the M0
// pages. Trial and scoring logic is in test-trials.ts; the render job is in test-render.ts.

import { createAudioContext } from '../host/engine-host';
import { decodeFile, downloadBlob, loadSources, recordMic, saveSources, type SavedSource } from '../influence/audio';
import { OfflineRenderer } from '../m1/offline';
import { Matcher, type Match } from './match';
import { RELEASE_S, RENDER_RATE, planMatchRender, toMono } from './test-render';
import {
  GATE_PCT, GATE_SOURCES, MIN_SOURCES, SUGGESTED_KINDS, buildReport, makeTrials, outcomeFor, score, useSoundFor,
  type Outcome, type SoundMode, type Trial,
} from './test-trials';

interface Rendered {
  match: Match;
  audio: Float32Array;
  sampleRate: number;
  matchMs: number;
  renderMs: number;
  grain: boolean;
}

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const mode = () => $<HTMLSelectElement>('mode').value as SoundMode;

let sources: SavedSource[] = [];
let ctx: AudioContext | null = null;
let matcher: Matcher | null = null;
let renderer: OfflineRenderer | null = null;
let busy = false;
const cache = new Map<string, Rendered>();
let playing: AudioBufferSourceNode[] = [];

// Cache key: the sound itself plus the settings that change the render.
const keyOf = (s: SavedSource, m: SoundMode) => `${s.name}|${s.sampleRate}|${s.signal.length}|${m}`;

function el<K extends keyof HTMLElementTagNameMap>(tag: K, text?: string, props: Partial<HTMLElementTagNameMap[K]> = {}): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (text !== undefined) e.textContent = text;
  return Object.assign(e, props);
}

function button(text: string, onclick: () => void): HTMLButtonElement {
  return el('button', text, { onclick: () => onclick() });
}

function setStatus(msg: string, err = false, id = 'status') {
  const e = $(id);
  e.textContent = msg;
  e.className = err ? 'err' : '';
}

const errText = (e: unknown) => (e instanceof Error ? e.message : String(e));

// ---------------------------------------------------------------- audio

function audioCtx(): AudioContext {
  ctx ??= createAudioContext();
  if (ctx.state === 'suspended') void ctx.resume();
  return ctx;
}

function stopAll() {
  for (const n of playing) { n.onended = null; try { n.stop(); } catch { /* already done */ } }
  playing = [];
}

/** Play clips one after another with `gapS` of silence between them. */
function playSeq(clips: { audio: Float32Array; sampleRate: number }[], gapS = 0.5) {
  const c = audioCtx();
  stopAll();
  let at = c.currentTime + 0.05;
  for (const clip of clips) {
    const buf = c.createBuffer(1, clip.audio.length, clip.sampleRate);
    buf.copyToChannel(clip.audio as Float32Array<ArrayBuffer>, 0);
    const n = c.createBufferSource();
    n.buffer = buf;
    n.connect(c.destination);
    n.start(at);
    playing.push(n);
    at += buf.duration + gapS;
  }
}

const asClip = (s: SavedSource) => ({ audio: s.signal, sampleRate: s.sampleRate });
const matchClip = (r: Rendered) => ({ audio: r.audio, sampleRate: r.sampleRate });

// ---------------------------------------------------------------- sources

async function persist() {
  try {
    await saveSources(sources);
  } catch (e) {
    setStatus(`Could not save sources (${errText(e)}). They stay for this visit only.`, true, 'srcmsg');
  }
}

async function reload() {
  try {
    sources = await loadSources();
  } catch (e) {
    setStatus(`Saved sources are unavailable (${errText(e)}).`, true, 'srcmsg');
  }
  drawSources();
  drawListen();
}

function drawSources() {
  const n = sources.length;
  $('count').textContent = `${n} of the ${GATE_SOURCES} sources the gate wants${n < MIN_SOURCES ? `; at least ${MIN_SOURCES} are needed for a test` : n < GATE_SOURCES ? ' (a run with fewer is practice, not the gate)' : ''}.`;
  $('kinds').textContent = `Varied kinds to cover: ${SUGGESTED_KINDS.join(', ')}.`;
  const t = $<HTMLTableElement>('srclist');
  t.replaceChildren();
  sources.forEach((s, i) => {
    const r = t.insertRow();
    r.insertCell().textContent = String(i + 1);
    r.insertCell().textContent = s.name;
    r.insertCell().textContent = `${(s.signal.length / s.sampleRate).toFixed(2)} s`;
    r.insertCell().append(button('Play source', () => playSeq([asClip(s)])));
    r.insertCell().append(button('Remove', () => void remove(i)));
  });
}

async function remove(i: number) {
  sources.splice(i, 1);
  await persist();
  drawSources();
  drawListen();
}

async function addSignal(name: string, signal: Float32Array, sampleRate: number) {
  sources.push({ name, signal, sampleRate });
  await persist();
}

async function addFiles(files: FileList | File[]) {
  const c = audioCtx();
  for (const f of Array.from(files)) {
    try {
      setStatus(`Decoding ${f.name}`, false, 'srcmsg');
      await addSignal(f.name, await decodeFile(c, f), c.sampleRate);
      setStatus(`Added ${f.name}.`, false, 'srcmsg');
    } catch (e) {
      setStatus(`${f.name}: ${errText(e)}`, true, 'srcmsg');
    }
    drawSources();
    drawListen();
  }
}

async function record() {
  const b = $<HTMLButtonElement>('rec');
  b.disabled = true;
  try {
    const c = audioCtx();
    setStatus('Recording 3 s...', false, 'srcmsg');
    const sig = await recordMic(c, 3);
    await addSignal(`mic ${new Date().toLocaleTimeString()}`, sig, c.sampleRate);
    setStatus('Recorded.', false, 'srcmsg');
    drawSources();
    drawListen();
  } catch (e) {
    setStatus(`Recording failed: ${errText(e)}`, true, 'srcmsg');
  } finally {
    b.disabled = false;
  }
}

// ---------------------------------------------------------------- match and render

async function renderOne(s: SavedSource, m: SoundMode): Promise<Rendered> {
  const key = keyOf(s, m);
  const hit = cache.get(key);
  if (hit) return hit;
  matcher ??= await Matcher.create();
  renderer ??= new OfflineRenderer();
  const t0 = performance.now();
  const match = await matcher.match(s.signal, s.sampleRate, { useSound: useSoundFor(m) });
  const matchMs = match.ms || performance.now() - t0;
  const plan = planMatchRender(s, match);
  const t1 = performance.now();
  const r = await renderer.render(plan.song, plan.options).promise;
  const renderMs = performance.now() - t1;
  const out: Rendered = { match, audio: toMono(r.left, r.right), sampleRate: r.sampleRate, matchMs, renderMs, grain: plan.grain };
  cache.set(key, out);
  return out;
}

/** Match and render every source that is not cached for the current setting. */
async function renderAll(): Promise<boolean> {
  if (busy) return false;
  busy = true;
  const m = mode();
  $<HTMLButtonElement>('matchall').disabled = true;
  try {
    for (let i = 0; i < sources.length; i++) {
      if (cache.has(keyOf(sources[i], m))) continue;
      setStatus(`Matching and rendering ${i + 1} of ${sources.length}: ${sources[i].name}`);
      await renderOne(sources[i], m);
      drawListen();
    }
    setStatus(`Done: ${sources.length} matches rendered (use the sound: ${m}).`);
    return true;
  } catch (e) {
    setStatus(`Failed: ${errText(e)}`, true);
    return false;
  } finally {
    busy = false;
    $<HTMLButtonElement>('matchall').disabled = false;
    drawListen();
  }
}

function drawListen() {
  const t = $<HTMLTableElement>('listen');
  t.replaceChildren();
  if (!sources.length) return;
  const h = t.insertRow();
  for (const c of ['Source', 'Pitch', 'Sound used', 'Match', 'Render', '', '', '']) h.append(el('th', c));
  const m = mode();
  for (const s of sources) {
    const r = t.insertRow();
    r.insertCell().textContent = s.name;
    const d = cache.get(keyOf(s, m));
    if (!d) {
      const c = r.insertCell();
      c.colSpan = 4;
      c.textContent = 'not matched yet';
      c.className = 'muted';
      r.insertCell().append(button('Play source', () => playSeq([asClip(s)])));
      continue;
    }
    r.insertCell().textContent = d.match.pitched ? `${(d.match.pitchCents / 100 + 12).toFixed(1)} (MIDI)` : 'unpitched (C4)';
    r.insertCell().textContent = d.grain ? 'yes' : 'no';
    r.insertCell().textContent = `${d.matchMs.toFixed(0)} ms`;
    r.insertCell().textContent = `${d.renderMs.toFixed(0)} ms`;
    r.insertCell().append(button('Play source', () => playSeq([asClip(s)])));
    r.insertCell().append(button('Play match', () => playSeq([matchClip(d)])));
    r.insertCell().append(button('Play A then B', () => playSeq([asClip(s), matchClip(d)])));
  }
}

// ---------------------------------------------------------------- blind test

async function start() {
  const seedText = $<HTMLInputElement>('seed').value.trim();
  const seed = seedText && Number.isFinite(Number(seedText)) ? Number(seedText) >>> 0 : Math.floor(Math.random() * 1e9);
  $<HTMLInputElement>('seed').value = String(seed);
  $('trial').replaceChildren();
  $('result').replaceChildren();
  if (sources.length < MIN_SOURCES) {
    setStatus(`Need at least ${MIN_SOURCES} sources, have ${sources.length}.`, true);
    return;
  }
  if (busy) return;
  $<HTMLButtonElement>('start').disabled = true;
  try {
    if (!(await renderAll())) return;
    const m = mode();
    const list = sources.slice();
    const rendered = list.map((s) => cache.get(keyOf(s, m))!);
    const trials = makeTrials(list.length, seed);
    setStatus(`Seed ${seed}, ${trials.length} trials, use the sound: ${m}.`);
    runTrials(list, rendered, trials, seed, m);
  } finally {
    $<HTMLButtonElement>('start').disabled = false;
  }
}

function runTrials(list: SavedSource[], rendered: Rendered[], trials: Trial[], seed: number, m: SoundMode) {
  const outcomes: Outcome[] = [];
  const box = $('trial');
  const show = (k: number) => {
    stopAll();
    box.replaceChildren();
    if (k >= trials.length) { finish(list, rendered, trials, outcomes, seed, m); return; }
    const t = trials[k];
    box.append(el('h3', `Trial ${k + 1} of ${trials.length}`), button('Play match', () => playSeq([matchClip(rendered[t.target])])));
    t.candidates.forEach((idx, pos) => {
      const label = 'ABCD'[pos];
      const row = el('div', undefined, { className: 'row cand' });
      row.append(
        button(`Play ${label}`, () => playSeq([asClip(list[idx])])),
        button(`It was ${label}`, () => {
          outcomes.push(outcomeFor(list, t, pos));
          show(k + 1);
        }),
      );
      box.append(row);
    });
    box.append(el('p', `Score so far: ${outcomes.filter((o) => o.correct).length} of ${outcomes.length}`, { id: 'running', className: 'muted' }));
  };
  show(0);
}

function finish(list: SavedSource[], rendered: Rendered[], trials: Trial[], outcomes: Outcome[], seed: number, m: SoundMode) {
  const s = score(outcomes, list.length);
  const el0 = $('result');
  el0.replaceChildren();
  const head = el('h3', `Score: ${s.right} of ${s.total} (${s.percent.toFixed(0)}%), gate ${GATE_PCT}%`, { className: s.atGate ? 'ok' : 'bad', id: 'score' });
  const notes: string[] = [];
  if (s.short) notes.push(`Only ${list.length} sources; the gate wants ${GATE_SOURCES}, so this is practice.`);
  if (m !== 'default') notes.push(`Use the sound is "${m}" for all sources; the gate run uses the default rule.`);
  const tbl = el('table');
  const hr = tbl.insertRow();
  for (const c of ['Source', 'You chose', 'Result']) hr.append(el('th', c));
  for (const o of outcomes) {
    const r = tbl.insertRow();
    r.insertCell().textContent = o.source;
    r.insertCell().textContent = o.correct ? '-' : o.chosen;
    const c = r.insertCell();
    c.textContent = o.correct ? 'correct' : 'wrong';
    c.className = o.correct ? 'ok' : 'bad';
  }
  const dl = button('Download results (JSON)', () => {
    const report = buildReport({
      seed, soundMode: m, trials, outcomes, renderRate: RENDER_RATE, releaseS: RELEASE_S,
      sources: list.map((x, i) => ({
        name: x.name, sampleRate: x.sampleRate, seconds: x.signal.length / x.sampleRate, pitched: rendered[i].match.pitched,
        pitchCents: rendered[i].match.pitchCents, useSound: rendered[i].grain, matchMs: rendered[i].matchMs, renderMs: rendered[i].renderMs,
      })),
    });
    downloadBlob(new Blob([JSON.stringify(report, null, 2)], { type: 'application/json' }), `earworm-m4-blind-test-${seed}.json`);
  });
  el0.append(head, ...notes.map((n) => el('p', n, { className: 'muted' })), dl, tbl);
}

// ---------------------------------------------------------------- wiring

const drop = $('drop');
drop.addEventListener('dragover', (e) => { e.preventDefault(); drop.classList.add('over'); });
drop.addEventListener('dragleave', () => drop.classList.remove('over'));
drop.addEventListener('drop', (e) => {
  e.preventDefault();
  drop.classList.remove('over');
  if (e.dataTransfer?.files.length) void addFiles(e.dataTransfer.files);
});
$<HTMLInputElement>('file').onchange = (e) => {
  const input = e.target as HTMLInputElement;
  if (input.files?.length) void addFiles(input.files).then(() => { input.value = ''; });
};
$('rec').onclick = () => void record();
$('clear').onclick = () => { sources = []; void persist().then(() => { drawSources(); drawListen(); }); };
$('matchall').onclick = () => void renderAll();
$('stop').onclick = () => stopAll();
$('start').onclick = () => void start();
$('mode').onchange = () => drawListen();
// Sources saved on the Influence page show up when this tab is shown again.
document.addEventListener('visibilitychange', () => { if (!document.hidden && !busy) void reload(); });
void reload();
