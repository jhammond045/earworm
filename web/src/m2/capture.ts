// M2 capture page: Live follow, hold to capture, second pass on release, loop on the M1 engine,
// and the by-ear gate panel (follow delay, voice + synth playback, keep tally, take export).

import { micConstraints } from '../host/engine-host';
import { SCALES } from '../host/protocol';
import { download, encodeWav } from '../common/wav';
import { readColors } from '../live/ribbon';
import { KlontHost, wantsFallback } from '../m1/klont-host';
import { encodeLayer, type LayerDef } from '../m1/layer';
import { CURVE_STRIDE } from '../m1/track';
import { LAYER_DEPTH_BREATH, LAYER_DEPTH_HARD, LAYER_DEPTH_LOUD, LAYER_DEPTH_MOUTH, LAYER_DEPTH_OPEN, LAYER_FOLLOW, LAYER_LEVEL,
  LIVE_KIT_BRIGHT, LIVE_KIT_LOW, LIVE_KIT_NOISE, LIVE_KIT_TAUGHT, LOAD_CURVE, LOAD_LAYER, LOAD_NAMES, LOAD_PATCH, LOAD_OK, LOAD_TRACK, METER_LEN,
  M_ARMED, M_DELAY_LEVEL, M_DELAY_PITCH, M_FLOOR_DB, M_GATE, M_HARD, M_IN_DB, M_LIVE_HITS, M_LIVE_SLOT, M_LOUD, M_OPEN, M_PITCH,
  M_PLAYING, M_SECTION_POS, M_TUNED, M_VOICING, M_VOWEL, M_BREATH, TUNE_OCTAVE, TUNE_REF_CENTS, TUNE_ROOT, TUNE_SCALE, TUNE_SLIDES, TUNE_SNAP,
  TUNE_SPEED, TUNE_VIBRATO } from '../m1/protocol';
import { drawBands, noteName } from '../live/ribbon';
import { Analysis, type Hit, type SecondPass, type TaughtSlot, type TuneSettings } from './analysis';
import { LiveRibbon, drawTake } from './draw';
import { BEAT_KITS, MELODY_SOUNDS, SLOT_COLORS, SLOT_NAMES } from './sounds';
import { FRAME_RATE, MAX_TAKE_SEC, MIN_TAKE_SEC, buildCurveBlob, buildTrackBlob, loopSeconds, meanLive, measureFollow, noteTargets,
  tallyText, type FollowResult, type PageKind, type TakeScore } from './take';

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const MELODY_LAYER = 0, BEAT_LAYER = 1;
const CURVE_SLOT = 0;
const STORE_KEY = 'earworm-m2-takes';

let host: KlontHost | null = null;
let analysis: Analysis | null = null;
let stream: MediaStream | null = null;
const meters = new Float32Array(METER_LEN);
const ribbon = new LiveRibbon($('ribbon'));

// ---------------------------------------------------------------- state

const state = {
  kind: 'melody' as PageKind,
  melodySound: 0,
  beatKit: 0,
  level: { melody: -6, beat: -6 },
  depths: { loud: 1, open: 0.5, hard: 0.5, breath: 0.5, mouth: 1 },
  follow: 1,
  tune: { snap: 0, speedMs: 60, slides: 0 as 0 | 1 | 2, vibrato: 1, root: 0, scale: 0, refCents: 0, levelOut: 0 } as TuneSettings,
  octave: 0,
};
const layerOf = (k: PageKind) => (k === 'beat' ? BEAT_LAYER : MELODY_LAYER);
const activeLayer = () => layerOf(state.kind);

interface Take {
  id: string;
  kind: PageKind;
  sr: number;
  mic: Float32Array;
  out: Float32Array;
  frames: Float32Array;
  sp: SecondPass;
  tuned: Float32Array;
  delays: FollowResult | null;
  record: TakeRecord;
}
let take: Take | null = null;

interface TakeRecord extends TakeScore {
  id: string;
  kind: PageKind;
  seconds: number;
  detected: string;
  secondPassMs: number;
  notes: number;
  hits: number;
  voicedFraction: number;
  onsetsPerSec: number;
  refCents: number;
  scale: string;
  root: number;
  loopSec: number;
  bpm: number | null;
  levelMs: number | null;
  pitchMs: number | null;
  pitchN: number;
  pitchMissed: number;
  tune: TuneSettings & { octave: number };
  gesture: typeof state.depths & { follow: number };
  sound: string;
  transport: string;
  sampleRate: number;
  baseLatencyMs: number;
  outputLatencyMs: number;
}

let records: TakeRecord[] = loadRecords();
function loadRecords(): TakeRecord[] {
  try { return JSON.parse(localStorage.getItem(STORE_KEY) ?? '[]'); } catch { return []; }
}
function saveRecords(): void {
  try { localStorage.setItem(STORE_KEY, JSON.stringify(records)); } catch { /* storage blocked: the tally lives in memory only */ }
}
function showTally(): void { $('tally').textContent = tallyText(records); }

// ---------------------------------------------------------------- small UI helpers

function fail(msg: string): void {
  const el = $('error');
  el.hidden = false;
  el.textContent = msg;
}
const setStatus = (s: string) => { $('status').textContent = s; };

interface SliderOpts { id: string; label: string; min: number; max: number; step: number; value: number; unit?: string; digits?: number; on: (v: number) => void }
function slider(parent: HTMLElement, o: SliderOpts): HTMLInputElement {
  const wrap = document.createElement('div');
  wrap.className = 'ctl';
  const lab = document.createElement('label');
  lab.htmlFor = o.id;
  lab.textContent = o.label;
  const out = document.createElement('output');
  out.className = 'mono small';
  const input = document.createElement('input');
  input.type = 'range';
  input.id = o.id;
  input.min = String(o.min); input.max = String(o.max); input.step = String(o.step); input.value = String(o.value);
  const show = () => { out.textContent = `${Number(input.value).toFixed(o.digits ?? 2)}${o.unit ?? ''}`; };
  show();
  input.addEventListener('input', () => { show(); o.on(Number(input.value)); });
  wrap.append(lab, out, input);
  parent.append(wrap);
  return input;
}

interface SelectOpts { id: string; label: string; options: string[]; value: number; on: (v: number) => void }
function select(parent: HTMLElement, o: SelectOpts): HTMLSelectElement {
  const wrap = document.createElement('div');
  wrap.className = 'ctl';
  const lab = document.createElement('label');
  lab.htmlFor = o.id;
  lab.textContent = o.label;
  const sel = document.createElement('select');
  sel.id = o.id;
  o.options.forEach((t, i) => sel.append(new Option(t, String(i))));
  sel.value = String(o.value);
  sel.addEventListener('change', () => o.on(Number(sel.value)));
  wrap.append(lab, sel);
  parent.append(wrap);
  return sel;
}

// ---------------------------------------------------------------- layers

function layerDef(kind: PageKind): LayerDef {
  const d = state.depths;
  if (kind === 'beat') {
    const k = BEAT_KITS[state.beatKit];
    return { kind: 'beat', level: state.level.beat, depths: d, follow: state.follow, kit: k.kit, kitCents: k.kitCents, base: k.base };
  }
  return { kind: 'melody', level: state.level.melody, depths: d, base: MELODY_SOUNDS[state.melodySound].sound };
}

async function loadLayer(kind: PageKind): Promise<void> {
  if (!host) return;
  const blobs = encodeLayer(layerDef(kind), layerOf(kind));
  const p = await host.load(LOAD_PATCH, layerOf(kind), blobs.patch);
  if (p !== LOAD_OK) throw new Error(`patch ${layerOf(kind)}: ${LOAD_NAMES[p] ?? p}`);
  const s = await host.load(LOAD_LAYER, layerOf(kind), blobs.layer);
  if (s !== LOAD_OK) throw new Error(`layer ${layerOf(kind)}: ${LOAD_NAMES[s] ?? s}`);
}

function pushLive(): void {
  if (!host) return;
  const t = state.tune;
  for (const l of [MELODY_LAYER, BEAT_LAYER]) {
    host.setTune(l, TUNE_SNAP, t.snap);
    host.setTune(l, TUNE_SPEED, t.speedMs);
    host.setTune(l, TUNE_SLIDES, t.slides);
    host.setTune(l, TUNE_VIBRATO, t.vibrato);
    host.setTune(l, TUNE_ROOT, t.root);
    host.setTune(l, TUNE_SCALE, t.scale);
    host.setTune(l, TUNE_REF_CENTS, t.refCents);
    host.setTune(l, TUNE_OCTAVE, state.octave);
  }
}

// ---------------------------------------------------------------- controls

const tuneInputs: Record<string, HTMLInputElement | HTMLSelectElement> = {};
let retuneTimer = 0;
let retuneSeq = 0;

function onTune(live: () => void, octaveChanged = false): void {
  live();
  window.clearTimeout(retuneTimer);
  retuneTimer = window.setTimeout(() => { void retune(octaveChanged); }, 30);
}

function buildControls(): void {
  const sc = $('soundCtl');
  select(sc, { id: 'melodySound', label: 'Melody sound', options: MELODY_SOUNDS.map((m) => `${m.name} (${m.blurb})`), value: 0,
    on: (v) => { state.melodySound = v; void loadLayer('melody').catch((e) => fail(String(e))); } });
  select(sc, { id: 'beatKit', label: 'Beat kit', options: BEAT_KITS.map((k) => k.name), value: 0,
    on: (v) => { state.beatKit = v; void loadLayer('beat').then(() => reloadTake()).catch((e) => fail(String(e))); } });
  slider(sc, { id: 'level', label: 'Level (dB)', min: -60, max: 12, step: 0.5, value: -6, digits: 1, unit: ' dB',
    on: (v) => { state.level[state.kind] = v; host?.setLayer(activeLayer(), LAYER_LEVEL, v); } });
  slider(sc, { id: 'follow', label: 'Follow (beats: how much hit shape is used)', min: 0, max: 1, step: 0.01, value: 1,
    on: (v) => { state.follow = v; host?.setLayer(BEAT_LAYER, LAYER_FOLLOW, v); } });

  const dc = $('depthCtl');
  const depth = (key: keyof typeof state.depths, label: string, field: number) =>
    slider(dc, { id: `d-${key}`, label, min: 0, max: 1, step: 0.01, value: state.depths[key],
      on: (v) => { state.depths[key] = v; for (const l of [MELODY_LAYER, BEAT_LAYER]) host?.setLayer(l, field, v); } });
  depth('loud', 'Loud (loudness depth)', LAYER_DEPTH_LOUD);
  depth('open', 'Open (openness depth)', LAYER_DEPTH_OPEN);
  depth('hard', 'Hard (hardness depth)', LAYER_DEPTH_HARD);
  depth('breath', 'Breath (breathiness depth)', LAYER_DEPTH_BREATH);
  depth('mouth', 'Mouth (vowel depth)', LAYER_DEPTH_MOUTH);

  const tc = $('tuneCtl');
  const t = state.tune;
  tuneInputs.snap = slider(tc, { id: 't-snap', label: 'Snap', min: 0, max: 1, step: 0.01, value: t.snap,
    on: (v) => onTune(() => { t.snap = v; pushLive(); }) });
  tuneInputs.speed = slider(tc, { id: 't-speed', label: 'Speed (lag of the snap)', min: 0, max: 250, step: 1, value: t.speedMs, digits: 0, unit: ' ms',
    on: (v) => onTune(() => { t.speedMs = v; pushLive(); }) });
  tuneInputs.slides = select(tc, { id: 't-slides', label: 'Slides', options: ['Glide', 'Gliss', 'Jump'], value: t.slides,
    on: (v) => onTune(() => { t.slides = v as 0 | 1 | 2; pushLive(); }) });
  tuneInputs.vibrato = slider(tc, { id: 't-vib', label: 'Vibrato (0 flatten, 1 keep, 2 exaggerate)', min: 0, max: 2, step: 0.01, value: t.vibrato,
    on: (v) => onTune(() => { t.vibrato = v; pushLive(); }) });
  tuneInputs.octave = slider(tc, { id: 't-oct', label: 'Octave', min: -2, max: 2, step: 1, value: 0, digits: 0,
    on: (v) => onTune(() => { state.octave = v; pushLive(); }, true) });
  tuneInputs.levelOut = slider(tc, { id: 't-lvl', label: 'Level out (loudness toward its median)', min: 0, max: 1, step: 0.01, value: t.levelOut,
    on: (v) => onTune(() => { t.levelOut = v; pushLive(); }) });
  tuneInputs.root = select(tc, { id: 't-root', label: 'Root', options: ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'], value: t.root,
    on: (v) => onTune(() => { t.root = v; pushLive(); }) });
  tuneInputs.scale = select(tc, { id: 't-scale', label: 'Scale', options: SCALES, value: t.scale,
    on: (v) => onTune(() => { t.scale = v; pushLive(); }) });
  tuneInputs.ref = slider(tc, { id: 't-ref', label: 'Ref (singer offset from concert pitch)', min: -50, max: 50, step: 1, value: t.refCents, digits: 0, unit: ' cents',
    on: (v) => onTune(() => { t.refCents = v; pushLive(); }) });

  $<HTMLSelectElement>('kind').addEventListener('change', (e) => {
    state.kind = (e.target as HTMLSelectElement).value as PageKind;
    $('kitPanel').hidden = state.kind !== 'beat';
    $<HTMLInputElement>('level').value = String(state.level[state.kind]);
    $('level').dispatchEvent(new Event('input'));
  });

  buildSlots();
}

function setSliderValue(el: HTMLInputElement | HTMLSelectElement, v: number): void {
  el.value = String(v);
  if (el instanceof HTMLInputElement) el.dispatchEvent(new Event('input'));
}

// ---------------------------------------------------------------- capture

interface Capture { teach: number | null; layer: number; f0: number; timer: number }
let cap: Capture | null = null;
const slotHits: Hit[][] = [[], [], [], []];
let taught: (TaughtSlot | null)[] = [null, null, null, null];

function startCapture(teach: number | null): void {
  if (!host || cap || busy) return;
  const layer = teach !== null ? BEAT_LAYER : activeLayer();
  void host.ctx.resume();
  host.stop(); // the old loop would only muddy the take
  const f0 = host.framesTotal();
  host.arm(layer, true);
  cap = { teach, layer, f0, timer: window.setTimeout(() => { void endCapture(); }, MAX_TAKE_SEC * 1000) };
  document.querySelectorAll('.down-target').forEach((b) => b.classList.add('down'));
  setStatus(teach !== null ? `Teaching slot ${teach + 1}: repeat one sound...` : 'Capturing...');
}

let busy = false;

async function endCapture(): Promise<void> {
  const c = cap;
  if (!c || !host) return;
  cap = null;
  window.clearTimeout(c.timer);
  host.arm(c.layer, false);
  const f1 = Math.min(host.framesTotal(), c.f0 + MAX_TAKE_SEC * FRAME_RATE);
  document.querySelectorAll('.down').forEach((b) => b.classList.remove('down'));
  const n = f1 - c.f0;
  if (n < MIN_TAKE_SEC * FRAME_RATE) {
    setStatus(`Ignored: ${(n / FRAME_RATE).toFixed(2)} s is under ${MIN_TAKE_SEC} s.`);
    return;
  }
  busy = true;
  try {
    await processTake(c, n);
  } catch (e) {
    setStatus('');
    fail(e instanceof Error ? e.message : String(e));
  } finally {
    busy = false;
  }
}

async function processTake(c: Capture, n: number): Promise<void> {
  if (!host || !analysis) return;
  const L = host.frameLen();
  const sr = host.sampleRate;
  const mic = host.readMic(c.f0 * L, n * L);
  const out = host.readOut(c.f0 * L, n * L);
  const frames = host.readFrames(c.f0, n);
  const kind: PageKind = c.teach !== null ? 'beat' : state.kind;
  setStatus(`Second pass on ${(n / FRAME_RATE).toFixed(1)} s...`);
  const sp = await analysis.secondPass(mic.slice(), sr, frames.slice(), { kind, taught: kind === 'beat' ? taught : undefined });

  if (c.teach !== null) {
    slotHits[c.teach] = sp.hits.map((h) => ({ ...h, slot: c.teach! }));
    updateKit();
    show(sp, n, null);
    setStatus(`Slot ${c.teach + 1} taught from ${sp.hits.length} hits in ${sp.ms.toFixed(0)} ms.`);
    return;
  }

  // Fill the tune controls from the second pass, then tune the take with them.
  const t = state.tune;
  t.root = sp.root; t.scale = sp.scale; t.refCents = Math.max(-50, Math.min(50, sp.refCents));
  setSliderValue(tuneInputs.root, t.root);
  setSliderValue(tuneInputs.scale, t.scale);
  setSliderValue(tuneInputs.ref, Math.round(t.refCents));
  t.refCents = Math.max(-50, Math.min(50, sp.refCents));
  pushLive();

  const tunedOut = await analysis.tune(frames.slice(), sp.pitch.slice(), t);
  const tuned = tunedOut.tuned;
  const tk: Take = {
    id: new Date().toISOString(), kind, sr, mic, out, frames, sp, tuned, delays: null,
    record: undefined as unknown as TakeRecord,
  };
  take = tk;
  await loadTake();

  // Follow delay, engine side.
  setStatus('Measuring follow delay...');
  await new Promise((r) => setTimeout(r, 0));
  const targets = kind === 'melody' ? sampleTargets(noteTargets(frames, sp.notes, state.octave), 8) : [];
  tk.delays = measureFollow(mic, out, sr, targets);
  tk.record = makeRecord(tk, n);
  show(sp, n, tk);
  showFollow(tk);
  for (const id of ['keep', 'redo', 'save', 'playboth']) $<HTMLButtonElement>(id).disabled = false;
  $<HTMLButtonElement>('keep').dataset.rated = '';
  setStatus(`Take of ${(n / FRAME_RATE).toFixed(1)} s analysed in ${sp.ms.toFixed(0)} ms, looping.`);
}

function sampleTargets<T>(v: T[], max: number): T[] {
  if (v.length <= max) return v;
  return Array.from({ length: max }, (_, i) => v[Math.floor((i * v.length) / max)]);
}

/** Build the curve and track blobs for the current take and play the loop. */
async function loadTake(): Promise<void> {
  if (!host || !take) return;
  const layer = layerOf(take.kind);
  const spec = { kind: take.kind, layer, curveSlot: CURVE_SLOT, sp: take.sp, frameCount: take.frames.length / CURVE_STRIDE, octave: state.octave };
  let s = await host.load(LOAD_CURVE, CURVE_SLOT, buildCurveBlob(take.frames, take.tuned, state.octave));
  if (s !== LOAD_OK) throw new Error(`curve: ${LOAD_NAMES[s] ?? s}`);
  s = await host.load(LOAD_TRACK, 0, buildTrackBlob(spec));
  if (s !== LOAD_OK) throw new Error(`track: ${LOAD_NAMES[s] ?? s}`);
  await host.ctx.resume();
  host.seek(0, 0);
  host.play();
}

/** A Tune setting changed: tune again, reload the curve only (and the track when the octave moved). */
async function retune(octaveChanged: boolean): Promise<void> {
  if (!host || !analysis || !take) return;
  const seq = ++retuneSeq;
  const r = await analysis.tune(take.frames.slice(), take.sp.pitch.slice(), state.tune);
  if (seq !== retuneSeq || !take) return;
  take.tuned = r.tuned;
  const s = await host.load(LOAD_CURVE, CURVE_SLOT, buildCurveBlob(take.frames, take.tuned, state.octave));
  if (s !== LOAD_OK) fail(`curve: ${LOAD_NAMES[s] ?? s}`);
  if (octaveChanged && take.kind === 'melody') {
    const spec = { kind: take.kind, layer: layerOf(take.kind), curveSlot: CURVE_SLOT, sp: take.sp, frameCount: take.frames.length / CURVE_STRIDE, octave: state.octave };
    await host.load(LOAD_TRACK, 0, buildTrackBlob(spec));
  }
}

function reloadTake(): void { if (take) void loadTake().catch((e) => fail(String(e))); }

// ---------------------------------------------------------------- kit

function buildSlots(): void {
  const box = $('slots');
  for (let k = 0; k < 4; k++) {
    const b = document.createElement('button');
    b.textContent = `Teach slot ${k + 1} (${SLOT_NAMES[k]})`;
    b.disabled = true;
    b.dataset.slot = String(k);
    b.style.borderLeft = `6px solid ${SLOT_COLORS[k]}`;
    const end = () => { if (cap && cap.teach === k) { b.classList.remove('down'); void endCapture(); } };
    b.addEventListener('pointerdown', (e) => { b.setPointerCapture(e.pointerId); b.classList.add('down'); startCapture(k); });
    b.addEventListener('pointerup', end);
    b.addEventListener('pointercancel', end);
    box.append(b);
  }
  $('clearKit').addEventListener('click', () => {
    for (let k = 0; k < 4; k++) slotHits[k] = [];
    updateKit();
  });
}

function updateKit(): void {
  if (!analysis || !host) return;
  taught = analysis.teach(slotHits);
  const parts: string[] = [];
  for (let k = 0; k < 4; k++) {
    const m = meanLive(slotHits[k]);
    host.setLiveKit(BEAT_LAYER, k, LIVE_KIT_TAUGHT, m && taught[k] ? 1 : 0);
    if (m) {
      host.setLiveKit(BEAT_LAYER, k, LIVE_KIT_BRIGHT, m[0]);
      host.setLiveKit(BEAT_LAYER, k, LIVE_KIT_LOW, m[1]);
      host.setLiveKit(BEAT_LAYER, k, LIVE_KIT_NOISE, m[2]);
      parts.push(`${k + 1}: ${slotHits[k].length} hits`);
    }
  }
  $('kitStatus').textContent = parts.length ? `Taught slots: ${parts.join(', ')}.` : 'No taught slots. Live hits use brightness thirds.';
}

// ---------------------------------------------------------------- result display

function sec(v: number): string { return v.toFixed(2); }

function show(sp: SecondPass, n: number, tk: Take | null): void {
  const r = $('result');
  r.hidden = false;
  const lines = [
    `second pass ${sp.ms.toFixed(0)} ms for ${(n / FRAME_RATE).toFixed(1)} s of take (budget: 2 s for a 10 s hum)`,
    `detected: ${sp.kind}, voiced ${(sp.voicedFraction * 100).toFixed(0)}%, ${sp.onsetsPerSec.toFixed(1)} onsets/s, ${sp.repaired} pitch frames repaired`,
    `ref ${sp.refCents.toFixed(0)} cents, root ${noteName(sp.root + 60).replace(/[0-9-]+$/, '')}, scale ${SCALES[sp.scale] ?? sp.scale}`,
    `tempo ${sp.bpm ? `${sp.bpm.toFixed(0)} bpm, ${sp.beats} beats` : 'none found'}, loop ${sec(sp.loopSec)} s`,
  ];
  if (sp.notes.length) {
    lines.push(`notes (${sp.notes.length}): t, dur, note, hard`);
    for (const x of sp.notes.slice(0, 60)) lines.push(`  ${sec(x.t)} ${sec(x.dur)} ${noteName(x.cents / 100 + 12)} ${x.hard.toFixed(2)}`);
  }
  if (sp.hits.length) {
    lines.push(`hits (${sp.hits.length}, ${sp.kinds} kinds): t, slot, strength`);
    for (const x of sp.hits.slice(0, 60)) lines.push(`  ${sec(x.t)} slot ${x.slot + 1} ${x.strength.toFixed(2)}`);
  }
  r.textContent = lines.join('\n');
  void tk;
}

function showFollow(tk: Take): void {
  const d = tk.delays;
  const f = (v: number | null | undefined) => (v === null || v === undefined ? 'n/a' : `${v.toFixed(1)} ms`);
  const el = $('follow');
  el.replaceChildren();
  if (!d) { el.textContent = 'No follow measurement.'; return; }
  const b = (t: string) => { const x = document.createElement('b'); x.textContent = t; return x; };
  el.append('engine follow: level ', b(f(d.levelMs)), ', pitch ', b(f(d.pitchMs)));
  if (tk.kind === 'melody') el.append(` (${d.pitchN} notes locked${d.pitchMissed ? `, ${d.pitchMissed} not within 50 cents in 400 ms` : ''})`);
  else el.append(' (pitch is for melody takes)');
  const dev = deviceMs();
  el.append(`. device (Chrome's base + output latency) ${dev.toFixed(1)} ms. `);
  if (d.pitchMs !== null || d.levelMs !== null) {
    const worst = Math.max(d.pitchMs ?? 0, d.levelMs ?? 0);
    el.append(`Engine + device = ${(worst + dev).toFixed(1)} ms before the measured round trip is added.`);
  }
}

function deviceMs(): number {
  const c = host?.ctx;
  return c ? ((c.baseLatency || 0) + ((c as AudioContext).outputLatency || 0)) * 1000 : 0;
}

function makeRecord(tk: Take, n: number): TakeRecord {
  const sp = tk.sp;
  const d = tk.delays;
  const c = host!.ctx;
  return {
    id: tk.id, kind: tk.kind, kept: null, seconds: n / FRAME_RATE, detected: sp.kind, secondPassMs: sp.ms,
    notes: sp.notes.length, hits: sp.hits.length, voicedFraction: sp.voicedFraction, onsetsPerSec: sp.onsetsPerSec,
    refCents: sp.refCents, scale: SCALES[sp.scale] ?? String(sp.scale), root: sp.root, loopSec: sp.loopSec, bpm: sp.bpm,
    levelMs: d?.levelMs ?? null, pitchMs: d?.pitchMs ?? null, pitchN: d?.pitchN ?? 0, pitchMissed: d?.pitchMissed ?? 0,
    tune: { ...state.tune, octave: state.octave }, gesture: { ...state.depths, follow: state.follow },
    sound: tk.kind === 'beat' ? BEAT_KITS[state.beatKit].name : MELODY_SOUNDS[state.melodySound].name,
    transport: host!.mode, sampleRate: tk.sr,
    baseLatencyMs: (c.baseLatency || 0) * 1000, outputLatencyMs: ((c as AudioContext).outputLatency || 0) * 1000,
  };
}

// ---------------------------------------------------------------- gate: playback, tally, export

let playing: AudioBufferSourceNode | null = null;
let loopWasPlaying = false;

function playBoth(): void {
  if (!host || !take) return;
  stopBoth();
  const ctx = host.ctx;
  const len = Math.min(take.mic.length, take.out.length);
  const buf = ctx.createBuffer(2, len, take.sr);
  const mic = take.mic.subarray(0, len), out = take.out.subarray(0, len);
  const peak = (a: Float32Array) => { let p = 0; for (let i = 0; i < a.length; i++) p = Math.max(p, Math.abs(a[i])); return p; };
  const pm = peak(mic), po = peak(out);
  const g = pm > 1e-6 && po > 1e-6 ? po / pm : 1; // bring the quiet mic up to the synth's level
  const mixed = $<HTMLInputElement>('mixed').checked;
  const l = buf.getChannelData(0), r = buf.getChannelData(1);
  for (let i = 0; i < len; i++) {
    if (mixed) { l[i] = r[i] = 0.5 * (mic[i] * g + out[i]); }
    else { l[i] = mic[i] * g; r[i] = out[i]; }
  }
  const src = ctx.createBufferSource();
  src.buffer = buf;
  src.connect(ctx.destination);
  loopWasPlaying = meters[M_PLAYING] > 0.5;
  if (loopWasPlaying) host.stop();
  src.onended = () => { if (playing === src) { playing = null; $<HTMLButtonElement>('stopboth').disabled = true; } };
  playing = src;
  src.start();
  $<HTMLButtonElement>('stopboth').disabled = false;
}

function stopBoth(): void {
  try { playing?.stop(); } catch { /* already stopped */ }
  playing = null;
  $<HTMLButtonElement>('stopboth').disabled = true;
}

function rate(kept: boolean): void {
  if (!take?.record) return;
  take.record.kept = kept;
  if (!records.includes(take.record)) records.push(take.record);
  saveRecords();
  showTally();
}

function downloadJson(): void {
  const data = {
    userAgent: navigator.userAgent, date: new Date().toISOString(), target: '80% of hum takes kept; engine follow 50 ms',
    summary: tallyText(records), takes: records,
  };
  download(new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' }), 'earworm-m2-takes.json');
}

function saveTake(): void {
  if (!take) return;
  const name = $<HTMLInputElement>('saveName').value.trim().replace(/[^\w.-]+/g, '-') || `take-${take.id.replace(/[:.]/g, '-')}`;
  const n = take.frames.length / CURVE_STRIDE;
  const f24 = new Float32Array(n * 24); // the stored frame format: 24 floats per 10 ms
  for (let i = 0; i < n; i++) f24.set(take.frames.subarray(i * CURVE_STRIDE, i * CURVE_STRIDE + 24), i * 24);
  download(encodeWav(take.mic, take.sr, 'float32'), `${name}.wav`);
  download(new Blob([f24], { type: 'application/octet-stream' }), `${name}.f32`);
  const sp = take.sp;
  const json = {
    name, sampleRate: take.sr, kind: take.kind, record: take.record,
    secondPass: { ...sp, pitch: Array.from(sp.pitch, (v) => (Number.isFinite(v) ? Math.round(v * 10) / 10 : null)) },
    tuned: Array.from(take.tuned, (v) => (Number.isFinite(v) ? Math.round(v * 10) / 10 : null)),
    notesCsvHint: 'label file for note_eval: <name>.notes.csv',
  };
  download(new Blob([JSON.stringify(json)], { type: 'application/json' }), `${name}.second-pass.json`);
}

// ---------------------------------------------------------------- live display

const METER_ROWS: [string, (m: Float32Array) => string][] = [
  ['input', (m) => `${m[M_IN_DB].toFixed(1)} dB (floor ${m[M_FLOOR_DB].toFixed(1)})`],
  ['gate', (m) => (m[M_GATE] > 0.5 ? 'open' : 'closed')],
  ['pitch raw', (m) => centsName(m[M_PITCH])],
  ['pitch tuned', (m) => centsName(m[M_TUNED])],
  ['loudness', (m) => m[M_LOUD].toFixed(2)],
  ['openness', (m) => m[M_OPEN].toFixed(2)],
  ['hardness', (m) => m[M_HARD].toFixed(2)],
  ['breath', (m) => m[M_BREATH].toFixed(2)],
  ['voicing', (m) => ['voiced', 'unvoiced', 'silent'][Math.round(m[M_VOICING])] ?? '-'],
  ['armed', (m) => (m[M_ARMED] > 0.5 ? 'yes' : 'no')],
  ['live slot / hits', (m) => `${Math.round(m[M_LIVE_SLOT]) + 1} / ${Math.round(m[M_LIVE_HITS])}`],
  ['engine delay (native)', (m) => `level ${m[M_DELAY_LEVEL].toFixed(1)} ms, pitch ${m[M_DELAY_PITCH].toFixed(1)} ms`],
];
const centsName = (c: number) => (Number.isFinite(c) && c > 0 ? `${noteName(c / 100 + 12)} (${(c % 100 > 50 ? (c % 100) - 100 : c % 100).toFixed(0)} c)` : '-');

function tick(): void {
  requestAnimationFrame(tick);
  if (!host) return;
  const col = readColors();
  const total = host.framesTotal();
  const n = Math.min(total, 800);
  ribbon.draw(n > 0 ? host.readFrames(total - n, n) : new Float32Array(0), col, !!cap);
  const have = host.readMeters(meters);
  if (have) {
    $('meters').innerHTML = METER_ROWS.map(([k, f]) => `<div><span>${k}</span> ${f(meters)}</div>`).join('');
    drawBands($('vowel') as HTMLCanvasElement, meters.subarray(M_VOWEL, M_VOWEL + 16), 16, 24, col);
  }
  const playhead = have && meters[M_PLAYING] > 0.5 ? meters[M_SECTION_POS] : null;
  drawTake($('take') as HTMLCanvasElement, take ? {
    seconds: take.frames.length / CURVE_STRIDE / FRAME_RATE, pitch: take.sp.pitch, tuned: shiftOctave(take.tuned),
    notes: take.sp.notes, hits: take.sp.hits, octave: state.octave, loopSec: loopSeconds(take.sp, take.frames.length / CURVE_STRIDE),
  } : null, playhead, col);
}

let shifted: { src: Float32Array; oct: number; out: Float32Array } | null = null;
function shiftOctave(t: Float32Array): Float32Array {
  if (!shifted || shifted.src !== t || shifted.oct !== state.octave) {
    shifted = { src: t, oct: state.octave, out: t.map((v) => (Number.isFinite(v) ? v + 1200 * state.octave : NaN)) };
  }
  return shifted.out;
}

// ---------------------------------------------------------------- start

$('start').addEventListener('click', async () => {
  const btn = $<HTMLButtonElement>('start');
  btn.disabled = true;
  try {
    const ctx = new AudioContext({ latencyHint: 'interactive' });
    setStatus('Waiting for microphone permission...');
    stream = await navigator.mediaDevices.getUserMedia(micConstraints());
    host = await KlontHost.create(ctx, { input: true, forceFallback: wantsFallback() });
    host.connectMic(stream);
    setStatus('Loading the engine...');
    analysis = await Analysis.create();
    await loadLayer('melody');
    await loadLayer('beat');
    host.setBudget(0);
    pushLive();
    $('transport').textContent = `Transport: ${host.mode === 'shared' ? 'shared (SharedArrayBuffer)' : 'fallback (MessagePort)'}. ${ctx.sampleRate} Hz. ` +
      `baseLatency ${((ctx.baseLatency || 0) * 1000).toFixed(1)} ms, outputLatency ${((ctx.outputLatency || 0) * 1000).toFixed(1)} ms. ` +
      `Use wired headphones: there is no feedback protection.`;
    $('device').textContent = `Device delay as Chrome reports it: ${deviceMs().toFixed(1)} ms (base + output).`;
    $<HTMLSelectElement>('kind').disabled = false;
    $<HTMLButtonElement>('hold').disabled = false;
    $<HTMLButtonElement>('playloop').disabled = false;
    $<HTMLButtonElement>('stoploop').disabled = false;
    document.querySelectorAll<HTMLButtonElement>('#slots button').forEach((b) => { b.disabled = false; });
    setStatus('Ready. Hold Space and sing or beatbox.');
    tick();
  } catch (e) {
    stream?.getTracks().forEach((t) => t.stop());
    stream = null;
    setStatus('Not started.');
    const err = e as DOMException;
    fail(err?.name === 'NotAllowedError' || err?.name === 'NotFoundError'
      ? `No microphone: ${err.message || err.name}. The page needs a mic to start.`
      : e instanceof Error ? e.message : String(e));
    btn.disabled = false;
  }
});

$('hold').classList.add('down-target');
$('hold').addEventListener('pointerdown', (e) => { ($('hold') as HTMLButtonElement).setPointerCapture(e.pointerId); startCapture(null); });
$('hold').addEventListener('pointerup', () => { if (cap && cap.teach === null) void endCapture(); });
$('hold').addEventListener('pointercancel', () => { if (cap && cap.teach === null) void endCapture(); });

const typing = (t: EventTarget | null) => t instanceof HTMLInputElement && (t.type === 'text' || t.type === 'number') || t instanceof HTMLTextAreaElement;
window.addEventListener('keydown', (e) => {
  if (e.code !== 'Space' || typing(e.target) || !host) return;
  e.preventDefault();
  if (!e.repeat) { $('hold').classList.add('down'); startCapture(null); }
});
window.addEventListener('keyup', (e) => {
  if (e.code !== 'Space' || typing(e.target)) return;
  e.preventDefault();
  $('hold').classList.remove('down');
  if (cap && cap.teach === null) void endCapture();
});
window.addEventListener('blur', () => { if (cap) void endCapture(); });

$('playloop').addEventListener('click', async () => { if (!host) return; await host.ctx.resume(); host.seek(0, 0); host.play(); });
$('stoploop').addEventListener('click', () => host?.stop());
$('playboth').addEventListener('click', playBoth);
$('stopboth').addEventListener('click', () => { stopBoth(); if (loopWasPlaying) host?.play(); });
$('keep').addEventListener('click', () => rate(true));
$('redo').addEventListener('click', () => rate(false));
$('dl-json').addEventListener('click', downloadJson);
$('save').addEventListener('click', saveTake);
$('reset-tally').addEventListener('click', () => {
  records = [];
  saveRecords();
  showTally();
});

buildControls();
showTally();
drawTake($('take') as HTMLCanvasElement, null, null, readColors());
window.addEventListener('beforeunload', () => { analysis?.dispose(); stream?.getTracks().forEach((t) => t.stop()); });
