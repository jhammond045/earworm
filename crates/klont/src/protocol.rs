//! Wire formats of the M1 engine. The TypeScript mirror is
//! `web/src/m1/protocol.ts`; keep the two in step (a Vitest test checks the
//! constants against this file).
//!
//! Everything is little endian. Parameter values on the wire are normalized
//! 0..1, indexed by the ParamSet layout in `params` (64 scalars, then 16 tone
//! bands, then 16 noise bands).

// ---------------------------------------------------------------- limits

pub const MAX_LAYERS: usize = 8;
pub const VOICES_PER_LAYER: usize = 8;
pub const KIT_SLOTS: usize = 4;
pub const N_MACROS: usize = 8;
pub const MAX_CURVES: usize = 64;
/// Floats in the curve pool shared by all curves: 2^21 floats is 8 MB, about
/// 14 minutes of 25-float frames at 100 per second.
pub const CURVE_POOL_FLOATS: usize = 1 << 21;
/// Staging area the host copies a blob into before `ew_load`: 4 MB.
pub const STAGING_BYTES: usize = 4 << 20;

// ---------------------------------------------------------------- control ring

/// Control message: 16 bytes.
/// `{ type: u16, layer: u8, index: u8, param: u16, pad: u16, value: f32, time: u32 }`
/// `time` is the sample offset inside the next process or render call at
/// which the message takes effect (clamped to the call length). Messages are
/// applied in ring order; the host writes them in time order.
pub const MSG_BYTES: usize = 16;
pub const CONTROL_CAP: usize = 256;

/// layer, index = target set (SET_*), param = ParamSet index, value 0..1.
pub const MSG_SET_PARAM: u16 = 1;
/// layer, index = macro 0..7, value 0..1 (0.5 is neutral).
pub const MSG_SET_MACRO: u16 = 2;
/// layer, param = LAYER_* field, value in that field's units.
pub const MSG_SET_LAYER: u16 = 3;
/// Live trigger. layer, index = kit slot (beat layers), value = cents above
/// C0 (NaN: the kit slot's own pitch), param = velocity in thousandths
/// (0 means full).
pub const MSG_NOTE_ON: u16 = 4;
/// Release live voices of this layer started with the same cents value.
pub const MSG_NOTE_OFF: u16 = 5;
pub const MSG_PLAY: u16 = 6;
/// Stop the transport and release every sequenced voice.
pub const MSG_STOP: u16 = 7;
/// index = section. Taken at the end of the current pass of the section.
pub const MSG_JUMP: u16 = 8;
/// value = active-partial budget for all layers. 0 = automatic: start at
/// BUDGET_MAX and adapt to the host-reported CPU load (live only; offline
/// renders use BUDGET_MAX). Whether the track loops is a flag in the track blob.
pub const MSG_SET_BUDGET: u16 = 9;
/// layer whose resolved values the meter block shows.
pub const MSG_SELECT_LAYER: u16 = 10;
/// Silence every voice and clear every effect tail at once.
pub const MSG_PANIC: u16 = 11;
/// Live follow (M2). layer, value 1 = arm the layer (the mic drives it), 0 =
/// disarm. Arming one layer disarms any other. No automatic mute exists: the
/// armed layer sounds whenever the gate is open.
pub const MSG_ARM: u16 = 12;
/// layer, param = TUNE_* field, value in that field's units. The live tuner
/// uses the armed layer's settings.
pub const MSG_SET_TUNE: u16 = 13;
/// Taught kit for live slot choice on beat layers. layer, index = kit slot,
/// param = LIVE_KIT_* field, value.
pub const MSG_SET_LIVE_KIT: u16 = 14;
/// Forget the learned singer average, openness range and noise floor.
pub const MSG_RESET_SINGER: u16 = 15;

/// MSG_SET_PARAM targets (the `index` byte).
pub const SET_BASE: u8 = 0;
/// Kit slot k is SET_KIT + k.
pub const SET_KIT: u8 = 1;
pub const SET_INFLUENCE: u8 = 5;

/// MSG_SET_LAYER fields (the `param` field).
pub const LAYER_LEVEL: u16 = 0; // dB, -60..12
pub const LAYER_PAN: u16 = 1; // -1..1
pub const LAYER_MUTE: u16 = 2; // 0 or 1
pub const LAYER_KIND: u16 = 3; // KIND_MELODY, KIND_BEAT, KIND_TEXTURE
pub const LAYER_DEPTH_LOUD: u16 = 4; // gesture depths 0..1, TDD GestureDepth
pub const LAYER_DEPTH_OPEN: u16 = 5;
pub const LAYER_DEPTH_HARD: u16 = 6;
pub const LAYER_DEPTH_BREATH: u16 = 7;
pub const LAYER_DEPTH_MOUTH: u16 = 8;
pub const LAYER_FOLLOW: u16 = 9; // hit-shape follow 0..1 (beat layers)
pub const LAYER_INFLUENCE_AMOUNT: u16 = 10; // 0..1
pub const LAYER_INFLUENCE_MASK: u16 = 11; // bit per aspect group, GROUP_BIT_*
pub const LAYER_KIT_CENTS: u16 = 12; // 12..=15: default pitch of kit slot 0..3, cents above C0
pub const LAYER_FIELDS: usize = 16;

/// MSG_SET_TUNE fields (TDD Tune). Defaults: snap 0, speed 60 ms, glide,
/// vibrato 1, root 0, chromatic, ref 0, octave 0.
pub const TUNE_SNAP: u16 = 0; // 0..1
pub const TUNE_SPEED: u16 = 1; // ms, 0..250
pub const TUNE_SLIDES: u16 = 2; // 0 glide, 1 gliss, 2 jump
pub const TUNE_VIBRATO: u16 = 3; // 0 flatten, 1 keep, 2 exaggerate
pub const TUNE_ROOT: u16 = 4; // pitch class 0..11, 0 = C
pub const TUNE_SCALE: u16 = 5; // index into dsp::tune::SCALES
pub const TUNE_REF_CENTS: u16 = 6; // singer offset from concert pitch, -50..50
pub const TUNE_OCTAVE: u16 = 7; // whole octaves added to the live carrier, -2..2
pub const TUNE_FIELDS: usize = 8;

/// MSG_SET_LIVE_KIT fields. A slot with TAUGHT = 1 has a centre in the three
/// live hit features (dsp::frame::live_hit_features); a live onset goes to
/// the nearest taught centre. With no taught slot, slots 0..2 are chosen by
/// brightness thirds.
pub const LIVE_KIT_TAUGHT: u16 = 0; // 0 or 1
pub const LIVE_KIT_BRIGHT: u16 = 1;
pub const LIVE_KIT_LOW: u16 = 2;
pub const LIVE_KIT_NOISE: u16 = 3;
pub const LIVE_KIT_FIELDS: usize = 4;

pub const KIND_MELODY: u32 = 0;
pub const KIND_BEAT: u32 = 1;
pub const KIND_TEXTURE: u32 = 2;

/// Influence aspect groups, in params.toml `group` order.
pub const GROUP_TONE: u32 = 0;
pub const GROUP_SHAPE: u32 = 1;
pub const GROUP_MOTION: u32 = 2;
pub const GROUP_GRIT: u32 = 3;
pub const GROUP_SPACE: u32 = 4;

/// Highest budget the automatic mode uses, and the floor it never goes below.
pub const BUDGET_MAX: u32 = 4096;
pub const BUDGET_MIN: u32 = 256;

// ---------------------------------------------------------------- ew_load

/// `ew_load(e, slot, kind, ptr, len)`: the host copies a blob into the
/// staging area and passes its address. Returns LOAD_OK or an error code.
/// Track: replaces the arrangement; slot is ignored. Layout in `seq.rs`.
pub const LOAD_TRACK: u32 = 1;
/// Layer setup; slot = layer. Layout below.
pub const LOAD_LAYER: u32 = 2;
/// Curve table; slot = curve id 0..MAX_CURVES. Layout in `curve.rs`.
pub const LOAD_CURVE: u32 = 3;

pub const LOAD_OK: i32 = 0;
pub const LOAD_BAD_KIND: i32 = -1;
pub const LOAD_BAD_SLOT: i32 = -2;
pub const LOAD_BAD_MAGIC: i32 = -3;
pub const LOAD_TOO_BIG: i32 = -4;
pub const LOAD_MALFORMED: i32 = -5;

/// Layer blob: 32-bit words.
/// ```text
/// word 0          u32 magic LAYER_MAGIC
/// words 1..17     f32 fields, LAYER_* order (level, pan, mute, kind, depths, follow,
///                 influence amount, influence mask, kit cents x4)
/// words 17..113   f32 base ParamSet (96, normalized)
/// words 113..121  f32 macros (8, 0..1, 0.5 neutral)
/// words 121..217  f32 influence target ParamSet (96)
/// words 217..601  f32 kit slots, 4 x 96
/// ```
pub const LAYER_MAGIC: u32 = 0x314C_5745; // "EWL1"
pub const LAYER_WORDS: usize = 1 + LAYER_FIELDS + 96 + N_MACROS + 96 + KIT_SLOTS * 96;
pub const LAYER_OFF_FIELDS: usize = 1;
pub const LAYER_OFF_BASE: usize = LAYER_OFF_FIELDS + LAYER_FIELDS;
pub const LAYER_OFF_MACROS: usize = LAYER_OFF_BASE + 96;
pub const LAYER_OFF_INFLUENCE: usize = LAYER_OFF_MACROS + N_MACROS;
pub const LAYER_OFF_KIT: usize = LAYER_OFF_INFLUENCE + 96;

// ---------------------------------------------------------------- meter block

/// Meter block slots (f32), written once per process or render call.
pub const M_BLOCKS: usize = 0;
pub const M_PLAYING: usize = 1;
pub const M_SECTION: usize = 2;
pub const M_SECTION_POS: usize = 3; // ticks (grid) or seconds (free)
pub const M_REPEAT: usize = 4;
pub const M_BPM: usize = 5;
pub const M_LOAD: usize = 6; // host-reported CPU load of the last block, 0..1+
pub const M_BUDGET: usize = 7;
pub const M_ACTIVE_PARTIALS: usize = 8;
pub const M_ACTIVE_VOICES: usize = 9;
pub const M_STEALS: usize = 10; // voices stolen since init
pub const M_OVERLOADS: usize = 11; // blocks with load above 0.8 since init
pub const M_LOAD_STATUS: usize = 12; // result of the last ew_load
pub const M_PEAK_L: usize = 13;
pub const M_PEAK_R: usize = 14;
pub const M_SELECTED: usize = 15;
pub const M_LAYER_PEAK: usize = 16; // 8 slots, per-layer peak (max of L and R)
pub const M_LAYER_VOICES: usize = 24; // 8 slots, sounding voices per layer
/// Selected layer after resolution step 3 (the stored sound), 96 normalized values.
pub const M_STORED: usize = 32;
/// Selected layer after step 5 (with the voice applied: its loudest sounding
/// voice, or the stored values when none sounds), 96 normalized values.
pub const M_LIVE: usize = 128;
/// Macro values with the gesture projected back onto each macro (TDD "What
/// the sliders show"), 8 values.
pub const M_MACRO_LIVE: usize = 224;
/// Live tier (M2): latest values, refreshed every process call.
pub const M_IN_DB: usize = 240; // input level, dBFS
pub const M_FLOOR_DB: usize = 241; // adaptive noise floor, dBFS
pub const M_GATE: usize = 242; // 0 or 1
pub const M_PITCH: usize = 243; // raw live pitch, cents above C0 (NaN unknown)
pub const M_CONF: usize = 244;
pub const M_TUNED: usize = 245; // tuned carrier pitch, cents (octave applied)
pub const M_LOUD: usize = 246;
pub const M_OPEN: usize = 247;
pub const M_BREATH: usize = 248;
pub const M_HARD: usize = 249;
pub const M_VOICING: usize = 250; // 0 voiced, 1 unvoiced, 2 silent
pub const M_ONSET: usize = 251; // onset strength in this call, else 0
pub const M_ARMED: usize = 252; // armed layer, -1 none
pub const M_DELAY_LEVEL: usize = 253; // nominal analysis delay, s
pub const M_DELAY_PITCH: usize = 254;
pub const M_LIVE_SLOT: usize = 255; // kit slot of the last live hit, -1 none
pub const M_LIVE_HITS: usize = 256; // live hits since init
pub const M_VOWEL: usize = 264; // 16 slots: live vowel shape, dB
pub const METER_LEN: usize = 280;

// ---------------------------------------------------------------- buffer table

/// Table returned by `ew_buffers`: byte offsets into wasm memory and sizes,
/// all u32 so JS can read it as a Uint32Array.
#[repr(C)]
pub struct BufferTable {
    pub input: u32, // f32[max_block], mono mic in; ew_process high-passes it in place
    pub output_l: u32, // f32[max_block]
    pub output_r: u32, // f32[max_block]
    pub max_block: u32,
    pub control: u32, // u8[CONTROL_CAP * MSG_BYTES], written by the host before each call
    pub control_cap: u32,
    pub control_count: u32, // address of a u32: messages in `control` for the next call
    pub meter: u32, // f32[METER_LEN]
    pub meter_len: u32,
    pub staging: u32, // u8[STAGING_BYTES] for ew_load
    pub staging_cap: u32,
    pub host_load: u32, // address of an f32 the live host sets to the last block's CPU load
    pub frames: u32, // f32[frames_cap * CURVE_STRIDE]: feature frames made by the last ew_process
    pub frames_cap: u32,
    pub frames_count: u32, // address of a u32: frames written by the last ew_process
}
pub const BT_FIELDS: usize = 15;
