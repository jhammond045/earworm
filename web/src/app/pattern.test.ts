import { describe, expect, it } from 'vitest';
import { newProject } from '../project/commands';
import { Store } from '../project/store';
import { PPQ } from '../project/types';
import { cellView, centsToName, playRow, positionLabel, rowCount } from './pattern';

describe('pattern grid', () => {
  it('names pitches and finds the play row', () => {
    expect(centsToName(6000)).toBe('C5');
    expect(centsToName(4800)).toBe('C4');
    const store = new Store(newProject('x'));
    const sec = store.doc.sections[0];
    expect(rowCount(sec)).toBe(32);
    expect(playRow(sec, 0)).toBe(0);
    expect(playRow(sec, PPQ)).toBe(4);
    expect(playRow(sec, sec.beats * PPQ - 1)).toBe(31);
    expect(positionLabel(sec, PPQ + PPQ / 4)).toBe('2.2');
  });

  it('shows the note or hit that starts in the row', () => {
    const store = new Store(newProject('x'));
    const secId = store.doc.sections[0].id;
    const mel = store.doc.layers[0];
    const beat = store.doc.layers[1];
    store.dispatch('addNote', { section: secId, layer: mel.id, note: { id: 'n', t: PPQ, dur: 480, cents: 6000, hard: 0.4 } });
    store.dispatch('addHit', { section: secId, layer: beat.id, hit: { id: 'h', t: 0, dur: 120, slot: 2, strength: 0.9, shape: { attack: 0, burst: 0, cents: null, bend: 0, hold: 0, vowel: 0, tone: 0, endStop: 0, endClick: 0 } } });
    const sec = store.doc.sections[0];
    expect(cellView(sec, store.doc.layers[0], 4).label).toBe('C5');
    expect(cellView(sec, store.doc.layers[0], 0).eventId).toBeNull();
    expect(cellView(sec, store.doc.layers[1], 0).label).toBe('hat');
  });
});
