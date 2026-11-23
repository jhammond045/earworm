// The Earworm app: one transport, one undo history, and one page from capture to the morph lane.
// Milestone and gate pages stay linked from Tests. This file is the shell; the document still changes only through commands.

import { decodeBytes, decodeFile, loadSources, MAX_SOURCE_S, trimAndCap, type SavedSource } from '../influence/audio';
import { stateVocab, vocabIndices } from '../synth/classic';
import { inputAt, layoutOf } from '../synth/patch';
import { download, encodeWav } from '../common/wav';
import { KlontHost, wantsFallback } from '../m1/klont-host';
import { OfflineRenderer } from '../m1/offline';
import {
  LAYER_DEPTH_BREATH, LAYER_DEPTH_HARD, LAYER_DEPTH_LOUD, LAYER_DEPTH_MOUTH, LAYER_DEPTH_OPEN, LAYER_FOLLOW,
  LAYER_INFLUENCE_AMOUNT, LAYER_INFLUENCE_MASK, LAYER_LEVEL, LAYER_PAN, LIVE_KIT_BRIGHT, LIVE_KIT_LOW, LIVE_KIT_NOISE, LIVE_KIT_TAUGHT,
  M_LAYER_VOICES, M_MACRO_LIVE, M_METRO_BEAT, M_PLAYING, M_SECTION, M_SECTION_POS, SET_INFLUENCE,
  TUNE_OCTAVE, TUNE_REF_CENTS, TUNE_ROOT, TUNE_SCALE, TUNE_SLIDES, TUNE_SNAP, TUNE_SPEED, TUNE_VIBRATO,
} from '../m1/protocol';
import { SCALES } from '../host/protocol';
import { snapChoices, stateColor } from '../m3/geometry';
import { metersOf, playedBar, sectionLabel, writtenAt } from '../project/meter';
import { lockTag, MeterRow } from './meter-ui';
import { Analysis, teach, type Hit as AnalysisHit } from '../m2/analysis';
import { SLOT_NAMES } from '../m2/sounds';
import { meanLive } from '../m2/take';
import { $, App } from '../m3/app';
import { holdButton, MicClip, typingIn } from '../m3/hold';
import { grainFor as grainRegion, layerUsesStates } from '../m3/influence';
import { readoutOf } from '../m3/readout';
import { measureRoundTrip } from '../m3/roundtrip';
import { Session } from '../m3/session';
import { liveMatch, type LiveMatch, type Match } from '../m4/match';
import { commands, newId } from '../project/commands';
import { Compiler, sectionUnitsPerSec } from '../project/compile';
import { ANCHOR_HARD, ANCHOR_LEN, flexAudio, quantizeFlex } from '../project/flex';
import { cleanupPlan, unusedRefs } from '../project/cleanup';
import { exportProject, importProject } from '../project/archive';
import { historyResets } from '../project/migrate';

/** Said once when a project opens upgraded past a schema whose undo steps could not come along. */
const UPGRADED = 'This project was saved by an earlier version and has been upgraded. Its undo history starts fresh.';
import { applyOps } from '../project/patch';
import { ProjectDb, flushOnHide, idbKv, memoryKv, storageEstimate, type Kv } from '../project/persist';
import { MACRO_NAMES, N_MACROS, P } from '../gen/params';
import { MAX_STATES, PPQ, defaultMetro, type GestureDepth, type HitShape, type Influence, type Layer, type Metro, type MorphShape, type Project, type Tune as TuneFields, type Clip, type Flex } from '../project/types';
import { GlTimeline } from './gl-timeline';
import { startLogo } from './logo';
import { AppCapture } from './capture';
import { DEMOS, demoById } from './demos';
import { cellView, playRow, positionLabel, rowCount, rowStart, STEPS_PER_BEAT } from './pattern';
import { TutorialRunner } from './tutorial';
import { LESSONS, type Advance } from './tutorials';
import { KlontGlyph } from './klont-glyph';
import { recordFromFrame } from './live-ribbon';
import { songSeconds, uploadsToSong } from './mix';
import { tuneFromTake } from './tune';
import { factoryPresets, parsePreset, presetFileName, presetFromLayer, presetMatches, presetText, type PresetFile } from '../presets/presets';
import { Ribbon } from '../live/ribbon';
import { keyLabel, SynthPanel } from './synth-ui';
import { PerformPanel } from './perform';
import { placeBubbles } from './help';

const NOTE_NAMES = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];
const SHAPE: HitShape = { attack: 0.004, burst: 0.4, cents: null, bend: 0, hold: 0.05, vowel: 0.4, tone: 0.6, endStop: 0, endClick: 0 };
const ASPECTS: { label: string; bit: number }[] = [
  { label: 'Tone', bit: 1 }, { label: 'Shape', bit: 2 }, { label: 'Motion', bit: 4 }, { label: 'Grit', bit: 8 }, { label: 'Space', bit: 16 },
];

const btn = (id: string) => $<HTMLButtonElement>(id);
const num = (id: string) => $<HTMLInputElement>(id);

/** Pan as a letter plus a number, so the slider is not the only cue. */
function panLabel(pan: number): string {
  if (Math.abs(pan) < 0.05) return 'C';
  return pan < 0 ? `L ${Math.round(-pan * 100)}` : `R ${Math.round(pan * 100)}`;
}

async function main(): Promise<void> {
  let kv: Kv;
  let storageNote = '';
  try { kv = await idbKv(); } catch { kv = memoryKv(); storageNote = 'This browser will not keep the project after the tab closes.'; }
  const db = new ProjectDb(kv);
  const session = new Session(kv, db);
  const app = new App(session);
  // Presets (stage S3): the factory ones, then the user's own from the settings store.
  const factory = factoryPresets();
  let userPresets: PresetFile[] = [];
  try {
    const saved = (await kv.get('settings', 'presets')) as unknown[] | undefined;
    userPresets = (saved ?? []).flatMap((x) => { try { return [parsePreset(x)]; } catch { return []; } });
  } catch { userPresets = []; }
  const keepPresets = async () => {
    try { await kv.put('settings', 'presets', userPresets); } catch (e) { app.fail(`The preset could not be saved: ${e instanceof Error ? e.message : String(e)}`); }
  };
  app.stopFlushOnHide = flushOnHide(db);
  if (storageNote) app.fail(storageNote);
  const tutorial = new TutorialRunner();
  const capture = new AppCapture(app, (s) => { $('capStatus').textContent = s; offerNext(s); }, () => tutorial.event('capture') && paintTour());
  const timeline = new GlTimeline(app, () => acted('timeline'));
  const glyphCanvas = document.getElementById('klont');
  const ribbonCanvas = document.getElementById('ribbon');
  const glyph = glyphCanvas instanceof HTMLCanvasElement ? new KlontGlyph(glyphCanvas) : null;
  const ribbon = ribbonCanvas instanceof HTMLCanvasElement ? new Ribbon(ribbonCanvas, 8, 36) : null;
  const ribbonFrames: Float32Array[] = [];
  /** The ribbon's colors, read from the theme's tokens (style.css). */
  const themeColors = () => {
    const css = getComputedStyle(document.documentElement);
    const v = (k: string, d: string) => css.getPropertyValue(k).trim() || d;
    return { fg: v('--text', '#e7eef6'), muted: v('--dim', '#b4c4d4'), line: v('--line', '#3d5166'), accent: v('--accent', '#f0b429') };
  };
  let ribbonColors = themeColors();
  const reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  let bands: 16 | 24 = 16;
  let liveTarget: ArrayLike<number> | null = null;
  let liveProvisional = false;
  let savedList: SavedSource[] = [];
  let sourceNode: AudioBufferSourceNode | null = null;
  const taughtHits: AnalysisHit[][] = [[], [], [], []];
  startLogo($('logo'), () => capture.holding);
  let cursor = { row: 0, col: 0 };
  let listening: { layerId: string; clip: MicClip; live: LiveMatch } | null = null;

  /** Slider readouts: each paints its slider's value beside it, on input and after every render. */
  const readouts: (() => void)[] = [];
  function readout(input: HTMLInputElement, fmt: (v: number) => string, out: HTMLOutputElement = document.createElement('output')): HTMLOutputElement {
    const paint = () => { out.textContent = fmt(Number(input.value)); };
    input.addEventListener('input', paint);
    readouts.push(paint);
    paint();
    return out;
  }
  const pct = (v: number) => `${Math.round(v)}%`;

  const section = () => app.doc.sections.find((s) => s.id === app.ui.sectionId) ?? app.doc.sections[0];
  const layer = () => app.doc.layers.find((l) => l.id === app.ui.layerId) ?? app.doc.layers[0];

  /** The section the transport is in while playing, else the selected one. */
  function playingSection(doc: Project) {
    const m = app.readMeters();
    return (m && m[M_PLAYING] === 1 ? doc.sections[Math.round(m[M_SECTION])] : undefined) ?? section();
  }
  /** The bar the click last heard: Meter 1's bar as the playing section plays it. */
  let metroBar = -1;
  /** The click: its settings, and the bar it accents as Meter 1 plays `sec` (docs/meter.md
   *  "The click"). The engine counts bars from each section's start; a pattern that changes the
   *  bar's length is sent when its section starts playing. */
  function pushMetro(doc: Project, sec = playingSection(doc)): void {
    const host = app.host;
    if (!host) return;
    const m = doc.metro ?? defaultMetro(doc.layers.find((l) => l.kind === 'beat')?.id ?? '');
    const i = Math.max(0, doc.layers.findIndex((l) => l.id === m.layerId));
    metroBar = playedBar(doc, sec);
    host.setMetro(m.on, m.level, m.voice, i, m.slot, metroBar);
  }

  function acted(kind: Advance): void {
    if (tutorial.event(kind)) paintTour();
  }
  const synth = new SynthPanel(app, () => layer(), (k) => acted(k), {
    patch: $('soundPatch'), mod: $('soundMod'), pick: $<HTMLSelectElement>('modPick'), pickBox: $('modPickBox'),
    views: [...document.querySelectorAll<HTMLButtonElement>('[data-view]')], simple: $('soundSimple'),
  });
  const perform = new PerformPanel(app, $('soundPerform'), {
    layer: () => layer(), listening: (id) => synth.listening(id), toggleListen: (id) => synth.toggleListen(id), kv,
    toggleRecord: () => capture.toggleRecord(), recording: () => capture.recording,
  });
  synth.perform = perform;
  app.rec = capture;
  capture.listeningTo = (id) => synth.listening(id);
  capture.sceneOf = (id) => perform.sceneOf(id);

  function paintTour(): void {
    document.querySelectorAll('[data-tour].tour-on').forEach((e) => e.classList.remove('tour-on'));
    const step = tutorial.step;
    const box = $('tour');
    if (!step) { box.hidden = true; tourDialog?.close(); tourDialog = null; return; }
    box.hidden = false;
    $('tourTitle').textContent = step.title;
    $('tourBody').textContent = step.body;
    btn('tourNext').textContent = step.advance === 'next' ? step.action : 'Skip';
    const target = document.querySelector(`[data-tour="${step.target}"]`);
    target?.classList.add('tour-on');
    const dlg = target?.closest('dialog') ?? null;
    if (tourDialog && tourDialog !== dlg) { tourDialog.close(); tourDialog = null; }
    if (dlg instanceof HTMLDialogElement && !dlg.open) { dlg.show(); tourDialog = dlg; }
    const soundish = step.target === 'influence' || step.target === 'states' || step.target === 'state-name' || step.target === 'macros' || step.target === 'gestures' || step.target === 'kit' || step.target === 'teach' || step.target === 'pan' || step.target === 'tune' || step.target === 'source' || step.target === 'hold-note'
      || ['sound-views', 'patch-view', 'mod-view', 'mod-pick', 'env1', 'route-amount', 'matrix', 'shaper', 'features'].includes(step.target);
    if (['env1', 'route-amount', 'matrix', 'shaper', 'features'].includes(step.target)) {
      const want = step.target === 'env1' ? 'patch' : 'mod';
      if (synth.view !== want) synth.setView(want);
    }
    const panel = step.target === 'layers' || step.target === 'section-edit' || step.target === 'feel' || step.target === 'layer-edit' ? 'layers' : soundish ? 'sound' : step.target === 'pattern' || step.target === 'morph' || step.target === 'timeline' || step.target === 'quantize' || step.target === 'takes' ? 'pattern' : '';
    if (panel && window.matchMedia('(max-width: 959px)').matches) showPanel(panel);
  }

  /** The transport's audio chip: the mic, and the MIDI inputs Perform is connected to. */
  function paintAudioChip(): void {
    const chip = $('audioState');
    const midi = perform.midiNames();
    const text = `${app.stream ? 'Mic on' : 'Mic off'}${midi.length ? ` · MIDI ${midi.join(', ')}` : ''}`;
    if (chip.textContent !== text) chip.textContent = text;
    chip.classList.toggle('on', !!app.stream);
  }

  /** A dialog the tour opened to show a step's control, closed when the tour moves on. */
  let tourDialog: HTMLDialogElement | null = null;

  // ---- places (docs/ui.md "Three places, one transport"). Narrow screens show one panel at a
  // time; from 960 px Track shows the layers, the track and the sound together, and Perform
  // shows the layers and the play surface.
  const PLACES = ['layers', 'pattern', 'sound', 'perform'] as const;
  const wideScreen = window.matchMedia('(min-width: 960px)');
  function showPanel(name: string): void {
    const place = wideScreen.matches && name !== 'perform' ? 'pattern' : name;
    document.body.dataset.place = place;
    for (const id of PLACES) $(`tab-${id}`).setAttribute('aria-selected', String(id === place));
    synth.performShown = place === 'perform';
    perform.active = place === 'perform';
    if (place === 'perform') perform.render();
    timeline.invalidate();
  }
  wideScreen.addEventListener('change', () => showPanel(document.body.dataset.place ?? 'pattern'));

  /** The next step after a take or a pass, offered in the status line (docs/ui.md "How one feature leads to the next"). */
  let nextAction: (() => void) | null = null;
  function setNext(label: string, act: (() => void) | null): void {
    const b = btn('nextStep');
    nextAction = act;
    b.hidden = !act;
    b.textContent = label;
  }
  function offerNext(s: string): void {
    if (/^(Placed|Imported)/.test(s)) {
      const sec = section();
      const l = layer();
      const clip = sec && l ? sec.clips[l.id] : undefined;
      if (sec?.feel === 'grid' && clip && !clip.flex?.on) {
        setNext('Quantize it', () => { showPanel('pattern'); btn('quantize').click(); });
        return;
      }
    }
    if (/^Recorded/.test(s)) {
      setNext('See it on the track', () => { showPanel('pattern'); $('timeline').scrollIntoView({ block: 'nearest' }); });
      return;
    }
    setNext('', null);
  }

  // ---- panels

  function renderLayers(): void {
    const list = $('layerList');
    list.replaceChildren();
    app.doc.layers.forEach((l, i) => {
      const b = document.createElement('button');
      b.type = 'button';
      b.className = 'layer' + (l.id === app.ui.layerId ? ' on' : '');
      b.setAttribute('role', 'option');
      b.setAttribute('aria-selected', String(l.id === app.ui.layerId));
      const sw = document.createElement('span');
      sw.className = 'sw';
      sw.dataset.kind = l.kind;
      sw.style.background = '#' + l.color.toString(16).padStart(6, '0');
      sw.title = l.kind;
      const name = document.createElement('span');
      name.textContent = `${i + 1}  ${l.name}`;
      name.className = 'mono';
      b.append(sw, name);
      if (l.mute) {
        const tag = document.createElement('span');
        tag.className = 'tag';
        tag.textContent = 'mute';
        b.append(tag);
      }
      const lock = lockTag(app.doc, l.id);
      if (lock) {
        const tag = document.createElement('span');
        tag.className = 'tag';
        tag.textContent = lock;
        tag.title = lock === 'M mixed' ? 'Its clips lock to different meters' : `Locked to Meter ${lock.slice(1)}`;
        b.append(tag);
      }
      b.addEventListener('click', () => {
        cursor.col = i;
        app.ui.layerId = l.id;
        app.ui.changed();
      });
      list.append(b);
    });
  }

  function renderPattern(): void {
    const sec = section();
    const table = $('pattern');
    const keepFocus = document.activeElement instanceof HTMLElement && table.contains(document.activeElement);
    if (!sec) { table.replaceChildren(); return; }
    const rows = rowCount(sec);
    if (cursor.row >= rows) cursor.row = rows - 1;
    if (cursor.col >= app.doc.layers.length) cursor.col = Math.max(0, app.doc.layers.length - 1);
    const thead = document.createElement('thead');
    const hr = document.createElement('tr');
    const corner = document.createElement('th');
    corner.textContent = sec.feel === 'grid' ? 'bt' : 's';
    hr.append(corner);
    for (const l of app.doc.layers) {
      const th = document.createElement('th');
      th.textContent = l.name;
      th.scope = 'col';
      hr.append(th);
    }
    thead.append(hr);
    const tb = document.createElement('tbody');
    for (let r = 0; r < rows; r++) {
      const tr = document.createElement('tr');
      tr.dataset.row = String(r);
      if (sec.feel === 'grid' && r % STEPS_PER_BEAT === 0) tr.classList.add('beat');
      const num = document.createElement('td');
      num.className = 'num';
      num.textContent = sec.feel === 'grid' ? `${Math.floor(r / STEPS_PER_BEAT) + 1}.${(r % STEPS_PER_BEAT) + 1}` : (r / STEPS_PER_BEAT).toFixed(2);
      tr.append(num);
      app.doc.layers.forEach((l, c) => {
        const td = document.createElement('td');
        const view = cellView(sec, l, r);
        const cell = document.createElement('button');
        cell.type = 'button';
        cell.className = 'cell' + (view.eventId ? ' has' : '') + (cursor.row === r && cursor.col === c ? ' cursor' : '');
        cell.textContent = view.label;
        cell.title = view.title;
        cell.setAttribute('role', 'gridcell');
        cell.setAttribute('aria-label', `${l.name}, row ${r + 1}, ${view.label}`);
        cell.tabIndex = cursor.row === r && cursor.col === c ? 0 : -1;
        cell.addEventListener('click', () => {
          cursor = { row: r, col: c };
          app.ui.layerId = l.id;
          if (view.eventId) app.ui.select(l.id, [view.eventId]);
          else app.ui.changed();
        });
        cell.addEventListener('dblclick', () => { cursor = { row: r, col: c }; addAtCursor(); });
        td.append(cell);
        tr.append(td);
      });
      tb.append(tr);
    }
    table.replaceChildren(thead, tb);
    table.tabIndex = -1;
    if (keepFocus) {
      const cell = table.querySelector(`tr[data-row="${cursor.row}"] td:nth-child(${cursor.col + 2}) button`);
      if (cell instanceof HTMLElement) cell.focus();
    }
    const morph = sec.clips[app.ui.layerId]?.morph ?? [];
    const box = $('morphList');
    box.replaceChildren();
    morph.forEach((m, i) => {
      const k = Math.max(0, layer()?.states.findIndex((s) => s.id === m.state) ?? 0);
      const name = layer()?.states.find((s) => s.id === m.state)?.name ?? m.state;
      const b = document.createElement('button');
      b.type = 'button';
      // The timeline marks a marker's state with notches as well as its color (docs/ui.md).
      const notch = document.createElement('span');
      notch.className = 'notch';
      notch.style.color = stateColor(k);
      notch.textContent = '▮'.repeat(k + 1);
      notch.setAttribute('aria-hidden', 'true');
      b.append(notch, `${positionLabel(sec, m.t)} ${name} ×`);
      b.title = 'Remove this marker';
      b.setAttribute('aria-label', `Remove the marker at ${positionLabel(sec, m.t)}, to ${name}`);
      b.addEventListener('click', () => app.run(() => app.store.dispatch('removeMorphMarker', { section: sec.id, layer: app.ui.layerId, index: i })));
      box.append(b);
    });
    renderLanePick(sec);
  }

  /** What the automation strip under the selected layer shows: the clip's lanes first, then the
   *  macros, level, pan, wheel, bend and pressure, then every value of the layer's patch. */
  function renderLanePick(sec: NonNullable<ReturnType<typeof section>>): void {
    const l = layer();
    const sel = $<HTMLSelectElement>('laneTarget');
    if (!l) return;
    const clip = sec.clips[l.id];
    const have = (clip?.auto ?? []).map((a) => a.target);
    const fixed = ['macro1', 'macro2', 'macro3', 'macro4', 'macro5', 'macro6', 'macro7', 'macro8', 'layer.level', 'layer.pan', 'wheel', 'bend', 'pressure'];
    const keys = layoutOf(l.patch).keys;
    const sig = [l.id, have.join(','), keys.length].join('|');
    const cur = timeline.laneTarget(l, clip);
    if (sel.dataset.sig !== sig) {
      sel.dataset.sig = sig;
      sel.replaceChildren(new Option('(nothing)', ''));
      const name = (k: string) => { const m = /^macro([1-8])$/.exec(k); return m ? `macro ${MACRO_NAMES[Number(m[1]) - 1]}` : k === 'layer.level' ? 'level' : k === 'layer.pan' ? 'pan' : keyLabel(k); };
      const group = (label: string, list: string[]) => {
        if (!list.length) return;
        const g = document.createElement('optgroup');
        g.label = label;
        for (const k of list) g.append(new Option(name(k), k));
        sel.append(g);
      };
      group('Lanes in this clip', have);
      group('Layer', fixed.filter((k) => !have.includes(k)));
      group('Values', keys.filter((k) => !have.includes(k)));
    }
    sel.value = cur;
    $<HTMLButtonElement>('laneClear').disabled = !have.includes(cur);
  }

  function renderSound(): void {
    const l = layer();
    const preset = $<HTMLSelectElement>('preset');
    const state = $<HTMLSelectElement>('state');
    if (!l) return;
    $('soundLayer').textContent = l.name;
    perform.renderCaptureMode($('captureMode'), l);
    {
      // Factory presets, then the user's, for this kind of layer; "Custom" when the sound matches none.
      const fits = (p: PresetFile) => (p.kind === 'beat') === (l.kind === 'beat');
      const st = app.ui.editState(l);
      const groups: [string, string, PresetFile[]][] = [['Factory', 'f', factory], ['Yours', 'u', userPresets]];
      let current = '';
      for (const [, tag, list] of groups) list.forEach((p, i) => { if (!current && fits(p) && presetMatches(l, st, p)) current = `${tag}${i}`; });
      const sig = `${l.kind}|${userPresets.map((p) => p.name).join('/')}`;
      if (preset.dataset.sig !== sig) {
        preset.dataset.sig = sig;
        const custom = document.createElement('option');
        custom.value = '';
        custom.textContent = 'Custom';
        preset.replaceChildren(custom, ...groups.filter(([, , list]) => list.some(fits)).map(([name, tag, list]) => {
          const g = document.createElement('optgroup');
          g.label = name;
          list.forEach((p, i) => {
            if (!fits(p)) return;
            const o = document.createElement('option');
            o.value = `${tag}${i}`;
            o.textContent = p.name;
            g.append(o);
          });
          return g;
        }));
      }
      if (document.activeElement !== preset) preset.value = current;
    }
    if (state.options.length !== l.states.length || [...state.options].some((o, i) => o.value !== l.states[i].id)) {
      const cur = app.ui.editState(l).id;
      state.replaceChildren();
      for (const s of l.states) {
        const o = document.createElement('option');
        o.value = s.id;
        o.textContent = s.name;
        state.append(o);
      }
      state.value = cur;
    }
    num('level').value = String(l.level);
    if (document.activeElement !== num('pan')) {
      num('pan').value = String(Math.round(l.pan * 100));
      $('panRead').textContent = panLabel(l.pan);
    }
    btn('mute').setAttribute('aria-pressed', String(l.mute));
    btn('mute').classList.toggle('down', l.mute);
    btn('mute').textContent = l.mute ? 'Muted' : 'Mute';
    const statesFull = l.states.length >= MAX_STATES;
    btn('addState').disabled = statesFull;
    btn('copyState').disabled = statesFull;
    btn('removeState').disabled = l.states.length < 2;
    const st = app.ui.editState(l);
    if (document.activeElement !== num('amount')) num('amount').value = String(Math.round(st.influenceAmount * 100));
    const infl = l.influence;
    $('infStatus').textContent = infl
      ? `${infl.sourceName}. ${Math.round(st.influenceAmount * 100)}% toward it.`
      : 'No Influence yet.';
    for (const a of ASPECTS) {
      const box = document.getElementById(`aspect-${a.bit}`) as HTMLInputElement | null;
      if (box) box.checked = !infl || (infl.mask & a.bit) !== 0;
    }
    num('useSound').checked = !!infl?.useSound;
    num('useSound').disabled = !infl;
    $('kitBox').hidden = l.kind !== 'beat';
    $('tuneBox').hidden = l.kind === 'beat';
    if (document.activeElement !== num('layerName')) num('layerName').value = l.name;
    btn('removeLayer').disabled = app.doc.layers.length < 2;
    const tune = l.tune;
    const tuneSnap = document.getElementById('tuneSnap');
    const tuneSpeed = document.getElementById('tuneSpeed');
    const tuneVibrato = document.getElementById('tuneVibrato');
    const tuneLevel = document.getElementById('tuneLevel');
    if (tuneSnap instanceof HTMLInputElement && document.activeElement !== tuneSnap) tuneSnap.value = String(Math.round(tune.snap * 100));
    if (tuneSpeed instanceof HTMLInputElement && document.activeElement !== tuneSpeed) tuneSpeed.value = String(Math.round(tune.speedMs));
    if (tuneVibrato instanceof HTMLInputElement && document.activeElement !== tuneVibrato) tuneVibrato.value = String(Math.round(tune.vibrato * 100));
    if (tuneLevel instanceof HTMLInputElement && document.activeElement !== tuneLevel) tuneLevel.value = String(Math.round(tune.levelOut * 100));
    const slides = $<HTMLSelectElement>('tuneSlides');
    const root = $<HTMLSelectElement>('tuneRoot');
    const scale = $<HTMLSelectElement>('tuneScale');
    if (document.activeElement !== slides) slides.value = String(tune.slides);
    if (document.activeElement !== root) root.value = String(tune.root);
    if (document.activeElement !== scale) scale.value = String(Math.min(scale.options.length - 1, tune.scale));
    if (document.activeElement !== num('tuneOctave')) num('tuneOctave').value = String(tune.octave);
    if (document.activeElement !== num('stateName')) num('stateName').value = st.name;
    paintMeters(liveTarget ?? infl?.target ?? null, stateVocab(l, st), liveTarget ? liveProvisional : false);
    fillTakes();
    for (let k = 0; k < N_MACROS; k++) {
      const el = document.getElementById(`macro-${k}`);
      if (el instanceof HTMLInputElement && document.activeElement !== el) el.value = String(Math.round((st.macros[k] ?? 0.5) * 100));
    }
    const depths: (keyof GestureDepth)[] = ['loud', 'open', 'hard', 'breath', 'mouth'];
    for (const key of depths) {
      const el = document.getElementById(`gesture-${key}`);
      if (el instanceof HTMLInputElement && document.activeElement !== el) el.value = String(Math.round(l.depth[key] * 100));
    }
    const follow = document.getElementById('gesture-follow');
    if (follow instanceof HTMLInputElement) {
      follow.parentElement!.hidden = l.kind !== 'beat';
      if (document.activeElement !== follow) follow.value = String(Math.round(l.follow * 100));
    }
    for (const paint of readouts) paint();
    synth.render();
  }

  function renderChrome(): void {
    const doc = app.doc;
    const sec = section();
    $('projectName').textContent = doc.name || 'Untitled';
    $('secName').textContent = sec ? sectionLabel(doc, sec) : '—';
    if (document.activeElement !== num('bpm') && sec?.feel === 'grid') num('bpm').value = String(sec.bpm);
    num('bpm').disabled = !sec || sec.feel !== 'grid';
    if (sec && document.activeElement !== num('secRename')) num('secRename').value = sec.name;
    const beatsLabel = $('beatsLabel');
    const secondsLabel = $('secondsLabel');
    beatsLabel.hidden = !sec || sec.feel !== 'grid';
    secondsLabel.hidden = !sec || sec.feel !== 'free';
    if (sec && document.activeElement !== num('beats')) num('beats').value = String(sec.beats);
    if (sec && document.activeElement !== num('seconds')) num('seconds').value = String(sec.seconds);
    if (sec && document.activeElement !== num('repeats')) num('repeats').value = String(sec.repeats);
    const feel = $<HTMLSelectElement>('feel');
    if (sec && document.activeElement !== feel) feel.value = sec.feel;
    if (sec && document.activeElement !== num('ramp')) num('ramp').value = String(sec.rampMs);
    const si = sec ? doc.sections.findIndex((s) => s.id === sec.id) : -1;
    btn('secEarlier').disabled = si <= 0;
    btn('secLater').disabled = si < 0 || si >= doc.sections.length - 1;
    btn('removeSection').disabled = doc.sections.length < 2;
    btn('loop').setAttribute('aria-pressed', String(doc.loop));
    btn('loop').classList.toggle('down', doc.loop);
    btn('loop').textContent = doc.loop ? 'Loop on' : 'Loop';
    const zoom = $<HTMLSelectElement>('zoom');
    if (document.activeElement !== zoom) zoom.value = String(app.ui.zoom);
    fillSnap();
    fillFlex();
    meterRow.render();
    const m = doc.metro ?? defaultMetro();
    btn('metro').setAttribute('aria-pressed', String(m.on));
    btn('metro').classList.toggle('down', m.on);
    btn('metro').textContent = m.on ? 'Metro on' : 'Metro';
    if (document.activeElement !== num('metroLevel')) num('metroLevel').value = String(Math.round(m.level * 100));
    if (document.activeElement !== num('metroBar')) num('metroBar').value = String(m.bar);
    btn('metroVoice').textContent = m.voice === 'kit' ? 'Kit' : 'Tick';
    btn('undo').disabled = !app.store.history.canUndo;
    btn('redo').disabled = !app.store.history.canRedo;
    btn('capture').disabled = !app.host || !app.stream;
    btn('start').hidden = !!app.host;
    btn('startMic').hidden = !!app.host;
    const chip = $('audioState');
    chip.hidden = !app.host;
    paintAudioChip();
    $('intoLayer').textContent = layer()?.name ?? '—';
    btn('record').disabled = !app.host;
    num('importTake').disabled = !app.host;
    btn('play').disabled = !app.host;
    btn('stop').disabled = !app.host;
    num('projNameInput').value = doc.name;
  }

  function renderScopes(): void {
    const box = $('scopes');
    if (box.childElementCount === app.doc.layers.length) {
      [...box.children].forEach((el, i) => {
        const name = el.querySelector('.name');
        if (name) name.textContent = app.doc.layers[i].name;
      });
      return;
    }
    box.replaceChildren();
    for (const l of app.doc.layers) {
      const d = document.createElement('div');
      d.className = 'scope';
      d.title = 'Voices sounding on this layer';
      d.setAttribute('role', 'meter');
      d.setAttribute('aria-label', `${l.name} voices sounding`);
      d.setAttribute('aria-valuemin', '0');
      d.setAttribute('aria-valuemax', '8');
      d.setAttribute('aria-valuenow', '0');
      const name = document.createElement('div');
      name.className = 'name';
      name.textContent = l.name;
      const voices = document.createElement('span');
      voices.className = 'voices';
      voices.textContent = '0';
      const bar = document.createElement('div');
      bar.className = 'bar';
      const span = document.createElement('span');
      bar.append(span);
      d.append(name, voices, bar);
      box.append(d);
    }
  }

  function paintMeters(target: ArrayLike<number> | null, base: ArrayLike<number>, provisional: boolean): void {
    const box = $('infMeters');
    const note = $('provisional');
    if (!target) { box.replaceChildren(); note.hidden = true; return; }
    const view = readoutOf(target, base);
    note.hidden = !provisional;
    if (box.childElementCount !== view.groups.length) {
      box.replaceChildren();
      for (const g of view.groups) {
        const row = document.createElement('div');
        row.className = 'meter';
        const name = document.createElement('span');
        name.className = 'name';
        name.textContent = g.label;
        const track = document.createElement('span');
        track.className = 'track';
        const fill = document.createElement('span');
        fill.className = 'fill';
        track.append(fill);
        const numEl = document.createElement('span');
        numEl.className = 'num';
        row.append(name, track, numEl);
        box.append(row);
      }
    }
    view.groups.forEach((g, i) => {
      const row = box.children[i];
      const fill = row.querySelector('.fill');
      const numEl = row.querySelector('.num');
      if (numEl) numEl.textContent = `${g.delta >= 0 ? '+' : ''}${g.delta.toFixed(2)}`;
      if (fill instanceof HTMLElement) {
        const w = Math.min(50, Math.abs(g.delta) * 50);
        fill.style.width = `${w}%`;
        fill.style.left = g.delta >= 0 ? '50%' : `${50 - w}%`;
      }
    });
  }

  function fillTakes(): void {
    const sec = section();
    const l = layer();
    const clip = sec && l ? sec.clips[l.id] : undefined;
    const sel = $<HTMLSelectElement>('takeStack');
    const takes = clip?.takes ?? [];
    const sig = takes.join(',');
    if (sel.dataset.takes !== sig) {
      sel.dataset.takes = sig;
      sel.replaceChildren(...(takes.length
        ? takes.map((id, i) => new Option(`${i + 1}`, id))
        : [new Option('None', '')]));
    }
    sel.disabled = takes.length === 0;
    if (clip?.take && [...sel.options].some((o) => o.value === clip.take) && document.activeElement !== sel) sel.value = clip.take;
  }

  function fillSnap(): void {
    const sec = section();
    const sel = $<HTMLSelectElement>('snap');
    const bar = metersOf(app.doc)[0].bars.beats;
    const choices = snapChoices(sec?.feel ?? 'grid', bar);
    const clip = sec?.clips[layer()?.id ?? ''];
    const cur = clip ? clip.timeSnap : Number(sel.value || 0);
    if (sel.dataset.feel !== `${sec?.feel ?? 'grid'}:${bar}`) {
      sel.dataset.feel = `${sec?.feel ?? 'grid'}:${bar}`;
      sel.replaceChildren(...choices.map((c) => new Option(c.label, String(c.value))));
    }
    const match = choices.find((c) => c.value === cur) ?? choices.find((c) => c.value === 0) ?? choices[0];
    if (document.activeElement !== sel && match) sel.value = String(match.value);
  }

  /** Feel grids: a beat down to an eighth of a beat (quarter notes to 32nds in 4/4). */
  const FLEX_GRIDS = [{ label: 'Beat', value: PPQ }, { label: '1/2 beat', value: PPQ / 2 }, { label: '1/4 beat', value: PPQ / 4 }, { label: '1/8 beat', value: PPQ / 8 }];

  /** The Feel row (docs/quant.md "Interface sketch"): the selected layer's clip in this section. */
  function fillFlex(): void {
    const sec = section();
    const l = layer();
    const clip = sec && l ? sec.clips[l.id] : undefined;
    const f = clip?.flex;
    const grid = sec?.feel === 'grid';
    const ups = sec ? sectionUnitsPerSec(sec) : 1;
    const on = num('flexOn');
    on.disabled = !clip || !grid;
    if (document.activeElement !== on) on.checked = !!f?.on;
    const sel = $<HTMLSelectElement>('flexGrid');
    const value = f?.grid ?? PPQ / 4;
    const choices = FLEX_GRIDS.some((c) => c.value === value) ? FLEX_GRIDS : [...FLEX_GRIDS, { label: `${+(value / PPQ).toFixed(3)} beat`, value }];
    const sig = choices.map((c) => c.value).join(',');
    if (sel.dataset.choices !== sig) {
      sel.dataset.choices = sig;
      sel.replaceChildren(...choices.map((c) => new Option(c.label, String(c.value))));
    }
    if (document.activeElement !== sel) sel.value = String(value);
    const editing = !!f && grid;
    for (const id of ['flexGrid', 'flexTriplets', 'flexFeel', 'flexSwing', 'flexStrength', 'flexCatch', 'flexAudio']) num(id).disabled = !editing;
    const audio = $<HTMLSelectElement>('flexAudio');
    if (document.activeElement !== audio) audio.value = f && l ? flexAudio(f, l.kind) : l?.kind === 'beat' ? 'hits' : 'smooth';
    if (document.activeElement !== num('flexTriplets')) num('flexTriplets').checked = !!f?.triplets;
    const feel = $<HTMLSelectElement>('flexFeel');
    if (document.activeElement !== feel) feel.value = f?.feel.kind ?? 'straight';
    $('flexSwingWrap').hidden = f?.feel.kind !== 'swing';
    if (document.activeElement !== num('flexSwing')) num('flexSwing').value = String(Math.round((f?.feel.kind === 'swing' ? f.feel.amount : 0.58) * 100));
    const strength = Math.round((f?.strength ?? 1) * 100);
    if (document.activeElement !== num('flexStrength')) num('flexStrength').value = String(strength);
    $('flexStrengthOut').textContent = `${strength}%`;
    if (document.activeElement !== num('flexCatch')) num('flexCatch').value = String(Math.round(((f?.catch ?? value / 2) / ups) * 1000));
    const kit = $('flexKit');
    kit.hidden = l?.kind !== 'beat';
    kit.querySelectorAll('input').forEach((el, k) => {
      el.disabled = !editing;
      if (document.activeElement !== el) el.value = String(Math.round((f?.slotStrength?.[k] ?? 1) * 100));
    });
    $('flexNote').textContent = clip ? flexNote(clip, l!, grid) : '';
    const gridName = choices.find((c) => c.value === value)?.label ?? '';
    $('flexSummary').textContent = !clip ? 'no clip on this layer in this section'
      : !grid ? 'a free section has no grid'
      : f?.on ? `on · ${gridName}${f.triplets ? ' triplets' : ''} · ${strength}%${f.feel.kind === 'swing' ? ` · swing ${Math.round(f.feel.amount * 100)}%` : ''} · ${audio.value === 'hits' ? 'hits' : 'smooth'}`
      : 'off: the clip plays as performed';
  }

  /** What the Feel row says about the clip: what its anchors are, and what Q1 cannot move yet. */
  function flexNote(clip: Clip, l: Layer, grid: boolean): string {
    if (!grid) return 'Free sections have no grid to quantize to.';
    const take = clip.take ? app.doc.takes[clip.take] : undefined;
    const stored = take ? session.anchorsFor(take) : null;
    const placed = clip.kind === 'beat' ? !!clip.warp?.length : clip.warp.length > 0;
    let what: string;
    if (stored && placed) {
      const n = stored.length / ANCHOR_LEN;
      let soft = 0;
      for (let i = 2; i < stored.length; i += ANCHOR_LEN) if ((stored[i] & ANCHOR_HARD) === 0) soft++;
      what = `Anchors: ${n} from the take${soft ? ` (${soft} soft)` : ''}.`;
    } else {
      what = `Anchors: the ${clip.kind === 'beat' ? 'hit' : 'note'} starts.`;
    }
    const raw = inputAt(l.patch) >= 0 && clip.take
      ? placed ? ' The raw input is stretched with the anchors.' : ' The raw input plays at the timing it was performed: the take has no placement to stretch it by.'
      : '';
    return clip.flex?.on ? `${what}${raw}` : 'Off: the clip plays as performed.';
  }

  /** Change the selected clip's Feel. */
  function setFlex(change: (f: Flex) => Flex): void {
    const sec = section();
    const l = layer();
    const clip = sec && l ? sec.clips[l.id] : undefined;
    if (!sec || !l || !clip) return;
    const grid = clip.timeSnap > 0 ? clip.timeSnap : PPQ / 4;
    const f = change(clip.flex ?? { ...quantizeFlex(undefined, grid), on: false });
    if (app.run(() => app.store.dispatch('setFlex', { section: sec.id, layer: l.id, flex: f }))) acted('quantize');
  }

  num('flexOn').addEventListener('change', () => setFlex((f) => ({ ...f, on: num('flexOn').checked })));
  $<HTMLSelectElement>('flexGrid').addEventListener('change', () => {
    const g = Number($<HTMLSelectElement>('flexGrid').value);
    setFlex((f) => ({ ...f, grid: g, catch: f.catch === f.grid / 2 ? g / 2 : f.catch, feel: f.feel.kind === 'swing' ? { ...f.feel, unit: g } : f.feel }));
  });
  num('flexTriplets').addEventListener('change', () => setFlex((f) => ({ ...f, triplets: num('flexTriplets').checked })));
  $<HTMLSelectElement>('flexAudio').addEventListener('change', () => setFlex((f) => ({ ...f, audio: $<HTMLSelectElement>('flexAudio').value === 'hits' ? 'hits' : 'smooth' })));
  $<HTMLSelectElement>('flexFeel').addEventListener('change', () => {
    const swing = $<HTMLSelectElement>('flexFeel').value === 'swing';
    setFlex((f) => ({ ...f, feel: swing ? { kind: 'swing', amount: Number(num('flexSwing').value) / 100 || 0.58, unit: f.grid } : { kind: 'straight' } }));
  });
  num('flexSwing').addEventListener('change', () => {
    const amount = Math.min(0.75, Math.max(0.5, Number(num('flexSwing').value) / 100 || 0.5));
    setFlex((f) => (f.feel.kind === 'swing' ? { ...f, feel: { ...f.feel, amount } } : f));
  });
  num('flexStrength').addEventListener('input', () => { $('flexStrengthOut').textContent = `${num('flexStrength').value}%`; });
  num('flexStrength').addEventListener('change', () => setFlex((f) => ({ ...f, strength: Number(num('flexStrength').value) / 100 })));
  num('flexCatch').addEventListener('change', () => {
    const sec = section();
    const ms = Math.max(0, Number(num('flexCatch').value) || 0);
    if (sec) setFlex((f) => ({ ...f, catch: (ms / 1000) * sectionUnitsPerSec(sec) }));
  });
  $('flexKit').querySelectorAll('input').forEach((el, k) => el.addEventListener('change', () => {
    const v = Math.min(1, Math.max(0, Number(el.value) / 100 || 0));
    setFlex((f) => {
      const s = [...(f.slotStrength ?? [1, 1, 1, 1])];
      s[k] = v;
      return { ...f, slotStrength: s };
    });
  }));

  const meterRow = new MeterRow(app, () => section(), () => layer(), () => {});

  function pushTune(l: Layer, patch: Partial<TuneFields>): void {
    const host = app.host;
    const i = app.doc.layers.findIndex((x) => x.id === l.id);
    if (!host || i < 0) return;
    const t = { ...l.tune, ...patch };
    host.setTune(i, TUNE_SNAP, t.snap);
    host.setTune(i, TUNE_SPEED, t.speedMs);
    host.setTune(i, TUNE_SLIDES, t.slides);
    host.setTune(i, TUNE_VIBRATO, t.vibrato);
    host.setTune(i, TUNE_ROOT, t.root);
    host.setTune(i, TUNE_SCALE, t.scale);
    host.setTune(i, TUNE_REF_CENTS, t.refCents);
    host.setTune(i, TUNE_OCTAVE, t.octave);
  }

  app.refreshAll = () => {
    app.ui.fit(app.doc);
    renderLayers();
    renderPattern();
    renderSound();
    renderChrome();
    renderScopes();
    timeline.invalidate();
  };
  app.ui.subscribe(() => app.refreshAll());

  let opened = false;
  session.onChange((doc, kind) => {
    if (kind === 'swap') {
      // Nothing on screen belongs to the project that was open: its message and its last capture
      // go. The first project of the page keeps the page's opening message.
      if (opened) $('capStatus').textContent = '';
      opened = true;
      if (ribbonCanvas instanceof HTMLCanvasElement) {
        ribbonCanvas.getContext('2d')?.clearRect(0, 0, ribbonCanvas.width, ribbonCanvas.height);
        ribbonCanvas.parentElement!.classList.remove('drawn');
      }
      app.ui.selection = { layerId: '', ids: [] };
      app.ui.marker = null;
      app.ui.sectionId = '';
      app.ui.layerId = '';
      cursor = { row: 0, col: 0 };
      if (app.host) {
        app.host.stop();
        void app.sync.request(doc).then(() => { pushMetro(doc); app.host?.seek(0, 0); });
      }
    } else {
      app.lastRequest = app.sync.request(app.rec.overlay(doc));
      pushMetro(doc);
    }
    app.refreshAll();
  });
  app.sync.framesFor = (take) => session.framesFor(take);
  app.sync.grainFor = (l) => session.grainFor(l);
  app.sync.audioFor = (t) => session.audioFor(t);
  app.sync.anchorsFor = (t) => session.anchorsFor(t);
  app.sync.noteCurvesFor = (t) => session.noteCurvesFor(t);
  app.sync.onPass = (r) => {
    if (r.errors.length) app.fail(`Engine: ${r.errors.join('; ')}`);
    else if (!r.skipped) $('engineStatus').textContent = `Engine in sync. ${r.uploads.length} blobs, ${r.loadMs.toFixed(0)} ms.`;
    timeline.invalidate();
  };

  // ---- pattern editing

  function addAtCursor(): void {
    const sec = section();
    const l = app.doc.layers[cursor.col];
    if (!sec || !l) return;
    app.ui.layerId = l.id;
    const view = cellView(sec, l, cursor.row);
    if (view.eventId) { app.ui.select(l.id, [view.eventId]); return; }
    const t = rowStart(sec, cursor.row);
    const step = sec.feel === 'grid' ? PPQ / STEPS_PER_BEAT : 0.25;
    if (l.kind === 'beat') {
      const id = newId('h');
      if (app.run(() => app.store.dispatch('addHit', { section: sec.id, layer: l.id, hit: { id, t, dur: 200, slot: 0, strength: 0.8, shape: SHAPE } }))) {
        app.ui.select(l.id, [id]);
        acted('hit');
      }
    } else {
      const id = newId('n');
      if (app.run(() => app.store.dispatch('addNote', { section: sec.id, layer: l.id, note: { id, t, dur: step, cents: 4800, hard: 0.35 } }))) {
        app.ui.select(l.id, [id]);
        acted('note');
      }
    }
  }

  function deleteAtCursor(): void {
    const sec = section();
    const l = app.doc.layers[cursor.col];
    if (!sec || !l) return;
    const ids = app.ui.selection.layerId === l.id && app.ui.selection.ids.length ? app.ui.selection.ids : [];
    const view = cellView(sec, l, cursor.row);
    const use = ids.length ? ids : view.eventId ? [view.eventId] : [];
    if (!use.length) return;
    if (l.kind === 'beat') app.run(() => app.store.dispatch('deleteHits', { section: sec.id, layer: l.id, ids: use }));
    else app.run(() => app.store.dispatch('deleteNotes', { section: sec.id, layer: l.id, ids: use }));
  }

  $('pattern').addEventListener('keydown', (e) => {
    if (typingIn(e.target) && e.target !== $('pattern')) return;
    const rows = rowCount(section() ?? app.doc.sections[0]);
    const cols = app.doc.layers.length;
    if (e.key === 'ArrowUp' || e.key === 'ArrowDown' || e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
      if (e.shiftKey && (e.key === 'ArrowUp' || e.key === 'ArrowDown')) {
        const sec = section();
        const l = app.doc.layers[cursor.col];
        const ids = app.ui.selection.ids;
        if (sec && l && l.kind !== 'beat' && ids.length) {
          e.preventDefault();
          app.run(() => app.store.dispatch('moveNotes', { section: sec.id, layer: l.id, ids, dt: 0, dcents: e.key === 'ArrowUp' ? 100 : -100 }));
        }
        return;
      }
      e.preventDefault();
      if (e.key === 'ArrowUp') cursor.row = (cursor.row + rows - 1) % rows;
      if (e.key === 'ArrowDown') cursor.row = (cursor.row + 1) % rows;
      if (e.key === 'ArrowLeft') cursor.col = (cursor.col + cols - 1) % cols;
      if (e.key === 'ArrowRight') cursor.col = (cursor.col + 1) % cols;
      app.ui.layerId = app.doc.layers[cursor.col]?.id ?? app.ui.layerId;
      app.ui.changed();
      return;
    }
    if (e.key === 'Enter') { e.preventDefault(); addAtCursor(); }
    if (e.key === 'Delete' || e.key === 'Backspace') { e.preventDefault(); deleteAtCursor(); }
    if (e.key >= '1' && e.key <= '4') {
      const sec = section();
      const l = app.doc.layers[cursor.col];
      const ids = app.ui.selection.layerId === l?.id ? app.ui.selection.ids : [];
      if (sec && l?.kind === 'beat' && ids.length) {
        app.run(() => app.store.dispatch('setHitSlot', { section: sec.id, layer: l.id, ids, slot: Number(e.key) - 1 }));
      }
    }
  });

  // ---- transport and metronome

  btn('play').addEventListener('click', async () => {
    if (!app.host) return;
    await app.host.ctx.resume();
    const i = Math.max(0, app.doc.sections.findIndex((s) => s.id === app.ui.sectionId));
    app.host.seek(i, 0);
    app.host.play();
    acted('play');
  });
  btn('stop').addEventListener('click', () => app.host?.stop());
  btn('secPrev').addEventListener('click', () => stepSection(-1));
  btn('secNext').addEventListener('click', () => stepSection(1));
  function stepSection(d: number): void {
    const n = app.doc.sections.length;
    const i = app.doc.sections.findIndex((s) => s.id === app.ui.sectionId);
    const next = app.doc.sections[(i + d + n) % n];
    if (!next) return;
    app.ui.sectionId = next.id;
    cursor.row = 0;
    app.ui.changed();
    if (app.host) app.host.seek(app.doc.sections.findIndex((s) => s.id === next.id), 0);
  }
  num('bpm').addEventListener('change', () => {
    const sec = section();
    if (!sec) return;
    const bpm = Number(num('bpm').value);
    app.run(() => app.store.dispatch('setSection', { id: sec.id, bpm }));
  });
  num('secRename').addEventListener('change', () => {
    const sec = section();
    const name = num('secRename').value.trim();
    if (!sec || !name) { if (sec) num('secRename').value = sec.name; return; }
    app.run(() => app.store.dispatch('setSection', { id: sec.id, name }));
  });
  num('beats').addEventListener('change', () => {
    const sec = section();
    const beats = Number(num('beats').value);
    if (!sec || !(beats > 0)) return;
    app.run(() => app.store.dispatch('setSection', { id: sec.id, beats }));
  });
  num('seconds').addEventListener('change', () => {
    const sec = section();
    const seconds = Number(num('seconds').value);
    if (!sec || !(seconds > 0)) return;
    app.run(() => app.store.dispatch('setSection', { id: sec.id, seconds }));
  });
  num('repeats').addEventListener('change', () => {
    const sec = section();
    const repeats = Number(num('repeats').value);
    if (!sec || !Number.isInteger(repeats) || repeats < 1) { if (sec) num('repeats').value = String(sec.repeats); return; }
    app.run(() => app.store.dispatch('setSection', { id: sec.id, repeats }));
  });
  $<HTMLSelectElement>('feel').addEventListener('change', () => {
    const sec = section();
    const feel = $<HTMLSelectElement>('feel').value;
    if (!sec || (feel !== 'grid' && feel !== 'free')) return;
    app.run(() => app.store.dispatch('setSection', { id: sec.id, feel }));
    acted('feel');
  });
  num('ramp').addEventListener('change', () => {
    const sec = section();
    const rampMs = Number(num('ramp').value);
    if (!sec || !(rampMs >= 0)) { if (sec) num('ramp').value = String(sec.rampMs); return; }
    app.run(() => app.store.dispatch('setSection', { id: sec.id, rampMs }));
  });
  btn('secEarlier').addEventListener('click', () => {
    const sec = section();
    const i = app.doc.sections.findIndex((s) => s.id === sec?.id);
    if (sec && i > 0) app.run(() => app.store.dispatch('moveSection', { id: sec.id, to: i - 1 }));
  });
  btn('secLater').addEventListener('click', () => {
    const sec = section();
    const i = app.doc.sections.findIndex((s) => s.id === sec?.id);
    if (sec && i >= 0 && i < app.doc.sections.length - 1) app.run(() => app.store.dispatch('moveSection', { id: sec.id, to: i + 1 }));
  });
  btn('loop').addEventListener('click', () => {
    app.run(() => app.store.dispatch('setLoop', { loop: !app.doc.loop }));
    acted('loop');
  });
  num('metroBar').addEventListener('change', () => {
    const bar = Number(num('metroBar').value);
    if (!Number.isInteger(bar) || bar < 1 || bar > 32) {
      num('metroBar').value = String(app.doc.metro?.bar ?? 4);
      return;
    }
    app.run(() => app.store.dispatch('setMetro', { bar }));
  });
  num('layerName').addEventListener('change', () => {
    const l = layer();
    const name = num('layerName').value.trim();
    if (!l || !name) { if (l) num('layerName').value = l.name; return; }
    if (app.run(() => app.store.dispatch('setLayer', { id: l.id, name }))) acted('layer');
  });
  btn('removeLayer').addEventListener('click', () => {
    const l = layer();
    if (!l || app.doc.layers.length < 2) return;
    app.run(() => app.store.dispatch('removeLayer', { id: l.id }));
  });
  $<HTMLSelectElement>('zoom').addEventListener('change', () => {
    const z = Number($<HTMLSelectElement>('zoom').value);
    app.ui.zoom = Number.isFinite(z) && z > 0 ? z : 1;
    timeline.invalidate();
    acted('quantize');
  });
  $<HTMLSelectElement>('snap').addEventListener('change', () => {
    const sec = section();
    const l = layer();
    if (!sec || !l) return;
    const timeSnap = Number($<HTMLSelectElement>('snap').value);
    if (!sec.clips[l.id]) return;
    app.run(() => app.store.dispatch('setClipFields', { section: sec.id, layer: l.id, timeSnap }));
  });
  btn('quantize').addEventListener('click', () => {
    const sec = section();
    const l = layer();
    if (!sec || !l) return;
    const clip = sec.clips[l.id];
    if (!clip) { app.fail('That lane has nothing to quantize yet.'); return; }
    if (sec.feel !== 'grid') { app.fail('Quantize needs a grid section; free sections have no grid.'); return; }
    // Quantize turns Feel on at full strength with the Snap value as the grid (docs/quant.md): the
    // notes keep their played timing and the clip plays them on the grid.
    const grid = clip.timeSnap > 0 ? clip.timeSnap : PPQ / 4;
    if (app.run(() => app.store.dispatch('setFlex', { section: sec.id, layer: l.id, flex: quantizeFlex(clip.flex, grid) }))) {
      $<HTMLDetailsElement>('flexBox').open = true;
      setNext('', null);
      acted('quantize');
    }
  });
  btn('dupSection').addEventListener('click', () => {
    const sec = section();
    if (!sec) return;
    const i = app.doc.sections.findIndex((s) => s.id === sec.id);
    if (app.run(() => app.store.dispatch('duplicateSection', { id: sec.id }))) {
      const copy = app.doc.sections[i + 1];
      if (copy) { app.ui.sectionId = copy.id; app.ui.changed(); }
    }
  });
  btn('removeSection').addEventListener('click', () => {
    const sec = section();
    if (!sec || app.doc.sections.length < 2) return;
    const i = app.doc.sections.findIndex((s) => s.id === sec.id);
    if (!app.run(() => app.store.dispatch('removeSection', { id: sec.id }))) return;
    const next = app.doc.sections[Math.min(i, app.doc.sections.length - 1)];
    if (next) { app.ui.sectionId = next.id; app.ui.changed(); }
  });
  num('roundTrip').addEventListener('change', () => {
    const v = Number(num('roundTrip').value);
    capture.roundTripMs = Number.isFinite(v) ? Math.max(0, Math.min(400, v)) : 30;
    num('roundTrip').value = String(capture.roundTripMs);
  });
  btn('metro').addEventListener('click', () => {
    const on = !(app.doc.metro?.on);
    app.run(() => app.store.dispatch('setMetro', { on }));
    if (on) acted('metro');
  });
  num('metroLevel').addEventListener('input', () => {
    const m = app.doc.metro ?? defaultMetro();
    const level = Number(num('metroLevel').value) / 100;
    const i = Math.max(0, app.doc.layers.findIndex((l) => l.id === m.layerId));
    app.host?.setMetro(m.on, level, m.voice, i, m.slot, playedBar(app.doc, playingSection(app.doc)));
  });
  num('metroLevel').addEventListener('change', () => {
    app.run(() => app.store.dispatch('setMetro', { level: Number(num('metroLevel').value) / 100 }));
  });
  btn('metroVoice').addEventListener('click', () => {
    const m: Metro = app.doc.metro ?? defaultMetro();
    const voice = m.voice === 'tick' ? 'kit' : 'tick';
    const beat = app.doc.layers.find((l) => l.kind === 'beat' && l.id === app.ui.layerId) ?? app.doc.layers.find((l) => l.kind === 'beat');
    app.run(() => app.store.dispatch('setMetro', { voice, layerId: beat?.id ?? m.layerId, slot: m.slot }));
  });
  btn('undo').addEventListener('click', () => app.run(() => app.store.undo()));
  btn('redo').addEventListener('click', () => app.run(() => app.store.redo()));

  const addLayer = (kind: 'melody' | 'beat' | 'texture') => {
    const at = app.doc.layers.length;
    if (!app.run(() => app.store.dispatch('addLayer', { kind }))) return;
    const added = app.doc.layers[at];
    if (!added) return;
    app.ui.layerId = added.id;
    cursor.col = at;
    app.ui.changed();
  };
  btn('addMelody').addEventListener('click', () => addLayer('melody'));
  btn('addBeat').addEventListener('click', () => addLayer('beat'));
  btn('addTexture').addEventListener('click', () => addLayer('texture'));
  btn('addSection').addEventListener('click', () => {
    const sec = section();
    if (app.run(() => app.store.dispatch('addSection', { after: sec?.id, feel: 'grid', beats: sec?.beats ?? 8, bpm: sec?.bpm ?? 100 }))) {
      const i = sec ? app.doc.sections.findIndex((s) => s.id === sec.id) + 1 : app.doc.sections.length - 1;
      const next = app.doc.sections[i];
      if (next) { app.ui.sectionId = next.id; app.ui.changed(); }
    }
  });

  // ---- sound, states, influence, morph

  const tuneRoot = $<HTMLSelectElement>('tuneRoot');
  const tuneScale = $<HTMLSelectElement>('tuneScale');
  NOTE_NAMES.forEach((n, i) => tuneRoot.append(new Option(n, String(i))));
  SCALES.forEach((n, i) => tuneScale.append(new Option(n, String(i))));
  const tuneRanges: { id: string; label: string; key: keyof TuneFields; min: number; max: number; scale: number }[] = [
    { id: 'tuneSnap', label: 'Pitch snap', key: 'snap', min: 0, max: 100, scale: 100 },
    { id: 'tuneSpeed', label: 'Speed', key: 'speedMs', min: 0, max: 250, scale: 1 },
    { id: 'tuneVibrato', label: 'Vibrato', key: 'vibrato', min: 0, max: 200, scale: 100 },
    { id: 'tuneLevel', label: 'Level', key: 'levelOut', min: 0, max: 100, scale: 100 },
  ];
  for (const spec of tuneRanges) {
    const label = document.createElement('label');
    const title = document.createElement('span');
    title.textContent = spec.label;
    const input = document.createElement('input');
    input.type = 'range';
    input.min = String(spec.min);
    input.max = String(spec.max);
    input.id = spec.id;
    input.setAttribute('aria-label', `${spec.label} of the recorded pitch`);
    const commit = (live: boolean) => {
      const l = layer();
      if (!l) return;
      const value = Number(input.value) / spec.scale;
      const tune = { [spec.key]: value } as Partial<TuneFields>;
      pushTune(l, tune);
      if (!live && app.run(() => app.store.dispatch('setLayer', { id: l.id, tune }))) acted('tune');
    };
    input.addEventListener('input', () => commit(true));
    input.addEventListener('change', () => commit(false));
    label.append(title, input, readout(input, spec.key === 'speedMs' ? (v) => `${v} ms` : pct));
    $('tuneRanges').append(label);
  }
  function commitTune(tune: Partial<TuneFields>): void {
    const l = layer();
    if (!l) return;
    pushTune(l, tune);
    if (app.run(() => app.store.dispatch('setLayer', { id: l.id, tune }))) acted('tune');
  }
  $<HTMLSelectElement>('tuneSlides').addEventListener('change', () => {
    const v = Number($<HTMLSelectElement>('tuneSlides').value);
    if (v === 0 || v === 1 || v === 2) commitTune({ slides: v });
  });
  tuneRoot.addEventListener('change', () => commitTune({ root: Number(tuneRoot.value) }));
  tuneScale.addEventListener('change', () => commitTune({ scale: Number(tuneScale.value) }));
  num('tuneOctave').addEventListener('change', () => {
    const octave = Number(num('tuneOctave').value);
    if (!Number.isInteger(octave) || octave < -4 || octave > 4) {
      const l = layer();
      if (l) num('tuneOctave').value = String(l.tune.octave);
      return;
    }
    commitTune({ octave });
  });
  btn('fillTune').addEventListener('click', () => {
    const l = layer();
    const sec = section();
    const id = sec && l ? sec.clips[l.id]?.take : '';
    const take = id ? app.doc.takes[id] : undefined;
    if (!l || !take) { app.fail('This layer has no take to fill Tune from.'); return; }
    commitTune(tuneFromTake(take));
  });

  $<HTMLSelectElement>('preset').addEventListener('change', () => {
    const l = layer();
    const v = $<HTMLSelectElement>('preset').value;
    if (!l || !v) return;
    const p = (v[0] === 'f' ? factory : userPresets)[Number(v.slice(1))];
    if (p) app.run(() => app.store.dispatch('loadPreset', { layer: l.id, state: app.ui.editState(l).id, preset: p }));
  });
  btn('savePreset').addEventListener('click', () => {
    const l = layer();
    if (!l) return;
    const name = prompt('Name of the preset', l.name)?.trim();
    if (!name) return;
    const p = presetFromLayer(l, app.ui.editState(l), name);
    const same = userPresets.findIndex((x) => x.name === name && x.kind === p.kind);
    if (same >= 0 && !confirm(`Replace your preset "${name}"?`)) return;
    if (same >= 0) userPresets[same] = p;
    else userPresets.push(p);
    void keepPresets().then(() => app.refreshAll());
  });
  btn('exportPreset').addEventListener('click', () => {
    const l = layer();
    if (!l) return;
    const v = $<HTMLSelectElement>('preset').value;
    const named = v ? (v[0] === 'f' ? factory : userPresets)[Number(v.slice(1))]?.name : undefined;
    const p = presetFromLayer(l, app.ui.editState(l), named ?? l.name);
    download(new Blob([presetText(p)], { type: 'application/json' }), presetFileName(p.name).replace(/\.json$/, '.earworm-patch.json'));
  });
  $<HTMLInputElement>('importPreset').addEventListener('change', async () => {
    const input = $<HTMLInputElement>('importPreset');
    const file = input.files?.[0];
    input.value = '';
    const l = layer();
    if (!file || !l) return;
    let p: PresetFile;
    try {
      p = parsePreset(JSON.parse(await file.text()));
    } catch (e) {
      app.fail(e instanceof SyntaxError ? `${file.name} is not a preset file.` : e instanceof Error ? e.message : String(e));
      return;
    }
    userPresets = [...userPresets.filter((x) => !(x.name === p.name && x.kind === p.kind)), p];
    await keepPresets();
    if ((p.kind === 'beat') === (l.kind === 'beat')) app.run(() => app.store.dispatch('loadPreset', { layer: l.id, state: app.ui.editState(l).id, preset: p }));
    else app.fail(`"${p.name}" is saved under Yours; it is ${p.kind === 'beat' ? 'a kit, for a beat layer' : 'for a melody or texture layer'}.`);
    app.refreshAll();
  });
  num('level').addEventListener('input', () => {
    const l = layer();
    const i = app.doc.layers.findIndex((x) => x.id === l?.id);
    if (i >= 0) app.host?.setLayer(i, LAYER_LEVEL, Number(num('level').value));
    if (l) app.rec.move(l.id, 'layer.level', Number(num('level').value));
  });
  num('level').addEventListener('change', () => {
    const l = layer();
    if (l && !app.rec.recording) app.run(() => app.store.dispatch('setLayer', { id: l.id, level: Number(num('level').value) }));
  });
  num('pan').addEventListener('input', () => {
    const l = layer();
    const pan = Number(num('pan').value) / 100;
    $('panRead').textContent = panLabel(pan);
    const i = app.doc.layers.findIndex((x) => x.id === l?.id);
    if (i >= 0) app.host?.setLayer(i, LAYER_PAN, pan);
    if (l) app.rec.move(l.id, 'layer.pan', pan);
  });
  num('pan').addEventListener('change', () => {
    const l = layer();
    if (l && !app.rec.recording) app.run(() => app.store.dispatch('setLayer', { id: l.id, pan: Number(num('pan').value) / 100 }));
  });
  btn('mute').addEventListener('click', () => {
    const l = layer();
    if (l) app.run(() => app.store.dispatch('setLayer', { id: l.id, mute: !l.mute }));
  });

  function macroValues(l: Layer, k: number, v: number): number[] {
    const out = app.ui.editState(l).macros.slice();
    while (out.length < N_MACROS) out.push(0.5);
    out[k] = v;
    return out;
  }
  const macroBox = $('macros');
  MACRO_NAMES.forEach((name, k) => {
    const label = document.createElement('label');
    const title = document.createElement('span');
    title.textContent = name;
    const input = document.createElement('input');
    input.type = 'range';
    input.min = '0';
    input.max = '100';
    input.step = '1';
    input.id = `macro-${k}`;
    input.setAttribute('aria-label', `${name} macro`);
    input.addEventListener('input', () => {
      const l = layer();
      if (!l) return;
      const macros = macroValues(l, k, Number(input.value) / 100);
      app.rec.move(l.id, `macro${k + 1}`, Number(input.value) / 100);
      app.previewDoc((d) => applyOps(d, commands.setState(d, { layer: l.id, id: app.ui.editState(l).id, macros })).doc);
    });
    input.addEventListener('change', () => {
      const l = layer();
      // While a pass records, where the hand leaves it is committed with the pass.
      if (!l || app.rec.recording) return;
      const macros = macroValues(l, k, Number(input.value) / 100);
      if (app.run(() => app.store.dispatch('setState', { layer: l.id, id: app.ui.editState(l).id, macros }))) acted('macro');
    });
    label.append(title, input, readout(input, (v) => String(v)));
    macroBox.append(label);
  });

  const gestureBox = $('gestures');
  const gestures: { key: keyof GestureDepth; label: string; field: number }[] = [
    { key: 'loud', label: 'Loud', field: LAYER_DEPTH_LOUD },
    { key: 'open', label: 'Open', field: LAYER_DEPTH_OPEN },
    { key: 'hard', label: 'Hard', field: LAYER_DEPTH_HARD },
    { key: 'breath', label: 'Breath', field: LAYER_DEPTH_BREATH },
    { key: 'mouth', label: 'Mouth', field: LAYER_DEPTH_MOUTH },
  ];
  for (const g of gestures) {
    const label = document.createElement('label');
    const title = document.createElement('span');
    title.textContent = g.label;
    const input = document.createElement('input');
    input.type = 'range';
    input.min = '0';
    input.max = '100';
    input.step = '1';
    input.id = `gesture-${g.key}`;
    input.setAttribute('aria-label', `${g.label} gesture depth`);
    input.addEventListener('input', () => {
      const l = layer();
      const i = app.doc.layers.findIndex((x) => x.id === l?.id);
      if (i >= 0) app.host?.setLayer(i, g.field, Number(input.value) / 100);
    });
    input.addEventListener('change', () => {
      const l = layer();
      if (!l) return;
      if (app.run(() => app.store.dispatch('setLayer', { id: l.id, depth: { [g.key]: Number(input.value) / 100 } }))) acted('gesture');
    });
    label.append(title, input, readout(input, pct));
    gestureBox.append(label);
  }
  {
    const label = document.createElement('label');
    const title = document.createElement('span');
    title.textContent = 'Follow';
    const input = document.createElement('input');
    input.type = 'range';
    input.min = '0';
    input.max = '100';
    input.step = '1';
    input.id = 'gesture-follow';
    input.setAttribute('aria-label', 'How far a hit follows the sung shape');
    input.addEventListener('input', () => {
      const l = layer();
      const i = app.doc.layers.findIndex((x) => x.id === l?.id);
      if (i >= 0) app.host?.setLayer(i, LAYER_FOLLOW, Number(input.value) / 100);
    });
    input.addEventListener('change', () => {
      const l = layer();
      if (l && app.run(() => app.store.dispatch('setLayer', { id: l.id, follow: Number(input.value) / 100 }))) acted('gesture');
    });
    label.append(title, input, readout(input, pct));
    gestureBox.append(label);
  }

  const kitSlot = $<HTMLSelectElement>('kitSlot');
  SLOT_NAMES.forEach((name, i) => kitSlot.append(new Option(`${i + 1} ${name}`, String(i))));
  async function useKitHit(signal: Float32Array, rate: number, name: string): Promise<void> {
    const l = layer();
    if (!l || l.kind !== 'beat') { app.fail('Pick a beat layer first.'); return; }
    const slot = Number(kitSlot.value);
    $('kitStatus').textContent = `Matching ${name}…`;
    const m = await (await app.matcher()).match(signal, rate, { decaying: true });
    const cents = m.pitched ? Math.max(0, Math.min(12000, Math.round(m.pitchCents))) : l.kit[slot]?.cents ?? 4800;
    if (app.run(() => app.store.dispatch('setKitSlot', { layer: l.id, slot, params: Array.from(m.target), cents }))) {
      $('kitStatus').textContent = `Slot ${slot + 1} (${SLOT_NAMES[slot]}) set from ${name}.`;
      acted('kit');
    }
  }
  $<HTMLInputElement>('kitFile').addEventListener('change', async () => {
    const file = $<HTMLInputElement>('kitFile').files?.[0];
    if (!file) return;
    try {
      const ctx = app.ctx ?? new AudioContext();
      const signal = await decodeBytes(ctx, await file.arrayBuffer());
      const rate = ctx.sampleRate;
      if (!app.ctx) void ctx.close();
      await useKitHit(signal, rate, file.name);
    } catch (e) {
      app.fail(e instanceof Error ? e.message : String(e));
    } finally {
      $<HTMLInputElement>('kitFile').value = '';
    }
  });
  let kitClip: MicClip | null = null;
  holdButton(btn('kitRecord'), () => {
    const host = app.host;
    if (!host || !app.stream) { app.fail('Start, then enable the mic, to record a hit.'); return; }
    kitClip = new MicClip(host, host.sampleRate, MAX_SOURCE_S);
    $('kitStatus').textContent = 'Recording one hit…';
  }, () => {
    const c = kitClip;
    kitClip = null;
    if (!c) return;
    const sig = trimAndCap(c.read(), c.rate);
    if (sig.length < 0.05 * c.rate) { $('kitStatus').textContent = 'Too short. Hold while you make the hit.'; return; }
    void useKitHit(sig, c.rate, 'a hit').catch((e) => app.fail(e instanceof Error ? e.message : String(e)));
  });
  let kitHeld: { i: number; cents: number } | null = null;
  holdButton(btn('kitHear'), () => {
    const l = layer();
    const host = app.host;
    if (!l || !host || l.kind !== 'beat') { app.fail('Pick a beat layer and press Start.'); return; }
    const i = app.doc.layers.findIndex((x) => x.id === l.id);
    const slot = Number(kitSlot.value);
    const cents = l.kit[slot]?.cents ?? 4800;
    void host.ctx.resume();
    host.noteOn(i, cents, slot);
    kitHeld = { i, cents };
  }, () => {
    if (kitHeld) app.host?.noteOff(kitHeld.i, kitHeld.cents);
    kitHeld = null;
  });
  $<HTMLSelectElement>('state').addEventListener('change', () => {
    const l = layer();
    if (l) app.ui.setEditState(l, $<HTMLSelectElement>('state').value);
  });
  num('stateName').addEventListener('change', () => {
    const l = layer();
    const name = num('stateName').value.trim();
    if (!l) return;
    const id = app.ui.editState(l).id;
    if (!name) { num('stateName').value = app.ui.editState(l).name; return; }
    if (app.run(() => app.store.dispatch('setState', { layer: l.id, id, name }))) acted('source');
  });
  btn('addState').addEventListener('click', () => {
    const l = layer();
    if (!l) return;
    if (app.run(() => app.store.dispatch('addState', { layer: l.id }))) {
      const added = app.doc.layers.find((x) => x.id === l.id)?.states.at(-1);
      if (added) app.ui.setEditState(l, added.id);
      acted('state');
    }
  });
  btn('copyState').addEventListener('click', () => {
    const l = layer();
    if (!l) return;
    const from = app.ui.editState(l).id;
    if (app.run(() => app.store.dispatch('addState', { layer: l.id, from }))) {
      const added = app.doc.layers.find((x) => x.id === l.id)?.states.at(-1);
      if (added) app.ui.setEditState(l, added.id);
      acted('state');
    }
  });
  btn('removeState').addEventListener('click', () => {
    const l = layer();
    if (!l || l.states.length < 2) return;
    const id = app.ui.editState(l).id;
    app.run(() => app.store.dispatch('removeState', { layer: l.id, id }));
  });
  btn('addMarker').addEventListener('click', () => {
    const sec = section();
    const l = layer();
    if (!sec || !l) return;
    const shape = $<HTMLSelectElement>('morphShape').value as MorphShape;
    const lenMs = Number(num('morphLen').value);
    const t = rowStart(sec, cursor.row);
    if (app.run(() => app.store.dispatch('addMorphMarker', { section: sec.id, layer: l.id, marker: { t, state: app.ui.editState(l).id, lenMs, shape } }))) acted('marker');
  });

  const aspectBox = $('aspects');
  for (const a of ASPECTS) {
    const label = document.createElement('label');
    const input = document.createElement('input');
    input.type = 'checkbox';
    input.id = `aspect-${a.bit}`;
    input.checked = true;
    input.addEventListener('change', () => {
      const l = layer();
      if (!l?.influence) return;
      let mask = 0;
      for (const x of ASPECTS) if ((document.getElementById(`aspect-${x.bit}`) as HTMLInputElement).checked) mask |= x.bit;
      app.run(() => app.store.dispatch('setInfluenceMask', { layer: l.id, mask }));
    });
    label.append(input, document.createTextNode(a.label));
    aspectBox.append(label);
  }
  readout(num('level'), (v) => `${v > 0 ? '+' : ''}${v} dB`, $<HTMLOutputElement>('levelRead'));
  readout(num('amount'), pct, $<HTMLOutputElement>('amountRead'));
  num('amount').addEventListener('input', () => {
    const l = layer();
    const i = app.doc.layers.findIndex((x) => x.id === l?.id);
    if (i >= 0) app.host?.setLayer(i, LAYER_INFLUENCE_AMOUNT, Number(num('amount').value) / 100);
  });
  num('amount').addEventListener('change', () => {
    const l = layer();
    if (!l) return;
    app.run(() => app.store.dispatch('setInfluenceAmount', { layer: l.id, state: app.ui.editState(l).id, amount: Number(num('amount').value) / 100 }));
  });
  btn('commit').addEventListener('click', () => {
    const l = layer();
    if (l?.influence) app.run(() => app.store.dispatch('commitInfluence', { layer: l.id, state: app.ui.editState(l).id }));
  });
  btn('clearInf').addEventListener('click', () => {
    const l = layer();
    if (l) app.run(() => app.store.dispatch('setInfluence', { layer: l.id, influence: null }));
  });

  async function commitMatch(layerId: string, signal: Float32Array, rate: number, name: string, m: Match): Promise<void> {
    const ref = `grain-${newId('s').slice(2)}`;
    try { await session.putGrain(ref, signal); } catch (e) { app.fail(e instanceof Error ? e.message : String(e)); }
    const l = app.doc.layers.find((x) => x.id === layerId);
    if (!l) return;
    const st = app.ui.editState(l);
    const influence: Influence = {
      target: Array.from(m.target), mask: 0x1f, source: ref, sourceName: name, sourceRate: rate,
      grain: grainRegion(m, signal.length), useSound: m.useSound,
    };
    app.runRestoring(() => app.store.batch('Set Influence', (run) => {
      run('setInfluence', { layer: layerId, influence });
      if (st.influenceAmount === 0) run('setInfluenceAmount', { layer: layerId, state: st.id, amount: 1 });
    }));
    acted('influence');
  }

  function previewTarget(l: Layer, target: ArrayLike<number>, amount: number, mask: number): void {
    const host = app.host;
    const i = app.doc.layers.findIndex((x) => x.id === l.id);
    if (!host || i < 0) return;
    vocabIndices(l.kind, l.patch).forEach((ix, k) => { for (const v of ix) host.setParam(i, v, target[k], SET_INFLUENCE); });
    host.setLayer(i, LAYER_INFLUENCE_AMOUNT, amount);
    host.setLayer(i, LAYER_INFLUENCE_MASK, mask);
    if (layerUsesStates(app.doc, l)) {
      app.previewDoc((d) => {
        const infl: Influence = { target: Array.from(target), mask, source: 'grain-live', sourceName: 'Listening', sourceRate: host.sampleRate, grain: null, useSound: false };
        const step = applyOps(d, commands.setInfluence(d, { layer: l.id, influence: infl }));
        return applyOps(step.doc, commands.setInfluenceAmount(step.doc, { layer: l.id, state: app.ui.editState(l).id, amount })).doc;
      });
    }
  }

  $<HTMLInputElement>('infFile').addEventListener('change', async () => {
    const file = $<HTMLInputElement>('infFile').files?.[0];
    const l = layer();
    if (!file || !l) return;
    try {
      const ctx = app.ctx ?? new AudioContext();
      const signal = await decodeFile(ctx, file);
      const rate = ctx.sampleRate;
      if (!app.ctx) void ctx.close();
      $('infStatus').textContent = 'Matching…';
      const m = await (await app.matcher()).match(signal, rate);
      await commitMatch(l.id, signal, rate, file.name, m);
    } catch (e) {
      app.fail(e instanceof Error ? e.message : String(e));
    }
  });

  holdButton(btn('listen'), () => {
    const host = app.host;
    const l = layer();
    if (!host || !l || !app.stream) { app.fail('Start, then enable the mic, to listen.'); return; }
    const clip = new MicClip(host, host.sampleRate, MAX_SOURCE_S);
    const amount = app.ui.editState(l).influenceAmount || 1;
    const live = liveMatch(() => clip.read(), { match: (sig, rate, opts) => app.matcher().then((mm) => mm.match(sig, rate, opts)) }, (m) => {
      const cur = app.doc.layers.find((x) => x.id === l.id);
      liveTarget = m.target;
      liveProvisional = m.provisional;
      if (cur) {
        previewTarget(cur, m.target, amount, cur.influence?.mask ?? 0x1f);
        paintMeters(m.target, stateVocab(cur, app.ui.editState(cur)), m.provisional);
      }
      $('infStatus').textContent = m.provisional ? 'Listening… motion is provisional.' : 'Listening…';
    }, { sampleRate: host.sampleRate, onError: (e) => app.fail(e.message) });
    listening = { layerId: l.id, clip, live };
  }, () => {
    const cur = listening;
    listening = null;
    liveTarget = null;
    if (!cur) return;
    void cur.live.finish().then((m) => commitMatch(cur.layerId, cur.clip.read(), cur.clip.rate, 'Listening', m)).catch((e) => app.fail(e instanceof Error ? e.message : String(e)));
  });

  num('useSound').addEventListener('change', async () => {
    const l = layer();
    const infl = l?.influence;
    if (!l || !infl) return;
    const data = session.grains.get(infl.source);
    if (!data) { app.fail('The source audio is not loaded yet.'); return; }
    try {
      const want = num('useSound').checked;
      const m = await (await app.matcher()).match(data, infl.sourceRate, { useSound: want });
      const next: Influence = { ...infl, target: Array.from(m.target), useSound: want, grain: want ? grainRegion(m, data.length) : infl.grain };
      app.run(() => app.store.dispatch('setInfluence', { layer: l.id, influence: next }));
    } catch (e) {
      app.fail(e instanceof Error ? e.message : String(e));
    }
  });

  async function fillSaved(): Promise<void> {
    try { savedList = await loadSources(); } catch { savedList = []; }
    const sel = $<HTMLSelectElement>('savedSource');
    sel.replaceChildren(...(savedList.length
      ? savedList.map((s, i) => new Option(`${s.name} (${(s.signal.length / s.sampleRate).toFixed(1)} s)`, String(i)))
      : [new Option('None saved', '')]));
    btn('useSaved').disabled = savedList.length === 0;
  }
  $<HTMLSelectElement>('savedSource').addEventListener('focus', () => { void fillSaved(); });
  btn('useSaved').addEventListener('click', async () => {
    const s = savedList[Number($<HTMLSelectElement>('savedSource').value)];
    const l = layer();
    if (!s || !l) { app.fail('No saved source. The Influence page keeps them.'); return; }
    try {
      $('infStatus').textContent = 'Matching…';
      const signal = trimAndCap(s.signal, s.sampleRate);
      const m = await (await app.matcher()).match(signal, s.sampleRate);
      await commitMatch(l.id, signal, s.sampleRate, s.name, m);
      acted('source');
    } catch (e) {
      app.fail(e instanceof Error ? e.message : String(e));
    }
  });
  void fillSaved();

  function stopSource(): void {
    const n = sourceNode;
    sourceNode = null;
    if (n) { n.onended = null; try { n.stop(); } catch { /* already stopped */ } n.disconnect(); }
    btn('playSource').textContent = 'Play source';
  }
  btn('playSource').addEventListener('click', async () => {
    if (sourceNode) { stopSource(); return; }
    const l = layer();
    const infl = l?.influence;
    const ctx = app.ctx;
    if (!infl || !ctx) { app.fail(ctx ? 'This layer has no source.' : 'Press Start first.'); return; }
    await session.loadGrain(infl.source);
    const data = session.grains.get(infl.source);
    if (!data) { app.fail('The source audio is not loaded yet.'); return; }
    await ctx.resume();
    const buf = ctx.createBuffer(1, data.length, infl.sourceRate);
    buf.copyToChannel(data as Float32Array<ArrayBuffer>, 0);
    const node = ctx.createBufferSource();
    node.buffer = buf;
    node.connect(ctx.destination);
    node.onended = () => { if (sourceNode === node) stopSource(); };
    node.start();
    sourceNode = node;
    btn('playSource').textContent = 'Stop source';
    acted('source');
  });
  let noteHeld: { i: number; cents: number } | null = null;
  holdButton(btn('holdNote'), () => {
    const l = layer();
    const host = app.host;
    if (!l || !host) { app.fail('Press Start first.'); return; }
    const i = app.doc.layers.findIndex((x) => x.id === l.id);
    const cents = l.influence?.grain?.refCents ?? 4800;
    void host.ctx.resume();
    host.noteOn(i, cents, l.kind === 'beat' ? Number($<HTMLSelectElement>('kitSlot').value) : 0);
    noteHeld = { i, cents };
    acted('source');
  }, () => {
    if (noteHeld) app.host?.noteOff(noteHeld.i, noteHeld.cents);
    noteHeld = null;
  });

  $<HTMLSelectElement>('takeStack').addEventListener('change', () => {
    const sec = section();
    const l = layer();
    const take = $<HTMLSelectElement>('takeStack').value;
    if (!sec || !l || !take) return;
    if (app.run(() => app.store.dispatch('selectTake', { section: sec.id, layer: l.id, take }))) acted('take');
  });

  let teachClip: { clip: MicClip; f0: number } | null = null;
  holdButton(btn('teachSlot'), () => {
    const host = app.host;
    const l = layer();
    if (!host || !app.stream || !l || l.kind !== 'beat') { app.fail('Start, enable the mic, and pick a beat layer.'); return; }
    teachClip = { clip: new MicClip(host, host.sampleRate, MAX_SOURCE_S), f0: host.framesTotal() };
    $('kitStatus').textContent = 'Teaching… repeat one sound.';
  }, () => {
    const held = teachClip;
    teachClip = null;
    const host = app.host;
    const l = layer();
    if (!held || !host || !l || !app.analysis) return;
    const c = held.clip;
    const signal = c.read();
    if (signal.length < 0.3 * c.rate) { $('kitStatus').textContent = 'Too short to teach. Hold a little longer.'; return; }
    const slot = Number($<HTMLSelectElement>('kitSlot').value);
    const nFrames = Math.max(1, Math.round(c.seconds * 100));
    const frames = host.readFrames(held.f0, nFrames);
    void app.analysis.secondPass(signal.slice(), c.rate, frames, { kind: 'beat' }).then((sp) => {
      taughtHits[slot] = sp.hits.map((h) => ({ ...h, slot }));
      const taught = teach(taughtHits);
      capture.taught = taught;
      const i = app.doc.layers.findIndex((x) => x.id === l.id);
      const m = meanLive(taughtHits[slot]);
      host.setLiveKit(i, slot, LIVE_KIT_TAUGHT, m && taught[slot] ? 1 : 0);
      if (m) {
        host.setLiveKit(i, slot, LIVE_KIT_BRIGHT, m[0]);
        host.setLiveKit(i, slot, LIVE_KIT_LOW, m[1]);
        host.setLiveKit(i, slot, LIVE_KIT_NOISE, m[2]);
      }
      $('kitStatus').textContent = `Slot ${slot + 1} taught from ${sp.hits.length} hits. The next beat take uses it.`;
      acted('teach');
    }).catch((e) => app.fail(e instanceof Error ? e.message : String(e)));
  });

  btn('measure').addEventListener('click', async () => {
    const host = app.host;
    if (!host || !app.stream) { app.fail('Start with the mic, then measure. Use headphones.'); return; }
    const button = btn('measure');
    button.disabled = true;
    $('capStatus').textContent = 'Measuring the round trip…';
    try {
      const r = await measureRoundTrip(host.ctx, app.stream);
      if (r.medianMs == null) { app.fail(r.error ?? 'No chirp came back.'); return; }
      const ms = Math.max(0, Math.min(400, Math.round(r.medianMs)));
      capture.roundTripMs = ms;
      num('roundTrip').value = String(ms);
      $('capStatus').textContent = `Round trip ${ms} ms from ${r.found} chirps.`;
      $('deviceNote').hidden = true;
      acted('measure');
    } catch (e) {
      app.fail(e instanceof Error ? e.message : String(e));
    } finally {
      button.disabled = false;
    }
  });
  $<HTMLSelectElement>('bands').addEventListener('change', () => {
    const n = Number($<HTMLSelectElement>('bands').value);
    bands = n === 24 ? 24 : 16;
    $('capStatus').textContent = bands === 16
      ? '16 bands. That is the song engine’s vowel bank.'
      : 'The song engine’s vowel bank is 16 bands. 24 is the Live page’s front end; takes here stay 16 so the frame stays one width.';
    acted('bands');
  });
  navigator.mediaDevices?.addEventListener('devicechange', () => {
    if (capture.holding) capture.cancel();
    const note = $('deviceNote');
    note.hidden = false;
    note.textContent = 'The audio device changed. Capture is paused. Measure the round trip again before the next take.';
  });

  // ---- capture and keys

  holdButton(btn('capture'), () => { capture.start(); acted('capture'); }, () => { void capture.end(); });
  btn('record').addEventListener('click', () => { capture.toggleRecord(); acted('capture'); });
  num('importTake').addEventListener('change', () => {
    const input = num('importTake');
    const file = input.files?.[0];
    input.value = '';
    const sec = section();
    const l = layer();
    if (!file || !sec || !l) return;
    capture.importFile(file, l.id, sec.id).catch((e) => app.fail(e instanceof Error ? e.message : String(e)));
  });
  $<HTMLSelectElement>('laneTarget').addEventListener('change', () => {
    const l = layer();
    if (!l) return;
    timeline.laneShown.set(l.id, $<HTMLSelectElement>('laneTarget').value);
    timeline.invalidate();
    app.refreshAll();
  });
  btn('laneClear').addEventListener('click', () => {
    const l = layer();
    const sec = section();
    if (!l || !sec) return;
    const target = timeline.laneTarget(l, sec.clips[l.id]);
    if (target) app.run(() => app.store.dispatch('setLane', { section: sec.id, layer: l.id, target, points: null }));
  });
  window.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') { app.host?.stop(); return; }
    if (e.altKey && !e.metaKey && !e.ctrlKey && ['Digit1', 'Digit2', 'Digit3'].includes(e.code)) {
      e.preventDefault();
      const place = e.code === 'Digit1' ? 'pattern' : e.code === 'Digit2' ? 'sound' : 'perform';
      showPanel(place);
      $(`panel-${place}`).focus();
      return;
    }
    if (e.key === '?' && !typingIn(e.target) && !document.querySelector('dialog[open]')) {
      e.preventDefault();
      $<HTMLDialogElement>('keysDialog').showModal();
      return;
    }
    const mod = e.metaKey || e.ctrlKey;
    if (mod && e.key.toLowerCase() === 'z') {
      e.preventDefault();
      app.run(() => e.shiftKey ? app.store.redo() : app.store.undo());
      return;
    }
    if (mod && e.key.toLowerCase() === 'y') { e.preventDefault(); app.run(() => app.store.redo()); return; }
    if (mod || e.altKey || typingIn(e.target) || document.querySelector('dialog:modal')) return;
    // One meaning per key (docs/UI1.md, question 1): Space plays and stops, hold C captures, R records a pass.
    if (e.code === 'KeyC') {
      e.preventDefault();
      if (e.repeat || capture.holding) return;
      if (!app.host || !app.stream) { $('capStatus').textContent = app.host ? 'Capture needs the mic: press Enable mic.' : 'Press Start first.'; return; }
      capture.start();
      acted('capture');
      return;
    }
    if (e.code === 'KeyR') {
      e.preventDefault();
      if (e.repeat || !app.host) return;
      capture.toggleRecord();
      acted('capture');
      return;
    }
    // On a button, a box or a tab, Space keeps its own meaning (press it); the pattern's cells are the exception.
    const onControl = e.target instanceof Element && !!e.target.closest('button:not(.cell), summary, a, input[type="checkbox"], input[type="radio"]');
    if (e.key !== ' ' || e.repeat || onControl) return;
    e.preventDefault();
    if (app.host) {
      const playing = app.readMeters()?.[M_PLAYING] === 1;
      if (playing) app.host.stop();
      else {
        const i = Math.max(0, app.doc.sections.findIndex((s) => s.id === app.ui.sectionId));
        void app.host.ctx.resume();
        app.host.seek(i, 0);
        app.host.play();
        acted('play');
      }
    }
  });
  window.addEventListener('keyup', (e) => {
    if (e.code === 'KeyC' && capture.holding) void capture.end();
  });

  // ---- file

  async function copyBlobs(): Promise<Map<string, Float32Array>> {
    const out = new Map<string, Float32Array>();
    for (const ref of await session.blobs.list()) {
      const d = await session.blobs.get(ref);
      if (d) out.set(ref, d.slice());
    }
    return out;
  }
  async function writeBlobs(blobs: Map<string, Float32Array>, doc: Project): Promise<void> {
    for (const [ref, data] of blobs) {
      if (ref.startsWith('grain-')) await session.putGrain(ref, data);
      else await session.blobs.put(ref, data);
    }
    await Promise.all(Object.keys(doc.takes).map((id) => session.loadTake(id)));
    await app.sync.request(app.doc);
  }
  async function refreshOpenList(): Promise<void> {
    const sel = $<HTMLSelectElement>('openList');
    const rows = await db.list();
    sel.replaceChildren();
    for (const r of rows) {
      const o = document.createElement('option');
      o.value = r.id;
      o.textContent = r.name || 'Untitled';
      if (r.id === app.doc.id) o.selected = true;
      sel.append(o);
    }
  }

  btn('fileBtn').addEventListener('click', () => { void refreshOpenList().then(() => $<HTMLDialogElement>('fileDialog').showModal()); });
  btn('newBtn').addEventListener('click', () => { void session.create('Untitled'); });
  btn('saveBtn').addEventListener('click', async () => { await db.flush(); $('capStatus').textContent = 'Saved.'; acted('save'); });
  btn('saveAsBtn').addEventListener('click', async () => {
    const name = num('projNameInput').value.trim() || `${app.doc.name} copy`;
    const blobs = await copyBlobs();
    const copy = structuredClone(app.doc);
    copy.id = newId('proj');
    copy.name = name;
    await session.adopt(copy);
    await writeBlobs(blobs, copy);
    $('capStatus').textContent = `Saved as ${name}.`;
  });
  btn('renameBtn').addEventListener('click', () => {
    const name = num('projNameInput').value.trim();
    if (name) app.run(() => app.store.dispatch('renameProject', { name }));
  });
  btn('deleteBtn').addEventListener('click', async () => {
    const id = app.doc.id;
    if (!confirm(`Delete “${app.doc.name}”? This removes it from this browser.`)) return;
    await session.remove(id);
    await session.init();
  });
  btn('openBtn').addEventListener('click', async () => {
    const id = $<HTMLSelectElement>('openList').value;
    if (id && (await session.open(id)) && session.historyReset) $('capStatus').textContent = UPGRADED;
  });
  btn('exportBtn').addEventListener('click', async () => {
    const { bytes, missing } = await exportProject(app.doc, (ref) => session.blobs.get(ref));
    if (missing.length) app.fail(`Exported without ${missing.length} missing file${missing.length === 1 ? '' : 's'}.`);
    const copy = new ArrayBuffer(bytes.byteLength);
    new Uint8Array(copy).set(bytes);
    const blob = new Blob([copy]);
    const a = document.createElement('a');
    a.href = URL.createObjectURL(blob);
    a.download = `${(app.doc.name || 'earworm').replace(/[^\w.-]+/g, '_')}.zip`;
    a.click();
    URL.revokeObjectURL(a.href);
  });
  btn('exportWav').addEventListener('click', async () => {
    const button = btn('exportWav');
    button.disabled = true;
    const renderer = new OfflineRenderer();
    try {
      const compiler = new Compiler();
      const uploads = compiler.compile(app.doc, (take, lay) => app.sync.cachedCurve(take, lay) ?? session.framesFor(take), {
        grainFor: (lay) => session.grainFor(lay),
        audioFor: (t) => session.audioFor(t),
        anchorsFor: (t) => session.anchorsFor(t),
        noteCurvesFor: (t) => session.noteCurvesFor(t),
      });
      const song = uploadsToSong(uploads);
      const seconds = songSeconds(app.doc);
      $('engineStatus').textContent = 'Rendering the mix…';
      // At the rate the takes were captured at, as live playback runs: a take stretches only at
      // its own rate (docs/Q2.md), and the export then matches what is heard.
      const sampleRate = app.host?.sampleRate;
      const job = renderer.render(song, { once: true, settle: true, maxSeconds: Math.min(180, Math.max(2, seconds + 12)), ...(sampleRate ? { sampleRate } : {}) }, (p) => {
        $('engineStatus').textContent = `Rendering ${p.seconds.toFixed(1)} s…`;
      });
      const res = await job.promise;
      download(encodeWav([res.left, res.right], res.sampleRate), `${(app.doc.name || 'earworm').replace(/[^\w.-]+/g, '_')}.wav`);
      $('engineStatus').textContent = `Mix saved. ${res.realtime.toFixed(1)}× realtime, peak ${res.peak.toFixed(2)}.`;
      acted('wav');
    } catch (e) {
      app.fail(e instanceof Error ? e.message : String(e));
    } finally {
      renderer.terminate();
      button.disabled = false;
    }
  });
  $<HTMLInputElement>('importFile').addEventListener('change', async () => {
    const file = $<HTMLInputElement>('importFile').files?.[0];
    if (!file) return;
    try {
      const got = await importProject(new Uint8Array(await file.arrayBuffer()));
      got.doc.id = newId('proj');
      if (!got.doc.name) got.doc.name = file.name.replace(/\.zip$/i, '');
      await session.adopt(got.doc);
      await writeBlobs(got.blobs, got.doc);
      $('capStatus').textContent = historyResets(got.savedSchema)
        ? `Imported ${got.doc.name}. It was saved by an earlier version, so its undo history starts fresh.`
        : `Imported ${got.doc.name}.`;
    } catch (e) {
      app.fail(e instanceof Error ? e.message : String(e));
    }
  });
  btn('cleanBtn').addEventListener('click', async () => {
    const plan = cleanupPlan(app.doc, app.store.history.toJSON());
    const drop = unusedRefs(await session.blobs.list(), plan);
    for (const ref of drop) await session.blobs.del(ref);
    if (plan.takeIds.length) app.run(() => app.store.dispatch('forgetTakes', { ids: plan.takeIds }));
    $('capStatus').textContent = drop.length || plan.takeIds.length
      ? `Removed ${plan.takeIds.length} unused take${plan.takeIds.length === 1 ? '' : 's'} and ${drop.length} file${drop.length === 1 ? '' : 's'}. Undo can restore a take’s place in the song, not audio that was deleted.`
      : 'Nothing unused. Takes still named by Undo stay.';
  });

  // ---- learn and demos

  async function openDemo(id: string): Promise<void> {
    const demo = demoById(id);
    if (!demo) return;
    const doc = structuredClone(demo.doc);
    doc.id = newId('proj');
    await session.adopt(doc);
    const blobs = new Map<string, Float32Array>();
    for (const [ref, data] of Object.entries(demo.blobs)) blobs.set(ref, data.slice());
    await writeBlobs(blobs, doc);
    app.ui.sectionId = doc.sections[0]?.id ?? '';
    app.ui.layerId = doc.layers[0]?.id ?? '';
    app.ui.changed();
  }

  const lessonList = $('lessonList');
  for (const lesson of LESSONS) {
    const b = document.createElement('button');
    b.type = 'button';
    b.textContent = lesson.title;
    b.title = lesson.blurb;
    b.addEventListener('click', () => {
      $<HTMLDialogElement>('helpDialog').close();
      void (async () => {
        if (lesson.demo) await openDemo(lesson.demo);
        tutorial.start(lesson);
        paintTour();
      })();
    });
    lessonList.append(b);
  }
  const demoList = $('demoList');
  for (const demo of DEMOS) {
    const b = document.createElement('button');
    b.type = 'button';
    b.textContent = demo.name;
    b.title = demo.blurb;
    b.addEventListener('click', () => { $<HTMLDialogElement>('helpDialog').close(); void openDemo(demo.id); });
    demoList.append(b);
  }
  btn('helpBtn').addEventListener('click', () => $<HTMLDialogElement>('helpDialog').showModal());
  btn('testsBtn').addEventListener('click', () => $<HTMLDialogElement>('testsDialog').showModal());
  btn('tourNext').addEventListener('click', () => {
    if (!tutorial.step) return;
    if (tutorial.step.advance === 'next') tutorial.event('next');
    else { tutorial.index += 1; if (tutorial.lesson && tutorial.index >= tutorial.lesson.steps.length) tutorial.stop(); }
    paintTour();
  });
  btn('tourClose').addEventListener('click', () => { tutorial.stop(); paintTour(); });
  for (const id of PLACES) {
    btn(`tab-${id}`).addEventListener('click', () => showPanel(id));
  }
  btn('setupBtn').addEventListener('click', () => $<HTMLDialogElement>('setupDialog').showModal());
  btn('keysBtn').addEventListener('click', () => $<HTMLDialogElement>('keysDialog').showModal());
  btn('nextStep').addEventListener('click', () => { const act = nextAction; setNext('', null); act?.(); });

  // ---- theme: System follows prefers-color-scheme; Light and Dark are kept in this browser.
  const THEME_KEY = 'earworm.theme';
  const retheme = () => { ribbonColors = themeColors(); timeline.retheme(); synth.render(); };
  for (const radio of document.querySelectorAll<HTMLInputElement>('input[name="theme"]')) {
    radio.checked = radio.value === (document.documentElement.dataset.theme ?? 'system');
    radio.addEventListener('change', () => {
      if (!radio.checked) return;
      const t = radio.value;
      if (t === 'light' || t === 'dark') document.documentElement.dataset.theme = t;
      else delete document.documentElement.dataset.theme;
      try { if (t === 'light' || t === 'dark') localStorage.setItem(THEME_KEY, t); else localStorage.removeItem(THEME_KEY); } catch { /* storage off: the choice lasts this visit */ }
      retheme();
    });
  }
  window.matchMedia('(prefers-color-scheme: light)').addEventListener('change', retheme);

  // ---- start

  let starting = false;
  async function start(withMic: boolean): Promise<void> {
    if (starting || app.host) return;
    starting = true;
    btn('start').disabled = true;
    btn('startMic').disabled = true;
    let ctx: AudioContext | null = null;
    try {
      ctx = new AudioContext({ latencyHint: 'interactive' });
      app.ctx = ctx;
      $('engineStatus').textContent = 'Loading the engine…';
      const host = await KlontHost.create(ctx, { input: true, forceFallback: wantsFallback() });
      app.analysis = await Analysis.create();
      host.setBudget(0);
      app.host = host;
      app.sync.host = host;
      app.sync.analysis = app.analysis;
      app.sync.reset();
      await app.sync.request(app.doc);
      pushMetro(app.doc);
      if (withMic) await capture.enableMic();
      else { btn('enableMic').hidden = false; $('capStatus').textContent = 'No mic. Enable mic to capture.'; }
      const iso = typeof crossOriginIsolated === 'boolean' && crossOriginIsolated ? '' : ' This page is not cross-origin isolated.';
      $('engineStatus').textContent = `${host.mode === 'shared' ? 'Shared' : 'Fallback'} audio, ${ctx.sampleRate} Hz.${iso}`;
      acted('start');
      app.refreshAll();
    } catch (e) {
      const err = e as DOMException;
      const mic = err?.name === 'NotAllowedError' || err?.name === 'NotFoundError';
      app.fail(mic ? `No microphone: ${err.message || err.name}. Start without the mic for playback.` : e instanceof Error ? e.message : String(e));
      if (!app.host) { void ctx?.close(); app.ctx = null; app.stream?.getTracks().forEach((t) => t.stop()); app.stream = null; }
      $('engineStatus').textContent = 'Not started.';
      btn('start').disabled = false;
      btn('startMic').disabled = false;
    } finally {
      starting = false;
    }
  }
  btn('start').addEventListener('click', () => { void start(false); });
  btn('startMic').addEventListener('click', () => { void start(true); });
  btn('enableMic').addEventListener('click', async () => {
    try { await capture.enableMic(); btn('enableMic').hidden = true; app.clearError(); app.refreshAll(); }
    catch (e) { app.fail(e instanceof Error ? e.message : String(e)); }
  });

  let lastMeters = 0;
  const tick = () => {
    requestAnimationFrame(tick);
    capture.poll();
    timeline.frame();
    document.body.classList.toggle('capturing', capture.holding || capture.recording);
    const recLabel = capture.recording ? '● Recording' : '● Record';
    if (btn('record').textContent !== recLabel) btn('record').textContent = recLabel;
    const singing = layer();
    const stateNow = singing ? app.ui.editState(singing) : null;
    const metersNow = app.readMeters();
    const macroNow = (i: number) => metersNow ? metersNow[M_MACRO_LIVE + i] : (stateNow?.macros[i] ?? 0.5);
    glyph?.draw({
      bright: macroNow(0), grit: macroNow(3), motion: macroNow(2), space: macroNow(4),
      noise: stateNow && singing ? stateVocab(singing, stateNow)[P['src.noise_mix']] : 0.1,
    }, reduceMotion);
    const origin = capture.originFrame();
    if (ribbon && origin != null && app.host) {
      const total = app.host.framesTotal();
      const n = Math.min(800, Math.max(0, total - origin));
      const raw = app.host.readFrames(total - n, n);
      ribbonFrames.length = 0;
      for (let i = 0; i < n; i++) ribbonFrames.push(recordFromFrame(raw.subarray(i * 25, i * 25 + 25)));
      ribbon.draw({ count: ribbonFrames.length, get: (back) => ribbonFrames[ribbonFrames.length - 1 - back] ?? null },
        singing?.tune.scale ?? 0, singing?.tune.root ?? 0, ribbonColors, true);
      ribbonCanvas!.parentElement!.classList.add('drawn');
    }
    const m = app.readMeters();
    synth.frame(m);
    const secNow = section();
    if (m && secNow) {
      const playing = m[M_PLAYING] === 1;
      btn('play').setAttribute('aria-pressed', String(playing));
      btn('record').setAttribute('aria-pressed', String(capture.recording));
      btn('play').textContent = playing ? '■ Playing' : '▶ Play';
      const pos = m[M_SECTION_POS];
      const secPlaying = app.doc.sections[Math.round(m[M_SECTION])] ?? secNow;
      $('posRead').textContent = playing ? positionLabel(secPlaying, pos) : positionLabel(secNow, 0);
      // The pattern grid is written time: under Meter 1's pattern the playing row skips dropped
      // beats and goes back for repeated ones.
      const row = playing ? playRow(secPlaying, writtenAt(app.doc, secPlaying, pos)) : -1;
      if (playing && playedBar(app.doc, secPlaying) !== metroBar) pushMetro(app.doc, secPlaying);
      const beat = m[M_METRO_BEAT];
      $('posRead').title = beat >= 0 ? `Beat ${Math.round(beat) + 1} of the bar` : '';
      document.querySelectorAll('#pattern tbody tr').forEach((tr) => {
        tr.classList.toggle('play', tr instanceof HTMLElement && Number(tr.dataset.row) === row);
      });
      if (performance.now() - lastMeters > 200) {
        lastMeters = performance.now();
        paintAudioChip();
        document.querySelectorAll('#scopes .scope').forEach((el, i) => {
          const voices = m[M_LAYER_VOICES + i] ?? 0;
          const bar = el.querySelector('.bar > span');
          if (bar instanceof HTMLElement) bar.style.width = `${Math.min(100, voices * 25)}%`;
          const n = el.querySelector('.voices');
          if (n) n.textContent = String(Math.round(voices));
          el.setAttribute('aria-valuenow', String(Math.round(voices)));
        });
      }
    }
  };

  placeBubbles();
  await session.init();
  if (session.historyReset) $('capStatus').textContent = UPGRADED;
  app.refreshAll();
  showPanel('pattern');
  requestAnimationFrame(tick);
  let storageSnap = { usage: 0, quota: 0 };
  const paintStorage = () => {
    const el = $('storageStatus');
    const n = db.pending;
    const save = n === 0 ? 'Saved' : `Saving ${n}`;
    const s = storageSnap;
    const full = s.quota > 0 && s.usage / s.quota >= 0.8;
    const use = s.quota > 0 ? `${(s.usage / 1e6).toFixed(1)} / ${(s.quota / 1e6).toFixed(0)} MB` : '';
    el.textContent = [save, use, full ? 'nearly full' : ''].filter(Boolean).join(' · ');
    el.classList.toggle('warn', full);
  };
  const readStorage = () => storageEstimate().then((s) => { storageSnap = s; paintStorage(); }).catch(() => paintStorage());
  void readStorage();
  window.setInterval(() => { paintStorage(); }, 400);
  window.setInterval(() => { void readStorage(); }, 10000);
}

main().catch((e) => {
  const el = document.getElementById('error');
  if (el) { el.hidden = false; el.textContent = `The page could not start: ${e instanceof Error ? e.message : String(e)}`; }
});
