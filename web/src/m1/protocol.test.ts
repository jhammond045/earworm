import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import * as proto from './protocol';

const rs = readFileSync(resolve(import.meta.dirname, '../../../crates/klont/src/protocol.rs'), 'utf8');

/** Evaluate `pub const NAME: type = expr;` lines in order. Expressions may use
 *  earlier constants, + * << and numeric literals with underscores. */
function parseConsts(src: string): Map<string, number> {
  const out = new Map<string, number>();
  for (const m of src.matchAll(/^pub const (\w+): (?:usize|u8|u16|u32|i32) = ([^;]+);/gm)) {
    const expr = m[2].replace(/0x[0-9A-Fa-f_]+|\d[\d_]*|[A-Z][A-Z0-9_]*/g, (t) => {
      if (/^0x/.test(t) || /^\d/.test(t)) return String(Number(t.replace(/_/g, '')));
      const v = out.get(t);
      if (v === undefined) throw new Error(`${m[1]}: unknown ${t}`);
      return String(v);
    });
    if (!/^[\d\s+\-*<>()]+$/.test(expr)) throw new Error(`${m[1]}: cannot evaluate ${expr}`);
    out.set(m[1], Function(`return (${expr});`)() as number);
  }
  return out;
}

describe('protocol.rs vs protocol.ts', () => {
  const consts = parseConsts(rs);
  const ts = proto as unknown as Record<string, unknown>;

  it('parses the constants', () => {
    expect(consts.size).toBeGreaterThan(80);
    expect(consts.get('METER_LEN')).toBe(280 + 2 * 1024);
  });
  it('every Rust constant exists in TS with the same value', () => {
    for (const [name, v] of consts) expect(ts[name], name).toBe(v);
  });
  it('BufferTable field order matches the struct', () => {
    const body = /pub struct BufferTable \{([^}]*)\}/.exec(rs)![1];
    const fields = [...body.matchAll(/pub (\w+): u32/g)].map((m) => m[1]);
    expect(fields.length).toBe(proto.BT_FIELDS);
    const camel = (s: string) => s.replace(/_(\w)/g, (_, c: string) => c.toUpperCase());
    const mem = new ArrayBuffer(proto.BT_FIELDS * 4);
    new Uint32Array(mem).forEach((_, i, a) => { a[i] = 100 + i; });
    const t = proto.readBufferTable(mem, 0) as unknown as Record<string, number>;
    fields.forEach((f, i) => expect(t[camel(f)], f).toBe(100 + i));
    expect(consts.get('BT_FIELDS')).toBe(proto.BT_FIELDS);
  });
  it('encodes a message in the documented layout', () => {
    const v = new DataView(new ArrayBuffer(16));
    proto.encodeMessage(v, 0, proto.MSG_NOTE_ON, 3, 2, 500, 4800, 77);
    expect([v.getUint16(0, true), v.getUint8(2), v.getUint8(3), v.getUint16(4, true), v.getFloat32(8, true), v.getUint32(12, true)])
      .toEqual([4, 3, 2, 500, 4800, 77]);
  });
});
