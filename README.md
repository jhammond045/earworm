# Earworm

Earworm is a local-first web app: sing or hum into the page, and shape that sound into a track. It runs in the browser. Nothing is uploaded.

The public build is at <https://jhammond045.github.io/earworm/>.

Use headphones and a microphone. Headphones keep the speakers out of the mic, which Live follow needs.

## Run locally

```sh
pnpm install
pnpm dev
```

Open the URL Vite prints (normally <http://localhost:5173>). `pnpm build` writes the static site to `web/dist`. A production build uses the base path `/earworm/` so it can be served from GitHub Pages. Set `EARWORM_BASE` when you build to use another path. The dev server stays at `/`.

Design: see docs/TDD.md.

- Milestone M0 (two throwaway prototypes, Live follow and Influence match): see docs/M0.md for what each prototype tests, how to judge it, and what was measured.
- Milestone M1 (the engine core: 8 layers of voices, drums, space, sequencer, offline render, benchmark): see docs/M1.md for how to run it and how to judge its gate on the reference laptop.
- Milestone M2 (capture: Live follow on the new engine, hold to capture, the second pass with notes, hits, hit shapes and kit slots, tuning): see docs/M2.md for how to run it and how to judge its gate (50 ms follow, hums kept).
- Milestone M3 (the track: sections, layers, tempo, timeline editing with undo, autosave, overdub capture placed with the round trip taken out): see docs/M3.md for how to run it and how to judge its gate (edit while playing, no loss).
- Milestone M4 (Influence over the full parameter table with live match, the grain source, kit slots from a sound, sound states and the morph lane): see docs/M4.md for how to run it and how to judge its gate (blind match 70% at 100%).
- Modular synth, stage S1 (the engine runs patches; the sound does not change): see docs/synth.md for the design and docs/S1.md for how to check it.
- Modular synth, stage S2 (the modulation matrix: every source, the shaper, layer LFOs, listening layers; the Patch and Mod views in the Sound panel): see docs/S2.md for how to check it.
- Modular synth, stage S3 (editing patches: oscillators, noise, a sampler, a channel filter; add, remove, rewire and reorder; the cost meter and voice count; preset save, load, import and export; a 5 ms fade on structure changes): see docs/S3.md for how to check it.
- Modular synth, stage S5 (recording and automation: Record a pass of notes, knob moves and scene taps; automation lanes in the timeline; the raw input beside the synth, crossfaded by a value): see docs/S5.md for how to check it.
- Polyphonic capture (a guitar's chords, a piano or other polyphonic sources into synth voices; design and feasibility on synthetic and public real recordings): see docs/poly.md for the measurements and the design, and experiments/poly for the prototype. Stage P3 (poly takes: a take into a layer set to Poly comes back as notes, several at once, each with its own curve, from a Rust port of Basic Pitch's note model): see docs/P3.md for how to check it.
- Flex quantize (quantizing at play time, one layer setting the timing of others, stretching take audio between anchors): see docs/quant.md for the design, and experiments/flexquant for the throwaway prototype behind its numbers. Stage Q1 (anchors found with every take, Feel on a clip quantizes its notes, hits and curves when the track plays; Quantize turns it on): see docs/Q1.md for how to check it. Stage Q2 (the take's audio stretched through the time map with its pitch kept, hits or smooth; import an audio file as a take): see docs/Q2.md for how to check it, and docs/q2-examples for before and after renders to listen to.
