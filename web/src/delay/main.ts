// Delay measurement page. Two runners (round trip, engine internal delay),
// a totals comparison against the TDD budget, and a JSON download.
import {
  EngineHost, createAudioContext, micConstraints, wantsFallback,
} from '../host/engine-host';
import { M_DELAY_LEVEL, M_DELAY_PITCH, METER_LEN, P } from '../host/protocol';
import {
  chirp, crossCorrelationLag, median, medianF0, onsetIndex, pitchLockIndex,
  segmentBursts, spread, synthHum,
} from './measure';
import loopbackUrl from './loopback-processor.ts?worker&url';

const CLICKS = 5;
const SPACING_S = 0.4;
const LEAD_S = 0.5;
const SEARCH_S = 0.3;
const MIN_NORM = 0.2; // below this a click was not really found

export interface RoundTripResult {
  sampleRate: number;
  lagsMs: (number | null)[];
  foundClicks: number;
  medianMs: number | null;
  minMs: number | null;
  maxMs: number | null;
  stdevMs: number | null;
  baseLatencyMs: number | null;
  outputLatencyMs: number | null;
  error?: string;
}

export interface BurstResult {
  micOnsetS: number;
  levelDelayMs: number | null;
  pitchDelayMs: number | null;
  inputF0: number | null;
}

export interface EngineResult {
  sampleRate: number;
  transport: string;
  bursts: BurstResult[];
  levelMedianMs: number | null;
  pitchMedianMs: number | null;
  meterDelayLevel: number | null;
  meterDelayPitch: number | null;
  error?: string;
}

const num = (x: number): number | null => (Number.isFinite(x) ? x : null);
const ms = (samples: number, sr: number) => (samples / sr) * 1000;
const sleep = (s: number) => new Promise((r) => setTimeout(r, s * 1000));

let ctx: AudioContext | null = null;
let stream: MediaStream | null = null;
let workletLoaded = false;
let last: { roundTrip?: RoundTripResult; engine?: EngineResult } = {};

const statusEl = document.getElementById('status') as HTMLElement;
const setStatus = (s: string) => { statusEl.textContent = s; };

async function session(): Promise<{ ctx: AudioContext; stream: MediaStream }> {
  if (!ctx) ctx = createAudioContext();
  if (ctx.state !== 'running') await ctx.resume();
  if (!stream) stream = await navigator.mediaDevices.getUserMedia(micConstraints());
  return { ctx, stream };
}

export async function runRoundTrip(): Promise<RoundTripResult> {
  setStatus('Round trip: playing clicks.');
  const s = await session();
  const sr = s.ctx.sampleRate;
  const base: RoundTripResult = {
    sampleRate: sr, lagsMs: [], foundClicks: 0, medianMs: null, minMs: null, maxMs: null,
    stdevMs: null,
    baseLatencyMs: num(s.ctx.baseLatency * 1000),
    outputLatencyMs: num((s.ctx.outputLatency ?? NaN) * 1000),
  };
  if (!workletLoaded) { await s.ctx.audioWorklet.addModule(loopbackUrl); workletLoaded = true; }
  const click = chirp(sr);
  const spacing = Math.round(SPACING_S * sr);
  const lead = Math.round(LEAD_S * sr);
  const total = lead + CLICKS * spacing + Math.round(SEARCH_S * sr) + click.length;
  const node = new AudioWorkletNode(s.ctx, 'loopback', {
    numberOfInputs: 1, numberOfOutputs: 1, outputChannelCount: [1],
    processorOptions: { click, spacing, count: CLICKS, lead, total },
  });
  const src = s.ctx.createMediaStreamSource(s.stream);
  const done = new Promise<Float32Array>((resolve) => {
    node.port.onmessage = (e) => resolve(e.data.rec as Float32Array);
  });
  src.connect(node);
  node.connect(s.ctx.destination);
  const rec = await done;
  src.disconnect();
  node.disconnect();

  const maxLag = Math.round(SEARCH_S * sr);
  const good: number[] = [];
  for (let k = 0; k < CLICKS; k++) {
    const r = crossCorrelationLag(rec, click, maxLag, lead + k * spacing);
    if (r.norm >= MIN_NORM && Number.isFinite(r.lag)) {
      const v = ms(r.lag, sr);
      good.push(v);
      base.lagsMs.push(v);
    } else base.lagsMs.push(null);
  }
  base.foundClicks = good.length;
  if (good.length === 0) {
    base.error = 'No click found in the recording. Turn the speakers up or use a loopback cable.';
  } else {
    const sp = spread(good);
    base.medianMs = median(good);
    base.minMs = sp.min; base.maxMs = sp.max; base.stdevMs = sp.stdev;
  }
  last.roundTrip = base;
  render();
  return base;
}

export async function runEngineDelay(): Promise<EngineResult> {
  setStatus('Engine delay: starting engine.');
  const s = await session();
  const sr = s.ctx.sampleRate;
  const host = await EngineHost.create(s.ctx, { nBands: 16, forceFallback: wantsFallback() });
  const res: EngineResult = {
    sampleRate: sr, transport: host.mode, bursts: [], levelMedianMs: null, pitchMedianMs: null,
    meterDelayLevel: null, meterDelayPitch: null,
  };
  // The engine runs but is not heard: its node goes to a muted gain.
  const mute = s.ctx.createGain();
  mute.gain.value = 0;
  host.node.disconnect();
  host.node.connect(mute);
  mute.connect(s.ctx.destination);
  const micSrc = host.connectMic(s.stream);

  host.setParam(P['depth.mouth'], 0);
  host.setParam(P['tune.snap'], 0);
  host.setParam(P['depth.loud'], 1);
  host.setParam(P['harm_mix'], 1);
  host.setParam(P['noise_mix'], 0);
  host.setLive(true);
  await sleep(0.3);

  const hum = synthHum(sr);
  const buf = s.ctx.createBuffer(1, hum.samples.length, sr);
  buf.copyToChannel(hum.samples as Float32Array<ArrayBuffer>, 0);
  const player = s.ctx.createBufferSource();
  player.buffer = buf;
  player.connect(s.ctx.destination);
  const c0 = host.captureCount();
  setStatus('Engine delay: playing test hum.');
  player.start(s.ctx.currentTime + 0.3);
  await sleep(0.3 + buf.duration + 0.5);

  const n = host.captureCount() - c0;
  const cap = host.readCapture(c0, n);
  const meters = new Float32Array(METER_LEN);
  if (host.readMeters(meters)) {
    res.meterDelayLevel = num(meters[M_DELAY_LEVEL]);
    res.meterDelayPitch = num(meters[M_DELAY_PITCH]);
  }
  host.setLive(false);
  micSrc.disconnect();
  player.disconnect();
  mute.disconnect();
  host.dispose();

  setStatus('Engine delay: analysing.');
  const w = (s: number) => Math.round(s * sr);
  const bursts = segmentBursts(cap.mic, sr);
  const levels: number[] = [];
  const pitches: number[] = [];
  for (const b of bursts) {
    const r: BurstResult = { micOnsetS: b.start / sr, levelDelayMs: null, pitchDelayMs: null, inputF0: null };
    // A low threshold on both: the synth reaches full level faster than the
    // faded-in test hum, so a 10% threshold would read as negative delay.
    const micOn = onsetIndex(cap.mic, sr, b.start - w(0.05), b.start + w(0.1), { frac: 0.02 });
    if (micOn >= 0) {
      r.micOnsetS = micOn / sr;
      const outOn = onsetIndex(cap.out, sr, micOn - w(0.03), micOn + w(0.4), { frac: 0.02 });
      if (outOn >= 0) {
        r.levelDelayMs = ms(outOn - micOn, sr);
        levels.push(r.levelDelayMs);
      }
      const f0 = medianF0(cap.mic, sr, micOn + w(0.2), Math.min(b.end - w(0.06), micOn + w(0.5)));
      r.inputF0 = num(f0);
      if (Number.isFinite(f0)) {
        const from = micOn - w(0.01), to = micOn + w(0.6);
        let lock = pitchLockIndex(cap.out, sr, from, to, f0);
        if (lock < 0) lock = pitchLockIndex(cap.out, sr, from, to, f0, 50, 3, 2, 0.8);
        if (lock >= 0) {
          r.pitchDelayMs = ms(lock - micOn, sr);
          pitches.push(r.pitchDelayMs);
        }
      }
    }
    res.bursts.push(r);
  }
  res.levelMedianMs = num(median(levels));
  res.pitchMedianMs = num(median(pitches));
  if (bursts.length === 0) res.error = 'No hum heard on the mic ring. Turn the speakers up or use a loopback cable.';
  else if (levels.length === 0) res.error = 'Engine output stayed silent. Is Live follow running?';
  last.engine = res;
  render();
  return res;
}

// ---------- rendering ----------

const fmt = (v: number | null | undefined, d = 1) => (v == null ? '-' : v.toFixed(d));

function table(head: string[], rows: string[][]): HTMLTableElement {
  const t = document.createElement('table');
  t.className = 'res';
  const h = t.createTHead().insertRow();
  for (const c of head) h.insertCell().outerHTML = `<th>${c}</th>`;
  const tb = t.createTBody();
  for (const r of rows) {
    const tr = tb.insertRow();
    for (const c of r) tr.insertCell().textContent = c;
  }
  return t;
}

function put(id: string, ...nodes: (Node | string)[]) {
  const el = document.getElementById(id)!;
  el.replaceChildren(...nodes);
}

function note(text: string): HTMLElement {
  const p = document.createElement('p');
  p.textContent = text;
  return p;
}

function render() {
  const rt = last.roundTrip;
  const en = last.engine;
  if (rt) {
    put('rt',
      table(['Median ms', 'Min', 'Max', 'Stdev', 'Clicks found', 'baseLatency ms', 'outputLatency ms'],
        [[fmt(rt.medianMs), fmt(rt.minMs), fmt(rt.maxMs), fmt(rt.stdevMs, 2), `${rt.foundClicks}/${CLICKS}`,
          fmt(rt.baseLatencyMs), fmt(rt.outputLatencyMs)]]),
      note(`Per click, ms: ${rt.lagsMs.map((v) => fmt(v)).join(', ')}`),
      ...(rt.error ? [note(rt.error)] : []));
  }
  if (en) {
    put('eng',
      table(['Burst', 'Mic onset s', 'Input f0 Hz', 'Level delay ms', 'Pitch delay ms'],
        en.bursts.map((b, i) => [String(i + 1), fmt(b.micOnsetS, 3), fmt(b.inputF0), fmt(b.levelDelayMs), fmt(b.pitchDelayMs)])
          .concat([['Median', '', '', fmt(en.levelMedianMs), fmt(en.pitchMedianMs)]])),
      note(`Transport: ${en.transport}. Engine's own meters (raw): level ${fmt(en.meterDelayLevel, 2)}, pitch ${fmt(en.meterDelayPitch, 2)}.`),
      ...(en.error ? [note(en.error)] : []));
  }
  if (rt?.medianMs != null && en) {
    const rows: string[][] = [];
    const add = (name: string, eng: number | null, budget: string) => {
      if (eng == null) return;
      const tot = rt.medianMs! + eng;
      rows.push([name, fmt(rt.medianMs), fmt(eng), fmt(tot), budget, tot <= 50 ? 'within 50 ms' : 'over 50 ms']);
    };
    add('Level and timbre', en.levelMedianMs, '20-45');
    add('Pitch', en.pitchMedianMs, '35-60');
    put('total',
      table(['Path', 'Round trip ms', 'Engine ms', 'Total ms', 'TDD budget ms', 'Target (50 ms wired)'], rows),
      note('Total = input device + engine + output device. The round trip already holds both devices, so it is added once.'));
  }
  (document.getElementById('btn-json') as HTMLButtonElement).disabled = !(rt || en);
}

function downloadJson() {
  const body = JSON.stringify({
    when: new Date().toISOString(), userAgent: navigator.userAgent,
    crossOriginIsolated: window.crossOriginIsolated, ...last,
  }, null, 2);
  const a = document.createElement('a');
  a.href = URL.createObjectURL(new Blob([body], { type: 'application/json' }));
  a.download = 'earworm-delay.json';
  a.click();
  URL.revokeObjectURL(a.href);
}

async function guarded<T>(f: () => Promise<T>): Promise<T | undefined> {
  const btns = document.querySelectorAll('button');
  btns.forEach((b) => (b.disabled = true));
  try {
    const r = await f();
    setStatus('Done.');
    return r;
  } catch (e) {
    setStatus(`Error: ${e instanceof Error ? e.message : String(e)}`);
    return undefined;
  } finally {
    btns.forEach((b) => (b.disabled = false));
    render();
  }
}

async function runAll() {
  const roundTrip = await runRoundTrip();
  await sleep(0.3);
  const engine = await runEngineDelay();
  document.body.dataset.done = '1';
  return { roundTrip, engine };
}

declare global {
  interface Window {
    earwormDelay: {
      runRoundTrip: () => Promise<RoundTripResult>;
      runEngineDelay: () => Promise<EngineResult>;
      runAll: () => Promise<{ roundTrip: RoundTripResult; engine: EngineResult }>;
      last: typeof last;
    };
  }
}
window.earwormDelay = { runRoundTrip, runEngineDelay, runAll, get last() { return last; } } as Window['earwormDelay'];

const auto = new URLSearchParams(location.search).get('auto') === '1';
document.getElementById('mode')!.textContent =
  `crossOriginIsolated: ${window.crossOriginIsolated}. Transport: ${wantsFallback() ? 'MessagePort fallback' : 'shared memory'}.` +
  (auto ? ' Auto mode: one click on Run all runs both tests.' : '');
document.getElementById('btn-rt')!.onclick = () => void guarded(runRoundTrip);
document.getElementById('btn-eng')!.onclick = () => void guarded(runEngineDelay);
document.getElementById('btn-all')!.onclick = () => void guarded(runAll);
document.getElementById('btn-json')!.onclick = downloadJson;
