// An imported file goes to the pass its layer asks for: a layer set to Poly gets the poly pass,
// as a Poly capture does (docs/poly.md P3), so a chord file comes back as the chords' notes with
// their curves rather than one mono line.
import { readFileSync } from 'node:fs';
import { expect, it } from 'vitest';
import { AnalysisCore } from '../m2/analysis';
import { guitar } from '../record/synth-guitar';
import { newProject } from './commands';
import { importPass, liveTierOver } from './import-take';
import { Store } from './store';

const SR = 48000;
const klont = readFileSync(new URL('../../public/wasm/klont.wasm', import.meta.url));
const analysis = AnalysisCore.fromBytes(readFileSync(new URL('../../public/wasm/analysis.wasm', import.meta.url)));

it('reads a chord file on a Poly layer as poly notes, and on a mono layer with the second pass', async () => {
  const { audio: x, played } = guitar(10 * SR, SR);
  const store = new Store(newProject('import'));
  const mel = store.doc.layers[0];
  const { audio, frames } = await liveTierOver(klont, x, SR);

  const mono = await importPass(analysis, mel, audio, frames.slice(), SR);
  expect('second' in mono).toBe(true);

  store.dispatch('setLayer', { id: mel.id, capture: { mode: 'poly', instrument: 'guitar', channels: 6 } });
  const pass = await importPass(analysis, store.doc.layers[0], audio, frames.slice(), SR);
  if (!('poly' in pass)) throw new Error('expected the poly pass');
  expect(pass.opts.instrument).toBe('guitar');
  // Five strums of four to six strings each; the note model finds most of them.
  expect(pass.poly.notes.length).toBeGreaterThan(played.length / 2);
  expect(pass.poly.curves[0]).toBe(pass.poly.notes.length);
});
