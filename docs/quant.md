# Flex quantize: timing decided at play time

Nov 22, 2026. Design, approved. Stage Q1 is built: see [Q1.md](Q1.md) for what it does and where it differs from this page; the later stages are not. A throwaway prototype in `experiments/flexquant` measured the parts that carry risk; its full output is [experiments/flexquant/results.md](../experiments/flexquant/results.md), and "Prototype" below summarizes it.

Today Quantize moves note starts once, in the document, and leaves the take behind. This page makes quantizing a setting of the clip that is applied every time the track plays or exports. The take keeps its performed timing. The app finds **anchors** in it (onsets, note starts, hits, peaks of a sung feature), pulls them toward a **target** (a grid at any resolution, a swing, a groove, or the timing of another layer) by a **strength** that a lane can automate, and the engine plays the notes, the curves and the take's audio through the resulting time map, stretching the audio between anchors without changing its pitch. One layer can set the timing of any number of others.

## Summary

| Concern | Decision |
| --- | --- |
| Where quantizing happens | At compile and play time, never in the stored notes. Each clip carries a **feel** setting. The compiler turns it into a time map (about a microsecond per clip in the prototype); the engine plays notes, curves and audio through that map. |
| Anchors | Points that should land on the beat: audio onsets, note starts, hits and peaks of a sung feature, found once per take by the analysis worker and stored with the take. **Hard** anchors are transients: the audio around them is copied untouched. **Soft** anchors (a legato pitch change, a peak) only bend time. |
| Target | A grid of any slot size, optionally with triplets, bent by a feel: straight, swing, a groove template, or another layer's timing. |
| Strength | 0 to 1 per clip, automatable with a lane and read once per anchor. Beat layers can scale it per kit slot. A catch window leaves anchors that are far from any slot alone. |
| One layer drives others | A clip's feel can be another layer: that layer's anchors as heard, hit by hit or averaged into a template, optionally only some kit slots (the bass follows the kick). Any number of followers; a cycle is refused. |
| What moves | Notes, hits, the take's curves (so every route that reads a sung feature) and the take's audio. Automation lanes and morph markers stay on the grid (question 1). |
| Audio | Stretched between anchors with its pitch kept: **hits** mode (slices, for beat layers) and **smooth** mode (phase vocoder with phase locking, for melody and texture). Each stretch between two hard anchors is rendered on its own, and the audio around every hard anchor is copied verbatim. |
| Live | The mic and live MIDI are never moved. A pass records raw timing and plays back quantized. Changing the feel while playing recompiles one clip. |
| Determinism | Kept. The map is plain arithmetic on stored data. The stretcher's output depends on the take, the map and where play started, not on how the calls are sliced. Two exports are bit-identical, and live playback from a stop matches an export from that point. |
| Cost | Measured on one core of the build machine: slices about 4 ns per sample, the phase vocoder about 100 ns per sample (up to 175 while the machine was busy with other work), which is 0.5 to 0.8% of a core per stretching layer at 48 kHz. Anchor detection adds about 1 ms per second of audio to the second pass (the high-band onset pass). |

What does not change: one engine binary for live and export; notes and hits in section units; curves in take seconds read through a clip's warp map; the feature-frame format; commands and undo; no allocation, locks or waits on the audio thread.

## What exists today

| Piece | Where | Today |
| --- | --- | --- |
| Notes and hits | Clips, in section units (960 ticks a beat in grid sections) | Placed where they were played, minus the round trip (`placeTake` in `overdub.ts`, the recorder in `record.ts`) |
| Quantize | `quantize` in `commands.ts`; the Quantize button next to Snap in the timeline header | Snaps note or hit starts to a grid, in the document, for good |
| Warp map | `MelodyClip.warp`: an anchor at every note start and end, written by `placeTake` | Maps clip time to take seconds. Beat clips have none |
| Take curves | `curve::warp_to_take`, read per voice (`read_frame` in `layer.rs`) and per layer (`take_frame` in `engine.rs`) | Read through the warp map |
| Take audio | `take_audio` in `engine.rs`, `render_blend` in `layer.rs` | Read at the mapped position with linear interpolation, so a stretch changes the pitch. Heard only on a layer with an `input` element (S5) |
| Onsets, notes, hits | The second pass (`analysis::signal`, `notes`, `beats`) | Computed after every take; notes and hits become the clip |
| Audio files | Influence and kit matching | Cannot come in as a take |

Two things about today's code matter here:

- **Quantize leaves the curves behind.** The `quantize` command moves note starts but not the warp map, and a voice reads its curve from its note's new start through the old map. So a quantized note reads the take from where it used to start: a note moved 30 ms earlier begins with 30 ms of whatever preceded it in the take. Flex replaces both with one time map.
- **Moving take audio through the warp map changes its pitch.** The prototype read a sung take through a quantize map the way the engine does: 68 cents off on average and up to 208 cents, where the stretchers below stay within 5 cents.

## The model

### Terms

- **Anchor.** A time in a take, or in a clip's note data, that should land on the beat. It has a weight (how strong the event is) and is hard or soft.
- **Slot.** A position on the target grid, in section units.
- **Feel.** What bends the grid before anchors are pulled to it.
- **Strength.** How far an anchor moves toward its slot: 0 leaves it, 1 puts it on the slot.
- **Catch.** Anchors further than this from their nearest slot are not pulled.
- **Rider.** An anchor that is not pulled to a slot: it is outside the catch, or a heavier anchor took its slot. It moves along with its neighbours. A hard rider still protects its transient.
- **Time map.** The piecewise-linear map from performed clip time to played clip time that the moved anchors and riders define.
- **Source and follower.** A layer whose timing is the feel of another layer's clip.

### Anchors

| Kind | Found from | Hard or soft | Measured in the prototype (found within 50 ms; error) |
| --- | --- | --- | --- |
| Onset | The second pass's spectral-flux detector on the audio, plus the same detector on a 4 kHz high-passed copy, which catches quiet bright hits under a louder low one | Hard | Drums: 56 of 64 onsets in each of two loops, mean 1.4 and 0.9 ms, max 8.3 ms. The detector alone found 44: it misses quiet off-beat hats under a kick's decay |
| Note start | The second pass's notes. A start with no onset within 20 ms is a legato join; it is moved back to where the pitch (pYIN on the audio) is halfway from the last note to this one | Soft at a legato join, hard otherwise | Sung line: 18 of 18, mean 3.3 ms. Legato joins: mean 10.6 ms, the worst 27 ms late. Without the move, legato joins come 28 ms late |
| Hit | The second pass's hits, with their kit slot | Hard | As onsets |
| Feature peak | A peak of sung volume or intensity in the stored frames, above a minimum prominence | Soft | Not measured; 10 ms steps |
| Frame onset | The live tier's onset slot in the stored frames, for frames without usable audio | Hard | 16 ms late (the live tier's delay) in 10 ms steps; refined on the audio, 1.2 to 1.8 ms mean, but it misses more |
| Note data | Note and hit starts of a clip without a take: drawn, or MIDI from a pass | Hard | Exact |

- **When.** The analysis worker finds a take's anchors right after the second pass. The full-band onsets are already part of it; the extra high-band pass costs about 1 ms per second of audio, against 17 to 28 ms per second for the second pass.
- **Stored, not re-found.** A take's anchors are kept as a blob (three floats each: take seconds, weight, flags) with the detector's version. A later detector never changes an old project's timing unless the user asks to find anchors again. Note-data anchors are not stored; the compiler reads them from the notes.
- **Edits.** The clip stores the user's changes: anchors added, removed, pinned (never moved), or switched between hard and soft. One sensitivity control per clip re-runs the detector in the worker with a lower or higher threshold and keeps the edits.

### Targets

A target is a slot size (`grid`, 1/4 to 1/32) and a feel. With `triplets` on, the triplet slots of the same size are added and each anchor goes to the nearer one.

| Feel | What it is |
| --- | --- |
| Straight | The section's grid. |
| Swing | Every second slot of an 8th or 16th pair falls at `amount` of the pair (50% straight, 66% triplet feel), as on an MPC. |
| Groove | A saved template: an offset in ms for each slot of one or two bars. A few factory grooves (swings, push, lay back); the user can save any layer's timing as a groove. |
| Layer | Another layer's anchors as heard; see "One layer drives others". |

An anchor goes to the slot whose **bent** time is nearest, not to the nearest straight slot. So a hat played with the swing it is being quantized to stays swung: in the prototype, drums played at 58% swing and quantized to 58% swing landed their anchored onsets 1.4 ms on average from the swung slots.

### Strength, catch and order

For an anchor played at `t` and given slot `k`:

```text
played = t + s(k) × (slot_time(k) - t)
s(k)   = strength (or the strength lane's value at slot k) × the kit slot's factor (beat layers)
```

- **Catch.** Default: half a slot. An anchor further from its slot rides along.
- **One anchor per slot.** The heaviest takes the slot; the others ride.
- **Order.** An anchor that would land less than 12 ms after the one before it rides instead, so anchors never cross and every stretch is long enough for its crossfade.
- **Lanes.** A strength lane is read once per anchor, at its slot. In the prototype, a lane rising from 0 to 1 over four bars left 0.87, 0.64, 0.38 and 0.12 of the original offsets in bars 1 to 4, against lane values of 0.875, 0.625, 0.375 and 0.125 mid-bar.

### The time map

The compiler builds one map per clip whose feel is on:

1. Each anchor's take time goes to its performed clip time through the take's placement (today's warp map).
2. Anchors are pulled to their slots as above; riders are placed on the line the moved anchors draw.
3. The map runs through the moved anchors and riders and repeats with the loop: an anchor played just before the loop end can be pulled to the next pass's first slot, and its note then starts at the top of the loop. (This page first pinned the map at the loop's ends; then a note sung a little early on the first beat, which placing a take stores just before the loop end, could never reach it.)

```text
 performed   ──x─────x────x──────x───x──────▶   anchors where they were played
               │      ╲    │      │    ╲
 played      ──x───────x───x──────x─────x───▶   each pulled toward its slot by the strength
               1       &   2      &     3
```

Everything the clip plays goes through that one map:

- **Notes and hits.** Start and end both go through the map, so durations follow.
- **Curves.** The warp map the engine gets is the anchors' take times paired with their played clip times. Voices and layer routes read the take's frames through it as today, so a swell stays on its note. In the prototype, the loudness the synth reads through the map and the loudness of the stretched audio agree within 0.1 ms (correlation 0.999).
- **Take audio.** Stretched through the same map ("Stretching the audio").
- **Not lanes or morph markers.** They are usually drawn on the grid and stay in section units (question 1).

### One layer drives others

```text
 section grid      |1 . . . 2 . . . 3 . . . 4 . . . |
 drums (source)     K   h   S   h   K h K   h   S   h     its anchors as heard, after its own feel
                    │       │       │   │       │
 groove             slot → the time the drums played it; between hits, interpolated
                    │       │       │   │       │
 bass (follower)    B───────────────B───B───────────      kick slots only, hit by hit
 vocal (follower)   v───────v───────v───v───────v───      every drum slot, 8th grid
```

- A clip's feel can name another layer. The source's anchors count **as heard**, after its own feel, so a follower of a tightened drum part follows the tightened drums.
- **Hit by hit** follows the source as it varies through the take, pushes and drags included. **Average** folds the source's offset per slot over the loop into a one- or two-bar template.
- A follower's slot where the source has no anchor gets a time interpolated between the source's neighbouring anchors, so a follower note between two kick hits lands between where they were played.
- On a beat source the follower can pick kit slots (the kick only).
- Sources and followers share a section. With uneven loops the follower reads the source's groove at the section position modulo the source's loop.
- Chains (A follows B, B follows C) are allowed. The compiler orders them, refuses a cycle as it refuses a cycle in a voice patch, and recompiles a source's followers when the source changes.
- Free-time sections have no grid; there the only feel is a layer, and anchors go to the nearest source anchor within the catch.

Measured in the prototype, as the distance from each true event to the source's hit on the same slot:

| Follower | Before | After |
| --- | --- | --- |
| Sung line following drum loop A, hit by hit | 18.1 ms mean, 58.4 max | 3.4 ms mean, 28.9 max (the late legato join) |
| Drum loop B following drum loop A, hit by hit | 15.1 ms mean, 45.2 max | 0.9 ms mean, 8.1 max |
| Drum loop B following A's one-bar average | 15.1 ms mean, 45.2 max | 7.9 ms mean: the average leaves A's push and drag out, by design |

Onsets the detector misses do not follow: they ride with their neighbours. In the drum cases the eight hats it missed ended 7 to 22 ms off on average, against about 1 ms for the onsets it found, which is why the recall of the anchor detector matters more than its precision.

### Stretching the audio

Today's read changes pitch and resamples attacks (attack peaks 1.2 dB off the ideal render on average, against 0.06 dB for the modes below). Two modes replace it:

| Mode | How | Default for |
| --- | --- | --- |
| Hits | Slices: each stretch plays at the take's own speed from its new start and fades out where the next one starts, or where its own audio runs out (a short gap when it is pulled apart). | Beat layers |
| Smooth | Phase vocoder (2,048-sample frames, a quarter-frame hop, identity phase locking), reset at every hard anchor. | Melody and texture layers |

Both are **anchor-locked**. The audio is cut at hard anchors only. Each stretch between two hard anchors is rendered on its own:

```text
 played time ─▶   A'−5ms A'                                                 B'−5ms  B'
                  │ pre │ hold: copied   │ stretched, bent through any   │ tail: the take's │
                  │     │ verbatim 20 ms │ soft anchors in between       │ own speed        │
 stretch A        ╱‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾╲
 stretch B                                                               ╱‾‾‾‾‾‾‾‾‾‾‾‾
                                                     5 ms crossfade, both sides reading the same samples
```

- **Pre-roll and hold.** A stretch starts 5 ms before its anchor and copies the 20 ms after it verbatim, so the attack is untouched. On the drums, attack peaks came out 0.06 dB from the ideal render on average (0.3 dB at worst) with both modes.
- **Tail.** The last half frame before the next anchor runs at the take's own speed, so it reads exactly the samples the next stretch's pre-roll reads and the crossfade adds two identical copies.
- **Nothing past the next anchor.** Reads beyond it come back silent, so no stretch plays the next attack early. Before this rule, the phase vocoder spread each next attack into the 20 ms before it, and the onset detector found its drum hits about 20 ms early.
- **Soft anchors do not cut.** A legato join only bends the stretch. When the prototype cut at legato joins too, the level dipped by up to 2.2 dB more than the input at a join (WSOLA) and up to 9.5 dB (phase vocoder); with soft anchors both stay within 0.3 dB of the input.

Results on the prototype's takes, quantized at full strength (drums A to straight 16ths, the sung line to straight 8ths):

- **Attacks** looks for the 22 ms of take audio around each hard anchor in the output, within 30 ms of where the map puts it, and gives the correlation where it matches best.
- **Onsets** runs the anchor detector on the output and compares with where the map puts the onsets the same detector finds in the input, so the detector's own bias cancels.
- **Spectral distance** is the RMS difference of third-octave band levels against the take rendered with every event exactly on the grid. The untouched input scores 4.4 dB (drums) and 3.0 dB (sung).

| Read | Attacks: correlation, drums / sung | Onsets, drums: mean / max ms | Onsets, sung | Pitch, sung: mean / max cents | Spectral distance, drums / sung | CPU, ns per sample |
| --- | --- | --- | --- | --- | --- | --- |
| Today's (linear, varispeed) | 0.42 / 0.48 | 0.6 / 12 | 0.6 / 4.6 | 68 / 208 | 4.9 / 4.0 dB | 4 |
| Hits (slices) | 0.999 / 1.000 | 2.4 / 5.0 | 7.2 / 50 (gaps) | 0.3 / 3.8 | 3.2 / 1.6 dB | 4 |
| Smooth (phase vocoder) | 1.000 / 1.000 | 1.5 / 20 | 0.1 / 0.8 | 0.3 / 3.3 | 3.2 / 2.0 dB | 97 to 108 |
| WSOLA, tried and not proposed | 1.000 / 1.000 | 1.3 / 16 | 1.0 / 11 | 0.3 / 2.1 | 3.5 / 2.4 dB | 55 to 62 |

- Every anchor-locked mode puts each attack at exactly the sample the map gives. Today's read resamples it, so it no longer matches the take's waveform.
- The 16 and 20 ms drum onsets are not misplaced attacks. The detector refines an onset by looking up to 40 ms back for the quietest point, and in a stretched hat tail before a hit it starts the onset early. Slices do not stretch tails.
- Slices score well on the sung line's spectrum because its moves are small, but they leave a gap or a cut at every join, up to the size of the move.
- WSOLA costs a little over half what the phase vocoder does and measured about the same on the synthetic voice, but its search shifted two sung onsets by 11 and 13 ms, and it cannot handle chords, which polyphonic capture ([poly.md](poly.md)) will bring. It stays an option if real voice takes prefer it (question 4).
- Cost is the same in 128-sample calls as in one whole-take call. It varied with the other work on the machine: the phase vocoder measured 97 to 175 ns per sample across the cases, 0.5 to 0.8% of one build-machine core per stretching layer at 48 kHz. The reference laptop is slower, so Q2 measures it there.

## How it fits the rest

### Clips, takes and sections

The feel belongs to the clip, so each section's clip has its own (a tighter chorus, a looser verse). "Copy to all sections" copies it. Schema 5 adds optional fields; a document without them plays exactly as today, so there is nothing to migrate.

```ts
interface Take {
  // id, audio, frames, rate, latencyMs: unchanged
  anchors?: BlobRef;          // Float32, three per anchor: take seconds, weight 0..1, flags
                              // (bit 0 hard; bits 1-2 kind: onset, note, hit, peak; bits 3-4 kit slot)
  anchorsVersion?: number;    // the detector that found them
}
interface ClipBase {
  // take, takes, loopLen, timeSnap, morph, auto: unchanged
  flex?: Flex;                // missing: plays as performed, as today
}
interface Flex {
  on: boolean;
  grid: number;               // slot size in section units (240 = 16ths)
  triplets: boolean;          // add the triplet slots of that size; the nearer wins
  feel: Feel;
  strength: number;           // 0..1; a lane on 'flex.strength' replaces it, read per anchor
  slotStrength?: number[];    // beat layers: a factor per kit slot
  catch: number;              // section units; anchors further from their slot ride along
  audio: 'hits' | 'smooth';
  sensitivity: number;        // 0..1, 0.5 = the detector as measured
  edits: AnchorEdit[];
}
type Feel =
  | { kind: 'straight' }
  | { kind: 'swing'; amount: number; unit: number }       // amount 0.5..0.75; unit in section units
  | { kind: 'groove'; groove: string }                    // an id in Project.grooves
  | { kind: 'layer'; layer: string; mode: 'hits' | 'average'; bars: 1 | 2; slots?: number[] };
interface Groove { id: string; name: string; period: number; offsets: { t: number; ms: number }[] }
interface AnchorEdit { t: number; op: 'add' | 'remove' | 'pin' | 'hard' | 'soft' }  // take seconds
```

- **Takes.** Anchors belong to the take, the feel to the clip, so switching takes in a clip's stack keeps the feel and uses the new take's anchors.
- **Clips without a take** (drawn notes, MIDI from a pass) use their note and hit starts as anchors. Flex then does what Quantize does today, without losing the played timing.
- **Polyphonic takes** ([poly.md](poly.md)) store notes with per-note curves read from each note's start. Their onset-snapped note starts are hard anchors, each note and its curve moves through the map like any note, and the take's audio needs smooth mode.
- **Importing a sample.** Flexing a sample or a loop needs it as a take: decode the file, run the live tier over it for frames, run the second pass and find anchors, as `frames_from_signal` already does for tests. The imported take goes on a layer with an `input` element at mix 1, so it plays as audio.

### Lanes

Two new lane targets, `flex.strength` and `flex.swing`, drawn and recorded like any lane. The engine never sees them: the compiler reads them once per anchor and builds the map. Moving the strength knob while playing recompiles that clip. A lane edit is one command, as today.

### Scenes and states

Flex is timing, not sound, so it is not part of a state and a scene does not change it. A section's clip carries its own feel instead. Whether a scene should be able to tighten the groove live is question 6.

### The modulation matrix

Strength is not a route target. Routes run per control tick in the engine, and a per-tick strength would mean quantizing in the engine: re-ordering events while they play, resolving collisions and keeping maps monotone without seeing the whole clip. A lane read at compile time gives the same automation with none of that. The routes that read sung features keep working on a flexed take, because the frames are read through the map.

### Recording

- A pass (S5) records raw: notes where they sounded, the take at its own time. The monitor is never quantized.
- When the pass is committed, the clip's feel applies at once on playback. A new take placed over a clip keeps the clip's feel, as it keeps its lanes.
- MIDI and on-screen notes recorded into a clip with a take are moved by the take's map. In a clip without a take, they are the anchors.

### Where it runs

| Work | Runs in | Thread |
| --- | --- | --- |
| Anchor detection, sensitivity re-runs | Rust, the analysis worker, after the second pass | Worker |
| Quantizing and the time map, groove templates, follower order | TypeScript, the compiler | Main |
| Notes, hits and the warp map through the time map | Compiled into the track blob | Main, then audio |
| Reading curves through the map | Rust, unchanged | Audio |
| Stretching take audio | Rust, in `crates/klont` | Audio; the same code in the export worker |

### Engine changes

| Change | Detail |
| --- | --- |
| Track blob v4 | Warp anchors gain a flags word (bit 0: hard). Clips gain the audio mode (0 today's read, 1 hits, 2 smooth). v1 to v3 still load, and the encoder keeps writing the oldest version that holds the track, so projects without flex stay byte-identical. |
| Stretcher | One per layer with two renderers, for the crossfade between stretches, plus a second stretcher for a map that changes while playing (it restarts at the playing position and crossfades 5 ms from the old one). All buffers are allocated at start-up: under 100 KB per phase vocoder renderer, under 400 KB per layer. |
| `take_audio`, `RawSrc::Take` | Hand the stretcher the clip's map and position instead of a varispeed step. Clips without flex keep today's read. |
| Loop wrap | The clip's loop start is a hard anchor: the last stretch fades out over 5 ms past the loop end while the first starts. |
| Beat clips | Get a warp map (today they have none), so a beat take's audio follows its hits. |
| Cost | `bench --costs` gains the stretcher, priced per layer; a layer pays it only while its clip stretches. |

### Determinism

The rules of synth.md "Deterministic playback" hold, with three additions:

1. **Stored anchors.** Playback and export use the stored anchors, never a fresh detection. The map is computed with additions, multiplications and divisions only, so every browser gets the same numbers, and the engine reads them from the blob as f32.
2. **A stretch depends on where it started.** The stretcher's output at a sample depends on the take, the map, and the later of its stretch's start and the point play started. It does not depend on call sizes. Measured: two renders are equal; renders in random call sizes from 1 to 3,000 samples are equal; play started at eight points repeats exactly, and matches a whole render from the next hard anchor on (the longest difference lasted one stretch: 229 ms on the drums, 1.1 s on a long sung note).
3. **Exports and live playback start the same way.** An export starts at Play like live playback, so the two match bit for bit, as S5's check 1 requires. The stretcher keeps nothing from before Play, so Play's reset needs nothing new.

### Live versus render time

| | Playback and export | Live |
| --- | --- | --- |
| Notes from clips | Through the clip's map | — |
| Take audio and frames | Stretched and read through the map | — |
| The mic (armed or listening layers) | — | Never moved: making sound earlier is impossible, later is a delay |
| MIDI and on-screen keys | Recorded raw, quantized on playback | Raw. Optionally delayed to the next slot (question 5) |
| A layer following another | Follows the source clip's anchors | Cannot follow a live player; that needs beat tracking (not in this design) |
| Feel changes | Recompile one clip; the stretcher crossfades 5 ms | Same |

## Prototype

`cargo run --release --manifest-path experiments/flexquant/Cargo.toml` prints [results.md](../experiments/flexquant/results.md); `FLEXQUANT_WAV=<dir>` also writes every input and output as a WAV file to listen to. The crate has its own workspace, so `pnpm test` does not build it.

- **Takes.** Synthetic, so every onset's true time and slot is known: drum loop A (4 bars of 16ths, 58% swing, a late snare, random timing and a slow push and drag), drum loop B (straight, an early snare, more random timing), both over room noise at about -66 dB, and a sung line of 18 notes with detached, "da" and legato joins, late and loose.
- **Detectors.** The app's own: the second pass's onset detector, the whole second pass, and the live tier's frames.
- **Cases.** Drums to straight 16ths; drums to their own swing; drums with a strength lane; the sung line to 8ths; the sung line following the drums; drums B following drums A, hit by hit and averaged.
- **Measured.** Anchor accuracy, where the map puts every true onset, where the attacks and onsets land in the audio, pitch, spectral distance to the ideal render, attack peaks, level at legato joins, CPU per sample, and the determinism checks above.
- **Limits.** No real recordings: the only WAV files on hand are synthetic test tones. The synthetic voice is cleaner than a real one, so the choice between the phase vocoder and WSOLA for voice is not settled by these numbers (question 4). CPU was measured on the build machine (Apple M4) with other work running.

## Interface sketch

The Quantize button in the timeline header becomes a Feel row for the selected layer's clip.

```text
┌ Timeline ─ Zoom [Fit ▾]  Snap [1/16 ▾]  Take [3 ▾] ────────────────────────────────────────────┐
│ Feel [■ on]  Grid [1/16 ▾] [☐ triplets]  Feel [Swing ▾] 58%  Strength ━━━━━●━ 80%  Catch ±38 ms │
│      Audio [Smooth ▾]   Anchors 58 (4 soft)  Sensitivity ━━●━━   [Copy to all sections]         │
├────────────────────────────────────────────────────────────────────────────────────────────────┤
│ Drums  ▏K  h  S  h  K h K  h  S  h …     ▏ hard anchor, ┆ soft; a faint line back to where played │
│ Bass   ▏B ────────── B ── B ───────      Feel: follows Drums (kick), hit by hit         ⇠ Drums   │
│ Vocal  ┆v ── v ──── v ─ v ──────         Feel: follows Drums, 1/8                       ⇠ Drums   │
│        strength lane ╱‾‾‾‾‾‾‾‾                                                                  │
└────────────────────────────────────────────────────────────────────────────────────────────────┘
```

- The Feel menu lists Straight, Swing, the saved grooves and "Follow layer", which asks for the layer, hit by hit or average, and (for a beat source) its kit slots.
- Each anchor is a tick at its played time with a faint line back to where it was played. Click selects one; dragging pins it where it is dropped; double-click adds one; H switches hard and soft; Delete removes it. Every edit is one undo step and has a keyboard path, like the rest of the timeline.
- Strength and swing appear under "Lane" like any lane target, drawn in the automation strip.
- Pressing Quantize, as today, turns Feel on at 100% with the Snap value as the grid.

## Build plan

Each stage ends with checks anyone can run. Q1 changes nothing for a project that does not turn Feel on.

### Q1: anchors and quantize without stretching

Anchor detection in the analysis worker (onsets with the high-band pass, note starts with soft legato joins, hits), stored per take; note-data anchors; `Flex` on clips (schema 5, optional fields); the compiler's time map for notes, hits and the warp map, so curves follow; beat clips get a warp map; straight and swing feels with triplets; strength, catch and kit slot factors; the Feel row; Quantize turns Feel on. Until Q2, a clip whose layer has an `input` element plays its raw audio at the performed timing, and the Feel row says so.

- Unit tests for quantizing: the nearest slot in the bent grid, catch, one anchor per slot, order, riders, the strength lane read per anchor, kit slot factors; the map stays monotone for any strength lane.
- Every golden fixture compiles to a byte-identical track blob with Feel off, and the golden renders are unchanged.
- Anchor accuracy on the synthetic corpora (`beat_corpus`, `hum_corpus` and the prototype's takes): at least 85% of onsets found within 50 ms, mean error 2 ms or less, legato joins 12 ms or less on average (prototype: 88%, 1.4 ms, 10.6 ms).
- A flexed sung take: every note's curve starts within 1 ms of its moved note.
- S5's record check re-run with Feel on in the recorded clip: live playback and the export are bit-identical.

### Q2: stretching the take's audio

The stretcher (hits and smooth) in `crates/klont`, track blob v4, the crossfade for a map that changes while playing, the loop wrap, the cost in `bench --costs`; importing an audio file as a take.

- Rust tests on the prototype's takes as fixtures: every attack at a hard anchor found at the sample the map gives (correlation above 0.99); sung pitch within 5 cents; legato joins within 0.5 dB of the input's level; spectral distance to the ideal render below the untouched input's.
- Determinism: two exports bit-identical; 128-sample and 333-sample calls bit-identical; play from eight points repeats exactly and matches a whole render from the next hard anchor on; live playback from a stop against an export, bit-identical.
- The no-allocation test with a stretching layer; a map change while playing makes no step larger than the 5 ms fade.
- Cost: 8 layers stretching in smooth mode on the reference laptop, in ns per sample, recorded beside the M1 bench.
- The owner listens to drums, a sung take and (once polyphonic capture lands) a chord take at strengths 0.5 and 1.

### Q3: one layer drives others

The layer feel, hit by hit and average, kit slot choice, chains with cycle refusal, followers recompiled when their source changes; saved grooves in the project, exported and imported as files; factory grooves.

- Drums following drums, as prototype case 6: onsets within 2 ms on average of the source's hit on the same slot (prototype: 0.9 ms). The sung line following drums within 5 ms on average (prototype: 3.4 ms).
- A cycle is refused with a message and the document does not change.
- Changing the source's feel recompiles only its followers, and two exports after the change are bit-identical.
- An averaged groove from a 4-bar source equals the mean offset of each slot.

### Q4: anchor editing, peaks and lanes

Anchor edits in the timeline (add, remove, pin, hard and soft), sensitivity, feature-peak anchors for takes without onsets, the strength and swing lanes, Copy to all sections.

- A strength lane from 0 to 1 over four bars leaves 1 minus the lane of each bar's offsets, within 0.02 (prototype: 0.87, 0.64, 0.38, 0.12).
- Each anchor edit undoes as one step, and a journal replay after a killed tab restores the edits.
- Finding anchors again at another sensitivity keeps the user's edits.
- A walkthrough step "Make the bass follow the kick" can be finished in under a minute by someone new to the app.

### Q5: only if wanted

Live input quantize: MIDI and on-screen notes delayed to their next slot within the catch, while playing. Apply: write the played times into the notes and turn Feel off, as one undo step. See questions 2 and 5.

## Not in this design

- Following a live player in real time (tracking the mic's beat so sequenced layers follow it). It needs tempo tracking and look-ahead the live path does not have.
- Changing a section's tempo to fit a free take. Flex moves events within a tempo; the second pass already proposes one.
- Moving notes inside a chord separately. A polyphonic take moves as a whole between anchors; its note data moves note by note.
- Stretching the sampler's and grains' samples per voice.
- Grooves that also scale velocity. The groove format has room for it.
- Humanize (seeded random offsets as a feel). Small to add as a feel later.
- Flex as a modulation source: a pulse at each anchor of a layer that could retrigger another layer's envelopes. A natural step after Q3.

## Questions for the owner

Each has a recommendation; the design above assumes it.

1. **Lanes and morph markers.** Keep them on the grid when a clip is flexed (recommended: they are usually drawn on it), or move them with the performance's time map, which suits lanes recorded in the same pass as the take?
2. **The Quantize button.** Turn it into "Feel on at 100%" and drop destructive quantizing from the app (recommended), or also offer Apply (Q5), which writes the quantized times into the notes?
3. **Following a layer.** Default to hit by hit (recommended: it keeps the source's pushes and drags), or to the averaged template?
4. **Smooth mode for voice.** The phase vocoder for melody and texture (recommended: one method that also handles chords), or WSOLA for single voices (a little over half the CPU)? The prototype only had synthetic takes. Could you record a sung line, a beatbox loop and, once polyphonic capture works, a chord take, so the choice is made by listening?
5. **Live input quantize.** Should MIDI and on-screen notes be delayed to their next slot while you play (Q5)? It makes early notes late to the beat instead of early, at the cost of up to the catch window of delay.
6. **Scenes and routes.** Keep strength a clip setting with lanes (recommended), or make it a value that scenes and routes can move? The second means quantizing in the engine per tick, a much larger build.
7. **Importing samples.** Build "import an audio file as a take", which flexing a sample or a loop needs, in Q2 (recommended), or as its own task?
