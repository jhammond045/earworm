import { matchSound } from '../analysis/match';
import { createAudioContext, OfflineEngine } from '../host/engine-host';
import { downloadBlob, loadSources, resample } from './audio';

const TAIL_S = 0.6;
const MIN_SOURCES = 4;
const TARGET_PCT = 70;

interface Item {
  name: string;
  signal: Float32Array;
  match: Float32Array;
}

interface Trial {
  target: number;
  candidates: number[]; // indices into items, in display order A-D
}

interface Outcome {
  source: string;
  chosen: string;
  correct: boolean;
}

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
let ctx: AudioContext | null = null;
let playing: AudioBufferSourceNode | null = null;

function mulberry32(seed: number) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function shuffle<T>(a: T[], rnd: () => number): T[] {
  const x = a.slice();
  for (let i = x.length - 1; i > 0; i--) {
    const j = Math.floor(rnd() * (i + 1));
    [x[i], x[j]] = [x[j], x[i]];
  }
  return x;
}

function play(audio: Float32Array) {
  const c = ctx!;
  if (c.state === 'suspended') void c.resume();
  if (playing) { playing.onended = null; try { playing.stop(); } catch { /* done */ } }
  const buf = c.createBuffer(1, audio.length, c.sampleRate);
  buf.copyToChannel(audio as Float32Array<ArrayBuffer>, 0);
  const n = c.createBufferSource();
  n.buffer = buf;
  n.connect(c.destination);
  n.start();
  playing = n;
}

function setStatus(msg: string, err = false) {
  const el = $('status');
  el.textContent = msg;
  el.className = err ? 'err' : '';
}

async function start() {
  const seedText = $<HTMLInputElement>('seed').value.trim();
  const seed = seedText ? Number(seedText) >>> 0 : Math.floor(Math.random() * 1e9);
  $<HTMLInputElement>('seed').value = String(seed);
  $('trial').replaceChildren();
  $('result').replaceChildren();
  $<HTMLButtonElement>('start').disabled = true;
  try {
    ctx ??= createAudioContext();
    const bands = Number($<HTMLSelectElement>('bands').value) as 16 | 24;
    let saved;
    try {
      saved = await loadSources();
    } catch (e) {
      setStatus(`Saved sources are unavailable (${e instanceof Error ? e.message : e}). Add sources on the Influence page first.`, true);
      return;
    }
    if (saved.length < MIN_SOURCES) {
      setStatus(`Need at least ${MIN_SOURCES} saved sources, found ${saved.length}. Add them on the Influence page (20 varied sounds recommended: voice, whistle, flute, plucked string, bell, drum, clap, hiss, synth).`, true);
      return;
    }
    const engine = await OfflineEngine.create(ctx.sampleRate, bands);
    const items: Item[] = [];
    for (let i = 0; i < saved.length; i++) {
      setStatus(`Rendering ${i + 1} of ${saved.length}: ${saved[i].name}`);
      const sig = resample(saved[i].signal, saved[i].sampleRate, ctx.sampleRate);
      const m = await matchSound(sig, ctx.sampleRate, bands);
      items.push({ name: saved[i].name, signal: sig, match: engine.renderNote(m, m.pitchCents, Math.max(0.05, m.durationS), TAIL_S) });
    }
    engine.dispose();
    setStatus(`Seed ${seed}. ${items.length} trials.`);

    const rnd = mulberry32(seed);
    const trials: Trial[] = shuffle(items.map((_, i) => i), rnd).map((target) => {
      const others = shuffle(items.map((_, i) => i).filter((i) => i !== target), rnd).slice(0, 3);
      return { target, candidates: shuffle([target, ...others], rnd) };
    });
    runTrials(items, trials, seed);
  } catch (e) {
    setStatus(`Failed: ${e instanceof Error ? e.message : e}`, true);
  } finally {
    $<HTMLButtonElement>('start').disabled = false;
  }
}

function runTrials(items: Item[], trials: Trial[], seed: number) {
  const outcomes: Outcome[] = [];
  const box = $('trial');

  const show = (k: number) => {
    if (k >= trials.length) { box.replaceChildren(); finish(outcomes, seed); return; }
    const t = trials[k];
    box.replaceChildren();
    const h = document.createElement('h2');
    h.textContent = `Trial ${k + 1} of ${trials.length}`;
    const play0 = document.createElement('button');
    play0.textContent = 'Play match';
    play0.onclick = () => play(items[t.target].match);
    box.append(h, play0);
    t.candidates.forEach((idx, pos) => {
      const row = document.createElement('div');
      row.className = 'row cand';
      const label = 'ABCD'[pos];
      const p = document.createElement('button');
      p.textContent = `Play ${label}`;
      p.onclick = () => play(items[idx].signal);
      const pick = document.createElement('button');
      pick.textContent = `It was ${label}`;
      pick.onclick = () => {
        outcomes.push({ source: items[t.target].name, chosen: items[idx].name, correct: idx === t.target });
        show(k + 1);
      };
      row.append(p, pick);
      box.append(row);
    });
  };
  show(0);
}

function finish(outcomes: Outcome[], seed: number) {
  const right = outcomes.filter((o) => o.correct).length;
  const pct = (100 * right) / outcomes.length;
  const el = $('result');
  el.replaceChildren();
  const h = document.createElement('h2');
  h.textContent = `Score: ${right} of ${outcomes.length} (${pct.toFixed(0)}%), target ${TARGET_PCT}%`;
  h.className = pct >= TARGET_PCT ? 'ok' : 'bad';
  const t = document.createElement('table');
  const hr = t.insertRow();
  for (const c of ['Source', 'You chose', 'Result']) { const th = document.createElement('th'); th.textContent = c; hr.append(th); }
  for (const o of outcomes) {
    const r = t.insertRow();
    r.insertCell().textContent = o.source;
    r.insertCell().textContent = o.correct ? '-' : o.chosen;
    const c = r.insertCell();
    c.textContent = o.correct ? 'correct' : 'wrong';
    c.className = o.correct ? 'ok' : 'bad';
  }
  const dl = document.createElement('button');
  dl.textContent = 'Download results (JSON)';
  dl.onclick = () => downloadBlob(new Blob([JSON.stringify({ seed, bands: $<HTMLSelectElement>('bands').value, right, total: outcomes.length, percent: pct, outcomes }, null, 2)], { type: 'application/json' }), `influence-blind-test-${seed}.json`);
  el.append(h, dl, t);
}

$('start').onclick = () => void start();
