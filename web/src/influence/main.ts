import { matchSound, type Match } from '../analysis/match';
import { createAudioContext, OfflineEngine } from '../host/engine-host';
import { MAX_BANDS, PARAMS } from '../host/protocol';
import { clearSources, decodeFile, downloadBlob, loadSources, recordMic, saveSources, wavBlob } from './audio';

const TAIL_S = 0.6;
const GAP_S = 0.3;

interface Source {
  name: string;
  signal: Float32Array;
  sampleRate: number;
  peaks: Float32Array;
  match?: Match;
  matchBands?: number;
  render?: { key: string; audio: Float32Array };
}

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const sources: Source[] = [];
let selected = -1;
let ctx: AudioContext | null = null;
const engines = new Map<number, Promise<OfflineEngine>>();
let playing: AudioBufferSourceNode[] = [];
let pitchChoice = 'source'; // 'source' or a MIDI note number
let busy = false;

const nBands = () => Number($<HTMLSelectElement>('bands').value) as 16 | 24;

function setStatus(msg: string, err = false) {
  const el = $('status');
  el.textContent = msg;
  el.className = err ? 'err' : '';
}

function audioCtx(): AudioContext {
  ctx ??= createAudioContext();
  if (ctx.state === 'suspended') void ctx.resume();
  return ctx;
}

function getEngine(bands: 16 | 24): Promise<OfflineEngine> {
  let e = engines.get(bands);
  if (!e) {
    e = OfflineEngine.create(audioCtx().sampleRate, bands);
    engines.set(bands, e);
  }
  return e;
}

function peaksOf(x: Float32Array, n = 200): Float32Array {
  const out = new Float32Array(n);
  const step = x.length / n;
  for (let i = 0; i < n; i++) {
    let m = 0;
    for (let j = Math.floor(i * step); j < Math.floor((i + 1) * step); j++) m = Math.max(m, Math.abs(x[j]));
    out[i] = m;
  }
  return out;
}

function drawWave(c: HTMLCanvasElement, peaks: Float32Array) {
  const w = (c.width = 200), h = (c.height = 36);
  const g = c.getContext('2d')!;
  g.clearRect(0, 0, w, h);
  g.fillStyle = '#7a7';
  for (let i = 0; i < peaks.length; i++) {
    const a = Math.max(1, peaks[i] * h);
    g.fillRect(i, (h - a) / 2, 1, a);
  }
}

function drawBars(c: HTMLCanvasElement, v: Float32Array, n: number) {
  const w = (c.width = 240), h = (c.height = 80);
  const g = c.getContext('2d')!;
  g.clearRect(0, 0, w, h);
  let lo = Infinity, hi = -Infinity;
  for (let i = 0; i < n; i++) { lo = Math.min(lo, v[i]); hi = Math.max(hi, v[i]); }
  if (!(hi > lo)) { lo = hi - 1; }
  g.fillStyle = '#69c';
  const bw = w / n;
  for (let i = 0; i < n; i++) {
    const a = 2 + ((v[i] - lo) / (hi - lo)) * (h - 4);
    g.fillRect(i * bw + 1, h - a, bw - 2, a);
  }
}

// ---- matching and rendering ----

async function ensureMatch(s: Source): Promise<Match> {
  const bands = nBands();
  if (!s.match || s.matchBands !== bands) {
    s.match = await matchSound(s.signal, s.sampleRate, bands);
    s.matchBands = bands;
    s.render = undefined;
  }
  return s.match;
}

function auditionCents(m: Match): number {
  return pitchChoice === 'source' ? m.pitchCents : (Number(pitchChoice) - 12) * 100;
}

async function ensureRender(s: Source): Promise<Float32Array> {
  const m = await ensureMatch(s);
  const bands = nBands();
  const cents = auditionCents(m);
  const key = `${bands}|${cents}`;
  if (!s.render || s.render.key !== key) {
    const eng = await getEngine(bands);
    s.render = { key, audio: eng.renderNote(m, cents, Math.max(0.05, m.durationS), TAIL_S) };
  }
  return s.render.audio;
}

async function process(s: Source) {
  await ensureMatch(s);
  await ensureRender(s);
}

async function reprocessAll() {
  if (busy) return;
  busy = true;
  try {
    for (let i = 0; i < sources.length; i++) {
      setStatus(`Matching ${i + 1} of ${sources.length}: ${sources[i].name}`);
      await process(sources[i]);
      renderList();
      if (i === selected) renderDetail();
    }
    setStatus(sources.length ? 'Done.' : '');
  } catch (e) {
    setStatus(`Match failed: ${e instanceof Error ? e.message : e}`, true);
  } finally {
    busy = false;
  }
}

// ---- playback ----

function stopAll() {
  for (const n of playing) { n.onended = null; try { n.stop(); } catch { /* already stopped */ } }
  playing = [];
}

function playSeq(parts: Float32Array[]) {
  const c = audioCtx();
  stopAll();
  let t = c.currentTime + 0.05;
  for (const p of parts) {
    const buf = c.createBuffer(1, p.length, c.sampleRate);
    buf.copyToChannel(p as Float32Array<ArrayBuffer>, 0);
    const n = c.createBufferSource();
    n.buffer = buf;
    n.connect(c.destination);
    n.start(t);
    playing.push(n);
    t += buf.duration + GAP_S;
  }
}

// ---- adding sources ----

async function persist() {
  try {
    // The M4 blind test page saves to the same store, so keep sources this page did not add
    // (by name) instead of replacing the whole set. "Clear saved" still clears everything.
    const mine = sources.map((s) => ({ name: s.name, sampleRate: s.sampleRate, signal: s.signal }));
    const names = new Set(mine.map((s) => s.name));
    const others = (await loadSources()).filter((s) => !names.has(s.name));
    await saveSources([...others, ...mine]);
  } catch (e) {
    setStatus(`Could not save sources for the test page (storage unavailable: ${e instanceof Error ? e.message : e}). The prototype still works.`, true);
    return false;
  }
  return true;
}

async function addSignal(name: string, signal: Float32Array) {
  const s: Source = { name, signal, sampleRate: audioCtx().sampleRate, peaks: peaksOf(signal) };
  sources.push(s);
  selected = sources.length - 1;
  renderList();
  renderDetail();
  try {
    await process(s);
  } catch (e) {
    setStatus(`Match failed for ${name}: ${e instanceof Error ? e.message : e}`, true);
  }
  renderList();
  renderDetail();
}

async function addFiles(files: File[]) {
  if (busy) return;
  busy = true;
  try {
    const c = audioCtx();
    for (const f of files) {
      setStatus(`Decoding ${f.name}...`);
      try {
        await addSignal(f.name, await decodeFile(c, f));
      } catch (e) {
        setStatus(`Could not use ${f.name}: ${e instanceof Error ? e.message : e}`, true);
        return;
      }
    }
    if (await persist()) setStatus(`${sources.length} source(s) saved for the blind test.`);
  } finally {
    busy = false;
  }
}

async function recordClip() {
  if (busy) return;
  busy = true;
  try {
    const c = audioCtx();
    setStatus('Recording 3 s...');
    const x = await recordMic(c, 3);
    await addSignal(`mic ${new Date().toLocaleTimeString()}`, x);
    if (await persist()) setStatus(`${sources.length} source(s) saved for the blind test.`);
  } catch (e) {
    setStatus(`Recording failed: ${e instanceof Error ? e.message : e}`, true);
  } finally {
    busy = false;
  }
}

// ---- UI ----

function fmt(v: number): string {
  return Math.abs(v) >= 100 ? v.toFixed(0) : Math.abs(v) >= 10 ? v.toFixed(1) : v.toFixed(3);
}

function renderList() {
  const el = $('list');
  el.replaceChildren();
  sources.forEach((s, i) => {
    const b = document.createElement('button');
    b.className = 'src' + (i === selected ? ' sel' : '');
    const c = document.createElement('canvas');
    drawWave(c, s.peaks);
    const t = document.createElement('span');
    t.textContent = s.name;
    const d = document.createElement('small');
    d.textContent = `${(s.signal.length / s.sampleRate).toFixed(2)} s` + (s.match ? `, matched in ${s.match.ms.toFixed(0)} ms` : ', matching...');
    b.append(t, c, d);
    b.onclick = () => { selected = i; renderList(); renderDetail(); };
    el.append(b);
  });
}

function paramTable(title: string, from: number, to: number, m: Match): HTMLElement {
  const wrap = document.createElement('div');
  const h = document.createElement('h3');
  h.textContent = title;
  const t = document.createElement('table');
  t.className = 'params';
  for (let i = from; i <= to; i++) {
    const r = t.insertRow();
    r.insertCell().textContent = PARAMS[i].name;
    r.insertCell().textContent = fmt(m.scalars[i]);
  }
  wrap.append(h, t);
  return wrap;
}

function btn(label: string, fn: () => void): HTMLButtonElement {
  const b = document.createElement('button');
  b.textContent = label;
  b.onclick = fn;
  return b;
}

function noteOptions(): HTMLSelectElement {
  const sel = document.createElement('select');
  const names = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];
  sel.add(new Option('source pitch', 'source'));
  for (let n = 36; n <= 84; n++) sel.add(new Option(`${names[n % 12]}${Math.floor(n / 12) - 1}`, String(n)));
  sel.value = pitchChoice;
  return sel;
}

function renderDetail() {
  const el = $('detail');
  el.replaceChildren();
  const s = sources[selected];
  if (!s) return;
  const h = document.createElement('h2');
  h.textContent = s.name;
  el.append(h);

  const row = document.createElement('div');
  row.className = 'row';
  const withRender = async (fn: (r: Float32Array) => void) => {
    try { fn(await ensureRender(s)); } catch (e) { setStatus(`Render failed: ${e instanceof Error ? e.message : e}`, true); }
  };
  row.append(
    btn('Play source', () => playSeq([s.signal])),
    btn('Play match', () => void withRender((r) => playSeq([r]))),
    btn('Play A then B', () => void withRender((r) => playSeq([s.signal, r]))),
    btn('Stop', stopAll),
  );
  el.append(row);

  const row2 = document.createElement('div');
  row2.className = 'row';
  const lab = document.createElement('label');
  lab.append('Audition pitch ');
  const sel = noteOptions();
  sel.onchange = () => {
    pitchChoice = sel.value;
    for (const x of sources) x.render = undefined;
    void withRender(() => {});
  };
  lab.append(sel);
  row2.append(lab, btn('Download match WAV', () => void withRender((r) =>
    downloadBlob(wavBlob(r, audioCtx().sampleRate), `${s.name.replace(/\.[^.]+$/, '')}-match.wav`))));
  el.append(row2);

  const m = s.match;
  if (!m) { el.append('Matching...'); return; }

  const info = document.createElement('p');
  const slow = m.ms > 1000;
  info.innerHTML = '';
  const ms = document.createElement('span');
  ms.className = slow ? 'slow' : '';
  ms.textContent = `Matched in ${m.ms.toFixed(0)} ms${slow ? ' (over the 1 s budget)' : ''}. `;
  info.append(ms, `${m.pitched ? 'Pitched' : 'Unpitched'}, ${(m.voicedFraction * 100).toFixed(0)}% voiced, ` +
    `hold ${m.durationS.toFixed(2)} s, pitch ${(m.pitchCents / 100 + 12).toFixed(1)} semitones (MIDI).`);
  el.append(info);

  const tables = document.createElement('div');
  tables.className = 'charts';
  tables.append(paramTable('Sources', 0, 10, m), paramTable('Envelope', 11, 18, m));
  el.append(tables);

  const bands = s.matchBands ?? nBands();
  const charts = document.createElement('div');
  charts.className = 'charts';
  for (const [title, v] of [['Tone bands (dB)', m.tone], ['Noise bands (dB)', m.noise]] as const) {
    const box = document.createElement('div');
    const cap = document.createElement('div');
    cap.textContent = title;
    const c = document.createElement('canvas');
    drawBars(c, v.subarray(0, Math.min(bands, MAX_BANDS)), Math.min(bands, MAX_BANDS));
    box.append(cap, c);
    charts.append(box);
  }
  el.append(charts);
}

function wire() {
  $<HTMLInputElement>('file').onchange = (e) => {
    const input = e.target as HTMLInputElement;
    void addFiles(Array.from(input.files ?? [])).then(() => { input.value = ''; });
  };
  $('rec').onclick = () => void recordClip();
  $('bands').onchange = () => { for (const s of sources) s.render = undefined; void reprocessAll(); };
  $('clear').onclick = async () => {
    try {
      await clearSources();
      setStatus('Saved sources cleared (the list on this page is unchanged).');
    } catch (e) {
      setStatus(`Could not clear saved sources: ${e instanceof Error ? e.message : e}`, true);
    }
  };
  const drop = $('drop');
  drop.ondragover = (e) => { e.preventDefault(); drop.classList.add('over'); };
  drop.ondragleave = () => drop.classList.remove('over');
  drop.ondrop = (e) => {
    e.preventDefault();
    drop.classList.remove('over');
    void addFiles(Array.from(e.dataTransfer?.files ?? []).filter((f) => f.type.startsWith('audio/') || /\.(wav|mp3|ogg|flac|m4a|aac)$/i.test(f.name)));
  };
}

wire();
