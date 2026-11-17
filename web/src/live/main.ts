// Live follow prototype page. Throwaway: one file of wiring, plain DOM.

import {
  EngineHost, OfflineEngine, createAudioContext, micConstraints, wantsFallback,
} from '../host/engine-host';
import {
  MSG_SET_PARAM, METER_LEN, M_ACTIVE_PARTIALS, M_BLEED, M_BREATH, M_DELAY_LEVEL, M_DELAY_PITCH, M_F0, M_FLOOR_DB,
  M_GATE, M_HARD, M_INPUT_DB, M_LOUD, M_OPEN, M_TONE_EFF, M_VOICING, M_VOWEL, PARAMS, P, RECORD_STRIDE, SCALES,
} from '../host/protocol';
import { download, encodeWav } from '../common/wav';
import { PRESETS, SOUND_PARAMS, presetValues } from './presets';
import { Ribbon, drawBands, noteName, readColors } from './ribbon';
import recorderUrl from './recorder-processor.ts?worker&url';

// Inside live-session.html (?embed=1): hide the links to the other page.
if (new URLSearchParams(location.search).has('embed')) document.documentElement.classList.add('embedded');

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

// ---------- settings (kept across engine restarts) ----------
const settings: Record<string, number> = {};
for (const d of PARAMS) settings[d.name] = d.default;
settings['level'] = -12;

interface Ctl {
  name: string; label: string; min: number; max: number; step: number;
  fmt: (v: number) => string; group: 'sound' | 'voice' | 'tuning';
}
const pct = (v: number) => Math.round(v * 100) + '%';
const num = (d: number, unit = '') => (v: number) => v.toFixed(d) + unit;
const SLIDES = ['glide', 'gliss', 'jump'];
const ROOTS = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];
const CTLS: Ctl[] = [
  { name: 'depth.mouth', label: 'Mouth depth', min: 0, max: 1, step: 0.01, fmt: pct, group: 'voice' },
  { name: 'depth.loud', label: 'Loud depth', min: 0, max: 1, step: 0.01, fmt: pct, group: 'voice' },
  { name: 'depth.open', label: 'Open depth', min: 0, max: 1, step: 0.01, fmt: pct, group: 'voice' },
  { name: 'depth.hard', label: 'Hard depth', min: 0, max: 1, step: 0.01, fmt: pct, group: 'voice' },
  { name: 'depth.breath', label: 'Breath depth', min: 0, max: 1, step: 0.01, fmt: pct, group: 'voice' },
  { name: 'tune.snap', label: 'Pitch Snap', min: 0, max: 1, step: 0.01, fmt: pct, group: 'tuning' },
  { name: 'tune.speed', label: 'Tune speed', min: 0, max: 250, step: 1, fmt: num(0, ' ms'), group: 'tuning' },
  { name: 'tune.slides', label: 'Slides', min: 0, max: 2, step: 1, fmt: (v) => SLIDES[Math.round(v)], group: 'tuning' },
  { name: 'tune.vibrato', label: 'Vibrato', min: 0, max: 2, step: 0.01, fmt: num(2), group: 'tuning' },
  { name: 'tune.octave', label: 'Octave', min: -2, max: 2, step: 1, fmt: (v) => (v > 0 ? '+' : '') + v, group: 'tuning' },
  { name: 'level', label: 'Level', min: -60, max: 6, step: 0.5, fmt: num(1, ' dB'), group: 'sound' },
  { name: 'harm_mix', label: 'Harmonic mix', min: 0, max: 1, step: 0.01, fmt: pct, group: 'sound' },
  { name: 'noise_mix', label: 'Noise mix', min: 0, max: 1, step: 0.01, fmt: pct, group: 'sound' },
  { name: 'tilt', label: 'Tilt', min: -24, max: 6, step: 0.5, fmt: num(1, ' dB/oct'), group: 'sound' },
  { name: 'partials', label: 'Partials', min: 1, max: 64, step: 1, fmt: num(0), group: 'sound' },
];

const setters = new Map<string, (v: number) => void>(); // update a control's display without sending

function addControl(c: Ctl) {
  const wrap = document.createElement('div');
  wrap.className = 'ctl';
  const id = 'c-' + c.name.replace('.', '-');
  const label = document.createElement('label');
  label.htmlFor = id;
  label.textContent = c.label;
  const out = document.createElement('output');
  out.className = 'mono';
  out.htmlFor = id;
  const input = document.createElement('input');
  input.type = 'range';
  input.id = id;
  input.min = String(c.min); input.max = String(c.max); input.step = String(c.step);
  const show = (v: number) => { input.value = String(v); out.textContent = c.fmt(v); };
  show(settings[c.name]);
  input.addEventListener('input', () => {
    const v = Number(input.value);
    out.textContent = c.fmt(v);
    settings[c.name] = v;
    host?.setParam(P[c.name], v);
  });
  setters.set(c.name, show);
  wrap.append(label, out, input);
  $(c.group).appendChild(wrap);
}

function addSelect(group: string, name: string, label: string, options: string[]) {
  const wrap = document.createElement('div');
  wrap.className = 'ctl';
  const id = 'c-' + name.replace('.', '-');
  const l = document.createElement('label');
  l.htmlFor = id;
  l.textContent = label;
  const sel = document.createElement('select');
  sel.id = id;
  options.forEach((o, i) => sel.add(new Option(o, String(i))));
  sel.value = String(settings[name]);
  sel.addEventListener('change', () => {
    settings[name] = Number(sel.value);
    host?.setParam(P[name], settings[name]);
  });
  wrap.append(l, sel);
  $(group).appendChild(wrap);
}

for (const c of CTLS.filter((c) => c.group === 'voice')) addControl(c);
for (const c of CTLS.filter((c) => c.group === 'tuning' && ['tune.snap', 'tune.speed', 'tune.slides', 'tune.vibrato'].includes(c.name))) addControl(c);
addSelect('tuning', 'tune.scale', 'Scale', SCALES);
addSelect('tuning', 'tune.root', 'Root', ROOTS);
addControl(CTLS.find((c) => c.name === 'tune.octave')!);
for (const c of CTLS.filter((c) => c.group === 'sound')) addControl(c);

// ---------- presets ----------
const presetBtns: HTMLButtonElement[] = [];
for (const p of PRESETS) {
  const b = document.createElement('button');
  b.type = 'button';
  b.textContent = p.name;
  b.title = p.blurb;
  b.setAttribute('aria-pressed', 'false');
  b.addEventListener('click', () => {
    const vals = presetValues(p);
    for (const name of SOUND_PARAMS) {
      settings[name] = vals[name];
      setters.get(name)?.(vals[name]);
      host?.setParam(P[name], vals[name]);
    }
    zeroBands();
    presetBtns.forEach((o) => o.setAttribute('aria-pressed', String(o === b)));
  });
  presetBtns.push(b);
  $('presets').appendChild(b);
}

function zeroBands() {
  if (!host) return;
  for (let i = 0; i < host.nBands; i++) { host.setToneBand(i, 0); host.setNoiseBand(i, 0); }
}

// ---------- meters ----------
interface MeterUi { set(frac: number, text: string, mark?: number): void }
function addMeter(label: string): MeterUi {
  const m = document.createElement('div');
  m.className = 'm';
  const l = document.createElement('span'); l.textContent = label;
  const track = document.createElement('div'); track.className = 'track';
  const fill = document.createElement('div'); fill.className = 'fill';
  const tick = document.createElement('div'); tick.className = 'tick'; tick.hidden = true;
  track.append(fill, tick);
  const val = document.createElement('span'); val.className = 'val mono';
  m.append(l, track, val);
  $('meters').appendChild(m);
  return {
    set(frac, text, mark) {
      fill.style.width = Math.max(0, Math.min(1, frac)) * 100 + '%';
      val.textContent = text;
      if (mark === undefined) tick.hidden = true;
      else { tick.hidden = false; tick.style.left = Math.max(0, Math.min(1, mark)) * 100 + '%'; }
    },
  };
}
const mInput = addMeter('Input / floor');
const mGate = addMeter('Gate');
const mVoicing = addMeter('Voicing');
const mLoud = addMeter('Loudness');
const mOpen = addMeter('Openness');
const mBreath = addMeter('Breathiness');
const mHard = addMeter('Hardness');
const mPitch = addMeter('f0');
const mParts = addMeter('Active partials');

// ---------- engine lifecycle ----------
let host: EngineHost | null = null;
let ctx: AudioContext | null = null;
let stream: MediaStream | null = null;
let micSrc: MediaStreamAudioSourceNode | null = null;
let outGain: GainNode | null = null; // live output; muted while a take records or plays
let recorder: AudioWorkletNode | null = null;
let nBands: 16 | 24 = 16;
let cursor = 0;

// Page-side ring of the last 60 s of frame records.
const REC_CAP = 6000;
const recBuf = new Float32Array(REC_CAP * RECORD_STRIDE);
let recTotal = 0;
const recSource = {
  get count() { return Math.min(recTotal, REC_CAP); },
  get(back: number): Float32Array | null {
    if (back < 0 || back >= Math.min(recTotal, REC_CAP)) return null;
    const i = ((recTotal - 1 - back) % REC_CAP) * RECORD_STRIDE;
    return recBuf.subarray(i, i + RECORD_STRIDE);
  },
};

function showError(msg: string) {
  const e = $('error');
  e.textContent = msg;
  e.hidden = !msg;
}

function drain() {
  if (!host) return;
  const b = host.readFrames(cursor);
  cursor = b.cursor;
  for (let k = 0; k < b.count; k++) {
    const dst = (recTotal % REC_CAP) * RECORD_STRIDE;
    recBuf.set(b.records.subarray(k * RECORD_STRIDE, (k + 1) * RECORD_STRIDE), dst);
    recTotal++;
  }
}

async function openMic(deviceId?: string) {
  micSrc?.disconnect();
  stream?.getTracks().forEach((t) => t.stop());
  stream = await navigator.mediaDevices.getUserMedia(micConstraints(deviceId));
  if (host) wireMic();
}

function wireMic() {
  micSrc = host!.connectMic(stream!);
  if (recorder) micSrc.connect(recorder);
}

async function startEngine(n: 16 | 24) {
  if (!ctx) throw new Error('no context');
  host = await EngineHost.create(ctx, { nBands: n, forceFallback: wantsFallback() });
  nBands = n;
  cursor = 0;
  recTotal = 0;
  if (stream) wireMic();
  host.node.disconnect();
  host.node.connect(outGain!);
  for (const d of PARAMS) host.setParam(P[d.name], settings[d.name]);
  zeroBands();
  host.setLive(true);
}

async function stopEngine() {
  micSrc?.disconnect();
  micSrc = null;
  host?.dispose();
  host = null;
}

async function refreshDevices() {
  const devs = (await navigator.mediaDevices.enumerateDevices()).filter((d) => d.kind === 'audioinput');
  const sel = $<HTMLSelectElement>('device');
  const cur = stream?.getAudioTracks()[0]?.getSettings().deviceId;
  sel.replaceChildren(...devs.map((d, i) => new Option(d.label || `Input ${i + 1}`, d.deviceId)));
  if (cur) sel.value = cur;
  sel.disabled = false;
}

$<HTMLButtonElement>('start').addEventListener('click', async (ev) => {
  const btn = ev.currentTarget as HTMLButtonElement;
  btn.disabled = true;
  showError('');
  try {
    ctx = createAudioContext(); // inside the click so the context may start
    await ctx.resume();
    outGain = ctx.createGain();
    outGain.connect(ctx.destination);
    await ctx.audioWorklet.addModule(recorderUrl);
    recorder = new AudioWorkletNode(ctx, 'earworm-recorder', {
      numberOfInputs: 1, numberOfOutputs: 1, channelCount: 1, channelCountMode: 'explicit',
    });
    recorder.connect(ctx.destination); // silent; keeps the node pulled
    recorder.port.onmessage = onRecorderMessage;
    stream = await navigator.mediaDevices.getUserMedia(micConstraints());
    await startEngine(nBands);
    await refreshDevices();
    for (const id of ['save', 'reset', 'panic']) $<HTMLButtonElement>(id).disabled = false;
    updateTakeUi();
    btn.textContent = 'Running';
  } catch (e) {
    showError('Could not start: ' + (e instanceof Error ? e.message : String(e)));
    btn.disabled = false;
  }
});

$<HTMLSelectElement>('device').addEventListener('change', async (ev) => {
  try {
    await openMic((ev.currentTarget as HTMLSelectElement).value);
    showError('');
  } catch (e) {
    showError('Could not open that input: ' + (e instanceof Error ? e.message : String(e)));
  }
});

const nbBtns = Array.from(document.querySelectorAll<HTMLButtonElement>('#nb button'));
for (const b of nbBtns) {
  b.addEventListener('click', async () => {
    const n = Number(b.dataset.n) as 16 | 24;
    nbBtns.forEach((o) => o.setAttribute('aria-pressed', String(o === b)));
    if (n === nBands && host) return;
    nBands = n;
    if (!ctx) return; // not started yet: used at Start
    try {
      await stopEngine();
      await startEngine(n);
    } catch (e) {
      showError('Could not restart the engine: ' + (e instanceof Error ? e.message : String(e)));
    }
  });
}

$<HTMLButtonElement>('reset').addEventListener('click', () => host?.resetSinger());
$<HTMLButtonElement>('panic').addEventListener('click', () => host?.panic());

// ---------- save last 30 s ----------
$<HTMLButtonElement>('save').addEventListener('click', () => {
  if (!host) return;
  drain();
  const sr = host.sampleRate;
  const end = host.captureCount();
  const len = Math.min(Math.round(30 * sr), end);
  const { mic } = host.readCapture(end - len, len);
  const nFrames = Math.min(3000, recSource.count);
  const frames = new Float32Array(nFrames * RECORD_STRIDE);
  for (let i = 0; i < nFrames; i++) frames.set(recSource.get(nFrames - 1 - i)!, i * RECORD_STRIDE);
  const stamp = new Date().toISOString().replace(/[:.]/g, '-');
  const meta = {
    sampleRate: sr,
    nBands: host.nBands,
    transportMode: host.mode,
    baseLatency: ctx?.baseLatency ?? null,
    outputLatency: ctx?.outputLatency ?? null,
    settings: { ...settings },
    micSamples: len,
    micEndSample: end,
    frames: nFrames,
    frameRate: 100,
    recordStride: RECORD_STRIDE,
    framesFile: `earworm-live-${stamp}.f32`,
    note: 'Float32 little-endian, oldest record first. Slot 0 pitch cents or NaN; last slot of each record is the tuned pitch.',
  };
  download(encodeWav(mic, sr), `earworm-live-${stamp}.wav`);
  download(new Blob([frames.buffer as ArrayBuffer], { type: 'application/octet-stream' }), meta.framesFile);
  download(new Blob([JSON.stringify(meta, null, 2)], { type: 'application/json' }), `earworm-live-${stamp}.json`);
});

// ---------- record a take, play it back ----------
// Record captures the raw mic with the live output muted. Play runs the take
// through a fresh offline engine on the same live path with the current
// settings, so changing a setting and pressing Play again re-renders it.
const TAKE_MAX_S = 60;
let takeChunks: Float32Array[] = [];
let take: Float32Array | null = null;
let recording = false;
let player: AudioBufferSourceNode | null = null;
let recTimer = 0;
let takeInfo = '';

function updateMute() {
  if (outGain) outGain.gain.value = recording || player ? 0 : 1;
}

function takeLength(): number {
  return takeChunks.reduce((a, c) => a + c.length, 0);
}

function updateTakeUi(msg?: string) {
  const started = !!host;
  $<HTMLButtonElement>('take-rec').disabled = !started || recording;
  $<HTMLButtonElement>('take-stop').disabled = !recording && !player;
  $<HTMLButtonElement>('take-play').disabled = !started || recording || !take;
  $<HTMLButtonElement>('take-voice').disabled = !started || recording || !take;
  if (msg !== undefined) $('take-status').textContent = msg;
}

function onRecorderMessage(ev: MessageEvent) {
  if (ev.data.chunk) {
    takeChunks.push(ev.data.chunk as Float32Array);
    if (host && takeLength() >= TAKE_MAX_S * host.sampleRate) stopRecording();
  } else if (ev.data.done) {
    const n = takeLength();
    take = new Float32Array(n);
    let o = 0;
    for (const c of takeChunks) { take.set(c, o); o += c.length; }
    takeChunks = [];
    takeInfo = `Take: ${(n / (host?.sampleRate ?? 48000)).toFixed(1)} s. Play synth renders it with the current settings.`;
    updateTakeUi(takeInfo);
  }
}

function stopRecording() {
  if (!recording) return;
  recording = false;
  clearInterval(recTimer);
  recorder?.port.postMessage('stop');
  updateMute();
  updateTakeUi('Finishing the take...');
}

function stopPlayback() {
  if (!player) return;
  const p = player;
  player = null;
  p.onended = null;
  p.stop();
  p.disconnect();
  updateMute();
  updateTakeUi(takeInfo);
}

function play(samples: Float32Array, label: string) {
  if (!ctx) return;
  stopPlayback();
  const buf = ctx.createBuffer(1, samples.length, ctx.sampleRate);
  buf.copyToChannel(samples as Float32Array<ArrayBuffer>, 0);
  player = ctx.createBufferSource();
  player.buffer = buf;
  player.connect(ctx.destination);
  player.onended = () => { player = null; updateMute(); updateTakeUi(takeInfo); };
  updateMute();
  player.start();
  updateTakeUi(label);
}

$<HTMLButtonElement>('take-rec').addEventListener('click', () => {
  if (!recorder || !host) return;
  stopPlayback();
  takeChunks = [];
  recording = true;
  updateMute();
  recorder.port.postMessage('start');
  const t0 = performance.now();
  recTimer = window.setInterval(() => {
    $('take-status').textContent = `Recording, live sound muted: ${((performance.now() - t0) / 1000).toFixed(0)} s (max ${TAKE_MAX_S} s).`;
  }, 250);
  updateTakeUi('Recording, live sound muted.');
});

$<HTMLButtonElement>('take-stop').addEventListener('click', () => {
  if (recording) stopRecording();
  else stopPlayback();
});

$<HTMLButtonElement>('take-play').addEventListener('click', async () => {
  if (!take || !ctx) return;
  const sr = ctx.sampleRate;
  const eng = await OfflineEngine.create(sr, nBands);
  let r;
  try {
    r = eng.processLive(take, PARAMS.map((d, i): [number, number, number, number] => [MSG_SET_PARAM, 0, i, settings[d.name]]));
  } finally {
    eng.dispose();
  }
  const muted = r.bleedSamples / sr;
  play(r.out, `Playing the synth for this take (${nBands} bands, current settings).` +
    (muted > 0.05 ? ` The bleed check muted ${muted.toFixed(1)} s of it: see the note below.` : ''));
  $('take-bleed').hidden = muted <= 0.05;
});

$<HTMLButtonElement>('take-voice').addEventListener('click', () => {
  if (take) play(take, 'Playing your voice as recorded.');
});

// ---------- drawing ----------
const ribbon = new Ribbon($('ribbon') as HTMLCanvasElement);
const meters = new Float32Array(METER_LEN);
const dbFrac = (db: number) => (db + 100) / 100;
let lastInfo = 0;

function updateInfo() {
  if (!host || !ctx) return;
  const lat = (x: number | undefined) => (x === undefined ? 'n/a' : (x * 1000).toFixed(1) + ' ms');
  $('transport').textContent =
    `transport ${host.mode} | ${host.sampleRate} Hz | baseLatency ${lat(ctx.baseLatency)} | outputLatency ${lat(ctx.outputLatency)}` +
    ` | nominal analysis delay level ${lat(meters[M_DELAY_LEVEL])}, pitch ${lat(meters[M_DELAY_PITCH])} | bands ${host.nBands}`;
}

function frame(now: number) {
  requestAnimationFrame(frame);
  if (document.hidden || !host) return;
  drain();
  const have = host.readMeters(meters);
  const col = readColors();
  const dark = matchMedia('(prefers-color-scheme: dark)').matches;
  ribbon.draw(recSource, settings['tune.scale'], settings['tune.root'], col, dark);
  if (!have) return;
  const n = host.nBands;
  drawBands($('vowel') as HTMLCanvasElement, meters.subarray(M_VOWEL, M_VOWEL + n), n, 24, col);
  drawBands($('tone') as HTMLCanvasElement, meters.subarray(M_TONE_EFF, M_TONE_EFF + n), n, 24, col);
  const m = meters;
  $('bleed').hidden = m[M_BLEED] < 0.5 || recording || !!player;
  mInput.set(dbFrac(m[M_INPUT_DB]), `${m[M_INPUT_DB].toFixed(0)} dB / floor ${m[M_FLOOR_DB].toFixed(0)} dB`, dbFrac(m[M_FLOOR_DB]));
  mGate.set(m[M_GATE], m[M_GATE] > 0.5 ? 'open' : 'closed');
  const v = Math.round(m[M_VOICING]);
  mVoicing.set(v === 0 ? 1 : v === 1 ? 0.5 : 0, ['voiced', 'unvoiced', 'silent'][v] ?? '?');
  mLoud.set(m[M_LOUD], m[M_LOUD].toFixed(2));
  mOpen.set(m[M_OPEN], m[M_OPEN].toFixed(2));
  mBreath.set(m[M_BREATH], m[M_BREATH].toFixed(2));
  mHard.set(m[M_HARD], m[M_HARD].toFixed(2));
  const f0 = m[M_F0];
  mPitch.set(f0 > 0 ? Math.log2(f0 / 70) / 4 : 0, f0 > 0 ? `${f0.toFixed(1)} Hz ${noteName(69 + 12 * Math.log2(f0 / 440))}` : 'none');
  mParts.set(m[M_ACTIVE_PARTIALS] / 64, String(Math.round(m[M_ACTIVE_PARTIALS])));
  if (now - lastInfo > 1000) { lastInfo = now; updateInfo(); }
}
requestAnimationFrame(frame);

// Keep collecting frames for Save even if the tab is in the background (the ring holds 60 s).
setInterval(() => { if (document.hidden) drain(); }, 1000);
