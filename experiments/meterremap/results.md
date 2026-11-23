# Meter remap prototype: results

Output of `cargo run --release --manifest-path experiments/meterremap/Cargo.toml` with METERREMAP_LOOPS set. See docs/meter.md for what it means.

## Remapped grooves

Each groove is 4 bars, rendered as performed (5 ms timing spread, the snare late, the bass a little late), then remapped. The ideal is the same events re-sequenced by the pattern and rendered, held notes released where the new order breaks them and the room running on: what the band would play in the new meter. Distances are against the ideal; lower is better.

- **engine, one take**: the whole mix as one take, cut at its hard anchors, played by the engine's stretcher (hits mode) unchanged.
- **engine, stems**: drums, bass and keys as three layers locked to one meter, each cut at its own anchors (or at the beat where it has none), summed.
- **grid cut**: the mix cut exactly at the beat times instead of at anchors, same stretcher.
- **slicer, stems**: as engine, stems, but a 30 ms equal-power crossfade where a stem has no anchor to cut at (held keys and bass); 5 ms at anchors.

| Case | Pattern | Joins per bar | Render | Spectral distance, all / near joins (dB) | Click before joins, mean / worst (dB over ideal) | Residual after joins (dB) | Residual, all (dB) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 5/4 to 4/4, beat 5 dropped | 1 2 3 4 | 1 | engine, one take | 0.64 / 2.14 | -5.5 / -1.4 | -15.1 | -21.7 |
| 5/4 to 4/4, beat 5 dropped | 1 2 3 4 | 1 | engine, stems | 0.64 / 2.19 | -7.9 / +0.0 | -16.5 | -23.1 |
| 5/4 to 4/4, beat 5 dropped | 1 2 3 4 | 1 | grid cut | 0.67 / 1.82 | +6.1 / +23.6 | -17.4 | -22.4 |
| 5/4 to 4/4, beat 5 dropped | 1 2 3 4 | 1 | slicer, stems | 0.63 / 2.15 | -5.5 / +0.0 | -16.5 | -23.1 |
| 5/4 to 4/4, beat 4 dropped | 1 2 3 5 | 1 | engine, one take | 3.68 / 9.97 | +1.9 / +3.1 | -13.8 | -20.3 |
| 5/4 to 4/4, beat 4 dropped | 1 2 3 5 | 1 | engine, stems | 3.62 / 9.50 | -1.3 / +2.9 | -14.3 | -20.7 |
| 5/4 to 4/4, beat 4 dropped | 1 2 3 5 | 1 | grid cut | 3.70 / 10.09 | +5.4 / +30.8 | -14.3 | -19.5 |
| 5/4 to 4/4, beat 4 dropped | 1 2 3 5 | 1 | slicer, stems | 3.73 / 9.99 | +0.5 / +3.8 | -14.2 | -20.5 |
| 4/4 to 3/4, beat 4 dropped | 1 2 3 | 1 | engine, one take | 0.41 / 1.46 | -3.4 / -0.9 | -18.1 | -23.7 |
| 4/4 to 3/4, beat 4 dropped | 1 2 3 | 1 | engine, stems | 0.39 / 1.27 | -6.9 / -0.1 | -19.1 | -25.3 |
| 4/4 to 3/4, beat 4 dropped | 1 2 3 | 1 | grid cut | 0.44 / 1.17 | +15.9 / +43.1 | -19.6 | -24.7 |
| 4/4 to 3/4, beat 4 dropped | 1 2 3 | 1 | slicer, stems | 0.39 / 1.30 | -4.4 / +0.2 | -19.1 | -25.2 |
| 3/4 to 4/4, beat 3 twice | 1 2 3 3 | 1 | engine, one take | 0.21 / 0.50 | -1.6 / -0.5 | -22.8 | -30.5 |
| 3/4 to 4/4, beat 3 twice | 1 2 3 3 | 1 | engine, stems | 0.22 / 0.46 | -0.2 / +0.0 | -22.8 | -30.8 |
| 3/4 to 4/4, beat 3 twice | 1 2 3 3 | 1 | grid cut | 0.34 / 0.90 | +10.0 / +31.6 | -23.8 | -19.1 |
| 3/4 to 4/4, beat 3 twice | 1 2 3 3 | 1 | slicer, stems | 0.22 / 0.46 | -0.2 / +0.0 | -22.8 | -30.8 |
| 3/4 to 4/4, beat 1 twice | 1 1 2 3 | 1 | engine, one take | 0.59 / 1.70 | -7.6 / -1.1 | -15.3 | -19.2 |
| 3/4 to 4/4, beat 1 twice | 1 1 2 3 | 1 | engine, stems | 0.57 / 1.58 | -5.4 / -0.1 | -15.5 | -19.7 |
| 3/4 to 4/4, beat 1 twice | 1 1 2 3 | 1 | grid cut | 0.81 / 2.92 | -17.3 / +1.8 | -17.5 | -18.0 |
| 3/4 to 4/4, beat 1 twice | 1 1 2 3 | 1 | slicer, stems | 0.62 / 1.67 | -3.8 / +0.3 | -15.6 | -19.8 |
| 4/4 shuffled 1 3 4 2 | 1 3 4 2 | 3 | engine, one take | 2.51 / 3.69 | -2.3 / +1.1 | -15.1 | -18.3 |
| 4/4 shuffled 1 3 4 2 | 1 3 4 2 | 3 | engine, stems | 2.48 / 3.49 | -3.0 / +1.7 | -15.7 | -19.2 |
| 4/4 shuffled 1 3 4 2 | 1 3 4 2 | 3 | grid cut | 2.60 / 3.76 | -0.3 / +35.8 | -15.9 | -18.2 |
| 4/4 shuffled 1 3 4 2 | 1 3 4 2 | 3 | slicer, stems | 2.56 / 3.69 | -2.0 / +2.2 | -15.6 | -18.9 |

### Notes at the joins

Bass and keys notes in the 4 bars. A note is cut when it runs more than a quarter beat past the end of its run; a slice starts inside a note when the note began before the slice and sounds on for more than a quarter beat. Soft cuts: run starts where a stem had no anchor near the beat (summed over the three stems).

| Case | Joins | Notes cut | Slices starting inside a note | Soft cuts |
| --- | --- | --- | --- | --- |
| 5/4 to 4/4, beat 5 dropped | 3 | 4 | 0 | 0 |
| 5/4 to 4/4, beat 4 dropped | 4 | 0 | 4 | 4 |
| 4/4 to 3/4, beat 4 dropped | 3 | 4 | 0 | 0 |
| 3/4 to 4/4, beat 3 twice | 4 | 0 | 0 | 0 |
| 3/4 to 4/4, beat 1 twice | 4 | 4 | 0 | 4 |
| 4/4 shuffled 1 3 4 2 | 11 | 4 | 4 | 12 |

### Does the result read as the new meter?

The meter detector below, run on the engine-stems render with the output beats known.

| Case | Beats per bar wanted | Detected | Downbeat found at |
| --- | --- | --- | --- |
| 5/4 to 4/4, beat 5 dropped | 4 | 4 | beat 1 |
| 5/4 to 4/4, beat 4 dropped | 4 | 4 | beat 1 |
| 4/4 to 3/4, beat 4 dropped | 3 | 3 | beat 1 |
| 3/4 to 4/4, beat 3 twice | 4 | 4 | beat 2 |
| 3/4 to 4/4, beat 1 twice | 4 | 4 | beat 3 |
| 4/4 shuffled 1 3 4 2 | 4 | 4 | beat 1 |

## Bars from audio: synthetic grooves

Random grooves in 3 to 7 beats per bar, 80 to 170 bpm, 8 bars, each started a random number of beats into the groove (a pickup), so the downbeat is not at the start. The beats are given (as the section grid gives them); the detector finds the beats per bar from 2 to 7 and the downbeat. Ten grooves per meter and band.

| Takes | Count | Beats per bar right | Half or double | Downbeat right, when beats per bar right | Downbeat right, beats per bar typed |
| --- | --- | --- | --- | --- | --- |
| Full | 50 | 86% (43 of 50) | 14% (7 of 50) | 91% (39 of 43) | 90% (45 of 50) |
| DrumsOnly | 50 | 100% (50 of 50) | 0% (0 of 50) | 72% (36 of 50) | 72% (36 of 50) |
| NoDrums | 50 | 84% (42 of 50) | 16% (8 of 50) | 100% (42 of 42) | 100% (50 of 50) |
| KeysOnly | 50 | 86% (43 of 50) | 12% (6 of 50) | 100% (43 of 43) | 100% (50 of 50) |
| **All** | 200 | 89% (178 of 200) | 10% (21 of 200) | 90% (160 of 178) | 90% (181 of 200) |
| 3 beats per bar | 40 | 55% (22 of 40) | 45% (18 of 40) | 100% (22 of 22) | 98% (39 of 40) |
| 4 beats per bar | 40 | 100% (40 of 40) | 0% (0 of 40) | 95% (38 of 40) | 95% (38 of 40) |
| 5 beats per bar | 40 | 100% (40 of 40) | 0% (0 of 40) | 95% (38 of 40) | 95% (38 of 40) |
| 6 beats per bar | 40 | 92% (37 of 40) | 8% (3 of 40) | 86% (32 of 37) | 88% (35 of 40) |
| 7 beats per bar | 40 | 98% (39 of 40) | 0% (0 of 40) | 77% (30 of 39) | 78% (31 of 40) |
| The 5/4 groove above, every start beat | 5 | 100% (5 of 5) | 0% (0 of 5) | 100% (5 of 5) | 100% (5 of 5) |
| The 4/4 groove above, every start beat | 4 | 100% (4 of 4) | 0% (0 of 4) | 100% (4 of 4) | 100% (4 of 4) |
| The 3/4 groove above, every start beat | 3 | 100% (3 of 3) | 0% (0 of 3) | 67% (2 of 3) | 67% (2 of 3) |

Cost: 4.5 ms per second of audio (features and detection, one core).

## Bars from audio: real recordings

1738 Apple Loops in 4/4 (the loops GarageBand installs), each tiled to at least 16 beats and started a random number of beats in. The truth is 4 beats per bar with the downbeat where the loop starts, as Apple loops are cut.

| Loops | Count | Beats per bar right (4) | Read as 2 | Downbeat right, when 4 found | Downbeat right, 4 typed |
| --- | --- | --- | --- | --- | --- |
| Bass | 344 | 70% (241 of 344) | 8% (28 of 344) | 53% (128 of 241) | 51% (175 of 344) |
| Drums | 541 | 94% (511 of 541) | 2% (9 of 541) | 45% (228 of 511) | 44% (240 of 541) |
| Guitars | 311 | 68% (212 of 311) | 8% (25 of 311) | 56% (118 of 212) | 56% (175 of 311) |
| Keyboards | 366 | 67% (246 of 366) | 13% (47 of 366) | 64% (158 of 246) | 65% (237 of 366) |
| Other | 171 | 68% (116 of 171) | 11% (18 of 171) | 50% (58 of 116) | 57% (97 of 171) |
| **All** | 1733 | 77% (1326 of 1733) | 7% (127 of 1733) | 52% (690 of 1326) | 53% (924 of 1733) |
| Bands: drums, bass and keys or guitar summed | 208 | 89% (186 of 208) | 1% (3 of 208) | 54% (101 of 186) | 54% (112 of 208) |

Wrong beats per bar, by what was found instead: 2: 127, 3: 39, 5: 20, 6: 43, 7: 178.

## Choosing the downbeat cues

Downbeat right with the beats per bar typed, for each weighting tried. The detector uses the row in bold.

| Cues | Random grooves | Named grooves, every start | Real loops |
| --- | --- | --- | --- |
| low 1, beat change 1, full 0.5 (first try) | 89% (178 of 200) | 25% (3 of 12) | 52% (909 of 1733) |
| low 1, full 0.5 | 76% (151 of 200) | 25% (3 of 12) | 48% (827 of 1733) |
| low 1 | 73% (146 of 200) | 0% (0 of 12) | 50% (875 of 1733) |
| bar change 1 | 58% (117 of 200) | 75% (9 of 12) | 49% (845 of 1733) |
| low 1, full 0.5, bar change 1 | 83% (166 of 200) | 75% (9 of 12) | 52% (893 of 1733) |
| low 1, full 0.5, bar change 2 | 82% (164 of 200) | 100% (12 of 12) | 52% (900 of 1733) |
| low 1, bar change 1 | 82% (163 of 200) | 75% (9 of 12) | 53% (927 of 1733) |
| **low 1, beat change 0.5, full 0.5, bar change 1** | 90% (181 of 200) | 92% (11 of 12) | 53% (924 of 1733) |
| low 1, mid -0.5 | 67% (134 of 200) | 0% (0 of 12) | 46% (804 of 1733) |
| low 1, mid -0.5, bar change 1 | 86% (173 of 200) | 75% (9 of 12) | 53% (910 of 1733) |
| low 1, mid -1, bar change 1 | 68% (137 of 200) | 75% (9 of 12) | 45% (787 of 1733) |
| low 1, high 0.5, mid -0.5, bar change 1 | 89% (178 of 200) | 75% (9 of 12) | 53% (920 of 1733) |
| low 1, beat change 0.5, mid -0.5, bar change 1 | 89% (178 of 200) | 83% (10 of 12) | 55% (950 of 1733) |

| Loops (kind, beats) | Right | 1 beat late | 2 beats off | 1 beat early |
| --- | --- | --- | --- | --- |
| Bass 16 | 101 | 19 | 38 | 35 |
| Bass 32 | 23 | 2 | 5 | 3 |
| Bass 8 | 46 | 17 | 23 | 23 |
| Drums 16 | 54 | 22 | 22 | 14 |
| Drums 32 | 106 | 39 | 29 | 31 |
| Drums 4 | 7 | 6 | 1 | 1 |
| Drums 8 | 70 | 47 | 32 | 53 |
| Guitars 16 | 83 | 17 | 20 | 16 |
| Guitars 4 | 3 | 3 | 2 | 6 |
| Guitars 8 | 87 | 18 | 33 | 20 |
| Horn/Wind 16 | 7 | 0 | 2 | 2 |
| Horn/Wind 8 | 7 | 4 | 2 | 1 |
| Keyboards 16 | 140 | 19 | 27 | 25 |
| Keyboards 32 | 31 | 4 | 6 | 2 |
| Keyboards 4 | 7 | 2 | 1 | 0 |
| Keyboards 8 | 56 | 11 | 17 | 13 |
| Mixed 16 | 10 | 1 | 2 | 1 |
| Mixed 8 | 8 | 2 | 0 | 0 |
| Percussion 4 | 6 | 3 | 1 | 2 |
| Percussion 8 | 15 | 6 | 10 | 4 |
| Strings 16 | 9 | 1 | 2 | 2 |
| Strings 8 | 8 | 1 | 4 | 1 |

## Remapping real loops

A band of three real loops at one tempo, 4 bars of 4/4: 09 Disco Funk: Boogie Right Beat 01, Agile Funk Synth Bass 01, Agile Funk Synth Lead 01 (116 bpm). No ideal exists for real audio, so the join measure here compares the high band in the 8 ms before each join with the same before every beat of the source: 0 dB means a join is no spikier than the take's own beat starts. The loops start on their downbeats, so the cuts use the true bars.

| Pattern | Render | Joins | Spike before joins, mean / worst (dB) |
| --- | --- | --- | --- |
| 1 2 3 | engine, one take | 3 | -18.5 / -13.3 |
| 1 2 3 | engine, stems | 3 | -10.8 / -8.9 |
| 1 2 3 | slicer, stems | 3 | -13.5 / -10.4 |
| 1 2 3 | grid cut | 3 | -11.7 / -8.0 |
| 1 3 4 2 | engine, one take | 11 | -21.6 / -5.2 |
| 1 3 4 2 | engine, stems | 11 | -26.6 / -10.4 |
| 1 3 4 2 | slicer, stems | 11 | -19.0 / -8.8 |
| 1 3 4 2 | grid cut | 11 | -12.7 / +0.3 |
| 1 2 3 4 4 | engine, one take | 4 | -10.8 / -0.4 |
| 1 2 3 4 4 | engine, stems | 4 | -10.8 / -1.3 |
| 1 2 3 4 4 | slicer, stems | 4 | -10.6 / -0.6 |
| 1 2 3 4 4 | grid cut | 4 | -0.2 / +0.2 |

## Determinism

- hits: two renders bit-identical; random call sizes 1 to 3,000 bit-identical; play from eight points: each equals the whole render from the next slice start on (differences last at most 0 ms).
- smooth: two renders bit-identical; random call sizes 1 to 3,000 bit-identical; play from eight points: each equals the whole render from the next slice start on (differences last at most 583 ms).

## Cost

The engine's stretcher playing a remapped 8-bar 5/4 take (25 runs) against the same take as one run, ns per output sample, best of 5, 128-sample calls:

| Mode | Remapped | One run |
| --- | --- | --- |
| Hits | 7.7 | 7.5 |
| Smooth | 122.6 | 117.0 |

Building the runs from beats and anchors (anchors already found): 5.9 us for 8 bars.

