// The offline renderer wants blobs grouped by kind. The compiler already produced that list
// for the live engine; this only sorts it.

import { LOAD_CURVE, LOAD_GRAIN, LOAD_LAYER, LOAD_STATES, LOAD_TRACK } from '../m1/protocol';
import type { SongBlobs } from '../m1/render-core';
import type { Upload } from '../project/compile';
import type { Project } from '../project/types';

export function uploadsToSong(uploads: Upload[]): SongBlobs {
  const song: SongBlobs = { track: new ArrayBuffer(0), layers: [] };
  for (const u of uploads) {
    if (u.kind === LOAD_TRACK) song.track = u.blob;
    else if (u.kind === LOAD_LAYER) song.layers[u.slot] = u.blob;
    else if (u.kind === LOAD_CURVE) (song.curves ??= []).push({ slot: u.slot, buf: u.blob });
    else if (u.kind === LOAD_GRAIN) (song.grains ??= []).push({ slot: u.slot, buf: u.blob });
    else if (u.kind === LOAD_STATES) (song.states ??= []).push({ slot: u.slot, buf: u.blob });
  }
  if (!song.track.byteLength) throw new Error('the song has no track to render');
  return song;
}

/** One pass of the song, in seconds, before the renderer's tail. */
export function songSeconds(doc: Project): number {
  let s = 0;
  for (const sec of doc.sections) {
    const one = sec.feel === 'grid' ? (sec.beats * 60) / Math.max(1, sec.bpm) : sec.seconds;
    s += one * Math.max(1, sec.repeats);
  }
  return s;
}
