# The Earworm interface: audit, model and plan

Written Nov 22, 2026, for the UI pass after Q2 and P3 landed. It has four parts: what the app was like to use (the audit), how its parts should fit together (the model), the layout and the design language that carry the model on every screen size, and the change list in the order it was built. [UI1.md](UI1.md) says what was built, how to check it and what is left.

The owner is red-green colorblind. Nothing in the app may be signalled by red against green, or by color alone: every color signal comes with text, an icon or a shape.

## 1. How the audit was done

The app ran from `pnpm dev` in headless Chrome, driven through the DevTools protocol, at six sizes:

| Name | Viewport | Input |
| --- | --- | --- |
| Phone | 375 × 812 and 390 × 844 | touch emulation |
| Tablet portrait | 768 × 1024 | touch emulation |
| Tablet landscape | 1024 × 768 | touch emulation |
| Laptop | 1440 × 900 | mouse, keyboard only |
| Large monitor | 2560 × 1440 | mouse |

Each size was captured with `prefers-color-scheme` dark and light. The walk: open the app, Start, open the Beat and Melody demos, Import take, Quantize and the Feel row, Takes set to Poly, the Patch and Mod views, Perform, Record, the mix controls, File and Export mix. A script measured, on every visible control, its size against WCAG 2.2's 24 px minimum, its text contrast against the background it sits on, and whether it has a name; another counted Tab stops.

Limits. Headless Chrome on the build machine never answers a microphone request (`getUserMedia` waits forever, with the fake-device flags too, as earlier stages also found), so Capture and Record with the mic were checked by reading the code and by their disabled and status states, not by singing. MIDI was checked with a stand-in `requestMIDIAccess` that sends note and controller messages to the page.

The before screenshots are in [ui-shots](ui-shots/) as `before-*.webp`.

## 2. What the audit found

### Broken

1. **The pattern grid is invisible on every screen 960 px and wider.** The middle panel is a flex column that scrolls, and the grid's wrapper is a flex child with `overflow: auto`, so it shrinks to its border: 2 px tall at 1440 × 900 (it only shows at 2560 × 1440 because the window is tall enough that nothing has to shrink). The keyboard path for editing notes and hits, and the playing-row triangle, are both in that grid. ([before-laptop-dark](ui-shots/before-laptop-dark.webp): the Pattern heading sits right on the timeline toolbar.)
2. **On a phone the controls take 61% of the screen before any content.** At 390 × 844 the transport, the command bank, the voice meters, the Klont and the ribbon end at 518 px; the panel the user is working in gets 299 px and scrolls inside itself ([before-phone390-dark](ui-shots/before-phone390-dark.webp), [before-phone390-perform-scrolled](ui-shots/before-phone390-perform-scrolled.webp)). Six pattern rows are visible.
3. **The timeline cannot be edited by touch.** The canvas leaves `touch-action` at `auto`, so a finger drag on a note scrolls the page and the browser cancels the pointer; notes and hits only move with a mouse or a pen.
4. **There is no light theme.** `color-scheme: dark` is fixed and the timeline's colors are written into the WebGL code, so a light system setting gives the dark page ([before-laptop-light](ui-shots/before-laptop-light.webp) is the same picture as the dark one).

### Layout, by size

- **Phone (375, 390).** Three toolbar rows wrap into nine. Start, Start with mic, Measure, Align, Bands, Tests and the metronome's level and bar box all sit above the work, every session, though most are set once. The Klont and the empty ribbon box take 100 px when nothing is being captured. The panel tabs are in the middle of the page, not where a thumb is. Perform's keys are 44 px wide in a panel that is 300 px tall.
- **Tablet portrait (768).** The same stack as the phone, two rows shorter. The pattern grid's two columns stretch to 350 px each for a five-letter cell.
- **Tablet landscape (1024) and laptop (1440).** The three columns work, but the Sound column is 320 to 380 px wide and carries everything from presets to the modulation matrix to Perform's keys. The Mod view's route table overflows its column (the shaper range wraps to `-1000000-1000000` across two lines, [before-laptop-mod](ui-shots/before-laptop-mod.webp)). The voice meters take a full-width row for two numbers. The ribbon box is empty unless Capture is held.
- **Large monitor (2560).** Everything stays at laptop size: 13 px text, a 240 px timeline, a pattern grid whose two columns are 740 px wide, and the bottom third of the middle column empty ([before-monitor-dark](ui-shots/before-monitor-dark.webp)).

### Flow: where features don't lead to each other

- **Starting.** Three buttons (Start, Start with mic, and Enable mic after the first) and a status line at the bottom say how to begin. Play is disabled with no reason given. After Start, both Start buttons stay on screen, disabled.
- **Getting sound in.** Capture (hold), Record (press, press again) and Import take do the same job, putting a take on the selected layer, but they sit among setup controls, and nothing says how they differ except their tooltips. Record appears twice (the command bank and Perform) with different labels while it runs.
- **After a take.** The status line says what was placed. Nothing points at the next step: Quantize and Feel are two panels away on a phone, and Takes (Mono or Poly) lives inside Perform even though it decides how every take into the layer is read.
- **Performing.** Perform is the fourth tab inside the Sound panel, a 320 to 380 px column on a laptop and a 300 px scroll window on a phone. There is no way to play notes from the computer keyboard. MIDI's state is only visible on that tab.
- **Recording a performance.** Record in the transport and Record in Perform are the same action; the transport one does not say "Recording" when pressed (only its color changes).
- **Finishing.** Export mix, the end of every session, is inside File, beside Delete.
- **Help.** "Learn" opens the lessons; "Learn" in Perform is MIDI learn. Lessons are good and cover every feature, but they are found under a button whose name is also used for something else.

### Names that collide

| Word | Means | Also means |
| --- | --- | --- |
| Feel | Section time: Grid or Free | Flex quantize on/off (the Feel row), and inside that row, Straight or Swing |
| Learn | The lessons | MIDI learn in Perform |
| Listen | Influence: hold to match a sound you make | Mod and Perform: let the mic drive this layer's sung features |
| Snap | Timeline: time grid for dragging | Tune: how hard pitch is pulled to the scale |
| Level | Layer volume | Tune's output level |

### Accessibility (WCAG 2.2 AA)

- **Contrast.** Text passed everywhere in the dark theme (no text below 4.5:1). Input borders (`--line` on the panel color) are about 2:1, under the 3:1 that 1.4.11 asks of a control's boundary.
- **Target size (2.5.8).** 42 visible controls under 24 px at 1440 px: the 32 pattern cells (22 px tall), the Feel and Triplets checkboxes and the five Influence aspect boxes (13 px), and the unstyled file inputs (21 px).
- **Keyboard.** Every pattern cell is its own Tab stop: 30 stops to reach the grid and 32 more to leave a two-layer, one-section song (an 8-layer, 64-row section would be 512). The timeline is focusable but does nothing with keys. No skip links. Space means "hold to capture" when the mic is on and "play or stop" when it is off, so the same key does two different things depending on a setting the user may not remember making.
- **Labels and status.** Every control has a name. Toggle buttons say on or off by color only in five places: Record (transport), MIDI learn, the scene buttons, the Sound view tabs and the pattern cursor.
- **Reduced motion.** Respected by the logo and the Klont.

### Color signals the owner cannot read

- **Kit slots on the timeline** are red (kick), blue (snare), green (hat) and amber (open). Kick and hat are a red-green pair, and the rows have no names.
- **Morph markers** are colored by the state they move to, from a palette with two red-green pairs, and the marker has no other mark of which state it is.
- **Toggles** listed above (color only).

## 3. How the parts fit together

Earworm is two tools in one: a track you build and edit (the DAW side) and an instrument you play (the performance side). Both act on the same project, layers and sounds. The interface should let a person move between them without losing their place, and every screen should answer three questions without reading: what is playing, what is selected, and what will the next press do.

### Three places, one transport

- **The transport** is always on screen, at every size. It holds what you reach for while the music runs: the section, tempo and position, Play and Stop, the three ways to put a take on the selected layer (Capture, Record, Import), Loop and the metronome, and Undo and Redo. It also says whether audio is on and whether the mic is on, and that is where Start lives until audio is running.
- **Track** is where the song is made: the layers and sections on the left, the pattern grid and the timeline in the middle, the selected layer's sound on the right.
- **Perform** is where the selected layer is played: keys or pads, the XY pad, the macros, the wheel, bend and pressure, scenes and MIDI, laid out to fill the screen. The layer list stays, so you can switch which layer you are playing.
- **Sound** (the selected layer's sound: Simple, Patch and Mod) is the inspector beside Track on a laptop, and its own place on a phone.

The rest is out of the way until asked for: **File** (the project name in the transport opens it), **Setup** (round trip, Measure, bands, the metronome's sound, the theme), and **Learn** (lessons and demos).

### The selection is the thread

One layer is selected everywhere. It is the row lit in the layer list, the lane lit in the timeline, the column the pattern cursor is in, the layer the Sound panel edits, the layer Perform plays and the layer Capture, Record and Import put a take on. The transport names it next to the capture buttons, so a press never goes somewhere unexpected.

### How one feature leads to the next

| After | The interface offers |
| --- | --- |
| Opening the app | Start (with or without the mic), a demo, or the lessons |
| A take lands | Quantize and the Feel row, on the layer the take went to, in the status line |
| Quantize | The Feel row opens and shows the grid and strength it set |
| Editing a sound | Perform, to play it; Mod, to route a source to the knob you are on |
| Playing in Perform | Record, which writes notes, knob moves and scene taps into the clip |
| Recording a pass | The automation lane it wrote, in Track |
| Finishing | Export mix, in File and in the transport's overflow |

### Keys

The keyboard is a first-class way in. Shortcuts never fire while typing in a text field.

| Key | Does |
| --- | --- |
| Space | Play or stop when the mic is off. With the mic on, hold to capture (unchanged; see the questions in UI1.md) |
| Esc | Stop |
| Ctrl/⌘ Z, Shift Ctrl/⌘ Z, Ctrl/⌘ Y | Undo, redo |
| Alt 1, Alt 2, Alt 3 | Track, Sound, Perform |
| ? | The list of keys |
| Pattern grid: arrows, Enter, Delete, Shift ↑↓, 1–4 | Move, add, delete, transpose a semitone, set a hit's kit slot |
| Timeline: ←→, Shift ↑↓, Delete | Move the selected notes or hits by the snap, transpose a semitone, delete |
| Perform: A W S E D F T G Y H U J K, Z and X | Play a chromatic octave from C, octave down and up |

The pattern grid and the timeline are each one Tab stop; the arrows move inside them.

## 4. Layout on every screen

One grid, five widths. Sizes are CSS pixels.

| Width | Name | Transport | Workspace |
| --- | --- | --- | --- |
| under 600 | Phone | One sticky row: Play, Stop, the capture buttons, position, and More (the rest) | One place at a time, picked from a bar at the bottom of the screen (Layers, Track, Sound, Perform). The page scrolls as a whole; nothing scrolls inside a box. The Klont and the voice meters are hidden; the ribbon shows while Capture is held. |
| 600 to 959 | Tablet portrait | Two rows | As the phone. |
| 960 to 1279 | Tablet landscape, small laptop | One row | Track: layers (200), the track (rest), Sound (340). Perform: layers (200), Perform (rest). |
| 1280 to 1919 | Laptop | One row | Track: layers (240), track, Sound (400). Perform: layers (240), Perform. |
| 1920 and over | Large monitor | One row | Track: layers (280), track, Sound (480), and the base text one step larger. The timeline takes the height the pattern grid does not need. |

Touch: on a coarse pointer every control is at least 44 px tall, the pattern cells 40, and the timeline pans with a finger on empty space. On a fine pointer controls are 32 px and nothing is under 24.

## 5. Design language

Plain and dense, like a hardware unit's front panel: the music is the color, the chrome is quiet. Amber marks what is on, selected or playing; blue marks modulation and links.

### Type

Two faces, both already shipped with the app: Atkinson Hyperlegible for words and Fragment Mono for numbers, readouts and the pattern.

| Token | Size | Use |
| --- | --- | --- |
| `--fs-0` | 12 px | Captions, legends, meter names |
| `--fs-1` | 13 px | Controls and body text (14 on a large monitor) |
| `--fs-2` | 15 px | Lesson titles, dialog text |
| `--fs-3` | 18 px | Dialog titles, the logo |

### Space

A 4 px step: `--s-1` 2, `--s-2` 4, `--s-3` 8, `--s-4` 12, `--s-5` 16, `--s-6` 24. Controls sit 4 apart in a group and groups 12 apart.

### Color

Every pair below is checked: text 4.5:1 or more against the surface it sits on, control boundaries and focus rings 3:1.

| Token | Dark | Light | Use |
| --- | --- | --- | --- |
| `--bg` | `#0e1218` | `#f3f0e9` | Page |
| `--panel` | `#171e28` | `#fbfaf6` | Panels, toolbars |
| `--panel-2` | `#212a36` | `#e9e4d8` | Buttons |
| `--well` | `#0a0d12` | `#ffffff` | Inputs, the pattern, the timeline |
| `--line` | `#3d5166` | `#c4bcab` | Dividers |
| `--line-strong` | `#6a86a2` | `#7d7565` | Control boundaries |
| `--text` | `#e7eef6` | `#1a1e23` | Text |
| `--dim` | `#b4c4d4` | `#4c545e` | Secondary text |
| `--accent` | `#f0b429` | `#9a5f00` | On, selected, playing; focus ring |
| `--hot` | `#ffe6b0` | `#5e3a00` | Text on an amber-tinted surface |
| `--amber-dim` | `#3a2c14` | `#fbe4ad` | Background of what is on or selected |
| `--signal` | `#9ec1ff` | `#1d5bb8` | Modulation, links |
| `--danger` | `#ff8f78` | `#b02f14` | Errors, always with "!" and words |

Kit slots and morph states keep their own hues, chosen so no two neighbours are a red-green pair, and both are also told apart without color: kit slots by a named row, morph markers by a count of notches.

### Motion

`--move` is 160 ms ease-out for state changes (a panel switch, a pressed button); nothing moves on its own except the playhead, the meters, the Klont and the logo. With reduced motion, transitions drop to nothing and the Klont stops wobbling.

### Icons

Text glyphs, always beside a word, never instead of one: ▶ Play, ■ Stop, ● Record, ◉ Capture. A toggle carries a ring that fills when it is on (○ off, ● on), so on and off differ in shape as well as color.

## 6. Change list

In priority order. P0 is broken or inaccessible; P1 is the model above; P2 is polish.

| # | Change | Priority |
| --- | --- | --- |
| 1 | Show the pattern grid on desktop (flex shrink) | P0 |
| 2 | Color tokens for both themes, a light theme that follows the system, and a Light / Dark / System switch; the timeline, ribbon and shaper read the tokens | P0 |
| 3 | Kit slot colors without a red-green pair, named slot rows on the timeline, notches on morph markers, a shape on every toggle | P0 |
| 4 | Targets: 24 px minimum everywhere, 44 on touch; checkboxes and file inputs sized as controls | P0 |
| 5 | Touch editing on the timeline, and panning on empty space | P0 |
| 6 | One Tab stop for the pattern grid (roving focus); keys on the timeline; skip links; the key list on ? | P0 |
| 7 | Transport: one bar with Start and the audio and mic state; setup moved to a Setup dialog; Capture, Record and Import grouped with the selected layer's name | P1 |
| 8 | Perform as its own place beside Track, full width, with the computer keyboard playing notes | P1 |
| 9 | Phone and tablet: sticky transport, bottom place bar, the page scrolls as a whole, Klont and meters hidden | P1 |
| 10 | Large monitor: wider columns, larger base text, the timeline takes the free height | P1 |
| 11 | Names: section Time (not Feel), Groove (Straight or Swing), MIDI learn, Listen (hold) and Teach (hold), Pitch snap | P1 |
| 12 | The next step after a take, and after Quantize | P1 |
| 13 | File grouped (project, share, remove), with Export mix first and Delete apart | P1 |
| 14 | The Feel row folds behind a summary; hints shortened and kept to one line where they repeat the lessons | P2 |
| 15 | The Mod view's route table fits its column | P2 |

What was built, and the screenshots after it, are in section 7 once the pass is done.
