// A project file is one zip: project.json plus every blob the document references
// (TDD "Project file"). Stored (uncompressed) entries, and deflate on the way in,
// so a zip from the app and a zip from an ordinary archiver both open.

import { migrate } from './migrate';
import type { Project } from './types';

export const MAX_ARCHIVE_BYTES = 256 * 1024 * 1024;
const MAX_FILES = 4000;

export interface ArchivedProject {
  doc: Project;
  blobs: Map<string, Float32Array>;
  /** The schema the file was saved with. */
  savedSchema: number;
}

const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

function crc32(data: Uint8Array): number {
  let c = 0xffffffff;
  for (let i = 0; i < data.length; i++) c = CRC_TABLE[(c ^ data[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function u16(n: number): number[] { return [n & 0xff, (n >>> 8) & 0xff]; }
function u32(n: number): number[] { return [n & 0xff, (n >>> 8) & 0xff, (n >>> 16) & 0xff, (n >>> 24) & 0xff]; }

interface ZipItem { name: string; data: Uint8Array }

/** Zip of stored (method 0) files. Names are relative and have no drive letters. */
export function zipStore(files: ZipItem[]): Uint8Array {
  const locals: Uint8Array[] = [];
  const central: number[][] = [];
  let offset = 0;
  for (const f of files) {
    const name = new TextEncoder().encode(f.name);
    if (name.length > 0xffff) throw new RangeError(`name too long: ${f.name}`);
    const crc = crc32(f.data);
    const local = [0x50, 0x4b, 0x03, 0x04, ...u16(20), ...u16(0), ...u16(0), ...u16(0), ...u16(0),
      ...u32(crc), ...u32(f.data.length), ...u32(f.data.length), ...u16(name.length), ...u16(0)];
    const head = new Uint8Array(local.length + name.length + f.data.length);
    head.set(local, 0);
    head.set(name, local.length);
    head.set(f.data, local.length + name.length);
    locals.push(head);
    central.push([0x50, 0x4b, 0x01, 0x02, ...u16(20), ...u16(20), ...u16(0), ...u16(0), ...u16(0), ...u16(0),
      ...u32(crc), ...u32(f.data.length), ...u32(f.data.length), ...u16(name.length), ...u16(0), ...u16(0),
      ...u16(0), ...u16(0), ...u32(0), ...u32(offset), ...name]);
    offset += head.length;
  }
  const cd = central.flat();
  const eocd = [0x50, 0x4b, 0x05, 0x06, ...u16(0), ...u16(0), ...u16(files.length), ...u16(files.length),
    ...u32(cd.length), ...u32(offset), ...u16(0)];
  const out = new Uint8Array(offset + cd.length + eocd.length);
  let p = 0;
  for (const part of locals) { out.set(part, p); p += part.length; }
  out.set(cd, p); p += cd.length;
  out.set(eocd, p);
  return out;
}

function readU16(b: Uint8Array, o: number): number { return b[o] | (b[o + 1] << 8); }
function readU32(b: Uint8Array, o: number): number { return (b[o] | (b[o + 1] << 8) | (b[o + 2] << 16) | (b[o + 3] << 24)) >>> 0; }

async function inflate(data: Uint8Array): Promise<Uint8Array> {
  if (typeof DecompressionStream === 'undefined') throw new Error('This zip is compressed and this browser cannot inflate it.');
  const copy = new ArrayBuffer(data.byteLength);
  new Uint8Array(copy).set(data);
  const stream = new Blob([copy]).stream().pipeThrough(new DecompressionStream('deflate-raw'));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

function findEocd(b: Uint8Array): number {
  const start = Math.max(0, b.length - 22 - 0xffff);
  for (let i = b.length - 22; i >= start; i--) {
    if (b[i] === 0x50 && b[i + 1] === 0x4b && b[i + 2] === 0x05 && b[i + 3] === 0x06) return i;
  }
  throw new Error('This file is not a zip.');
}

/** Read a zip. Method 0 (stored) and method 8 (deflate) are accepted. */
export async function unzip(bytes: Uint8Array): Promise<ZipItem[]> {
  if (bytes.length > MAX_ARCHIVE_BYTES) throw new Error('That file is over 256 MB.');
  const e = findEocd(bytes);
  const count = readU16(bytes, e + 10);
  if (count > MAX_FILES) throw new Error('That zip has too many files.');
  let p = readU32(bytes, e + 16);
  const out: ZipItem[] = [];
  for (let i = 0; i < count; i++) {
    if (readU32(bytes, p) !== 0x02014b50) throw new Error('The zip directory is damaged.');
    const method = readU16(bytes, p + 10);
    const comp = readU32(bytes, p + 20);
    const raw = readU32(bytes, p + 24);
    const nameLen = readU16(bytes, p + 28);
    const extra = readU16(bytes, p + 30);
    const comment = readU16(bytes, p + 32);
    const local = readU32(bytes, p + 42);
    const name = new TextDecoder().decode(bytes.subarray(p + 46, p + 46 + nameLen));
    p += 46 + nameLen + extra + comment;
    if (!name || name.endsWith('/')) continue;
    if (name.includes('..') || name.startsWith('/') || name.includes('\\')) throw new Error(`Unsafe path in the zip: ${name}`);
    const sig = readU32(bytes, local);
    if (sig !== 0x04034b50) throw new Error('A file header in the zip is damaged.');
    const localName = readU16(bytes, local + 26);
    const localExtra = readU16(bytes, local + 28);
    const dataOff = local + 30 + localName + localExtra;
    const slice = bytes.subarray(dataOff, dataOff + comp);
    if (slice.length !== comp) throw new Error(`The zip ended inside ${name}.`);
    let data: Uint8Array;
    if (method === 0) data = slice;
    else if (method === 8) data = await inflate(slice);
    else throw new Error(`Cannot read compression method ${method} in ${name}.`);
    if (data.length !== raw && raw !== 0xffffffff) throw new Error(`${name} is the wrong size.`);
    out.push({ name, data });
  }
  return out;
}

const SAFE_REF = /^[A-Za-z0-9][A-Za-z0-9._-]{0,120}$/;

function floatsToBytes(data: Float32Array): Uint8Array {
  return new Uint8Array(data.buffer, data.byteOffset, data.byteLength).slice();
}

function bytesToFloats(data: Uint8Array, name: string): Float32Array {
  if (data.byteLength % 4 !== 0) throw new Error(`${name} is not a float buffer.`);
  const copy = data.slice();
  return new Float32Array(copy.buffer, copy.byteOffset, copy.byteLength / 4);
}

/** Blob refs named by the document: every take's audio, frames, anchors and note curves, and every Influence source. */
export function documentRefs(doc: Project): string[] {
  const refs = new Set<string>();
  for (const t of Object.values(doc.takes)) {
    if (t.audio) refs.add(t.audio);
    if (t.frames) refs.add(t.frames);
    if (t.anchors) refs.add(t.anchors);
    if (t.noteCurves) refs.add(t.noteCurves);
  }
  for (const l of doc.layers) {
    const src = l.influence?.source;
    if (src?.startsWith('grain-')) refs.add(src);
  }
  return [...refs].sort();
}

/** One zip of the document and the blobs `read` can find. A missing blob is skipped and named in `missing`. */
export async function exportProject(doc: Project, read: (ref: string) => Promise<Float32Array | null>): Promise<{ bytes: Uint8Array; missing: string[] }> {
  const missing: string[] = [];
  const files: ZipItem[] = [{ name: 'project.json', data: new TextEncoder().encode(JSON.stringify(doc)) }];
  for (const ref of documentRefs(doc)) {
    if (!SAFE_REF.test(ref)) throw new Error(`Refusing to export an unsafe blob name: ${ref}`);
    const data = await read(ref);
    if (!data) { missing.push(ref); continue; }
    files.push({ name: `blobs/${ref}.f32`, data: floatsToBytes(data) });
  }
  const bytes = zipStore(files);
  if (bytes.length > MAX_ARCHIVE_BYTES) throw new Error('The project file would be over 256 MB.');
  return { bytes, missing };
}

/** Validate, migrate and unpack a project file. The document keeps the id it was saved with. */
export async function importProject(bytes: Uint8Array): Promise<ArchivedProject> {
  const files = await unzip(bytes);
  const json = files.find((f) => f.name === 'project.json');
  if (!json) throw new Error('The zip has no project.json.');
  let parsed: unknown;
  try { parsed = JSON.parse(new TextDecoder().decode(json.data)); }
  catch { throw new Error('project.json is not valid JSON.'); }
  const savedSchema = typeof (parsed as { schema?: unknown })?.schema === 'number' ? (parsed as { schema: number }).schema : 1;
  const doc = migrate(parsed);
  if (!doc.id || typeof doc.id !== 'string' || !SAFE_REF.test(doc.id)) throw new Error('The project id is missing or not a plain name.');
  if (!Array.isArray(doc.sections) || !Array.isArray(doc.layers)) throw new Error('The project has no sections or layers.');
  const blobs = new Map<string, Float32Array>();
  for (const f of files) {
    if (f.name === 'project.json') continue;
    const m = /^blobs\/(.+)\.f32$/.exec(f.name);
    if (!m || !SAFE_REF.test(m[1])) throw new Error(`Unexpected file in the zip: ${f.name}`);
    blobs.set(m[1], bytesToFloats(f.data, f.name));
  }
  return { doc, blobs, savedSchema };
}
