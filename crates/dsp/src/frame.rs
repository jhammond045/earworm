//! The feature-frame format (TDD "Feature frames"). One record per 10 ms.
//!
//! Slots 0..8 are fixed; slots 8.. hold the vowel shape, one dB value per
//! band. The TDD fixes 16 bands (24 floats per frame). M0 also supports 24
//! bands (32 floats) so the "16 vs 24 bands" question can be settled by ear.

pub const FRAME_RATE: f32 = 100.0;
pub const FRAME_SECONDS: f32 = 1.0 / FRAME_RATE;
pub const MAX_BANDS: usize = 24;
pub const DEFAULT_BANDS: usize = 16;

pub const PITCH: usize = 0; // cents above C0, NaN when unvoiced
pub const CONFIDENCE: usize = 1; // 0..1
pub const LOUDNESS: usize = 2; // 0..1 after floor/ceiling calibration
pub const OPENNESS: usize = 3; // 0..1
pub const BREATHINESS: usize = 4; // 0..1
pub const ONSET: usize = 5; // 0..1
pub const HARDNESS: usize = 6; // 0..1, of the most recent onset
pub const VOICING: usize = 7; // 0 voiced, 1 unvoiced, 2 silent
pub const BANDS: usize = 8; // first vowel-shape band, dB

pub const HEADER: usize = 8;
pub const MAX_FRAME_LEN: usize = HEADER + MAX_BANDS;

pub const VOICED: f32 = 0.0;
pub const UNVOICED: f32 = 1.0;
pub const SILENT: f32 = 2.0;

/// Floats per frame record for a given band count.
pub const fn frame_len(n_bands: usize) -> usize {
    HEADER + n_bands
}

/// One frame record, sized for the largest band count. Only the first
/// `frame_len(n_bands)` slots are meaningful.
pub type Frame = [f32; MAX_FRAME_LEN];

/// A silent frame: pitch NaN, everything else 0, voicing = SILENT.
pub fn silent_frame() -> Frame {
    let mut f = [0.0; MAX_FRAME_LEN];
    f[PITCH] = f32::NAN;
    f[VOICING] = SILENT;
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sixteen_bands_is_24_floats() {
        assert_eq!(frame_len(16), 24);
        assert_eq!(frame_len(24), 32);
        assert_eq!(BANDS + 16, 24);
    }

    #[test]
    fn silent_frame_shape() {
        let f = silent_frame();
        assert!(f[PITCH].is_nan());
        assert_eq!(f[VOICING], SILENT);
        assert!(f[BANDS..].iter().all(|&x| x == 0.0));
    }
}
