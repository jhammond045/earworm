# Meter remap: playing beats in a new order

Nov 22, 2026. Design, approved by the owner with every recommended answer; stage R1 is built ([R1.md](R1.md)). A throwaway prototype in `experiments/meterremap` measured the parts that carry risk; its full output is [experiments/meterremap/results.md](../experiments/meterremap/results.md), and "Prototype" below summarizes it.

The owner asked whether a take could change meter: a 4/4 groove played in 3/4 by dropping a beat from every bar, a 3/4 one played in 4/4 by repeating one, a 5/4 one in 4/4 (the well-known edits of Take Five, which is in 5/4), and whether the beats of a bar could be shuffled (1 2 3 4 played as 1 3 4 2). Any track should be able to provide the bars for others, any number of these meters should exist, and a track can lock to one of them or stay free.

This page makes that an extension of Feel ([quant.md](quant.md)). Feel bends a clip's timing within the beat; a **meter** cuts the clip into beat slices along its bars and plays them in the order a **pattern** gives. Notes, curves and the take's audio all go through it, at play time, and the take and its notes never change.

## Summary

| Concern | Decision |
| --- | --- |
| Is it worth building? | Yes for grooves whose beats stand on their own (drums, bass, comping), with a caveat the owner should hear first: where a dropped beat's sound rings on into a kept one (a held chord, a ride, the room), its tail plays without its attack. See "Where it sounds bad". |
| What a meter is | Bars over the section's beats: how many beats a bar has and where bar 1 is. **Typed** (4 per bar, from each section's start) or **from a layer** (that layer's take's bars, detected and then confirmed by ear). Any number of meters; the first one sets each section's bars. |
| Locking | Each clip is locked to one meter or free. The track header sets the lock for all of a track's clips at once. A locked clip plays its meter's pattern; a free clip plays as written. |
| The pattern | Per section and meter: for each beat of the new bar, which beat of the source bar plays there. `1 2 3` drops beat 4, `1 2 3 3` repeats beat 3, `1 3 4 2` shuffles. Missing means as written. |
| Where it happens | At compile time, after Feel. The compiler turns each locked clip into runs: stretches of the source that follow on, each moved by a whole number of beats. Notes, hits, the warp map the curves are read through and the take audio's stretch map are built from the runs. |
| Cuts | At the first hard anchor near each beat (within an eighth of a beat, 60 ms at most), so a hit played early stays with its beat; at the beat itself where a clip has no anchor there. The join is the engine's existing 5 ms crossfade before the anchor. |
| The time map | No longer increasing: it jumps back for a repeat or a shuffle and forward for a drop. It is built and read only in the direction "played time to source time", which stays a function; nothing inverts it. |
| Engine | No change. The v5 track blob already carries runs in the stretch map and allows two warp anchors at one clip time; the prototype played every case through `klont::stretch::Stretcher` unmodified. |
| Determinism | Kept: two renders bit-identical, call sizes change nothing, play from any point equals a whole render from the next slice on (hits mode, measured). |
| Bars from audio | A suggestion only. A hand-made detector finds beats per bar in 89% of synthetic grooves and 77% of 1,733 real loops, but the downbeat in only 53% of the real loops (chance is 25%). Bars default to typed, and the user confirms a layer's bars by ear. |
| Cost | Hits-mode playback of a remapped take costs what the same take as one run costs, within the noise (7.7 against 7.5 ns per sample). Building the runs: microseconds per clip. Bar detection: 4 to 7 ms per second of audio, once per take. |

What does not change: Feel and its anchors; the take, its frames and its notes; one engine binary for live and export; the track blob format; commands and undo; no allocation, locks or waits on the audio thread.

## What exists today

| Piece | Where | Today |
| --- | --- | --- |
| Bars | `metro.bar` (the click's accent, project-wide, default 4); `gridLines` in `web/src/m3/geometry.ts` draws a strong line every 4 beats; Snap's "Bar" is 4 beats | No meter in the document. A section has a length in beats and a tempo |
| Feel | `Clip.flex`, compiled by `flexClip` and `stretchMap` in `web/src/project/flex.ts` (Q1, Q2) | Anchors pulled toward a grid through an increasing time map |
| Curves | `curve::warp_to_take` in `crates/klont/src/curve.rs` | Reads the warp map at a clip time; the blob checks only that clip times do not decrease, so two anchors may share one |
| Take audio | `Stretcher` in `crates/klont/src/stretch.rs` (Q2) | Plays runs of map points; a run point starts a new run anywhere in the take (Q2 uses it where a take wraps the loop end) |
| Tempo | `analysis::tempo::find_tempo` | Proposes a tempo and a beat phase for a take. Nothing finds bars |

## The model

### Terms

- **Meter.** Bars over a section's beats: beats per bar and where bar 1 falls. Named Meter 1, Meter 2, and so on. **Meter 1** is the primary one: it sets each section's bars, the grid's bar lines and the click's accent.
- **Source.** Where a meter's bars come from: typed, or a layer's take.
- **Pattern.** For one section and one meter: the source beats, 1-based, that the new bar plays, in order. Its length is the new beats per bar.
- **Lock.** A clip locked to a meter is cut into beat slices along that meter's bars and played by the section's pattern. A **free** clip has no meter.
- **Run.** Output beats whose source beats follow on unbroken. Each run plays one stretch of the source, moved by a whole number of beats.
- **Join.** Where one run ends and the next starts: the only places anything is cut.

```text
 source bar     | 1     | 2     | 3     | 4     |      a 4/4 groove, cut at the first hard anchor
                  K h S   h K S   K h     S h K        near each beat
 pattern 1 2 3  | 1     | 2     | 3     ‖ 1'    | ...  one run per bar: beats 1 to 3 play as they
                                         ↑               were, then the next bar's beat 1
                                         join: beat 4 is never played

 pattern 1 3 4 2| 1     ‖ 3     | 4     ‖ 2     ‖ 1'   three runs per bar, three joins
```

### Meters and their bars

| Source | Beats per bar | Bar 1 | Use |
| --- | --- | --- | --- |
| Typed | Typed (2 to 16) | Each section's start | The default. Takes recorded after the count-in start on bar 1, and Q2's import puts a file's start at the section start |
| A layer | Found on that layer's take, or typed there | Found on the take, or dragged in the timeline | A take whose bars are not the section's: a 5/4 loop imported into a 4/4 song, a take with a pickup |

- **Stored with the take.** A layer meter reads the bars of whichever take that layer's clip plays in each section. The analysis worker proposes them once per take, after the second pass: beats per bar and the take second of a downbeat, stored on the take with a flag that says they were found, not set. The user confirms or corrects them (type the beats per bar, drag bar 1), and that edit is stored on the take. A later detector never changes them.
- **As heard after Feel.** The bars are placed through the clip's placement and its Feel map, so a meter from a tightened drum take counts the tightened drums. Where the source layer has no clip in a section, the meter has no bars there, and its locked clips play as written in that section.
- **Any number.** Meters are a list in the project, like Q3's grooves. A typed meter and a layer meter can coexist; two meters can name the same layer (the same bars, different patterns: the bass plays `1 2 3`, the shaker `1 3 2`).

### Patterns

A pattern is a list of source beats. Every remap the owner described is one:

| Want | Source | Pattern |
| --- | --- | --- |
| 4/4 to 3/4, drop the last beat | 4 | `1 2 3` |
| 5/4 to 4/4 | 5 | `1 2 3 4`, or `1 2 3 5` |
| 3/4 to 4/4, repeat a beat | 3 | `1 2 3 3`, or `1 1 2 3` |
| 4/4 to 5/4 | 4 | `1 2 3 4 4` |
| Shuffle within the bar | 4 | `1 3 4 2` |
| As written | any | missing |

- Whole beats only. Eighths (`1 1& 2 ...`) are a small step later (question 7).
- The pattern applies bar by bar: output bar *i* plays the pattern over source bar *i*. A clip's last part bar plays the pattern's beats that exist in it.
- The beat length stays the section's: a 4/4 bar becomes a shorter 3/4 bar at the same tempo.
- Patterns belong to the section, one per meter, so a chorus can drop a beat while the verse plays as written. "Copy to all sections" copies one, as Feel's row does.

### Locking

- A clip's lock is stored on the clip, as its Feel is. The track header shows and sets the lock for all of a track's clips: "Bass: Meter 1". When a track's clips differ (one recorded in the new meter, below), it says so.
- **Locked to Meter 1, pattern `1 2 3`.** The clip plays as above. Its loop becomes its source bars times 3 beats.
- **Free.** Plays as written, at its own loop length. In a section that Meter 1 shortens, a free clip loops as before and is cut at the section's end, as a loop longer than its section is today.
- **Meter 1 sets the section's played length.** The section's length stays as typed, in written beats; while Meter 1 plays a pattern in it, the compiler plays it as its written bars times the pattern's length (4 bars of 4/4 at `1 2 3` play as 12 beats), and a part bar at the end plays the pattern's beats that exist in it. Nothing in the document changes, so removing the pattern gives the written length back. Other meters do not change the length: their clips loop inside the section, which is how polymeter happens (a 3-beat shaker over a 4-beat groove).

### What moves, and how

The compiler builds the runs for each locked clip after its Feel map:

1. **Beats.** The meter's bar lines in the section, and its beats between them, in played time (after Feel).
2. **Cuts.** For each beat that starts a run or ends one, the earliest hard anchor of this clip within an eighth of a beat of it (60 ms at most) that weighs at least 0.3 of the heaviest there. Without one, the beat itself. Each locked clip cuts at its own anchors, so the bass cuts at its own note starts and the drums at their hits.
3. **Runs.** Output beats are grouped into runs; each run is the played clip between its two cuts, moved by a whole number of beats. Where a run's start would fall before the end of the run before it (a late hit after an early one), the earlier run ends there instead.

Everything the clip plays comes from the runs:

| What | How |
| --- | --- |
| Notes and hits | A note belongs to the slice its played start falls in, so a note played a few ms before its beat moves with that beat. It is copied into every run that plays its slice (twice in a repeat), dropped with a dropped beat, and ends where its run ends. A held note that a run starts in the middle of (its start is in a beat the run does not play) starts again at the run's start, if a quarter beat or more of it is left (question 3) |
| Curves | The warp map is rebuilt per run: each run's anchors moved by its shift, and at each join two anchors at the same clip time, the run ending and the run starting. Voices and routes read the take's frames where that slice was sung. A repeated note reads its frames twice |
| Polyphonic takes | Each note's curve is read from its own start ([poly.md](poly.md) P3), so a copied note reads its whole curve again; a note that starts again mid-way re-reads its curve from its start |
| Take audio | The stretch map is the Feel stretch map cut into the runs: a run point at each run's start, hard, plus the hard and soft anchors inside the run. The engine's stretcher plays each run with its 5 ms pre-roll fading in under the 5 ms fade-out of the run before. Hits mode is the default for a remap without Feel: each slice is copied as recorded |
| Lanes and morph markers | Stay in the clip's played time, like Feel leaves them on the grid. A lane point past a shortened loop's end does not play, and comes back if the pattern does (question 4) |
| The grid | The timeline draws Meter 1's played bars (3 beats each after `1 2 3`), the click accents them, and Snap's "Bar" is that long. A clip locked to another meter shows that meter's bar lines faintly on its row |

### Why a jump is safe

Feel's time map runs from performed time to played time and only ever increases (Q1 checks it). A remap is different: played time maps to source time by a shift per run, so the map steps back for a repeat or a shuffle and forward for a drop. Its inverse is not a function: a repeated beat plays twice, a dropped one never.

Nothing needs the inverse. The compiler builds every output by walking the output runs in order and asking, for each, which stretch of the source it plays. The engine only ever asks the same question, "at this clip time, which take second?": the warp map (`warp_to_take` finds the last anchor at or before a clip time, so at a join it reads the run that starts there) and the stretch map (each run is rendered on its own from its own start). Both are functions of played time, sorted by played time, as the blob requires. (Every run sends at least its start and end anchors, so a join is always two anchors with a segment after it.) Recording goes the same way: a note played into a remapped clip is stored at the source time the map gives for when it was played, and a mic take is not remapped at all (below).

### Feel, followers and order

```text
  1. Feel: every clip's time map (sources before their followers, Q3)
  2. Bars: each meter's bars in each section, read from its source as heard after step 1
  3. Remap: each locked clip cut into runs along its meter's bars and laid out by the pattern
```

- **Feel first.** Feel pulls anchors to the grid inside each beat; the remap then moves whole beats. With Feel on at full strength the cuts fall on the grid; with Feel off they fall at the played hits.
- **Followers follow the written order.** A Feel follower reads its source's anchors as heard after the source's Feel and before any remap. A follower locked to the same meter as its source is remapped the same way and stays with it. A free follower of a locked source would follow timing the source no longer plays in that order, so the Feel row offers to lock the follower to the source's meter.
- **No cycles can form.** A meter reads its source's bars before any remap, and a remap never feeds Feel or bars, so every dependency points from step 1 to step 2 to step 3. A layer can provide Meter 1's bars and be locked to Meter 2. Cycles would only become possible if a meter could count the bars another meter's remap produces ("Meter 2 counts Meter 1's 3/4 bars"). This design does not offer that; if it is ever added, the compiler orders meters and refuses a cycle with a message and no change to the document, as Q3 does for followers.

### Recording

- A pass (S5) records raw, as today, and the monitor plays what the meters play.
- **Notes into a remapped clip** (MIDI, on-screen keys) were played against the remapped playback, so they are written through the played-to-source direction of the map: each lands in the source beat it was played over, and plays wherever that beat plays.
- **A mic take into a remapped clip** was performed in the new meter, so remapping it again would scramble it. Arming a layer whose clip its meter remaps asks first: "Record in the new meter" unlocks that clip (in the same undo step as the pass), and the take then plays as recorded. The track header then shows the track's clips as mixed.

### Bars from audio

The prototype's detector takes the beats as given (the section grid, or a loop's own tempo) and only groups them. Per beat it measures onset strength in a low, a middle and a high band at four points through the beat, and the pitch classes sounding through it.

- **Beats per bar** is the lag, 2 to 7 beats, at which beats look most alike. A pattern that repeats every 2 beats is read as 4.
- **The downbeat** is the beat position that most often has a low-band onset, a change of harmony from the beat before, a strong onset overall, and the largest change of harmony between bar-long windows starting there.

It runs in the analysis worker after the second pass, 4 to 7 ms per second of audio depending on the machine's load, and its answer is a suggestion with the timeline showing where it put bar 1. The numbers below are why it is not trusted on its own.

## Prototype

`cargo run --release --manifest-path experiments/meterremap/Cargo.toml` prints [results.md](../experiments/meterremap/results.md). `METERREMAP_WAV=<dir>` writes every source, render and ideal as a WAV file, and `METERREMAP_EXAMPLES=docs/meter-examples` writes the short set kept in the repo. `METERREMAP_LOOPS=<dir>` adds real recordings: `python3 -I experiments/meterremap/loops.py "/Library/Audio/Apple Loops/Apple" <dir>` decodes the Apple Loops that GarageBand installs (1,738 of them in 4/4, each starting on its downbeat) with their metadata. The loops are not in the repository and must not be; their renders stay outside it too. The crate has its own workspace, so `pnpm test` does not build it.

- **Grooves.** Synthetic, so every event's beat is known: a 5/4 jazz figure at 160 bpm grouped 3 + 2 (ride, kick, rim, walking bass, comping chords), a 4/4 groove at 100 bpm (kick, late snare, hats, bass, held electric-piano chords), and a 3/4 waltz at 144 bpm, each 4 bars with 5 ms of timing spread and a small room. Rendered as stems (drums, bass, keys) that sum to the mix.
- **Ideal.** The same events re-sequenced by the pattern and rendered, each keeping its played offset, a held note released where the new order breaks it, the room running on: what the band would play in the new meter.
- **Renders.** The engine's stretcher (hits mode, unchanged) on the mix as one take, cut at its anchors; the same on each stem cut at its own anchors, summed (three layers locked to one meter); the mix cut at the exact beat times; and a plain slicer with a 30 ms equal-power crossfade where a stem has no anchor to cut at.
- **Real recordings.** All 1,733 4/4 Apple Loops with whole bars for bar detection, alone and as 208 bands (a drum, a bass and a keys or guitar loop of one genre and tempo, summed). One band remapped: drums, a synth bass and a synth lead from the Disco Funk set at 116 bpm.

### Results

Spectral distance to the ideal (RMS difference of third-octave band levels, as quant.md measures it; 0 is identical), over the whole render and over the 10 ms before to 150 ms after each join. Clicks: the high band in the 8 ms before each join against the ideal's, dB. Engine on stems, which is how locked layers would play:

| Case | Pattern | Joins per bar | Distance, all / near joins | Clicks, mean / worst |
| --- | --- | --- | --- | --- |
| 5/4 to 4/4, drop beat 5 | `1 2 3 4` | 1 | 0.64 / 2.19 dB | -7.9 / +0.0 dB |
| 5/4 to 4/4, drop beat 4 | `1 2 3 5` | 1 | 3.62 / 9.50 dB | -1.3 / +2.9 dB |
| 4/4 to 3/4 | `1 2 3` | 1 | 0.39 / 1.27 dB | -6.9 / -0.1 dB |
| 3/4 to 4/4, repeat beat 3 | `1 2 3 3` | 1 | 0.22 / 0.46 dB | -0.2 / +0.0 dB |
| 3/4 to 4/4, repeat beat 1 | `1 1 2 3` | 1 | 0.57 / 1.58 dB | -5.4 / -0.1 dB |
| 4/4 shuffled | `1 3 4 2` | 3 | 2.48 / 3.49 dB | -3.0 / +1.7 dB |

- **Cut at anchors, not at the beat.** Cutting at the exact beat times split every hit played a few ms early: worst clicks of +24 to +43 dB over the ideal in five of the six cases, against +3 dB at worst when cut at the anchors.
- **Stems or one take.** Cutting each stem at its own anchors measured a little closer than cutting the mix at the mix's anchors (for example 0.39 against 0.41 dB for 4/4 to 3/4, clicks -6.9 against -3.4 dB). A whole song imported as one take works nearly as well.
- **Longer crossfades do not help.** The 30 ms equal-power crossfade where a stem had no anchor measured the same or slightly worse than the engine's 5 ms (shuffle: 2.56 against 2.48 dB), so the engine keeps its crossfade.
- **Real loops.** On the real band, the high band in the 8 ms before each join was 11 to 27 dB below the same before the source's own beats on average, and at worst 0.4 dB below (`1 2 3 4 4`): no join is spikier than the take's own beat starts. Apple loops are tightly quantized, so the beat-time cut does less harm there (worst +0.3 dB).
- **The result reads as the new meter.** The bar detector, run on each remapped render, found the new beats per bar in all six cases.

Bars from audio, beats given:

| Takes | Beats per bar right | Downbeat right, beats per bar typed |
| --- | --- | --- |
| 200 random synthetic grooves, 3 to 7 beats per bar, a random pickup | 89% (10% half or double: 3 read as 6) | 90% (drums alone 72%) |
| 1,733 real 4/4 loops, a random pickup | 77% (drums 94%, others 67 to 70%) | 53% |
| 208 real bands of three loops | 89% | 54% |

Chance for the downbeat in 4/4 is 25%. The misses on real loops are spread over every offset, not just half a bar out, and no weighting of the cues tried did better than 55% (results.md lists them). A trained downbeat model would do better; that is a project of its own (question 5).

Determinism and cost, on the build machine (Apple M4) with other work running:

- **Hits mode:** two renders bit-identical; 128-sample calls and random calls of 1 to 3,000 samples bit-identical; play from eight points equals the whole render from the next slice on. **Smooth mode:** the same, except that after a start point it differs until the next hard point, up to 583 ms here, because the prototype's runs have no hard anchors inside them; with Feel's anchors in the run, as the design sends it, that is the Q2 behaviour.
- **CPU:** 7.7 ns per output sample for a remapped take in hits mode, 7.5 for the same take as one run; smooth 123 against 117. Repeated runs moved these by a nanosecond or more either way (smooth by 10 or more) with other work on the machine, so the remap adds no cost that can be measured here. Building the runs from beats and anchors: 6 us for 8 bars in Rust (the TypeScript compiler will be slower, as Q1's map was: 176 us per clip).

### Where it sounds bad

I could not listen to these; everything here is from the measurements. The owner should listen before deciding.

- **A dropped beat that rings on.** Dropping beat 4 of the 5/4 figure leaves the chord comped on beat 4 and the ride's ring playing into beat 5 with no attack in front of them: 3.6 dB from the ideal, against 0.6 for dropping beat 5. The take's audio holds everything that sounds in a beat, including tails of what came before, and slicing cannot separate them. Dropping the bar's last beat is cleaner because the next downbeat's hit covers the join. Locked synth layers driven by notes do not have this problem: they replay the notes, and their tails ring naturally.
- **Shuffles cut held sounds three times a bar.** Over 4 bars, `1 3 4 2` on the 4/4 groove cuts four held bass and keys notes short and starts four slices in the middle of one: 2.5 dB from the ideal.
- **Sung and legato material** was not tested. A legato line cut at a beat jumps mid-phrase; expect it to sound worse than the comping here.
- **No real 5/4 or 3/4 recordings.** The real loops are all 4/4. The 5/4 and 3/4 grooves are synthetic and cleaner than a real band.
- **Downbeat detection** is right about half the time on real loops, so it can only suggest.

### Listening

All mono 48 kHz, 4 bars, in `docs/meter-examples/`; each groove's files share one gain, so they can be compared by ear. Listen in this order:

1. **`four-source.wav`, then `four-drop4-123.wav`.** 4/4 to 3/4: the clearest test of whether this is useful.
2. **`five-source.wav`, then `five-drop5-1234.wav` and `five-drop4-1235.wav`.** The 5/4 figure in 4/4 two ways. The second is the ringing case above: listen for the chord and the ride on beat 5 with no attack.
3. **`three-source.wav`, then `three-repeat3-1233.wav`.** 3/4 to 4/4 by repeating a beat.
4. **`four-shuffle-1342.wav`.** The shuffle.

The full set is outside the repo, in `/Users/jhammond/firstmate/data/earworm-meter-remap/wav/` on this machine: every render of every case, the ideal (`*-9-ideal.wav`), the beat-time cut (`*-3-grid-cut.wav`, to hear why cuts go at anchors), and the real band (`real-0-source.wav`, then `real-drop4-2-engine-stems.wav`, `real-shuffle-2-engine-stems.wav`, `real-repeat4-2-engine-stems.wav`).

## How it fits the rest

### The document

Schema 6 adds optional fields; a document without them plays exactly as today.

```ts
interface Project {
  // ...unchanged
  meters?: Meter[];            // missing: one typed meter of 4 beats, as today's grid
}
interface Meter {
  id: string;
  name: string;                // "Meter 1"; the first one sets each section's bars
  bars:
    | { from: 'typed'; beats: number }          // bars of `beats` from each section's start
    | { from: 'layer'; layer: string };         // the bars of that layer's take, per section
}
interface Take {
  // ...unchanged
  bars?: { beats: number; downbeat: number; found: boolean };  // downbeat in take seconds;
                                                               // found: the detector's guess
}
interface Section {
  // ...unchanged
  play?: Record<string, number[]>;   // by meter id: source beats (1-based) in play order
}
interface ClipBase {
  // ...unchanged
  meter?: string;              // the meter this clip is locked to; missing: free
}
```

- **Commands.** `setMeter` (add, rename, set its source; removing a meter frees its clips), `setMeterPlay` (a section's pattern), `lockClips` (one clip or all of a track's), `setTakeBars` (confirm or correct a take's bars). Each is one undo step.
- **The click.** Accents Meter 1's played bars. Its own bar setting (`metro.bar`) becomes Meter 1's typed beats per bar (question 6).

### Scenes, states and the matrix

A meter is arrangement, not sound: it is not part of a state, scenes do not change it, and the pattern is not a route target. As with Feel's strength, a per-tick pattern would mean re-ordering the track inside the engine while it plays.

### The compiler and the engine

| Work | Runs in | Thread |
| --- | --- | --- |
| Bar detection, once per take | Rust, the analysis worker, after the second pass | Worker |
| Bars per meter and section, runs, notes, warp and stretch maps | TypeScript, the compiler, after Feel | Main |
| Playing notes, reading curves, stretching audio | Rust, unchanged | Audio |

- **Remapped clips with take audio send a stretch map** even without Feel (today only a flexed clip does), in hits mode by default. The blob encoder already writes v5 only when a clip stretches.
- **Map size.** A run adds two points; 8 bars shuffled `1 3 4 2` add about 50, well inside the 1,024 a clip's map holds.
- **Export and live playback** run the same compiled track through the same engine, so a remap is heard live as it exports, and S5's live-against-export check holds as it does for Q2.

### Live versus render time

The mic and live MIDI are never remapped: a remap can play a beat before it has been played. Live, a locked layer that is listening plays the mic as it comes; playback plays its clip remapped.

## Interface sketch

A separate pass is reworking the interface now, so this stays at the level of concepts and controls. A Meter row sits with the Feel row for the selected clip, a lock sits on each track header, and the meters themselves are listed once for the project.

```text
┌ Timeline ─ Zoom [Fit ▾]  Snap [1/16 ▾]  Take [3 ▾] ──────────────────────────────────────────┐
│ Feel  [■ on]  Grid [1/16 ▾]  Feel [Swing ▾] 58%  Strength ━━━━━●━ 80%  ...                   │
│ Meter [Meter 1 ▾]  Bars: from Drums, 4 per bar (found; check by ear)                         │
│       Play [1][2][3] [+]   As written · Drop last · Repeat last · 1 3 4 2   [Copy to all]    │
│       Section: plays 4 bars of 3 (written 4 bars of 4)                                       │
├──────────────────────────────────────────────────────────────────────────────────────────────┤
│ Drums  [M1, bars]  ‖1  2  3 ‖1  2  3 ‖ ...  slices numbered by their source beat             │
│ Bass   [M1]        ‖1  2  3 ‖1  2  3 ‖                                                       │
│ Keys   [M1]        ‖1  2  3 ‖1  2  3 ‖                                                       │
│ Shaker [M2]        ┆1  3  2 ┆1  3  2 ┆      Meter 2: from Shaker, 3 per bar, play 1 3 2      │
│ Vocal  [free]      plays as written                                                          │
└──────────────────────────────────────────────────────────────────────────────────────────────┘
```

- **Meters list** (project level, beside the grooves Q3 adds): each meter's name, colour and source (typed beats per bar, or a layer). Adding one names it Meter *n*.
- **Track lock**: a chip per track that cycles through the meters and "free", in the meter's colour. "M1, bars" marks the track a meter reads its bars from.
- **Meter row**: the clip's meter, where its bars come from and whether they were found or set, and the pattern as beat chips: drag to reorder, click a chip to repeat it, remove it to drop the beat, or type `1 3 4 2`. Presets for the common ones. Copy to all sections.
- **Bars in the timeline**: bar 1 of a layer meter drawn on its source track and draggable to correct it; slice boundaries and source beat numbers on locked clips; dropped beats visible on demand.
- **Keyboard paths** for every control, and each edit is one undo step, as the rest of the timeline.

What the UI pass should leave room for:

1. A project-level list (meters, beside grooves) and a per-track chip that can name one of several items by colour.
2. A second row under or beside the Feel row, per selected clip, with an editable row of small numbered chips.
3. Timeline bar lines that are not always 4 beats, per section, plus faint per-track bar lines for a second meter.
4. Clips drawn in played order with labels on segments (source beat numbers), and a draggable marker on a track (bar 1).
5. A short confirmation when arming a track whose clip is remapped ("Record in the new meter").

## Build plan

Each stage ends with checks anyone can run. Nothing changes for a project without meters.

### R1: typed meters, locking and patterns

Schema 6 (meters, locks, patterns); the compiler's runs after Feel, cut at each clip's own anchors; notes, hits, warp and stretch maps from the runs; Meter 1 sets the section's played length, the grid's bars and the click's accent; the Meter row with patterns and presets, the track lock chip.

- Unit tests for runs: drops, repeats, shuffles, a pattern longer than the bar, the last part bar, cuts at the earliest strong anchor, a late run start trimming the run before, notes copied, dropped, ended at their run and started again mid-way.
- Every golden fixture compiles to a byte-identical track blob with no meters, and the golden renders are unchanged.
- The prototype's six cases as fixtures through the engine: spectral distance to the ideal within 0.1 dB of the prototype's, no click more than 3 dB over the ideal.
- Determinism: two exports bit-identical; 128- and 333-sample calls bit-identical; play from eight points matches a whole render from the next slice on; live playback from a stop against an export, bit-identical; the no-allocation test with eight remapped layers.
- A flexed and remapped sung take: every note's curve starts within 1 ms of its moved note, copies included.
- The owner listens to a drum take, a full imported song and a sung take, each `1 2 3` and `1 3 4 2`.

### R2: bars from a layer, and many meters

Bar detection in the analysis worker, stored on the take; the layer source; bar 1 drawn and dragged; beats per bar typed on a take; any number of meters, polymeter for the ones after Meter 1; the Feel row's offer to lock a follower to its source's meter.

- Detection on the prototype's corpora at least as good as measured: beats per bar 85% on the synthetic grooves and 75% on the real loops, downbeat 85% and 50%. Correcting a wrong downbeat is one drag and one undo step.
- A meter from a layer follows that layer's take: swapping the take in its clip's stack moves the bars with it.
- Two meters on one layer, three meters with followers each: every locked clip plays its own meter's pattern, and changing one meter's pattern recompiles only its clips.

### R3: recording, drawing and polish

Recording into remapped clips (notes through the map, a mic take unlocking its clip); the timeline drawing clips in played order with source beat numbers; Copy to all sections; a walkthrough step "Play this groove in 3/4".

- S5's record check with a remapped clip in the section: live playback and the export bit-identical.
- MIDI notes recorded over a repeated beat play in both copies; a mic take recorded on a remapped track plays as recorded, and undo restores the lock.
- Someone new to the app finishes the walkthrough step in under a minute.

### R4: only if wanted

A trained downbeat model (as Basic Pitch was ported for poly takes); patterns in eighths; keeping the bar's length instead of the beat's (the tempo changes with the meter); writing the remapped notes into the clip (Apply).

## Not in this design

- Bars in free-time sections. A meter needs beats; a free section has none. The second pass's tempo could supply them later.
- Remapping the mic or live MIDI while playing.
- Counting another meter's remapped bars (the only way cycles could arise; see "No cycles can form").
- Separating a held sound from a beat's audio so a dropped beat leaves no tail. That would need source separation.
- Remapping inside a beat (swapping the two 8ths of a beat). Eighth-note patterns (R4) would cover it.

## Questions for the owner

Each has a recommendation; the design above assumes it.

1. **Build it?** Listen to the examples first. Recommended: build R1 if `four-drop4-123.wav` and `five-drop5-1234.wav` sound useful to you; `five-drop4-1235.wav` and the shuffle show where it gets rough. If even the clean cases do not sound worth having, skip it, as you said.
2. **Where the lock lives.** On each clip, with the track header setting all of a track's clips at once (recommended: a take recorded in the new meter can then be free in that section only), or only on the track?
3. **A held note a slice starts in the middle of.** Start it again at the slice start (recommended: the audio slice holds that sound too), or leave it out?
4. **Lanes and morph markers.** Keep them in the clip's played time (recommended, as Feel does), or cut and move them with the beats like the notes?
5. **Downbeat detection.** A suggestion the user confirms by ear (recommended: 53% right on real loops), or a trained model first (R4) so that bars from a layer can be trusted without checking?
6. **Meter 1 and the section.** Meter 1 sets the section's played length, its bar lines and the click's accent, and the click's own bar setting becomes Meter 1's beats per bar (recommended), or keep every section at its typed length and let remapped clips loop inside it?
7. **Pattern resolution.** Whole beats only for now (recommended), or eighths from the start (needed to remap a 7/8 or to swap the halves of a beat)?
