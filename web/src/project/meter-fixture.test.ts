// The meter prototype's six cases through the app's compiler (docs/meter.md, R1). The grooves'
// stems and their anchors come from crates/analysis/tests/fixtures/meter_anchors.txt (written by
// crates/analysis/tests/meter.rs); each case is compiled as the app compiles three layers locked
// to Meter 1, and the stretch maps go to meter_maps.txt beside it, which the Rust test plays
// through the engine. WRITE_FIXTURE=1 writes the maps; otherwise they must match.
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { type TrackInput } from '../m1/track';
import { commands, newProject } from './commands';
import { Compiler } from './compile';
import { METER1, remapRuns, playedAnchors } from './meter';
import { applyOps } from './patch';
import { PPQ, type BeatClip, type MelodyClip, type Project, type Take } from './types';

const dir = resolve(import.meta.dirname, '../../../crates/analysis/tests/fixtures');
const anchorsPath = resolve(dir, 'meter_anchors.txt');
const mapsPath = resolve(dir, 'meter_maps.txt');

interface Groove { name: string; bpm: number; meter: number; bars: number; pre: number; take: number; stems: Float32Array[] }

function grooves(): Map<string, Groove> {
  const out = new Map<string, Groove>();
  const lines = readFileSync(anchorsPath, 'utf8').split('\n').filter((l) => l && !l.startsWith('#'));
  let g: Groove | null = null;
  for (let i = 0; i < lines.length; i++) {
    const w = lines[i].split(' ');
    const val = (k: string) => Number(w[w.indexOf(k) + 1]);
    if (w[0] === 'groove') {
      g = { name: w[1], bpm: val('bpm'), meter: val('meter'), bars: val('bars'), pre: val('pre'), take: val('take'), stems: [] };
      out.set(g.name, g);
    } else if (w[0] === 'stem') {
      const n = Number(w[2]);
      const a = new Float32Array(n * 3);
      for (let k = 0; k < n; k++) {
        const [t, wt] = lines[++i].split(' ').map(Number);
        a.set([t, wt, 1], k * 3); // hard
      }
      g!.stems.push(a);
    }
  }
  return out;
}

const CASES: [string, string, number[]][] = [
  ['five-drop5', 'five', [1, 2, 3, 4]],
  ['five-drop4', 'five', [1, 2, 3, 5]],
  ['four-drop4', 'four', [1, 2, 3]],
  ['three-repeat3', 'three', [1, 2, 3, 3]],
  ['three-repeat1', 'three', [1, 1, 2, 3]],
  ['four-shuffle', 'four', [1, 3, 4, 2]],
];

/** A project with one section of the groove and three layers (drums, bass, keys) whose takes are
 *  the stems, placed with beat 1 at the section start, each layer playing its raw input. */
function project(g: Groove, pattern: number[]): { doc: Project; anchors: Map<string, Float32Array> } {
  const run = (d: Project, ops: ReturnType<typeof commands.setMeter>) => applyOps(d, ops).doc;
  let d = newProject('meter fixture');
  d = run(d, commands.setSection(d, { id: d.sections[0].id, beats: g.meter * g.bars, bpm: g.bpm }));
  for (const l of [...d.layers]) d = run(d, commands.removeLayer(d, { id: l.id }));
  d = run(d, commands.addLayer(d, { kind: 'beat', name: 'Drums' }));
  d = run(d, commands.addLayer(d, { kind: 'melody', name: 'Bass' }));
  d = run(d, commands.addLayer(d, { kind: 'melody', name: 'Keys' }));
  const anchors = new Map<string, Float32Array>();
  const sec = d.sections[0];
  const len = g.meter * g.bars * PPQ;
  d.layers.forEach((l, s) => {
    d = run(d, commands.addChannelFx(d, { layer: l.id, type: 'input' }));
    const take: Take = { id: `take-${s}`, audio: `a-${s}`, frames: `f-${s}`, rate: 48000, latencyMs: 0, frameCount: Math.round(g.take * 100),
      kind: l.kind, refCents: 0, root: 0, scale: 0, bpm: g.bpm, anchors: `an-${s}` };
    anchors.set(take.id, g.stems[s]);
    d = run(d, commands.addTake(d, { take }));
    const warp = [{ takeSec: g.pre, clipT: 0 }];
    const clip: BeatClip | MelodyClip = l.kind === 'beat'
      ? { kind: 'beat', take: take.id, takes: [take.id], loopLen: len, timeSnap: 0, morph: [], hits: [], warp }
      : { kind: 'melody', take: take.id, takes: [take.id], loopLen: len, timeSnap: 0, morph: [], notes: [], warp };
    d = run(d, commands.setClip(d, { section: sec.id, layer: l.id, clip }));
    d = run(d, commands.lockClips(d, { layer: l.id, meter: METER1 }));
  });
  d = run(d, commands.setMeter(d, { id: METER1, beats: g.meter }));
  d = run(d, commands.setMeterPlay(d, { section: sec.id, meter: METER1, play: pattern }));
  return { doc: d, anchors };
}

function compile(doc: Project, anchors: Map<string, Float32Array>): TrackInput {
  const audio = new Float32Array(16);
  const c = new Compiler();
  const anchorsFor = (t: Take) => anchors.get(t.id) ?? null;
  c.compile(doc, () => null, { audioFor: () => audio, anchorsFor });
  return (c as unknown as { trackInput: (d: Project, a: typeof anchorsFor) => TrackInput }).trackInput(doc, anchorsFor);
}

function mapsText(): string {
  const gs = grooves();
  let s = '# Written by web/src/project/meter-fixture.test.ts (WRITE_FIXTURE=1): the stretch map each stem\'s clip sends the engine, per case.\n';
  for (const [name, gname, pattern] of CASES) {
    const g = gs.get(gname)!;
    const { doc, anchors } = project(g, pattern);
    const t = compile(doc, anchors);
    const ups = (g.bpm * PPQ) / 60;
    s += `case ${name} groove ${gname} pattern ${pattern.join(' ')} ups ${ups} loop ${t.clips[0].loopLen} section ${t.sections[0].length}\n`;
    t.clips.forEach((c, k) => {
      const m = c.audioWarp ?? [];
      s += `stem ${k} ${m.length}\n`;
      for (const p of m) s += `${p.clipT} ${p.takeSec} ${p.flags ?? 0}\n`;
    });
    // Where runs start in the drums' clip (the joins the measures look at), from the same runs.
    const L = doc.sections[0].clips[doc.layers[0].id].loopLen;
    const an = anchors.get('take-0')!;
    const placed = [];
    for (let i = 0; i < an.length; i += 3) placed.push({ t: (((an[i] - g.pre) * ups) % L + L) % L, w: an[i + 1], hard: true, kit: -1 });
    const r = remapRuns({ loopLen: L, perBar: g.meter, pattern, ups, anchors: playedAnchors(placed, null, L) })!;
    s += `joins ${r.runs.length} ${r.runs.map((x) => ((x.a % r.loopLen) + r.loopLen) % r.loopLen).join(' ')}\n`;
  }
  return s;
}

describe('the meter prototype cases through the compiler', () => {
  it('match crates/analysis/tests/fixtures/meter_maps.txt', () => {
    const got = mapsText();
    if (process.env.WRITE_FIXTURE === '1') writeFileSync(mapsPath, got);
    expect(existsSync(mapsPath), 'run with WRITE_FIXTURE=1').toBe(true);
    expect(got).toBe(readFileSync(mapsPath, 'utf8'));
  });
});
