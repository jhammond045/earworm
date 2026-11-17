//! Influence instant match (TDD "Sound Influence" / "Instant match"), M0
//! subset: only the parameters the M0 engine has (sources and envelopes).

use dsp::frame::MAX_BANDS;
use dsp::params::{COUNT, ParamSet};

/// Floats in the flat result: params, tone bands, noise bands, then
/// pitch_cents, duration_s, voiced_fraction, pitched (0/1).
/// Mirrored as MATCH_* in web/src/host/protocol.ts.
pub const MATCH_LEN: usize = COUNT + 2 * MAX_BANDS + 4;

#[derive(Clone, Debug)]
pub struct MatchResult {
    /// Target parameters in real units. Gesture depths and tuning keep defaults.
    pub params: ParamSet,
    /// Pitch to audition the match at (median voiced pitch, cents above C0);
    /// for unpitched sources a neutral 4800 (C4).
    pub pitch_cents: f32,
    /// How long to hold the audition note (source sound length up to its offset).
    pub duration_s: f32,
    pub voiced_fraction: f32,
    pub pitched: bool,
}

impl MatchResult {
    pub fn to_floats(&self, out: &mut [f32]) {
        out[..COUNT].copy_from_slice(&self.params.scalars);
        out[COUNT..COUNT + MAX_BANDS].copy_from_slice(&self.params.tone_bands);
        out[COUNT + MAX_BANDS..COUNT + 2 * MAX_BANDS].copy_from_slice(&self.params.noise_bands);
        let b = COUNT + 2 * MAX_BANDS;
        out[b] = self.pitch_cents;
        out[b + 1] = self.duration_s;
        out[b + 2] = self.voiced_fraction;
        out[b + 3] = self.pitched as u8 as f32;
    }
}

/// Measure `signal` and return the target parameter set.
pub fn instant_match(signal: &[f32], sample_rate: f32, n_bands: usize) -> MatchResult {
    todo!()
}
