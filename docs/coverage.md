# Coverage: what each stage put in front of a person

Checked against `docs/TDD.md` (Interface, Persistence, Export) and `docs/M0.md` through `docs/M5.md`. Status is about the index app, not about whether the engine can do the thing.

- **supported** — reachable in the app, and a walkthrough step points at it (or it is a status, not a control).
- **partial** — some of it is in the app, and the rest is named in the notes.
- **missing** — not in the app. A command or a page may still exist.
- **test-page only** — the milestone page under Tests still has it. The app does not.

Hosting, the LLM service and Refine are out of scope for the app, and are listed so the gap is explicit.

## M0

| Capability | Stage | Status | Notes |
| --- | --- | --- | --- |
| Live follow: sing and the synth follows, with the ribbon, six presets, Mouth, Snap, Glide, Gliss and Jump | M0 | test-page only | `live.html`, with the checklist on `live-test.html` and `live-session.html`. While Capture is held the app does arm the live voice. It does not offer the ribbon or those tuning controls. |
| Record a take, play the synth, play the voice, save the last 30 s | M0 | test-page only | `live.html`. The app places a take on the track instead of looping it there. |
| 16 or 24 bands | M0 | missing | The switch is on the Live and Influence pages. The app has no band control. |
| Influence audition: file, drop, record 3 s, play source, play match, play A then B, audition pitch | M0 | test-page only | `influence.html`. The app's Influence is the track control from M4. |
| Blind match, four candidates | M0 | test-page only | `influence-test.html`. The M4 gate page is the one that scores the klont engine. |
| Delay and round-trip measurement | M0 | test-page only | `delay.html`. The app's Align field stores the number. It does not run the chirps. |
| MessagePort fallback when the page is not isolated | M0 | supported | Same host as the other pages. The status line says Shared or Fallback, and says so when isolation is missing. Not a walkthrough step. |
| Index lists every page and whether the page is isolated | M0 | partial | Tests lists the pages. Isolation is reported in the status line when it is off, and as Shared or Fallback after Start. |

## M1

| Capability | Stage | Status | Notes |
| --- | --- | --- | --- |
| Play an 8-layer song, with mute, level and a voice count | M1 | partial | `m1.html` still plays the engine demo. The app has mute, level, pan and a voice count (a number and a bar) on the song you are editing. Learn opens the three small demos. |
| Peak meter, CPU load, partial budget, steals, overloads | M1 | test-page only | `m1.html`. The engine's layer peak only rises, which is why the app shows voices instead. |
| Export WAV, faster than real time | M1 | test-page only | `m1.html`. The app exports a project zip, not a mix. |
| Benchmark: worst case, demo song, block timing, live 8 layers | M1 | test-page only | `bench.html`. |

## M2

| Capability | Stage | Status | Notes |
| --- | --- | --- | --- |
| Hold to capture. The second pass turns it into notes or hits | M2 | supported | Capture, with the M5 count-in when the transport is stopped. Walkthrough "Capture onto the track". `m2.html` still loops a take on its own. |
| Mouth, Snap and the slide modes while you sing | M2 | partial | Mouth, loud, open, hard and breath are gesture depths, with a walkthrough step. Snap, speed, glide, gliss, jump, vibrato, root and scale are on `m2.html` and `m3.html` only. |
| Keep / Redo tally, and Save take as files | M2 | test-page only | `m2.html`. |
| Teach my kit | M2 | test-page only | `m2.html`. M3 left it off the track page on purpose. |
| Per-take engine follow readout, and play voice against synth | M2 | test-page only | `m2.html`. |
| Engine follow test with no mic | M2 | test-page only | `delay.html`. |

## M3

| Capability | Stage | Status | Notes |
| --- | --- | --- | --- |
| Start, Start with mic, Enable mic | M3 | supported | The first walkthrough step is Start. The capture lesson names Enable mic. |
| Reopen the last project | M3 | supported | The session does this on load. File's Open is the walkthrough for projects you pick. |
| Name, New, Open, Delete, and a save state | M3 | supported | File. The footer says Saved, or Saving and how many groups are still queued. |
| Storage used, and a warning at 80% of the quota | M3 | supported | The footer. At 80% it adds "nearly full". Not a walkthrough step. |
| Play, Stop, Undo, Redo, including the keyboard | M3 | supported | Walkthrough. Play seeks to the start of the selected section and then plays. |
| Go to the start of the section without the Play key | M3 | partial | There is no separate seek. Pressing Play restarts the section. |
| Add, select, name, length, bpm, repeats, copy and remove a section | M3 | supported | The stepper, BPM, and the section block under Layers. Walkthrough "Shape the section". The last section cannot be removed. |
| Grid or free feel, tempo ramp, reorder sections | M3 | missing | The commands exist. A free section that is already in a file shows its length in seconds. The app cannot switch the feel, set a ramp, or move a section. |
| Loop the track | M3 | missing | New projects loop. There is still no control, as M3 noted. |
| Add a melody, beat or texture. Level, pan, mute | M3 | supported | Pan reads L, C or R. Mute changes the button to Muted and the row says mute. Walkthrough "Place the layer" for pan. Adding a layer has no step of its own. |
| Rename a layer, remove a layer, change its kind | M3 | missing | Kind is chosen when the layer is added. |
| Tune: snap, speed, slides, vibrato, octave, root, scale, level out | M3 | missing | On `m3.html`. Capture still does not write Tune from the second pass. |
| Timeline: notes, ribbons, hits, drag, resize, delete, double-click | M3 | partial | The WebGL timeline drags, resizes from the right edge, and double-clicks to add. The pattern grid does pitch, delete and kit slot. Zoom, snap, quantize, loop length and dragging a hit between slots stay on `m3.html`. |
| Edit-while-playing stress test, reload check, crash check | M3 | test-page only | `m3.html`. Autosave itself runs in the app. |
| Overdub placed with the round trip taken out | M3 | partial | Align sets the milliseconds. Measuring them stays on `m3.html` and `delay.html`. The capture lesson has a step for Align. |
| Hold to overdub while the track is already playing | M3 | supported | Capture. The count-in is only when the transport is stopped. |

## M4

| Capability | Stage | Status | Notes |
| --- | --- | --- | --- |
| States: add, copy, remove, and pick the one being edited | M4 | supported | Up to 8. Walkthrough "Add a state". Copy counts as that step too. Rename is not a field. |
| Influence: file or Listen, amount, five aspects, use the sound itself, Commit, Clear | M4 | supported | Listen reruns the match while it is held. Walkthrough "Match a sound". |
| Pick a source saved on the Influence page | M4 | missing | `m3.html` has the list. The app takes a file or the mic. |
| Play the source, and hold to play a note | M4 | partial | Hear slot plays the selected kit slot. There is no play-source, and no hold-to-play for a melody note. |
| Provisional readout while Listen is running | M4 | missing | The status names the source. It does not show the aspect meters. |
| Kit slot from one hit, mic or file, then hear it | M4 | supported | Walkthrough "Use your sound for a kit slot". |
| Morph lane: marker, shape, length, and the sound moves while playing | M4 | partial | Add at the pattern row, shape, length, and remove. The timeline drags a marker. Marker time as a number, M at the playhead, and the bracket keys stay on `m3.html`. Walkthrough "Place a morph marker". |
| Blind match at 100%, 20 sources | M4 | test-page only | `m4-test.html`. |

## M5

| Capability | Stage | Status | Notes |
| --- | --- | --- | --- |
| Metronome: on, level, tick or a kit slot, heard live only | M5 | supported | Walkthrough. The bar stays 4. There is no control for it. |
| One-bar count-in, cancelled if you let go | M5 | supported | Named in the capture step. |
| Pattern: 16ths, notes, hits, keys 1–4, the playing row | M5 | supported | The playing row has a triangle as well as the wash, so it is not color alone. Walkthrough "Read the pattern". |
| Voices sounding, per layer | M5 | supported | A number and a bar above the pattern. |
| WebGL timeline of the same section | M5 | supported | Walkthrough "Read the timeline". |
| Eight macros on the state being edited | M5 | supported | Walkthrough. |
| Gesture depth, including hit follow on a beat | M5 | supported | Walkthrough. |
| File: save, save as, rename, export zip, import zip, clean up | M5 | supported | The keep lesson. Export and clean up advance with Next. They do not wait for the download. |
| Three demos and the walkthroughs | M5 | supported | Learn. |
| Earlier pages kept under Tests | M5 | supported | The menu. No walkthrough step. |
| One undo stack for the app, kept with the project | M5 | supported | Walkthrough. |

## TDD interface, where no milestone shipped it

| Capability | Stage | Status | Notes |
| --- | --- | --- | --- |
| The Klont glyph that moves with Bright, Grit, Motion and Space | TDD | missing | Not on any page. |
| A ribbon that grows under the voice during capture | TDD | missing | Ribbons draw for a take that is already placed. |
| Macros as pull, squash and shake on the Klont | TDD | missing | Macros are sliders. |
| Device change pauses capture and asks for a new calibration | TDD | missing | |
| Svelte components over one WebGL canvas | TDD | missing | M5 records this as a choice: the app is Vite and TypeScript, with a WebGL timeline under a DOM pattern. |
| Export one zip of the project and its audio | TDD | supported | File. |
| Warn at 80% of the storage quota | TDD | supported | Footer. |
| Render the song to a file from the app | TDD | missing | WAV export is the M1 player. |
| LLM sound, suggestions and names | TDD | missing | Not built. Out of scope here. |
| Refine | TDD | missing | Not built. Out of scope here. |
| Echo canceller | TDD | missing | Still open. The app assumes headphones. |

## Wired in this pass

These were already commands or fields, with no control on the index page:

- Section name, length, repeats, copy and remove.
- Pan, with an L / C / R readout, heard while the slider moves.
- Copy state and remove state.
- Align, the round-trip milliseconds capture was already applying (it had been fixed at 30).
- Saved / Saving, and the nearly-full line, which `m3.html` already showed.

Walkthrough steps cover the section block, Align and pan.

## Follow-ups

Left alone because each one is a new editing surface, a measurement rig, or work this task set aside:

- Tune (snap, slides, scale, octave) and filling it from a take.
- Section feel, tempo ramp, and reordering sections. A loop switch. Metronome bar length.
- Rename or remove a layer.
- Timeline zoom, quantize, snap, and dragging a hit to another slot.
- Round-trip measurement, rather than typing Align.
- Influence: saved sources, play source, hold to play a note, the provisional meters. Rename a state.
- Teach my kit, the take stack, and a WAV export of the mix.
- The Klont visualizer, the live ribbon, and device-change handling.
- Hosting, the LLM service, and Refine.
