//! Wire formats of the M1 engine. The TypeScript mirror is
//! `web/src/m1/protocol.ts`; keep the two in step (a Vitest test checks the
//! constants against this file).
//!
//! Everything is little endian. Values on the wire are normalized 0..1,
//! indexed by the layer's value vector, whose layout the layer's patch sets
//! (`LOAD_PATCH`, patch.rs).

// ---------------------------------------------------------------- limits

pub const MAX_LAYERS: usize = 8;
pub const VOICES_PER_LAYER: usize = 8;
pub const KIT_SLOTS: usize = 4;
pub const N_MACROS: usize = 8;
/// Values in one layer's value vector (patch.rs).
pub const MAX_VALUES: usize = 1024;
pub const MAX_CURVES: usize = 64;
/// Floats in the curve pool shared by all curves: 2^21 floats is 8 MB, about
/// 14 minutes of 25-float frames at 100 per second.
pub const CURVE_POOL_FLOATS: usize = 1 << 21;
/// Staging area the host copies a blob into before `ew_load`: 16 MB, so a 59 s take at 48 kHz
/// (its audio, the raw input on playback, S5) fits in one blob.
pub const STAGING_BYTES: usize = 16 << 20;

// ---------------------------------------------------------------- control ring

/// Control message: 16 bytes.
/// `{ type: u16, layer: u8, index: u8, param: u16, pad: u16, value: f32, time: u32 }`
/// `time` is the sample offset inside the next process or render call at
/// which the message takes effect (clamped to the call length). Messages are
/// applied in ring order; the host writes them in time order.
pub const MSG_BYTES: usize = 16;
pub const CONTROL_CAP: usize = 256;

/// layer, index = SET_BASE or SET_INFLUENCE, param = index into the layer's
/// value vector, value 0..1.
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
/// Metronome. param = METRO_*, value in that field's units. Clicks land on the
/// sample of a grid beat and are mixed into live output only: `ew_render` and
/// the mic buffer never contain them.
pub const MSG_SET_METRO: u16 = 16;
/// One-bar count-in at `value` bpm (0 uses the transport bpm, or 100). Ignored
/// while the transport is already playing. When the bar ends, the transport
/// starts on the next sample.
pub const MSG_COUNT_IN: u16 = 17;
/// Performance sources (docs/synth.md "Live use"). layer (255: every layer),
/// index 0 mod wheel (0..1), 1 pitch bend (-1..1), 2 channel pressure (0..1),
/// value. Routes read them as sources; nothing else does.
pub const MSG_SET_PERF: u16 = 18;
/// layer, value 1 = listen (the layer's sung features come from the mic,
/// while its notes come from elsewhere), 0 = stop. Any number of layers may
/// listen; the live tier runs once whatever the number.
pub const MSG_LISTEN: u16 = 19;
/// Scene (docs/synth.md "Live use"): layer, index = state, value = ms. The
/// layer morphs from where it is now to that state over `value` ms. A clip
/// with morph markers wins while the transport plays; the scene holds when it
/// stops. Needs states (LOAD_STATES).
pub const MSG_MORPH_TO: u16 = 20;
/// Recording (S5, docs/synth.md "DAW use"): value 1 while a pass records, 0 after. While it
/// records, a value, macro, level, pan or performance source moved by a message (a hand: the
/// screen or a controller) leaves its automation lane until the transport stops or recording
/// ends, so the move is heard and recorded over the lane. Otherwise a lane stays in charge.
pub const MSG_RECORD: u16 = 21;
/// MSG_SET_PERF layer meaning every layer.
pub const ALL_LAYERS: u8 = 255;

/// MSG_SET_METRO fields (the `param` field).
pub const METRO_ON: u16 = 0; // 0 or 1
pub const METRO_LEVEL: u16 = 1; // 0..1
pub const METRO_VOICE: u16 = 2; // 0 plain tick, 1 a layer's kit slot
pub const METRO_LAYER: u16 = 3; // layer index, used when METRO_VOICE is kit
pub const METRO_SLOT: u16 = 4; // kit slot 0..3
pub const METRO_BAR: u16 = 5; // beats per bar, 1..32; the downbeat is accented

/// MSG_SET_PARAM targets (the `index` byte): the stored values, or the Influence target.
pub const SET_BASE: u8 = 0;
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
/// A sample of the pool; slot = sample id 0..MAX_SAMPLES. Layout below.
pub const LOAD_SAMPLE: u32 = 4;
/// Morph states; slot = layer. Layout below.
pub const LOAD_STATES: u32 = 5;
/// The layer's patch; slot = layer. Layout in `patch.rs`. Send it before the
/// layer and states blobs: they must hold one value per value of the patch.
/// A patch equal to the loaded one changes nothing; any other silences the
/// layer's voices and returns its values to the patch's starting values.
pub const LOAD_PATCH: u32 = 6;

pub const LOAD_OK: i32 = 0;
pub const LOAD_BAD_KIND: i32 = -1;
pub const LOAD_BAD_SLOT: i32 = -2;
pub const LOAD_BAD_MAGIC: i32 = -3;
pub const LOAD_TOO_BIG: i32 = -4;
pub const LOAD_MALFORMED: i32 = -5;

/// Layer blob v2: 32-bit words, `n` = the patch's value count.
/// ```text
/// word 0          u32 magic LAYER_MAGIC
/// words 1..17     f32 fields, LAYER_* order (level, pan, mute, kind, depths, follow,
///                 influence amount, influence mask, kit cents x4)
/// word 17         u32 n
/// then            f32 values (n, normalized)
///                 f32 macros (8, 0..1, 0.5 neutral)
///                 f32 Influence target (n, normalized)
///                 u32 group bits (n): 1 << the value's aspect group when Influence moves it, else 0
/// ```
pub const LAYER_MAGIC: u32 = 0x324C_5745; // "EWL2"
pub const LAYER_OFF_FIELDS: usize = 1;
pub const LAYER_OFF_N: usize = LAYER_OFF_FIELDS + LAYER_FIELDS;
pub const LAYER_HEADER_WORDS: usize = LAYER_OFF_N + 1;
pub const fn layer_words(n: usize) -> usize {
    LAYER_HEADER_WORDS + 3 * n + N_MACROS
}

/// Sample blob (`LOAD_SAMPLE`): 32-bit words.
/// ```text
/// u32 magic SAMPLE_MAGIC, f32 sample_rate (of the data), f32 ref_cents (cents
/// above C0 of the sample's pitch; NaN = unpitched), u32 n_samples (0 clears
/// the sample), then n f32 samples
/// ```
/// All samples share one pool of SAMPLE_POOL_FLOATS floats (32 MB), allocated
/// at init so loading never allocates (samples.rs).
pub const SAMPLE_MAGIC: u32 = 0x314D_5745; // "EWM1"
pub const SAMPLE_HEADER_WORDS: usize = 4;
pub const MAX_SAMPLES: usize = 64;
pub const SAMPLE_POOL_FLOATS: usize = 1 << 23;

/// States blob v2 (`LOAD_STATES`): 32-bit words.
/// ```text
/// u32 magic STATES_MAGIC, u32 n_states (0..=MAX_STATES; 0 clears the states),
/// u32 n (the patch's value count), then n_states x (n + N_MACROS + 1) f32:
/// n values (normalized), 8 macros (0..1), 1 Influence amount (0..1)
/// ```
pub const STATES_MAGIC: u32 = 0x3253_5745; // "EWS2"
pub const MAX_STATES: usize = 8;
pub const fn state_words(n: usize) -> usize {
    n + N_MACROS + 1
}

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
/// Value count of the selected layer's patch: the length of M_STORED and M_LIVE.
pub const M_N_VALUES: usize = 32;
/// Live intensity of the mic (docs/synth.md "The sung features"), 0..1, NaN when unknown.
pub const M_INTENSITY: usize = 33;
/// Bit per layer: listening (MSG_LISTEN).
pub const M_LISTEN: usize = 34;
/// Voices the selected layer's patch may sound at once (its cost against the layer budget).
pub const M_MAX_VOICES: usize = 35;
/// The selected layer's sources this tick, patch.rs LS_* order (21 slots): macros (8),
/// wheel, bend, pressure, layer LFOs (2), sung features (pitch in cents, volume, openness,
/// breath, hardness, onset, voicing, intensity; NaN when the layer has none).
pub const M_LAYER_SRC: usize = 40;
/// Macro values with the gesture projected back onto each macro through its
/// layer routes (TDD "What the sliders show"), 8 values.
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
/// Engine clock, low part: samples processed since init, modulo 2^20. The clock
/// is a u64 counter advanced by the frames of every `ew_process` call. `ew_render`
/// advances it too (offline renders count their samples), so a host that mixes
/// both must not compare marks across a render. Value = HI * 2^20 + LO, both
/// exact in f32. The count is taken at the end of the call, so it names the next
/// sample to be processed, and `M_SECTION_POS` is the transport position there.
pub const M_CLOCK_LO: usize = 257;
/// Engine clock, high part: the sample count divided by 2^20, floored.
pub const M_CLOCK_HI: usize = 258;
/// Transport units (ticks or seconds) per sample at the end of the call; 0 when stopped.
pub const M_UNITS_PER_SAMPLE: usize = 259;
/// Metronome enabled, 0 or 1. The click itself is not a meter; it is in the live output.
pub const M_METRO: usize = 260;
/// Beat within the bar (0 = downbeat), or -1 when stopped and not counting in.
pub const M_METRO_BEAT: usize = 261;
/// Count-in beats still to play, 0 when idle.
pub const M_COUNTIN: usize = 262;
/// How many count-ins have finished and started the transport.
pub const M_COUNT_SEQ: usize = 263;
pub const M_VOWEL: usize = 264; // 16 slots: live vowel shape, dB
/// Selected layer after resolution step 4, MAX_VALUES normalized values.
pub const M_STORED: usize = 280;
/// Selected layer after step 6 (with its loudest sounding voice's routes
/// applied, or the stored values when none sounds), MAX_VALUES values.
pub const M_LIVE: usize = M_STORED + MAX_VALUES;
/// Selected layer's route outputs (the shaped source, 0..1; NaN when a route has no value):
/// voice patch k's route i at M_ROUTES + k * 48 + i from its loudest sounding voice, then the
/// layer routes from M_ROUTES + 192.
pub const M_ROUTES: usize = M_LIVE + MAX_VALUES;
pub const N_ROUTE_METERS: usize = KIT_SLOTS * 48 + 64;
pub const METER_LEN: usize = M_ROUTES + N_ROUTE_METERS;

// ---------------------------------------------------------------- buffer table

/// Table returned by `ew_buffers`: byte offsets into wasm memory and sizes,
/// all u32 so JS can read it as a Uint32Array.
#[repr(C)]
pub struct BufferTable {
    pub input: u32,    // f32[max_block], mono mic in; ew_process high-passes it in place
    pub output_l: u32, // f32[max_block]
    pub output_r: u32, // f32[max_block]
    pub max_block: u32,
    pub control: u32, // u8[CONTROL_CAP * MSG_BYTES], written by the host before each call
    pub control_cap: u32,
    pub control_count: u32, // address of a u32: messages in `control` for the next call
    pub meter: u32,         // f32[METER_LEN]
    pub meter_len: u32,
    pub staging: u32, // u8[STAGING_BYTES] for ew_load
    pub staging_cap: u32,
    pub host_load: u32, // address of an f32 the live host sets to the last block's CPU load
    pub frames: u32, // f32[frames_cap * CURVE_STRIDE]: feature frames made by the last ew_process
    pub frames_cap: u32,
    pub frames_count: u32, // address of a u32: frames written by the last ew_process
}
pub const BT_FIELDS: usize = 15;
