// Structure edits (docs/synth.md "Changing the structure", stage S3): add, remove, rewire and
// reorder voice modules, envelopes, LFOs and channel effects. Each returns a new patch; the caller
// (project/commands.ts) gives every state the new keys at their starting values and drops the keys
// that went. A structure edit is an edit to the layer, not to a state: every state shares it.

import { type ChannelFxType, type ModDef, type Patch, type Port, type Route, type VoiceModuleType, CAPS, MONO_FX, SOURCE_TYPES, layoutOf } from './patch';
import { kitPrefix } from './routes';

const fail = (msg: string): never => { throw new RangeError(msg); };

/** The first `<type><n>` id not in `taken`. */
function freshId(type: string, taken: Iterable<string>): string {
  const t = new Set(taken);
  let n = 1;
  while (t.has(`${type}${n}`)) n++;
  return `${type}${n}`;
}

function withVoice(p: Patch, slot: number, f: (v: Patch['voices'][number]) => Patch['voices'][number]): Patch {
  if (!p.voices[slot]) fail(`no voice patch ${slot}`);
  return { ...p, voices: p.voices.map((v, k) => (k === slot ? f(v) : v)) };
}

/** Does a route read or write anything of `id` (a module, envelope or LFO of voice patch `slot`)? */
function touches(p: Patch, slot: number, r: Route, layer: boolean, id: string): boolean {
  const pre = kitPrefix(p, slot);
  const key = r.target.replace(/^(bands|oct):/, '');
  const local = !layer ? key : key.startsWith('k*.') ? key.slice(3) : key.startsWith(pre) ? key.slice(pre.length) : null;
  if (local !== null && local.startsWith(`${id}.`)) return true;
  return !layer && (r.source === id || r.via === id);
}

/** Add a voice module. A new source feeds what the rack's first source feeds (or the voice output);
 *  a new processor goes on the end of the voice, between the output and what fed it. */
export function addModule(p: Patch, slot: number, type: VoiceModuleType): { patch: Patch; id: string } {
  if (!(type in CAPS.voice)) fail(`unknown module type ${type}`);
  const vp = p.voices[slot] ?? fail(`no voice patch ${slot}`);
  const have = vp.modules.filter((m) => m.type === type).length;
  if (have >= CAPS.voice[type]) fail(`a voice has at most ${CAPS.voice[type]} ${type} module${CAPS.voice[type] > 1 ? 's' : ''}`);
  if (vp.modules.length >= CAPS.modules) fail(`a voice has at most ${CAPS.modules} modules`);
  const id = freshId(type, vp.modules.map((m) => m.id));
  const patch = withVoice(p, slot, (v) => {
    const modules = [...v.modules, { id, type }];
    if (SOURCE_TYPES.includes(type)) {
      const first = v.modules.find((m) => SOURCE_TYPES.includes(m.type));
      const to = first ? v.cables.find((c) => c.from === first.id && c.port === 'in')?.to : undefined;
      return to ? { ...v, modules, cables: [...v.cables, { from: id, to, port: 'in' as Port }] } : { ...v, modules, out: [...v.out, id] };
    }
    const cables = [...v.cables, ...v.out.map((o) => ({ from: o, to: id, port: 'in' as Port }))];
    return { ...v, modules, cables, out: [id] };
  });
  return { patch, id };
}

/** Remove a voice module, its cables, and the routes into its values. */
export function removeModule(p: Patch, slot: number, id: string): Patch {
  const vp = p.voices[slot] ?? fail(`no voice patch ${slot}`);
  if (!vp.modules.some((m) => m.id === id)) fail(`no module ${id}`);
  if (vp.modules.length === 1) fail('a voice needs at least one module');
  const next = withVoice(p, slot, (v) => ({
    ...v,
    modules: v.modules.filter((m) => m.id !== id),
    cables: v.cables.filter((c) => c.from !== id && c.to !== id),
    out: v.out.filter((o) => o !== id),
    routes: v.routes.filter((r) => !touches(p, slot, r, false, id)),
  }));
  return { ...next, routes: next.routes.filter((r) => !touches(p, slot, r, true, id)) };
}

/** Set every cable into `id`'s `port` (`from`: the modules that feed it, summed). */
export function setInputs(p: Patch, slot: number, id: string, port: Port, from: string[]): Patch {
  const vp = p.voices[slot] ?? fail(`no voice patch ${slot}`);
  const m = vp.modules.find((x) => x.id === id) ?? fail(`no module ${id}`);
  if (port === 'pm' && m.type !== 'osc') fail('only an oscillator has a phase input');
  if (port === 'in' && SOURCE_TYPES.includes(m.type)) fail(`${id} makes sound itself and has no input`);
  for (const f of from) if (!vp.modules.some((x) => x.id === f)) fail(`no module ${f}`);
  if (new Set(from).size !== from.length) fail('an input lists a module twice');
  return withVoice(p, slot, (v) => ({
    ...v,
    cables: [...v.cables.filter((c) => !(c.to === id && c.port === port)), ...from.map((f) => ({ from: f, to: id, port }))],
  }));
}

/** The modules summed into the voice output. */
export function setOutputs(p: Patch, slot: number, ids: string[]): Patch {
  const vp = p.voices[slot] ?? fail(`no voice patch ${slot}`);
  for (const f of ids) if (!vp.modules.some((x) => x.id === f)) fail(`no module ${f}`);
  if (new Set(ids).size !== ids.length) fail('the output lists a module twice');
  return withVoice(p, slot, (v) => ({ ...v, out: [...ids] }));
}

function move<T>(list: T[], from: number, to: number): T[] {
  const out = [...list];
  const [x] = out.splice(from, 1);
  out.splice(Math.max(0, Math.min(out.length, to)), 0, x);
  return out;
}

/** Move a voice module to position `to` in the rack (run order still follows the cables). */
export function moveModule(p: Patch, slot: number, id: string, to: number): Patch {
  const vp = p.voices[slot] ?? fail(`no voice patch ${slot}`);
  const i = vp.modules.findIndex((m) => m.id === id);
  if (i < 0) fail(`no module ${id}`);
  return withVoice(p, slot, (v) => ({ ...v, modules: move(v.modules, i, to) }));
}

/** Add an envelope or LFO to voice patch `slot`. */
export function addMod(p: Patch, slot: number, type: 'env' | 'lfo'): { patch: Patch; id: string } {
  const vp = p.voices[slot] ?? fail(`no voice patch ${slot}`);
  const cap = type === 'env' ? CAPS.envs : CAPS.lfos;
  if (vp.mods.filter((m) => m.type === type).length >= cap) fail(`a voice has at most ${cap} ${type === 'env' ? 'envelopes' : 'LFOs'}`);
  const id = freshId(type, vp.mods.map((m) => m.id));
  const mod: ModDef = { id, type };
  return { patch: withVoice(p, slot, (v) => ({ ...v, mods: [...v.mods, mod] })), id };
}

/** Remove an envelope or LFO and the routes from it or into it. The first envelope ends the voice
 *  and stays; an envelope that drives a layer source stays until that source goes. */
export function removeMod(p: Patch, slot: number, id: string): Patch {
  const vp = p.voices[slot] ?? fail(`no voice patch ${slot}`);
  const i = vp.mods.findIndex((m) => m.id === id);
  if (i < 0) fail(`no envelope or LFO ${id}`);
  if (i === 0) fail('the first envelope ends the voice, so it stays');
  const driven = p.layerSources.find((s) => s.voice === slot && s.drive === id);
  if (driven) fail(`${id} drives ${driven.id}`);
  const next = withVoice(p, slot, (v) => ({ ...v, mods: v.mods.filter((m) => m.id !== id), routes: v.routes.filter((r) => !touches(p, slot, r, false, id)) }));
  return { ...next, routes: next.routes.filter((r) => !touches(p, slot, r, true, id)) };
}

/** Add a channel effect: crush and tape before the first delay or reverb (they run on the voice
 *  sum), the others at the end. The `input` element goes first, so every effect runs on the blend
 *  of the synth and the raw input until one is moved before it. */
export function addChannelFx(p: Patch, type: ChannelFxType): { patch: Patch; id: string } {
  if (!['crush', 'tape', 'cfilter', 'delay', 'reverb', 'input'].includes(type)) fail(`unknown channel effect ${type}`);
  if (p.channel.length >= CAPS.channel) fail(`a channel has at most ${CAPS.channel} effects`);
  if (type === 'input' && p.channel.some((x) => x.type === 'input')) fail('a channel has at most one input');
  const id = freshId(type, p.channel.map((x) => x.id));
  const fx = { id, type, bypass: false };
  const stereo = p.channel.findIndex((x) => x.type === 'delay' || x.type === 'reverb');
  const at = type === 'input' ? 0 : MONO_FX.includes(type) && stereo >= 0 ? stereo : p.channel.length;
  return { patch: { ...p, channel: [...p.channel.slice(0, at), fx, ...p.channel.slice(at)] }, id };
}

/** Effects after the `input` element run on the blend: they lose a path they had before it. */
function blendPaths(channel: Patch['channel']): Patch['channel'] {
  const input = channel.findIndex((x) => x.type === 'input');
  return channel.map((x, i) => (x.path && (input < 0 || i > input) ? (({ path: _p, ...rest }) => rest)(x) : x));
}

/** Run an effect before the `input` element on the synth or on the raw input alone. */
export function setFxPath(p: Patch, id: string, path: 'synth' | 'raw'): Patch {
  const i = p.channel.findIndex((x) => x.id === id);
  if (i < 0) fail(`no channel effect ${id}`);
  const input = p.channel.findIndex((x) => x.type === 'input');
  if (input < 0 || i > input) fail(`${id} runs after the input, on the blend; move it before the input first`);
  if (p.channel[i].type === 'input') fail('the input has no path');
  return { ...p, channel: p.channel.map((x) => (x.id !== id ? x : path === 'raw' ? { ...x, path } : (({ path: _p, ...rest }) => rest)(x))) };
}

/** Remove a channel effect and the routes into it. */
export function removeChannelFx(p: Patch, id: string): Patch {
  if (!p.channel.some((x) => x.id === id)) fail(`no channel effect ${id}`);
  const into = (r: Route) => r.target.replace(/^(bands|oct):/, '').startsWith(`${id}.`);
  return { ...p, channel: blendPaths(p.channel.filter((x) => x.id !== id)), routes: p.routes.filter((r) => !into(r)) };
}

export function moveChannelFx(p: Patch, id: string, to: number): Patch {
  const i = p.channel.findIndex((x) => x.id === id);
  if (i < 0) fail(`no channel effect ${id}`);
  return { ...p, channel: blendPaths(move(p.channel, i, to)) };
}

export function setBypass(p: Patch, id: string, bypass: boolean): Patch {
  if (!p.channel.some((x) => x.id === id)) fail(`no channel effect ${id}`);
  if (bypass && p.channel.some((x) => x.id === id && x.type === 'input')) fail('the input cannot be bypassed');
  return { ...p, channel: p.channel.map((x) => (x.id === id ? { ...x, bypass } : x)) };
}

/** A state's values for a new patch: every key of it, kept where it was already set, else at its
 *  starting value; keys the patch no longer has are dropped. */
export function revalue(next: Patch, values: Record<string, number>): Record<string, number> {
  const lay = layoutOf(next);
  const out: Record<string, number> = {};
  lay.keys.forEach((k, i) => { out[k] = values[k] ?? lay.init[i]; });
  return out;
}
