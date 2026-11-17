//! The tuning function (TDD "Tuning"). Causal, frame by frame: the same code
//! runs during Live follow (per YIN hop) and over stored takes (per frame).
//!
//! Steps: split slow pitch (3 Hz one-pole) from vibrato; pick the nearest scale
//! degree with 60% hysteresis; pull by snap then lag by speed; slide handling
//! (glide / gliss / jump); add vibrato back scaled; hold through gaps.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slides {
    Glide,
    Gliss,
    Jump,
}

impl Slides {
    pub fn from_index(i: f32) -> Self {
        match i.round() as i32 {
            1 => Slides::Gliss,
            2 => Slides::Jump,
            _ => Slides::Glide,
        }
    }
}

/// Scales as 12-bit pitch-class masks relative to the root (bit 0 = root).
/// Index order is the `tune.scale` parameter.
pub const SCALES: [(&str, u16); 6] = [
    ("chromatic", 0b1111_1111_1111),
    ("major", 0b1010_1011_0101),
    ("minor", 0b0101_1010_1101),
    ("major pentatonic", 0b0010_1001_0101),
    ("minor pentatonic", 0b0100_1010_1001),
    ("blues", 0b0100_1110_1001),
];

#[derive(Clone, Copy, Debug)]
pub struct TuneSettings {
    /// Pitch Snap, 0..1.
    pub snap: f32,
    /// Tune speed: lag time constant in ms, 0..250. 0 = instant.
    pub speed_ms: f32,
    pub slides: Slides,
    /// 0 flatten, 1 keep, 2 exaggerate.
    pub vibrato: f32,
    /// Pitch class of the root, 0 = C.
    pub root: u8,
    /// Pitch-class mask relative to root (see SCALES).
    pub mask: u16,
    /// Singer's offset from concert pitch in cents; scale degrees shift by this.
    pub ref_cents: f32,
}

impl Default for TuneSettings {
    fn default() -> Self {
        Self { snap: 0.0, speed_ms: 60.0, slides: Slides::Glide, vibrato: 1.0, root: 0, mask: SCALES[0].1, ref_cents: 0.0 }
    }
}

/// Frames with confidence below this, or NaN pitch, are gaps.
pub const MIN_CONFIDENCE: f32 = 0.5;
/// Slow-pitch split cutoff.
pub const SPLIT_HZ: f32 = 3.0;
/// Hysteresis: target moves once pitch is this share of the way to the next degree.
pub const HYSTERESIS: f32 = 0.6;
/// A slide: slow-pitch rate above this (cents/s) ...
pub const SLIDE_RATE: f32 = 400.0;
/// ... for longer than this (s).
pub const SLIDE_MIN_S: f32 = 0.040;
/// Jump: switch once within this many cents of a new degree ...
pub const JUMP_WINDOW_CENTS: f32 = 50.0;
/// ... for this long (s).
pub const JUMP_HOLD_S: f32 = 0.060;

pub struct Tuner {
    dt: f32,
    settings: TuneSettings,
    // implementation-defined state
}

impl Tuner {
    /// `dt`: seconds between calls (0.005 live per YIN hop, 0.01 for stored frames).
    pub fn new(dt: f32) -> Self {
        todo!()
    }
    pub fn set(&mut self, s: TuneSettings) {
        self.settings = s;
    }
    pub fn settings(&self) -> &TuneSettings {
        &self.settings
    }
    /// One step. `pitch_cents` is cents above C0 or NaN. Returns tuned cents,
    /// or NaN if nothing voiced has been seen since the last reset.
    pub fn process(&mut self, pitch_cents: f32, confidence: f32) -> f32 {
        todo!()
    }
    pub fn reset(&mut self) {
        todo!()
    }
}

/// Run the tuner over a stored pitch curve (one value per 10 ms frame).
/// Writes `out[i]` for each input. Used by tests and by the live/stored parity check.
pub fn tune_curve(settings: TuneSettings, pitch: &[f32], conf: &[f32], out: &mut [f32]) {
    let mut t = Tuner::new(crate::frame::FRAME_SECONDS);
    t.set(settings);
    for i in 0..pitch.len() {
        out[i] = t.process(pitch[i], conf[i]);
    }
}
