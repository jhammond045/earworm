<!-- Output of: cargo run --release --manifest-path experiments/flexquant/Cargo.toml. Run on Nov 22, 2026 on the build machine (Apple M4, 10 cores) with other work running; 1-minute load 5.58 when the run started. -->

# Flex quantize prototype results

Takes (48 kHz, 100 bpm, synthetic, so every onset's true time and grid slot is known): drum loop A, 4 bars of 16ths, played with 58% swing, the snare 12 ms late, 6 ms random timing and a 10 ms push and drag over the loop (10.4 s); drum loop B, the same pattern straight, the snare 8 ms early, 12 ms random timing (10.4 s); a sung line of 18 notes (detached, "da" and legato joins), 15 ms late on average with 20 ms random timing (10.5 s).

## Anchor accuracy

A found anchor matches a true onset within 50 ms, one to one. Errors are found minus true, in ms.

| Take and detector | True | Matched | Missed | Extra | Mean error | p95 | Max | Bias |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Drums A, audio onsets (second pass detector) | 64 | 46 | 18 | 0 | 1.6 | 7.0 | 8.3 | -1.6 |
| Drums A, second pass hits | 64 | 46 | 18 | 0 | 1.6 | 7.0 | 8.3 | -1.6 |
| Drums A, live frames' onset slot (10 ms) | 64 | 43 | 21 | 0 | 15.9 | 19.6 | 21.5 | 15.9 |
| Drums A, live frames refined on the audio | 64 | 43 | 21 | 0 | 1.9 | 7.9 | 8.5 | -1.9 |
| Drums A, kick and snare only, audio onsets | 20 | 20 | 0 | 26 | 2.3 | 7.0 | 7.9 | -2.3 |
| Drums A, audio onsets plus a 4 kHz high band pass | 64 | 58 | 6 | 0 | 1.4 | 7.0 | 8.3 | -1.4 |
| Drums B, audio onsets | 64 | 44 | 20 | 0 | 1.0 | 3.9 | 7.3 | -1.0 |
| Drums B, audio onsets plus a 4 kHz high band pass | 64 | 56 | 8 | 0 | 0.9 | 3.9 | 7.3 | -0.9 |
| Sung, audio onsets | 18 | 14 | 4 | 0 | 1.3 | 5.0 | 5.0 | -0.4 |
| Sung, second pass note starts | 18 | 18 | 0 | 0 | 7.2 | 30.6 | 30.6 | 5.9 |
| Sung, live frames' onset slot (10 ms) | 18 | 5 | 13 | 0 | 17.5 | 22.7 | 22.7 | 17.5 |
| Sung, live frames refined on the audio | 18 | 5 | 13 | 0 | 1.2 | 1.7 | 1.7 | 0.7 |
| Sung, legato joins only, second pass | 4 | 4 | 0 | 14 | 28.0 | 30.6 | 30.6 | 28.0 |
| Sung, second pass with legato joins moved to the glide's midpoint | 18 | 18 | 0 | 0 | 3.3 | 26.7 | 26.7 | 2.0 |
| Sung, legato joins only, moved to the glide's midpoint | 4 | 4 | 0 | 14 | 10.6 | 26.7 | 26.7 | 10.6 |

Detection cost, single core: audio onsets 0.9 ms per second of audio; the whole second pass 22.1 ms (drums) and 27.9 ms (sung) per second of audio. It already runs in a worker after every take.

## Quantizing and stretching

Each case quantizes the anchors, builds the warp map and plays the take through it four ways. The line above each table says how close the warp map itself puts every true onset to where it should land: that is the anchors' accuracy, whatever plays the audio. The table then checks the audio: it finds the onsets in the output with the same detector that found the anchors and compares them with where the map puts the onsets that detector finds in the input (within 50 ms), so the detector's own bias cancels. Spectral distance compares third-octave band levels with the take rendered with every event exactly on the grid (lower is closer; the input row is what the timing alone costs). Pitch error is the median YIN pitch 60 to 210 ms into each note against the sung pitch. Attack rows compare the 10 ms after each kick and snare, and the 15 to 2 ms before it, with the ideal render: smeared or doubled attacks show as a lower peak and more level before the attack. CPU is the best of five whole-take renders on one core of the build machine.

### 1. Drums A to straight 16ths, full strength

58 anchors, 58 pulled to a slot, 60 warp points. True onsets against where they should land, before: mean 27.0 ms, p95 61.1 ms, max 71.0 ms (64); after the warp map: mean 5.3 ms, p95 42.2 ms, max 53.5 ms (64); of those, the onsets with an anchor: mean 1.2 ms, p95 6.7 ms, max 7.2 ms (58), the ones without: mean 44.3 ms, p95 53.5 ms, max 53.5 ms (6).

| Method | Onsets found in the output | Off where the map puts them: mean / p95 / max ms | Spectral distance to ideal, dB | Attack peak vs ideal, mean / worst dB | Before the attack vs ideal, mean dB | CPU ns per sample | x real time |
| --- | --- | --- | --- | --- | --- | --- | --- |
| input, not quantized | 48/58 | 18.3 / 48.0 / 49.0 | 16.08 | 10.17 / 66.62 | +34.2 | - | - |
| varispeed (today) | 58/58 | 0.3 / 2.0 / 6.0 | 18.23 | 1.37 / 2.86 | +11.5 | 4.6 | 4519 |
| slice | 56/58 | 1.9 / 5.0 / 5.0 | 4.99 | 0.06 / 0.30 | -5.8 | 4.6 | 4544 |
| wsola | 58/58 | 1.9 / 14.0 / 26.0 | 7.47 | 0.06 / 0.30 | +16.2 | 93.9 | 222 |
| phase vocoder | 58/58 | 1.5 / 11.0 / 22.0 | 7.72 | 0.06 / 0.30 | +16.8 | 152.3 | 137 |

### 2. Drums A to 16ths with 58% swing (the feel it was played with)

58 anchors, 58 pulled to a slot, 60 warp points. True onsets against where they should land, before: mean 15.0 ms, p95 37.1 ms, max 47.0 ms (64); after the warp map: mean 3.1 ms, p95 18.2 ms, max 29.5 ms (64); of those, the onsets with an anchor: mean 1.3 ms, p95 6.7 ms, max 7.7 ms (58), the ones without: mean 20.3 ms, p95 29.5 ms, max 29.5 ms (6).

| Method | Onsets found in the output | Off where the map puts them: mean / p95 / max ms | CPU ns per sample | x real time |
| --- | --- | --- | --- | --- |
| input, not quantized | 58/58 | 14.3 / 37.0 / 47.0 | - | - |
| varispeed (today) | 58/58 | 0.6 / 1.0 / 13.0 | 6.3 | 3311 |
| slice | 56/58 | 1.8 / 5.0 / 5.0 | 5.6 | 3688 |
| wsola | 58/58 | 0.1 / 0.0 / 4.0 | 62.7 | 332 |
| phase vocoder | 58/58 | 0.3 / 4.0 / 6.0 | 133.0 | 157 |

### 3. Drums A to straight 16ths, strength automated from 0 to 1 over the loop

58 anchors, 58 pulled to a slot, 60 warp points. True onsets against where they should land, before: mean 12.2 ms, p95 34.6 ms, max 48.8 ms (64); after the warp map: mean 3.1 ms, p95 24.3 ms, max 51.1 ms (64); of those, the onsets with an anchor: mean 0.7 ms, p95 5.0 ms, max 6.3 ms (58), the ones without: mean 27.0 ms, p95 51.1 ms, max 51.1 ms (6).

| Method | Onsets found in the output | Off where the map puts them: mean / p95 / max ms | CPU ns per sample | x real time |
| --- | --- | --- | --- | --- |
| input, not quantized | 58/58 | 10.8 / 34.1 / 48.2 | - | - |
| varispeed (today) | 58/58 | 0.9 / 5.8 / 13.5 | 5.8 | 3576 |
| slice | 56/58 | 1.9 / 5.5 / 5.7 | 5.3 | 3900 |
| wsola | 57/58 | 1.0 / 9.7 / 12.6 | 64.6 | 322 |
| phase vocoder | 57/58 | 1.1 / 6.8 / 18.9 | 127.5 | 163 |

Offset from the straight grid after quantizing, by bar, as a share of the offset before (the lane is at 0.125, 0.375, 0.625, 0.875 mid-bar): bar 1 0.87; bar 2 0.64; bar 3 0.38; bar 4 0.13;

### 4. Sung line to straight 8ths, full strength

18 anchors, 18 pulled to a slot, 20 warp points. True onsets against where they should land, before: mean 14.8 ms, p95 50.4 ms, max 50.4 ms (18); after the warp map: mean 3.3 ms, p95 27.4 ms, max 27.4 ms (18).

| Method | Onsets found in the output | Off where the map puts them: mean / p95 / max ms | Spectral distance to ideal, dB | Pitch error: mean / max cents | CPU ns per sample | x real time |
| --- | --- | --- | --- | --- | --- | --- |
| input, not quantized | 17/18 | 12.9 / 41.0 / 41.0 | 2.99 | 0.1 / 0.3 | - | - |
| varispeed (today) | 16/18 | 0.6 / 4.6 / 4.6 | 4.01 | 67.5 / 208.2 | 4.3 | 4799 |
| slice | 18/18 | 7.2 / 50.0 / 50.0 | 1.57 | 0.3 / 3.8 | 4.5 | 4668 |
| wsola | 18/18 | 1.0 / 11.0 / 11.0 | 2.36 | 0.3 / 2.1 | 69.3 | 300 |
| phase vocoder | 18/18 | 0.1 / 0.8 / 0.8 | 1.96 | 0.3 / 3.3 | 108.4 | 192 |

Level at the four legato joins of case 4 (dB below the level around them; the joins carry no level change, so closer to the input is better):

| Join | Input | WSOLA, legato anchors soft | Phase vocoder, soft | WSOLA, every anchor hard | Phase vocoder, every anchor hard |
| --- | --- | --- | --- | --- | --- |
| beat 1.5 | -2.2 | -2.1 | -2.2 | -2.0 | -2.0 |
| beat 5 | -2.1 | -1.9 | -1.8 | -4.2 | -5.7 |
| beat 10 | -1.3 | -1.3 | -1.3 | -3.5 | -10.8 |
| beat 13 | -2.4 | -2.4 | -2.5 | -2.3 | -6.0 |

### 5. Sung line to drum loop A's groove, hit by hit (8ths)

18 anchors, 18 pulled to a slot, 20 warp points. True onsets against where they should land, before: mean 18.1 ms, p95 58.4 ms, max 58.4 ms (18); after the warp map: mean 3.4 ms, p95 28.9 ms, max 28.9 ms (18).

| Method | Onsets found in the output | Off where the map puts them: mean / p95 / max ms | Pitch error: mean / max cents | CPU ns per sample | x real time |
| --- | --- | --- | --- | --- | --- |
| input, not quantized | 17/18 | 15.4 / 49.0 / 49.0 | 0.1 / 0.2 | - | - |
| varispeed (today) | 15/18 | 2.9 / 37.0 / 37.0 | 101.0 / 247.4 | 4.2 | 5008 |
| slice | 18/18 | 11.9 / 48.0 / 48.0 | 0.5 / 4.7 | 4.4 | 4735 |
| wsola | 18/18 | 0.9 / 13.0 / 13.0 | 0.2 / 0.5 | 69.0 | 302 |
| phase vocoder | 18/18 | 0.3 / 3.0 / 3.0 | 0.3 / 2.2 | 106.9 | 195 |

### 6. Drums B to drum loop A's groove, hit by hit (16ths)

56 anchors, 56 pulled to a slot, 58 warp points. True onsets against where they should land, before: mean 24.5 ms, p95 65.2 ms, max 69.2 ms (64); after the warp map: mean 3.0 ms, p95 20.7 ms, max 58.7 ms (64); of those, the onsets with an anchor: mean 1.0 ms, p95 4.3 ms, max 9.1 ms (56), the ones without: mean 17.3 ms, p95 58.7 ms, max 58.7 ms (8).

| Method | Onsets found in the output | Off where the map puts them: mean / p95 / max ms | CPU ns per sample | x real time |
| --- | --- | --- | --- | --- |
| input, not quantized | 45/56 | 17.4 / 44.0 / 46.0 | - | - |
| varispeed (today) | 56/56 | 0.4 / 4.0 / 8.0 | 4.6 | 4490 |
| slice | 56/56 | 1.5 / 5.0 / 8.0 | 4.5 | 4588 |
| wsola | 56/56 | 1.1 / 10.0 / 29.0 | 59.0 | 353 |
| phase vocoder | 56/56 | 1.9 / 18.0 / 19.0 | 105.7 | 197 |

### 7. Drums B to drum loop A's groove averaged into a one-bar template

56 anchors, 56 pulled to a slot, 58 warp points. True onsets against where they should land, before: mean 29.1 ms, p95 65.1 ms, max 81.2 ms (64); after the warp map: mean 8.2 ms, p95 60.9 ms, max 88.5 ms (64); of those, the onsets with an anchor: mean 1.0 ms, p95 4.6 ms, max 8.6 ms (56), the ones without: mean 59.1 ms, p95 88.5 ms, max 88.5 ms (8).

| Method | Onsets found in the output | Off where the map puts them: mean / p95 / max ms | CPU ns per sample | x real time |
| --- | --- | --- | --- | --- |
| input, not quantized | 48/56 | 19.3 / 48.0 / 49.5 | - | - |
| varispeed (today) | 56/56 | 0.8 / 6.2 / 9.3 | 4.8 | 4337 |
| slice | 56/56 | 2.1 / 7.2 / 9.5 | 4.4 | 4772 |
| wsola | 56/56 | 2.2 / 17.5 / 29.0 | 56.0 | 372 |
| phase vocoder | 56/56 | 1.0 / 5.5 / 14.3 | 102.0 | 204 |

Following: distance from each true event's landing point to drum A's hit on the same slot.

| Follower | Before, mean / max ms | After (anchors moved), mean / max ms |
| --- | --- | --- |
| Sung line, hit by hit | 18.1 / 58.4 | 3.4 / 28.9 |
| Drums B, hit by hit | 26.0 / 69.2 | 2.4 / 58.7 |
| Drums B, one-bar template | 26.0 / 69.2 | 9.4 / 66.6 |

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
| drums A | slice | 4.0 | 4.1 |
| drums A | wsola | 55.0 | 54.9 |
| drums A | phase vocoder | 103.6 | 105.6 |
| sung | slice | 3.9 | 3.9 |
| sung | wsola | 62.5 | 62.4 |
| sung | phase vocoder | 97.2 | 99.3 |

Quantize plus warp map for the sung line following the drums: 1.0 us per run (2000 points over 100 runs).
