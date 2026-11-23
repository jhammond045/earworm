// Walkthroughs are data. A step names the control to highlight and the action that advances it.
// Add a lesson by appending to LESSONS; the app reads this list and nothing else.

export type Advance =
  | 'next'
  | 'start'
  | 'play'
  | 'metro'
  | 'note'
  | 'hit'
  | 'capture'
  | 'influence'
  | 'state'
  | 'marker'
  | 'save'
  | 'timeline'
  | 'macro'
  | 'gesture'
  | 'kit'
  | 'tune'
  | 'feel'
  | 'loop'
  | 'layer'
  | 'quantize'
  | 'measure'
  | 'source'
  | 'teach'
  | 'take'
  | 'wav'
  | 'bands'
  | 'view'
  | 'pick'
  | 'route'
  | 'amount';

export interface LessonStep {
  id: string;
  title: string;
  body: string;
  /** Matches a `data-tour` attribute on the control to highlight. */
  target: string;
  advance: Advance;
  /** Button label when the step advances on its own, or the hint of the action to perform. */
  action: string;
}

export interface Lesson {
  id: string;
  title: string;
  blurb: string;
  /** Demo id to open when the lesson starts, if any. */
  demo?: string;
  steps: LessonStep[];
}

export const LESSONS: Lesson[] = [
  {
    id: 'around',
    title: 'Find your way around',
    blurb: 'The transport, the pattern and the panels.',
    steps: [
      { id: 'start', title: 'Start the sound', target: 'start', advance: 'start', action: 'Press Start',
        body: 'Earworm plays in the browser. Press Start to open the audio. Use Start with mic only when you are ready to sing; nothing turns the microphone on until you ask.' },
      { id: 'play', title: 'Play the section', target: 'transport', advance: 'play', action: 'Press Play',
        body: 'One transport runs the whole track. Play starts at the selected section. Stop ends it. The highlighted pattern row is the beat you are hearing.' },
      { id: 'metro', title: 'Turn on the click', target: 'metro', advance: 'metro', action: 'Turn the metronome on',
        body: 'The click sits on the beat, louder on the downbeat. It is heard live only: a recording from the mic and an export do not contain it. Setup holds its level, the beats in a bar (also the length of the count-in), and Kit, which uses the selected beat layer’s slot instead of the plain tick.' },
      { id: 'grid', title: 'Read the pattern', target: 'pattern', advance: 'next', action: 'Next',
        body: 'Each column is a layer. Each row is a 16th note of the section. A letter is a note, a short word is a drum. The row that is playing is highlighted. Arrow keys move the cursor.' },
      { id: 'layers', title: 'Layers and sections', target: 'layers', advance: 'next', action: 'Next',
        body: 'The layer list is the instrument bank. Pick one to edit its sound. A circle is a melody, a square is a beat, a diamond is a texture, and a muted layer says mute. The stepper at the top changes which section the pattern shows.' },
      { id: 'section', title: 'Shape the section', target: 'section-edit', advance: 'next', action: 'Next',
        body: 'Name the section, set how many beats it lasts, and how many times it repeats. Copy makes the next section from this one. Remove drops one. The last section stays.' },
    ],
  },
  {
    id: 'write',
    title: 'Write a pattern',
    blurb: 'Notes, hits, and undo.',
    demo: 'melody',
    steps: [
      { id: 'note', title: 'Add a note', target: 'pattern', advance: 'note', action: 'Enter on an empty cell',
        body: 'Move with the arrows and press Enter, or double-click a cell. On a melody layer that adds a note. Shift with the up and down arrows changes its pitch. Delete removes it. Undo is one history for the whole app.' },
      { id: 'hit', title: 'Add a hit', target: 'layers', advance: 'hit', action: 'Add a hit on the Beat layer',
        body: 'Select the Beat layer, then Enter on a row. The cell shows the kit slot (kick, snare, hat, open). Keys 1 to 4 set the slot of the selected hit.' },
      { id: 'undo', title: 'Undo', target: 'undo', advance: 'next', action: 'Next',
        body: 'Undo and redo walk one stack, including notes, sounds, sections and Influence. The same stack is saved with the project, so a reload keeps it.' },
    ],
  },
  {
    id: 'capture',
    title: 'Capture onto the track',
    blurb: 'Hold to record, with a count-in.',
    steps: [
      { id: 'mic', title: 'Enable the mic', target: 'start', advance: 'next', action: 'Next',
        body: 'Start with mic, or press Enable mic after starting without one. Use wired headphones. Nothing checks for feedback or mutes the speakers.' },
      { id: 'hold', title: 'Hold to capture', target: 'capture', advance: 'capture', action: 'Hold Capture',
        body: 'Hold Capture, or hold C. If the track is stopped you hear one bar of count-in, then the section starts and the take is placed where you sang. Let go to keep it. A take under half a second is ignored.' },
      { id: 'align', title: 'Line the take up', target: 'align', advance: 'next', action: 'Next',
        body: 'Align is the round trip, in milliseconds, from the speakers back to the mic. The take is placed that much earlier, so a note you sang on the beat lands on the beat. 30 ms is the guess until you have measured your headphones.' },
    ],
  },
  {
    id: 'sound',
    title: 'Influence, states and the morph lane',
    blurb: 'Move a sound, then morph between states.',
    demo: 'states',
    steps: [
      { id: 'influence', title: 'Match a sound', target: 'influence', advance: 'influence', action: 'Set an Influence',
        body: 'Choose an audio file, or hold Listen and make a sound. The layer moves toward it. Amount and the aspect toggles decide how much of Tone, Shape, Motion, Grit and Space is used. Commit writes the blend into the state.' },
      { id: 'state', title: 'Add a state', target: 'states', advance: 'state', action: 'Add a state',
        body: 'A layer can hold several states. The sound controls edit the state selected here. Add a second one, or Copy the one you are editing. Remove drops a state when more than one remains. Then change its preset or macros.' },
      { id: 'pan', title: 'Place the layer', target: 'pan', advance: 'next', action: 'Next',
        body: 'Level is how loud, in decibels. Pan moves the layer left or right. The letters beside the slider read L, C or R, so the side is not only a position on the track. Mute silences the layer and the list says mute.' },
      { id: 'marker', title: 'Place a morph marker', target: 'morph', advance: 'marker', action: 'Add a marker',
        body: 'A marker on the morph lane moves the layer from the previous state to this one. Pick the shape and the length. While the track plays, the sound follows the lane.' },
      { id: 'timeline', title: 'Read the timeline', target: 'timeline', advance: 'timeline', action: 'Click the timeline',
        body: 'Under the pattern, each lane is a layer drawn in time. A bar is a written note. A ribbon is the pitch and loudness of a take. Drag a bar to move it, drag its right edge to change its length, and double-click empty space to add. The strip under a lane is the morph.' },
      { id: 'macros', title: 'Move a macro', target: 'macros', advance: 'macro', action: 'Move a macro',
        body: 'Macros are eight named moves on the state you are editing: bright, body, wobble, grit, space, punch, air and length. They sit on top of the preset. The sound updates while you drag, and letting go writes one undo.' },
      { id: 'gestures', title: 'Set a gesture depth', target: 'gestures', advance: 'gesture', action: 'Move a gesture',
        body: 'Gesture depth is how far a sung loudness, openness, hardness, breath or mouth shape pushes this layer. It is the live performance path, separate from the written notes.' },
    ],
  },
  {
    id: 'kit',
    title: 'Use your sound for a kit slot',
    blurb: 'One hit becomes a drum.',
    demo: 'beat',
    steps: [
      { id: 'slot', title: 'Match one hit', target: 'kit', advance: 'kit', action: 'Record or choose a hit',
        body: 'On a beat layer, pick a slot. Hold Record one hit and make a single sound, or choose a file with one hit. The match treats it as a decaying sound and replaces that slot. Hear slot plays it. The mic must already be on for recording.' },
    ],
  },
  {
    id: 'edit',
    title: 'Edit the arrangement',
    blurb: 'Tune, feel, layers and the timeline.',
    demo: 'melody',
    steps: [
      { id: 'tune', title: 'Tune a take', target: 'tune', advance: 'tune', action: 'Move a Tune control',
        body: 'Tune shapes the pitch that was sung: snap pulls it to the scale, slides choose glide, gliss or jump, and octave shifts the whole line. Fill from take copies the root, the scale and the reference the second pass found. A new take does that on its own.' },
      { id: 'feel', title: 'Grid or free time', target: 'feel', advance: 'feel', action: 'Change the time',
        body: 'Grid counts beats at the tempo. Free counts seconds. Ramp is how long the tempo takes to arrive from the previous section. Earlier and Later move this section in the song.' },
      { id: 'loop', title: 'Loop the song', target: 'loop', advance: 'loop', action: 'Switch the loop',
        body: 'Loop on returns to the first section after the last. Loop off plays the song once. The click’s Bar, in Setup, is how many beats are in a bar, and how long the count-in lasts.' },
      { id: 'layer', title: 'Rename a layer', target: 'layer-edit', advance: 'layer', action: 'Rename the layer',
        body: 'The name is the column you see on the pattern. Remove layer drops it, and its notes, when another layer remains.' },
      { id: 'quantize', title: 'Zoom, snap and quantize', target: 'quantize', advance: 'quantize', action: 'Zoom or quantize',
        body: 'Zoom changes how much of the timeline you see. Snap is the grid a drag lands on. Quantize turns on Feel for this lane: it plays the notes or hits on that grid while keeping how you played them, so Strength can bring some of the feel back. Drag a hit up or down to put it on another kit slot.' },
    ],
  },
  {
    id: 'further',
    title: 'Measure, match and keep',
    blurb: 'The round trip, Influence, the kit and the mix.',
    steps: [
      { id: 'measure', title: 'Measure the round trip', target: 'measure', advance: 'measure', action: 'Press Measure',
        body: 'Measure plays five chirps and times how long they take to come back through the mic. That number replaces the Align guess, so a note you sang on the beat lands on the beat. Use headphones. If the audio device changes, Capture pauses and asks you to measure again.' },
      { id: 'source', title: 'Use a saved sound', target: 'source', advance: 'source', action: 'Play a source or use a saved one',
        body: 'Saved lists sounds kept by the Influence page. Use saved matches one into this layer. Play source plays the file you matched. Hold note plays this layer at that pitch. The meters show how far each aspect moved, and say when motion is still provisional. The name field renames the state you are editing.' },
      { id: 'takes', title: 'Pick a take', target: 'takes', advance: 'take', action: 'Choose a take',
        body: 'Each capture stacks on the last. The menu chooses which take the ribbon follows. The written notes stay as they are.' },
      { id: 'teach', title: 'Teach a kit slot', target: 'teach', advance: 'teach', action: 'Hold Teach',
        body: 'On a beat layer, hold Teach and repeat one sound. The next beat take groups hits with what you taught, and a live hit can follow that slot.' },
      { id: 'wav', title: 'Export the mix', target: 'wav', advance: 'wav', action: 'Press Export mix',
        body: 'File, then Export mix, renders the song to a stereo WAV. It is one pass, faster than real time. The click is not in the file. Open File to reach the button.' },
      { id: 'klont', title: 'Watch the Klont', target: 'klont', advance: 'next', action: 'Next',
        body: 'The glyph’s spikes follow bright and grit, its wobble follows motion, and the halo follows space. The ribbon beside it grows under your voice while Capture is held.' },
      { id: 'bands', title: 'Bands', target: 'bands', advance: 'bands', action: 'Set the bands',
        body: '16 is the song engine’s vowel bank, and the width a take is stored at. 24 is the front end on the Live page. Choosing it here does not widen the song engine, so a take and the timeline stay one frame wide.' },
    ],
  },
  {
    id: 'keep',
    title: 'Keep the project',
    blurb: 'Save, export and open a demo.',
    steps: [
      { id: 'save', title: 'Save', target: 'file', advance: 'save', action: 'Press Save',
        body: 'The project is already journaled as you work. Save writes it now. Save as stores a copy under a new name. Open, Rename and Delete act on the projects kept in this browser.' },
      { id: 'export', title: 'Export a zip', target: 'file', advance: 'next', action: 'Next',
        body: 'Export downloads one zip: the project and the audio it uses. Import reads that zip back, including a project saved by an older version of Earworm. Clean up drops takes and sources nothing in the song or the undo stack still needs.' },
      { id: 'demo', title: 'Demos', target: 'help', advance: 'next', action: 'Done',
        body: 'Demos are small songs you can open and edit: a melody, a beat, and a texture with states. They are ordinary projects, so Save as keeps your changes without replacing the built-in one.' },
    ],
  },
  {
    id: 'hardness',
    title: 'Make hardness set the attack',
    blurb: 'Route a source to a knob, then shape the route.',
    demo: 'melody',
    steps: [
      { id: 'patch', title: 'Open the Patch view', target: 'patch-view', advance: 'view', action: 'Press Patch',
        body: 'The Sound panel has three views. Simple is the preset, the states and the macros. Patch shows every knob of the selected layer’s sound. Mod lists the routes that move those knobs.' },
      { id: 'pick', title: 'Pick hardness', target: 'mod-pick', advance: 'pick', action: 'Choose Note hardness',
        body: 'Modulate with picks a source. Note hardness is how hard each note starts: the strength of a sung onset, or a written note’s hardness. A + appears beside every knob it can reach.' },
      { id: 'route', title: 'Route it to the attack', target: 'env1', advance: 'route', action: 'Press + beside attack in env1',
        body: 'env1 is the envelope that shapes each note’s level. Press + beside its attack. The slider gets a bar showing how far the route can move it, and a badge naming the source.' },
      { id: 'amount', title: 'Harder means quicker', target: 'route-amount', advance: 'amount', action: 'Drag the amount below 0',
        body: 'The Mod view shows the new route. Drag Amount left of the middle: the harder a note starts, the shorter its attack. The curve above it shapes how hardness maps onto the attack.' },
    ],
  },
];

export function lessonById(id: string): Lesson | undefined {
  return LESSONS.find((l) => l.id === id);
}
