// TypeScript mirror of crates/klont/src/protocol.rs. protocol.test.ts parses
// the Rust file and checks every constant here, so keep names identical.

// ---- limits
export const MAX_LAYERS = 8;
export const VOICES_PER_LAYER = 8;
export const KIT_SLOTS = 4;
export const N_MACROS = 8;
export const MAX_VALUES = 1024;
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
export const MSG_SET_METRO = 16;
export const MSG_COUNT_IN = 17;
/** layer (ALL_LAYERS: every layer), index 0 mod wheel (0..1), 1 pitch bend (-1..1), 2 pressure (0..1). */
export const MSG_SET_PERF = 18;
/** layer, value 1 = listen (its sung features come from the mic), 0 = stop. */
export const MSG_LISTEN = 19;
/** Scene: layer, index = state, value = ms. Morph from where the layer is now to that state. */
export const MSG_MORPH_TO = 20;
/** Recording (S5): 1 while a pass records; a hand move then takes its target from its lane until the transport stops. */
export const MSG_RECORD = 21;
export const ALL_LAYERS = 255;

export const METRO_ON = 0;
export const METRO_LEVEL = 1;
export const METRO_VOICE = 2;
export const METRO_LAYER = 3;
export const METRO_SLOT = 4;
export const METRO_BAR = 5;

export const SET_BASE = 0;
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
export const LOAD_SAMPLE = 4;
export const LOAD_STATES = 5;
export const LOAD_PATCH = 6;

export const LOAD_OK = 0;
export const LOAD_BAD_KIND = -1;
export const LOAD_BAD_SLOT = -2;
export const LOAD_BAD_MAGIC = -3;
export const LOAD_TOO_BIG = -4;
export const LOAD_MALFORMED = -5;

export const LAYER_MAGIC = 0x324c5745;
export const LAYER_OFF_FIELDS = 1;
export const LAYER_OFF_N = LAYER_OFF_FIELDS + LAYER_FIELDS;
export const LAYER_HEADER_WORDS = LAYER_OFF_N + 1;
/** Words in a layer blob v2 for a patch of `n` values. */
export const layerWords = (n: number): number => LAYER_HEADER_WORDS + 3 * n + N_MACROS;

export const LOAD_NAMES: Record<number, string> = {
  [LOAD_OK]: 'ok', [LOAD_BAD_KIND]: 'bad kind', [LOAD_BAD_SLOT]: 'bad slot',
  [LOAD_BAD_MAGIC]: 'bad magic', [LOAD_TOO_BIG]: 'too big', [LOAD_MALFORMED]: 'malformed',
};

export const SAMPLE_MAGIC = 0x314d5745;
export const SAMPLE_HEADER_WORDS = 4;
export const MAX_SAMPLES = 64;
export const SAMPLE_POOL_FLOATS = 1 << 23;
export const STATES_MAGIC = 0x32535745;
export const MAX_STATES = 8;
/** Words per state in a states blob v2 for a patch of `n` values. */
export const stateWords = (n: number): number => n + N_MACROS + 1;

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
export const M_N_VALUES = 32;
/** Live intensity of the mic, 0..1 (NaN while the gate is closed). */
export const M_INTENSITY = 33;
/** Bit per listening layer. */
export const M_LISTEN = 34;
/** Voices the selected layer's patch may sound at once (its cost against the layer budget). */
export const M_MAX_VOICES = 35;
/** The selected layer's sources (21): macros 8, wheel, bend, pressure, layer LFOs 2, features 8
 *  (pitch cents, volume, openness, breath, hardness, onset, voicing, intensity). */
export const M_LAYER_SRC = 40;
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
export const M_METRO = 260;
export const M_METRO_BEAT = 261;
export const M_COUNTIN = 262;
export const M_COUNT_SEQ = 263;
export const M_VOWEL = 264;
export const M_STORED = 280;
export const M_LIVE = M_STORED + MAX_VALUES;
/** Route outputs (0..1, NaN without a value): voice patch k's route i at k * 48 + i, layer routes from 192. */
export const M_ROUTES = M_LIVE + MAX_VALUES;
export const N_ROUTE_METERS = KIT_SLOTS * 48 + 64;
export const METER_LEN = M_ROUTES + N_ROUTE_METERS;

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
