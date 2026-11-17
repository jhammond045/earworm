import { defineConfig } from 'vite';
import { resolve } from 'node:path';

// SharedArrayBuffer needs a cross-origin isolated page (TDD "Cross-origin
// isolation and the fallback"). `pnpm dev:nocoi` drops the headers to
// exercise the MessagePort fallback.
const isolate = !process.env.EARWORM_NO_COI;
const headers = isolate
  ? { 'Cross-Origin-Opener-Policy': 'same-origin', 'Cross-Origin-Embedder-Policy': 'require-corp' }
  : {};

const page = (name: string) => resolve(import.meta.dirname, name);

export default defineConfig({
  root: import.meta.dirname,
  server: { headers, port: 5173, strictPort: false },
  preview: { headers },
  build: {
    target: 'es2022',
    rollupOptions: {
      input: {
        index: page('index.html'),
        live: page('live.html'),
        liveTest: page('live-test.html'),
        influence: page('influence.html'),
        influenceTest: page('influence-test.html'),
        delay: page('delay.html'),
      },
    },
  },
  worker: { format: 'es' },
});
