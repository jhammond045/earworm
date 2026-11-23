# UI1: the interface pass

Built Nov 22, 2026, from [ui.md](ui.md), after Q2 and P3 landed. The app had grown one control at a time across fifteen stages; this pass audited it at six screen sizes, in both color schemes, by mouse, touch, keyboard and MIDI, and rebuilt the page around one model: a transport that is always there, two places (Track to make the song, Perform to play it), and the selected layer as the thread between them. It also fixes what the audit found broken, chiefly a pattern grid that could not be seen on any screen 960 px and wider. After the owner's first look ("much of the layout is still pretty sloppy"), a second pass put every control on one field grid and moved the explanations behind help bubbles. No engine, compiler or project-format code changed: the audio and determinism guarantees of S5 and Q2 stand as they were.

## What was built

| Part | Where |
| --- | --- |
| The audit, the model, the layout for each width, the design language, the change list, and before and after screenshots | `docs/ui.md`, `docs/ui-shots/` |
| The pattern grid shows on wide screens (it was a 2 px flex child) | `web/src/app/style.css` |
| Color tokens for dark and light, contrast-checked (text 4.5:1, control boundaries 3:1). Light follows the system unless Setup picks Light or Dark; the choice is kept in this browser (`earworm.theme`) and set before the first paint | `style.css` `:root`; the script in `web/index.html`'s head; the theme block in `app/main.ts` |
| The timeline, the live ribbon and the Klont read the tokens. The Klont draws with alpha, so the theme's well shows around it | `readColors` and `retheme` in `app/gl-timeline.ts`; `themeColors` in `main.ts`; `app/klont-glyph.ts` |
| One transport row: File and the project, the section, tempo and position, Play and Stop, Loop and Metro, Undo and Redo, Start (then the mic and MIDI state), Setup, Learn and the key list | `web/index.html`, `renderChrome` and `paintAudioChip` in `main.ts` |
| Setup: Align and Measure, the metronome's level, bar and sound, the vowel bands, the theme, and the test pages. A lesson step whose control is in a dialog opens it beside the tour bar | `index.html`; `paintTour` in `main.ts` |
| Places. From 960 px: Track (layers, track, sound) and Perform (layers and the play surface). Narrower: Layers, Track, Sound and Perform, one at a time. Alt 1, 2 and 3 switch | `showPanel` in `main.ts`; `body[data-place]` in `style.css` |
| Perform is its own place, laid out across the width with the keys first. The computer keyboard presses its on-screen keys (A W S E D F T G Y H U J K, Z and X for the octave; A S D F for a beat layer's slots), so a note sounds, records and lights up as a tap does. Its Record button is gone (the transport's is always on screen). The Mono/Poly row moved to the Layers panel, beside the layer it describes | `app/perform.ts` (`active`, `qkeys`, `renderCaptureMode`); `performShown` in `app/synth-ui.ts` |
| Capture, Record and Import sit together and name the layer they put a take on ("Into Melody"). Record says "Recording" while it runs | `index.html`; the animation frame in `main.ts` |
| Phones and tablets in portrait: the page scrolls as a whole; a dock at the bottom holds the status line, Play and Stop, the take buttons and the place bar. On a phone on its side they share one row. The Klont and voice meters show only while Capture or Record runs, and on short wide screens too | `style.css` |
| Large monitors: wider columns, text one step larger, a taller grid, Perform across the full width | `style.css` |
| Colorblind-safe signals. Kit slots: Okabe-Ito colors, a shape per slot (kick a wide block, snare a tall one, hat a small square, open a hollow square), an outline in the text color, and the slot names beside their rows. Morph markers: one notch per state number, in the timeline and in the marker list under it. Toggles: a ring that fills when on | `SLOT_COLORS` in `web/src/m2/sounds.ts`; `draw` and `layoutNames` in `gl-timeline.ts`; `renderPattern` in `main.ts`; `style.css` |
| Keyboard. The pattern grid is one Tab stop (the cursor cell). The timeline: ← → pick a note or hit, ↑ ↓ pick a lane, Shift with an arrow moves it by the snap or a semitone (a hit: its slot), Delete removes, and a live region says what is picked. Skip links to the track and the sound. `?` lists the keys | `renderPattern` in `main.ts`; `onKey` in `gl-timeline.ts`; `index.html` |
| Touch. The timeline takes touch (it had `touch-action: auto`, so a drag scrolled the page): a finger on a note drags it, on empty space pans, and two taps add | `startPan`, `movePan`, `endPan` in `gl-timeline.ts` |
| Targets: nothing under 24 px; 44 px on a coarse pointer, pattern cells 40. File inputs look and size like the buttons beside them | `style.css`, `index.html` |
| Names that no longer collide: the section's Time (Grid or Free), Groove (Straight or Swing) inside Feel, Pitch snap in Tune, MIDI learn, Listen (hold), Teach (hold). The lessons use the new names and say where the metronome's settings went | `index.html`, `main.ts`, `perform.ts`, `app/tutorials.ts` |
| The next step. After a take lands the status line offers "Quantize it"; after a pass, "See it on the track". Quantize opens the Feel box, whose summary line says its settings when it is folded | `offerNext` and `fillFlex` in `main.ts` |
| File: Share (Export mix first), Projects, and Tidy, with Delete apart and marked | `index.html` |
| The Mod view's routes are cards (source and amount; the target and its live output; the shaper; what scales it), and an open range reads ∞ | `app/synth-ui.ts`, `style.css` |
| **Second pass.** One field grid in every panel and dialog: the name in a label column, the control beside it at full width, the buttons that act on it lined up under it. A column under 220 px puts names above controls (a container query) | `style.css` ("forms"), `index.html` |
| Help bubbles: every explanation that was a paragraph on the page is behind a "?" beside its heading, legend or toolbar; the bubble is a native popover placed beside its button and kept in the window. Only live status stays on the page | `app/help.ts`; `index.html`; `perform.ts`, `synth-ui.ts` |
| Values beside sliders: level in dB, Tune, the macros, gestures, Influence's amount | `readout` in `main.ts` |
| Influence's aspects are chips; the theme switch is a segmented control; Perform's top is two toolbars (the layer and how it is played, then MIDI) | `index.html`, `perform.ts`, `style.css` |
| One meaning per key: Space plays and stops (on a focused button, box or tab it presses that control), hold C captures, R starts and stops a pass. The key list, the lessons and the tooltips say so | the window key handlers in `main.ts`; `index.html`; `tutorials.ts`; `capture.ts` |

## How to check it

| Command or action | What it checks |
| --- | --- |
| `pnpm test` | Every TypeScript and Rust test |
| `pnpm typecheck` and `pnpm build` | The page types and builds |
| `pnpm dev`, then Learn, Beat demo, Start, at 1440 × 900 | The pattern grid shows above the timeline; the transport is one row |
| Setup, Theme, Light; reload | The page, the timeline and the Klont redraw light, and stay light |
| Click the Beat lane in the timeline | Its rows read kick, snare, hat and open; each slot's hits have their own shape |
| Tab from the top of the page | "Skip to the track" first; the grid takes one Tab; arrows move inside it |
| Focus the timeline: ←, →, ↓, Shift →, Shift ↓, Delete | Picks a hit, the next lane, moves it a step, changes its slot, removes it; a screen reader hears each |
| Alt 3, then hold A, press X, record a pass with Record | Perform plays from the keyboard; "Recorded 3 played notes" and "See it on the track" |
| Import take with an audio file on Melody | The status line says what was imported and offers "Quantize it"; pressing it opens Feel, "on · 1/4 beat · 100% · smooth" |
| Chrome DevTools device toolbar at 390 × 844 and 844 × 390, touch on | The dock at the bottom; the page scrolls; dragging empty timeline space pans; two taps add a note |
| `?` | The list of keys |
| Space, Space, R, R, hold C | Play, stop, start a pass ("● Recording"), stop it; C without the mic says to press Enable mic |
| Any "?" | A bubble opens beside it, inside the window; Esc or a click outside closes it |
| A tablet at 1024 × 768 | The layer list's names sit above their controls; nothing in it is cut off |
| Learn, "Capture onto the track", Next to "Line the take up" | Setup opens beside the tour bar with Align outlined |

## Results

Measured on the build machine in headless Chrome, Nov 22, 2026. The before and after screenshots are side by side in ui.md, section 7.

| Check | Result |
| --- | --- |
| Pattern grid at 1440 × 900 | 342 px tall, 16 rows (was 2 px) |
| Phone 390 × 844 | The work starts at 198 px and ends at the dock's top, 702 px: 504 px on load, all 702 once the header scrolls away, against a fixed 299 px box before. 10 pattern rows on load (was 6). No horizontal scroll at 375 or 390 |
| Phone on its side, 844 × 390 | 214 px of work under the header before it scrolls away |
| Targets under 24 px | 0 at 1440 and at 390 (were 42 and 2) |
| Text contrast under 4.5:1 | 0 in dark and 0 in light, on every visible text node, by script |
| Keyboard | 33 Tab stops to the grid (one with the skip link), 1 inside it (was 32 for two layers), 39 to the timeline (was 68) |
| Timeline keys | "Beat: kick at 1.1", then "hat at 1.1"; Shift → moved it (one undo step); Shift ↓ set the slot ("open") |
| Perform, computer keyboard | Holding A sounds one voice on Melody and lights the C key; letting go ends it; X moves the keys up an octave. A recorded pass with A, D and G: "Recorded 3 played notes, 0 lanes" |
| Virtual MIDI (a stand-in `requestMIDIAccess` with one input) | Connect MIDI shows "MIDI: Virtual keys", in Perform and in the transport. Note 64 on sounds one voice; note off ends it |
| Import take, then Quantize from the status line | A 6 s drum file on Melody: 7 notes, 7 anchors, "Quantize it" offered; pressed, Feel reads "on · 1/4 beat · 100% · smooth" and opens |
| Export mix | "Mix saved. 30.8× realtime, peak 0.56", from File |
| A lesson step inside Setup | Setup opens without a backdrop, the tour bar stays usable, Align is outlined |
| Keys | Space played ("■ Playing") and stopped; R started a pass ("● Recording") and stopped it; C without the mic said "Capture needs the mic: press Enable mic." |
| After the second pass, both themes, 1440 × 900 and 390 × 844 | No control under 24 px, no text under 4.5:1, no horizontal scroll. Grid 342 px at 1440; on the phone the work runs from 198 px to the dock at 702 px |
| Console errors during the walks | None |
| `pnpm test` | All pass, after the second pass and rebased onto main at e9de134 (the poly-take limit and the meter design landed during the pass): 449 Vitest tests in 57 files, and 365 Rust tests (4 ignored, as before) |

## Where the build differs from ui.md

- **No More menu on a phone.** The header (File, section, tempo, Loop, Metro, Undo) scrolls away with the page, and the controls used while playing are in the bottom dock. That kept every control one tap away, with nothing hidden in a menu.
- **The timeline does not grow into free height on a large monitor.** Its height is its lanes (one per layer), so the grid grew instead.
- **Record left Perform.** The transport's Record is always on screen in both places, so the second one was only a duplicate with a different label.
- **The keys changed from TDD's design.** TDD had Space hold to capture; the owner chose Space for play and stop only, hold C to capture and R for a pass.

## What remains

The list is in ui.md, section 7 ("Left for a later pass"). The one that matters most is capture with a real microphone, which headless Chrome here cannot grant, so it was not walked (only its buttons moved; the capture code did not change).

## Decided

The owner decided the four questions the first draft asked, each as recommended:

1. **Keys.** Space always plays and stops, hold C captures, R starts and stops a pass. Built.
2. **The light theme** stays warm paper white with dark amber for "on".
3. **Kit slot colors** stay orange, sky blue, purple and yellow, each with its shape and its named row.
4. **Undo on a phone** stays in the header, which scrolls away; the dock keeps Play, Stop and the take buttons.

## Questions for the owner

1. **Help for a first visit.** Every explanation is now one press away, behind a "?". The lessons still walk through each feature. Recommended: leave it so. Or should a first visit show a short line under each panel heading until it is dismissed?
2. **What next.** The two largest things left are a mute button on each layer row (with solo), and a list of sections to click instead of the stepper. Recommended: those two first, in the next UI pass.
