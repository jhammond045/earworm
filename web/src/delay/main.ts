// Delay measurement page. Two runners (round trip, engine internal delay),
// a totals comparison against the TDD budget, and a JSON download.
import {
  EngineHost, createAudioContext, micConstraints, wantsFallback,
} from '../host/engine-host';
import { M_DELAY_LEVEL, M_DELAY_PITCH, METER_LEN, P } from '../host/protocol';
import { KlontHost } from '../m1/klont-host';
import { MSG_ARM, M_DELAY_LEVEL as K_DELAY_LEVEL, M_DELAY_PITCH as K_DELAY_PITCH, METER_LEN as K_METER_LEN } from '../m1/protocol';
import {
  ENGINE_TEST_PARAMS, KLONT_TEST_SETTINGS, analyseEngineCapture, chirp, crossCorrelationLag, median, roomHum,
  spread, synthHum, type BurstDelay,
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

export type BurstResult = BurstDelay;

export type EngineKind = 'M2 klont' | 'M0';

/** Which engine the page measures: M2 klont unless ?engine=m0. */
export const engineKind = (): EngineKind =>
  new URLSearchParams(location.search).get('engine') === 'm0' ? 'M0' : 'M2 klont';

export interface FollowResult extends EngineResult {
  /** No mic and no device: OfflineAudioContext through the real klont AudioWorklet. */
  offline: true;
  renderMs: number;
}

export interface EngineResult {
  /** Which engine was measured. */
  engine: EngineKind;
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
let last: { roundTrip?: RoundTripResult; engine?: EngineResult; follow?: FollowResult } = {};

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

/** Engine internal delay through the mic and speakers. M2 klont by default, M0 with ?engine=m0. */
export async function runEngineDelay(): Promise<EngineResult> {
  return engineKind() === 'M0' ? runEngineDelayM0() : runEngineDelayM2();
}

function finishEngine(res: EngineResult, mic: Float32Array, out: Float32Array, clarity?: number) {
  const sr = res.sampleRate;
  res.bursts = analyseEngineCapture(mic, out, sr, clarity == null ? {} : { clarity });
  const levels = res.bursts.flatMap((b) => (b.levelDelayMs == null ? [] : [b.levelDelayMs]));
  const pitches = res.bursts.flatMap((b) => (b.pitchDelayMs == null ? [] : [b.pitchDelayMs]));
  res.levelMedianMs = num(median(levels));
  res.pitchMedianMs = num(median(pitches));
  if (res.bursts.length === 0) res.error = 'No hum heard on the mic ring. Turn the speakers up or use a loopback cable.';
  else if (levels.length === 0) res.error = 'Engine output stayed silent. Is Live follow running?';
}

/** Hum into a buffer source on `c`, at `at` seconds. */
function humPlayer(c: BaseAudioContext, samples: Float32Array, at: number): AudioBufferSourceNode {
  const buf = c.createBuffer(1, samples.length, c.sampleRate);
  buf.copyToChannel(samples as Float32Array<ArrayBuffer>, 0);
  const player = c.createBufferSource();
  player.buffer = buf;
  player.start(at);
  return player;
}

async function runEngineDelayM2(): Promise<EngineResult> {
  setStatus('Engine delay (M2 klont): starting engine.');
  const s = await session();
  const sr = s.ctx.sampleRate;
  const host = await KlontHost.create(s.ctx, { input: true, forceFallback: wantsFallback() });
  const res: EngineResult = {
    engine: 'M2 klont', sampleRate: sr, transport: host.mode, bursts: [], levelMedianMs: null,
    pitchMedianMs: null, meterDelayLevel: null, meterDelayPitch: null,
  };
  // The engine runs but is not heard: its node goes to a muted gain.
  const mute = s.ctx.createGain();
  mute.gain.value = 0;
  host.node.disconnect();
  host.node.connect(mute);
  mute.connect(s.ctx.destination);
  const micSrc = host.connectMic(s.stream);
  for (const m of KLONT_TEST_SETTINGS) host.send(m[0], m[1], m[2], m[3], m[4]);
  await sleep(0.3);

  const hum = synthHum(sr);
  const player = humPlayer(s.ctx, hum.samples, s.ctx.currentTime + 0.3);
  player.connect(s.ctx.destination);
  const s0 = host.samplesTotal();
  setStatus('Engine delay (M2 klont): playing test hum.');
  await sleep(0.3 + hum.samples.length / sr + 0.5);
  // In the message fallback the rings lag up to 100 ms.
  await sleep(0.2);
  const s1 = host.samplesTotal();
  const mic = host.readMic(s0, s1 - s0);
  const out = host.readOut(s0, s1 - s0);
  const meters = new Float32Array(K_METER_LEN);
  if (host.readMeters(meters)) {
    res.meterDelayLevel = num(meters[K_DELAY_LEVEL] * 1000);
    res.meterDelayPitch = num(meters[K_DELAY_PITCH] * 1000);
  }
  host.send(MSG_ARM, 0, 0, 0, 0);
  micSrc.disconnect();
  player.disconnect();
  host.disconnect();
  mute.disconnect();

  setStatus('Engine delay (M2 klont): analysing.');
  finishEngine(res, mic, out, M2_CLARITY);
  last.engine = res;
  render();
  return res;
}

/** Pitch lock clarity for the M2 engine's output, see analyseEngineCapture. */
const M2_CLARITY = 0.8;

/** Engine-side follow delay with no mic and no device: a hum through the real
 *  klont AudioWorklet in an OfflineAudioContext, measured against the same hum. */
export async function runFollowNoMic(): Promise<FollowResult> {
  setStatus('Engine follow (no mic): setting up.');
  const sr = 48000;
  const input = roomHum(sr);
  const octx = new OfflineAudioContext(2, input.length + Math.round(0.5 * sr), sr);
  // KlontHost only needs addModule, createNode and destination, which an
  // offline context has. Message transport: no SharedArrayBuffer needed.
  const host = await KlontHost.create(octx as unknown as AudioContext, { input: true, forceFallback: true });
  const res: FollowResult = {
    engine: 'M2 klont', offline: true, renderMs: 0, sampleRate: sr, transport: 'offline AudioWorklet',
    bursts: [], levelMedianMs: null, pitchMedianMs: null, meterDelayLevel: null, meterDelayPitch: null,
  };
  for (const m of KLONT_TEST_SETTINGS) host.send(m[0], m[1], m[2], m[3], m[4]);
  await sleep(0.2); // let the settings reach the worklet before rendering starts
  const player = humPlayer(octx, input, 0);
  player.connect(host.node);
  setStatus('Engine follow (no mic): rendering.');
  const t0 = performance.now();
  const rendered = await octx.startRendering();
  res.renderMs = performance.now() - t0;
  const out = rendered.getChannelData(0);
  await sleep(0.1); // meter messages posted during the render arrive after it
  const meters = new Float32Array(K_METER_LEN);
  if (host.readMeters(meters)) {
    res.meterDelayLevel = num(meters[K_DELAY_LEVEL] * 1000);
    res.meterDelayPitch = num(meters[K_DELAY_PITCH] * 1000);
  }
  host.disconnect();
  finishEngine(res, input, out, M2_CLARITY);
  if (res.error?.startsWith('No hum')) res.error = 'No hum found in the input.';
  last.follow = res;
  render();
  return res;
}

async function runEngineDelayM0(): Promise<EngineResult> {
  setStatus('Engine delay (M0): starting engine.');
  const s = await session();
  const sr = s.ctx.sampleRate;
  const host = await EngineHost.create(s.ctx, { nBands: 16, forceFallback: wantsFallback() });
  const res: EngineResult = {
    engine: 'M0', sampleRate: sr, transport: host.mode, bursts: [], levelMedianMs: null, pitchMedianMs: null,
    meterDelayLevel: null, meterDelayPitch: null,
  };
  // The engine runs but is not heard: its node goes to a muted gain.
  const mute = s.ctx.createGain();
  mute.gain.value = 0;
  host.node.disconnect();
  host.node.connect(mute);
  mute.connect(s.ctx.destination);
  const micSrc = host.connectMic(s.stream);

  for (const [name, v] of ENGINE_TEST_PARAMS) host.setParam(P[name], v);
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
    res.meterDelayLevel = num(meters[M_DELAY_LEVEL] * 1000);
    res.meterDelayPitch = num(meters[M_DELAY_PITCH] * 1000);
  }
  host.setLive(false);
  micSrc.disconnect();
  player.disconnect();
  mute.disconnect();
  host.dispose();

  setStatus('Engine delay (M0): analysing.');
  finishEngine(res, cap.mic, cap.out);
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

function engineNodes(en: EngineResult): Node[] {
  return [
    note(`Engine measured: ${en.engine}${'offline' in en ? ' (offline AudioWorklet, no mic, no device)' : ''}.`),
    table(['Burst', 'Mic onset s', 'Input f0 Hz', `Level delay ms, engine (${en.engine})`, `Pitch delay ms, engine (${en.engine})`],
      en.bursts.map((b, i) => [String(i + 1), fmt(b.micOnsetS, 3), fmt(b.inputF0), fmt(b.levelDelayMs), fmt(b.pitchDelayMs)])
        .concat([['Median', '', '', fmt(en.levelMedianMs), fmt(en.pitchMedianMs)]])),
    note(`Transport: ${en.transport}. Nominal analysis delay from the engine: level ${fmt(en.meterDelayLevel, 1)} ms, pitch ${fmt(en.meterDelayPitch, 1)} ms.`),
    ...(en.error ? [note(en.error)] : []),
  ];
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
  if (en) put('eng', ...engineNodes(en));
  if (last.follow) put('follow', ...engineNodes(last.follow));
  if (rt?.medianMs != null && en) {
    const rows: string[][] = [];
    const add = (name: string, eng: number | null, budget: string) => {
      if (eng == null) return;
      const tot = rt.medianMs! + eng;
      rows.push([`${name}, engine (${en.engine})`, fmt(rt.medianMs), fmt(eng), fmt(tot), budget, tot <= 50 ? 'within 50 ms' : 'over 50 ms']);
    };
    add('Level and timbre', en.levelMedianMs, '20-45');
    add('Pitch', en.pitchMedianMs, '35-60');
    put('total',
      note(`Device (input + output, round trip): ${fmt(rt.medianMs)} ms. Engine side (${en.engine}): level ${fmt(en.levelMedianMs)} ms, pitch ${fmt(en.pitchMedianMs)} ms. Reported separately; the table adds them.`),
      table(['Path', 'Device round trip ms', 'Engine ms', 'Total ms', 'TDD budget ms', 'Target (50 ms wired)'], rows),
      note('Total = input device + engine + output device. The round trip already holds both devices, so it is added once.'));
  }
  (document.getElementById('btn-json') as HTMLButtonElement).disabled = !(rt || en || last.follow);
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
      runFollowNoMic: () => Promise<FollowResult>;
      runAll: () => Promise<{ roundTrip: RoundTripResult; engine: EngineResult }>;
      last: typeof last;
    };
  }
}
window.earwormDelay = { runRoundTrip, runEngineDelay, runFollowNoMic, runAll, get last() { return last; } } as Window['earwormDelay'];

const auto = new URLSearchParams(location.search).get('auto') === '1';
document.getElementById('mode')!.textContent =
  `crossOriginIsolated: ${window.crossOriginIsolated}. Transport: ${wantsFallback() ? 'MessagePort fallback' : 'shared memory'}.` +
  ` Engine for the mic test: ${engineKind()} (add ?engine=m0 for the M0 engine).` +
  (auto ? ' Auto mode: one click on Run all runs both tests.' : '');
document.getElementById('btn-rt')!.onclick = () => void guarded(runRoundTrip);
document.getElementById('btn-eng')!.onclick = () => void guarded(runEngineDelay);
document.getElementById('btn-follow')!.onclick = () => void guarded(runFollowNoMic);
document.getElementById('btn-all')!.onclick = () => void guarded(runAll);
document.getElementById('btn-json')!.onclick = downloadJson;
