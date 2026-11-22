# Synth engine: patches and modulation

Nov 20, 2026. Design approved. Stage S1 is built and this page now describes it as built (the owner accepted the changes [S1.md](S1.md) lists). Stage S2 is built; [S2.md](S2.md) says how to check it and where it differs from this page. Stage S3 is built; [S3.md](S3.md) does the same for it. Stage S4 is built; [S4.md](S4.md) does the same for it. Stage S5 is built, with the raw input beside the synth; [S5.md](S5.md) does the same for it.

Today a layer's sound is one fixed chain, the Klont voice, set by 96 numbers, and the app offers a short list of presets to pick from plus eight macros. This page turns that into an open patch: a small graph of modules the user can add, remove and wire, and one modulation system in which every envelope, LFO, knob and sung feature (pitch, intensity, volume, hardness) can drive any synth or channel-effect parameter. The aim is a tool that works as a real-time DAW and as an instrument for improvising.

It supersedes the parts of [TDD.md](TDD.md) that assume a fixed parameter table and a fixed resolution order. Those sections of the TDD now point here.

## Summary

| Concern | Decision |
| --- | --- |
| Sound | A **patch**: modules from a fixed catalog, wired as a graph, plus modulation routes. Today's Klont chain is one patch, the **classic patch**. |
| Scopes | A **voice** section runs once per sounding voice (mono). A **channel** section runs once per layer on the summed voices (stereo). Audio flows voice to channel, never back. |
| Structure and values | Structure (which modules, cables and routes) belongs to the layer. Values (every knob, every route amount) belong to the sound state. States of one layer share one structure, so morphing stays a blend of numbers. |
| Modulation | One shape for every modulation: **source, shaper, target**. Sources include envelopes, LFOs, note data, macros, MIDI expression and the sung features. The shaper (range, curve, smoothing, latch) is the "universal envelope". |
| Sung features | Pitch, volume, intensity, hardness, openness, breathiness, onset, voicing and the vowel shape, all from the feature frames the engine already makes. Intensity is new and is derived from the existing frame, so stored takes gain it too. |
| Where it runs | Every module, source and route runs in Rust in the AudioWorklet. TypeScript owns the document, validates and compiles patches, handles Web MIDI and draws the interface. |
| Presets | A preset is a patch plus values, saved as JSON. Today's ten presets become factory presets of the classic patch and sound the same. |
| Old projects | Schema 4. A migration turns every layer into the classic patch with today's values and today's fixed gesture maps as default routes. |
| DAW and live | One engine and one patch format. They differ in where notes and control values come from (stored clips and lanes, or the mic, MIDI and screen) and in what gets recorded. |

What does not change: one engine binary for live, export and matching; the feature-frame format; the sequencer living in the engine; commands and undo; no allocation, locks or waits on the audio thread; parameter values stored normalized 0 to 1 and described by one generated table.

## What exists today

| Piece | Where | Fixed today |
| --- | --- | --- |
| 64 scalar parameters and two 16-band shapes | `crates/params/params.toml` | The list, and that every sound uses all of them |
| Voice: partial bank, click, clang, fold, comb, filter, envelopes, two LFOs, chaos | `crates/klont/src/voice.rs` | The order of the chain |
| Shaped noise and grain cloud, per layer, driven by the voices' envelopes | `crates/klont/src/layer.rs`, `grain.rs` | That they bypass the voice filter |
| Layer effects: crush, tape, delay, reverb, width | `crates/dsp/src/layer_fx.rs`, `Layer::render` | The order, and one of each |
| Gesture maps: loudness to amplitude, openness to tilt, F1 and cutoff, breathiness to noise, hardness to click and attack, mouth to the band shapes | `gesture_offsets` and `hit_start` in `layer.rs` | Which parameter each feature moves, by how much |
| Hit-shape maps on beat layers | `hit_start` in `layer.rs` | Same |
| Macros: a sparse weight matrix over 31 parameters | `macros = {...}` lines in `params.toml` | The weights |
| Presets: eight melody sounds and two beat kits | `web/src/m2/sounds.ts` | The list in the Sound panel's Preset menu |
| Sound panel: preset, level, pan, states, eight macros, five gesture depths | `web/index.html`, `web/src/m3/panels.ts` | No access to single parameters |

The M0 pages (`live.html` and the others) run their own older engine and keep their own presets. They are test pages and stay as they are.

## Patches

### Terms

- **Module.** One processing unit from the catalog: an oscillator, a filter, a delay. It has typed inputs, one output and a set of parameters.
- **Cable.** An audio connection from one module's output to another module's input inside the voice section. Several cables into one input are summed.
- **Channel.** The layer's effect chain: an ordered list of effects on the sum of its voices, with level and pan between the mono effects and the first stereo one, and width last.
- **Mod source.** Anything that produces a control value: an envelope, an LFO, the note's velocity, a macro, the sung volume.
- **Route.** One row of the modulation matrix: a source, a shaper, an amount and a target.
- **Value.** One stored number, normalized 0 to 1: a module parameter, a route amount, an envelope time.

### The two scopes

A layer plays up to 8 voices. Work that has to happen per note runs per voice. Work on the mix runs once per layer. Today's engine already has this split, and the patch keeps it.

```text
         per voice (x8, mono)                         per layer (stereo)
 ┌──────────────────────────────────────┐
 │ sources → processors → amp → out ────┼──┐
 └──────────────────────────────────────┘  │
 layer sources (shaped noise, grains) ─────┼──→ sum → mono fx → level, pan → stereo fx → width → master
                                           │
 (each layer source is gated by the voices' envelopes, as today)
```

- **Voice section.** A directed graph without cycles. Mono, 32-sample control ticks, audio processed in spans that never cross a tick. Ends at the voice output, which is summed into the layer bus after the voice's choke gain.
- **Layer sources.** Shaped noise and grains stay per layer because they are expensive per voice (16 band-pass filters, 32 grains). Their level follows the sum of the sounding voices' chosen envelope, exactly as today. They go straight into the layer bus.
- **Channel section.** An ordered chain, not a free graph: effects in series, each with a bypass and its own mix. The mono effects (crush, tape) come first and run on the voice sum; level and pan then spread it to stereo for the stereo effects (delay, reverb); width is last. This is the old engine's order, so a panned layer with delay or reverb sounds as it did.

Audio cannot flow from the channel back into a voice. Modulation can reach channel parameters, but only from layer-scope sources (see "Modulation").

### Module catalog

Every module is built from code that exists today, except the four marked new.

| Module | Scope | Inputs | Parameters | From |
| --- | --- | --- | --- | --- |
| `partials` | voice | none (plays the voice pitch) | level, tilt, odd/even, stretch, jitter, modal, damp, F1 and F2 frequency and gain, tone bands (16), octave, fine | Partial bank in `voice.rs` |
| `osc` (new) | voice | `pm` (phase modulation) | level, wave (sine, triangle, saw, square as one morph value), pulse width, octave, semitone, fine, pm depth | New, band-limited with a four-sample polyBLEP and polyBLAMP |
| `noise` (new) | voice | none | level, colour (white to pink to brown as one morph value) | `dsp::noise` |
| `sampler` (new) | voice | none | level, start, loop (off or on), loop start, loop length, pitch follow, octave, fine | Cubic read from the grain code |
| `click` | voice | none | start amount, end amount, colour | `dsp::fx::Click` |
| `filter` | voice | `in` | cutoff, resonance, morph (low-pass, band-pass, high-pass), key track | `dsp::filters::Svf` |
| `amp` (new) | voice | `in` | gain | A gain stage that ramps across each tick |
| `fold` | voice | `in` | amount, bias | `dsp::fx::Folder` |
| `clang` | voice | `in` | amount, ratio | `dsp::fx::Clang` |
| `comb` | voice | `in` | amount, tune, decay | `dsp::fx::CombPair` |
| `shaped_noise` | layer source | none | level (each voice's share of the bank), noise bands (16, per layer) | `ShapedNoise` in `layer.rs` |
| `grains` | layer source | none | level (each voice's share), size, density, position, spray, scatter | `grain.rs` |
| `cfilter` | channel | mono or stereo, where it sits | cutoff, resonance, morph | Two `Svf` |
| `input` (S5) | channel | the raw input (mic or take audio) | mix, trim | The mix point; see "The raw input beside the synth" |
| `crush` | channel | stereo | bits, rate, mix | `dsp::layer_fx::Crush` |
| `tape` | channel | stereo | amount | `dsp::layer_fx::Tape` |
| `delay` | channel | stereo | time (seconds or beats), feedback, shift, mix | `dsp::layer_fx::StereoDelay` |
| `reverb` | channel | stereo | size, damp, mix, freeze | `dsp::layer_fx::Reverb` |

Level and pan sit between the mono and the stereo effects, and width ends every channel. Width is a value like any other, so it can be modulated and automated. Level and pan are layer fields, not values: automation lanes can move them (S5), routes and scenes cannot.

`sampler` and `grains` read a **sample**: a mono buffer in the sample pool (see "Engine changes"). In S3 each layer has one, its Influence source (a file, a recording or a saved source): both the grains and a sampler play it.

Smear stays a v1 item, as in the TDD. When it is built it is a channel effect.

### Rules for the graph

- **No cycles in the voice section.** Feedback lives inside modules (comb, delay), not in cables. The rack lists the modules in any order; the compiler sorts them so each runs after the modules that feed it, refuses a loop, and the engine runs them in that order (a cable may only run forward in the blob).
- **One output per module, mono in the voice section.** An input with several cables sums them at unity. An `amp` sets a level.
- **Audio-rate modulation only through cables.** An `osc` reads its `pm` input every sample, so one oscillator can frequency-modulate another. Every other modulation runs at the control rate (every 32 samples, 0.67 ms at 48 kHz).
- **Pitch.** Each voice has one pitch: the note, the tuned curve or the live voice. Pitched modules play that pitch plus their own octave and fine values plus any pitch routes.
- **Caps.** Per voice patch: at most 16 modules (at most 1 `partials`, 4 `osc`, 2 `noise`, 1 `sampler`, 1 `click`, 2 `filter`, 4 `amp`, 1 `fold`, 1 `clang`, 1 `comb`), 4 envelopes, 4 LFOs and 48 routes. Per layer: one `shaped_noise` per voice patch (so up to 4 on a beat layer, as today's five noise banks allow) and one `grains`, 8 channel effects (at most 2 delays and 1 reverb), 2 layer LFOs, 64 layer routes and 1,024 values. The engine allocates every instance at start-up, so a patch that fits the caps loads without allocating.
- **Cost.** Each module type has a cost per sample (`cost` in `modules.toml`, measured by the benchmark's `costs` scene), counted for every module the patch holds whether its values switch it on or not. Voice modules, envelopes, LFOs, routes and a partial bank that some route makes rebuild every tick cost per voice; layer sources, channel effects, layer LFOs and layer routes once per layer. A layer may spend what the classic kit costs with 8 voices; a patch gets the voices its per-voice cost leaves room for after its layer cost, 1 to 8. The classic patches get 8. The patch editor shows the cost and the voice count. Measured in S3, this budget does not keep the M1 gate true for every patch: the classic structure with every module switched on already costs about 3.5x real time on the build machine (see [S3.md](S3.md)).

### Structure per layer, values per state

A layer has one patch structure. Each sound state holds a value for every key in it. The morph lane blends two states' values, as it does today, because both states have the same keys.

- **Keys.** A value's key is `<id>.<param>`: `filter1.cutoff`, `env2.decay`, `delay1.mix`. Ids are unique within the layer. On a beat layer the editor names kit-slot modules with the slot, as in `k2.filter1`, and a layer route can target the same key in all four slots at once as `k*.filter1.cutoff`, counted as one route (today's macros act on every kit slot this way). A route's amount is the value `route.<id>`, so a state can hold a deeper or shallower modulation than another.
- **Stepped values.** A few values are steps, not a continuum (sampler loop on or off, LFO sync on or off). A morph switches them at its midpoint. Filter type and oscillator wave stay continuous morphs, as the TDD intended.
- **Changing the structure** (adding a module, rewiring, adding a route) is an edit to the layer, not to a state. Every state gets the new keys at their neutral values. It is not something a morph or a performance does. While the layer sounds, the engine plays on with the old structure and fades the layer's output (voices and channel tails) to silence over 5 ms, then takes the new one; a note that starts during the fade starts when it is taken. Bypassing a channel effect and changing a route's shaper are not structure changes.

### Beat layers and kit slots

Each of the four kit slots has its own voice patch: a sampled hat, a partials kick and a noise snare can share a layer. The channel is shared. The states hold the values of all four slots and the channel, so a morph can move the kit too. (Today the kit sits outside the states; migration copies the kit's values into every state, so old projects do not change.)

### Serialization

The patch is part of the project document (schema 4). Values stay normalized 0 to 1.

```ts
interface Layer {
  // id, name, kind, color, level, pan, mute, tune, depth, follow, influence: unchanged
  patch: Patch;
  states: State[];
  kit: { cents: number }[];        // beat layers: default pitch per slot; the sound is patch.voices[slot]
}
interface State {
  id: string; name: string;
  values: Record<string, number>;  // every key of the layer's patch, 0..1; replaces base: number[]
  macros: number[];                // eight, 0.5 neutral
  influenceAmount: number;
}
interface Patch {
  voices: VoicePatch[];            // 1 for melody and texture, 4 for beat (one per kit slot)
  layerSources: LayerSource[];
  channel: ChannelFx[];            // signal order
  lfos: ModDef[];                  // layer-scope LFOs
  routes: Route[];                 // layer-scope routes
  macroNames: string[];            // eight
  descs?: Record<string, string>;  // a key's descriptor where it is not the module table's default
  defaults?: Record<string, number>; // a key's starting value (real units) where it is not the neutral
}
interface VoicePatch {
  modules: { id: string; type: VoiceModuleType }[];   // rack order; a sampler plays the layer's sample
  cables: { from: string; to: string; port: 'in' | 'pm' }[];
  out: string[];                   // modules summed into the voice output
  mods: ModDef[];                  // env1..env4, lfo1..lfo4
  routes: Route[];                 // per-voice routes
}
interface LayerSource { id: string; type: 'shaped_noise' | 'grains'; voice: number; drive: string; sample?: BlobRef }
interface ChannelFx  { id: string; type: ChannelFxType; bypass: boolean; path?: 'synth' | 'raw' }  // path: before an `input` element (S5)
interface ModDef     { id: string; type: 'env' | 'lfo'; trigger?: 'note' | 'onset' }
interface Route {
  id: string;
  source: string;                  // 'env1', 'lfo2', 'note.velocity', 'voice.volume', 'layer.intensity', 'macro3', 'wheel'
  target: string;                  // a value key, 'pitch', 'bands:<key>' (a 16-band shape) or 'oct:<key>' (a log-scaled value moved in octaves)
  mode: 'add' | 'scale' | 'pull';
  inLo: number; inHi: number;      // the part of the source's range that is used
  bend: number;                    // -1..1, 0 is a straight line
  points: [number, number][] | null; // up to 8; replaces bend when set
  riseMs: number; fallMs: number;
  latch: boolean;                  // sample once at note start
  polarity: 'up' | 'down' | 'both';
  via: string | null;              // a second source that scales the amount
  group: 'loud' | 'open' | 'hard' | 'breath' | 'mouth' | 'follow' | null;
  unit?: number;                   // target units per unit of amount (default 1; 4800 cents for pitch)
}
interface Note     { /* id, t, dur, cents, hard: unchanged */ vel: number }  // 0..1
interface ClipBase { /* unchanged */ auto: AutoLane[] }
interface AutoLane {
  target: string;                  // a value key, 'macro3', 'layer.level', 'layer.pan', or 'wheel', 'bend', 'pressure'
  points: { t: number; v: number; shape: 'line' | 'ease' | 'step' }[];  // t in section units
}
```

A **preset file** is one layer's sound without the notes:

```json
{
  "format": "earworm-patch", "v": 1,
  "name": "Choir pad", "kind": "melody",
  "patch": { "voices": [ ... ], "layerSources": [ ... ], "channel": [ ... ], "lfos": [], "routes": [ ... ], "macroNames": [ ... ] },
  "values": { "partials1.tilt": 0.5, "filter1.cutoff": 0.81, "route.r3": 0.7 },
  "macros": [0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5],
  "kitCents": null
}
```

Loading a preset replaces the layer's patch and the edited state's values in one undoable command; the other states get the preset's values too, so the layer stays consistent. Missing keys take the patch's starting value (`defaults`, else the descriptor's neutral); unknown keys are dropped. Factory presets ship as JSON files in `web/src/presets/`. User presets live in the IndexedDB settings store and export and import as single files.

A patch may give a key another descriptor of the same kind (`descs`) and its own starting value (`defaults`). The classic patch uses both, so the old parameters keep their ranges and old values map over unchanged.

### The module table

`crates/params/params.toml` today lists 96 parameters. It becomes `crates/params/modules.toml`: one entry per module type, mod-source type and channel effect, each with its inputs and its parameters described exactly as parameters are now (`id`, `scale`, `min`, `max`, `neutral`, `unit`, `smooth_ms`, `doc`, and the Influence `group`). Two new fields:

- `steps`: for stepped values, the number of steps.
- `scalable`: true for gains, levels and mixes, the only values a `scale` route may multiply.

`paramgen` keeps its job: it emits the Rust descriptors, the TypeScript types and slider metadata, and the parameter dictionary. Ids stay stable and are never reused; a parameter added later takes its `neutral` value in old patches.

The old 96 ids remain as the **Klont vocabulary**: a fixed list, each mapped to a module type and parameter (`src.tilt` to `partials.tilt`, `filt.cutoff` to `filter.cutoff`, `env.attack` to `env1.attack`, `verb.mix` to `reverb.mix`, and so on). Influence matching and the future LLM function speak that vocabulary (see "Influence and the LLM function").

### The classic patch and today's presets

The classic patch is today's chain, written as a patch. Nothing about its sound changes.

```text
voice:   partials1 → amp1 ─┐
         click1 ───────────┴→ clang1 → fold1 → comb1 → filter1 → out
         env1 (amp, ADSR), env2 (noise, decay mode), env3 (pitch drop, one-shot), lfo1, lfo2, lfo3 (drift: today's chaos)
layer:   shaped_noise1 (driven by env2), grains1 (driven by env1)
channel: crush1 → tape1 → level, pan → delay1 → reverb1 → width
```

The ten presets in `web/src/m2/sounds.ts` (Choir pad, Reed, Glass, Buzz, Breath, Bell, Demo bass, Demo lead, and the two kits) are converted once by a script into classic-patch preset files: same structure, their values moved to the new keys, today's gesture, hit-shape, LFO and macro maps as default routes. The Preset menu lists them first, then the user's own.

## Modulation

### One shape for every modulation

Every modulation, from an envelope opening a filter to the sung volume driving a delay's feedback, is the same three parts:

```text
 source ──→ shaper ──→ amount × group depth × via ──→ target
 (0..1)     range, curve, smoothing, latch, polarity
```

The shaper is the "universal envelope": it says how a source becomes a movement. The same editor shapes an ADSR, a sung pitch, a mod wheel or a macro.

### Sources

Each source gives one value per control tick. Unipolar sources run 0 to 1. Bipolar sources (LFOs, pitch bend) run -1 to 1 and enter the shaper as 0 to 1.

**Per voice** (each voice has its own value):

| Source | What it is |
| --- | --- |
| `env1` to `env4` | A mode, then attack, hold, decay, sustain, release, with a curve per envelope. Modes: ADSR (note-off starts the release); decay (starts at full and falls with decay; while `env1` releases, the release adds to the fall); one-shot (starts at full, falls with decay, ignores note-off). Trigger: the note, or each sung onset. `env1` decides when the voice ends. |
| `lfo1` to `lfo4` | Rate in Hz, or in beats when synced. Wave is a morph: sine, triangle, saw, square, random steps, drift (today's chaos). Phase restarts on each note unless set to run free. |
| `note.pitch`, `note.velocity`, `note.hardness` | The note's pitch, velocity or hit strength, and hardness (a stored note's `hard`, or the onset hardness of a live voice). |
| `note.attack`, `note.burst`, `note.bend`, `note.hold`, `note.vowel`, `note.tone`, `note.endClick` | A beat hit's shape fields, as measured by the second pass. Fixed for the note. |
| `voice.<feature>` | The sung features, read from the voice's own take curve at its clip time, or from the mic for the voice Live follow plays. Listed below. |

**Per layer** (one value for the whole layer):

| Source | What it is |
| --- | --- |
| `macro1` to `macro8` | The state's eight macros. |
| `wheel`, `bend`, `pressure` | MIDI mod wheel, pitch bend and channel pressure, or their on-screen stand-ins. |
| `layer.lfo1`, `layer.lfo2` | LFOs that run once per layer, for channel targets. |
| `layer.<feature>` | The sung features of the layer's feature frame: the mic when the layer listens to it, otherwise the take curve at the transport position. |

**The sung features.** All come from the 10 ms feature frames the live tier already computes, so they work the same live and on stored takes.

| Feature | Frame source | Range | Notes |
| --- | --- | --- | --- |
| `pitch` | Tuned pitch (curve slot 24), else frame slot 0 | Cents above C0; the shaper's range is set in notes (C3 to C5) | Holds through unvoiced frames, like the tuner. |
| `volume` | Loudness, slot 2 | 0 to 1 between the noise floor and the running ceiling | What the TDD calls loudness. |
| `intensity` | Derived: mean of the bands from about 2 to 6 kHz minus the mean of the bands from about 300 Hz to 800 Hz | -10 dB to +10 dB mapped to 0 to 1; 0.5 is the singer's usual tilt | New. Vocal effort: pressing or belting adds high harmonics at any level, so it is separate from volume. The bands already have the singer's average removed (live tier requirement 5), so no new calibration is needed. Computed when the frame is read, so the frame format and every stored take stay as they are. |
| `hardness` | Slot 6 | 0 to 1 | The attack of the most recent onset (level slope at the trigger). Held until the next onset. |
| `openness` | Slot 3 | 0 to 1 | Vowel brightness, as today. |
| `breath` | Slot 4 | 0 to 1 | As today. |
| `onset` | Slot 5 | 0 to 1, for one frame | A pulse. Useful as an envelope trigger or with a slow fall time. |
| `voicing` | Slot 7 | 0 voiced, 0.5 unvoiced, 1 silent | |
| `vowel` | Slots 8 to 23 | 16 band levels in dB | Only for `pull` routes into a 16-band shape, which move the shape toward the sung vowel (today's Mouth). |

Softness and hardness are covered by two features on purpose: `hardness` is the attack of a sound, and `intensity` is how pressed or bright it is while it lasts.

### The shaper

| Field | Meaning |
| --- | --- |
| `inLo`, `inHi` | The part of the source used. Below `inLo` reads 0, above `inHi` reads 1. Pitch shows these as notes, times as ms. |
| `bend` | -1 to 1. 0 is a straight line; positive bows the curve up, negative down. |
| `points` | Up to 8 points drawn in the editor, joined by straight lines. When set they replace `bend`. |
| `riseMs`, `fallMs` | One-pole smoothing, separate for rising and falling values. 5 ms rise and 300 ms fall turns the sung volume into an envelope follower. |
| `latch` | Take the value once when the note starts and hold it. A hardness route that sets the attack time is latched. |
| `polarity` | `up` adds 0 to 1, `down` adds -1 to 0, `both` adds -1 to 1. |
| amount | The value `route.<id>`, -1 to 1, times the route's `unit` (target units per unit of amount, 1 unless set). For `add` routes, 1 moves the target across its whole range. For pitch the unit is 4800 cents, so 1 is 48 semitones. A fixed constant of an old map rides in `unit` with amount 1, which keeps it exact. |
| `via` | Optional second source that scales the amount: the mod wheel deciding how much vibrato there is. |
| `group` | Optional: `loud`, `open`, `hard`, `breath`, `mouth` or `follow`. The route's amount is also scaled by that group's depth, which is today's Gestures sliders and Follow. |

Evaluation per route, per tick:

```text
u = clamp((x - inLo) / (inHi - inLo), 0, 1)
y = points ? lerp(points, u) : curve(u, bend)
y = latch ? y_at_note_start : smooth(y, riseMs, fallMs)
c = polarity == up ? y : polarity == down ? y - 1 : 2y - 1
k = amount × unit × depth(group) × via

add:    target += k × c          (normalized domain)
scale:  target ×= 1 + k × c      (real units; scalable values only)
pull:   target += k × (s - target)  (the target's real units; s is the source value: the raw value
                                     clamped to inLo..inHi, or the curve's y spread across that range;
                                     for a 16-band shape, per band)
oct:    target ×= 2^(k × c)      (a log-scaled value, after mapping; may pass the value's own range
                                     up to what the module allows)
```

A latched pull holds what it set at note start. A route whose target is another route's amount adds to that amount in the amount's own units, by the source value itself (the hit's bend deepens the pitch envelope in semitones); such a route comes before the route it moves.

Smoothing and latch state start at the source's value when a voice starts, so a note does not begin with a sweep.

### Targets

- Any value of any module in the voice patch, including the voice pitch (`pitch`). A log-scaled value can also be moved in octaves after mapping (`oct:<key>`), so an LFO can sweep a filter past 20 kHz up to the filter's own limit, as the old LFO maps did.
- Any parameter of the envelopes and LFOs (times, rates, curves), from any source except an envelope or an LFO.
- Any channel value: effect parameters and width (level and pan are layer fields, which only automation lanes move). Only layer-scope sources may target the channel, because the channel is one per layer and a per-voice value has no single meaning there.

### Order of evaluation

This replaces the TDD's eight-step resolution order. Steps 1 to 4 run once per layer per tick; steps 5 to 7 run per voice.

1. **Morph.** Blend the two neighbouring states on the morph lane: values, macros, Influence amount. Unchanged.
2. **Automation.** Each automation lane on the clip replaces its value. New; see "DAW use".
3. **Influence.** Blend toward the Influence target per aspect group. Unchanged.
4. **Layer routes.** Add every route whose source is per layer (macros, wheel, bend, pressure, layer LFOs, layer features), then clamp. Channel values are finished here: smooth with each value's `smooth_ms` and map to real units. Macro routes are what the macro weight matrix was. A slider's fixed thumb shows the value after morph, automation, Influence and macros, as it does today; the live thumb shows it after step 6.
5. **Voice start.** The voice copies the layer's values (or its kit slot's).
6. **Voice routes.** Evaluate every route whose source is per voice (envelopes, LFOs, note data, voice features). Sum the add routes into the voice's values, then apply the pull routes.
7. **Clamp and map.** Clamp to 0 to 1, map to real units, then multiply by the scale routes. The layer values were already smoothed in step 4; envelopes, LFOs and frame reads are smooth by construction, and modules that are sensitive to steps (gains) ramp across the tick.

Old steps 5, 6 and 7 (gesture, hit shape, modulators) all adjust the voice's values on top of the layer's, so step 6 does them in one pass.

**No cycles.** Envelopes and LFOs cannot modulate envelope or LFO parameters, and no other source reads a modulated value. So no source can depend on itself, and the order inside a tick is fixed: sources that nothing modulates, then envelopes and LFOs, then module values.

### Today's fixed maps, as default routes

Every migrated layer and every factory preset carries these routes. Their amounts are today's constants; the group depth is today's slider. Golden renders (build stage S1) decide that the sound did not change.

| Today | Route |
| --- | --- |
| Loudness to amplitude, `(1 - d) + d × l` | `voice.volume` → `amp1.gain`, scale, polarity down, amount 1, group loud |
| Openness to tilt +0.4, F1 +0.25, cutoff +0.2 | three routes from `voice.openness`, add, group open |
| Breathiness to noise +0.5 | `voice.breath` → `shaped_noise1` level, add, group breath |
| Mouth | `voice.vowel` → `partials1.tone_bands`, pull, group mouth. Today's code also writes the noise bands per voice, but the noise bank reads the layer's bands, so only the meters show it. A `layer.vowel` route to `shaped_noise1.bands` would make it audible; it is not a default, so the sound does not change. |
| Hardness to click +0.5, attack -0.3 | `note.hardness` → `click1.start` and `env1.attack`, latched, group hard |
| Velocity and the envelope on amplitude | `note.velocity` and `env1` → `amp1.gain`, scale, polarity down |
| Pitch envelope | `env3` (one-shot, decay = old pitch time) → `pitch`, amount = old pitch amount, unit 100 cents |
| LFO 1 to pitch, filter, level; LFO 2 to pitch, filter, grain; chaos to pitch and filter | eight routes from `lfo1`, `lfo2`, `lfo3`; the filter routes move the cutoff in octaves (`oct:filter1.cutoff`) |
| Hit shape on beat layers | eleven latched routes from the `note.*` hit fields, group follow. Attack and tone move the target toward the hit's value (the attack in seconds), so they are pull routes; the rest are add routes. |
| Macro weights (34 non-zero weights over 31 parameters) | one route per weight from `macro1`..`macro8`, polarity both, unit W and amount 1 (an add route moves by amount × (2u - 1), so the old weight W is the amount) |

Two of today's rules swap values rather than offset them, so they stay fixed rules of the voice instead of becoming routes:

- **Voicing swap.** On unvoiced frames, pitched sources (`partials`, `osc`, `sampler`) fade out over 10 ms and noise sources rise to at least the pitched level, as `gesture_offsets` does today. One value per voice patch, `voicing.depth`, scales it (1 by default).
- **End stop.** A beat hit with `endStop` chokes at the end of its hold.

### Cost of modulation

Routes from layer sources are evaluated once per layer and the result is added to each voice. Only per-voice routes cost per voice. At the caps (48 per-voice routes, 8 voices, 8 layers, 1,500 ticks a second) that is about 4.6 million route evaluations a second. At an estimated 5 to 10 ns each, that is 2 to 5% of one core, worst case. Build stage S2 measures it.

## Sung features in real time

### What runs where

| Work | Runs in | Thread |
| --- | --- | --- |
| Live tier: bands, loudness, pitch, voicing, openness, breathiness, onset, hardness | Rust (`dsp::capture`), unchanged | Audio, inside `ew_process`, before the voices |
| Intensity and other derived features | Rust, when a frame is read | Audio |
| Envelopes, LFOs, the shaper, routes, every module, the channel | Rust (`crates/klont`) | Audio; the same code in the offline worker |
| Automation lane playback, morph, Influence blend | Rust | Audio |
| Patch document, commands, undo, validation, compiling a patch to an engine blob | TypeScript | Main |
| Web MIDI input and the mapping table | TypeScript (Web MIDI exists only on the main thread) | Main |
| Interface: rack, matrix, shaper editor, meters | TypeScript | Main |
| Second pass, Influence match | Rust in workers, unchanged | Workers |

The engine also checks every patch blob it is given (types known, caps respected, offsets in range, no cycle) and refuses a bad one with `LOAD_MALFORMED`, changing nothing. Imported projects are untrusted, and the engine must not trust the compiler either.

### Arm and listen

Today one layer can be armed: the mic plays it (Live follow). The voice that plays it reads the live frame. That stays. New: any number of layers can **listen**: their layer features come from the mic, while their notes come from somewhere else (the sequencer, MIDI, the on-screen keys). Singing into a pad that a MIDI keyboard plays, or into a delay on the drums, is listening. The live tier runs once whatever the number of listening layers.

### Latency

| Path | Delay inside the engine | Notes |
| --- | --- | --- |
| Volume, openness, breath, vowel, intensity to a parameter | About 5 ms analysis (2.3 ms measured in Chrome, M2) plus up to 0.67 ms to the next tick, plus the route's rise time | Intensity comes from the same bands, so it has the same delay as openness. |
| Pitch to a parameter | About 24 ms (measured, M2) | Requirement 11 of the TDD still holds: live uses each feature as soon as it exists. |
| Hardness and onset | One render quantum after the onset (2.7 ms) | Live uses the first estimate; stored frames use the one refined 10 ms later. |
| MIDI note or controller to sound | Up to one render quantum (2.7 ms) after the main thread posts it, plus however late the main thread runs | Applied at the start of the next block, never scheduled ahead, because scheduling ahead would add delay. |
| Screen control to sound | Same as today: one block plus the value's smoothing | |

Device delay (input, output) is added on top, as in the TDD's delay budget. The 50 ms Live follow budget is unchanged, because nothing here adds a thread hop.

### CPU

| Part | Budget | How it is held |
| --- | --- | --- |
| Live tier | Unchanged | Runs once, armed or listening |
| Derived features | A few dozen adds per tick per layer | |
| Routes | 2 to 5% of a core at the caps | Caps per patch; layer routes evaluated once |
| Voice modules | A layer may spend what 8 classic voices cost | Voice count drops for heavier patches; the editor shows it |
| Channel | Starts at today's full chain (crush, tape, delay, reverb) plus one filter | Cap per type; revisited with the laptop numbers |
| Whole engine, 8 layers, worst patches | At least 4x real time on the reference laptop | The M1 gate, re-run in stage S3 with a worst-case patch |

The live overload policy stays as it is: lower the partial budget first, then steal voices.

## Engine changes

| Change | Detail |
| --- | --- |
| `LOAD_PATCH` (new load kind) | Slot = layer. Patch blob v3 since S3. The compiled patch: module list with types and value offsets, the voice program in run order, cable buffer assignments, channel chain, mod sources, routes with their shaper fields and curve points. Swapped at a block boundary like other tables. |
| Layer blob v2 | Fields as today, then the layer's value vector (up to 1,024), macros, the Influence target as a value vector with one group bit per value. The compiler does the Klont-vocabulary binding, so the engine gets aligned vectors. Kit sets disappear: the kit's values are in the vector. |
| States blob v2 | `n_states` vectors of the layer's value length, plus macros and Influence amount each. |
| `MSG_SET_PARAM` | `param` is an index into the layer's value vector. `index` is unused. |
| `MSG_SET_PERF` (new) | Layer (255 is all layers), index 0 wheel, 1 bend, 2 pressure, value. |
| `MSG_LISTEN` (new) | Layer, 1 or 0. Several layers may listen. |
| `MSG_MORPH_TO` (new) | Layer, index = state, value = ms. A live morph, used by scenes. |
| Samples | `LOAD_GRAIN` becomes `LOAD_SAMPLE`: up to 64 samples in one pool of 2^23 floats (32 MB, about 2.9 minutes at 48 kHz), allocated at start-up and compacted the way the curve pool is. It replaces today's two 480,000-float buffers per layer (30.7 MB), so memory stays about the same. A sample being replaced stays until the 50 ms crossfade ends. |
| Track blob v3 | Per clip: the take's audio as a sample id (the raw input on playback), and automation lanes (target kind and index, points with time, value and shape). Built in S5; see `seq.rs`. |
| `MSG_RECORD` (S5) | 1 while a pass records: a hand move (a value, macro, level, pan or performance message) takes its target from its lane until the transport stops or recording ends. |
| Notes | A document note gains a velocity (`vel`, default 1). The track blob already has a strength per event; the compiler fills it from `vel` instead of writing 1. |
| Meter block | The selected layer's values after step 4 and after step 6, 1,024 each instead of 96; the current output of each route (for the matrix view); the live intensity. |

Everything else in the ABI (`ew_init`, `ew_process`, `ew_render`, `ew_load`, `ew_seek`) stays.

### Inside a voice

At note start the voice is given its patch's program: an ordered list of module slots it owns from its pre-allocated pool. Each control tick it evaluates its mod sources and routes, then hands each module its values. For each span (at most 32 samples, never crossing a tick) it runs the modules in order, each reading its input buffers and writing its output buffer. A voice is free when `env1` is idle and its output has been quiet, as today (the comb-tail rule). The 3 ms choke for stealing and end stops is a gain after the program.

Output stays bit-identical however the host slices the calls, because modules run whole spans in a fixed order and spans never cross a tick.

## Influence and the LLM function

The Influence match measures a sound in the Klont vocabulary and does not change. Applying it to a patch goes through the vocabulary table: each id is bound to the first module of its type in the patch (`src.*` to the `partials` module, `filt.*` to the first voice `filter`, `env.*` to `env1`, `verb.*` to the `reverb`). Ids with no module to bind to are skipped, and the Influence panel lists them ("not set: grains, comb"). On the classic patch every id binds, so Influence behaves exactly as today, and the blind-match gate keeps its meaning.

The LLM `/api/sound` endpoint is not built. When it is, its schema is generated per request from the keys of the layer's patch instead of once from the table.

## Two ways to play

### DAW use

- **Recording.** Capture gains a latched mode (press to start, press to stop) beside hold. While recording it keeps what it keeps today (mic audio and frames, then the second pass) and adds:
  - MIDI and on-screen notes, written into the clip as notes or hits with velocity, at the transport position where they sounded minus the output latency, as takes subtract the round trip.
  - Moves of any value (screen, MIDI controller, macro) and of the wheel, bend and pressure sources, written into automation lanes.
  - Scene taps written into the morph lane as markers.
  - One recorded pass is one undo group. Takes stack as they do today.
- **Automation lanes.** Per clip, beside the morph lane. A lane targets one value (a module parameter, a route amount, a macro, level or pan) or one of the wheel, bend and pressure sources, which it then plays back. Points have a time in section units (ticks or seconds, like notes), a value, and a shape to the next point (line, ease, step). Drawn in the timeline or recorded. During playback a value with a lane follows the lane. Moving it by hand while recording automation overwrites the lane from that point on; moving it otherwise leaves the lane in charge.
- **Deterministic playback.** Export and playback must agree.
  1. Everything that moves a value is a function of the sample clock and stored data. Nothing reads wall time.
  2. Synced LFOs read the transport position. Play resets free LFO phases and re-seeds every random source (noise, drift, random steps, jitter, tape dropouts) per layer, as export already does at render start. So live playback from a point matches an export from that point. As built (S5), Play from a stop also cuts sequenced notes and effect tails still ringing from the last playback, and an export settles a second with the transport stopped before Play, so its values start settled as a live engine's are.
  3. Live inputs never play back directly. The mic becomes a take, MIDI becomes notes, controllers become lanes, and playback reads those.
  4. Smoothing and latch state start fresh at each note.
  5. The live partial budget still adapts to CPU load and offline renders still do not, so an overloaded live playback can differ from the export. That is today's rule.

### Live use

- **Low latency.** Live follow stays in the engine's process call. MIDI goes straight into the control ring and is applied at the next block.
- **Performance controls.**
  - **MIDI keys** play the selected layer, or a layer chosen per MIDI channel. On a beat layer, notes 36, 38, 42 and 46 (the General MIDI kick, snare, closed and open hat, which match the slot names) play kit slots 1 to 4. MIDI export, when built, should use the same four.
  - **MIDI controllers** are mapped to values with Learn: press Learn, touch a control on screen, move the controller. A controller sets the value outright, like turning the on-screen knob, and is committed to the document as one undo step after it rests for 500 ms.
  - **Mod wheel, pitch bend and pressure** are sources, routed with amounts like any other source, because they are expression rather than settings.
  - **On-screen** keys, an XY pad (two values chosen by the user) and the macro knobs stand in where there is no MIDI device. Web MIDI needs a permission prompt. Checked for stage S4 (Nov 2026): Chrome, Edge and Firefox have it; Safari still does not, on macOS or iOS (only a third-party extension adds it). Without it, the on-screen controls still work.
- **Scenes.** The layer's states, shown as buttons. Tapping one morphs to it over the layer's scene time. When the transport plays a clip that has morph markers, the lane wins; scenes act when it has none or the transport is stopped.
- **Gesture mapping.** The sung features are sources, so mapping a gesture is adding a route. Pick a source in the matrix, then click any knob; the route starts with a sensible range and amount.

### Where the two differ

| | DAW use | Live use |
| --- | --- | --- |
| Notes come from | Clips | Mic (armed layer), MIDI, on-screen keys |
| Features come from | The take curve at clip time | The mic, for armed and listening layers |
| Value changes come from | Lanes, morph markers | Controllers, screen, scenes |
| Pitch and level alignment | Aligned within a frame (stored frames are shifted) | Each feature as soon as it exists (TDD requirement 11) |
| What is kept | Everything, as data | Nothing, unless recording |
| Structure edits | Normal editing | Allowed, but they cut sounding notes (5 ms fade) |
| CPU adaptation | Offline renders never adapt | Partial budget adapts, then voices are stolen |

There is no mode switch. Both run at once: a layer can play its clip while another listens to the mic and a third is played from a keyboard.

### The raw input beside the synth

Added for stage S5 at the owner's request. A layer that sings into the synth can also let the voice itself through, and slide from one to the other: singing that drives a pad and then becomes the sung vocal.

- **The `input` element.** A channel element (at most one per layer) that marks where the raw input audio joins the layer's signal. Its values are `mix` (0 is only the synth, 1 only the raw input) and `trim` (dB on the raw input, -24 to +12). `mix` is a value like any other: routes, scenes, Perform controls and automation lanes move it.
- **The blend.** An equal-power crossfade: the synth is scaled by cos(πm/2) and the raw input by sin(πm/2), exactly 1 and 0 at the ends, so 0 and 1 are exactly synth-only and raw-only. The gains ramp across each control tick like level and pan.
- **What the raw input is.** Live, the mic, while the layer listens or is armed. Otherwise, while the transport plays, the audio of the take on the layer's clip, read at the same take time as the take's features (through the clip's warp map). Otherwise silence. The synth part is everything the layer makes today: its voices and layer sources.
- **Effects.** The channel stays one ordered list, and the `input` element's place in it is the mix point. Effects after it run on the blend, so with the element first (where adding it puts it) one effect chain serves both. Effects before it run before the blend, each on the synth (as today) or on the raw input (an effect's `path`, `synth` or `raw`). That gives the same effects on both (after the element), different ones (before it, one path each) or effects on either alone. Level and pan apply to both paths, width to the blend. The rule that crush and tape come before any delay or reverb holds for the whole list. A layer without the element runs exactly as before.
- **Alignment.** The synth reacts to the sung level about 5 ms after the mic hears it (the live tier's level delay, "Latency"), so live the mic is held back by that delay before the blend. Pitch is later still (about 24 ms); holding the voice back that long would be heard as a late monitor, so live alignment follows the level. On playback the take's audio plays at its own time, on the beat, and the synth reads the take's stored frames ahead by the frame sink's shift toward the pitch delay (two frames, 20 ms at 48 kHz), so it follows the take as it follows the mic live. [S5.md](S5.md) reports the measured offsets.
- **Cost.** The element costs about what a channel filter does; a raw-path stereo effect runs a second level and pan. The take audio goes to the engine as a sample in the sample pool.

## Interface sketch

The Sound panel keeps today's simple view and gains Patch and Mod; the channel's cards sit in the Patch view after the voice's (stage S3). A Perform view joins the main views.

```text
┌ Sound ── [Simple] [Patch] [Mod] ─────────────────────────────────────────┐
│ Simple: Preset ▾  Level  Pan  Mute · States · Macros · Gestures · Tune ·  │
│         Influence — as today. Gestures sliders are the five group depths. │
└───────────────────────────────────────────────────────────────────────────┘

┌ Patch: Choir pad ─────────────────────────────── cost ▮▮▮▯▯  8 voices ───┐
│ VOICE                                                                     │
│ ① Partials  in —        level ◐ tilt ◐ odd/even ◐ stretch ◐ …   [⋯]      │
│ ② Amp       in ①        gain ◐ ⟲env1 ⟲volume ⟲velocity                    │
│ ③ Click     in —        start ◐ end ◐ colour ◐                            │
│ ④ Clang     in ② ③      amount ◐ ratio ◐                                  │
│ ⑤ Fold      in ④        amount ◐ bias ◐                                   │
│ ⑥ Comb      in ⑤        amount ◐ tune ◐ decay ◐                           │
│ ⑦ Filter    in ⑥        cutoff ◐ ⟲openness ⟲lfo1  res ◐ morph ◐ track ◐   │
│ → voice out ⑦                                          [+ module ▾]      │
│ LAYER SOURCES  Shaped noise (env2)   Grains (env1, no sample)             │
│ ENVELOPES  env1 ╱‾‾╲  env2 ╲__  env3 ╲   LFOs  lfo1 ∿  lfo2 ∿  lfo3 ≈     │
└───────────────────────────────────────────────────────────────────────────┘
```

- **A rack, not a wire canvas.** Modules are cards in signal order. A card's inputs are picked from a list ("in: ② ③"), not drawn. A small read-only diagram beside the rack shows the wiring. Every control is a DOM input with a label and a keyboard path, like the rest of the app.
- **Knobs show modulation.** A ring around a knob shows how far its routes can move it; the live thumb moves with the sound, as the sliders do today from the meter block. The ⟲ badges list the routes into a knob and open them.

```text
┌ Mod ──────────────────────────────────────────────────────────────────────┐
│ Source        live   Shaper                  Amount  Target          Via  │
│ volume        ▮▮▮▯   0.10–0.90  ╱  5/120 ms  ×1.00  Amp gain (×)     —    │
│ intensity     ▮▮▯▯   0.30–1.00  ⌒            +0.35  Filter cutoff    —    │
│ hardness   L  ▮▯▯▯   0–1        ╱            −0.30  Env1 attack      —    │
│ layer.pitch   C4     C3–C5      ╱            +0.50  Delay mix        wheel│
│ [+ route]   or pick a source here, then click any knob                     │
└───────────────────────────────────────────────────────────────────────────┘

┌ Shaper: intensity → Filter cutoff ───────────────────────────┐
│ 1 ┤                    ●━━━━━━━    in  0.30 … 1.00           │
│   ┤              ●━━━━━┛           bend +0.4 (or drag points) │
│   ┤        ●━━━━━┛                 rise 5 ms   fall 200 ms    │
│ 0 ┼━━━━━━━━┛         ▲ live        latch ☐   polarity up      │
│   0                         1      amount +0.35               │
└──────────────────────────────────────────────────────────────┘

┌ Perform ── Layer: Lead ▾ ── [Listen ●] [Arm ○] ── MIDI: Keystation ✓ [Learn] ┐
│ volume ▮▮▮▮▯  intensity ▮▮▯▯▯  hardness ▮▯▯▯▯  pitch C4 +12¢                 │
│ macros ◐ ◐ ◐ ◐ ◐ ◐ ◐ ◐         XY [        •      ]   wheel ▮▮▯  bend ─┼─     │
│ scenes [Calm] [Bright] [Wild]  morph 400 ms                   [Record ●]   │
└────────────────────────────────────────────────────────────────────────────┘
```

The channel cards in the Patch view are the effect chain in order, with add, remove, earlier, later and bypass on each card, then width. Level and pan stay in the Simple view.

## Existing projects and presets

- **Schema 4.** `migrate.ts` turns each layer into the classic patch. State `base` sets become `values` under the new keys. Kit slot sets move into every state. Gesture depths and Follow stay layer fields and become the group depths of the default routes. Macro weights become macro routes. The Influence target stays in the Klont vocabulary. Notes get `vel` 1 and clips get no lanes.
- **Same sound.** A migrated project must render within -90 dB of the same project on today's engine, which is the TDD's golden-render rule. Golden files are recorded from today's engine before stage S1 starts.
- **Undo history.** The journal stores patches against paths like `states[0].base`, which no longer exist. Migration rewrites the snapshot and starts a fresh undo history, and the app says so once. Importing a schema 3 zip does the same.
- **Presets.** The Preset menu lists factory presets (today's ten, converted) and user presets. A layer whose values match no preset reads "Custom", as today.
- **M0 pages** keep their own engine and presets. The M1 to M4 test pages use the klont engine, so they move to the new blob formats with it; their gates are re-run in stage S1.

## Build plan

Each stage ends with checks anyone can run. Stage S1 changes nothing the user can hear or see; every later stage adds something they can use.

### S1: the engine runs patches

Split the voice into modules, add the per-voice program, the channel chain, the value vector, the route evaluator with groups, `LOAD_PATCH`, blob v2 formats and the sample pool. In TypeScript: patch types, the classic patch, the compiler, schema 4 migration, and the factory preset files made from `sounds.ts`.

- `pnpm test` passes, including the no-allocation test, which now also loads a patch.
- Golden renders: the M1 demo song, the app's three demos and five projects saved from today's app, migrated, render within -90 dB of today's engine, or each difference is listed and approved by ear.
- The benchmark worst case on the build machine is within 10% of the M1 figures.
- `blind_match` and `round_trip` give the same scores.
- A schema 3 project zip imports, plays and says its undo history was reset.

### S2: the modulation matrix

All sources including intensity, the shaper, latch, via, the scale and pull modes, groups, layer LFOs, listening layers. In the interface: the Mod view, the shaper editor, rings on knobs, pick-a-source-then-click-a-knob, feature meters; knobs for every module value in the Patch view (structure still fixed to the classic patch).

- Unit tests for each shaper field, and for the rule that no source can modulate itself.
- Two offline renders with every source type routed are bit-identical.
- Loopback: a step in input level moves a filter cutoff in the output within 6 ms inside the engine; a pitch route within 26 ms.
- 8 layers x 8 voices with 48 voice routes each renders at 4x real time or better on the build machine; the figure is recorded for the laptop run.
- A walkthrough step "Make hardness set the attack" can be finished in under a minute by someone new to the app.

### S3: editing patches

New modules (`osc`, `noise`, `sampler`, `amp`, channel `filter`); add, remove, rewire and reorder; caps and the cost meter; voice count from cost; preset save, load, import and export; the 5 ms fade on structure changes.

- Validation tests: a cycle, a cap overrun, an unknown type and a truncated blob are each refused, and the engine keeps playing the old patch.
- A saw `osc` at 2 kHz keeps its aliasing at least 40 dB below the fundamental at 48 kHz; the figures from 100 Hz to 8 kHz are recorded.
- A structure edit while 8 voices play makes no step larger than the 5 ms fade.
- The M1 bench, re-run with each layer's worst allowed patch, renders at 4x real time or better on the reference laptop.
- Each factory preset saved and loaded again gives a bit-identical render.

### S4: live play

Web MIDI input, Learn, the controller-to-value map (stored with the project; a per-device default in settings), wheel, bend and pressure, on-screen keys and XY pad, scenes, the Perform view.

- Engine time from a MIDI message being written to the ring to the note's first output sample: one render quantum or less. Main-thread delay from the MIDI event to the write: under 5 ms at the 95th percentile during playback of the demo song.
- Three listening layers cost the live tier once (benchmark).
- A Learn mapping survives a reload and a project export and import.
- Without Web MIDI (Safari), the on-screen keys, pad and knobs work.

### S5: recording and automation

Latched record, notes from MIDI and the screen into clips, value moves into lanes, scene taps into morph markers, lane drawing in the timeline, track blob v3, the Play-time reset of LFO phases and random seeds.

- Record 30 s with MIDI notes, controller moves and singing into a listening layer. Live playback from the start and an export null within -90 dB. Two exports are bit-identical.
- Undo removes a recorded pass as one step; redo puts it back.
- Killing the tab after a recorded pass and reopening restores the lanes (journal replay).

Added to S5: the raw input beside the synth ("The raw input beside the synth").

- At mix 0 and 1 the output nulls within -90 dB offline against the synth alone and the raw input alone.
- A recorded take with an automated mix sweep renders bit-identically twice.
- The offset between the raw input and the synth is measured and reported, live and on playback.

## Choices for the owner

Each has a recommendation; the design above assumes it.

1. **Patch editor: rack or wires.** A rack of cards with input lists (recommended: accessible, keyboard-friendly, fits the app's plain TypeScript and DOM) or a node canvas with drawn cables (more familiar to modular users, much harder to make accessible). The rack can gain a canvas later; the patch format is the same.
2. **What "intensity" means.** Vocal effort from spectral tilt, separate from volume (recommended; it works on every existing take), or simply another name for volume.
3. **Kit slots.** Each kit slot has its own voice patch (recommended; a sampled hat beside a synthesized kick), or one patch for all four slots with different values.
4. **Undo history on upgrade.** Start fresh when a project moves to schema 4 (recommended), or translate old journal entries, which is a lot of work for undo steps that only reach back to before the upgrade.
5. **MIDI controllers.** Set values outright, like turning the on-screen knob, with wheel, bend and pressure as modulation sources (recommended), or make every controller a modulation source routed with an amount.
6. **Order of S4 and S5.** Live play first (as planned) or recording and automation first, depending on which use the owner wants to try sooner.

## Not in this design

- Mono and legato voice modes with glide for MIDI playing, MPE and polyphonic pressure. Small to add in S4 if wanted.
- Send and return buses shared between layers, and sidechain audio between layers (a layer listening to another layer's audio). Layers listen only to the mic.
- Audio-rate modulation other than an oscillator's `pm` input.
- Per-voice stereo or per-voice pan.
- MIDI output, MIDI clock sync, and hosting third-party plugins.
- Smear (still v1), and pull, squash and shake on the Klont glyph (still missing, per `coverage.md`).
- Widening the song engine to 24 bands.
