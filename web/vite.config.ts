import { defineConfig, type Plugin } from 'vite';
import { resolve } from 'node:path';

// SharedArrayBuffer needs a cross-origin isolated page (TDD "Cross-origin
// isolation and the fallback"). `pnpm dev:nocoi` drops the headers to
// exercise the MessagePort fallback.
const isolate = !process.env.EARWORM_NO_COI;
const headers = isolate
  ? { 'Cross-Origin-Opener-Policy': 'same-origin', 'Cross-Origin-Embedder-Policy': 'require-corp' }
  : {};

const page = (name: string) => resolve(import.meta.dirname, name);

/** GitHub Pages project site. Dev stays at `/` even if EARWORM_BASE is set. */
function siteBase(mode: string): string {
  if (mode !== 'production') return '/';
  const env = process.env.EARWORM_BASE;
  const raw = env != null && env !== '' ? env : '/earworm/';
  if (raw === '/') return '/';
  const lead = raw.startsWith('/') ? raw : `/${raw}`;
  return lead.endsWith('/') ? lead : `${lead}/`;
}

/** GitHub Pages cannot set COOP/COEP. The vendored coi-serviceworker reloads
 *  once and adds those headers. Dev already sends them, or omits them on
 *  purpose (`pnpm dev:nocoi`), so the shim is not injected into the dev server.
 *  require-corp matches the dev server. The stock script defaults to
 *  credentialless and can change headers after the document has loaded. */
function coiServiceWorker(): Plugin {
  const boot = [
    'window.coi={coepCredentialless:function(){return false;},coepDegrade:function(){return false;}};',
    'if(navigator.serviceWorker){',
    'navigator.serviceWorker.addEventListener("controllerchange",function(){',
    'if(window.crossOriginIsolated)return;',
    'if(sessionStorage.getItem("coiReloadedBySelf")==="controllerchange")return;',
    'sessionStorage.setItem("coiReloadedBySelf","controllerchange");',
    'location.reload();',
    '});',
    '}',
  ].join('');
  return {
    name: 'earworm-coi-serviceworker',
    transformIndexHtml: {
      order: 'pre',
      handler(_html, ctx) {
        if (ctx.server) return;
        return [
          { tag: 'script', children: boot, injectTo: 'head-prepend' },
          { tag: 'script', attrs: { src: '/coi-serviceworker.js' }, injectTo: 'head-prepend' },
        ];
      },
    },
  };
}

export default defineConfig(({ mode }) => ({
  base: siteBase(mode),
  root: import.meta.dirname,
  plugins: [coiServiceWorker()],
  server: { headers, port: 5173, strictPort: false },
  preview: { headers },
  build: {
    target: 'es2022',
    rollupOptions: {
      input: {
        index: page('index.html'),
        live: page('live.html'),
        liveTest: page('live-test.html'),
        liveSession: page('live-session.html'),
        influence: page('influence.html'),
        influenceTest: page('influence-test.html'),
        delay: page('delay.html'),
        m1: page('m1.html'),
        bench: page('bench.html'),
        m2: page('m2.html'),
        m3: page('m3.html'),
        m4Test: page('m4-test.html'),
      },
    },
  },
  worker: { format: 'es' },
}));
