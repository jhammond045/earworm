// Generates crates/params/src/table.rs, web/src/gen/params.ts,
// web/src/gen/params.schema.json and web/src/gen/params-dictionary.md from
// crates/params/params.toml. Run with `pnpm gen:params`.
// Plain Node, erasable TypeScript only (Node strips the types).

import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';

const GROUPS = ['tone', 'shape', 'motion', 'grit', 'space'];
const SCALES = ['lin', 'log', 'db', 'bipolar'];
const N_BANDS = 16;
const N_SCALARS = 64;
const COUNT = 96;

type Value = string | number | string[] | Record<string, number>;
interface Entry {
  kind: string;
  line: number;
  fields: Record<string, Value>;
}
interface Param {
  id: string;
  group: string;
  scale: string;
  min: number;
  max: number;
  neutral: number;
  unit: string;
  smoothMs: number;
  doc: string;
  macros: Record<string, number>;
}
export interface Generated {
  rust: string;
  ts: string;
  schema: string;
  dictionary: string;
}

function fail(line: number, msg: string): never {
  throw new Error(`params.toml:${line}: ${msg}`);
}

// Removes a trailing comment, ignoring # inside strings.
function stripComment(s: string, line: number): string {
  let inStr = false;
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (inStr) {
      if (c === '\\') i++;
      else if (c === '"') inStr = false;
    } else if (c === '"') inStr = true;
    else if (c === '#') return s.slice(0, i);
  }
  if (inStr) fail(line, 'unterminated string');
  return s;
}

function parseString(s: string, line: number): [string, string] {
  // s starts with a quote; returns [value, rest].
  let out = '';
  let i = 1;
  for (; i < s.length; i++) {
    const c = s[i];
    if (c === '\\') {
      const n = s[++i];
      if (n === '"' || n === '\\') out += n;
      else fail(line, `unsupported escape \\${n}`);
    } else if (c === '"') return [out, s.slice(i + 1)];
    else out += c;
  }
  return fail(line, 'unterminated string');
}

function parseNumber(s: string, line: number): [number, string] {
  const m = /^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?/.exec(s);
  if (!m) return fail(line, `expected a number near "${s.slice(0, 20)}"`);
  return [Number(m[0]), s.slice(m[0].length)];
}

function parseValue(s: string, line: number): [Value, string] {
  s = s.trimStart();
  if (s.startsWith('"')) return parseString(s, line);
  if (s.startsWith('[')) {
    const arr: string[] = [];
    let rest = s.slice(1).trimStart();
    while (!rest.startsWith(']')) {
      if (!rest.startsWith('"')) fail(line, 'only string arrays are supported');
      const [v, r] = parseString(rest, line);
      arr.push(v);
      rest = r.trimStart();
      if (rest.startsWith(',')) rest = rest.slice(1).trimStart();
      else if (!rest.startsWith(']')) fail(line, 'expected , or ] in array');
    }
    return [arr, rest.slice(1)];
  }
  if (s.startsWith('{')) {
    const tbl: Record<string, number> = {};
    let rest = s.slice(1).trimStart();
    while (!rest.startsWith('}')) {
      const k = /^[A-Za-z_][A-Za-z0-9_]*/.exec(rest);
      if (!k) fail(line, 'expected a key in inline table');
      rest = rest.slice(k[0].length).trimStart();
      if (!rest.startsWith('=')) fail(line, 'expected = in inline table');
      const [n, r] = parseNumber(rest.slice(1).trimStart(), line);
      if (k[0] in tbl) fail(line, `duplicate key ${k[0]} in inline table`);
      tbl[k[0]] = n;
      rest = r.trimStart();
      if (rest.startsWith(',')) rest = rest.slice(1).trimStart();
      else if (!rest.startsWith('}')) fail(line, 'expected , or } in inline table');
    }
    return [tbl, rest.slice(1)];
  }
  return parseNumber(s, line);
}

function parseToml(text: string): Entry[] {
  const entries: Entry[] = [];
  let cur: Entry | null = null;
  const lines = text.split('\n');
  for (let ln = 1; ln <= lines.length; ln++) {
    const s = stripComment(lines[ln - 1], ln).trim();
    if (s === '') continue;
    if (s.startsWith('[')) {
      const m = /^(\[\[(param|bands|desc|module)\]\]|\[(macros|vocab)\])$/.exec(s);
      if (!m) fail(ln, `unsupported table header ${s}`);
      cur = { kind: m[2] ?? m[3], line: ln, fields: {} };
      entries.push(cur);
      continue;
    }
    const m = /^([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$/.exec(s);
    if (!m) fail(ln, `cannot parse "${s}"`);
    if (!cur) fail(ln, 'key before any table');
    const [v, rest] = parseValue(m[2], ln);
    if (rest.trim() !== '') fail(ln, `trailing text "${rest.trim()}"`);
    if (m[1] in cur.fields) fail(ln, `duplicate key ${m[1]}`);
    cur.fields[m[1]] = v;
  }
  return entries;
}

function need<T extends Value>(e: Entry, key: string, type: string): T {
  const v = e.fields[key];
  if (v === undefined) fail(e.line, `missing ${key}`);
  const ok = type === 'string' ? typeof v === 'string' : type === 'number' ? typeof v === 'number' : false;
  if (!ok) fail(e.line, `${key} must be a ${type}`);
  return v as T;
}

function toParam(e: Entry, macroNames: string[], allowed: string[]): Param {
  for (const k of Object.keys(e.fields)) if (!allowed.includes(k)) fail(e.line, `unknown key ${k}`);
  const p: Param = {
    id: need<string>(e, 'id', 'string'),
    group: need<string>(e, 'group', 'string'),
    scale: need<string>(e, 'scale', 'string'),
    min: need<number>(e, 'min', 'number'),
    max: need<number>(e, 'max', 'number'),
    neutral: need<number>(e, 'neutral', 'number'),
    unit: need<string>(e, 'unit', 'string'),
    smoothMs: need<number>(e, 'smooth_ms', 'number'),
    doc: need<string>(e, 'doc', 'string'),
    macros: {},
  };
  if (!/^[a-z][a-z0-9_]*(\.[a-z0-9_]+)*$/.test(p.id)) fail(e.line, `bad id ${p.id}`);
  if (!GROUPS.includes(p.group)) fail(e.line, `bad group ${p.group}`);
  if (!SCALES.includes(p.scale)) fail(e.line, `bad scale ${p.scale}`);
  if (!(p.min < p.max)) fail(e.line, `${p.id}: min must be below max`);
  if (!(p.min <= p.neutral && p.neutral <= p.max)) fail(e.line, `${p.id}: neutral outside min..max`);
  if (p.scale === 'log' && !(p.min > 0)) fail(e.line, `${p.id}: log scale needs min > 0`);
  if (p.smoothMs < 0) fail(e.line, `${p.id}: smooth_ms must not be negative`);
  if (!/^[\x20-\x7e]*$/.test(p.doc + p.unit)) fail(e.line, `${p.id}: doc and unit must be printable ASCII`);
  const mac = e.fields.macros;
  if (mac !== undefined) {
    if (typeof mac !== 'object' || Array.isArray(mac)) fail(e.line, 'macros must be an inline table');
    for (const [k, w] of Object.entries(mac)) {
      if (!macroNames.includes(k)) fail(e.line, `${p.id}: unknown macro ${k}`);
      p.macros[k] = w;
    }
  }
  return p;
}

// ---- number and string formatting ----

function num(x: number): string {
  const s = String(x);
  return /^-?\d+$/.test(s) ? `${s}.0` : s;
}
function rstr(s: string): string {
  return `"${s.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`;
}
function rangeText(p: Param): string {
  return `${num(p.min)} to ${num(p.max)}${p.unit ? ' ' + p.unit : ''}`;
}

// ---- generator ----

export function generate(tomlText: string): Generated {
  const entries = parseToml(tomlText);
  const macroEntries = entries.filter((e) => e.kind === 'macros');
  if (macroEntries.length !== 1) throw new Error('params.toml: need exactly one [macros] table');
  const names = macroEntries[0].fields.names;
  if (!Array.isArray(names) || names.length === 0) fail(macroEntries[0].line, 'macros.names must be a string array');
  if (new Set(names).size !== names.length) fail(macroEntries[0].line, 'duplicate macro name');
  const macroNames = names;

  const scalarKeys = ['id', 'group', 'scale', 'min', 'max', 'neutral', 'unit', 'smooth_ms', 'doc', 'macros'];
  const bandKeys = ['id', 'group', 'scale', 'min', 'max', 'neutral', 'unit', 'smooth_ms', 'doc', 'count'];
  const scalars = entries.filter((e) => e.kind === 'param').map((e) => toParam(e, macroNames, scalarKeys));
  const bandEntries = entries.filter((e) => e.kind === 'bands');
  const bands = bandEntries.map((e) => {
    const p = toParam(e, macroNames, bandKeys);
    const count = need<number>(e, 'count', 'number');
    if (count !== N_BANDS) fail(e.line, `bands count must be ${N_BANDS}`);
    return p;
  });

  if (scalars.length !== N_SCALARS) throw new Error(`params.toml: expected ${N_SCALARS} scalars, found ${scalars.length}`);
  if (bands.length !== 2 || bands[0].id !== 'tone_bands' || bands[1].id !== 'noise_bands')
    throw new Error('params.toml: expected [[bands]] tone_bands then noise_bands');

  const all: Param[] = [...scalars];
  for (const b of bands) for (let j = 0; j < N_BANDS; j++) all.push({ ...b, id: `${b.id}.${j}`, macros: {} });
  if (all.length !== COUNT) throw new Error(`params.toml: expected ${COUNT} parameters, found ${all.length}`);
  const seen = new Set<string>();
  for (const p of all) {
    if (seen.has(p.id)) throw new Error(`params.toml: duplicate id ${p.id}`);
    seen.add(p.id);
  }

  const W: number[][] = macroNames.map(() => new Array(COUNT).fill(0));
  all.forEach((p, i) => {
    for (const [k, w] of Object.entries(p.macros)) W[macroNames.indexOf(k)][i] = w;
  });

  return {
    rust: genRust(scalars, all, macroNames, W),
    ts: genTs(scalars, all, macroNames, W),
    schema: genSchema(scalars, bands),
    dictionary: genDictionary(scalars, bands, macroNames, W),
  };
}

const constName = (id: string) => id.toUpperCase().replace(/\./g, '_');
const camel = (g: string) => g[0].toUpperCase() + g.slice(1);

function genRust(scalars: Param[], all: Param[], macroNames: string[], W: number[][]): string {
  const o: string[] = [];
  o.push('// Generated by tools/paramgen/paramgen.ts from crates/params/params.toml.');
  o.push('// Do not edit by hand; run `pnpm gen:params`.');
  o.push('');
  o.push(`pub const COUNT: usize = ${COUNT};`);
  o.push(`pub const SCALARS: usize = ${N_SCALARS};`);
  o.push(`pub const N_BANDS: usize = ${N_BANDS};`);
  o.push(`pub const TONE_BANDS: usize = ${N_SCALARS};`);
  o.push(`pub const NOISE_BANDS: usize = ${N_SCALARS + N_BANDS};`);
  o.push(`pub const N_MACROS: usize = ${macroNames.length};`);
  o.push('');
  scalars.forEach((p, i) => o.push(`pub const ${constName(p.id)}: usize = ${i};`));
  o.push('');
  o.push('pub static DESCS: [crate::Desc; COUNT] = [');
  for (const p of all) {
    o.push(
      `    crate::Desc { id: ${rstr(p.id)}, group: crate::Group::${camel(p.group)}, scale: crate::Scale::${camel(p.scale)}, ` +
        `min: ${num(p.min)}, max: ${num(p.max)}, neutral: ${num(p.neutral)}, unit: ${rstr(p.unit)}, ` +
        `smooth_ms: ${num(p.smoothMs)}, doc: ${rstr(p.doc)} },`,
    );
  }
  o.push('];');
  o.push('');
  o.push('pub static MACRO_NAMES: [&str; N_MACROS] = [' + macroNames.map(rstr).join(', ') + '];');
  o.push('');
  o.push('pub static MACRO_W: [[f32; COUNT]; N_MACROS] = [');
  W.forEach((row, k) => {
    o.push(`    // ${macroNames[k]}`);
    o.push('    [');
    for (let i = 0; i < row.length; i += 12) o.push('        ' + row.slice(i, i + 12).map(num).join(', ') + ',');
    o.push('    ],');
  });
  o.push('];');
  o.push('');
  return o.join('\n');
}

function genTs(scalars: Param[], all: Param[], macroNames: string[], W: number[][]): string {
  const o: string[] = [];
  o.push('// Generated by tools/paramgen/paramgen.ts from crates/params/params.toml.');
  o.push('// Do not edit by hand; run `pnpm gen:params`.');
  o.push('');
  o.push(`export const COUNT = ${COUNT};`);
  o.push(`export const SCALARS = ${N_SCALARS};`);
  o.push(`export const N_BANDS = ${N_BANDS};`);
  o.push(`export const TONE_BANDS = ${N_SCALARS};`);
  o.push(`export const NOISE_BANDS = ${N_SCALARS + N_BANDS};`);
  o.push(`export const N_MACROS = ${macroNames.length};`);
  o.push('');
  o.push("export type Group = " + GROUPS.map((g) => `'${g}'`).join(' | ') + ';');
  o.push("export type Scale = " + SCALES.map((g) => `'${g}'`).join(' | ') + ';');
  o.push('');
  o.push('export interface Desc {');
  o.push('  id: string;');
  o.push('  group: Group;');
  o.push('  scale: Scale;');
  o.push('  min: number;');
  o.push('  max: number;');
  o.push('  /** Real units. */');
  o.push('  neutral: number;');
  o.push('  unit: string;');
  o.push('  smoothMs: number;');
  o.push('  doc: string;');
  o.push('}');
  o.push('');
  o.push('/** Index of each scalar parameter in a ParamSet. */');
  o.push('export const P = {');
  scalars.forEach((p, i) => o.push(`  ${JSON.stringify(p.id)}: ${i},`));
  o.push('} as const;');
  o.push('');
  o.push('export type ParamId = keyof typeof P;');
  o.push('');
  o.push('export const DESCS: readonly Desc[] = [');
  for (const p of all) {
    o.push(
      `  { id: ${JSON.stringify(p.id)}, group: '${p.group}', scale: '${p.scale}', min: ${num(p.min)}, max: ${num(p.max)}, ` +
        `neutral: ${num(p.neutral)}, unit: ${JSON.stringify(p.unit)}, smoothMs: ${num(p.smoothMs)}, doc: ${JSON.stringify(p.doc)} },`,
    );
  }
  o.push('];');
  o.push('');
  o.push('export const MACRO_NAMES: readonly string[] = ' + JSON.stringify(macroNames) + ';');
  o.push('');
  o.push('/** Macro weights in normalized units, MACRO_W[macro][param]. */');
  o.push('export const MACRO_W: number[][] = [');
  W.forEach((row, k) => {
    o.push(`  // ${macroNames[k]}`);
    o.push('  [');
    for (let i = 0; i < row.length; i += 12) o.push('    ' + row.slice(i, i + 12).map(num).join(', ') + ',');
    o.push('  ],');
  });
  o.push('];');
  o.push('');
  o.push(`const clamp01 = (x: number): number => (x < 0 ? 0 : x > 1 ? 1 : x);`);
  o.push('');
  o.push('/** Normalized 0..1 to real units. Clamps n; NaN counts as the neutral value. */');
  o.push('export function toReal(i: number, n: number): number {');
  o.push('  const d = DESCS[i];');
  o.push('  if (Number.isNaN(n)) return d.neutral;');
  o.push('  n = clamp01(n);');
  o.push("  const r = d.scale === 'log' ? d.min * Math.pow(d.max / d.min, n) : d.min + (d.max - d.min) * n;");
  o.push('  return Math.min(d.max, Math.max(d.min, r));');
  o.push('}');
  o.push('');
  o.push('/** Real units to normalized, clamped to 0..1. NaN counts as the neutral value. */');
  o.push('export function toNorm(i: number, real: number): number {');
  o.push('  const d = DESCS[i];');
  o.push('  if (Number.isNaN(real)) real = d.neutral;');
  o.push("  if (d.scale === 'log') {");
  o.push('    if (!(real > 0)) return 0;');
  o.push('    return clamp01(Math.log(real / d.min) / Math.log(d.max / d.min));');
  o.push('  }');
  o.push('  return clamp01((real - d.min) / (d.max - d.min));');
  o.push('}');
  o.push('');
  o.push('/** Every parameter at its neutral value, normalized. */');
  o.push('export function neutralSet(): Float32Array {');
  o.push('  const s = new Float32Array(COUNT);');
  o.push('  for (let i = 0; i < COUNT; i++) s[i] = toNorm(i, DESCS[i].neutral);');
  o.push('  return s;');
  o.push('}');
  o.push('');
  return o.join('\n');
}

function genSchema(scalars: Param[], bands: Param[]): string {
  const props: Record<string, unknown> = {};
  for (const p of scalars) {
    props[p.id] = {
      type: 'number',
      description: `${p.doc} Normalized 0 to 1; real range ${rangeText(p)}, neutral ${num(p.neutral)}.`,
    };
  }
  for (const b of bands) {
    props[b.id] = {
      type: 'array',
      description: `${b.doc} ${N_BANDS} numbers from 0 to 1 (low to high frequency, 80 Hz to 12 kHz); real range ${rangeText(b)}, neutral ${num(b.neutral)}.`,
      items: { type: 'number' },
    };
  }
  const schema = {
    type: 'object',
    description: 'A sound: every parameter as a number from 0 to 1.',
    properties: props,
    required: Object.keys(props),
    additionalProperties: false,
  };
  return JSON.stringify(schema, null, 2) + '\n';
}

function genDictionary(scalars: Param[], bands: Param[], macroNames: string[], W: number[][]): string {
  const o: string[] = [];
  o.push('<!-- Generated by tools/paramgen/paramgen.ts from crates/params/params.toml. Do not edit by hand. -->');
  o.push('# Parameter dictionary');
  o.push('');
  o.push('Every value you return is a number from 0 to 1. Real ranges below show what 0 and 1 mean.');
  o.push('');
  o.push('## Scalars');
  o.push('');
  o.push('Format: id | group | real range (0 to 1) | neutral | meaning. The scale says how 0..1 maps: lin and db and bipolar are straight lines, log is exponential.');
  o.push('');
  for (const p of scalars) {
    o.push(`- ${p.id} | ${p.group} | ${rangeText(p)} (${p.scale}) | neutral ${num(p.neutral)} | ${p.doc}`);
  }
  o.push('');
  o.push('## Band shapes');
  o.push('');
  for (const b of bands) {
    o.push(
      `- ${b.id} | ${b.group} | ${N_BANDS} numbers, low to high frequency (80 Hz to 12 kHz, log spaced) | ${rangeText(b)} (${b.scale}) | neutral ${num(b.neutral)} | ${b.doc}`,
    );
  }
  o.push('');
  o.push('## Macros');
  o.push('');
  o.push('Eight macro sliders each push several parameters up or down. The listed weight is in normalized units at the macro extremes.');
  o.push('');
  const ids = scalars.map((p) => p.id);
  macroNames.forEach((m, k) => {
    const parts: string[] = [];
    ids.forEach((id, i) => {
      if (W[k][i] !== 0) parts.push(`${id} ${W[k][i] > 0 ? '+' : ''}${num(W[k][i])}`);
    });
    o.push(`- ${m}: ${parts.length ? parts.join(', ') : 'none'}`);
  });
  o.push('');
  return o.join('\n');
}

// ---- script entry ----

// ---- module table (crates/params/modules.toml)

const KINDS = ['voice', 'source', 'layer', 'channel'];

interface ModParam { name: string; desc: number; count: number }
interface ModType { type: string; kind: string; code: number; params: ModParam[]; nValues: number }

function mfail(line: number, msg: string): never {
  throw new Error(`modules.toml:${line}: ${msg}`);
}

/** The vocabulary descriptors (params.toml) plus modules.toml's, and the module types. */
export function generateModules(paramsToml: string, modulesToml: string): { rust: string; ts: string } {
  const vocab = parseToml(paramsToml);
  const names = (vocab.find((e) => e.kind === 'macros')!.fields.names as string[]);
  const scalarKeys = ['id', 'group', 'scale', 'min', 'max', 'neutral', 'unit', 'smooth_ms', 'doc', 'macros'];
  const bandKeys = [...scalarKeys.filter((k) => k !== 'macros'), 'count'];
  const all: Param[] = vocab.filter((e) => e.kind === 'param').map((e) => toParam(e, names, scalarKeys));
  for (const e of vocab.filter((x) => x.kind === 'bands')) {
    const b = toParam(e, names, bandKeys);
    for (let j = 0; j < N_BANDS; j++) all.push({ ...b, id: `${b.id}.${j}`, macros: {} });
  }
  const entries = parseToml(modulesToml);
  const steps: number[] = all.map(() => 0);
  const scalable: boolean[] = all.map(() => false);
  const v = entries.find((e) => e.kind === 'vocab');
  for (const id of (v?.fields.scalable as string[] | undefined) ?? []) {
    const i = all.findIndex((p) => p.id === id);
    if (i < 0) mfail(v!.line, `unknown vocabulary id ${id}`);
    scalable[i] = true;
  }
  const descKeys = [...scalarKeys.filter((k) => k !== 'macros'), 'steps', 'scalable'];
  for (const e of entries.filter((x) => x.kind === 'desc')) {
    const p = toParam(e, names, descKeys);
    if (all.some((q) => q.id === p.id)) mfail(e.line, `duplicate descriptor ${p.id}`);
    const st = e.fields.steps;
    if (st !== undefined && !(typeof st === 'number' && Number.isInteger(st) && st >= 2)) mfail(e.line, 'steps must be an integer of at least 2');
    all.push(p);
    steps.push((st as number | undefined) ?? 0);
    scalable.push(e.fields.scalable === 1);
  }
  const index = new Map(all.map((p, i) => [p.id, i]));
  const types: ModType[] = [];
  for (const e of entries.filter((x) => x.kind === 'module')) {
    for (const k of Object.keys(e.fields)) if (!['type', 'kind', 'code', 'params'].includes(k)) mfail(e.line, `unknown key ${k}`);
    const type = e.fields.type as string, kind = e.fields.kind as string, code = e.fields.code as number;
    if (typeof type !== 'string' || !/^[a-z_]+$/.test(type)) mfail(e.line, 'bad type');
    if (!KINDS.includes(kind)) mfail(e.line, `bad kind ${kind}`);
    if (!Number.isInteger(code) || code <= 0 || code > 255 || types.some((t) => t.code === code)) mfail(e.line, `bad or duplicate code ${code}`);
    if (types.some((t) => t.type === type)) mfail(e.line, `duplicate type ${type}`);
    const params: ModParam[] = [];
    for (const s of (e.fields.params as string[]) ?? []) {
      const m = /^([a-z_0-9]+):([a-z_0-9.]+?)(?:\*(\d+))?$/.exec(s);
      if (!m) mfail(e.line, `bad parameter "${s}"`);
      const d = index.get(m[2]);
      if (d === undefined) mfail(e.line, `unknown descriptor ${m[2]}`);
      if (params.some((p) => p.name === m[1])) mfail(e.line, `duplicate parameter ${m[1]}`);
      params.push({ name: m[1], desc: d, count: m[3] ? Number(m[3]) : 1 });
    }
    types.push({ type, kind, code, params, nValues: params.reduce((n, p) => n + p.count, 0) });
  }
  return { rust: genModulesRust(all, steps, scalable, types), ts: genModulesTs(all, steps, scalable, types) };
}

function genModulesRust(all: Param[], steps: number[], scalable: boolean[], types: ModType[]): string {
  const o: string[] = [];
  o.push('// Generated by tools/paramgen/paramgen.ts from crates/params/modules.toml.');
  o.push('// Do not edit by hand; run `pnpm gen:params`.');
  o.push('');
  o.push(`/// Descriptors: the ${COUNT} of the Klont vocabulary (DESCS), then the module table's own.`);
  o.push(`pub const N_DESCS: usize = ${all.length};`);
  all.slice(COUNT).forEach((p, i) => o.push(`pub const D_${constName(p.id)}: usize = ${COUNT + i};`));
  o.push('');
  o.push(`pub static EXTRA_DESCS: [crate::Desc; ${all.length - COUNT}] = [`);
  for (const p of all.slice(COUNT)) {
    o.push(
      `    crate::Desc { id: ${rstr(p.id)}, group: crate::Group::${camel(p.group)}, scale: crate::Scale::${camel(p.scale)}, ` +
        `min: ${num(p.min)}, max: ${num(p.max)}, neutral: ${num(p.neutral)}, unit: ${rstr(p.unit)}, ` +
        `smooth_ms: ${num(p.smoothMs)}, doc: ${rstr(p.doc)} },`,
    );
  }
  o.push('];');
  o.push('');
  o.push('/// Steps of a stepped value (0: continuous).');
  o.push(`pub static STEPS: [u8; N_DESCS] = [${steps.join(', ')}];`);
  o.push('/// Values a `scale` route may multiply.');
  o.push(`pub static SCALABLE: [bool; N_DESCS] = [${scalable.join(', ')}];`);
  o.push('');
  for (const t of types) o.push(`pub const T_${constName(t.type)}: u32 = ${t.code};`);
  o.push('');
  o.push(`pub static MODULE_TYPES: [crate::ModuleType; ${types.length}] = [`);
  for (const t of types) {
    const ps = t.params.map((p) => `crate::ModParam { name: ${rstr(p.name)}, desc: ${p.desc}, count: ${p.count} }`).join(', ');
    o.push(`    crate::ModuleType { name: ${rstr(t.type)}, code: ${t.code}, kind: crate::ModKind::${camel(t.kind)}, n_values: ${t.nValues}, params: &[${ps}] },`);
  }
  o.push('];');
  o.push('');
  o.push('/// Value offsets of each parameter inside its module.');
  for (const t of types) {
    o.push(`pub mod ${t.type} {`);
    let at = 0;
    for (const p of t.params) {
      o.push(`    pub const ${p.name.toUpperCase()}: usize = ${at};`);
      at += p.count;
    }
    o.push(`    pub const N: usize = ${at};`);
    o.push('}');
  }
  o.push('');
  return o.join('\n');
}

function genModulesTs(all: Param[], steps: number[], scalable: boolean[], types: ModType[]): string {
  const o: string[] = [];
  o.push('// Generated by tools/paramgen/paramgen.ts from crates/params/modules.toml.');
  o.push('// Do not edit by hand; run `pnpm gen:params`.');
  o.push('');
  o.push("import { COUNT, DESCS, type Desc } from './params';");
  o.push('');
  o.push('/** Descriptors beyond the Klont vocabulary; value descriptor ids continue after COUNT. */');
  o.push('export const EXTRA_DESCS: readonly Desc[] = [');
  for (const p of all.slice(COUNT)) {
    o.push(
      `  { id: ${JSON.stringify(p.id)}, group: '${p.group}', scale: '${p.scale}', min: ${num(p.min)}, max: ${num(p.max)}, ` +
        `neutral: ${num(p.neutral)}, unit: ${JSON.stringify(p.unit)}, smoothMs: ${num(p.smoothMs)}, doc: ${JSON.stringify(p.doc)} },`,
    );
  }
  o.push('];');
  o.push('');
  o.push('/** Every descriptor, by value descriptor id. */');
  o.push('export const ALL_DESCS: readonly Desc[] = [...DESCS, ...EXTRA_DESCS];');
  o.push(`export const N_DESCS = ${all.length};`);
  o.push('');
  o.push('/** Descriptor id by descriptor name. */');
  o.push('export const DESC_ID: Readonly<Record<string, number>> = Object.fromEntries(ALL_DESCS.map((d, i) => [d.id, i]));');
  o.push('');
  o.push(`export const STEPS: readonly number[] = [${steps.join(', ')}];`);
  o.push(`export const SCALABLE: readonly boolean[] = [${scalable.join(', ')}];`);
  o.push('');
  o.push("export type ModKind = 'voice' | 'source' | 'layer' | 'channel';");
  o.push('export interface ModParam { name: string; desc: number; count: number }');
  o.push('export interface ModuleType { type: string; code: number; kind: ModKind; nValues: number; params: readonly ModParam[] }');
  o.push('');
  o.push('export const MODULE_TYPES: Readonly<Record<string, ModuleType>> = {');
  for (const t of types) {
    const ps = t.params.map((p) => `{ name: '${p.name}', desc: ${p.desc}, count: ${p.count} }`).join(', ');
    o.push(`  ${t.type}: { type: '${t.type}', code: ${t.code}, kind: '${t.kind}', nValues: ${t.nValues}, params: [${ps}] },`);
  }
  o.push('};');
  o.push('');
  o.push('if (DESCS.length !== COUNT) throw new Error(\'modules.ts: vocabulary size changed; run pnpm gen:params\');');
  o.push('');
  return o.join('\n');
}

function main(): void {
  const root = resolve(import.meta.dirname, '../..');
  const toml = readFileSync(resolve(root, 'crates/params/params.toml'), 'utf8');
  const g = generate(toml);
  const m = generateModules(toml, readFileSync(resolve(root, 'crates/params/modules.toml'), 'utf8'));
  writeFileSync(resolve(root, 'crates/params/src/modules.rs'), m.rust);
  writeFileSync(resolve(root, 'web/src/gen/modules.ts'), m.ts);
  writeFileSync(resolve(root, 'crates/params/src/table.rs'), g.rust);
  writeFileSync(resolve(root, 'web/src/gen/params.ts'), g.ts);
  writeFileSync(resolve(root, 'web/src/gen/params.schema.json'), g.schema);
  writeFileSync(resolve(root, 'web/src/gen/params-dictionary.md'), g.dictionary);
  console.log(`paramgen: wrote ${COUNT} parameters, ${macroNames(g)} macros`);
}
function macroNames(g: Generated): number {
  return Number(/N_MACROS: usize = (\d+)/.exec(g.rust)?.[1] ?? 0);
}

if (process.argv[1] && resolve(process.argv[1]) === import.meta.filename) main();
