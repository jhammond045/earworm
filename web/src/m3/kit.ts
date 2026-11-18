// "Use my sound for a kit slot" (TDD "Sound Influence"): one hit, from the mic or a file, is matched in
// decaying-sound mode and becomes a slot of the selected beat layer's kit, with its pitch as the slot's default.

import { decodeBytes, MAX_SOURCE_S, trimAndCap } from '../influence/audio';
import { SLOT_NAMES } from '../m2/sounds';
import { KIT_SLOTS, type Layer, type Project } from '../project/types';
import { $, type App } from './app';
import { holdButton, MicClip } from './hold';
import { h, nextId } from './panels';

const MAX_CENTS = 12000;
/** A hit shorter than this (from the mic) is ignored. */
const MIN_HIT_S = 0.05;

export interface KitPanel { refresh(doc: Project): void }

export function initKit(app: App): KitPanel {
  const root = $('kitPanel');
  const layer = (doc = app.doc): Layer | undefined => doc.layers.find((l) => l.id === app.ui.layerId);

  const slotId = nextId('kit-slot');
  const slot = h('select', { id: slotId });
  SLOT_NAMES.forEach((n, i) => slot.append(new Option(`${i + 1}: ${n}`, String(i))));
  const fileId = nextId('kit-file');
  const file = h('input', { type: 'file', id: fileId, accept: 'audio/*' });
  const rec = h('button', { type: 'button', id: 'kitRecord', class: 'hold' }, 'Record one hit (hold)');
  const tryBtn = h('button', { type: 'button', id: 'kitTry', class: 'hold' }, 'Hold to hear the slot');
  const status = h('p', { class: 'small mono', id: 'kitStatus' });
  status.setAttribute('role', 'status');
  const heading = h('p', { class: 'small', id: 'kitFor' });
  const note = h('p', { class: 'small muted' },
    'Pick a slot, then hold the button and make one hit with your mouth, or choose a file with one hit in it. ' +
    'The hit is matched as a decaying sound and replaces that slot; its pitch becomes the slot\'s default pitch (an unpitched hit keeps the old one). The mic must already be on.');
  root.append(heading, note, h('div', { class: 'row' },
    h('label', { htmlFor: slotId }, 'Kit slot ', slot), h('label', { htmlFor: fileId }, 'Hit audio file ', file), rec, tryBtn), status);

  const say = (s: string) => { status.textContent = s; };
  let busy = false;

  async function useHit(signal: Float32Array, rate: number, name: string): Promise<void> {
    const l = layer();
    if (!l || l.kind !== 'beat' || busy) return;
    const k = Number(slot.value);
    busy = true;
    try {
      say(`Matching ${name}...`);
      const m = await (await app.matcher()).match(signal, rate, { decaying: true });
      const old = l.kit[k];
      const cents = m.pitched ? Math.max(0, Math.min(MAX_CENTS, Math.round(m.pitchCents))) : old?.cents ?? 4800;
      if (app.run(() => app.store.dispatch('setKitSlot', { layer: l.id, slot: k, params: Array.from(m.target), cents }))) {
        say(`Slot ${k + 1} (${SLOT_NAMES[k]}) set from ${name}: ${m.pitched ? `pitched, ${cents} cents above C0` : `unpitched, pitch kept at ${cents} cents`}, matched in ${m.ms.toFixed(0)} ms.`);
      }
    } catch (e) {
      say('');
      app.fail(`The match failed: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      busy = false;
    }
  }

  file.addEventListener('change', () => {
    const f = file.files?.[0];
    if (!f) return;
    void (async () => {
      try {
        const ctx: BaseAudioContext = app.ctx ?? new OfflineAudioContext(1, 1, 48000);
        await useHit(await decodeBytes(ctx, await f.arrayBuffer()), ctx.sampleRate, f.name);
      } catch (e) {
        app.fail(`That file could not be used: ${e instanceof Error ? e.message : String(e)}`);
      } finally {
        file.value = '';
      }
    })();
  });

  let clip: MicClip | null = null;
  holdButton(rec, () => {
    const { host, ctx } = app;
    if (!host || !ctx) { say('Press Start first.'); return; }
    if (!app.stream) { say('The mic is off. Press Enable mic first; nothing turns it on for you.'); return; }
    clip = new MicClip(host, ctx.sampleRate, MAX_SOURCE_S);
    say('Recording one hit...');
  }, () => {
    const c = clip;
    clip = null;
    if (!c) return;
    const sig = trimAndCap(c.read(), c.rate);
    if (sig.length < MIN_HIT_S * c.rate) { say('Too short. Hold while you make the hit.'); return; }
    void useHit(sig, c.rate, `mic hit ${new Date().toLocaleTimeString()}`);
  });

  let held: { layerIdx: number; cents: number } | null = null;
  holdButton(tryBtn, () => {
    const l = layer();
    const host = app.host;
    if (!l || !host) { say('Press Start first.'); return; }
    const i = app.doc.layers.findIndex((x) => x.id === l.id);
    const k = Number(slot.value);
    const cents = l.kit[k]?.cents ?? 4800;
    void host.ctx.resume();
    host.noteOn(i, cents, k);
    held = { layerIdx: i, cents };
  }, () => {
    if (held) app.host?.noteOff(held.layerIdx, held.cents);
    held = null;
  });

  return {
    refresh(doc) {
      const l = layer(doc);
      const beat = !!l && l.kind === 'beat';
      root.hidden = !beat;
      if (!l || !beat) return;
      heading.textContent = `Layer ${l.name}: ${KIT_SLOTS} kit slots.`;
    },
  };
}
