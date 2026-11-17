import { describe, expect, it } from 'vitest';
import * as proto from './protocol';
import paramsRs from '../../../crates/dsp/src/params.rs?raw';
import protocolRs from '../../../crates/engine/src/protocol.rs?raw';


describe('params.rs vs PARAMS', () => {
  const src = paramsRs;
  const num = String.raw`(-?[0-9.]+(?:e-?\d+)?)`;
  const re = new RegExp(String.raw`\bp\(\s*"([^"]+)"\s*,\s*${num}\s*,\s*${num}\s*,\s*${num}`, 'g');
  const rust = [...src.matchAll(re)].map((m) => ({ name: m[1], min: +m[2], max: +m[3], default: +m[4] }));

  it('finds every parameter', () => {
    expect(rust.length).toBeGreaterThan(30);
    expect(rust.length).toBe(Number(/pub const COUNT: usize = (\d+)/.exec(src)![1]));
  });
  it('has the same names, order, min, max and default', () => {
    expect(proto.PARAMS.map((d) => d.name)).toEqual(rust.map((d) => d.name));
    for (const [i, d] of rust.entries()) {
      expect(proto.PARAMS[i].min, `${d.name} min`).toBeCloseTo(d.min, 9);
      expect(proto.PARAMS[i].max, `${d.name} max`).toBeCloseTo(d.max, 9);
      expect(proto.PARAMS[i].default, `${d.name} default`).toBeCloseTo(d.default, 9);
    }
  });
});

describe('engine protocol.rs vs protocol.ts', () => {
  const src = protocolRs;
  const consts = new Map<string, number>();
  for (const m of src.matchAll(/pub const (\w+): \w+ = (\d+);/g)) consts.set(m[1], Number(m[2]));

  it('MSG_*, M_*, sizes and counts match', () => {
    const names = [...consts.keys()].filter((n) => /^(MSG_|M_|METER_LEN|CONTROL_CAP|MAX_FRAMES_PER_CALL)/.test(n));
    expect(names.length).toBeGreaterThan(30);
    for (const n of names) expect((proto as Record<string, unknown>)[n], n).toBe(consts.get(n));
  });
  it('RECORD_STRIDE and RECORD_TUNED follow MAX_FRAME_LEN', () => {
    expect(proto.RECORD_STRIDE).toBe(proto.MAX_FRAME_LEN + 1);
    expect(proto.RECORD_TUNED).toBe(proto.MAX_FRAME_LEN);
  });
  it('BufferTable field order matches BT_*', () => {
    const body = /pub struct BufferTable \{([^}]*)\}/.exec(src)![1];
    const fields = [...body.matchAll(/pub (\w+): u32/g)].map((m) => m[1]);
    expect(fields.length).toBe(proto.BT_FIELDS);
    fields.forEach((f, i) => expect((proto as Record<string, unknown>)[`BT_${f.toUpperCase()}`], f).toBe(i));
  });
  it('message size comment: 16 bytes', () => {
    expect(proto.MSG_BYTES).toBe(16);
  });
});
