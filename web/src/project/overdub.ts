// Overdub: put a fresh take on the timeline so it lines up with what the singer heard.
// TDD "Devices and calibration", take alignment: a mic sample's clip time is the transport
// time at capture minus the round trip (speaker out -> ear -> mic in). The singer sang in
// time with the transport they heard, and that sound reached the mic one round trip later.

import { newId } from './commands';
import { M_CLOCK_HI, M_CLOCK_LO, M_PLAYING, M_SECTION, M_SECTION_POS, M_UNITS_PER_SAMPLE } from '../m1/protocol';
import type { Hit as TakeHit, Note as TakeNote } from '../m2/analysis';
import { sectionLength, sectionUnitsPerSec } from './compile';
import type { BeatClip, Clip, Hit, MelodyClip, Note, Project, Take, WarpAnchor } from './types';

/** The newest transport reading from the engine's meter block. */
export interface ClockMark {
  /** Engine clock: samples processed since init, at the end of the last call. */
  sample: number;
  section: number;
  /** Section units (ticks or seconds) at `sample`. */
  pos: number;
  unitsPerSample: number;
  playing: boolean;
}

const TWO_20 = 1 << 20;

/** Read a ClockMark from a meter block (m1/protocol.ts M_CLOCK_*). */
export function clockMark(meters: Float32Array): ClockMark {
  return {
    sample: meters[M_CLOCK_HI] * TWO_20 + meters[M_CLOCK_LO],
    section: meters[M_SECTION],
    pos: meters[M_SECTION_POS],
    unitsPerSample: meters[M_UNITS_PER_SAMPLE],
    playing: meters[M_PLAYING] === 1,
  };
}

/** Transport position of engine sample `s`, extrapolated from `mark` at the speed it reported.
 *  It stays inside the mark's section: it never wraps or moves to another section, so it can
 *  come out below 0 or past the section end. Callers wrap it into the loop they care about.
 *  A tempo ramp or a section end between the mark and `s` makes it approximate. */
export function positionAt(mark: ClockMark, s: number): { section: number; pos: number } {
  const ups = mark.playing ? mark.unitsPerSample : 0;
  return { section: mark.section, pos: mark.pos + (s - mark.sample) * ups };
}


const wrap = (x: number, len: number) => ((x % len) + len) % len;

export interface PlaceTakeArgs {
  doc: Project;
  sectionId: string;
  layerId: string;
  take: Take;
  /** Engine clock sample at which the take's first sample was captured. */
  startSample: number;
  /** A reading taken at or after the take (any recent one works while the tempo is steady). */
  mark: ClockMark;
  roundTripSamples: number;
  /** Second-pass results, in take seconds. */
  notes: TakeNote[];
  hits: TakeHit[];
}

/** Build the layer's clip for a new take.
 *
 *  Take second x lands at clip time p0 + x * unitsPerSec, where p0 is the transport position at
 *  (startSample - roundTrip) and unitsPerSec is the section's nominal rate. It is wrapped into
 *  [0, loopLen) per note or hit; loopLen is the existing clip's, else the section length.
 *  If the mark is in a different section from the target, p0 is 0 (nothing to line up with).
 *
 *  Warp anchors map clip time to take seconds (curve.rs warp_to_take, piecewise linear between
 *  anchors sorted by clip time). There is one at every note start and end. Each note's start is
 *  wrapped by itself, but its end is start + dur, which may pass loopLen: the engine's voices
 *  keep counting past the loop end while a note sounds, so the end anchor stays consistent
 *  with its start. Hits carry no curve, so beat clips get no anchors. */
export function placeTake(o: PlaceTakeArgs): Clip {
  const si = o.doc.sections.findIndex((s) => s.id === o.sectionId);
  if (si < 0) throw new RangeError(`no section ${o.sectionId}`);
  const section = o.doc.sections[si];
  const layer = o.doc.layers.find((l) => l.id === o.layerId);
  if (!layer) throw new RangeError(`no layer ${o.layerId}`);
  const existing = section.clips[o.layerId];
  const loopLen = existing?.loopLen ?? sectionLength(section);
  if (!(loopLen > 0)) throw new RangeError('loop length must be above 0');
  const ups = sectionUnitsPerSec(section);
  const at = positionAt(o.mark, o.startSample - o.roundTripSamples);
  const p0 = at.section === si ? at.pos : 0;
  const place = (sec: number) => wrap(p0 + sec * ups, loopLen);

  const base = {
    take: o.take.id,
    takes: [...(existing?.takes ?? []).filter((id) => id !== o.take.id), o.take.id],
    loopLen,
    timeSnap: existing?.timeSnap ?? 0,
    // A new take keeps the morph lane of the clip it replaces.
    morph: existing?.morph ?? [],
  };

  if (layer.kind === 'beat') {
    const hits: Hit[] = o.hits.map((h) => ({
      id: newId('hit'), t: place(h.t), dur: h.dur * ups, slot: h.slot, strength: h.strength, shape: { ...h.shape },
    }));
    hits.sort((a, b) => a.t - b.t);
    const clip: BeatClip = { kind: 'beat', ...base, hits };
    return clip;
  }

  const notes: Note[] = [];
  const warp: WarpAnchor[] = [];
  for (const n of o.notes) {
    const t = place(n.t);
    const dur = n.dur * ups;
    notes.push({ id: newId('note'), t, dur, cents: n.cents, hard: n.hard });
    warp.push({ takeSec: n.t, clipT: t }, { takeSec: n.t + n.dur, clipT: t + dur });
  }
  notes.sort((a, b) => a.t - b.t);
  warp.sort((a, b) => a.clipT - b.clipT || a.takeSec - b.takeSec);
  const clip: MelodyClip = { kind: layer.kind, ...base, notes, warp };
  return clip;
}
