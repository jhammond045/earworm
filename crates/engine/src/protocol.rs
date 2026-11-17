//! Wire formats shared with `web/src/host/protocol.ts`. Keep the two in step.

use dsp::frame::MAX_FRAME_LEN;

/// Control message: 16 bytes, little endian.
/// `{ type: u16, layer: u8, index: u8, param: u16, pad: u16, value: f32, time: u32 }`
pub const MSG_BYTES: usize = 16;
pub const CONTROL_CAP: usize = 256;

pub const MSG_SET_PARAM: u16 = 1; // param, value (real units)
pub const MSG_SET_TONE_BAND: u16 = 2; // index, value (dB)
pub const MSG_SET_NOISE_BAND: u16 = 3; // index, value (dB)
pub const MSG_NOTE_ON: u16 = 4; // value = cents above C0
pub const MSG_NOTE_OFF: u16 = 5;
pub const MSG_SET_LIVE: u16 = 6; // value 1 = Live follow on, 0 = off
pub const MSG_RESET_SINGER: u16 = 7;
pub const MSG_PANIC: u16 = 8;

/// Frame ring record: one feature frame (MAX_FRAME_LEN floats, only the first
/// frame_len(n_bands) meaningful) followed by the tuned pitch in cents
/// (display only, not part of the stored frame format).
pub const RECORD_STRIDE: usize = MAX_FRAME_LEN + 1;
pub const RECORD_TUNED: usize = MAX_FRAME_LEN;
/// Worst case frames per process call (a 4096-frame offline chunk at 96 kHz
/// is under 5 frames; live blocks are 128 samples).
pub const MAX_FRAMES_PER_CALL: usize = 8;

// Meter block slots (f32).
pub const M_BLOCKS: usize = 0;
pub const M_INPUT_DB: usize = 1;
pub const M_FLOOR_DB: usize = 2;
pub const M_OUTPUT_DB: usize = 3;
pub const M_GATE: usize = 4;
pub const M_PITCH: usize = 5; // raw live pitch, cents
pub const M_CONF: usize = 6;
pub const M_TUNED: usize = 7; // tuned pitch, cents (carrier, after octave)
pub const M_LOUD: usize = 8;
pub const M_OPEN: usize = 9;
pub const M_BREATH: usize = 10;
pub const M_HARD: usize = 11;
pub const M_VOICING: usize = 12;
pub const M_BLEED: usize = 13;
pub const M_ONSET: usize = 14;
pub const M_LIVE: usize = 15;
pub const M_F0: usize = 16; // carrier f0 in Hz
pub const M_VOICED_MIX: usize = 17; // 1 = partials, 0 = noise
pub const M_ACTIVE_PARTIALS: usize = 18;
pub const M_N_BANDS: usize = 19;
pub const M_VOWEL: usize = 20; // 24 slots: live vowel shape dB
pub const M_TONE_EFF: usize = 44; // 24 slots: effective tone bands dB
pub const M_DELAY_LEVEL: usize = 68; // nominal analysis delay, s
pub const M_DELAY_PITCH: usize = 69;
pub const METER_LEN: usize = 72;

/// Table returned by `ew_buffers`: byte offsets into wasm memory and sizes,
/// all u32 so JS can read it as a Uint32Array.
#[repr(C)]
pub struct BufferTable {
    pub input: u32,       // f32[max_block], mic in; high-passed in place by ew_process
    pub output: u32,      // f32[max_block], mono out
    pub max_block: u32,
    pub control: u32,     // u8[CONTROL_CAP * MSG_BYTES], written by JS before each call
    pub control_cap: u32,
    pub control_count: u32, // address of a u32 the host sets to the number of messages
    pub meter: u32,       // f32[METER_LEN]
    pub meter_len: u32,
    pub frames: u32,      // f32[MAX_FRAMES_PER_CALL * RECORD_STRIDE]
    pub record_stride: u32,
    pub frames_count: u32, // address of a u32 the engine sets to frames written this call
    pub n_bands: u32,
}
