// The Sound panel's Patch and Mod views (docs/synth.md "Interface sketch", stage S2).
//
// Patch: a card per module, envelope, LFO, layer source and channel effect of the layer's patch,
// with a slider for every value of the state being edited. A ring under a slider shows how far its
// routes can move it, a thumb shows where the sound has it now, and badges list the routes into it.
// Modulate: pick a source, then press + on any slider to route that source to it.
// Mod: the matrix (every route of the kit slot's voice patch and of the layer, with its live
// output), the shaper editor for the selected route, the sung-feature meters and Listen.
//
// Stage S3: the Patch view also edits the structure. A bar on top shows the patch's cost against
// the layer's budget and the voice count that follows, and adds modules, envelopes, LFOs and
// channel effects; each card can be rewired (its inputs, an oscillator's phase input, the voice
// output), moved in the rack and removed; a channel effect can be bypassed. Layer sources (shaped
// noise, grains) keep their place.

import { ALL_DESCS } from '../gen/modules';
import { M_LAYER_SRC, M_LIVE, M_LISTEN, M_ROUTES, M_STORED, MSG_LISTEN } from '../m1/protocol';
import type { App } from '../m3/app';
import { applyOps } from '../project/patch';
import { commands } from '../project/commands';
import type { Layer, Project } from '../project/types';
import { CAPS, FEATURES, LAYER_SOURCES, SOURCE_TYPES, VOICE_SOURCES, isBipolarSource, isLayerSource, layerBudget, layerCost, layoutOf, maxVoiceCost, moduleKeys, voiceCount,
  type ChannelFxType, type Layout, type Patch, type Port, type Route, type VoiceModuleType } from '../synth/patch';
import { classicPatch } from '../synth/classic';
import { allRoutes, amountKey, amountNorm, findRoute, kitPrefix, routesInto, type RouteFields, type Scope } from '../synth/routes';
import { toNorm, toReal } from '../synth/values';
import type { Advance } from './tutorials';

export type SoundView = 'simple' | 'patch' | 'mod';

const FEATURE_NAMES: Record<string, string> = {
  pitch: 'pitch', volume: 'volume', openness: 'openness', breath: 'breath', hardness: 'hardness', onset: 'onset',
  voicing: 'voicing', vowel: 'vowel', intensity: 'intensity',
};
const NOTE_NAMES: Record<string, string> = {
  pitch: 'Note pitch', velocity: 'Velocity', hardness: 'Note hardness', attack: 'Hit attack', burst: 'Hit burst', bend: 'Hit bend',
  hold: 'Hit hold', vowel: 'Hit vowel', tone: 'Hit tone', endClick: 'Hit end click',
};

/** A source's name in the interface. */
export function sourceName(s: string, patch?: Patch): string {
  const m = /^macro([1-8])$/.exec(s);
  if (m) return `Macro ${m[1]}${patch?.macroNames[Number(m[1]) - 1] ? ` (${patch.macroNames[Number(m[1]) - 1]})` : ''}`;
  if (s.startsWith('note.')) return NOTE_NAMES[s.slice(5)] ?? s;
  if (s.startsWith('voice.')) return `Sung ${FEATURE_NAMES[s.slice(6)]}, per note`;
  if (/^layer\.lfo[12]$/.test(s)) return `Layer LFO ${s.slice(-1)}`;
  if (s.startsWith('layer.')) return `Sung ${FEATURE_NAMES[s.slice(6)]}, whole layer`;
  if (/^env\d$/.test(s)) return `Envelope ${s.slice(3)}`;
  if (/^lfo\d$/.test(s)) return `LFO ${s.slice(3)}`;
  return { wheel: 'Mod wheel', bend: 'Pitch bend', pressure: 'Pressure' }[s] ?? s;
}

const WAVES = ['sine', 'triangle', 'saw', 'square', 'random steps', 'drift'];
const ENV_MODES = ['ADSR', 'decay', 'one-shot'];

/** A value's reading in real units. */
function readout(desc: number, norm: number): string {
  const d = ALL_DESCS[desc];
  const r = toReal(desc, norm);
  if (d.id === 'lfo.wave') return WAVES[Math.round(r * 5)] ?? String(r);
  if (d.id === 'env.mode') return ENV_MODES[Math.round(r)] ?? String(r);
  const a = Math.abs(r);
  const n = a >= 1000 ? r.toFixed(0) : a >= 100 ? r.toFixed(0) : a >= 10 ? r.toFixed(1) : a >= 1 ? r.toFixed(2) : r.toFixed(3);
  return d.unit ? `${n} ${d.unit}` : n;
}

/** "env1.attack" -> "attack"; "k2.filter1.cutoff" -> "filter1 cutoff". */
function keyLabel(key: string): string {
  return key.replace(/^k\d\./, '').replace(/^route\./, 'amount of ').replace(/\./g, ' ').replace(/_/g, ' ');
}

const SCOPE_METER = (scope: Scope, i: number) => (scope === 'layer' ? 192 + i : scope * 48 + i);
const LS_FEATURE = 8 + 3 + 2;

/** A card of the Patch view; `module`, `mod` and `fx` name what its structure controls edit. */
interface Card { title: string; keys: string[]; env?: string; module?: string; mod?: string; fx?: string }

/** What one layer may spend (docs/synth.md "Rules for the graph"), from the classic patches. */
let budget = 0;
const layerBudgetNow = () => budget || (budget = layerBudget([classicPatch('melody'), classicPatch('beat')]));

const MODULE_NAMES: Record<VoiceModuleType, string> = {
  partials: 'partial bank', osc: 'oscillator', noise: 'noise', sampler: 'sampler (plays the layer\'s sound source)', click: 'click',
  filter: 'filter', amp: 'amp', fold: 'fold', clang: 'clang', comb: 'comb',
};
const FX_NAMES: Record<ChannelFxType, string> = { crush: 'crush', tape: 'tape', cfilter: 'filter', delay: 'delay', reverb: 'reverb' };

export class SynthPanel {
  view: SoundView = 'simple';
  /** The source picked under Modulate, or ''. */
  picked = '';
  /** Kit slot (voice patch) the views show on a beat layer. */
  slot = 0;
  /** The route the shaper editor shows. */
  selected: { scope: Scope; id: string } | null = null;
  private listen = new Map<string, boolean>();
  private sentListen = new Map<number, boolean>();
  private selectedSent = -1;
  private hostSeen: unknown = null;
  private built: { patch: Patch; slot: number } | null = null;
  private knobs = new Map<string, { input: HTMLInputElement; out: HTMLOutputElement; ring: HTMLElement; live: HTMLElement; badges: HTMLElement; to: HTMLButtonElement }>();
  private routeRows: { scope: Scope; index: number; bar: HTMLElement }[] = [];
  private modBuilt: Patch | null = null;
  private modSlot = -1;
  private modSelected = '';

  constructor(
    private app: App,
    private layer: () => Layer | undefined,
    private acted: (k: Advance) => void,
    private els: { patch: HTMLElement; mod: HTMLElement; pick: HTMLSelectElement; pickBox: HTMLElement; views: HTMLButtonElement[]; simple: HTMLElement },
  ) {
    els.views.forEach((b) => b.addEventListener('click', () => this.setView(b.dataset.view as SoundView)));
    els.pick.addEventListener('change', () => {
      this.picked = els.pick.value;
      els.patch.classList.toggle('picking', this.picked !== '');
      if (this.picked) this.acted('pick');
      this.paintKnobTargets();
    });
  }

  setView(v: SoundView): void {
    this.view = v;
    this.els.views.forEach((b) => b.setAttribute('aria-selected', String(b.dataset.view === v)));
    this.els.simple.hidden = v !== 'simple';
    this.els.patch.hidden = v !== 'patch';
    this.els.mod.hidden = v !== 'mod';
    this.els.pickBox.hidden = v === 'simple';
    this.render();
    if (v !== 'simple') this.acted('view');
  }

  /** Re-read the document. Cheap when the patch and slot have not changed. */
  render(): void {
    const l = this.layer();
    if (!l) return;
    this.syncEngine(l);
    this.fillPick(l);
    if (this.slot >= l.patch.voices.length) this.slot = 0;
    if (this.view === 'patch') this.renderPatch(l);
    if (this.view === 'mod') this.renderMod(l);
  }

  // ---- engine messages that are not in the document: the selected layer (meters) and Listen

  private syncEngine(l: Layer): void {
    const host = this.app.host;
    if (!host) return;
    if (host !== this.hostSeen) {
      this.hostSeen = host;
      this.sentListen.clear();
      this.selectedSent = -1;
    }
    const i = this.app.doc.layers.indexOf(l);
    if (i !== this.selectedSent) {
      host.selectLayer(i);
      this.selectedSent = i;
    }
    this.app.doc.layers.forEach((x, k) => {
      const on = this.listen.get(x.id) ?? false;
      if (this.sentListen.get(k) !== on) {
        host.send(MSG_LISTEN, k, 0, 0, on ? 1 : 0);
        this.sentListen.set(k, on);
      }
    });
  }

  private fillPick(l: Layer): void {
    const sel = this.els.pick;
    const vp = l.patch.voices[this.slot] ?? l.patch.voices[0];
    const want = ['', ...vp.mods.map((m) => m.id), ...VOICE_SOURCES.filter((s) => l.kind === 'beat' || !s.startsWith('note.') || !/attack|burst|bend|hold|vowel|tone|endClick/.test(s)), ...LAYER_SOURCES];
    const sig = want.join(',');
    if (sel.dataset.sig !== sig) {
      sel.dataset.sig = sig;
      sel.replaceChildren(...want.map((s) => {
        const o = document.createElement('option');
        o.value = s;
        o.textContent = s ? sourceName(s, l.patch) : '(none)';
        return o;
      }));
      sel.value = want.includes(this.picked) ? this.picked : '';
      this.picked = sel.value;
    }
  }

  // ---- Patch view

  private cards(l: Layer): Card[] {
    const p = l.patch;
    const lay = layoutOf(p);
    const pre = kitPrefix(p, this.slot);
    const vp = p.voices[this.slot];
    const keysOf = (id: string, type: string) => moduleKeys(id, type).map((x) => pre + x.local).filter((k) => lay.index.has(k));
    const out: Card[] = [];
    vp.modules.forEach((m, i) => {
      out.push({ title: `${i + 1}. ${m.id}`, keys: keysOf(m.id, m.type), module: m.id });
    });
    for (const m of vp.mods) out.push({ title: m.id, keys: keysOf(m.id, m.type), env: m.type === 'env' ? m.id : undefined, mod: m.id });
    out.push({ title: 'voicing swap', keys: [pre + 'voicing.depth'] });
    for (const s of p.layerSources.filter((x) => x.voice === this.slot)) out.push({ title: `${s.id} (driven by ${s.drive})`, keys: keysOf(s.id, s.type) });
    for (const fx of p.channel) out.push({ title: `channel: ${fx.id}${fx.bypass ? ' (bypassed)' : ''}`, keys: moduleKeys(fx.id, fx.type).map((x) => x.local), fx: fx.id });
    out.push({ title: 'channel: width', keys: ['width'] });
    for (const m of p.lfos) out.push({ title: m.id, keys: moduleKeys(m.id, m.type).map((x) => x.local) });
    return out;
  }

  private renderPatch(l: Layer): void {
    const box = this.els.patch;
    const p = l.patch;
    if (!this.built || this.built.patch !== p || this.built.slot !== this.slot) {
      this.built = { patch: p, slot: this.slot };
      this.knobs.clear();
      box.replaceChildren();
      box.append(this.slotPicker(l), this.structureBar(l));
      for (const c of this.cards(l)) box.append(this.card(l, c));
      const hint = document.createElement('p');
      hint.className = 'hint';
      hint.textContent = 'The bar under a slider shows how far its routes can move it; the mark shows where the sound has it now. Pick a source under Modulate, then press + beside a slider to route it there.';
      box.append(hint);
    }
    const st = this.app.ui.editState(l);
    const lay = layoutOf(p);
    for (const [key, k] of this.knobs) {
      const i = lay.index.get(key)!;
      const v = st.values[key] ?? lay.init[i];
      if (document.activeElement !== k.input) k.input.value = String(Math.round(v * 1000));
      k.out.value = readout(lay.descs[i], v);
      this.paintRing(l, lay, key, v, k.ring);
      const into = routesInto(p, key);
      k.badges.replaceChildren(...into.map(({ route, scope }) => {
        const b = document.createElement('button');
        b.type = 'button';
        b.className = 'badge';
        b.textContent = `⟲ ${sourceName(route.source, p)}`;
        b.title = 'Open this route in the Mod view';
        b.addEventListener('click', () => { this.selected = { scope, id: route.id }; this.setView('mod'); });
        return b;
      }));
    }
    this.paintKnobTargets();
  }

  private slotPicker(l: Layer): HTMLElement {
    const row = document.createElement('div');
    row.className = 'row';
    if (l.patch.voices.length > 1) {
      const lab = document.createElement('label');
      lab.textContent = 'Kit slot ';
      const s = document.createElement('select');
      l.patch.voices.forEach((_, k) => {
        const o = document.createElement('option');
        o.value = String(k);
        o.textContent = String(k + 1);
        s.append(o);
      });
      s.value = String(this.slot);
      s.addEventListener('change', () => { this.slot = Number(s.value); this.built = null; this.modBuilt = null; this.render(); });
      lab.append(s);
      row.append(lab);
    }
    return row;
  }

  private card(l: Layer, c: Card): HTMLElement {
    const f = document.createElement('fieldset');
    f.className = 'card';
    f.dataset.tour = c.title.replace(/^\d+\. /, '').replace(/ .*$/, '');
    const lg = document.createElement('legend');
    lg.textContent = c.title;
    f.append(lg);
    const bands = c.keys.filter((k) => /\.(tone_bands|bands)\.\d+$/.test(k));
    const plain = c.keys.filter((k) => !bands.includes(k));
    for (const k of plain) f.append(this.knob(l, k));
    if (bands.length) {
      const d = document.createElement('details');
      const s = document.createElement('summary');
      s.textContent = `${keyLabel(bands[0]).replace(/ 0$/, '')} (${bands.length})`;
      d.append(s, ...bands.map((k) => this.knob(l, k)));
      f.append(d);
    }
    const wiring = this.wiring(l, c);
    if (wiring) f.append(wiring);
    if (c.env) {
      const lab = document.createElement('label');
      const box = document.createElement('input');
      box.type = 'checkbox';
      box.checked = l.patch.voices[this.slot].mods.find((m) => m.id === c.env)?.trigger === 'onset';
      box.addEventListener('change', () => {
        const cur = this.layer();
        if (cur) this.app.run(() => this.app.store.dispatch('setEnvTrigger', { layer: cur.id, slot: this.slot, env: c.env!, trigger: box.checked ? 'onset' : 'note' }));
      });
      lab.append(box, ' Also restart at each sung onset');
      f.append(lab);
    }
    return f;
  }

  private knob(l: Layer, key: string): HTMLElement {
    const row = document.createElement('div');
    row.className = 'knob';
    row.dataset.key = key;
    const name = document.createElement('span');
    name.className = 'name';
    name.textContent = keyLabel(key).replace(/^\S+ /, '');
    name.title = key;
    const track = document.createElement('span');
    track.className = 'track';
    const input = document.createElement('input');
    input.type = 'range';
    input.min = '0';
    input.max = '1000';
    input.step = '1';
    input.setAttribute('aria-label', keyLabel(key));
    const ring = document.createElement('span');
    ring.className = 'ring';
    const live = document.createElement('span');
    live.className = 'live';
    track.append(input, ring, live);
    const out = document.createElement('output');
    const to = document.createElement('button');
    to.type = 'button';
    to.className = 'to';
    to.textContent = '+';
    to.addEventListener('click', () => this.routeTo(key));
    const badges = document.createElement('span');
    badges.className = 'badges';
    const args = (doc: Project) => {
      const cur = doc.layers.find((x) => x.id === l.id)!;
      return { layer: cur.id, state: this.app.ui.editState(cur).id, values: { [key]: Number(input.value) / 1000 } };
    };
    input.addEventListener('input', () => this.app.previewDoc((d) => applyOps(d, commands.setValues(d, args(d))).doc));
    input.addEventListener('change', () => { this.app.runRestoring(() => this.app.store.dispatch('setValues', args(this.app.doc))); });
    row.append(name, track, out, to, badges);
    this.knobs.set(key, { input, out, ring, live, badges, to });
    return row;
  }

  /** Show + only on sliders the picked source can reach (a per-voice source stays in its voice patch). */
  private paintKnobTargets(): void {
    const l = this.layer();
    if (!l) return;
    const lay = layoutOf(l.patch);
    const [lo, hi] = lay.ranges[this.slot] ?? [0, 0];
    for (const [key, k] of this.knobs) {
      const i = lay.index.get(key) ?? -1;
      const ok = this.picked !== '' && (isLayerSource(this.picked) || (i >= lo && i < hi));
      k.to.hidden = !ok;
      k.to.setAttribute('aria-label', `Route ${sourceName(this.picked, l.patch)} to ${keyLabel(key)}`);
    }
  }

  private routeTo(key: string): void {
    const l = this.layer();
    if (!l || !this.picked) return;
    const source = this.picked;
    const before = new Set(allRoutes(l.patch).map((x) => `${x.scope}/${x.route.id}`));
    const ok = this.app.run(() => this.app.store.dispatch('addRoute', { layer: l.id, slot: this.slot, route: { source, target: key } }));
    if (!ok) return;
    const now = this.layer()!;
    const added = allRoutes(now.patch).find((x) => !before.has(`${x.scope}/${x.route.id}`));
    if (added) this.selected = { scope: added.scope, id: added.route.id };
    this.acted('route');
  }

  /** The reach of the routes into `key` as a band under the slider (normalized). */
  private paintRing(l: Layer, lay: Layout, key: string, v: number, ring: HTMLElement): void {
    const into = routesInto(l.patch, key);
    if (!into.length) { ring.hidden = true; return; }
    const st = this.app.ui.editState(l);
    const desc = lay.descs[lay.index.get(key)!];
    let lo = v, hi = v;
    for (const { route, scope } of into) {
      const ak = amountKey(l.patch, scope, route.id);
      const ai = lay.index.get(ak);
      if (ai === undefined) continue;
      const amt = toReal(lay.descs[ai], st.values[ak] ?? lay.init[ai]) * (route.unit ?? 1) * (route.group ? (route.group === 'follow' ? l.follow : l.depth[route.group]) : 1);
      const [c0, c1] = route.polarity === 'up' ? [0, 1] : route.polarity === 'down' ? [-1, 0] : [-1, 1];
      if (route.mode === 'pull') {
        const a = toNorm(desc, route.inLo), b = toNorm(desc, route.inHi);
        lo = Math.min(lo, v + Math.abs(amt) * (Math.min(a, b) - v));
        hi = Math.max(hi, v + Math.abs(amt) * (Math.max(a, b) - v));
      } else if (route.mode === 'scale') {
        lo = Math.min(lo, v * (1 + Math.min(amt * c0, amt * c1)));
        hi = Math.max(hi, v * (1 + Math.max(amt * c0, amt * c1)));
      } else {
        const d = ALL_DESCS[desc];
        const k = route.target.startsWith('oct:') && d.scale === 'log' ? amt / Math.log2(d.max / d.min) : amt;
        lo += Math.min(k * c0, k * c1);
        hi += Math.max(k * c0, k * c1);
      }
    }
    lo = Math.max(0, lo);
    hi = Math.min(1, hi);
    ring.hidden = false;
    ring.style.left = `${lo * 100}%`;
    ring.style.width = `${Math.max(0.5, (hi - lo) * 100)}%`;
  }

  // ---- structure (stage S3)

  private edit(name: Parameters<App['store']['dispatch']>[0], args: Record<string, unknown>): void {
    const l = this.layer();
    if (l) this.app.run(() => this.app.store.dispatch(name, { layer: l.id, ...args } as never));
  }

  /** The cost meter and the add menus. */
  private structureBar(l: Layer): HTMLElement {
    const p = l.patch;
    const f = document.createElement('fieldset');
    f.className = 'structure';
    f.dataset.tour = 'structure';
    const lg = document.createElement('legend');
    lg.textContent = 'Structure';
    const b = layerBudgetNow();
    const voices = voiceCount(p, b);
    const share = (layerCost(p) + 8 * maxVoiceCost(p)) / b;
    const cost = document.createElement('div');
    cost.className = 'row cost';
    const label = document.createElement('label');
    label.textContent = 'Cost ';
    const meter = document.createElement('meter');
    meter.min = 0;
    meter.max = 2;
    meter.low = 1;
    meter.high = 1.5;
    meter.optimum = 0.5;
    meter.value = share;
    meter.setAttribute('aria-label', 'Cost of this patch at 8 voices, against what a layer may spend');
    label.append(meter);
    const text = document.createElement('span');
    text.textContent = `${Math.round(share * 100)}% · ${voices} voice${voices === 1 ? '' : 's'}`;
    text.title = 'A layer may spend what the classic patch spends with 8 voices. A heavier patch plays fewer voices at once.';
    cost.append(label, text);
    const add = document.createElement('div');
    add.className = 'row';
    const vp = p.voices[this.slot];
    const menu = (aria: string, first: string, items: [string, string, boolean][], pick: (v: string) => void) => {
      const s = document.createElement('select');
      s.setAttribute('aria-label', aria);
      const o0 = document.createElement('option');
      o0.value = '';
      o0.textContent = first;
      s.append(o0, ...items.map(([v, t, ok]) => {
        const o = document.createElement('option');
        o.value = v;
        o.textContent = t;
        o.disabled = !ok;
        return o;
      }));
      s.addEventListener('change', () => { if (s.value) pick(s.value); s.value = ''; });
      return s;
    };
    const room = vp.modules.length < CAPS.modules;
    add.append(menu('Add a module to the voice', '+ module', (Object.keys(CAPS.voice) as VoiceModuleType[]).map((t) => [t, MODULE_NAMES[t], room && vp.modules.filter((m) => m.type === t).length < CAPS.voice[t]]),
      (t) => this.edit('addModule', { slot: this.slot, type: t })));
    const envs = vp.mods.filter((m) => m.type === 'env').length;
    add.append(menu('Add an envelope or LFO to the voice', '+ envelope or LFO', [['env', 'envelope', envs < CAPS.envs], ['lfo', 'LFO', vp.mods.length - envs < CAPS.lfos]],
      (t) => this.edit('addMod', { slot: this.slot, type: t })));
    add.append(menu('Add an effect to the channel', '+ channel effect', (Object.keys(FX_NAMES) as ChannelFxType[]).map((t) => [t, FX_NAMES[t],
      p.channel.length < CAPS.channel && (t !== 'delay' || p.channel.filter((x) => x.type === 'delay').length < CAPS.delay) && (t !== 'reverb' || !p.channel.some((x) => x.type === 'reverb'))]),
    (t) => this.edit('addChannelFx', { type: t })));
    f.append(lg, cost, add);
    return f;
  }

  /** A card's structure controls: inputs, phase input and output (modules), bypass (effects), move
   *  and remove. */
  private wiring(l: Layer, c: Card): HTMLElement | null {
    const p = l.patch;
    const vp = p.voices[this.slot];
    const row = document.createElement('div');
    row.className = 'row wiring';
    const button = (text: string, aria: string, fn: () => void, off = false) => {
      const b = document.createElement('button');
      b.type = 'button';
      b.textContent = text;
      b.setAttribute('aria-label', aria);
      b.disabled = off;
      b.addEventListener('click', fn);
      return b;
    };
    const check = (text: string, on: boolean, fn: (on: boolean) => void) => {
      const lab = document.createElement('label');
      const box = document.createElement('input');
      box.type = 'checkbox';
      box.checked = on;
      box.addEventListener('change', () => fn(box.checked));
      lab.append(box, ` ${text}`);
      return lab;
    };
    if (c.module) {
      const id = c.module;
      const m = vp.modules.find((x) => x.id === id)!;
      const inputs = (port: Port) => {
        const g = document.createElement('span');
        g.className = 'inputs';
        g.setAttribute('role', 'group');
        g.setAttribute('aria-label', `${port === 'pm' ? 'Phase input' : 'Input'} of ${id}`);
        g.append(port === 'pm' ? 'phase in:' : 'in:');
        const from = vp.cables.filter((x) => x.to === id && x.port === port).map((x) => x.from);
        for (const o of vp.modules.filter((x) => x.id !== id)) {
          g.append(check(o.id, from.includes(o.id), (on) => this.edit('setInputs', { slot: this.slot, id, port, from: on ? [...from, o.id] : from.filter((x) => x !== o.id) })));
        }
        return g;
      };
      if (!SOURCE_TYPES.includes(m.type)) row.append(inputs('in'));
      if (m.type === 'osc') row.append(inputs('pm'));
      row.append(check('to output', vp.out.includes(id), (on) => this.edit('setOutputs', { slot: this.slot, ids: on ? [...vp.out, id] : vp.out.filter((x) => x !== id) })));
      const i = vp.modules.findIndex((x) => x.id === id);
      row.append(
        button('↑', `Move ${id} up the rack`, () => this.edit('moveModule', { slot: this.slot, id, to: i - 1 }), i === 0),
        button('↓', `Move ${id} down the rack`, () => this.edit('moveModule', { slot: this.slot, id, to: i + 1 }), i === vp.modules.length - 1),
        button('Remove', `Remove ${id} and the routes into it`, () => this.edit('removeModule', { slot: this.slot, id }), vp.modules.length === 1),
      );
      if (m.type === 'sampler' && !l.influence?.grain) {
        const hint = document.createElement('span');
        hint.className = 'dim';
        hint.textContent = 'Silent until the layer has a sound source (Influence: File or Listen).';
        row.append(hint);
      }
      return row;
    }
    if (c.mod) {
      const id = c.mod;
      if (vp.mods[0]?.id === id) return null;
      row.append(button('Remove', `Remove ${id} and its routes`, () => this.edit('removeMod', { slot: this.slot, id })));
      return row;
    }
    if (c.fx) {
      const id = c.fx;
      const i = p.channel.findIndex((x) => x.id === id);
      const fx = p.channel[i];
      row.append(
        check('bypass', fx.bypass, (on) => this.edit('setBypass', { id, bypass: on })),
        button('←', `Move ${id} earlier in the chain`, () => this.edit('moveChannelFx', { id, to: i - 1 }), i === 0),
        button('→', `Move ${id} later in the chain`, () => this.edit('moveChannelFx', { id, to: i + 1 }), i === p.channel.length - 1),
        button('Remove', `Remove ${id} and the routes into it`, () => this.edit('removeChannelFx', { id })),
      );
      return row;
    }
    return null;
  }

  // ---- Mod view

  private renderMod(l: Layer): void {
    const box = this.els.mod;
    const p = l.patch;
    const sel = this.selected ? `${this.selected.scope}/${this.selected.id}` : '';
    if (this.modBuilt !== p || this.modSlot !== this.slot || this.modSelected !== sel || box.dataset.state !== this.app.ui.editState(l).id) {
      this.modBuilt = p;
      this.modSlot = this.slot;
      this.modSelected = sel;
      box.dataset.state = this.app.ui.editState(l).id;
      this.routeRows = [];
      const f = this.selected ? findRoute(p, this.selected.scope, this.selected.id) : null;
      if (!f) this.selected = null;
      this.curve = null;
      box.replaceChildren(this.slotPicker(l), this.meters(l));
      if (f && this.selected) box.append(this.shaper(l, this.selected.scope, f));
      box.append(this.adder(l), this.matrix(l));
    }
  }

  private meters(l: Layer): HTMLElement {
    const f = document.createElement('fieldset');
    f.dataset.tour = 'features';
    const lg = document.createElement('legend');
    lg.textContent = 'Sung features of this layer';
    const row = document.createElement('div');
    row.className = 'row';
    const b = document.createElement('button');
    b.type = 'button';
    b.id = 'listenLayer';
    const on = this.listen.get(l.id) ?? false;
    b.setAttribute('aria-pressed', String(on));
    b.textContent = on ? 'Listening' : 'Listen';
    b.title = 'The mic drives this layer\'s sung features while its notes come from elsewhere.';
    b.addEventListener('click', () => {
      this.listen.set(l.id, !(this.listen.get(l.id) ?? false));
      this.modBuilt = null;
      this.render();
    });
    row.append(b);
    const meters = document.createElement('div');
    meters.className = 'meters feat';
    for (const name of FEATURES.filter((x) => x !== 'vowel')) {
      const m = document.createElement('div');
      m.className = 'meter';
      m.dataset.feature = name;
      const n = document.createElement('span');
      n.className = 'name';
      n.textContent = FEATURE_NAMES[name];
      const t = document.createElement('span');
      t.className = 'track';
      const fill = document.createElement('span');
      fill.className = 'fill';
      t.append(fill);
      const num = document.createElement('span');
      num.className = 'num';
      m.append(n, t, num);
      meters.append(m);
    }
    const hint = document.createElement('p');
    hint.className = 'hint';
    hint.textContent = 'From the mic while the layer listens, else from its take while the song plays. The per-note features come from each note\'s own take, or from the mic for the voice Live follow plays.';
    f.append(lg, row, meters, hint);
    return f;
  }

  private matrix(l: Layer): HTMLElement {
    const p = l.patch;
    const st = this.app.ui.editState(l);
    const lay = layoutOf(p);
    const f = document.createElement('fieldset');
    f.dataset.tour = 'matrix';
    const lg = document.createElement('legend');
    lg.textContent = 'Routes';
    f.append(lg);
    const newTable = () => {
      const t = document.createElement('table');
      t.className = 'matrix';
      const head = document.createElement('tr');
      for (const h of ['Source', 'Live', 'Shaper', 'Amount', 'Target', 'Via', '']) {
        const th = document.createElement('th');
        th.textContent = h;
        head.append(th);
      }
      t.append(head);
      const wrap = document.createElement('div');
      wrap.className = 'matrixwrap';
      wrap.append(t);
      return { t, wrap };
    };
    const voice = newTable();
    const layerT = newTable();
    const macro = newTable();
    const rows: { route: Route; scope: Scope; index: number }[] = [
      ...p.voices[this.slot].routes.map((route, index) => ({ route, scope: this.slot as Scope, index })),
      ...p.routes.map((route, index) => ({ route, scope: 'layer' as Scope, index })),
    ];
    for (const { route, scope, index } of rows) {
      const table = scope !== 'layer' ? voice.t : route.source.startsWith('macro') ? macro.t : layerT.t;
      const tr = document.createElement('tr');
      const isSel = this.selected?.scope === scope && this.selected.id === route.id;
      tr.classList.toggle('sel', isSel);
      const cell = (x: string | HTMLElement) => {
        const td = document.createElement('td');
        td.append(x);
        tr.append(td);
        return td;
      };
      const src = document.createElement('button');
      src.type = 'button';
      src.className = 'link';
      src.textContent = sourceName(route.source, p);
      src.title = 'Edit the shaper';
      src.addEventListener('click', () => { this.selected = { scope, id: route.id }; this.render(); });
      cell(src);
      const bar = document.createElement('span');
      bar.className = 'livebar';
      const fill = document.createElement('span');
      bar.append(fill);
      cell(bar);
      this.routeRows.push({ scope, index, bar: fill });
      const shape = [
        `${fmt(route.inLo)}–${fmt(route.inHi)}`,
        route.points ? `${route.points.length} points` : route.bend ? `bend ${route.bend > 0 ? '+' : ''}${route.bend.toFixed(2)}` : '',
        route.riseMs || route.fallMs ? `${route.riseMs}/${route.fallMs} ms` : '',
        route.latch ? 'latched' : '',
        route.mode !== 'add' ? route.mode : '',
        route.group ? `group ${route.group}` : '',
      ].filter(Boolean).join(', ');
      cell(shape);
      const ak = amountKey(p, scope, route.id);
      const ai = lay.index.get(ak)!;
      const amount = toReal(lay.descs[ai], st.values[ak] ?? lay.init[ai]);
      cell(`${amount >= 0 ? '+' : ''}${amount.toFixed(2)}${route.mode === 'scale' ? ' ×' : ''}`);
      const tgt = route.target === 'pitch' ? 'pitch' : keyLabel(route.target.replace(/^(bands|oct|k\*):?/, '').replace(/^\./, ''));
      cell(scope === 'layer' && route.target.startsWith('k*.') ? `${tgt} (every kit slot)` : tgt);
      cell(route.via ? sourceName(route.via, p) : '—');
      const del = document.createElement('button');
      del.type = 'button';
      del.textContent = '×';
      del.setAttribute('aria-label', `Remove the route from ${sourceName(route.source, p)}`);
      del.addEventListener('click', () => {
        if (this.app.run(() => this.app.store.dispatch('removeRoute', { layer: l.id, scope, id: route.id }))) {
          if (this.selected?.id === route.id) this.selected = null;
        }
      });
      cell(del);
      table.append(tr);
    }
    const sub = (text: string) => {
      const h = document.createElement('p');
      h.className = 'hint';
      h.textContent = text;
      return h;
    };
    f.append(sub(p.voices.length > 1 ? `Per voice, kit slot ${this.slot + 1}` : 'Per voice'), voice.wrap);
    if (layerT.t.rows.length > 1) f.append(sub('Whole layer'), layerT.wrap);
    if (macro.t.rows.length > 1) {
      const d = document.createElement('details');
      const s = document.createElement('summary');
      s.textContent = `Macro routes (${macro.t.rows.length - 1})`;
      d.append(s, macro.wrap);
      f.append(d);
    }
    return f;
  }

  private adder(l: Layer): HTMLElement {
    const p = l.patch;
    const lay = layoutOf(p);
    const row = document.createElement('div');
    row.className = 'row';
    const lab = document.createElement('label');
    lab.textContent = 'Add a route from ';
    const src = document.createElement('select');
    for (const o of [...this.els.pick.options].filter((x) => x.value)) src.append(o.cloneNode(true));
    src.value = this.picked || src.options[0]?.value || '';
    lab.append(src);
    const tl = document.createElement('label');
    tl.textContent = 'to ';
    const tgt = document.createElement('select');
    const fill = () => {
      const [lo, hi] = lay.ranges[this.slot];
      const voice = !isLayerSource(src.value);
      const keys = lay.keys.filter((k, i) => (voice ? i >= lo && i < hi : true) && !/\.(tone_bands|bands)\.\d+$/.test(k));
      const bandBases = [...new Set(lay.keys.filter((k, i) => (voice ? i >= lo && i < hi : true) && /\.(tone_bands|bands)\.0$/.test(k)))];
      const opts: [string, string][] = [
        ...(voice ? [['pitch', 'pitch'] as [string, string]] : []),
        ...keys.map((k) => [k, keyLabel(k)] as [string, string]),
        ...bandBases.map((k) => [`bands:${k.slice(0, -2)}`, `${keyLabel(k.slice(0, -2))} (all 16)`] as [string, string]),
      ];
      tgt.replaceChildren(...opts.map(([v, t]) => {
        const o = document.createElement('option');
        o.value = v;
        o.textContent = t;
        return o;
      }));
    };
    fill();
    src.addEventListener('change', fill);
    tl.append(tgt);
    const add = document.createElement('button');
    add.type = 'button';
    add.textContent = 'Add';
    add.addEventListener('click', () => {
      this.picked = src.value;
      this.els.pick.value = src.value;
      if (tgt.value.startsWith('bands:')) {
        const before = new Set(allRoutes(l.patch).map((x) => `${x.scope}/${x.route.id}`));
        const pre = kitPrefix(l.patch, this.slot);
        const t = isLayerSource(src.value) ? tgt.value : `bands:${tgt.value.slice(6).replace(pre, '')}`;
        if (this.app.run(() => this.app.store.dispatch('addRoute', { layer: l.id, slot: this.slot, route: { source: src.value, target: t } }))) {
          const added = allRoutes(this.layer()!.patch).find((x) => !before.has(`${x.scope}/${x.route.id}`));
          if (added) this.selected = { scope: added.scope, id: added.route.id };
          this.acted('route');
        }
      } else this.routeTo(tgt.value);
    });
    row.append(lab, tl, add);
    const f = document.createElement('fieldset');
    const lg = document.createElement('legend');
    lg.textContent = 'New route';
    const hint = document.createElement('p');
    hint.className = 'hint';
    hint.textContent = 'Or pick a source under Modulate, then press + beside any slider in the Patch view.';
    f.append(lg, row, hint);
    return f;
  }

  private shaper(l: Layer, scope: Scope, r: Route): HTMLElement {
    const p = l.patch;
    const lay = layoutOf(p);
    const f = document.createElement('fieldset');
    f.className = 'shaper';
    f.dataset.tour = 'shaper';
    const lg = document.createElement('legend');
    lg.textContent = `Shaper: ${sourceName(r.source, p)} → ${r.target === 'pitch' ? 'pitch' : keyLabel(r.target.replace(/^(bands|oct):/, ''))}`;
    f.append(lg);
    const set = (fields: RouteFields, commit: boolean) => {
      const args = { layer: l.id, scope, id: r.id, fields };
      if (commit) this.app.runRestoring(() => this.app.store.dispatch('setRoute', args));
      else this.app.previewDoc((d) => applyOps(d, commands.setRoute(d, args)).doc);
    };
    // The curve: y against the part of the source's range in use. Click adds a point, drag moves one,
    // double-click removes one.
    const canvas = document.createElement('canvas');
    canvas.width = 240;
    canvas.height = 120;
    canvas.className = 'curve';
    canvas.setAttribute('aria-label', 'Shaper curve. Click to add a point, drag to move it, double-click to remove it.');
    let pts: [number, number][] | null = r.points ? r.points.map((q) => [q[0], q[1]] as [number, number]) : null;
    const draw = () => paintCurve(canvas, { ...r, points: pts }, NaN);
    draw();
    const at = (e: MouseEvent): [number, number] => {
      const b = canvas.getBoundingClientRect();
      return [clamp01((e.clientX - b.left) / b.width), clamp01(1 - (e.clientY - b.top) / b.height)];
    };
    let drag = -1;
    canvas.addEventListener('mousedown', (e) => {
      const [x, y] = at(e);
      pts ??= [[0, 0], [1, 1]];
      drag = pts.findIndex((q) => Math.abs(q[0] - x) < 0.05 && Math.abs(q[1] - y) < 0.08);
      if (drag < 0 && pts.length < 8) {
        pts.push([x, y]);
        pts.sort((a, b) => a[0] - b[0]);
        drag = pts.findIndex((q) => q[0] === x && q[1] === y);
      }
      draw();
    });
    canvas.addEventListener('mousemove', (e) => {
      if (drag < 0 || !pts) return;
      const [x, y] = at(e);
      const lo = drag > 0 ? pts[drag - 1][0] : 0, hi = drag < pts.length - 1 ? pts[drag + 1][0] : 1;
      pts[drag] = [Math.min(hi, Math.max(lo, x)), y];
      draw();
      set({ points: pts.map((q) => [q[0], q[1]]) }, false);
    });
    const up = () => {
      if (drag < 0) return;
      drag = -1;
      set({ points: pts ? pts.map((q) => [q[0], q[1]]) : null }, true);
    };
    canvas.addEventListener('mouseup', up);
    canvas.addEventListener('mouseleave', up);
    canvas.addEventListener('dblclick', (e) => {
      if (!pts) return;
      const [x, y] = at(e);
      const k = pts.findIndex((q) => Math.abs(q[0] - x) < 0.05 && Math.abs(q[1] - y) < 0.08);
      if (k >= 0 && pts.length > 2) { pts.splice(k, 1); set({ points: pts.map((q) => [q[0], q[1]]) }, true); }
    });
    this.curve = { canvas, route: () => ({ ...r, points: pts }), scope, index: (scope === 'layer' ? p.routes : p.voices[scope].routes).findIndex((x) => x.id === r.id) };
    f.append(canvas);

    const grid = document.createElement('div');
    grid.className = 'shaperfields';
    const num = (label: string, v: number, step: number, field: 'inLo' | 'inHi' | 'riseMs' | 'fallMs') => {
      const lab = document.createElement('label');
      lab.textContent = `${label} `;
      const i = document.createElement('input');
      i.type = 'number';
      i.step = String(step);
      i.value = String(v);
      i.addEventListener('change', () => set({ [field]: Number(i.value) }, true));
      lab.append(i);
      grid.append(lab);
    };
    num('From', r.inLo, /pitch/.test(r.source) ? 100 : 0.01, 'inLo');
    num('To', r.inHi, /pitch/.test(r.source) ? 100 : 0.01, 'inHi');
    const bend = document.createElement('label');
    bend.textContent = 'Bend ';
    const bi = document.createElement('input');
    bi.type = 'range';
    bi.min = '-100';
    bi.max = '100';
    bi.value = String(Math.round(r.bend * 100));
    bi.setAttribute('aria-label', 'Bend of the curve');
    bi.addEventListener('input', () => { set({ bend: Number(bi.value) / 100 }, false); paintCurve(canvas, { ...r, points: pts, bend: Number(bi.value) / 100 }, NaN); });
    bi.addEventListener('change', () => set({ bend: Number(bi.value) / 100 }, true));
    bend.append(bi);
    grid.append(bend);
    const straight = document.createElement('button');
    straight.type = 'button';
    straight.textContent = 'Straight line';
    straight.addEventListener('click', () => set({ points: null, bend: 0 }, true));
    grid.append(straight);
    num('Rise ms', r.riseMs, 1, 'riseMs');
    num('Fall ms', r.fallMs, 1, 'fallMs');
    const choose = <T extends string>(label: string, value: T, opts: [T, string][], field: 'polarity' | 'mode' | 'group' | 'via') => {
      const lab = document.createElement('label');
      lab.textContent = `${label} `;
      const s = document.createElement('select');
      for (const [v, t] of opts) {
        const o = document.createElement('option');
        o.value = v;
        o.textContent = t;
        s.append(o);
      }
      s.value = value;
      s.addEventListener('change', () => set({ [field]: s.value === '' ? null : s.value } as RouteFields, true));
      lab.append(s);
      grid.append(lab);
    };
    choose('Polarity', r.polarity, [['up', 'up (0 to +1)'], ['down', 'down (-1 to 0)'], ['both', 'both (-1 to +1)']], 'polarity');
    choose('Mode', r.mode, r.target === 'pitch' ? [['add', 'add']] : [['add', 'add'], ['scale', 'scale (gains only)'], ['pull', 'pull toward the source']], 'mode');
    choose('Group', (r.group ?? '') as string, [['', 'none'], ['loud', 'loud'], ['open', 'open'], ['hard', 'hard'], ['breath', 'breath'], ['mouth', 'mouth'], ['follow', 'follow']], 'group');
    const vias: [string, string][] = [['', 'none'], ...[...this.els.pick.options].filter((o) => o.value && (scope !== 'layer' || isLayerSource(o.value))).map((o) => [o.value, o.textContent ?? o.value] as [string, string])];
    choose('Via', r.via ?? '', vias, 'via');
    if (scope !== 'layer') {
      const lab = document.createElement('label');
      const box = document.createElement('input');
      box.type = 'checkbox';
      box.checked = r.latch;
      box.addEventListener('change', () => set({ latch: box.checked }, true));
      lab.append(box, ' Latch at note start');
      grid.append(lab);
    }
    f.append(grid);

    // Amount: a value of the state being edited.
    const ak = amountKey(p, scope, r.id);
    const ai = lay.index.get(ak)!;
    const st = this.app.ui.editState(l);
    const amt = document.createElement('label');
    amt.dataset.tour = 'route-amount';
    amt.textContent = 'Amount ';
    const ar = document.createElement('input');
    ar.type = 'range';
    ar.min = '-1000';
    ar.max = '1000';
    ar.value = String(Math.round(toReal(lay.descs[ai], st.values[ak] ?? lay.init[ai]) * 1000));
    ar.setAttribute('aria-label', 'Route amount');
    const ao = document.createElement('output');
    ao.value = (Number(ar.value) / 1000).toFixed(2);
    const args = (doc: Project) => {
      const cur = doc.layers.find((x) => x.id === l.id)!;
      return { layer: cur.id, state: this.app.ui.editState(cur).id, values: { [ak]: amountNorm(Number(ar.value) / 1000) } };
    };
    ar.addEventListener('input', () => { ao.value = (Number(ar.value) / 1000).toFixed(2); this.app.previewDoc((d) => applyOps(d, commands.setValues(d, args(d))).doc); });
    ar.addEventListener('change', () => { if (this.app.runRestoring(() => this.app.store.dispatch('setValues', args(this.app.doc)))) this.acted('amount'); });
    amt.append(ar, ao);
    const unit = r.target === 'pitch' ? `1 is ${Math.round((r.unit ?? 4800) / 100)} semitones` : r.mode === 'scale' ? 'multiplies the target by 1 + amount × the curve' : '1 moves the target across its whole range';
    const hint = document.createElement('p');
    hint.className = 'hint';
    hint.textContent = `${unit}. Per state, so a morph can deepen it.${isBipolarSource(r.source) ? ' This source swings both ways; its middle is 0.5 on the curve.' : ''}`;
    f.append(amt, hint);
    return f;
  }

  private curve: { canvas: HTMLCanvasElement; route: () => Route; scope: Scope; index: number } | null = null;

  /** Live parts, every animation frame or so: thumbs, route outputs, feature meters. */
  frame(m: Float32Array | null): void {
    if (!m || this.view === 'simple') return;
    const l = this.layer();
    if (!l) return;
    const lay = layoutOf(l.patch);
    if (this.view === 'patch') {
      for (const [key, k] of this.knobs) {
        const i = lay.index.get(key);
        if (i === undefined) continue;
        const live = m[M_LIVE + i];
        k.live.hidden = !Number.isFinite(live) || Math.abs(live - m[M_STORED + i]) < 1e-4;
        if (!k.live.hidden) k.live.style.left = `${live * 100}%`;
      }
    }
    if (this.view === 'mod') {
      for (const r of this.routeRows) {
        const y = m[M_ROUTES + SCOPE_METER(r.scope, r.index)];
        r.bar.style.width = Number.isFinite(y) ? `${clamp01(y) * 100}%` : '0';
      }
      this.els.mod.querySelectorAll<HTMLElement>('.feat .meter').forEach((el) => {
        const f = FEATURES.indexOf(el.dataset.feature as (typeof FEATURES)[number]);
        const slot = f === 8 ? 7 : f;
        const x = m[M_LAYER_SRC + LS_FEATURE + slot];
        const fill = el.querySelector<HTMLElement>('.fill')!;
        const num = el.querySelector<HTMLElement>('.num')!;
        if (el.dataset.feature === 'pitch') {
          fill.style.width = Number.isFinite(x) ? `${clamp01((x - 3600) / 2400) * 100}%` : '0';
          num.textContent = Number.isFinite(x) ? noteName(x) : '—';
        } else if (el.dataset.feature === 'voicing') {
          fill.style.width = Number.isFinite(x) ? `${clamp01(x / 2) * 100}%` : '0';
          num.textContent = Number.isFinite(x) ? ['voiced', 'unvoiced', 'silent'][Math.round(x)] ?? '' : '—';
        } else {
          fill.style.width = Number.isFinite(x) ? `${clamp01(x) * 100}%` : '0';
          num.textContent = Number.isFinite(x) ? x.toFixed(2) : '—';
        }
      });
      const b = this.els.mod.querySelector('#listenLayer');
      const i = this.app.doc.layers.indexOf(l);
      if (b) b.classList.toggle('down', ((m[M_LISTEN] >>> 0) & (1 << i)) !== 0);
      if (this.curve) {
        const y = m[M_ROUTES + SCOPE_METER(this.curve.scope, this.curve.index)];
        paintCurve(this.curve.canvas, this.curve.route(), y);
      }
    }
  }
}

const clamp01 = (x: number) => (x < 0 ? 0 : x > 1 ? 1 : x);
const fmt = (x: number) => (Math.abs(x) >= 100 ? x.toFixed(0) : Math.abs(x) >= 1 ? x.toFixed(1) : x.toFixed(2));

function noteName(cents: number): string {
  const n = Math.round(cents / 100);
  return `${['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'][((n % 12) + 12) % 12]}${Math.floor(n / 12)}`;
}

/** y of the shaper at u (as the engine computes it, without range and smoothing). */
export function curveAt(r: Pick<Route, 'bend' | 'points'>, u: number): number {
  const p = r.points;
  if (p && p.length >= 2) {
    if (u <= p[0][0]) return p[0][1];
    for (let i = 1; i < p.length; i++) {
      if (u <= p[i][0]) {
        const span = p[i][0] - p[i - 1][0];
        const t = span > 0 ? (u - p[i - 1][0]) / span : 1;
        return p[i - 1][1] + (p[i][1] - p[i - 1][1]) * t;
      }
    }
    return p[p.length - 1][1];
  }
  return r.bend === 0 ? u : Math.pow(u, Math.pow(2, -2 * r.bend));
}

function paintCurve(c: HTMLCanvasElement, r: Route, liveY: number): void {
  const g = c.getContext('2d');
  if (!g) return;
  const css = getComputedStyle(c);
  const [w, h] = [c.width, c.height];
  g.clearRect(0, 0, w, h);
  g.strokeStyle = css.getPropertyValue('--line').trim() || '#3d5166';
  g.strokeRect(0.5, 0.5, w - 1, h - 1);
  g.strokeStyle = css.getPropertyValue('--accent').trim() || '#f0b429';
  g.lineWidth = 2;
  g.beginPath();
  for (let x = 0; x <= w; x++) {
    const y = curveAt(r, x / w);
    if (x === 0) g.moveTo(x, h - y * h); else g.lineTo(x, h - y * h);
  }
  g.stroke();
  g.fillStyle = css.getPropertyValue('--hot').trim() || '#ffe6b0';
  for (const q of r.points ?? []) g.fillRect(q[0] * w - 3, h - q[1] * h - 3, 6, 6);
  if (Number.isFinite(liveY)) {
    g.strokeStyle = css.getPropertyValue('--signal').trim() || '#9ec1ff';
    g.lineWidth = 1;
    g.beginPath();
    g.moveTo(0, h - liveY * h);
    g.lineTo(w, h - liveY * h);
    g.stroke();
  }
}
