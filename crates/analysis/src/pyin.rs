//! pYIN-style second-pass pitch tracker (TDD "Second pass", Pitch repair):
//! several YIN candidates per frame, a Viterbi path over them, then repair of
//! the live curve where the two disagree by > 80 cents for >= 30 ms.

/// Pitch curve at 100 frames per second (cents above C0, NaN unvoiced),
/// frame i centred on time i * 0.01 s to match the live tier's stored frames.
pub fn pyin(signal: &[f32], sample_rate: f32) -> Vec<f32> {
    todo!()
}

/// Replace `live[i]` with `second[i]` where they differ by more than
/// `REPAIR_CENTS` for at least `REPAIR_MIN_FRAMES` consecutive frames.
/// Returns the repaired curve and the number of frames replaced.
pub fn repair(live: &[f32], second: &[f32]) -> (Vec<f32>, usize) {
    todo!()
}

pub const REPAIR_CENTS: f32 = 80.0;
pub const REPAIR_MIN_FRAMES: usize = 3;
