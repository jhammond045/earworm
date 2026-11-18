# TDD: Earworm — technical design

Nov 16, 2026 · @Jacob Hammond

This document specifies how each part of the [Earworm PRD](https://claude.ai/artifact/3ZXQPGZdEtMPEqWRHxcQvi) is built. It covers the MVP in full and marks v1 items where they shape MVP design.

## Summary

Earworm is a local-first web app: a Rust audio engine compiled to WebAssembly on the audio thread, Rust analysis code in workers, a TypeScript interface, and one small server function for LLM calls. Nothing else runs on a server.

| Concern | Decision |
| --- | --- |
| Audio engine | Rust compiled to WebAssembly with SIMD, running in a single AudioWorklet |
| Thread links | A shared-memory parameter block and lock-free ring buffers, with a message-passing fallback |
| Analysis | Rust and WebAssembly in Web Workers, sharing one DSP crate with the engine |
| Pitch tracking | YIN live; a pYIN-style second pass by default; a small neural model as the bake-off alternative |
| Interface | TypeScript, Svelte for controls, one WebGL2 canvas for the timeline and the Klont |
| App state | One immutable project document, changed only through commands, with patch-based undo |
| Storage | Origin private file system for audio and curves, IndexedDB for the project and its journal |
| LLM | One stateless server function that proxies the OpenAI Responses API with a strict JSON schema |
| Export | A second engine instance in a worker, rendering faster than real time |

Five design decisions shape everything below:

1. **One engine binary, three hosts.** The same WebAssembly module plays live in the worklet, renders exports in a worker, and renders candidates for Influence matching. What you hear is what exports and what gets matched.
2. **One parameter table.** A single source file defines every parameter. The Rust enum, the TypeScript types, the slider UI and the LLM's JSON schema are all generated from it.
3. **One fixed resolution order.** State morph, Influence, macros, voice gesture, hit shape and modulators always apply in that order. Section 5 specifies it.
4. **One feature-frame format.** All analysis first reduces audio to frames at 100 per second. Melody, beats, gestures, hit shapes and Influence are all computed from those frames. The same frames drive the synth live during capture, so Live follow and playback are one code path.
5. **The sequencer lives in the engine.** The interface never schedules a note. It edits data; the engine plays it against the sample clock.

## Runtime architecture

Three kinds of thread exchange data through four fixed blocks of shared memory, and the audio thread never blocks, allocates or waits on anything.

&#91;embedded content: runtime architecture · three threads, four shared blocks\]

The audio thread hosts both the engine and the live capture front end, which is what lets a voice drive the synth with no thread hop in between.

### The four shared blocks

| Block | Writer, reader | Layout | Used for |
| --- | --- | --- | --- |
| Control ring | Main, audio | Single-producer single-consumer ring of fixed 16-byte messages | Parameter sets, transport, live triggers, table swaps |
| Meter block | Audio, main | Flat Float32 slots guarded by a version counter | Playhead, layer peaks, every effective parameter of the selected layer, CPU load |
| Mic ring | Audio, workers | 60 s of mono Float32 at the context rate, 11.5 MB at 48 kHz | Retro capture, second-pass analysis, Influence |
| Frame ring | Audio, main and workers | 6,000 frames of 24 floats | Live ribbon, Live follow display, second-pass input |

### Boundary rules

- **Audio thread.** No allocation, no locks, no I/O after start-up. All voices, delay lines, tables and buffers are pooled at init.
- **Control messages.** Each is `{ type: u16, layer: u8, index: u8, param: u16, pad: u16, value: f32, time: u32 }`. The engine drains the ring at the top of every block.
- **Large data.** Compiled clips, curve tables and grain audio are posted as transferred ArrayBuffers. The engine parks them in a pre-allocated slot, swaps at the next block boundary, and posts the old buffer back for the main thread to free.
- **Meter block.** The engine writes it once per block. The interface reads it once per animation frame and retries if the version counter changed mid-read.
- **WebAssembly memory is private.** The engine's linear memory is not shared. Small JavaScript glue in the worklet copies ring contents in and meter values out each block. This avoids the WebAssembly threads toolchain entirely.
- **Workers never touch the engine.** They read the mic and frame rings and return results to the main thread, which turns them into commands.

### Cross-origin isolation and the fallback

SharedArrayBuffer needs a cross-origin isolated page, which means serving `Cross-Origin-Opener-Policy: same-origin` and `Cross-Origin-Embedder-Policy: require-corp` ([Chrome for Developers](https://developer.chrome.com/blog/enabling-shared-array-buffer)). Every asset is self-hosted and the LLM function is same-origin, so nothing third-party has to opt in.

If `crossOriginIsolated` is false, the same message formats travel over the worklet's MessagePort, batched once per block. Meters drop to 30 updates per second and mic audio is posted in chunks. Everything still works; Live follow display is less smooth.

## Repository and build

One repository holds five Rust crates and six TypeScript packages. The DSP crate is shared by the engine and the analysis code, so a filter or pitch tracker is written once.

```text
earworm/
  crates/
    dsp/            no_std primitives: filters, FFT, windows, resamplers, YIN, band bank
    params/         params.toml and the code generated from it
    engine/         Klont voices, effects, sequencer, transport, capture front end
    analysis/       second pass: notes, hits, hit shapes, tuning, Influence match
    engine-wasm/    C-ABI exports for the worklet and offline hosts
    analysis-wasm/  C-ABI exports for analysis workers
  packages/
    app/            Svelte interface, stores, commands
    render/         WebGL2 timeline and Klont shaders
    audio-host/     worklet glue, ring buffers, device handling, calibration
    workers/        analysis, export and storage workers
    schema/         project types, migrations, generated parameter types
    llm-fn/         the server function
  tools/
    paramgen/       params.toml to Rust, TypeScript and JSON Schema
    corpus/         test recordings and labels
```

### Toolchain

- **Rust stable, target `wasm32-unknown-unknown`.** Flags: `-C target-feature=+simd128`, `panic=abort`, `opt-level=3`, LTO on.
- **SIMD only, no scalar build.** [WebAssembly fixed-width SIMD](https://web-platform-dx.github.io/web-features-explorer/features/wasm-simd) has been Baseline widely available since September 2025, from Chrome 91, Firefox 89 and Safari 16.4.
- **No wasm-bindgen on the audio path.** The worklet scope lacks most web APIs. The main thread fetches the module bytes and passes them in `processorOptions`; the processor instantiates them synchronously in its constructor.
- **TypeScript side.** pnpm workspaces, Vite, Svelte, Vitest, Playwright.
- **Hosting.** Any static host that allows custom response headers, plus one serverless function on the same origin.

### Engine ABI

```rust
#[no_mangle] pub extern "C" fn ew_init(sample_rate: f32, max_block: u32) -> *mut Engine;
#[no_mangle] pub extern "C" fn ew_buffers(e: *mut Engine) -> *const BufferTable; // in, out, control, meter, frame
#[no_mangle] pub extern "C" fn ew_process(e: *mut Engine, frames: u32);          // live: one render quantum
#[no_mangle] pub extern "C" fn ew_render(e: *mut Engine, frames: u32) -> u32;    // offline: any length
#[no_mangle] pub extern "C" fn ew_load(e: *mut Engine, slot: u32, kind: u32, ptr: *const u8, len: u32);
#[no_mangle] pub extern "C" fn ew_seek(e: *mut Engine, section: u32, ticks: f64);
```

The worklet host and the offline hosts link the same exports. The only difference is who calls `ew_process` or `ew_render`.

## Data model

A project is one JSON document plus binary blobs for audio and curves. The interface edits the document only through commands; a compiler turns changed parts into engine tables.

```ts
type Ticks = number;     // 960 per beat, used in gridded sections
type Seconds = number;   // used in free-time sections
type BlobRef = string;   // id of a binary file in the origin private file system

interface Project  { schema: number; id: string; name: string; sections: Section[]; layers: Layer[] }
interface Section  { id: string; feel: 'grid' | 'free'; beats?: number; seconds?: number;
                     bpm: number; repeats: number; rampMs: number; clips: Record<string, Clip> }
interface Layer    { id: string; kind: 'melody' | 'beat' | 'texture'; color: number;
                     level: number; pan: number; mute: boolean;
                     states: State[]; influence?: Influence; tune: Tune; depth: GestureDepth;
                     kit?: KitSlot[] }                       // beat layers: four slots
interface State    { id: string; base: ParamSet; macros: number[]; influenceAmount: number }
interface Influence{ target: ParamSet; mask: number; source: BlobRef; grain?: BlobRef }
interface Tune     { snap: number; speed: number; slides: 'glide' | 'gliss' | 'jump';
                     vibrato: number; levelOut: number; scale: Scale; refCents: number }
interface GestureDepth { loud: number; open: number; hard: number; breath: number; mouth: number }

type Clip = MelodyClip | BeatClip | TextureClip;
interface ClipBase   { take: string; takes: string[]; loopLen: number; timeSnap: number; morph: MorphMarker[] }
interface MelodyClip extends ClipBase { notes: Note[]; warp: WarpAnchor[]; edits: CurveEdit[] }
interface BeatClip   extends ClipBase { hits: Hit[]; follow: number; kinds: number }
interface Note { id: string; t: number; dur: number; cents: number; hard: number }
interface Hit  { id: string; t: number; slot: number; strength: number; shape: HitShape }
interface HitShape { attack: number; burst: number; cents: number | null; bend: number;
                     hold: number; vowel: number; tone: number; endStop: number; endClick: number }
interface Take { id: string; audio: BlobRef; frames: BlobRef; rate: number; latencyMs: number }
interface MorphMarker { t: number; state: string; lenMs: number; shape: 'line' | 'ease' | 'late' | 'step' | 'wobble' }
interface WarpAnchor  { takeSec: number; clipT: number }
```

### Feature frames

A take's `frames` blob holds one 24-float record per 10 ms. Every analysis and every gesture channel reads this format.

| Slot | Field | Unit |
| --- | --- | --- |
| 0 | Pitch | Cents above C0, or NaN when unvoiced |
| 1 | Pitch confidence | 0 to 1 |
| 2 | Loudness | 0 to 1 after floor and ceiling calibration |
| 3 | Openness | 0 to 1 |
| 4 | Breathiness | 0 to 1 |
| 5 | Onset strength | 0 to 1 |
| 6 | Hardness of the most recent onset | 0 to 1 |
| 7 | Voiced, unvoiced or silent | 0, 1 or 2 |
| 8 to 23 | Vowel shape: 16 band levels with loudness and the singer's average removed | dB |

### Time

- **Notes and hits live in ticks** in gridded sections, so a tempo change re-times them with no data change.
- **Curves live in take seconds.** A warp map of anchors at every note start and end maps take time to clip time. Curves are read through it, so a swell stays attached to its note when timing is snapped or tempo changes.
- **Free-time sections** use seconds for both, and the warp map is the identity.

### Commands and undo

- **One immutable document.** Every change is a named command that produces a patch and its inverse.
- **Undo is a stack of patch groups.** A drag is one group. The stack is persisted, so undo survives a reload.
- **The compiler is a diff.** After each command it recompiles only the clips, layers or sections whose identity changed, and posts the new tables to the engine.
- **Previews skip the document.** While a slider or note is being dragged, values go straight to the control ring. The command is committed on release.

## Parameter system

Every sound is 64 scalar parameters plus two 16-band shapes: 96 numbers, each stored normalized from 0 to 1. One table defines them, and one fixed order turns stored values into what the DSP uses.

### The parameter table

```toml
[[param]]
id        = "src.tilt"     # stable string id, never reused
group     = "tone"         # tone | shape | motion | grit | space  (Influence aspects)
scale     = "lin"          # lin | log | db | bipolar
min       = -24.0
max       = 6.0
neutral   = -6.0           # value used when an old project lacks this parameter
unit      = "dB/oct"
smooth_ms = 15
doc       = "Slope of partial levels. Lower is duller."
macros    = { bright = 0.6, body = -0.2 }   # weights into the macro matrix
```

`paramgen` reads this file and emits the Rust `ParamId` enum and descriptor array, the TypeScript types and slider metadata, the JSON Schema used by the LLM function, and the parameter dictionary in the LLM prompt.

### Parameter list

| Stage | Parameters | Count |
| --- | --- | --- |
| Sources | harm\_mix, noise\_mix, grain\_mix, tilt, oddeven, stretch, jitter, modal, damp, f1\_freq, f1\_gain, f2\_freq, f2\_gain, grain.size, grain.density, grain.pos, grain.spray, grain.scatter | 18 |
| Warp | fold.amt, fold.bias, clang.amt, clang.ratio, comb.amt, comb.tune, comb.decay, smear.amt, smear.freeze, crush.bits, crush.rate, tape.amt | 12 |
| Shape | filt.cutoff, filt.res, filt.morph, filt.track, env.attack, env.hold, env.decay, env.sustain, env.release, env.noise\_decay, env.pitch\_amt, env.pitch\_time, strike.click, strike.color, strike.end\_click, lfo1.rate, lfo1.to\_pitch, lfo1.to\_filter, lfo1.to\_level, lfo2.rate, lfo2.to\_pitch, lfo2.to\_filter, lfo2.to\_grain, chaos.rate, chaos.amt | 25 |
| Space | delay.time, delay.fb, delay.shift, delay.mix, verb.size, verb.damp, verb.mix, verb.freeze, width | 9 |
| Shapes | tone\_bands\[16\], noise\_bands\[16\] | 32 |

There are no switches. Filter type is a morph value, and each LFO has a depth per destination instead of a destination selector. That keeps every pair of sounds interpolable.

### Resolution order

Stored values become DSP values through eight steps, always in this order.

1. **Morph.** Interpolate the two neighbouring states on the morph lane: base, macros and Influence amount.
2. **Influence.** Blend base toward the Influence target by the amount, per aspect group.
3. **Macros.** Add each macro's weighted offset, then clamp to 0..1.
4. **Voice start.** Copy the layer result to the voice. Steps 5 to 7 run per voice.
5. **Gesture.** Add loudness, openness, hardness and breathiness offsets, scaled by each channel's depth. Blend the band shapes toward the frame's vowel shape by Mouth depth.
6. **Hit shape.** For beat voices, apply the hit's own pitch, length, vowel, start and end, scaled by Follow.
7. **Modulators.** Add LFO and chaos outputs.
8. **Smooth and map.** One-pole smoothing with the parameter's `smooth_ms`, then map from 0..1 to real units.

Steps 1 to 3 run once per layer every 32 samples. Steps 2 and 3 are:

```latex
p^{(2)}_i = (1 - a\,m_{g(i)})\,b_i + a\,m_{g(i)}\,t_i
```

```latex
p^{(3)}_i = \operatorname{clamp}\!\Big(p^{(2)}_i + \sum_{k=1}^{8} W_{k,i}\,(2u_k - 1),\; 0,\; 1\Big)
```

Here b is the morphed base, t the Influence target, a the amount, m the 0-or-1 mask for the parameter's aspect group, u the eight macro values and W the sparse macro weight matrix from the table.

### What the sliders show

"All the sliders move as it captures" is implemented from the meter block, not by changing stored values.

- **Two snapshots per block.** The engine publishes the selected layer's values after step 3 (the stored sound) and after step 5 (the sound with the voice applied).
- **Parameter sliders** draw a fixed thumb at the stored value and a live thumb at the step-5 value.
- **Macro sliders** show the gesture projected back onto each macro's weight vector:

```latex
u^{\text{live}}_k = u_k + \frac{W_k \cdot (p^{(5)} - p^{(3)})}{2\,\lVert W_k \rVert^2}
```

- **Listen is different.** During Sound Influence capture the target itself is re-estimated several times a second, so the stored thumbs move.

## Klont DSP

Each layer runs up to 8 voices through per-voice modules and then one set of per-layer modules. Control values update every 32 samples; audio-rate loops are written for 128-bit SIMD.

| Module | Runs | Algorithm |
| --- | --- | --- |
| Partial bank | Per voice | Up to 64 recursive sine oscillators processed four at a time. Frequency of partial k is f0 times a ratio; `modal` morphs the ratio table from harmonic to drumhead to bar, then `stretch` is applied. Amplitude is tilt times odd-even balance times formants times `tone_bands` read at the partial's frequency, recomputed each control tick and ramped. With `damp` above zero each partial also decays on its own clock. |
| Shaped noise | Per layer | White noise into 16 band-pass state-variable filters at fixed centres from 80 Hz to 12 kHz. Gains come from `noise_bands`. Level follows the sum of the voices' noise envelopes, which is equivalent to per-voice noise because the path is linear. |
| Grain cloud | Per layer | Up to 32 Hann-windowed grains with cubic interpolation, each tagged to a voice for its envelope. Pitch is the note divided by the source's reference pitch. Source is a mono buffer of up to 10 s. |
| Click | Per voice | A 2 ms raised-cosine noise burst through one band-pass set by `strike.color`, with an optional second burst at release. |
| Fold | Per voice | Sine wavefolder, y = sin(g x + bias), oversampled 2x through a half-band filter. |
| Clang | Per voice | Phase modulation of the partial bank by a sine at `clang.ratio` times f0, crossfaded with ring modulation. |
| Comb | Per voice | Two feedback combs tuned to f0 times `comb.tune`, a damping low-pass in each loop, allpass-interpolated fractional delay. |
| Smear (v1) | Per layer | Short-time Fourier transform, 1024 window, 256 hop. Blur smooths bin magnitudes over time and randomises phase; freeze holds magnitudes. Adds 21 ms at 48 kHz, compensated only while active. |
| Crush | Per layer | Fractional sample-and-hold plus a bit quantiser with dither. |
| Tape | Per layer | Modulated short delay with slow wow, fast flutter and random dropouts. |
| Filter | Per voice | Topology-preserving state-variable filter. `filt.morph` crossfades its low-pass, band-pass and high-pass outputs. Key tracking from `filt.track`. |
| Envelopes | Per voice | Attack, hold, decay, sustain, release with exponential segments. A separate decay for noise. A pitch envelope. A 3 ms choke for hard stops. |
| Motion | Per voice | Two LFOs and one smoothed-random chaos source, evaluated at control rate. |
| Delay | Per layer | Stereo delay up to 2 s. The feedback path has a low-pass and a two-tap crossfading pitch shifter. |
| Reverb | Per layer | Eight-line feedback delay network with Hadamard mixing, per-line damping and slow line modulation. Freeze sets feedback to unity and mutes the input. |
| Output | Per layer, then master | Mid-side width, level, pan. Master soft clip and DC blocker. |

### Vowel shape: the vocoder path

Live follow sounds like a vocoder because the engine is one: the voice supplies a spectral envelope and the Klont is the carrier.

- **Band layout.** `tone_bands` and `noise_bands` are 16 gains on the same log-spaced centres the capture front end measures.
- **Applying it.** Each partial reads its gain at its own frequency by linear interpolation in log frequency. Noise bands are scaled directly.
- **Mouth depth d.** The effective shape is (1 - d) times the stored shape plus d times the frame's vowel shape.
- **Voiced and unvoiced.** The frame's voicing flag crossfades the partial bank against noise over 10 ms, so "s" and "sh" come out as noise and vowels as partials.
- **Pitch is independent.** The carrier's pitch comes from the tuned pitch curve, so the same vowel shape can ride on autotuned, glissando or fixed notes.

### Drum model

A kit slot is a full parameter set and a hit is an ordinary voice. Nothing in a beat layer plays back a stored drum recording.

- **Body.** The partial bank in modal mode. Ratio tables: harmonic is 1, 2, 3 and so on; drumhead is 1, 1.59, 2.14, 2.30, 2.65, 2.92 and onward; bar is 1, 2.76, 5.40, 8.93, 13.34 and onward. `modal` interpolates between adjacent tables.
- **Ring.** Partial k decays with time constant equal to `env.decay` times its ratio raised to minus 2 times `damp`. High partials die first, as in a struck object.
- **Pitch drop.** `env.pitch_amt` and `env.pitch_time` sweep f0 at the start of the hit, which is what makes a kick or a zap.
- **Noise.** The shaped-noise path with `env.noise_decay`, independent of the body. A snare is a short body with a longer noise tail; a hat is noise with high bands only.
- **Click.** `strike.click` sets the start transient and `strike.end_click` the closing one, as in "nnt".
- **After the sources.** Fold, Comb, filter and Space apply as on any layer, so a drum can be folded, resonated, frozen in reverb or morphed.

Each hit bends its slot's parameters at resolution step 6:

| Hit shape field | Parameter it offsets |
| --- | --- |
| attack, burst | env.attack, strike.click |
| cents, bend | Voice pitch, env.pitch\_amt |
| hold | env.hold, env.decay, env.noise\_decay |
| vowel | tone\_bands tilt, f1\_freq |
| tone | harm\_mix against noise\_mix |
| endStop | Choke at the end of hold |
| endClick | strike.end\_click |

### Cost control

- **Worst case is 4,096 oscillators:** 8 layers, 8 voices, 64 partials. The engine skips partials above 0.45 times the sample rate or below -72 dB, and holds a global budget of active partials shared by loudness.
- **Overload policy.** When the CPU meter passes 80%, lower the partial budget first, then steal voices: oldest released, then quietest.
- **Denormals.** WebAssembly has no flush-to-zero mode, so every feedback path adds a tiny constant offset.
- **The partial budget is an estimate.** Milestone M1 benchmarks it on the target laptop before anything is built on top.

## Sequencer, transport and tuning

The engine owns time. It holds compiled tables for the whole track, advances a tick position per sample, and applies tuning to pitch curves with one function that is used both live and on stored takes.

### Compiled tables

- **Per section and layer:** a sorted array of note or hit events, a reference to the take's frame blob with its warp map, the morph markers, and the clip's loop length.
- **Per track:** the section order, repeat counts, tempos and ramps.
- **Per layer:** states, Influence target, macro matrix row cache, kit slots.

### Transport

- **Position** is a 64-bit float in ticks. Each sample advances it by bpm times 960, divided by 60 times the sample rate.
- **Tempo ramps** change bpm linearly across the ramp and are integrated per sample.
- **Free-time sections** advance in seconds and ignore bpm.
- **Section end.** Repeat, move to the next section, or take a queued jump. Jumps always land on a boundary.
- **Uneven loops.** Each layer reads at the section position modulo its own clip length.
- **Sample accuracy.** Each 128-frame quantum is split at event times and rendered in sub-blocks.

### Reading curves at play time

Every control tick, each sounding voice maps clip time back through the warp map to take seconds and reads the frame record by linear interpolation. Band levels interpolate in dB. A voice in its release keeps reading up to its note's end and then holds.

### Tuning

Autotune acts on the pitch curve, frame by frame. Every step is causal, so the same code runs during Live follow and over stored takes.

1. **Split.** A 3 Hz one-pole low-pass gives the slow pitch. The remainder is vibrato.
2. **Target.** The nearest scale degree to the slow pitch, with hysteresis: the target changes only once the pitch is 60% of the way to the next degree.
3. **Pull.** Move the slow pitch toward the target by Pitch Snap, then lag it by Tune speed, from 0 ms to 250 ms.
4. **Slides.** A slide is a slow-pitch rate above 400 cents per second for more than 40 ms. Glide suspends the pull so the slide stays smooth. Glissando forces full snap and zero lag, so the output steps through every scale degree the slide crosses. Jump holds the old note until the pitch has sat within 50 cents of a new degree for 60 ms, then switches at once.
5. **Vibrato.** Add the remainder back, scaled from 0 (flatten) through 1 (keep) to 2 (exaggerate).
6. **Gaps.** Unvoiced or low-confidence frames hold the last output.

Level out compresses loudness toward its running median by the Level out amount. For stored takes the tuned curve is recomputed in a worker whenever a Tune setting changes, which takes well under a millisecond for a 10-second take, and is sent to the engine as a new curve table.

## Capture pipeline

Capture analysis is split into a live tier that runs causally on the audio thread and a second pass that runs over the whole take after release. Live follow is the reason for the split.

&#91;embedded content: capture pipeline · live tier, second pass, three clip kinds\]

The live tier's frames are both what drives the synth during capture and what the take stores. The second pass adds structure on top of them.

### What Live follow demands of the analysis

The voice plays the synth while it is being sung. So every quantity that shapes the sound must be estimated without looking ahead, inside a fixed delay, and already in the synth's own terms. Fourteen requirements follow.

| # | Requirement | Design consequence |
| --- | --- | --- |
| 1 | Causal only | No smoothing that needs future audio. Viterbi pitch paths, "find the stable part of the note" and peak-picking with look-ahead are second-pass only |
| 2 | Source and filter separated in real time | The voice is decomposed each frame into pitch, level, spectral envelope and a voiced flag, as a vocoder does. Each is its own channel so each can be scaled or retargeted |
| 3 | The envelope must not contain the voice's own harmonics | Bands are wider than the harmonic spacing at low pitch. At high pitch a band that falls between two harmonics is filled from its neighbours |
| 4 | Shape separated from level | Band levels are divided by total energy, so loudness and vowel shape are independent channels |
| 5 | Neutral voice means no change | The singer's long-term average band shape is learned and subtracted. Mic colour and voice colour then do not tilt every preset |
| 6 | Onsets decided immediately | A rise threshold with a 40 ms refractory period, no look-ahead. A false trigger costs one quiet click live and is removed from the clip by the second pass |
| 7 | Sound before pitch is known | The first 20 ms of a note has no pitch estimate. The voice starts at once on the last pitch, or on noise after silence, and glides to the estimate over 10 ms |
| 8 | Octave slips without look-ahead | The pitch search is held within 7 semitones of the running median unless a new region persists for 30 ms |
| 9 | Silence must be silent | An adaptive noise floor and a gate with hysteresis, 5 ms attack and 60 ms release |
| 10 | The mic must hear only the voice | Browser echo cancellation is off, so speaker playback would enter the analysis. M0 prompts for wired headphones; the bleed detector it first used was removed because it muted Live follow with headphones on |
| 11 | Channels arrive with different delays | Level and vowel shape lead; pitch trails by about 12 ms. Live uses each as soon as it exists. Stored frames are shifted so channels line up |
| 12 | Output is parameters, not features | Frames map to parameter offsets through fixed linear maps, which is what lets every slider move |
| 13 | Tuning must be causal | Section 7's autotune uses only past frames. Glissando is free; Jump costs 60 ms |
| 14 | Some things cannot be known live | Note boundaries, scale, tempo, vibrato rate and kit grouping need the whole take. Live follow does not need them, because the synth follows the continuous voice |

Requirement 11 is the one deliberate difference between what is heard live and what plays back: playback has pitch and level aligned to within a frame.

### Live tier

The live tier runs inside the engine's process call, before the voices, so its output is used in the same block.

| Channel | Method | Delay |
| --- | --- | --- |
| Input | 60 Hz high-pass, then written to the mic ring | None |
| Vowel shape | 16 band-pass filters at the context rate, squared, smoothed with 5 ms attack and 15 ms release, in dB, normalized as in rows 4 and 5 | About 5 ms |
| Loudness | Weighted band sum in dB, scaled between the noise floor and a running ceiling | About 5 ms |
| Pitch | Decimate to 16 kHz. YIN with a 512-sample window and 80-sample hop, 70 to 1,000 Hz, parabolic refinement, continuity guard from row 8 | 16 ms plus up to 5 ms |
| Voicing | Pitch confidence, the share of energy above 4 kHz and zero-crossing rate, giving voiced, unvoiced or silent | About 10 ms |
| Openness | Centroid of the normalized bands between 300 Hz and 3 kHz, scaled by the singer's learned range | About 5 ms |
| Breathiness | Low pitch confidence at audible level, plus high-band noise share | About 10 ms |
| Onset and hardness | Summed positive band flux against an adaptive threshold. Hardness is the level slope in dB per ms at the trigger, refined 10 ms later for storage | One block |

### Delay budget for Live follow

| Stage | Delay |
| --- | --- |
| Input device and browser capture | 5 to 15 ms |
| Analysis, level and vowel shape | About 5 ms |
| Analysis, pitch | About 20 ms |
| One render quantum, 128 frames at 48 kHz | 2.7 ms |
| Output device | 5 to 20 ms wired |
| Total for level and timbre | About 20 to 45 ms |
| Total for pitch | About 35 to 60 ms |

These are estimates for wired audio, and milestone M0 measures them. Output delay is read from [`AudioContext.outputLatency`](https://web.dev/articles/audio-output-latency), an estimate available from Chrome 102, Firefox 70 and Safari 18.4. Bluetooth output adds far more and disables audible Live follow.

### Second pass

- **Pitch repair.** A pYIN-style tracker: several YIN candidates per frame and a Viterbi path over them. It replaces live pitch only where the two differ by more than 80 cents for 30 ms or longer. A small neural tracker is the bake-off alternative, judged on the hum corpus.
- **Timbre is never recomputed.** Loudness, vowel shape, openness and breathiness stay exactly as heard.
- **Kind detection.** From the voiced fraction, onsets per second, median event length and pitch stability. Thresholds are fitted on the corpus.
- **Notes.** Split a voiced run where the slow pitch moves more than 80 cents for 60 ms, where loudness dips by 6 dB, or at an onset. Minimum note 80 ms. Each note keeps its median pitch and its onset hardness.
- **Tuning reference.** The circular mean of pitch modulo 100 cents gives the singer's offset from concert pitch.
- **Scale.** A duration-weighted pitch-class histogram is correlated against every scale template at 12 roots. Ties go to the simpler scale.
- **Tempo and loop length.** An inter-onset histogram proposes tempos from 60 to 180 bpm. The winner minimizes snap error, and the loop is rounded to a whole number of beats.

### Beats

- **Events.** Each onset opens an event that ends when level falls 30 dB below its peak or the next onset arrives, capped at 2 s.
- **Hit shape.** Measured per event:

| Field | Measurement |
| --- | --- |
| attack | 10% to 90% rise time of level |
| burst | Share of energy above 4 kHz in the first 20 ms |
| cents, bend | Median pitch of voiced frames; slope of a line fit in semitones |
| hold | Event length |
| vowel | Mean openness across the body |
| tone | Voiced fraction of the body |
| endStop | Whether the final fall is faster than a set slope |
| endClick | A flux peak inside the last 40 ms |

- **Grouping.** A standardized six-feature vector per hit: log attack, burst, low-band share, centroid, tone and half-weighted log hold. Ward clustering is cut at the "kinds" value, defaulting to the count with the best silhouette. Groups are ordered by centroid and assigned to slots.
- **Teach my kit.** Stores a centre and spread per taught sound. New hits go to the nearest centre and to the spare slot when none is close.
- **Live slot choice.** Grouping needs the whole take, so Live follow uses the taught kit if there is one and brightness thirds otherwise. The second pass may move a hit to another slot.

### Devices and calibration

- **Constraints.** `getUserMedia` with echo cancellation, noise suppression and auto gain off, one channel. The audio context uses the interactive latency hint.
- **Round trip.** Five clicks are played and recorded; the median cross-correlation lag is stored per input and output device pair. A device change prompts a re-run.
- **Take alignment.** A mic sample's clip time is the transport time at capture, minus the round trip, with each channel shifted by its known analysis delay.
- **Per-mic memory.** Noise floor, long-term band shape and openness range are stored per input device.

## Sound Influence

Influence turns a sound into a target parameter set by direct measurement, updates that target several times a second while the sound is still playing, and optionally refines it afterwards by search.

### Instant match

The match works on feature frames plus a higher-resolution spectrum of the same audio (4,096-point transform, Hann window, 10 ms hop).

| Parameters | How they are measured |
| --- | --- |
| harm\_mix, noise\_mix | Energy at harmonic peaks against the residual after those peaks are removed |
| tilt | Least-squares slope of harmonic level in dB against log2 of harmonic number |
| oddeven | Mean level of odd harmonics minus even |
| stretch | Fit of measured peak frequencies to k times f0 times the square root of 1 + B k squared |
| modal, damp | For decaying sounds: best match of peak ratios to the three ratio tables, and the slope of per-partial decay time against frequency |
| tone\_bands, f1, f2 | Harmonic levels with the tilt removed, pooled into 16 bands; the two largest peaks between 200 Hz and 3.5 kHz |
| noise\_bands | The residual spectrum pooled into 16 bands |
| env.attack, hold, decay, sustain, release | Fit to the loudness curve: 10% to 90% rise, exponential fall to the plateau, time from offset to -40 dB. No plateau means sustain 0 |
| env.noise\_decay, strike.click | Decay of the residual alone; energy of the first 5 ms against the body |
| env.pitch\_amt, env.pitch\_time | Exponential fit to pitch across the first 150 ms |
| lfo rates and depths, chaos | Peaks between 0.5 and 20 Hz in the spectra of the pitch, loudness and centroid tracks. A broad spectrum with no peak goes to chaos |
| fold.amt, crush.rate | Flatness of the upper harmonics and crest factor; bandwidth below 6 kHz |
| verb.size, verb.mix, width | Slope of the tail after the source stops, used only if the tail exceeds 300 ms; inter-channel correlation |

Unpitched sources, where under 30% of frames are voiced, skip the harmonic rows. Noise bands come from the whole spectrum, and "use the sound itself" is switched on by default.

### Matching while it listens

The sliders move during Listen because the estimators are incremental.

- **Running sums.** Harmonic levels, band levels and residual energy are accumulated per frame, so tone parameters are available from the first 100 ms and sharpen as more arrives.
- **Event-driven fits.** Envelope and pitch-drop parameters update when an onset or an offset is seen.
- **Slow statistics.** Motion rates need at least two cycles, so they appear late and are flagged provisional until then.
- **Publish rate.** A new target goes to the engine every 250 ms. The engine's parameter smoothing turns the steps into glides.
- **On release** the same code runs once over the whole capture and the result replaces the running estimate.

### Refine (v1)

- **Free variables.** About 14 parameters that measurement only guesses: fold.bias, the Clang and Comb pairs, the filter set, tape.amt and jitter.
- **Search.** CMA-ES with a population of 12. Each candidate renders 0.75 s at 24 kHz through the offline engine.
- **Loss.** L1 distance between log-mel spectrograms at three transform sizes, plus half-weighted L2 distance between loudness envelopes.
- **Stop.** After 5 s of wall-clock time or five generations without improvement.
- **Audible progress.** Each improvement is sent to the live engine as a target update.

### Blending and storage

- **Blend** is resolution step 2. Aspect toggles are the mask over parameter groups.
- **Grain source.** With "use the sound itself" on, the capture is trimmed to its steady region, its reference pitch is stored, and `grain_mix` in the target is raised. Changing source crossfades two grain buffers over 50 ms.
- **Commit** writes the blended values into the state's base and clears the target.
- **Use my sound for a kit slot** runs the same match on one hit's audio, with the decaying-sound rows active.

## Interface

Controls are Svelte components over one full-window WebGL2 canvas. Anything that animates with the audio reads the meter block directly each frame and bypasses framework reactivity.

### State

| Store | Holds | Changes through |
| --- | --- | --- |
| `project` | The immutable project document | Commands only |
| `ui` | Selection, zoom, scroll, open panels | Direct sets |
| `live` | A typed view over the meter block and frame ring | Never written by the interface; read in the animation frame |

### Rendering

- **One renderer, retained geometry.** `packages/render` keeps vertex buffers per clip and rebuilds them only on edit or zoom. A frame with no edits uploads nothing.
- **Ribbons.** One triangle strip per note, with a vertex pair every 10 ms. Centre height comes from the tuned pitch curve, half-width from loudness, brightness from openness. The start cap is a signed-distance shape that sharpens with hardness.
- **Live ribbon.** During capture the newest frames are appended to a ring-buffered strip, so the ribbon grows under the voice with no rebuild.
- **Hits.** Instanced signed-distance shapes. Instance attributes are start hardness, tail length from hold, height from pitch, size from strength and tail tilt from bend.
- **Morph lane.** State markers as small Klont glyphs joined by a gradient strip that follows the transition shape.
- **The Klont.** A fragment shader on one quad: a circle's distance field displaced by three octaves of noise. Spike height follows Bright and Grit, slow wobble follows Motion depth and rate, halo radius follows Space, surface grain follows noise mix. Its inputs are the step-5 values from the meter block, so it moves with the voice.
- **Sliders.** DOM elements for accessibility. The live thumb's position is written as a transform inside the animation frame, not through component state.
- **Playhead.** Tick position from the meter block, extrapolated by the audio clock between updates.

### Input

- **Pointer events** for everything, with capture on drag.
- **Drags preview through the control ring** and commit one command on release.
- **Hold to capture.** Key down arms the layer and starts Live follow in the same frame. Key up closes the take and queues the second pass.
- **Klont gestures.** Pull maps to Bright, squash to Body, shake speed to Wobble. Each is a macro drag underneath.

### Robustness

- The audio context is created on the first user gesture and resumed on `statechange`.
- Device changes pause capture, keep playback, and prompt recalibration.
- Reduced-motion preference freezes the Klont shader's time input and disables bounce.
- Every control has an ARIA role and a keyboard path.

## Persistence

Everything is stored on the device: binary data in the origin private file system, the project document and its journal in IndexedDB, both written by one storage worker.

```text
OPFS
  projects/<id>/audio/<take>.f32     mono Float32 at the capture rate
  projects/<id>/frames/<take>.f32    24 floats per 10 ms
  projects/<id>/grain/<blob>.f32     Influence and texture sources
  projects/<id>/freeze/<layer>.f32   rendered layers
IndexedDB
  projects      latest snapshot of each project document
  journal       append-only patch groups since the snapshot
  settings      device calibrations, taught kit, per-mic memory
```

- **Why a worker.** The fast synchronous file handles exist only in workers. The [origin private file system](https://developer.mozilla.org/en-US/docs/Web/API/File_System_API/Origin_private_file_system) has been available across browsers since March 2023.
- **Autosave.** Each command's patch group is appended to the journal within 300 ms. Every 50 groups the snapshot is rewritten and the journal trimmed.
- **Crash recovery.** Load the snapshot, replay the journal, drop a trailing partial group.
- **Takes are written while recording.** The storage worker drains the mic ring to disk during capture, so a crash mid-take loses at most the last second.
- **Quota.** Storage is subject to the browser's quota and is deleted if the user clears site data. The app requests persistent storage, shows usage from `navigator.storage.estimate()`, and warns at 80%.
- **Size.** A minute of take audio is 11.5 MB at 48 kHz. Old takes outside the current stack are offered for deletion first.
- **Project file.** Export writes one zip: `project.json` plus every referenced blob. Import validates, migrates and copies blobs in.

### Schema migration

- `schema` is an integer on the document. Migrations are pure functions from version n to n + 1, run in order on load and on import.
- Parameter ids are stable strings. A parameter added later takes its `neutral` value in old projects, so they sound the same.
- A removed parameter stays in a tombstone list so its id is never reused.

## LLM service

One stateless function on the app's own origin holds the OpenAI key and exposes three endpoints. It receives text and numbers and returns JSON; audio never reaches it.

| Endpoint | Request | Response |
| --- | --- | --- |
| `POST /api/sound` | `{ text, current: ParamSet }` | `{ target: ParamSet, name }` |
| `POST /api/next` | `{ summary }`: sections, layers, kinds, tempo, scale, density | `{ suggestions: [{ label, command }] }`, three items |
| `POST /api/name` | `{ kind, summary }` | `{ names: string[] }` |

### Implementation

- **API.** The OpenAI Responses API with a strict JSON Schema in `text.format`, per the [Structured Outputs guide](https://developers.openai.com/api/docs/guides/structured-outputs).
- **Schema for `/api/sound`.** Generated by `paramgen`: one number per scalar parameter id and two 16-number arrays, all required, no additional properties.
- **Schema for `/api/next`.** A discriminated union over about 20 command names with typed arguments. These are the same commands the interface dispatches, so every suggestion is undoable.
- **Validation.** The function re-validates the reply, clamps every number to 0..1, and maps a refusal or incomplete reply to a typed error the interface shows as "no idea, try other words".
- **Prompt.** A fixed prefix with the generated parameter dictionary and twelve presets as description-to-values examples, then the user's text and current values. The fixed prefix is identical across calls so it can be cached.
- **Client behaviour.** The target arrives as an Influence target at 50%. If the function is unreachable, the feature hides itself.
- **Limits.** Text capped at 500 characters, a per-client rate limit, and no logging of user text.

### Model choice and cost

The model is a configuration value. The evaluation ladder runs the 50-prompt test from the PRD from cheapest upward and stops at the first model that reaches 80%.

| Model | Input, per million tokens | Output, per million tokens | Estimated cost per `/api/sound` call |
| --- | --- | --- | --- |
| gpt-6-luna | $0.10 | $0.50 | $0.0007 |
| gpt-6.1-terra | $1.00 | $5.00 | $0.007 |
| gpt-6.1-sol | $2.00 | $10.00 | $0.014 |
| gpt-6-astra | $10.00 | $50.00 | $0.07 |

Prices are from [OpenAI's pricing page](https://developers.openai.com/api/docs/pricing). The per-call figure assumes about 4,000 input tokens and 600 output tokens, which is an estimate to confirm once the prompt exists. OpenAI's guide uses gpt-6-astra in its examples; the ladder exists because this task may not need it.

## Export and offline render

Export runs the same engine module in a worker, calling `ew_render` in a loop instead of being driven by the audio clock.

- **Audio file.** The export worker loads the compiled tables, renders 48 kHz stereo in 4,096-frame chunks, and stops when the track has ended and output has stayed below -80 dB for one second, or 10 s after the end. Output is 24-bit WAV with dither, written to the origin private file system and then offered as a download.
- **Same sound.** Noise generators are seeded and LFO phases reset at render start, so two exports of one project are bit-identical and match live playback apart from free-running LFO phase.
- **Freeze (v1).** One layer is rendered solo for the section's loop length into `freeze/`. The engine swaps that layer for a streaming player and frees its voices. Unfreeze swaps back.
- **Stems (v1).** One solo pass per layer, same code.
- **MIDI (v1).** A type 1 file with one track per layer. Notes come from the tuned note list. Pitch bend carries glides, controller 11 carries loudness and controller 74 carries openness. Beat slots map to four fixed drum notes on channel 10.
- **Progress and cancel.** The worker posts progress per chunk and checks a cancel flag between chunks.

## Performance

Each PRD budget has a named mechanism and a measurement that runs in CI or on the reference laptop.

| Budget | Target | How it is met | How it is measured |
| --- | --- | --- | --- |
| Open to first sound, repeat visit | 10 s or less | Engine module under 1 MB, cached by a service worker; no model download on the main path | Playwright trace from navigation to first non-silent output block |
| Download before first sound | 5 MB or less | Analysis and ML modules load lazily on first capture | Bundle size check in CI |
| Control change to audible | 30 ms or less | Control ring drained every 2.7 ms block; parameter smoothing 5 to 20 ms | Loopback test: step a parameter, time the output change |
| Live follow, voice to synth | 50 ms or less, wired | Live tier inside the process call; no thread hop | Loopback: play a recorded hum into the input, cross-correlate with output |
| Hum stop to melody playing | 2 s or less for 10 s | Second pass works on 1,000 ready-made frames, not raw audio | Timer around the second pass on corpus takes |
| Influence instant match | 1 s or less | Running estimators already converged at release | Timer on the corpus |
| 8 layers playing | No dropouts, under 50% of one core | SIMD partial bank, partial budget, shared noise and space per layer | Offline render speed must exceed 4x real time on the reference laptop |
| Interface | 60 fps | Retained geometry; meter reads outside framework reactivity | Frame-time histogram during a scripted session |
| Export | Faster than real time | Same engine, no clock | Render speed logged per export |

The reference laptop is a 2020 Apple M1 MacBook Air running Chrome. Two numbers in this table rest on estimates that milestone M1 must confirm: the partial bank's cost and the offline render speed.

## Testing

DSP is tested against golden renders, analysis against labelled recordings, and the two headline qualities against listening tests.

| Layer | What is tested | How |
| --- | --- | --- |
| DSP primitives | Filters, oscillators, envelopes, FFT, resamplers | Rust unit tests against closed-form results |
| Engine | Whole-engine renders of 40 fixed projects | Golden files; a change must match within -90 dB or be approved by ear |
| Parameter system | Blend, macros, resolution order, smoothing | Property tests: blend at 0 and 1 returns the endpoints; every parameter stays in range; no clicks above a threshold on any morph |
| Live and stored parity | The live tier against the same code run over a file | Frames must match exactly; tuned curves must match exactly |
| Melody analysis | 100 hums by at least 10 untrained voices, hand-labelled notes | Note-level precision and recall; the 80% kept target |
| Beat analysis | 50 takes of taps and mouth sounds, hand-labelled hits | Detection rate, false hits per 8 bars, grouping agreement |
| Hit shapes | "Pop", "pope", "oom", "boom", "nnt" by each voice | Listeners name the played-back hit; target 80% |
| Voice gesture | "Mmmwaaa" by each voice, through every preset | Measured rise in output level and spectral centroid |
| Influence | 20 varied source sounds | Four-way blind match; target 70% at 100% |
| Timing | Overdub alignment, Live follow delay | Loopback rig |
| Interface | Capture, edit, undo, export flows | Playwright with Chrome's fake audio device fed from corpus files |
| Storage | Crash mid-take, journal replay, migration from every past schema | Automated kill-and-reload tests |
| Browsers | Chrome, Edge, Firefox, Safari on macOS and Windows | The Playwright suite plus a manual pass per release |

The corpus lives in `tools/corpus` with labels in plain text. It is recorded during milestone M0 and grows whenever a failure is found in use.

## Security and privacy

Audio never leaves the device, and the only network calls after load are the three LLM endpoints.

- **Isolation headers.** `Cross-Origin-Opener-Policy: same-origin` and `Cross-Origin-Embedder-Policy: require-corp` on every response. No third-party scripts, fonts or analytics, so nothing needs an exemption.
- **Content Security Policy.** `default-src 'self'`, with `wasm-unsafe-eval` for WebAssembly and `connect-src 'self'`.
- **Microphone.** Requested on the first capture, not on load. The mic ring lives in memory only; audio reaches disk only when a take is kept.
- **API key.** Stored as a server secret in the function. The single-user shortcut of a locally stored key is a build flag, off by default.
- **LLM payloads.** Text and parameter numbers only. No audio, no project names unless the user asks for a name.
- **Imported project files** are treated as untrusted: size-capped, schema-validated, and blobs checked for length before use.

## Build order

Six milestones lead to the MVP, each closed by one measurable gate, and nothing past M0 starts until both prototypes pass.

&#91;embedded content: build roadmap · six milestones, one gate each\]

M0 is throwaway code that answers two questions: does singing into the page feel good, and does a matched sound resemble its source. M1 and M2 can overlap, because the live tier only needs the DSP crate. M3 onward needs both. No durations are given here because team size is not yet known.

What M0 must contain, given Live follow: the full live tier, a one-layer engine with the partial bank, shaped noise and vowel shape path, the tuning function, and the loopback delay measurement. That is the smallest thing that can prove or kill the vocoder-like feel.

## Open technical questions

- [ ] Second-pass pitch: pYIN-style or a small neural tracker? Decided on the hum corpus in M0.
- [ ] Are 16 bands enough for vowels and words to come through in Live follow, or does it need 24? Decided by ear in M0.
- [ ] What partial budget does the reference laptop sustain with 8 layers? Measured in M1.
- [ ] Is a reference-based echo canceller needed so Live follow can sound on speakers?
- [ ] For untaught kits, is brightness-based live slot choice good enough, or should the first beat capture prompt Teach my kit?
- [ ] Rust is assumed for the engine. If the builders know C++ better, the same design holds with Emscripten.
- [ ] How far does Safari go in the MVP: full support, or playback and editing with capture marked experimental?
- [ ] Which host for the static site and the function? Any that sets the two isolation headers works.

## Sources

Pages opened on 16 November 2026 for platform and pricing facts. The algorithms named in this document (YIN, pYIN, CMA-ES, feedback delay networks, state-variable filters, modal ratios) are standard signal-processing methods described from general knowledge, not looked up for this document.

- [Chrome for Developers: SharedArrayBuffer and cross-origin isolation](https://developer.chrome.com/blog/enabling-shared-array-buffer). Required headers and their effect on cross-origin resources.
- [Web Features Explorer: WebAssembly fixed-width SIMD](https://web-platform-dx.github.io/web-features-explorer/features/wasm-simd). Baseline status and browser versions.
- [MDN: Origin private file system](https://developer.mozilla.org/en-US/docs/Web/API/File_System_API/Origin_private_file_system). Availability, worker-only synchronous handles, quota behaviour.
- [web.dev: audio output latency](https://web.dev/articles/audio-output-latency). What `outputLatency` reports and where it is supported.
- [OpenAI: Structured Outputs guide](https://developers.openai.com/api/docs/guides/structured-outputs). Schema-constrained replies in the Responses API. Only the first part of this long page was read.
- [OpenAI: API pricing](https://developers.openai.com/api/docs/pricing). GPT-6 family prices.
- The [Earworm PRD](https://claude.ai/artifact/3ZXQPGZdEtMPEqWRHxcQvi) and its own source list.
