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
  | 'kit';

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
        body: 'The click sits on the beat, louder on the downbeat. It is heard live only: a recording from the mic and an export do not contain it. Level sets how loud. Kit uses the selected beat layer’s slot instead of the plain tick.' },
      { id: 'grid', title: 'Read the pattern', target: 'pattern', advance: 'next', action: 'Next',
        body: 'Each column is a layer. Each row is a 16th note of the section. A letter is a note, a short word is a drum. The row that is playing is highlighted. Arrow keys move the cursor.' },
      { id: 'layers', title: 'Layers and sections', target: 'layers', advance: 'next', action: 'Next',
        body: 'The layer list is the instrument bank. Pick one to edit its sound. Sections are the song order: the stepper at the top changes which section the pattern shows, and the bpm of a grid section.' },
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
        body: 'Hold Capture, or hold Space. If the track is stopped you hear one bar of count-in, then the section starts and the take is placed where you sang. Let go to keep it. A take under half a second is ignored.' },
    ],
  },
  {
    id: 'sound',
    title: 'Influence, states and the morph lane',
    blurb: 'Move a sound, then morph between states.',
    demo: 'states',
    steps: [
      { id: 'influence', title: 'Match a sound', target: 'influence', advance: 'influence', action: 'Set an Influence',
        body: 'Choose an audio file or hold Listen and make a sound. The layer moves toward it. Amount and the aspect toggles decide how much of Tone, Shape, Motion, Grit and Space is used. Commit writes the blend into the state.' },
      { id: 'state', title: 'Add a state', target: 'states', advance: 'state', action: 'Add a state',
        body: 'A layer can hold several states. The sound controls edit the state selected here. Add a second one and change its preset or macros.' },
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
];

export function lessonById(id: string): Lesson | undefined {
  return LESSONS.find((l) => l.id === id);
}
