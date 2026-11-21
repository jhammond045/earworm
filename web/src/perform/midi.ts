// Live play without the page (docs/synth.md "Live use", stage S4): reading MIDI messages, which
// layer and kit slot a note plays, which value a controller sets, and the per-device default map.
// The Perform view (app/perform.ts) does the Web MIDI and DOM parts.

import type { Layer, MidiMapping, PerfTarget, Project } from '../project/types';

export type MidiEvent =
  | { kind: 'on'; channel: number; note: number; velocity: number }
  | { kind: 'off'; channel: number; note: number }
  | { kind: 'cc'; channel: number; cc: number; value: number }
  /** -1..1, 0 at the centre. */
  | { kind: 'bend'; channel: number; value: number }
  /** Channel pressure, 0..1. */
  | { kind: 'pressure'; channel: number; value: number };

/** One channel message, or null for anything else (system messages, poly pressure, program change). */
export function parseMidi(d: ArrayLike<number>): MidiEvent | null {
  if (d.length < 2) return null;
  const type = d[0] & 0xf0;
  const channel = d[0] & 0x0f;
  const a = d[1] & 0x7f;
  const b = d.length > 2 ? d[2] & 0x7f : 0;
  switch (type) {
    case 0x90: return b === 0 ? { kind: 'off', channel, note: a } : { kind: 'on', channel, note: a, velocity: b / 127 };
    case 0x80: return { kind: 'off', channel, note: a };
    case 0xb0: return d.length > 2 ? { kind: 'cc', channel, cc: a, value: b / 127 } : null;
    case 0xe0: {
      if (d.length < 3) return null;
      const v = ((b << 7) | a) - 8192;
      return { kind: 'bend', channel, value: Math.max(-1, v / (v < 0 ? 8192 : 8191)) };
    }
    case 0xd0: return { kind: 'pressure', channel, value: a / 127 };
    default: return null;
  }
}

/** The mod wheel is a source (routed in the Mod view), so Learn leaves it alone. */
export const MOD_WHEEL_CC = 1;

/** General MIDI kick, snare, closed hat and open hat play kit slots 1 to 4. */
export const GM_KIT = [36, 38, 42, 46] as const;

/** Cents above C0 of a MIDI note (60 is C4, 4800). */
export const noteCents = (note: number) => (note - 12) * 100;

/** What a note plays on `layer`: the kit slot and its pitch on a beat layer (the kit's GM notes
 *  only), else the note's pitch. null: the note plays nothing there. */
export function notePlays(layer: Layer, note: number): { slot: number; cents: number } | null {
  if (layer.kind !== 'beat') return { slot: 0, cents: noteCents(note) };
  const slot = (GM_KIT as readonly number[]).indexOf(note);
  if (slot < 0) return null;
  // The slot's own pitch, so that letting go of one pad does not release another.
  return { slot, cents: layer.kit[slot]?.cents ?? noteCents(note) };
}

/** The layer MIDI `channel` plays: the one the project names for it, else the selected layer. */
export function channelLayer(doc: Project, channel: number, selected: string): Layer | undefined {
  const id = doc.perform?.channels[channel] || selected;
  return doc.layers.find((l) => l.id === id) ?? doc.layers.find((l) => l.id === selected);
}

/** A mapping kept in settings as a device's default: the layer by its place in the project. */
export interface DefaultMapping { channel: number; cc: number; layer: number; key: string }

/** The project's mappings for `device`, as that device's default. */
export function toDefault(doc: Project, device: string): DefaultMapping[] {
  return (doc.perform?.map ?? []).filter((m) => m.device === device).flatMap((m) => {
    const layer = doc.layers.findIndex((l) => l.id === m.target.layer);
    return layer < 0 ? [] : [{ channel: m.channel, cc: m.cc, layer, key: m.target.key }];
  });
}

/** Does `target` name a layer of `doc` and a value of its patch (or a macro). */
export function targetExists(doc: Project, t: PerfTarget, keys: (l: Layer) => { has(k: string): boolean }): boolean {
  const l = doc.layers.find((x) => x.id === t.layer);
  return !!l && (/^macro[1-8]$/.test(t.key) || keys(l).has(t.key));
}

/** What controller (`device`, `channel`, `cc`) sets: the project's own map for that device, or,
 *  when the project has none for it, the device's default from settings. null: nothing. */
export function mappedTarget(doc: Project, defaults: DefaultMapping[] | undefined, device: string, channel: number, cc: number,
  keys: (l: Layer) => { has(k: string): boolean }): PerfTarget | null {
  const map: MidiMapping[] = doc.perform?.map ?? [];
  const own = map.filter((m) => m.device === device);
  let t: PerfTarget | null = null;
  if (own.length) t = own.find((m) => m.channel === channel && m.cc === cc)?.target ?? null;
  else {
    const d = defaults?.find((m) => m.channel === channel && m.cc === cc);
    const l = d ? doc.layers[d.layer] : undefined;
    t = d && l ? { layer: l.id, key: d.key } : null;
  }
  return t && targetExists(doc, t, keys) ? t : null;
}

/** The p-th percentile (0..100) of `xs`, nearest rank. NaN when empty. */
export function percentile(xs: readonly number[], p: number): number {
  if (!xs.length) return NaN;
  const s = [...xs].sort((a, b) => a - b);
  return s[Math.min(s.length - 1, Math.max(0, Math.ceil((p / 100) * s.length) - 1))];
}

