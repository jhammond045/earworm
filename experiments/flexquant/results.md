<!-- Output of: cargo run --release --manifest-path experiments/flexquant/Cargo.toml. Run on Nov 22, 2026 on the build machine (Apple M4, 10 cores) with other work running; 1-minute load 3.41 when the run started. -->

# Flex quantize prototype results

Takes (48 kHz, 100 bpm, synthetic, so every onset's true time and grid slot is known): drum loop A, 4 bars of 16ths, played with 58% swing, the snare 12 ms late, 6 ms random timing and a 10 ms push and drag over the loop (10.4 s); drum loop B, the same pattern straight, the snare 8 ms early, 12 ms random timing (10.4 s); a sung line of 18 notes (detached, "da" and legato joins), 15 ms late on average with 20 ms random timing (10.5 s).

## Anchor accuracy

A found anchor matches a true onset within 50 ms, one to one. Errors are found minus true, in ms.

| Take and detector | True | Matched | Missed | Extra | Mean error | p95 | Max | Bias |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Drums A, audio onsets (second pass detector) | 64 | 44 | 20 | 0 | 1.7 | 7.0 | 8.3 | -1.7 |
| Drums A, second pass hits | 64 | 44 | 20 | 0 | 1.7 | 7.0 | 8.3 | -1.7 |
| Drums A, live frames' onset slot (10 ms) | 64 | 40 | 24 | 0 | 15.8 | 19.8 | 20.1 | 15.8 |
| Drums A, live frames refined on the audio | 64 | 40 | 24 | 0 | 1.8 | 7.0 | 8.3 | -1.8 |
| Drums A, kick and snare only, audio onsets | 20 | 20 | 0 | 24 | 2.3 | 7.0 | 7.9 | -2.3 |
| Drums A, audio onsets plus a 4 kHz high band pass | 64 | 56 | 8 | 0 | 1.4 | 7.0 | 8.3 | -1.4 |
| Drums B, audio onsets | 64 | 44 | 20 | 0 | 1.0 | 3.9 | 7.3 | -1.0 |
| Drums B, audio onsets plus a 4 kHz high band pass | 64 | 56 | 8 | 0 | 0.9 | 3.9 | 7.3 | -0.9 |
| Sung, audio onsets | 18 | 14 | 4 | 0 | 1.3 | 5.0 | 5.0 | -0.4 |
| Sung, second pass note starts | 18 | 18 | 0 | 0 | 7.2 | 30.6 | 30.6 | 5.9 |
| Sung, live frames' onset slot (10 ms) | 18 | 5 | 13 | 0 | 17.5 | 22.7 | 22.7 | 17.5 |
| Sung, live frames refined on the audio | 18 | 5 | 13 | 0 | 1.2 | 1.7 | 1.7 | 0.7 |
| Sung, legato joins only, second pass | 4 | 4 | 0 | 14 | 28.0 | 30.6 | 30.6 | 28.0 |
| Sung, second pass with legato joins moved to the glide's midpoint | 18 | 18 | 0 | 0 | 3.3 | 26.7 | 26.7 | 2.0 |
| Sung, legato joins only, moved to the glide's midpoint | 4 | 4 | 0 | 14 | 10.6 | 26.7 | 26.7 | 10.6 |

Detection cost, single core: audio onsets 0.7 ms per second of audio; the whole second pass 18.1 ms (drums) and 17.2 ms (sung) per second of audio. It already runs in a worker after every take.

## Quantizing and stretching

Each case quantizes the anchors, builds the warp map and plays the take through it four ways. The line above each table says how close the warp map itself puts every true onset to where it should land: that is the anchors' accuracy, whatever plays the audio. The table then checks the audio. "Attacks matched" looks for the 22 ms of take audio around each hard anchor (2 ms before, 20 ms after) in the output, within 30 ms of where the map puts it, and gives the offset and the correlation where it matches best. The onset columns find the onsets in the output with the same detector that found the anchors and compares them with where the map puts the onsets that detector finds in the input (within 50 ms), so the detector's own bias cancels. Spectral distance compares third-octave band levels with the take rendered with every event exactly on the grid (lower is closer; the input row is what the timing alone costs). Pitch error is the median YIN pitch 60 to 210 ms into each note against the sung pitch. Attack rows compare the 10 ms after each kick and snare, and the 15 to 2 ms before it, with the ideal render: smeared or doubled attacks show as a lower peak and more level before the attack. CPU is the best of five whole-take renders on one core of the build machine.

### 1. Drums A to straight 16ths, full strength

56 anchors, 56 pulled to a slot, 58 warp points. True onsets against where they should land, before: mean 15.0 ms, p95 37.1 ms, max 47.0 ms (64); after the warp map: mean 4.0 ms, p95 25.4 ms, max 35.8 ms (64); of those, the onsets with an anchor: mean 1.3 ms, p95 7.0 ms, max 7.6 ms (56), the ones without: mean 22.4 ms, p95 35.8 ms, max 35.8 ms (8).

| Method | Onsets found in the output | Off where the map puts them: mean / p95 / max ms | Attacks matched at hard anchors: off mean / max ms, correlation | Spectral distance to ideal, dB | Attack peak vs ideal, mean / worst dB | Before the attack vs ideal, mean dB | CPU ns per sample | x real time |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| input, not quantized | 56/56 | 13.3 / 34.0 / 37.0 | - | 4.40 | 8.80 / 53.80 | +9.7 | - | - |
| varispeed (today) | 56/56 | 0.6 / 1.0 / 12.0 | 10.8 / 27.8 (0.419) | 4.91 | 1.19 / 2.53 | -1.4 | 4.5 | 4589 |
| slice | 56/56 | 2.4 / 5.0 / 5.0 | 0.0 / 0.0 (0.999) | 3.18 | 0.06 / 0.30 | -15.9 | 4.2 | 4933 |
| wsola | 56/56 | 1.3 / 13.0 / 16.0 | 0.0 / 0.0 (1.000) | 3.45 | 0.06 / 0.30 | -0.6 | 55.2 | 377 |
| phase vocoder | 56/56 | 1.5 / 14.0 / 20.0 | 0.0 / 0.0 (1.000) | 3.17 | 0.06 / 0.30 | -0.6 | 108.1 | 193 |

### 2. Drums A to 16ths with 58% swing (the feel it was played with)

56 anchors, 56 pulled to a slot, 58 warp points. True onsets against where they should land, before: mean 8.5 ms, p95 23.0 ms, max 25.2 ms (64); after the warp map: mean 2.1 ms, p95 7.5 ms, max 15.9 ms (64); of those, the onsets with an anchor: mean 1.4 ms, p95 7.2 ms, max 8.9 ms (56), the ones without: mean 7.2 ms, p95 15.9 ms, max 15.9 ms (8).

| Method | Onsets found in the output | Off where the map puts them: mean / p95 / max ms | Attacks matched at hard anchors: off mean / max ms, correlation | CPU ns per sample | x real time |
| --- | --- | --- | --- | --- | --- |
| input, not quantized | 56/56 | 8.1 / 23.0 / 26.0 | - | - | - |
| varispeed (today) | 56/56 | 0.7 / 9.0 / 11.0 | 9.2 / 26.8 (0.473) | 4.5 | 4656 |
| slice | 56/56 | 2.3 / 7.0 / 8.0 | 0.0 / 0.0 (1.000) | 4.0 | 5150 |
| wsola | 56/56 | 0.3 / 4.0 / 7.0 | 0.0 / 0.0 (1.000) | 54.6 | 381 |
| phase vocoder | 56/56 | 0.5 / 5.0 / 7.0 | 0.0 / 0.0 (1.000) | 108.3 | 192 |

### 3. Drums A to straight 16ths, strength automated from 0 to 1 over the loop

56 anchors, 56 pulled to a slot, 58 warp points. True onsets against where they should land, before: mean 6.2 ms, p95 15.4 ms, max 25.9 ms (64); after the warp map: mean 2.0 ms, p95 9.6 ms, max 27.8 ms (64); of those, the onsets with an anchor: mean 0.7 ms, p95 5.0 ms, max 6.3 ms (56), the ones without: mean 11.3 ms, p95 27.8 ms, max 27.8 ms (8).

| Method | Onsets found in the output | Off where the map puts them: mean / p95 / max ms | Attacks matched at hard anchors: off mean / max ms, correlation | CPU ns per sample | x real time |
| --- | --- | --- | --- | --- | --- |
| input, not quantized | 56/56 | 5.5 / 12.0 / 24.6 | - | - | - |
| varispeed (today) | 56/56 | 0.8 / 5.9 / 11.4 | 10.4 / 26.6 (0.455) | 4.5 | 4636 |
| slice | 56/56 | 1.9 / 5.4 / 5.5 | 0.0 / 0.0 (0.999) | 4.1 | 5084 |
| wsola | 56/56 | 1.1 / 10.2 / 12.6 | 0.0 / 0.0 (1.000) | 54.3 | 384 |
| phase vocoder | 56/56 | 1.0 / 5.9 / 20.6 | 0.0 / 0.0 (1.000) | 107.4 | 194 |

Offset from the straight grid after quantizing, by bar, as a share of the offset before (the lane is at 0.125, 0.375, 0.625, 0.875 mid-bar): bar 1 0.87; bar 2 0.64; bar 3 0.38; bar 4 0.12;

### 4. Sung line to straight 8ths, full strength

18 anchors, 18 pulled to a slot, 20 warp points. True onsets against where they should land, before: mean 14.8 ms, p95 50.4 ms, max 50.4 ms (18); after the warp map: mean 3.3 ms, p95 27.4 ms, max 27.4 ms (18).

| Method | Onsets found in the output | Off where the map puts them: mean / p95 / max ms | Attacks matched at hard anchors: off mean / max ms, correlation | Spectral distance to ideal, dB | Pitch error: mean / max cents | CPU ns per sample | x real time |
| --- | --- | --- | --- | --- | --- | --- | --- |
| input, not quantized | 17/18 | 12.9 / 41.0 / 41.0 | - | 2.99 | 0.1 / 0.3 | - | - |
| varispeed (today) | 16/18 | 0.6 / 4.6 / 4.6 | 11.0 / 29.5 (0.477) | 4.01 | 67.5 / 208.2 | 3.7 | 5587 |
| slice | 18/18 | 7.2 / 50.0 / 50.0 | 0.0 / 0.0 (1.000) | 1.57 | 0.3 / 3.8 | 3.9 | 5373 |
| wsola | 18/18 | 1.0 / 11.0 / 11.0 | 0.0 / 0.0 (1.000) | 2.36 | 0.3 / 2.1 | 62.3 | 335 |
| phase vocoder | 18/18 | 0.1 / 0.8 / 0.8 | 0.0 / 0.0 (1.000) | 1.96 | 0.3 / 3.3 | 97.1 | 215 |

Level at the four legato joins of case 4 (dB below the level around them; the joins carry no level change, so closer to the input is better):

| Join | Input | WSOLA, legato anchors soft | Phase vocoder, soft | WSOLA, every anchor hard | Phase vocoder, every anchor hard |
| --- | --- | --- | --- | --- | --- |
| beat 1.5 | -2.2 | -2.1 | -2.2 | -2.0 | -2.0 |
| beat 5 | -2.1 | -1.9 | -1.8 | -4.2 | -5.7 |
| beat 10 | -1.3 | -1.3 | -1.3 | -3.5 | -10.8 |
| beat 13 | -2.4 | -2.4 | -2.5 | -2.3 | -6.0 |

### 5. Sung line to drum loop A's groove, hit by hit (8ths)

18 anchors, 18 pulled to a slot, 20 warp points. True onsets against where they should land, before: mean 18.1 ms, p95 58.4 ms, max 58.4 ms (18); after the warp map: mean 3.4 ms, p95 28.9 ms, max 28.9 ms (18).

| Method | Onsets found in the output | Off where the map puts them: mean / p95 / max ms | Attacks matched at hard anchors: off mean / max ms, correlation | Pitch error: mean / max cents | CPU ns per sample | x real time |
| --- | --- | --- | --- | --- | --- | --- |
| input, not quantized | 17/18 | 15.4 / 49.0 / 49.0 | - | 0.1 / 0.2 | - | - |
| varispeed (today) | 15/18 | 2.9 / 37.0 / 37.0 | 8.7 / 29.5 (0.478) | 101.0 / 247.4 | 3.7 | 5566 |
| slice | 18/18 | 11.9 / 48.0 / 48.0 | 0.0 / 0.0 (1.000) | 0.5 / 4.7 | 3.9 | 5348 |
| wsola | 18/18 | 0.9 / 13.0 / 13.0 | 0.0 / 0.0 (1.000) | 0.2 / 0.5 | 62.1 | 335 |
| phase vocoder | 18/18 | 0.3 / 3.0 / 3.0 | 0.0 / 0.0 (1.000) | 0.3 / 2.2 | 97.6 | 213 |

### 6. Drums B to drum loop A's groove, hit by hit (16ths)

56 anchors, 56 pulled to a slot, 58 warp points. True onsets against where they should land, before: mean 14.6 ms, p95 37.0 ms, max 45.2 ms (64); after the warp map: mean 2.1 ms, p95 8.1 ms, max 27.0 ms (64); of those, the onsets with an anchor: mean 0.9 ms, p95 3.8 ms, max 8.1 ms (56), the ones without: mean 10.4 ms, p95 27.0 ms, max 27.0 ms (8).

| Method | Onsets found in the output | Off where the map puts them: mean / p95 / max ms | Attacks matched at hard anchors: off mean / max ms, correlation | CPU ns per sample | x real time |
| --- | --- | --- | --- | --- | --- |
| input, not quantized | 56/56 | 15.5 / 42.0 / 46.0 | - | - | - |
| varispeed (today) | 56/56 | 0.7 / 9.0 / 16.0 | 10.5 / 27.0 (0.424) | 4.7 | 4471 |
| slice | 56/56 | 2.7 / 8.0 / 9.0 | 0.0 / 0.0 (1.000) | 18.4 | 1130 |
| wsola | 56/56 | 0.4 / 3.0 / 11.0 | 0.0 / 0.0 (1.000) | 108.9 | 191 |
| phase vocoder | 56/56 | 0.6 / 6.0 / 19.0 | 0.0 / 0.0 (1.000) | 175.3 | 119 |

### 7. Drums B to drum loop A's groove averaged into a one-bar template

56 anchors, 56 pulled to a slot, 58 warp points. True onsets against where they should land, before: mean 13.6 ms, p95 34.7 ms, max 42.6 ms (64); after the warp map: mean 2.1 ms, p95 7.6 ms, max 26.6 ms (64); of those, the onsets with an anchor: mean 0.9 ms, p95 4.1 ms, max 7.6 ms (56), the ones without: mean 10.4 ms, p95 26.6 ms, max 26.6 ms (8).

| Method | Onsets found in the output | Off where the map puts them: mean / p95 / max ms | Attacks matched at hard anchors: off mean / max ms, correlation | CPU ns per sample | x real time |
| --- | --- | --- | --- | --- | --- |
| input, not quantized | 56/56 | 14.9 / 36.2 / 43.2 | - | - | - |
| varispeed (today) | 56/56 | 1.0 / 7.3 / 11.2 | 10.0 / 26.8 (0.411) | 4.6 | 4547 |
| slice | 56/56 | 2.9 / 8.2 / 10.3 | 0.0 / 0.0 (1.000) | 4.2 | 4943 |
| wsola | 56/56 | 1.4 / 11.2 / 20.2 | 0.0 / 0.0 (1.000) | 65.3 | 319 |
| phase vocoder | 56/56 | 0.9 / 3.5 / 19.2 | 0.0 / 0.0 (1.000) | 140.7 | 148 |

Following: distance from each true event's landing point to drum A's hit on the same slot.

| Follower | Before, mean / max ms | After (anchors moved), mean / max ms |
| --- | --- | --- |
| Sung line, hit by hit | 18.1 / 58.4 | 3.4 / 28.9 |
| Drums B, hit by hit | 15.1 / 45.2 | 0.9 / 8.1 |
| Drums B, one-bar template | 15.1 / 45.2 | 7.9 / 21.4 |

## The synth stays on the stretched voice

Case 4 with WSOLA: the loudness the synth reads from the stored frames through the warp map, against the loudness of the stretched audio re-analysed: best lag 0.1 ms (positive: the audio comes later), correlation 0.999 at lag 0.
Same check for varispeed: lag 0.2 ms, correlation 0.999.

## Determinism

Two whole renders; a render in calls of random size (1 to 3,000 samples); eight plays started from points spread over the take (a seek, as Play from a stop does), each rendered twice; and, for those plays, how long after the start point their output still differs from the whole render. "Equal from the next anchor" checks that every difference ends before the next warp point.

| Case | Method | Two renders equal | Call size changes nothing | Play from a point repeats exactly | Equal from the next anchor | Longest difference after a start point, ms |
| --- | --- | --- | --- | --- | --- | --- |
| 1 (drums) | varispeed (today) | yes | yes | yes | yes | 0.0 |
| 1 (drums) | slice | yes | yes | yes | yes | 0.0 |
| 1 (drums) | wsola | yes | yes | yes | yes | 229.1 |
| 1 (drums) | phase vocoder | yes | yes | yes | yes | 229.1 |
| 4 (sung) | varispeed (today) | yes | yes | yes | yes | 0.0 |
| 4 (sung) | slice | yes | yes | yes | yes | 0.0 |
| 4 (sung) | wsola | yes | yes | yes | yes | 1140.9 |
| 4 (sung) | phase vocoder | yes | yes | yes | yes | 1140.9 |

## Cost in the worklet's call size

| Take | Method | ns per sample, whole take in one call | ns per sample, 128-sample calls |
| --- | --- | --- | --- |
| drums A | slice | 4.6 | 4.6 |
| drums A | wsola | 57.0 | 57.0 |
| drums A | phase vocoder | 112.2 | 118.6 |
| sung | slice | 4.0 | 4.1 |
| sung | wsola | 66.5 | 65.8 |
| sung | phase vocoder | 105.3 | 103.4 |

Quantize plus warp map for the sung line following the drums: 1.3 us per run (2000 points over 100 runs).
