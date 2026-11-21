// Player page for the M1 engine: plays the demo song in the klont worklet,
// shows meters and engine stats, exports a WAV through the render worker.

import { download, encodeWav } from '../common/wav';
import { DEMO_SECONDS, LAYER_NAMES, encodeDemo, layerDefs } from './demo';
import { KlontHost } from './klont-host';
import { OfflineRenderer } from './offline';
import { LAYER_LEVEL, LAYER_MUTE, LOAD_LAYER, LOAD_NAMES, LOAD_PATCH, LOAD_OK, LOAD_TRACK, METER_LEN, M_ACTIVE_PARTIALS, M_ACTIVE_VOICES,
  M_BPM, M_BUDGET, M_LAYER_PEAK, M_LAYER_VOICES, M_LOAD, M_OVERLOADS, M_PEAK_L, M_PEAK_R, M_PLAYING, M_REPEAT, M_SECTION,
  M_SECTION_POS, M_STEALS, MAX_LAYERS } from './protocol';

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const meters = new Float32Array(METER_LEN);
let host: KlontHost | null = null;

const rows: { fill: HTMLElement; voices: HTMLElement; peak: number }[] = [];
for (let l = 0; l < MAX_LAYERS; l++) {
  const row = document.createElement('div');
  row.className = 'layer';
  const name = document.createElement('span');
  name.textContent = LAYER_NAMES[l];
  const mute = document.createElement('label');
  const cb = document.createElement('input');
  cb.type = 'checkbox';
  cb.disabled = true;
  cb.dataset.layer = String(l);
  mute.append(cb, ' mute');
  const slider = document.createElement('input');
  slider.type = 'range';
  slider.min = '-60'; slider.max = '12'; slider.step = '0.5';
  slider.value = String(layerDefs[l].level ?? 0);
  slider.disabled = true;
  slider.setAttribute('aria-label', `${LAYER_NAMES[l]} level`);
  const db = document.createElement('span');
  db.className = 'mono small';
  db.textContent = `${slider.value} dB`;
  const meter = document.createElement('div');
  meter.className = 'meter';
  const fill = document.createElement('div');
  fill.className = 'fill';
  meter.append(fill);
  const voices = document.createElement('span');
  voices.className = 'mono small';
  row.append(name, mute, slider, db, meter, voices);
  $('layers').append(row);
  rows.push({ fill, voices, peak: 0 });
  cb.addEventListener('change', () => host?.setLayer(l, LAYER_MUTE, cb.checked ? 1 : 0));
  slider.addEventListener('input', () => {
    db.textContent = `${slider.value} dB`;
    host?.setLayer(l, LAYER_LEVEL, Number(slider.value));
  });
}

function fail(msg: string): void {
  const el = $('error');
  el.hidden = false;
  el.textContent = msg;
}

$('start').addEventListener('click', async () => {
  const btn = $<HTMLButtonElement>('start');
  btn.disabled = true;
  try {
    const ctx = new AudioContext({ latencyHint: 'interactive' });
    const force = new URLSearchParams(location.search).get('fallback') === '1';
    host = await KlontHost.create(ctx, { forceFallback: force });
    const song = encodeDemo();
    for (let l = 0; l < MAX_LAYERS; l++) {
      const ps = await host.load(LOAD_PATCH, l, song.patches[l]);
      if (ps !== LOAD_OK) throw new Error(`patch ${l}: ${LOAD_NAMES[ps] ?? ps}`);
      const s = await host.load(LOAD_LAYER, l, song.layers[l]);
      if (s !== LOAD_OK) throw new Error(`layer ${l}: ${LOAD_NAMES[s] ?? s}`);
    }
    const s = await host.load(LOAD_TRACK, 0, song.track);
    if (s !== LOAD_OK) throw new Error(`track: ${LOAD_NAMES[s] ?? s}`);
    host.setBudget(0); // automatic
    $('transport').textContent = `Transport: ${host.mode === 'shared' ? 'shared (SharedArrayBuffer)' : 'fallback (MessagePort)'}. ` +
      `${ctx.sampleRate} Hz. CPU timer in the worklet: ${host.perfNow ? 'performance.now()' : 'Date.now() window average'}.`;
    document.querySelectorAll<HTMLInputElement>('#layers input').forEach((i) => { i.disabled = false; });
    for (const id of ['play', 'stop', 'export']) $<HTMLButtonElement>(id).disabled = false;
    tick();
  } catch (e) {
    fail(e instanceof Error ? e.message : String(e));
    btn.disabled = false;
  }
});

$('play').addEventListener('click', async () => {
  if (!host) return;
  await host.ctx.resume();
  host.play();
});
$('stop').addEventListener('click', () => host?.stop());

$('export').addEventListener('click', async () => {
  const btn = $<HTMLButtonElement>('export');
  const status = $('exportStatus');
  btn.disabled = true;
  const r = new OfflineRenderer();
  try {
    status.textContent = 'Rendering...';
    const job = r.render(encodeDemo(), { once: true, maxSeconds: 180 }, (p) => {
      status.textContent = `Rendering ${p.seconds.toFixed(1)} s of about ${DEMO_SECONDS.toFixed(0)} s...`;
    });
    const res = await job.promise;
    status.textContent = `Done: ${(res.frames / res.sampleRate).toFixed(1)} s, ${res.realtime.toFixed(1)}x realtime, peak ${res.peak.toFixed(2)}, stopped on ${res.stopReason}.`;
    download(encodeWav([res.left, res.right], res.sampleRate), 'earworm-demo.wav');
  } catch (e) {
    status.textContent = '';
    fail(e instanceof Error ? e.message : String(e));
  } finally {
    r.terminate();
    btn.disabled = false;
  }
});

function tick(): void {
  requestAnimationFrame(tick);
  if (!host || !host.readMeters(meters)) return;
  const m = meters;
  const beats = m[M_SECTION_POS] / 960;
  $('position').textContent = `${m[M_PLAYING] > 0.5 ? 'Playing' : 'Stopped'}. Section ${m[M_SECTION] + 1} of 2, repeat ${m[M_REPEAT] + 1}, ` +
    `bar ${Math.floor(beats / 4) + 1} beat ${(beats % 4 + 1).toFixed(1)}, ${m[M_BPM].toFixed(1)} bpm`;
  rows.forEach((r, l) => {
    r.peak = Math.max(m[M_LAYER_PEAK + l], r.peak * 0.92);
    r.fill.style.width = `${Math.min(100, r.peak * 100)}%`;
    r.voices.textContent = `${m[M_LAYER_VOICES + l]} voices`;
  });
  const stat = (k: string, v: string) => `<div><span>${k}</span> ${v}</div>`;
  $('stats').innerHTML = [
    stat('CPU load', `${(m[M_LOAD] * 100).toFixed(0)}%`),
    stat('Partial budget', String(m[M_BUDGET])),
    stat('Active partials', String(m[M_ACTIVE_PARTIALS])),
    stat('Active voices', String(m[M_ACTIVE_VOICES])),
    stat('Steals', String(m[M_STEALS])),
    stat('Overloads', String(m[M_OVERLOADS])),
    stat('Peak L / R', `${m[M_PEAK_L].toFixed(2)} / ${m[M_PEAK_R].toFixed(2)}`),
  ].join('');
}
