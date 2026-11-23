// KlontHost.load through the browser path: the host's own checks, a MessagePort, and the real
// klont-processor.ts handler calling ew_load in the committed klont.wasm. The worklet globals are
// stand-ins (a MessageChannel for the node's port), so what runs is the code the app ships.
// Loading straight into the engine (render-core.ts, record/poly.test.ts) skips the host, which
// is how a kind check there once rejected every poly take's note curves.
import { readFileSync } from 'node:fs';
import { MessageChannel } from 'node:worker_threads';
import { afterAll, beforeAll, expect, it, vi } from 'vitest';
import * as pr from './protocol';
import { encodeNoteCurves, NOTE_REC } from './track';

vi.mock('./klont-processor.ts?worker&url', () => ({ default: 'klont-processor.js' }));

const SR = 48000;
const wasm = readFileSync(new URL('../../public/wasm/klont.wasm', import.meta.url));
const saved: Record<string, unknown> = {};
const fake: Record<string, unknown> = {};
let processorCtor: (new (o: unknown) => unknown) | null = null;

beforeAll(() => {
  class AudioWorkletProcessor {
    readonly port: unknown;
    constructor() { this.port = nextPort; }
  }
  let nextPort: unknown = null;
  class AudioWorkletNode {
    readonly port;
    onprocessorerror: (() => void) | null = null;
    constructor(_ctx: unknown, _name: string, opts: { processorOptions: unknown }) {
      const ch = new MessageChannel();
      this.port = ch.port1;
      nextPort = ch.port2;
      new processorCtor!({ processorOptions: opts.processorOptions });
    }
    connect(): void {}
    disconnect(): void {}
  }
  Object.assign(fake, {
    sampleRate: SR,
    AudioWorkletProcessor,
    AudioWorkletNode,
    registerProcessor: (_n: string, c: new (o: unknown) => unknown) => { processorCtor = c; },
    fetch: async () => new Response(wasm),
  });
  for (const k of Object.keys(fake)) {
    saved[k] = (globalThis as Record<string, unknown>)[k];
    (globalThis as Record<string, unknown>)[k] = fake[k];
  }
});

afterAll(() => {
  for (const k of Object.keys(saved)) (globalThis as Record<string, unknown>)[k] = saved[k];
});

/** Two notes, three records each: the shape a poly take stores (m2/analysis.ts decodePolyPass). */
function noteCurves(): ArrayBuffer {
  const nNotes = 2, nRecs = 6;
  const c = new Float32Array(2 + nNotes * 3 + nRecs * NOTE_REC);
  c.set([nNotes, nRecs, 0, 0, 3, 0.5, 3, 3]);
  for (let r = 0; r < nRecs; r++) c.fill(0.25, 2 + nNotes * 3 + r * NOTE_REC, 2 + nNotes * 3 + (r + 1) * NOTE_REC);
  return encodeNoteCurves(c);
}

it('loads a poly take\'s note curves through the host and the worklet', async () => {
  await import('./klont-processor');
  const { KlontHost } = await import('./klont-host');
  const ctx = { sampleRate: SR, destination: {}, audioWorklet: { addModule: async () => {} } } as unknown as AudioContext;
  const host = await KlontHost.create(ctx, { forceFallback: true });
  try {
    expect(await host.load(pr.LOAD_NOTE_CURVES, 0, noteCurves())).toBe(pr.LOAD_OK);
    expect(await host.load(pr.LOAD_NOTE_CURVES + 1, 0, noteCurves())).toBe(pr.LOAD_BAD_KIND);
  } finally {
    host.disconnect();
  }
});
