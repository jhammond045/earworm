// Routes in the document (docs/synth.md "Modulation"): where a new route goes, the range and amount
// it starts with, and the rules the engine also checks (so an edit is refused with a plain message
// before the engine refuses the patch).
//
// A route whose source is one per layer (macros, wheel, bend, pressure, layer LFOs, the layer's
// sung features) is a layer route (patch.routes) and may target any value of the layer, written as
// its full key (`k2.filter1.cutoff` on a beat layer). Any other route belongs to one voice patch and
// targets that patch's values by their local key (`filter1.cutoff`).

import { DESC_ID, SCALABLE } from '../gen/modules';
import { isBipolarSource, isLayerSource, layoutOf, moduleKeys, type Patch, type Route } from './patch';
import { toNorm } from './values';

export type RouteFields = Partial<Omit<Route, 'id'>>;

/** Where a route lives: a voice patch index, or 'layer'. */
export type Scope = number | 'layer';

/** Fields a route may change without a structure change in the engine (its shaper). */
export const SHAPER_FIELDS = ['inLo', 'inHi', 'bend', 'points', 'riseMs', 'fallMs', 'latch', 'polarity', 'mode', 'group', 'via', 'unit'] as const;

const C3 = 3600;
const C5 = 6000;

/** The part of a source's range a new route uses: notes C3 to C5 for pitch, the hit fields in their
 *  own units, 0..1 for the rest. */
export function defaultRange(source: string): [number, number] {
  if (/(^|\.)pitch$/.test(source)) return [C3, C5];
  if (/\.voicing$/.test(source)) return [0, 2];
  if (source === 'note.attack') return [0.001, 0.2];
  if (source === 'note.bend') return [-12, 12];
  if (source === 'note.hold') return [Math.fround(Math.log2(0.05)), Math.fround(Math.log2(0.8))];
  return [0, 1];
}

/** Starting amount (real units, -1..1) of a new route into `target`. */
export function defaultAmount(target: string): number {
  return target === 'pitch' ? 2 / 48 : 0.5;
}

/** A new route with the usual shaper: a straight line over the source's range, no smoothing. A
 *  bipolar source (an LFO, the bend) moves both ways. */
export function makeRoute(id: string, source: string, target: string, more: RouteFields = {}, mods: Patch['voices'][number]['mods'] = []): Route {
  const [inLo, inHi] = defaultRange(source);
  return {
    id, source, target, mode: 'add', polarity: isBipolarSource(source, mods) ? 'both' : 'up',
    inLo, inHi, bend: 0, points: null, riseMs: 0, fallMs: 0, latch: false, via: null, group: null, ...more,
  };
}

/** Kit prefix of a voice patch's keys (beat layers). */
export const kitPrefix = (patch: Patch, vp: number): string => (patch.voices.length > 1 ? `k${vp + 1}.` : '');

/** The scope a source's route lives in. A per-voice source needs the voice patch it reads. */
export function scopeOf(source: string, vp: number): Scope {
  return isLayerSource(source) ? 'layer' : vp;
}

/** Every route of a patch with its scope. */
export function allRoutes(patch: Patch): { route: Route; scope: Scope }[] {
  return [
    ...patch.voices.flatMap((v, k) => v.routes.map((route) => ({ route, scope: k as Scope }))),
    ...patch.routes.map((route) => ({ route, scope: 'layer' as Scope })),
  ];
}

/** Route `id` of `scope`. Ids are unique within a scope (each kit slot's voice patch has its own). */
export function findRoute(patch: Patch, scope: Scope, id: string): Route | null {
  const list = scope === 'layer' ? patch.routes : patch.voices[scope]?.routes ?? [];
  return list.find((x) => x.id === id) ?? null;
}

/** The full key of a route's target value (null for pitch), and its value key when it is a band shape or octave move. */
export function targetKey(patch: Patch, r: Route, scope: Scope): string | null {
  if (r.target === 'pitch') return null;
  const t = r.target.replace(/^(bands|oct):/, '');
  const local = r.target.startsWith('bands:') ? `${t}.0` : t;
  return scope === 'layer' ? local : kitPrefix(patch, scope) + local;
}

/** Routes whose target is the value `key` (a full key), for the knob badges and rings. */
export function routesInto(patch: Patch, key: string): { route: Route; scope: Scope }[] {
  return allRoutes(patch).filter(({ route, scope }) => {
    if (route.target.startsWith('k*.')) return patch.voices.some((_, k) => kitPrefix(patch, k) + route.target.slice(3) === key);
    if (route.target.startsWith('bands:')) {
      const base = (scope === 'layer' ? '' : kitPrefix(patch, scope)) + route.target.slice(6);
      return key.startsWith(`${base}.`);
    }
    return targetKey(patch, route, scope) === key;
  });
}

/** A route id no scope of the patch uses. */
export function nextRouteId(patch: Patch): string {
  const used = new Set(allRoutes(patch).map((x) => x.route.id));
  for (let i = 1; ; i++) if (!used.has(`r${i}`)) return `r${i}`;
}

const fail = (msg: string): never => { throw new RangeError(msg); };

/** Keys of envelope and LFO values (voice mods and layer LFOs), full keys. */
function modKeys(patch: Patch): Set<string> {
  const out = new Set<string>();
  patch.voices.forEach((v, k) => v.mods.forEach((m) => moduleKeys(m.id, m.type).forEach((x) => out.add(kitPrefix(patch, k) + x.local))));
  patch.lfos.forEach((m) => moduleKeys(m.id, m.type).forEach((x) => out.add(x.local)));
  return out;
}

const isModSource = (patch: Patch, s: string, scope: Scope): boolean =>
  /^layer\.lfo[12]$/.test(s) || (scope !== 'layer' && patch.voices[scope].mods.some((m) => m.id === s));

/** The engine's rules for a route, as plain messages: the source fits the scope, the target exists,
 *  and no envelope or LFO moves an envelope or LFO value (so no source can modulate itself). */
export function checkRoute(patch: Patch, r: Route, scope: Scope): void {
  if (scope === 'layer' && !isLayerSource(r.source)) fail(`${r.source} is per voice, so it cannot drive a layer route`);
  if (scope !== 'layer' && isLayerSource(r.source)) fail(`${r.source} is one per layer, so its route is a layer route`);
  if (r.target === 'pitch' && scope === 'layer') fail('pitch is per voice: route it from a voice source');
  if (!(r.inLo === r.inLo && r.inHi === r.inHi && Number.isFinite(r.inLo) && Number.isFinite(r.inHi))) fail('the shaper range must be numbers');
  if (!(r.bend >= -1 && r.bend <= 1)) fail('bend must be from -1 to 1');
  if (!(r.riseMs >= 0 && r.fallMs >= 0)) fail('rise and fall must be 0 ms or more');
  if (r.points) {
    if (r.points.length > 8) fail('a shaper has at most 8 points');
    r.points.forEach(([x, y], i) => {
      if (!(x >= 0 && x <= 1 && y >= 0 && y <= 1)) fail('shaper points must lie in 0..1');
      if (i > 0 && x < r.points![i - 1][0]) fail('shaper points must run left to right');
    });
  }
  if (r.latch && scope === 'layer') fail('a layer route has no note to latch to');
  const lay = layoutOf(patch);
  const key = targetKey(patch, r, scope);
  if (key !== null && !r.target.startsWith('k*.') && !lay.index.has(key)) fail(`there is no value ${key}`);
  if (key !== null && scope !== 'layer') {
    // A per-voice source reaches only its own voice patch's values; the channel is one per layer.
    const i = lay.index.get(key)!;
    const [lo, hi] = lay.ranges[scope];
    if (i < lo || i >= hi) fail(`${key} is not in this voice patch: only a layer source (a macro, the wheel, a layer LFO, a layer feature) can reach it`);
  }
  if (r.mode === 'scale' && r.target !== 'pitch') {
    // The engine multiplies only gains, levels and mixes.
    const d = lay.descs[lay.index.get(key!) ?? -1];
    if (d !== undefined && !SCALABLE_DESCS.has(d)) fail('scale only multiplies a gain, a level or a mix');
  }
  if (r.target === 'pitch' && r.mode !== 'add') fail('a pitch route adds');
  if (isModSource(patch, r.source, scope) && key !== null && modKeys(patch).has(key)) {
    fail('an envelope or LFO cannot move an envelope or LFO setting (no source may modulate itself)');
  }
}

const SCALABLE_DESCS = new Set(SCALABLE.flatMap((s, i) => (s ? [i] : [])));

/** `patch` with `r` added to `scope`. A route from or via a layer LFO the patch lacks adds that LFO. */
export function withRoute(patch: Patch, r: Route, scope: Scope): Patch {
  const lfos = patch.lfos.slice();
  for (const s of [r.source, r.via]) if (s && /^layer\.lfo[12]$/.test(s) && !lfos.some((m) => m.id === s)) lfos.push({ id: s, type: 'lfo' });
  lfos.sort((a, b) => a.id.localeCompare(b.id));
  const next: Patch = { ...patch, lfos };
  if (scope === 'layer') next.routes = [...patch.routes, r];
  else next.voices = patch.voices.map((v, k) => (k === scope ? { ...v, routes: [...v.routes, r] } : v));
  return next;
}

/** `patch` with route `id` of `scope` changed by `fields`, kept in its place (order matters when a
 *  route moves another route's amount). */
export function withRouteFields(patch: Patch, scope: Scope, id: string, fields: RouteFields): Patch {
  const old = findRoute(patch, scope, id) ?? fail(`no route ${id}`);
  const r: Route = { ...old, ...fields, id };
  const lfos = withRoute(patch, r, scope).lfos;
  const put = (list: Route[]) => list.map((x) => (x.id === id ? r : x));
  return scope === 'layer'
    ? { ...patch, lfos, routes: put(patch.routes) }
    : { ...patch, lfos, voices: patch.voices.map((v, k) => (k === scope ? { ...v, routes: put(v.routes) } : v)) };
}

/** `patch` without route `id` of `scope`. */
export function withoutRoute(patch: Patch, scope: Scope, id: string): Patch {
  return scope === 'layer'
    ? { ...patch, routes: patch.routes.filter((x) => x.id !== id) }
    : { ...patch, voices: patch.voices.map((v, k) => (k === scope ? { ...v, routes: v.routes.filter((x) => x.id !== id) } : v)) };
}

/** The value key of a route's amount. */
export function amountKey(patch: Patch, scope: Scope, id: string): string {
  return (scope === 'layer' ? '' : kitPrefix(patch, scope)) + `route.${id}`;
}

/** A normalized amount from a real one (-1..1). */
export const amountNorm = (real: number): number => toNorm(DESC_ID['route.amount'], real);
