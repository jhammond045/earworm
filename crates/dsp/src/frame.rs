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

/// Live hit features of a frame with `n_bands` bands, used to choose a kit
/// slot for a live hit (TDD "Live slot choice") and, from the same frames,
/// to teach a kit. Three values, each about 0..1:
/// - brightness: power-weighted centroid of the vowel bands over band index
/// - low: how far the lowest quarter of the bands sits above the mean, dB / 24 + 0.5
/// - noise: breathiness
///
/// Non-finite band values count as 0 dB.
pub fn live_hit_features(f: &[f32], n_bands: usize) -> [f32; 3] {
    let n = n_bands.clamp(1, MAX_BANDS).min(f.len().saturating_sub(BANDS));
    if n == 0 {
        return [0.5, 0.5, 0.0];
    }
    let band = |i: usize| {
        let v = f[BANDS + i];
        if v.is_finite() { v.clamp(-60.0, 60.0) } else { 0.0 }
    };
    let (mut wsum, mut isum, mut mean) = (0.0f32, 0.0f32, 0.0f32);
    for i in 0..n {
        let w = 10f32.powf(band(i) / 10.0);
        wsum += w;
        isum += w * i as f32;
        mean += band(i);
    }
    mean /= n as f32;
    let bright = if n > 1 && wsum > 0.0 { isum / wsum / (n - 1) as f32 } else { 0.5 };
    let q = (n / 4).max(1);
    let low_mean = (0..q).map(band).sum::<f32>() / q as f32;
    let low = ((low_mean - mean) / 24.0 + 0.5).clamp(0.0, 1.0);
    let b = f.get(BREATHINESS).copied().unwrap_or(0.0);
    let noise = if b.is_finite() { b.clamp(0.0, 1.0) } else { 0.0 };
    [bright.clamp(0.0, 1.0), low, noise]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_hit_features_order_bright_and_dark() {
        let mut dark = silent_frame();
        let mut bright = silent_frame();
        for i in 0..16 {
            dark[BANDS + i] = 12.0 - 2.0 * i as f32;
            bright[BANDS + i] = -12.0 + 2.0 * i as f32;
        }
        let d = live_hit_features(&dark, 16);
        let b = live_hit_features(&bright, 16);
        assert!(d[0] < 0.3 && b[0] > 0.7, "{d:?} {b:?}");
        assert!(d[1] > 0.5 && b[1] < 0.5);
        let flat = live_hit_features(&silent_frame(), 16);
        assert!((flat[0] - 0.5).abs() < 1e-5 && (flat[1] - 0.5).abs() < 1e-5);
    }

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
