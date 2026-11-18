//! Earworm M0 engine: the live capture front end, the tuning function and a
//! one-layer synth behind the C ABI from the TDD ("Engine ABI").
//!
//! The same module runs in the AudioWorklet (`ew_process`, one render quantum
//! with mic input) and offline (`ew_render`, any length, no input). All
//! memory is allocated in `ew_init`; process calls never allocate.

pub mod protocol;
pub mod synth;

use dsp::capture::{FrameSink, LiveTier};
use dsp::frame::MAX_FRAME_LEN;
use dsp::params::{self, ParamSet};
use dsp::tune::{SCALES, Slides, TuneSettings, Tuner};
use protocol::*;
use synth::{LiveDrive, Synth};

pub struct Engine {
    sample_rate: f32,
    n_bands: usize,
    max_block: usize,
    input: Vec<f32>,
    output: Vec<f32>,
    control: Vec<u8>,
    control_count: u32,
    meter: Vec<f32>,
    frames: Vec<f32>,
    frames_count: u32,
    table: BufferTable,
    live_on: bool,
    tier: LiveTier,
    sink: FrameSink,
    tuner: Tuner,
    tuned_cents: f32,
    params: ParamSet,
    synth: Synth,
    blocks: u32,
}

impl Engine {
    pub fn new(sample_rate: f32, max_block: usize, n_bands: usize) -> Box<Engine> {
        let n_bands = if n_bands == 24 { 24 } else { 16 };
        let max_block = max_block.max(1);
        let mut e = Box::new(Engine {
            sample_rate,
            n_bands,
            max_block,
            input: vec![0.0; max_block],
            output: vec![0.0; max_block],
            control: vec![0; CONTROL_CAP * MSG_BYTES],
            control_count: 0,
            meter: vec![0.0; METER_LEN],
            frames: vec![0.0; MAX_FRAMES_PER_CALL * RECORD_STRIDE],
            frames_count: 0,
            table: BufferTable {
                input: 0,
                output: 0,
                max_block: 0,
                control: 0,
                control_cap: 0,
                control_count: 0,
                meter: 0,
                meter_len: 0,
                frames: 0,
                record_stride: 0,
                frames_count: 0,
                n_bands: 0,
            },
            live_on: false,
            tier: LiveTier::new(sample_rate, n_bands),
            // Worst case: a 4096-sample offline chunk at 48 kHz is 8.5 frames.
            sink: FrameSink::with_capacity(MAX_FRAMES_PER_CALL * 2),
            tuner: Tuner::new(0.005),
            tuned_cents: f32::NAN,
            params: ParamSet::default(),
            synth: Synth::new(sample_rate, n_bands, max_block),
            blocks: 0,
        });
        e.table = BufferTable {
            input: e.input.as_ptr() as usize as u32,
            output: e.output.as_ptr() as usize as u32,
            max_block: max_block as u32,
            control: e.control.as_ptr() as usize as u32,
            control_cap: CONTROL_CAP as u32,
            control_count: &e.control_count as *const u32 as usize as u32,
            meter: e.meter.as_ptr() as usize as u32,
            meter_len: METER_LEN as u32,
            frames: e.frames.as_ptr() as usize as u32,
            record_stride: RECORD_STRIDE as u32,
            frames_count: &e.frames_count as *const u32 as usize as u32,
            n_bands: n_bands as u32,
        };
        let d = e.tier.delays();
        e.meter[M_DELAY_LEVEL] = d.level_s;
        e.meter[M_DELAY_PITCH] = d.pitch_s;
        e.meter[M_N_BANDS] = n_bands as f32;
        e.apply_tune_settings();
        e
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn n_bands(&self) -> usize {
        self.n_bands
    }

    /// Native helper (tests, tools): queue one control message.
    pub fn push_message(&mut self, ty: u16, index: u8, param: u16, value: f32) {
        let i = self.control_count as usize;
        if i >= CONTROL_CAP {
            return;
        }
        let m = &mut self.control[i * MSG_BYTES..(i + 1) * MSG_BYTES];
        m[0..2].copy_from_slice(&ty.to_le_bytes());
        m[2] = 0;
        m[3] = index;
        m[4..6].copy_from_slice(&param.to_le_bytes());
        m[6..8].copy_from_slice(&0u16.to_le_bytes());
        m[8..12].copy_from_slice(&value.to_le_bytes());
        m[12..16].copy_from_slice(&0u32.to_le_bytes());
        self.control_count += 1;
    }

    pub fn input_mut(&mut self) -> &mut [f32] {
        &mut self.input
    }

    pub fn output(&self) -> &[f32] {
        &self.output
    }

    pub fn meter(&self) -> &[f32] {
        &self.meter
    }

    /// Frame records written by the last call (RECORD_STRIDE floats each).
    pub fn frame_records(&self) -> &[f32] {
        &self.frames[..self.frames_count as usize * RECORD_STRIDE]
    }

    fn apply_tune_settings(&mut self) {
        let s = &self.params.scalars;
        let scale = (s[params::TUNE_SCALE].round() as usize).min(SCALES.len() - 1);
        self.tuner.set(TuneSettings {
            snap: s[params::TUNE_SNAP],
            speed_ms: s[params::TUNE_SPEED],
            slides: Slides::from_index(s[params::TUNE_SLIDES]),
            vibrato: s[params::TUNE_VIBRATO],
            root: (s[params::TUNE_ROOT].round() as i32).rem_euclid(12) as u8,
            mask: SCALES[scale].1,
            ref_cents: s[params::TUNE_REF_CENTS],
        });
    }

    fn drain_control(&mut self) {
        let n = (self.control_count as usize).min(CONTROL_CAP);
        self.control_count = 0;
        let mut tune_changed = false;
        for i in 0..n {
            let m = &self.control[i * MSG_BYTES..(i + 1) * MSG_BYTES];
            let ty = u16::from_le_bytes([m[0], m[1]]);
            let index = m[3] as usize;
            let param = u16::from_le_bytes([m[4], m[5]]) as usize;
            let value = f32::from_le_bytes([m[8], m[9], m[10], m[11]]);
            match ty {
                MSG_SET_PARAM if param < params::COUNT && value.is_finite() => {
                    let v = params::clamp(param, value);
                    self.params.scalars[param] = v;
                    if (params::TUNE_SNAP..=params::TUNE_REF_CENTS).contains(&param) {
                        tune_changed = true;
                    }
                    self.synth.set_param(param, v);
                }
                MSG_SET_TONE_BAND if index < self.n_bands && value.is_finite() => {
                    self.params.tone_bands[index] = value;
                    self.synth.set_tone_band(index, value);
                }
                MSG_SET_NOISE_BAND if index < self.n_bands && value.is_finite() => {
                    self.params.noise_bands[index] = value;
                    self.synth.set_noise_band(index, value);
                }
                MSG_NOTE_ON if value.is_finite() => self.synth.note_on(value, 1.0),
                MSG_NOTE_OFF => self.synth.note_off(),
                MSG_SET_LIVE => {
                    let on = value > 0.5;
                    if on != self.live_on {
                        self.live_on = on;
                        self.tuner.reset();
                        self.tuned_cents = f32::NAN;
                    }
                }
                MSG_RESET_SINGER => self.tier.reset_singer(),
                MSG_PANIC => self.synth.panic(),
                _ => {}
            }
        }
        if tune_changed {
            self.apply_tune_settings();
        }
    }

    /// Live: one render quantum with mic input in `input[..frames]`.
    pub fn process(&mut self, frames: usize) {
        let frames = frames.min(self.max_block);
        self.drain_control();
        self.sink.clear();
        self.tier.process(&mut self.input[..frames], &mut self.sink);

        let g = self.tier.gesture();
        for &(cents, conf) in &g.pitch_updates[..g.n_pitch_updates] {
            self.tuned_cents = self.tuner.process(cents, conf);
        }
        let octave = self.params.scalars[params::TUNE_OCTAVE].round() * 1200.0;
        let carrier = self.tuned_cents + octave;

        let out = &mut self.output[..frames];
        if self.live_on {
            let drive = LiveDrive { tuned_cents: carrier, gesture: g };
            self.synth.render(out, Some(&drive));
        } else {
            self.synth.render(out, None);
        }
        self.write_frames(carrier);
        self.write_meters(frames, carrier);
    }

    /// Offline: render `frames` samples with no input. Returns samples written.
    pub fn render(&mut self, frames: usize) -> usize {
        let frames = frames.min(self.max_block);
        self.drain_control();
        self.sink.clear();
        let out = &mut self.output[..frames];
        self.synth.render(out, None);
        self.frames_count = 0;
        self.write_meters(frames, f32::NAN);
        frames
    }

    fn write_frames(&mut self, carrier: f32) {
        let n = self.sink.frames().len().min(MAX_FRAMES_PER_CALL);
        for (i, f) in self.sink.frames()[..n].iter().enumerate() {
            let rec = &mut self.frames[i * RECORD_STRIDE..(i + 1) * RECORD_STRIDE];
            rec[..MAX_FRAME_LEN].copy_from_slice(f);
            rec[RECORD_TUNED] = carrier;
        }
        self.frames_count = n as u32;
    }

    fn write_meters(&mut self, frames: usize, carrier: f32) {
        self.blocks = self.blocks.wrapping_add(1);
        let peak = self.output[..frames].iter().fold(0.0f32, |a, &x| a.max(x.abs()));
        let g = self.tier.gesture();
        let m = &mut self.meter;
        m[M_BLOCKS] = self.blocks as f32;
        m[M_INPUT_DB] = g.input_db;
        m[M_FLOOR_DB] = g.floor_db;
        m[M_OUTPUT_DB] = dsp::lin_to_db(peak);
        m[M_GATE] = g.gate as u8 as f32;
        m[M_PITCH] = g.pitch_cents;
        m[M_CONF] = g.pitch_conf;
        m[M_TUNED] = carrier;
        m[M_LOUD] = g.loudness;
        m[M_OPEN] = g.openness;
        m[M_BREATH] = g.breathiness;
        m[M_HARD] = g.hardness;
        m[M_VOICING] = g.voicing;
        m[M_ONSET] = g.onset;
        m[M_LIVE] = self.live_on as u8 as f32;
        let sm = self.synth.meters();
        m[M_F0] = sm.f0_hz;
        m[M_VOICED_MIX] = sm.voiced_mix;
        m[M_ACTIVE_PARTIALS] = sm.active_partials as f32;
        m[M_VOWEL..M_VOWEL + 24].copy_from_slice(&g.bands_db);
        m[M_TONE_EFF..M_TONE_EFF + 24].copy_from_slice(&sm.tone_eff_db);
    }
}

// ---- C ABI (TDD "Engine ABI"). ew_load and ew_seek are not needed in M0. ----

#[unsafe(no_mangle)]
pub extern "C" fn ew_init(sample_rate: f32, max_block: u32, n_bands: u32) -> *mut Engine {
    Box::into_raw(Engine::new(sample_rate, max_block as usize, n_bands as usize))
}

/// # Safety
/// `e` must come from `ew_init`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ew_buffers(e: *mut Engine) -> *const BufferTable {
    unsafe { &(*e).table }
}

/// # Safety
/// `e` must come from `ew_init`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ew_process(e: *mut Engine, frames: u32) {
    unsafe { (*e).process(frames as usize) }
}

/// # Safety
/// `e` must come from `ew_init`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ew_render(e: *mut Engine, frames: u32) -> u32 {
    unsafe { (*e).render(frames as usize) as u32 }
}

/// # Safety
/// `e` must come from `ew_init` and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ew_free(e: *mut Engine) {
    if !e.is_null() {
        drop(unsafe { Box::from_raw(e) });
    }
}
