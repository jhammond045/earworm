//! One-layer Klont for M0: partial bank, shaped noise, vowel-shape path,
//! envelopes and the voice-gesture mapping (resolution step 5). One voice.

use dsp::capture::Gesture;
use dsp::frame::MAX_BANDS;

/// What Live follow feeds the synth each block.
pub struct LiveDrive<'a> {
    /// Tuned carrier pitch in cents above C0 (octave transpose applied), NaN
    /// until a pitch is known.
    pub tuned_cents: f32,
    pub gesture: &'a Gesture,
}

/// Values published to the meter block.
#[derive(Clone, Debug, Default)]
pub struct SynthMeters {
    pub f0_hz: f32,
    pub voiced_mix: f32,
    pub active_partials: u32,
    pub tone_eff_db: [f32; MAX_BANDS],
}

pub struct Synth {
    // implementation-defined
}

impl Synth {
    pub fn new(sample_rate: f32, n_bands: usize, max_block: usize) -> Self {
        todo!()
    }
    /// Real units (see dsp::params). Tuning params are ignored here.
    pub fn set_param(&mut self, id: usize, value: f32) {
        todo!()
    }
    pub fn set_tone_band(&mut self, i: usize, db: f32) {
        todo!()
    }
    pub fn set_noise_band(&mut self, i: usize, db: f32) {
        todo!()
    }
    /// Note mode (Influence audition, offline render). Ignored while live.
    pub fn note_on(&mut self, cents: f32, velocity: f32) {
        todo!()
    }
    pub fn note_off(&mut self) {
        todo!()
    }
    /// Silence everything immediately.
    pub fn panic(&mut self) {
        todo!()
    }
    /// Render `out.len()` samples (<= max_block), overwriting `out`.
    /// `drive` is Some while Live follow is on.
    pub fn render(&mut self, out: &mut [f32], drive: Option<&LiveDrive>) {
        todo!()
    }
    pub fn meters(&self) -> &SynthMeters {
        todo!()
    }
}
