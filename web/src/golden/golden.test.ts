// Golden renders (docs/synth.md, stage S1): every fixture project, compiled by the compiler and
// rendered by the engine from before the modular synth, against the same project migrated to the
// current schema, compiled and rendered by the current code. A render passes when no sample differs by more than -90 dBFS. A case that does not is
// listed in KNOWN with the level it reached and why, for the owner to judge by ear.
//
// The fixtures were written by build-fixtures.test.ts in the commit that added them (it built schema 3
// documents with the app as it was, so it was removed once the app moved on).
// The reference side does not use the current protocol or renderer: it loads the stored blobs
// into klont-reference.wasm (a copy of the engine before S1) with its own copy of the old buffer
// table layout, so it keeps working whatever the current engine becomes.
import { readFileSync, readdirSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { uploadsToSong } from '../app/mix';
import { renderSong } from '../m1/render-core';
import { instantiateEngine } from '../m1/protocol';
import { exportProject, importProject } from '../project/archive';
import { Compiler } from '../project/compile';
import { historyResets, migrate } from '../project/migrate';
import type { Layer, Project, Take } from '../project/types';

const SR = 48000;
const CHUNK = 4096;
const MAX_S = 20;
const here = import.meta.dirname;
const fixtures = resolve(here, 'fixtures');
const refWasm = readFileSync(resolve(here, 'klont-reference.wasm'));
const curWasm = readFileSync(resolve(here, '../../public/wasm/klont.wasm'));

/** Worst difference allowed for a case that is not listed in KNOWN. */
const LIMIT_DB = -90;
/** Cases that differ by more than LIMIT_DB, with the level they reached and the reason. */
const KNOWN: Record<string, { db: number; why: string }> = {
  // Both come from the bit crusher: its quantizer turns last-bit differences in its input (from
  // sums now taken in another order) into one step of its output. With the crusher at 16 bits
  // both renders match within -121 dB.
  arrangement: { db: -89.8, why: 'one bit-crusher step on the beat layer (grit macro)' },
  'm1-demo': { db: -69.8, why: 'one 10-bit crusher step on the texture layer' },
};

// ---- reference engine, old ABI (protocol before S1)

const OLD = { LOAD_TRACK: 1, MSG_PLAY: 6, MSG_BYTES: 16, BT: { outputL: 1, outputR: 2, control: 4, controlCount: 6, staging: 9 } };

interface RefUpload { kind: number; slot: number; file: string }

function refRender(dir: string, frames: number): { l: Float32Array; r: Float32Array } {
  const list: RefUpload[] = JSON.parse(readFileSync(resolve(dir, 'uploads.json'), 'utf8'));
  return renderOld(list.map((u) => ({ kind: u.kind, slot: u.slot, blob: readFileSync(resolve(dir, 'uploads', u.file)) })), frames);
}

function renderOld(uploads: { kind: number; slot: number; blob: Uint8Array }[], frames: number): { l: Float32Array; r: Float32Array } {
  const { ex, e } = instantiateEngine(refWasm, SR, CHUNK);
  const table = () => new Uint32Array(ex.memory.buffer as ArrayBuffer, ex.ew_buffers(e), 15);
  // Layers, states, curves and grains first, then the track with its loop flag cleared (one pass), as renderSong did.
  const order = (k: number) => (k === OLD.LOAD_TRACK ? 1 : 0);
  for (const u of [...uploads].sort((a, b) => order(a.kind) - order(b.kind))) {
    let blob = u.blob;
    if (u.kind === OLD.LOAD_TRACK) {
      blob = new Uint8Array(blob);
      new DataView(blob.buffer, blob.byteOffset).setUint32(20, 0, true);
    }
    new Uint8Array(ex.memory.buffer as ArrayBuffer, table()[OLD.BT.staging], blob.length).set(blob);
    const s = ex.ew_load(e, u.slot, u.kind, table()[OLD.BT.staging], blob.length);
    if (s !== 0) throw new Error(`reference load ${u.kind}/${u.slot} failed: ${s}`);
  }
  const dv = new DataView(ex.memory.buffer as ArrayBuffer, table()[OLD.BT.control], OLD.MSG_BYTES);
  dv.setUint16(0, OLD.MSG_PLAY, true);
  new Uint32Array(ex.memory.buffer as ArrayBuffer, table()[OLD.BT.controlCount], 1)[0] = 1;
  const l = new Float32Array(frames), r = new Float32Array(frames);
  for (let pos = 0; pos < frames; pos += CHUNK) {
    const n = Math.min(CHUNK, frames - pos);
    ex.ew_render(e, n);
    const t = table();
    l.set(new Float32Array(ex.memory.buffer as ArrayBuffer, t[OLD.BT.outputL], n), pos);
    r.set(new Float32Array(ex.memory.buffer as ArrayBuffer, t[OLD.BT.outputR], n), pos);
  }
  ex.ew_free(e);
  return { l, r };
}

// ---- current code

const f32File = (p: string) => {
  const b = readFileSync(p);
  return new Float32Array(b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength));
};

function loadCase(dir: string): { doc: Project; curves: Map<string, Float32Array>; blobs: Map<string, Float32Array> } {
  const doc = JSON.parse(readFileSync(resolve(dir, 'doc.json'), 'utf8')) as Project;
  const curves = new Map(readdirSync(resolve(dir, 'curves')).map((f) => [f.replace(/\.f32$/, ''), f32File(resolve(dir, 'curves', f))]));
  const blobs = new Map(readdirSync(resolve(dir, 'blobs')).map((f) => [f.replace(/\.f32$/, ''), f32File(resolve(dir, 'blobs', f))]));
  return { doc, curves, blobs };
}

function compileCurrent(doc: Project, curves: Map<string, Float32Array>, blobs: Map<string, Float32Array>) {
  return new Compiler().compile(doc, (take: Take) => curves.get(take.id) ?? null, {
    grainFor: (l: Layer) => {
      const g = l.influence?.grain;
      const src = l.influence ? blobs.get(l.influence.source) : undefined;
      return g && src ? { data: src.subarray(g.start, g.end), rate: l.influence!.sourceRate, refCents: g.refCents } : null;
    },
  });
}

async function currentRender(dir: string, frames: number): Promise<{ l: Float32Array; r: Float32Array }> {
  const { doc, curves, blobs } = loadCase(dir);
  const uploads = compileCurrent(migrate(doc), curves, blobs);
  const { ex } = instantiateEngine(curWasm, SR, CHUNK);
  const res = await renderSong(ex, uploadsToSong(uploads), { maxSeconds: frames / SR });
  return { l: res.left, r: res.right };
}

// ---- compare

/** Worst sample difference in dBFS (-Infinity when identical) and where it is. */
function worst(a: { l: Float32Array; r: Float32Array }, b: { l: Float32Array; r: Float32Array }, frames: number): { db: number; at: number } {
  let m = 0, at = 0;
  for (let i = 0; i < frames; i++) {
    const d = Math.max(Math.abs((a.l[i] ?? 0) - (b.l[i] ?? 0)), Math.abs((a.r[i] ?? 0) - (b.r[i] ?? 0)));
    if (d > m) { m = d; at = i; }
  }
  return { db: m > 0 ? 20 * Math.log10(m) : -Infinity, at: at / SR };
}

const summary: Record<string, { seconds: number }> = JSON.parse(readFileSync(resolve(fixtures, 'summary.json'), 'utf8'));

function verdict(name: string, w: { db: number; at: number }): void {
  const known = KNOWN[name];
  console.log(`golden ${name}: worst difference ${w.db.toFixed(1)} dBFS at ${w.at.toFixed(3)} s${known ? ` (known: ${known.why})` : ''}`);
  if (known) expect(w.db).toBeLessThanOrEqual(known.db + 1);
  else expect(w.db).toBeLessThanOrEqual(LIMIT_DB);
}

describe('golden renders', () => {
  for (const name of Object.keys(summary)) {
    it(name, async () => {
      const dir = resolve(fixtures, name);
      const frames = Math.min(Math.round(summary[name].seconds * SR), MAX_S * SR);
      const ref = refRender(dir, frames);
      const cur = await currentRender(dir, frames);
      verdict(name, worst(ref, cur, frames));
    }, 60_000);
  }

  it('m1-demo', async () => {
    const dir = resolve(fixtures, 'm1-demo');
    const frames = MAX_S * SR;
    const blobs = [...Array(8).keys()].map((i) => ({ kind: 2, slot: i, blob: readFileSync(resolve(dir, `layer${i}.bin`)) }));
    const ref = renderOld([...blobs, { kind: 1, slot: 0, blob: readFileSync(resolve(dir, 'track.bin')) }], frames);
    const { encodeDemo } = await import('../m1/demo');
    const song = encodeDemo();
    const { ex } = instantiateEngine(curWasm, SR, CHUNK);
    const res = await renderSong(ex, song, { maxSeconds: MAX_S });
    verdict('m1-demo', worst(ref, { l: res.left, r: res.right }, frames));
  }, 60_000);

  it('a schema 3 project zip imports, plays and says its undo history starts fresh', async () => {
    const dir = resolve(fixtures, 'sung-melody');
    const { doc, curves, blobs } = loadCase(dir);
    expect(doc.schema).toBe(3);
    const zip = await exportProject(doc, async (ref) => blobs.get(ref) ?? (ref.startsWith('audio-') ? new Float32Array(480) : null));
    expect(zip.missing).toEqual([]);
    const got = await importProject(zip.bytes);
    expect(got.savedSchema).toBe(3);
    expect(historyResets(got.savedSchema)).toBe(true);
    expect(got.doc.schema).toBe(4);
    const { ex } = instantiateEngine(curWasm, SR, CHUNK);
    const res = await renderSong(ex, uploadsToSong(compileCurrent(got.doc, curves, got.blobs)), { maxSeconds: 6 });
    expect(res.peak).toBeGreaterThan(0.1);
  }, 60_000);
});
