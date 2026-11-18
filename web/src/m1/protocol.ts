// TypeScript mirror of crates/klont/src/protocol.rs. protocol.test.ts parses
// the Rust file and checks every constant here, so keep names identical.

// ---- limits
export const MAX_LAYERS = 8;
export const VOICES_PER_LAYER = 8;
export const KIT_SLOTS = 4;
export const N_MACROS = 8;
export const MAX_CURVES = 64;
export const CURVE_POOL_FLOATS = 1 << 21;
export const STAGING_BYTES = 4 << 20;

// ---- control ring
export const MSG_BYTES = 16;
export const CONTROL_CAP = 256;

export const MSG_SET_PARAM = 1;
export const MSG_SET_MACRO = 2;
export const MSG_SET_LAYER = 3;
export const MSG_NOTE_ON = 4;
export const MSG_NOTE_OFF = 5;
export const MSG_PLAY = 6;
export const MSG_STOP = 7;
export const MSG_JUMP = 8;
export const MSG_SET_BUDGET = 9;
export const MSG_SELECT_LAYER = 10;
export const MSG_PANIC = 11;
export const MSG_ARM = 12;
export const MSG_SET_TUNE = 13;
export const MSG_SET_LIVE_KIT = 14;
export const MSG_RESET_SINGER = 15;

export const SET_BASE = 0;
export const SET_KIT = 1;
export const SET_INFLUENCE = 5;

export const LAYER_LEVEL = 0;
export const LAYER_PAN = 1;
export const LAYER_MUTE = 2;
export const LAYER_KIND = 3;
export const LAYER_DEPTH_LOUD = 4;
export const LAYER_DEPTH_OPEN = 5;
export const LAYER_DEPTH_HARD = 6;
export const LAYER_DEPTH_BREATH = 7;
export const LAYER_DEPTH_MOUTH = 8;
export const LAYER_FOLLOW = 9;
export const LAYER_INFLUENCE_AMOUNT = 10;
export const LAYER_INFLUENCE_MASK = 11;
export const LAYER_KIT_CENTS = 12;
export const LAYER_FIELDS = 16;

export const TUNE_SNAP = 0;
export const TUNE_SPEED = 1;
export const TUNE_SLIDES = 2;
export const TUNE_VIBRATO = 3;
export const TUNE_ROOT = 4;
export const TUNE_SCALE = 5;
export const TUNE_REF_CENTS = 6;
export const TUNE_OCTAVE = 7;
export const TUNE_FIELDS = 8;

export const LIVE_KIT_TAUGHT = 0;
export const LIVE_KIT_BRIGHT = 1;
export const LIVE_KIT_LOW = 2;
export const LIVE_KIT_NOISE = 3;
export const LIVE_KIT_FIELDS = 4;

export const KIND_MELODY = 0;
export const KIND_BEAT = 1;
export const KIND_TEXTURE = 2;

export const GROUP_TONE = 0;
export const GROUP_SHAPE = 1;
export const GROUP_MOTION = 2;
export const GROUP_GRIT = 3;
export const GROUP_SPACE = 4;

export const BUDGET_MAX = 4096;
export const BUDGET_MIN = 256;

// ---- ew_load
export const LOAD_TRACK = 1;
export const LOAD_LAYER = 2;
export const LOAD_CURVE = 3;
export const LOAD_GRAIN = 4;
export const LOAD_STATES = 5;

export const LOAD_OK = 0;
export const LOAD_BAD_KIND = -1;
export const LOAD_BAD_SLOT = -2;
export const LOAD_BAD_MAGIC = -3;
export const LOAD_TOO_BIG = -4;
export const LOAD_MALFORMED = -5;

export const LAYER_MAGIC = 0x314c5745;
export const LAYER_WORDS = 1 + LAYER_FIELDS + 96 + N_MACROS + 96 + KIT_SLOTS * 96;
export const LAYER_OFF_FIELDS = 1;
export const LAYER_OFF_BASE = LAYER_OFF_FIELDS + LAYER_FIELDS;
export const LAYER_OFF_MACROS = LAYER_OFF_BASE + 96;
export const LAYER_OFF_INFLUENCE = LAYER_OFF_MACROS + N_MACROS;
export const LAYER_OFF_KIT = LAYER_OFF_INFLUENCE + 96;

export const LOAD_NAMES: Record<number, string> = {
  [LOAD_OK]: 'ok', [LOAD_BAD_KIND]: 'bad kind', [LOAD_BAD_SLOT]: 'bad slot',
  [LOAD_BAD_MAGIC]: 'bad magic', [LOAD_TOO_BIG]: 'too big', [LOAD_MALFORMED]: 'malformed',
};

export const GRAIN_MAGIC = 0x31475745;
export const GRAIN_MAX_SAMPLES = 480000;
export const GRAIN_HEADER_WORDS = 4;
export const STATES_MAGIC = 0x31535745;
export const MAX_STATES = 8;
export const STATE_WORDS = 96 + N_MACROS + 1 + 3;

// ---- meter block
export const M_BLOCKS = 0;
export const M_PLAYING = 1;
export const M_SECTION = 2;
export const M_SECTION_POS = 3;
export const M_REPEAT = 4;
export const M_BPM = 5;
export const M_LOAD = 6;
export const M_BUDGET = 7;
export const M_ACTIVE_PARTIALS = 8;
export const M_ACTIVE_VOICES = 9;
export const M_STEALS = 10;
export const M_OVERLOADS = 11;
export const M_LOAD_STATUS = 12;
export const M_PEAK_L = 13;
export const M_PEAK_R = 14;
export const M_SELECTED = 15;
export const M_LAYER_PEAK = 16;
export const M_LAYER_VOICES = 24;
export const M_STORED = 32;
export const M_LIVE = 128;
export const M_MACRO_LIVE = 224;
export const M_IN_DB = 240;
export const M_FLOOR_DB = 241;
export const M_GATE = 242;
export const M_PITCH = 243;
export const M_CONF = 244;
export const M_TUNED = 245;
export const M_LOUD = 246;
export const M_OPEN = 247;
export const M_BREATH = 248;
export const M_HARD = 249;
export const M_VOICING = 250;
export const M_ONSET = 251;
export const M_ARMED = 252;
export const M_DELAY_LEVEL = 253;
export const M_DELAY_PITCH = 254;
export const M_LIVE_SLOT = 255;
export const M_LIVE_HITS = 256;
export const M_CLOCK_LO = 257;
export const M_CLOCK_HI = 258;
export const M_UNITS_PER_SAMPLE = 259;
export const M_VOWEL = 264;
export const METER_LEN = 280;

// ---- buffer table (u32 fields, in this order; BT_* are field indices)
export const BT_INPUT = 0;
export const BT_OUTPUT_L = 1;
export const BT_OUTPUT_R = 2;
export const BT_MAX_BLOCK = 3;
export const BT_CONTROL = 4;
export const BT_CONTROL_CAP = 5;
export const BT_CONTROL_COUNT = 6;
export const BT_METER = 7;
export const BT_METER_LEN = 8;
export const BT_STAGING = 9;
export const BT_STAGING_CAP = 10;
export const BT_HOST_LOAD = 11;
export const BT_FRAMES = 12;
export const BT_FRAMES_CAP = 13;
export const BT_FRAMES_COUNT = 14;
export const BT_FIELDS = 15;

export interface BufferTable {
  input: number; outputL: number; outputR: number; maxBlock: number;
  control: number; controlCap: number; controlCount: number;
  meter: number; meterLen: number; staging: number; stagingCap: number; hostLoad: number;
  frames: number; framesCap: number; framesCount: number;
}

export function readBufferTable(mem: ArrayBuffer, ptr: number): BufferTable {
  const t = new Uint32Array(mem, ptr, BT_FIELDS);
  return {
    input: t[BT_INPUT], outputL: t[BT_OUTPUT_L], outputR: t[BT_OUTPUT_R], maxBlock: t[BT_MAX_BLOCK],
    control: t[BT_CONTROL], controlCap: t[BT_CONTROL_CAP], controlCount: t[BT_CONTROL_COUNT],
    meter: t[BT_METER], meterLen: t[BT_METER_LEN], staging: t[BT_STAGING], stagingCap: t[BT_STAGING_CAP],
    hostLoad: t[BT_HOST_LOAD], frames: t[BT_FRAMES], framesCap: t[BT_FRAMES_CAP], framesCount: t[BT_FRAMES_COUNT],
  };
}

/** Exports of klont.wasm (engine.rs "C ABI"). Pointers are byte offsets. */
export interface EngineExports {
  memory: WebAssembly.Memory;
  ew_init(sampleRate: number, maxBlock: number): number;
  ew_buffers(e: number): number;
  ew_process(e: number, frames: number): void;
  ew_render(e: number, frames: number): number;
  ew_load(e: number, slot: number, kind: number, ptr: number, len: number): number;
  ew_seek(e: number, section: number, ticks: number): void;
  ew_free(e: number): void;
}

/** Control message, 16 bytes: type u16, layer u8, index u8, param u16, pad u16, value f32, time u32. */
export function encodeMessage(view: DataView, offset: number, type: number, layer: number, index: number,
                              param: number, value: number, time = 0): void {
  view.setUint16(offset, type, true);
  view.setUint8(offset + 2, layer);
  view.setUint8(offset + 3, index);
  view.setUint16(offset + 4, param, true);
  view.setUint16(offset + 6, 0, true);
  view.setFloat32(offset + 8, value, true);
  view.setUint32(offset + 12, time >>> 0, true);
}

export type Msg = [type: number, layer: number, index: number, param: number, value: number, time?: number];

/** Instantiate klont.wasm and set up an engine. Shared by the worklet, worker and tests. */
export function instantiateEngine(bytes: BufferSource, sampleRate: number, maxBlock: number):
    { ex: EngineExports; e: number } {
  const ex = new WebAssembly.Instance(new WebAssembly.Module(bytes), {}).exports as unknown as EngineExports;
  return { ex, e: ex.ew_init(sampleRate, maxBlock) };
}
