# For developers

[Guide contents](README.md)

The user guide describes the app as a musician sees it. The documents below are the design notes and build records behind it, written for people working on Earworm's code. They are kept as they were written, stage by stage; where they and this guide differ, the code decides.

How to run the app locally is in the [README](../../README.md#run-locally).

## The whole design

- [TDD.md](../TDD.md): the technical design, from the engine to the interface.
- [coverage.md](../coverage.md): which capability of each milestone is reachable in the app.
- [params-dictionary.md](../../web/src/gen/params-dictionary.md): every synth parameter, generated from the module catalog.

## Milestones

- [M0.md](../M0.md): the two throwaway prototypes, Live follow and Influence match.
- [M1.md](../M1.md): the engine core: layers, drums, space, sequencer, offline render, benchmark.
- [M2.md](../M2.md): capture, the second pass, hits and kit slots, tuning.
- [M3.md](../M3.md): the track: sections, layers, timeline editing, undo, autosave, overdub placement.
- [M4.md](../M4.md): Influence, the grain source, states and the morph lane.
- [M5.md](../M5.md): the single app, the click, the project file.

## Modular synth

- [synth.md](../synth.md): the design.
- [S1.md](../S1.md): the engine runs patches.
- [S2.md](../S2.md): the modulation matrix, the Patch and Mod views.
- [S3.md](../S3.md): editing patches, the cost meter, presets.
- [S4.md](../S4.md): live play, MIDI, MIDI learn, scenes.
- [S5.md](../S5.md): recording passes, automation lanes, the raw input.

## Quantizing

- [quant.md](../quant.md): the design of Feel and stretching.
- [Q1.md](../Q1.md): anchors and Feel.
- [Q2.md](../Q2.md): stretching take audio, importing takes; listening examples in [q2-examples](../q2-examples).

## Polyphonic capture

- [poly.md](../poly.md): feasibility and design.
- [P3.md](../P3.md): Poly takes with the Basic Pitch note model.

## Meters

- [meter.md](../meter.md): the design of re-metering.
- [R1.md](../R1.md): typed meters, locks and patterns; listening examples in [r1-examples](../r1-examples).

## Interface

- [ui.md](../ui.md): the interface audit, the model and the design language.
- [UI1.md](../UI1.md): the interface pass.

## This guide

The screenshots in [images](images) were taken from the running app in headless Chrome at 1440 × 900 (scaled to 1200 wide) and 390 × 844, plus 844 × 390 and 1024 × 768 for the phone-on-its-side and tablet views, and saved as WebP. The tutorials were walked in the same browser: capture, Record, Import take, Feel, Poly and the meter row, with a stand-in microphone that played recorded audio into the page, because headless Chrome on the build machine cannot open a real one.

The in-app lessons are data in `web/src/app/tutorials.ts`. Tutorial 3 follows the lesson "Make hardness set the attack" step for step; keep the two in step when either changes.
