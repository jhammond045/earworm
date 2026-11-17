//! The live capture front end (TDD "Live tier" and the fourteen Live follow
//! requirements). Runs causally inside the engine's process call, before the
//! voices, and also over files (analysis, parity tests): same code both ways.
//!
//! Per block it high-passes the input in place, updates the latest `Gesture`
//! (what the synth reads this block) and emits one feature frame per 10 ms
//! into a `FrameSink`. Stored frames are shifted so all channels line up with
//! pitch (requirement 11); the live `Gesture` uses each channel as soon as it
//! exists.

use crate::frame::{Frame, MAX_BANDS};

/// Latest live values, refreshed every block.
#[derive(Clone, Debug)]
pub struct Gesture {
    /// Raw live pitch in cents above C0, NaN until the first voiced estimate.
    /// After the continuity guard (requirement 8).
    pub pitch_cents: f32,
    pub pitch_conf: f32,
    /// Pitch estimates produced during this block, oldest first, for the tuner
    /// (one per YIN hop, so 0..=MAX_PITCH_UPDATES per block).
    pub pitch_updates: [(f32, f32); MAX_PITCH_UPDATES],
    pub n_pitch_updates: usize,
    /// 0..1 after floor/ceiling calibration.
    pub loudness: f32,
    pub openness: f32,
    pub breathiness: f32,
    /// > 0 only in the block an onset fires (its strength 0..1).
    pub onset: f32,
    /// Hardness of the most recent onset, 0..1.
    pub hardness: f32,
    /// frame::VOICED / UNVOICED / SILENT.
    pub voicing: f32,
    /// Gate open (requirement 9: adaptive floor, hysteresis, 5 ms att / 60 ms rel).
    pub gate: bool,
    /// Gate envelope 0..1 (smoothed, so the synth can fade without clicks).
    pub gate_env: f32,
    /// Vowel shape in dB: loudness and the singer's long-term average removed
    /// (requirements 3, 4, 5). Only `n_bands` entries are meaningful.
    pub bands_db: [f32; MAX_BANDS],
    /// Speaker bleed detected (requirement 10): the engine should mute.
    pub bleed: bool,
    /// Input level in dBFS (for meters).
    pub input_db: f32,
    /// Current adaptive noise floor in dBFS (for meters).
    pub floor_db: f32,
}

pub const MAX_PITCH_UPDATES: usize = 8;

/// Fixed-capacity output for frames produced in one call. Never reallocates.
pub struct FrameSink {
    frames: Vec<Frame>,
    count: usize,
}

impl FrameSink {
    pub fn with_capacity(n: usize) -> Self {
        Self { frames: vec![crate::frame::silent_frame(); n], count: 0 }
    }
    pub fn clear(&mut self) {
        self.count = 0;
    }
    /// Push a frame; silently drops it if full (callers size for the worst case).
    pub fn push(&mut self, f: &Frame) {
        if self.count < self.frames.len() {
            self.frames[self.count] = *f;
            self.count += 1;
        }
    }
    pub fn frames(&self) -> &[Frame] {
        &self.frames[..self.count]
    }
}

/// Measured analysis delays of the live tier, for take alignment and for the
/// delay report. Seconds.
#[derive(Clone, Copy, Debug)]
pub struct Delays {
    pub level_s: f32,
    pub pitch_s: f32,
}

pub struct LiveTier {
    n_bands: usize,
    gesture: Gesture,
    // implementation-defined state: high-pass, BandBank, Decimator, Yin,
    // floor/ceiling trackers, singer average, openness range, onset detector,
    // gate, bleed detector, frame accumulator and alignment history.
}

impl LiveTier {
    pub fn new(sample_rate: f32, n_bands: usize) -> Self {
        todo!()
    }
    pub fn n_bands(&self) -> usize {
        self.n_bands
    }
    /// Process one block of mic input. `input` is high-passed in place (the
    /// caller then copies it to the mic ring). `engine_out` is the engine's
    /// previous output block (same length or empty), used only by the bleed
    /// detector. Frames completed during the block are pushed to `sink`
    /// (the caller clears it).
    pub fn process(&mut self, input: &mut [f32], engine_out: &[f32], sink: &mut FrameSink) {
        todo!()
    }
    pub fn gesture(&self) -> &Gesture {
        &self.gesture
    }
    /// Forget the learned singer average, openness range and noise floor.
    pub fn reset_singer(&mut self) {
        todo!()
    }
    /// Nominal analysis delays (constants derived from filter/window sizes).
    pub fn delays(&self) -> Delays {
        todo!()
    }
}

/// Convenience for files: run the live tier over a whole signal in 128-sample
/// blocks and collect every frame. Allocates; not for the audio thread.
pub fn frames_from_signal(signal: &[f32], sample_rate: f32, n_bands: usize) -> Vec<Frame> {
    let mut tier = LiveTier::new(sample_rate, n_bands);
    let mut sink = FrameSink::with_capacity(8);
    let mut out = Vec::new();
    let mut buf = [0.0f32; 128];
    for chunk in signal.chunks(128) {
        let b = &mut buf[..chunk.len()];
        b.copy_from_slice(chunk);
        sink.clear();
        tier.process(b, &[], &mut sink);
        out.extend_from_slice(sink.frames());
    }
    out
}
