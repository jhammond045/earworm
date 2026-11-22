//! Flex anchors (docs/quant.md "Anchors"): the points in a take that should land on the beat.
//! Found once per take, right after the second pass, from what it already computed plus one
//! extra onset pass on a 4 kHz high-passed copy of the audio (about 1 ms per second of audio).
//!
//! - Beat and texture takes: the second pass's onsets plus the high-band onsets (which catch
//!   quiet bright hits under a louder low one). An onset within 20 ms of a hit carries the
//!   hit's kit slot. All hard.
//! - Melody takes: the second pass's note starts. A start with no onset within 20 ms is a
//!   legato join: it is moved back to where the pitch (pYIN on the audio) is halfway from the
//!   last note to this one, and is soft (it bends time but does not cut the audio).
//!
//! Stored with the take as three floats per anchor (take seconds, weight, flags) next to
//! `VERSION`, so a later detector never changes an old project's timing.

use crate::beats::Hit;
use crate::notes::Note;
use crate::signal::{FPS, Levels, Onset, detect_onsets, flux};

/// The detector's version, stored with the anchors.
pub const VERSION: u32 = 1;
/// Floats per anchor in the flat layout: take seconds, weight, flags.
pub const ANCHOR_LEN: usize = 3;

/// Flag bit 0: a transient. The audio around it is never stretched.
pub const HARD: u32 = 1;
/// Flag bits 1-2: what found it.
pub const KIND_ONSET: u32 = 0;
pub const KIND_NOTE: u32 = 1;
pub const KIND_HIT: u32 = 2;
pub const KIND_PEAK: u32 = 3;
pub const KIND_SHIFT: u32 = 1;
/// Flag bits 3-4: the kit slot of a hit anchor.
pub const SLOT_SHIFT: u32 = 3;

/// A full-band onset this close to a high-band one wins; an onset this close to a note start
/// makes it a hard start; an onset this close to a hit takes the hit's kit slot. Seconds.
const NEAR: f32 = 0.02;
/// The high-band copy is high-passed here, Hz.
const HIGH_BAND_HZ: f32 = 4000.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    /// Take seconds.
    pub t: f32,
    /// How strong the event is; of two anchors on one slot, the heavier takes it.
    pub w: f32,
    pub flags: u32,
}

impl Anchor {
    pub fn hard(&self) -> bool {
        self.flags & HARD != 0
    }
    pub fn kind(&self) -> u32 {
        (self.flags >> KIND_SHIFT) & 3
    }
    pub fn slot(&self) -> u32 {
        (self.flags >> SLOT_SHIFT) & 3
    }
}

pub fn flags(hard: bool, kind: u32, slot: u32) -> u32 {
    (hard as u32) | (kind & 3) << KIND_SHIFT | (slot & 3) << SLOT_SHIFT
}

/// Fourth-order high-pass at `fc` (two Butterworth biquads).
pub fn highpass(x: &[f32], sr: f32, fc: f32) -> Vec<f32> {
    let w0 = std::f32::consts::TAU * fc / sr;
    let (sn, cs) = w0.sin_cos();
    let alpha = sn / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
    let a0 = 1.0 + alpha;
    let (b0, b1, b2) = ((1.0 + cs) / 2.0 / a0, -(1.0 + cs) / a0, (1.0 + cs) / 2.0 / a0);
    let (a1, a2) = (-2.0 * cs / a0, (1.0 - alpha) / a0);
    let mut y = x.to_vec();
    for _ in 0..2 {
        let (mut x1, mut x2, mut y1, mut y2) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for v in y.iter_mut() {
            let x0 = *v;
            let out = b0 * x0 + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
            x2 = x1;
            x1 = x0;
            y2 = y1;
            y1 = out;
            *v = out;
        }
    }
    y
}

/// The second pass's onsets plus the onsets of a high-passed copy that are not within `NEAR`
/// of one of them (at half weight), sorted by time. `n`: frames, as the second pass counts them.
pub fn onsets_with_high_band(audio: &[f32], sr: f32, n: usize, full: &[Onset]) -> Vec<(f32, f32)> {
    let mut all: Vec<(f32, f32)> = full.iter().map(|o| (o.t, o.flux)).collect();
    if sr > 2.0 * HIGH_BAND_HZ {
        let hf = highpass(audio, sr, HIGH_BAND_HZ);
        let lv = Levels::new(&hf, sr, n);
        for o in detect_onsets(&lv, &flux(&hf, sr, n)) {
            if !full.iter().any(|f| (f.t - o.t).abs() < NEAR) {
                all.push((o.t, o.flux * 0.5));
            }
        }
    }
    all.sort_by(|a, b| a.0.total_cmp(&b.0));
    all
}

/// Beat and texture takes: every onset, hard, with the kit slot of a hit within `NEAR`.
pub fn onset_anchors(onsets: &[(f32, f32)], hits: &[Hit]) -> Vec<Anchor> {
    onsets
        .iter()
        .map(|&(t, w)| {
            let hit = hits.iter().filter(|h| (h.t - t).abs() < NEAR).min_by(|a, b| (a.t - t).abs().total_cmp(&(b.t - t).abs()));
            match hit {
                Some(h) => Anchor { t, w, flags: flags(true, KIND_HIT, h.slot as u32) },
                None => Anchor { t, w, flags: flags(true, KIND_ONSET, 0) },
            }
        })
        .collect()
}

/// Melody takes: the note starts, legato joins moved to the middle of the glide and soft.
/// `pitch`: pYIN on the audio, one frame-centred value per 10 ms (cents, NaN unvoiced).
pub fn note_anchors(notes: &[Note], onsets: &[(f32, f32)], pitch: &[f32]) -> Vec<Anchor> {
    let mut out = Vec::with_capacity(notes.len());
    for (i, n) in notes.iter().enumerate() {
        let mut t = n.t;
        let hard = i == 0 || onsets.iter().any(|o| (o.0 - t).abs() < NEAR);
        if !hard {
            let (prev, cur) = (notes[i - 1].cents, n.cents);
            let k0 = ((t - 0.12) * FPS).floor().max(1.0) as usize;
            let k1 = ((t * FPS).ceil() as usize + 2).min(pitch.len());
            let frac = |k: usize| (pitch[k] - prev) / (cur - prev);
            for k in k0..k1 {
                let (f0, f1) = (frac(k - 1), frac(k));
                if f0.is_finite() && f1.is_finite() && f0 < 0.5 && f1 >= 0.5 {
                    t = (k as f32 - 1.0 + (0.5 - f0) / (f1 - f0)) / FPS;
                    break;
                }
            }
        }
        out.push(Anchor { t, w: 0.5 + n.hard, flags: flags(hard, KIND_NOTE, 0) });
    }
    out
}

/// Flat layout: three floats per anchor.
pub fn to_floats(anchors: &[Anchor], out: &mut Vec<f32>) {
    for a in anchors {
        out.extend_from_slice(&[a.t, a.w, a.flags as f32]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_bits() {
        let f = flags(true, KIND_HIT, 3);
        let a = Anchor { t: 0.0, w: 1.0, flags: f };
        assert!(a.hard());
        assert_eq!(a.kind(), KIND_HIT);
        assert_eq!(a.slot(), 3);
        assert_eq!(f, 1 | 2 << 1 | 3 << 3);
        let s = Anchor { t: 0.0, w: 1.0, flags: flags(false, KIND_NOTE, 0) };
        assert!(!s.hard());
        assert_eq!(s.kind(), KIND_NOTE);
    }

    #[test]
    fn high_pass_removes_lows_and_keeps_highs() {
        let sr = 48_000.0;
        let tone = |hz: f32| -> Vec<f32> { (0..9600).map(|i| (std::f32::consts::TAU * hz * i as f32 / sr).sin()).collect() };
        let rms = |x: &[f32]| (x[4800..].iter().map(|v| v * v).sum::<f32>() / 4800.0).sqrt();
        assert!(rms(&highpass(&tone(200.0), sr, 4000.0)) < 1e-3);
        assert!((rms(&highpass(&tone(12_000.0), sr, 4000.0)) - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.02);
    }

    #[test]
    fn a_legato_join_moves_to_the_glide_midpoint_and_is_soft() {
        // 2000 cents until 1.0 s, a glide to 2200 centred at 0.95 s, an onset only at the first note.
        let pitch: Vec<f32> = (0..200)
            .map(|k| {
                let t = k as f32 / FPS;
                2000.0 + 200.0 * ((t - 0.90) / 0.10).clamp(0.0, 1.0)
            })
            .collect();
        let notes = [Note { t: 0.2, dur: 0.78, cents: 2000.0, hard: 0.5 }, Note { t: 0.98, dur: 0.5, cents: 2200.0, hard: 0.1 }];
        let a = note_anchors(&notes, &[(0.2, 5.0)], &pitch);
        assert!(a[0].hard() && a[0].t == 0.2);
        assert!(!a[1].hard());
        assert!((a[1].t - 0.95).abs() < 0.002, "{}", a[1].t);
    }

    #[test]
    fn an_onset_near_a_hit_carries_its_slot() {
        let hits = [Hit { t: 0.5, slot: 2, ..Hit::default() }];
        let a = onset_anchors(&[(0.1, 3.0), (0.505, 4.0)], &hits);
        assert_eq!(a[0].kind(), KIND_ONSET);
        assert_eq!((a[1].kind(), a[1].slot()), (KIND_HIT, 2));
        assert!(a.iter().all(|x| x.hard()));
    }
}
