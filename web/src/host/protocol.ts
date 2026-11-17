// Wire formats shared with crates/engine/src/protocol.rs and
// crates/dsp/src/{params,frame}.rs. Keep these in step with the Rust side;
// protocol.test.ts checks the parameter list against params.rs.

export const MSG_BYTES = 16;
export const CONTROL_CAP = 256;

export const MSG_SET_PARAM = 1;
export const MSG_SET_TONE_BAND = 2;
export const MSG_SET_NOISE_BAND = 3;
export const MSG_NOTE_ON = 4;
export const MSG_NOTE_OFF = 5;
export const MSG_SET_LIVE = 6;
export const MSG_RESET_SINGER = 7;
export const MSG_PANIC = 8;

// Feature frames (dsp::frame)
export const FRAME_RATE = 100;
export const MAX_BANDS = 24;
export const HEADER = 8;
export const MAX_FRAME_LEN = HEADER + MAX_BANDS;
export const F_PITCH = 0;
export const F_CONFIDENCE = 1;
export const F_LOUDNESS = 2;
export const F_OPENNESS = 3;
export const F_BREATHINESS = 4;
export const F_ONSET = 5;
export const F_HARDNESS = 6;
export const F_VOICING = 7;
export const F_BANDS = 8;
export const VOICED = 0;
export const UNVOICED = 1;
export const SILENT = 2;
export const frameLen = (nBands: number) => HEADER + nBands;

// Frame ring records: a frame (MAX_FRAME_LEN floats) plus tuned pitch.
export const RECORD_STRIDE = MAX_FRAME_LEN + 1;
export const RECORD_TUNED = MAX_FRAME_LEN;
export const MAX_FRAMES_PER_CALL = 8;

// Meter block slots
export const M_BLOCKS = 0;
export const M_INPUT_DB = 1;
export const M_FLOOR_DB = 2;
export const M_OUTPUT_DB = 3;
export const M_GATE = 4;
export const M_PITCH = 5;
export const M_CONF = 6;
export const M_TUNED = 7;
export const M_LOUD = 8;
export const M_OPEN = 9;
export const M_BREATH = 10;
export const M_HARD = 11;
export const M_VOICING = 12;
export const M_BLEED = 13;
export const M_ONSET = 14;
export const M_LIVE = 15;
export const M_F0 = 16;
export const M_VOICED_MIX = 17;
export const M_ACTIVE_PARTIALS = 18;
export const M_N_BANDS = 19;
export const M_VOWEL = 20;
export const M_TONE_EFF = 44;
export const M_DELAY_LEVEL = 68;
export const M_DELAY_PITCH = 69;
export const METER_LEN = 72;

// BufferTable returned by ew_buffers: 12 u32 fields, in this order.
export const BT_INPUT = 0;
export const BT_OUTPUT = 1;
export const BT_MAX_BLOCK = 2;
export const BT_CONTROL = 3;
export const BT_CONTROL_CAP = 4;
export const BT_CONTROL_COUNT = 5;
export const BT_METER = 6;
export const BT_METER_LEN = 7;
export const BT_FRAMES = 8;
export const BT_RECORD_STRIDE = 9;
export const BT_FRAMES_COUNT = 10;
export const BT_N_BANDS = 11;
export const BT_FIELDS = 12;

/** Engine wasm exports (no imports needed to instantiate). */
export interface EngineExports {
  memory: WebAssembly.Memory;
  ew_init(sampleRate: number, maxBlock: number, nBands: number): number;
  ew_buffers(e: number): number;
  ew_process(e: number, frames: number): void;
  ew_render(e: number, frames: number): number;
  ew_free(e: number): void;
}

export interface ParamDesc {
  name: string;
  min: number;
  max: number;
  default: number;
}

const p = (name: string, min: number, max: number, def: number): ParamDesc => ({ name, min, max, default: def });

/** Same order as dsp::params::PARAMS (the index is the wire id). */
export const PARAMS: ParamDesc[] = [
  p('harm_mix', 0, 1, 1),
  p('noise_mix', 0, 1, 0.15),
  p('tilt', -24, 6, -6),
  p('oddeven', -1, 1, 0),
  p('stretch', 0, 0.01, 0),
  p('modal', 0, 2, 0),
  p('damp', 0, 1, 0),
  p('f1_freq', 200, 1200, 600),
  p('f1_gain', 0, 24, 0),
  p('f2_freq', 700, 3500, 1500),
  p('f2_gain', 0, 24, 0),
  p('env.attack', 0.001, 2, 0.01),
  p('env.hold', 0, 2, 0),
  p('env.decay', 0.01, 4, 0.3),
  p('env.sustain', 0, 1, 0.8),
  p('env.release', 0.005, 4, 0.15),
  p('env.noise_decay', 0.005, 4, 0.2),
  p('env.pitch_amt', -24, 48, 0),
  p('env.pitch_time', 0.001, 0.5, 0.05),
  p('depth.loud', 0, 1, 1),
  p('depth.open', 0, 1, 0.5),
  p('depth.hard', 0, 1, 0.5),
  p('depth.breath', 0, 1, 0.5),
  p('depth.mouth', 0, 1, 1),
  p('tune.snap', 0, 1, 0),
  p('tune.speed', 0, 250, 60),
  p('tune.slides', 0, 2, 0),
  p('tune.vibrato', 0, 2, 1),
  p('tune.root', 0, 11, 0),
  p('tune.scale', 0, 5, 0),
  p('tune.ref_cents', -50, 50, 0),
  p('tune.octave', -2, 2, 0),
  p('level', -60, 6, -12),
  p('partials', 1, 64, 48),
];

/** Parameter id by name, e.g. P['depth.mouth']. */
export const P: Record<string, number> = Object.fromEntries(PARAMS.map((d, i) => [d.name, i]));

export const SCALES = ['chromatic', 'major', 'minor', 'major pentatonic', 'minor pentatonic', 'blues'];

/** Influence match result layout (analysis::MATCH_LEN floats). */
export const MATCH_TONE = PARAMS.length;
export const MATCH_NOISE = PARAMS.length + MAX_BANDS;
export const MATCH_PITCH = PARAMS.length + 2 * MAX_BANDS;
export const MATCH_DURATION = MATCH_PITCH + 1;
export const MATCH_VOICED = MATCH_PITCH + 2;
export const MATCH_PITCHED = MATCH_PITCH + 3;
export const MATCH_LEN = MATCH_PITCH + 4;

export const centsToHz = (c: number) => 16.351598 * Math.pow(2, c / 1200);
export const hzToCents = (hz: number) => 1200 * Math.log2(hz / 16.351598);
