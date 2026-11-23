// Meter remap through the compiler (docs/meter.md, R1): what a locked clip sends the engine, that
// a project without patterns compiles exactly as before, and a sung take's curves after Feel and
// a remap.
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { AUDIO_HITS, POINT_RUN, type TrackInput } from '../m1/track';
import { Compiler } from './compile';
import { flexClip, quantizeFlex, warpToTake } from './flex';
import { METER1, remapEvents, remapRuns, remapWarp, playedAnchors } from './meter';
import { migrate } from './migrate';
import { commands } from './commands';
import { applyOps } from './patch';
import type { BeatClip, MelodyClip, Project, Take } from './types';
import { PPQ } from './types';

const fixture = (name: string) => {
  const dir = resolve(import.meta.dirname, '../golden/fixtures', name);
  const doc = migrate(JSON.parse(readFileSync(resolve(dir, 'doc.json'), 'utf8')));
  const f = readFileSync(resolve(dir, 'curves', 'take_golden1.f32'));
  return { doc, curve: new Float32Array(f.buffer.slice(f.byteOffset, f.byteOffset + f.byteLength)) };
};

const run = <K extends keyof typeof commands>(doc: Project, k: K, a: Parameters<(typeof commands)[K]>[1]): Project =>
  applyOps(doc, (commands[k] as (d: Project, x: unknown) => ReturnType<typeof commands.setMeter>)(doc, a)).doc;

function input(doc: Project, curve: Float32Array | null = null, audio?: (t: Take) => Float32Array | null): TrackInput {
  const c = new Compiler();
  c.compile(doc, () => curve, { audioFor: audio });
  return (c as unknown as { trackInput: (d: Project, a: (t: Take) => null) => TrackInput }).trackInput(doc, () => null);
}

function trackBlob(doc: Project, curve: Float32Array | null): Uint8Array {
  const up = new Compiler().compile(doc, () => curve).find((u) => u.what === 'track')!;
  return new Uint8Array(up.blob);
}

describe('compiling locked clips', () => {
  const { doc: sung, curve } = fixture('sung-melody');
  const melody = sung.layers[0].id;
  const sec = sung.sections[0].id;

  it('meters and locks without a pattern compile to the same track, byte for byte', () => {
    let d = run(sung, 'setMeter', {});
    d = run(d, 'lockClips', { layer: melody, meter: METER1 });
    expect(d.meters).toHaveLength(2);
    expect(Buffer.from(trackBlob(d, curve)).equals(Buffer.from(trackBlob(sung, curve)))).toBe(true);
    // A pattern on another meter does not touch the section or the clip locked to Meter 1.
    const other = d.meters![1].id;
    d = run(d, 'setMeterPlay', { section: sec, meter: other, play: [1, 2, 3] });
    expect(Buffer.from(trackBlob(d, curve)).equals(Buffer.from(trackBlob(sung, curve)))).toBe(true);
  });

  it('Meter 1 at 1 2 3 shortens the section; a locked clip loops in 3/4, a free one as written', () => {
    let d = run(sung, 'setMeterPlay', { section: sec, meter: METER1, play: [1, 2, 3] });
    const free = input(d, curve);
    expect(free.sections[0].length).toBe(6 * PPQ);
    expect(free.clips[0].loopLen).toBe(8 * PPQ);
    expect(free.clips[0].events.map((e) => e.t)).toEqual((sung.sections[0].clips[melody] as MelodyClip).notes.map((n) => n.t));
    d = run(d, 'lockClips', { layer: melody, meter: METER1 });
    const locked = input(d, curve);
    expect(locked.clips[0].loopLen).toBe(6 * PPQ);
    // Notes on written beats 4 and 8 are gone; the others moved back a beat from the second bar on.
    const notes = (sung.sections[0].clips[melody] as MelodyClip).notes;
    const beatOf = (t: number) => Math.floor(t / PPQ);
    const kept = notes.filter((n) => beatOf(n.t) % 4 !== 3).map((n) => n.t - (beatOf(n.t) >= 4 ? PPQ : 0));
    expect(locked.clips[0].events.map((e) => Math.round(e.t))).toEqual(kept.map((t) => Math.round(t)));
    // Removing the pattern gives the written length back.
    const back = input(run(d, 'setMeterPlay', { section: sec, meter: METER1, play: null }), curve);
    expect(back.sections[0].length).toBe(8 * PPQ);
  });

  it('a locked take with audio and no Feel sends a stretch map in hits mode', () => {
    let d = run(sung, 'setMeterPlay', { section: sec, meter: METER1, play: [1, 3, 4, 2] });
    d = run(d, 'lockClips', { layer: melody, meter: METER1 });
    // The classic patch has no input element, so give the compiler the take's audio by asking for
    // it on a layer that plays its raw input.
    d = applyOps(d, commands.addChannelFx(d, { layer: melody, type: 'input' })).doc;
    const audio = new Float32Array(48000 * 5);
    const c = input(d, curve, () => audio).clips[0];
    expect(c.audioMode).toBe(AUDIO_HITS);
    const runs = c.audioWarp!.filter((p) => (p.flags ?? 0) & POINT_RUN);
    // Two bars of 1 3 4 2: three joins a bar.
    expect(runs.length).toBeGreaterThanOrEqual(6);
    for (let i = 1; i < c.audioWarp!.length; i++) expect(c.audioWarp![i].clipT).toBeGreaterThanOrEqual(c.audioWarp![i - 1].clipT);
  });

  it('a locked beat clip drops its hits on dropped beats and plays the others in order', () => {
    const doc = migrate(JSON.parse(readFileSync(resolve(import.meta.dirname, '../golden/fixtures/demo-beat/doc.json'), 'utf8')));
    const s = doc.sections[0];
    const beat = doc.layers.find((l) => l.kind === 'beat')!.id;
    let d = run(doc, 'setMeterPlay', { section: s.id, meter: METER1, play: [1, 2, 3] });
    d = run(d, 'lockClips', { layer: beat, meter: METER1 });
    const hits = (s.clips[beat] as BeatClip).hits;
    const out = input(d).clips.find((c) => c.layer === doc.layers.findIndex((l) => l.id === beat))!;
    const L = (s.clips[beat] as BeatClip).loopLen;
    const nBeats = Math.ceil(L / PPQ);
    // Every hit whose start is in a kept beat (no anchors: cuts at the beats) plays once.
    const kept = hits.filter((h) => Math.floor(h.t / PPQ) % 4 !== 3 && Math.floor(h.t / PPQ) < nBeats);
    expect(out.events).toHaveLength(kept.length);
  });
});

describe('a flexed and remapped sung take', () => {
  const { doc: sung } = fixture('sung-melody');
  const melody = sung.layers[0].id;
  const base = sung.sections[0].clips[melody] as MelodyClip;
  const clip: MelodyClip = { ...base, flex: { ...quantizeFlex(undefined, PPQ / 2), strength: 1 } };
  const ups = (sung.sections[0].bpm * PPQ) / 60;
  const flexed = flexClip(clip as MelodyClip & { flex: NonNullable<MelodyClip['flex']> }, null, ups);

  for (const pattern of [[1, 3, 4, 2], [1, 2, 3, 3], [1, 2, 3]]) {
    it(`every note's curve starts within 1 ms of its moved note, copies included (${pattern.join(' ')})`, () => {
      const L = clip.loopLen;
      // The cuts fall at the notes as Feel plays them (the clip has no stored anchors).
      const anchors = playedAnchors(flexed.notes!.map((n) => ({ t: n.t, w: 1, hard: true, kit: -1 })), null, L);
      const r = remapRuns({ loopLen: L, perBar: 4, pattern, ups, anchors })!;
      const notes = remapEvents(flexed.notes!, r, L, false);
      const warp = remapWarp(flexed.warp!, r, 1 / ups);
      expect(notes.length).toBeGreaterThan(0);
      for (const n of notes) {
        const src = flexed.notes!.find((x) => x.id === n.id)!;
        const want = warpToTake(flexed.warp!, src.t, 1 / ups);
        const got = warpToTake(warp, n.t, 1 / ups);
        expect(Math.abs(got - want) * 1000, `note ${n.id} at ${n.t}`).toBeLessThanOrEqual(1);
      }
    });
  }
});

describe('meter commands', () => {
  const { doc } = fixture('sung-melody');
  const melody = doc.layers[0].id;
  const sec = doc.sections[0].id;

  it('add, rename, set beats, remove: removing frees its clips and drops its patterns', () => {
    let d = run(doc, 'setMeter', { beats: 3 });
    expect(d.schema).toBe(7);
    expect(d.meters!.map((m) => [m.name, m.bars.beats])).toEqual([['Meter 1', 4], ['Meter 2', 3]]);
    const m2 = d.meters![1].id;
    d = run(d, 'setMeterPlay', { section: sec, meter: m2, play: [1, 3, 2] });
    d = run(d, 'lockClips', { layer: melody, meter: m2 });
    expect(d.sections[0].clips[melody].meter).toBe(m2);
    d = run(d, 'setMeter', { id: m2, name: 'Shaker' });
    expect(d.meters![1].name).toBe('Shaker');
    d = run(d, 'setMeter', { id: m2, remove: true });
    expect(d.meters).toHaveLength(1);
    expect(d.sections[0].play).toBeUndefined();
    expect(d.sections[0].clips[melody].meter).toBeUndefined();
    expect(() => run(d, 'setMeter', { id: METER1, remove: true })).toThrow(/cannot be removed/);
    expect(() => run(d, 'setMeter', { beats: 17 })).toThrow(RangeError);
  });

  it("Meter 1's beats per bar are the click's bar, both ways", () => {
    let d = run(doc, 'setMeter', { id: METER1, beats: 3 });
    expect(d.metro!.bar).toBe(3);
    expect(d.meters![0].bars.beats).toBe(3);
    d = run(d, 'setMetro', { bar: 5 });
    expect(d.meters![0].bars.beats).toBe(5);
  });

  it('patterns: checked against the meter, as written clears, all copies to every grid section, undo restores', () => {
    expect(() => run(doc, 'setMeterPlay', { section: sec, meter: METER1, play: [1, 5] })).toThrow(/1 to 4/);
    expect(() => run(doc, 'setMeterPlay', { section: sec, meter: 'nope', play: [1] })).toThrow(/no meter/);
    let d = run(doc, 'addSection', { feel: 'grid', beats: 8 });
    d = run(d, 'addSection', { feel: 'free', seconds: 4 });
    const ops = commands.setMeterPlay(d, { section: sec, meter: METER1, play: [1, 3, 4, 2], all: true });
    const { doc: all, inverse } = applyOps(d, ops);
    expect(all.sections.map((s) => s.play?.[METER1])).toEqual([[1, 3, 4, 2], [1, 3, 4, 2], undefined]);
    expect(applyOps(all, inverse).doc.sections.map((s) => s.play)).toEqual([undefined, undefined, undefined]);
    expect(commands.setMeterPlay(all, { section: sec, meter: METER1, play: [1, 3, 4, 2] })).toEqual([]);
    const cleared = run(all, 'setMeterPlay', { section: sec, meter: METER1, play: null });
    expect(cleared.sections[0].play).toBeUndefined();
    expect(() => run(d, 'setMeterPlay', { section: d.sections[2].id, meter: METER1, play: [1] })).toThrow(/free section/);
  });

  it('locks one clip or all of a layer, and frees them', () => {
    let d = run(doc, 'lockClips', { layer: melody, section: sec, meter: METER1 });
    expect(d.sections[0].clips[melody].meter).toBe(METER1);
    expect(commands.lockClips(d, { layer: melody, meter: METER1 })).toEqual([]);
    d = run(d, 'lockClips', { layer: melody, meter: null });
    expect('meter' in d.sections[0].clips[melody] && d.sections[0].clips[melody].meter !== undefined).toBe(false);
    expect(() => run(d, 'lockClips', { layer: doc.layers[1].id, section: sec, meter: METER1 })).toThrow(/no clip/);
  });
});
