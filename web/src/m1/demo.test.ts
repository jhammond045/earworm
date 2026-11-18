import { describe, expect, it } from 'vitest';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { encodeDemo } from './demo';
import { renderSong } from './render-core';
import { instantiateEngine } from './protocol';

const dir = resolve(import.meta.dirname, '../../public/m1/demo');
const wasm = resolve(import.meta.dirname, '../../public/wasm/klont.wasm');

describe('demo song', () => {
  const song = encodeDemo();
  const files: [string, ArrayBuffer][] = [['track.bin', song.track], ...song.layers.map((b, i): [string, ArrayBuffer] => [`layer${i}.bin`, b])];

  it('matches the committed blobs (WRITE_DEMO=1 rewrites them)', () => {
    if (process.env.WRITE_DEMO === '1') {
      mkdirSync(dir, { recursive: true });
      for (const [n, b] of files) writeFileSync(resolve(dir, n), new Uint8Array(b));
    }
    for (const [n, b] of files) {
      const p = resolve(dir, n);
      expect(existsSync(p), `${n} missing: run pnpm gen:demo`).toBe(true);
      expect(Buffer.from(b).equals(readFileSync(p)), n).toBe(true);
    }
  });

  // Needs a built klont.wasm. Peak stays under the master ceiling, every layer sounds, voices overlap.
  it.skipIf(!existsSync(wasm))('renders 20 s without clipping', async () => {
    const { ex } = instantiateEngine(readFileSync(wasm), 48000, 128);
    const r = await renderSong(ex, song, { maxSeconds: 20, once: false });
    expect(r.frames).toBe(20 * 48000);
    expect(r.peak).toBeGreaterThan(0.2);
    expect(r.peak).toBeLessThan(0.92);
    expect(r.left.every(Number.isFinite)).toBe(true);
  }, 60_000);
});
