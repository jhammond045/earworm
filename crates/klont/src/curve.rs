//! Curve tables: a take's feature frames plus its tuned pitch, read at play
//! time through the clip's warp map (TDD "Reading curves at play time").
//!
//! # Curve blob (`ew_load` kind LOAD_CURVE, slot = curve id)
//!
//! ```text
//! u32 magic CURVE_MAGIC, u32 n_frames,
//! n_frames x CURVE_STRIDE f32: the 24-float feature frame (dsp::frame layout
//! with 16 bands) followed by the tuned pitch in cents (NaN when unvoiced).
//! ```
//! Frame i is at take time i / 100 s.

use crate::protocol::{CURVE_POOL_FLOATS, MAX_CURVES};
use crate::seq::WarpAnchor;

pub const CURVE_MAGIC: u32 = 0x3143_5745; // "EWC1"
/// Floats per curve record: 24 frame floats plus tuned pitch.
pub const CURVE_STRIDE: usize = 25;
/// Index of the tuned pitch in a record.
pub const TUNED: usize = 24;
pub const FRAME_RATE: f64 = 100.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CurveError {
    BadSlot,
    BadMagic,
    TooBig,
    Malformed,
}

#[derive(Clone, Copy, Default)]
struct Slot {
    /// Offset into the pool, in floats.
    off: usize,
    /// Frames in use.
    frames: usize,
    /// Floats reserved at `off` (>= frames * CURVE_STRIDE).
    cap: usize,
}

/// All curves live in one pool allocated in `new` (CURVE_POOL_FLOATS).
/// Loading into a slot reuses that slot's space when the new curve fits,
/// otherwise appends at the end of the used part; when the pool is full it
/// compacts the live curves to the front first (memmove, no allocation) and
/// fails with TooBig only if they still do not fit. A failed load changes
/// nothing. A slot that is the last in the pool grows in place.
pub struct CurvePool {
    data: Vec<f32>,
    slots: [Slot; MAX_CURVES],
    /// End of the used part of the pool, in floats.
    used: usize,
    /// How many frames ahead of the asked take time `read` reads: the lag of stored frames
    /// behind their audio, so features line up with the take's audio.
    lead: f64,
    /// The note curves of poly takes, read with the take curves (docs/poly.md, P3).
    pub notes: crate::notecurve::NoteCurves,
}

impl Default for CurvePool {
    fn default() -> Self {
        Self::new()
    }
}

impl CurvePool {
    pub fn new() -> Self {
        Self { data: vec![0.0; CURVE_POOL_FLOATS], slots: [Slot::default(); MAX_CURVES], used: 0, lead: 0.0, notes: crate::notecurve::NoteCurves::new() }
    }

    /// Read every curve `frames` frames ahead of the asked time (see `read`).
    pub fn set_lead(&mut self, frames: usize) {
        self.lead = frames as f64;
    }

    /// How far ahead `read` reads, in seconds.
    pub fn lead_s(&self) -> f64 {
        self.lead / FRAME_RATE
    }

    /// Move every live curve (except `skip`) to the front, in pool order.
    fn compact(&mut self, skip: usize) {
        self.slots[skip] = Slot::default();
        let mut order = [0u8; MAX_CURVES];
        let mut n = 0;
        for s in 0..MAX_CURVES {
            if self.slots[s].frames > 0 {
                // Insertion sort by offset.
                let mut k = n;
                while k > 0 && self.slots[order[k - 1] as usize].off > self.slots[s].off {
                    order[k] = order[k - 1];
                    k -= 1;
                }
                order[k] = s as u8;
                n += 1;
            }
        }
        let mut at = 0;
        for &s in &order[..n] {
            let slot = &mut self.slots[s as usize];
            let len = slot.frames * CURVE_STRIDE;
            if slot.off != at {
                self.data.copy_within(slot.off..slot.off + len, at);
                slot.off = at;
            }
            slot.cap = len;
            at += len;
        }
        self.used = at;
    }

    pub fn load(&mut self, slot: usize, bytes: &[u8]) -> Result<(), CurveError> {
        if slot >= MAX_CURVES {
            return Err(CurveError::BadSlot);
        }
        if bytes.len() < 8 {
            return Err(CurveError::Malformed);
        }
        let magic = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        if magic != CURVE_MAGIC {
            return Err(CurveError::BadMagic);
        }
        let frames = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
        let need = frames.checked_mul(CURVE_STRIDE).ok_or(CurveError::TooBig)?;
        if need > CURVE_POOL_FLOATS {
            return Err(CurveError::TooBig);
        }
        if bytes.len() != 8 + need * 4 {
            return Err(CurveError::Malformed);
        }

        let old = self.slots[slot];
        let off = if need <= old.cap {
            old.off
        } else if old.cap > 0 && old.off + old.cap == self.used && old.off + need <= CURVE_POOL_FLOATS {
            // Last in the pool: grow in place.
            self.used = old.off + need;
            self.slots[slot].cap = need;
            old.off
        } else if self.used + need <= CURVE_POOL_FLOATS {
            let off = self.used;
            self.used += need;
            off
        } else {
            let live: usize = (0..MAX_CURVES).filter(|&s| s != slot).map(|s| self.slots[s].frames * CURVE_STRIDE).sum();
            if live + need > CURVE_POOL_FLOATS {
                return Err(CurveError::TooBig);
            }
            self.compact(slot);
            let off = self.used;
            self.used += need;
            off
        };
        for (d, c) in self.data[off..off + need].iter_mut().zip(bytes[8..].chunks_exact(4)) {
            *d = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
        }
        let cap = self.slots[slot].cap.max(need);
        let cap = if off == old.off { cap } else { need };
        self.slots[slot] = Slot { off, frames, cap };
        Ok(())
    }

    /// Frames in a slot (0 when empty or out of range).
    pub fn len(&self, slot: usize) -> usize {
        self.slots.get(slot).map_or(0, |s| s.frames)
    }

    pub fn is_empty(&self, slot: usize) -> bool {
        self.len(slot) == 0
    }

    fn frame(&self, s: &Slot, i: usize) -> &[f32] {
        let a = s.off + i * CURVE_STRIDE;
        &self.data[a..a + CURVE_STRIDE]
    }

    /// The record at take time `sec` (plus the lead), by linear interpolation between the two
    /// nearest frames. Band levels and loudness interpolate linearly (bands
    /// are already dB). Pitch and tuned pitch interpolate in cents when both
    /// sides are finite, else take the nearer side. Voicing and onset take
    /// the nearer frame. Before the first frame or after the last, hold the
    /// end frame. Returns false (and leaves `out` alone) for an empty slot.
    ///
    /// Which record floats take the nearer frame: onset strength (5),
    /// hardness of the last onset (6) and voicing (7). Confidence (1),
    /// loudness (2), openness (3), breathiness (4) and the 16 bands (8..24)
    /// interpolate linearly. A tie (exactly halfway) takes the later frame.
    pub fn read(&self, slot: usize, sec: f64, out: &mut [f32; CURVE_STRIDE]) -> bool {
        let Some(s) = self.slots.get(slot) else { return false };
        if s.frames == 0 {
            return false;
        }
        let x = sec * FRAME_RATE + self.lead;
        let last = s.frames - 1;
        if !(x > 0.0) {
            out.copy_from_slice(self.frame(s, 0));
            return true;
        }
        if x >= last as f64 {
            out.copy_from_slice(self.frame(s, last));
            return true;
        }
        let i = x.floor() as usize;
        let f = (x - i as f64) as f32;
        let (a, b) = (self.frame(s, i), self.frame(s, i + 1));
        let near = if f < 0.5 { a } else { b };
        for k in 0..CURVE_STRIDE {
            out[k] = match k {
                5..=7 => near[k],
                0 | TUNED => {
                    if a[k].is_finite() && b[k].is_finite() {
                        a[k] + (b[k] - a[k]) * f
                    } else if f < 0.5 {
                        a[k]
                    } else {
                        b[k]
                    }
                }
                _ => a[k] + (b[k] - a[k]) * f,
            };
        }
        true
    }
}

/// Map clip time to take seconds through warp anchors (sorted by clip_t),
/// piecewise linear. Outside the anchors, extend the nearest segment's
/// slope. With a single anchor, assume slope `default_slope` (take seconds
/// per clip unit) through it; with none, return `clip_t * default_slope`.
/// A segment of zero width (two anchors at the same clip_t) has no slope of
/// its own and uses `default_slope`.
pub fn warp_to_take(anchors: &[WarpAnchor], clip_t: f64, default_slope: f64) -> f64 {
    match anchors.len() {
        0 => clip_t * default_slope,
        1 => anchors[0].take_sec + (clip_t - anchors[0].clip_t) * default_slope,
        n => {
            // Last anchor at or before clip_t, kept inside 0..n-2 so that
            // (i, i + 1) is a segment.
            let i = anchors.partition_point(|a| a.clip_t <= clip_t).saturating_sub(1).min(n - 2);
            let (a, b) = (anchors[i], anchors[i + 1]);
            let w = b.clip_t - a.clip_t;
            let slope = if w > 0.0 { (b.take_sec - a.take_sec) / w } else { default_slope };
            a.take_sec + (clip_t - a.clip_t) * slope
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frame i: every float is `i + k / 100` except pitch, tuned and flags.
    fn blob(frames: usize, tag: f32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(CURVE_MAGIC.to_le_bytes());
        b.extend((frames as u32).to_le_bytes());
        for i in 0..frames {
            for k in 0..CURVE_STRIDE {
                let v = match k {
                    0 | TUNED => 1000.0 * i as f32 + tag,
                    _ => i as f32 + k as f32 / 100.0 + tag,
                };
                b.extend(v.to_le_bytes());
            }
        }
        b
    }
    fn rd(p: &CurvePool, slot: usize, sec: f64) -> [f32; CURVE_STRIDE] {
        let mut o = [-1.0; CURVE_STRIDE];
        assert!(p.read(slot, sec, &mut o));
        o
    }

    #[test]
    fn load_read_and_errors() {
        let mut p = CurvePool::new();
        assert!(!p.read(0, 0.0, &mut [0.0; CURVE_STRIDE]));
        assert_eq!(p.load(MAX_CURVES, &blob(2, 0.0)), Err(CurveError::BadSlot));
        assert_eq!(p.load(0, &[0; 4]), Err(CurveError::Malformed));
        let mut bad = blob(2, 0.0);
        bad[0] = 9;
        assert_eq!(p.load(0, &bad), Err(CurveError::BadMagic));
        let mut short = blob(2, 0.0);
        short.pop();
        assert_eq!(p.load(0, &short), Err(CurveError::Malformed));
        let mut huge = blob(0, 0.0);
        huge[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(p.load(0, &huge), Err(CurveError::TooBig));
        assert_eq!(p.len(0), 0);
        p.load(3, &blob(4, 0.0)).unwrap();
        assert_eq!(p.len(3), 4);
        assert_eq!(p.len(99), 0);
    }

    #[test]
    fn read_interpolates_and_holds() {
        let mut p = CurvePool::new();
        p.load(0, &blob(4, 0.0)).unwrap();
        // Before and after: end frames.
        assert_eq!(rd(&p, 0, -1.0)[2], 0.02);
        assert_eq!(rd(&p, 0, f64::NAN)[2], 0.02);
        assert_eq!(rd(&p, 0, 5.0)[2], 3.02);
        assert_eq!(rd(&p, 0, 0.03)[2], 3.02);
        // Linear between frames 1 and 2 at a quarter.
        let o = rd(&p, 0, 0.0125);
        assert!((o[2] - 1.27).abs() < 1e-5, "{}", o[2]);
        assert!((o[0] - 1250.0).abs() < 1e-3);
        assert!((o[TUNED] - 1250.0).abs() < 1e-3);
        assert!((o[10] - 1.35).abs() < 1e-5);
        // Nearer-frame fields: 5, 6, 7.
        assert!((o[5] - 1.05).abs() < 1e-6);
        assert!((o[7] - 1.07).abs() < 1e-6);
        let o = rd(&p, 0, 0.0175);
        assert!((o[5] - 2.05).abs() < 1e-6);
    }

    #[test]
    fn unvoiced_pitch_takes_nearer_side() {
        let mut b = blob(2, 0.0);
        // Frame 1 pitch = NaN, frame 0 tuned = NaN.
        let off = 8 + CURVE_STRIDE * 4;
        b[off..off + 4].copy_from_slice(&f32::NAN.to_bits().to_le_bytes());
        b[8 + TUNED * 4..8 + TUNED * 4 + 4].copy_from_slice(&f32::NAN.to_bits().to_le_bytes());
        let mut p = CurvePool::new();
        p.load(0, &b).unwrap();
        assert_eq!(rd(&p, 0, 0.002)[0], 0.0);
        assert!(rd(&p, 0, 0.008)[0].is_nan());
        assert!(rd(&p, 0, 0.002)[TUNED].is_nan());
        assert_eq!(rd(&p, 0, 0.008)[TUNED], 1000.0);
    }

    #[test]
    fn slot_reuse_append_and_grow() {
        let mut p = CurvePool::new();
        p.load(0, &blob(10, 0.0)).unwrap();
        p.load(1, &blob(10, 100.0)).unwrap();
        // Smaller reload reuses slot 0's space.
        let used = p.used;
        p.load(0, &blob(5, 200.0)).unwrap();
        assert_eq!((p.used, p.slots[0].off, p.len(0)), (used, 0, 5));
        assert_eq!(rd(&p, 0, 0.0)[2], 200.02);
        // Bigger than its space: appends (slot 1 is last, grows in place).
        p.load(1, &blob(20, 300.0)).unwrap();
        assert_eq!(p.slots[1].off, 10 * CURVE_STRIDE);
        assert_eq!(p.used, 30 * CURVE_STRIDE);
        // Slot 0 growing past its space appends at the end; old space is dead.
        p.load(0, &blob(12, 400.0)).unwrap();
        assert_eq!(p.slots[0].off, 30 * CURVE_STRIDE);
        assert_eq!(rd(&p, 1, 0.19)[2], 19.02 + 300.0);
        assert_eq!(rd(&p, 0, 0.11)[2], 11.02 + 400.0);
    }

    #[test]
    fn compaction_and_too_big() {
        let mut p = CurvePool::new();
        let n = CURVE_POOL_FLOATS / CURVE_STRIDE; // frames that fill the pool
        let third = n / 3;
        p.load(0, &blob(third, 0.0)).unwrap();
        p.load(1, &blob(third, 1000.0)).unwrap();
        p.load(2, &blob(third, 2000.0)).unwrap();
        // Slot 1 grows: no room at the end, slot 2 is last. Dead space after
        // slot 1 moves would not fit without compaction: shrink slot 0 first.
        p.load(0, &blob(10, 3000.0)).unwrap();
        p.load(1, &blob(third + 50, 4000.0)).unwrap(); // does not fit the tail, forces compaction
        assert_eq!(rd(&p, 0, 0.05)[2], 5.02 + 3000.0);
        assert_eq!(rd(&p, 2, 0.05)[2], 5.02 + 2000.0);
        assert_eq!(rd(&p, 1, 0.05)[2], 5.02 + 4000.0);
        assert_eq!(p.len(1), third + 50);
        assert!(p.used <= CURVE_POOL_FLOATS);
        // Everything still reads; now ask for more than the rest can hold.
        let r = p.load(3, &blob(n - 2 * third, 0.0));
        assert_eq!(r, Err(CurveError::TooBig));
        assert_eq!(p.len(3), 0);
        assert_eq!(rd(&p, 2, (third - 1) as f64 / 100.0)[2], (third - 1) as f32 + 0.02 + 2000.0);
        // A single curve bigger than the pool.
        let r = p.load(4, &blob(n + 1, 0.0));
        assert_eq!(r, Err(CurveError::TooBig));
    }

    #[test]
    fn warp() {
        let a = |t: f64, c: f64| WarpAnchor { take_sec: t, clip_t: c };
        assert_eq!(warp_to_take(&[], 480.0, 0.001), 0.48);
        assert_eq!(warp_to_take(&[a(1.0, 960.0)], 1440.0, 0.002), 1.0 + 480.0 * 0.002);
        assert_eq!(warp_to_take(&[a(1.0, 960.0)], 0.0, 0.002), 1.0 - 960.0 * 0.002);
        let v = [a(0.0, 0.0), a(1.0, 1000.0), a(1.5, 2000.0)];
        assert_eq!(warp_to_take(&v, 500.0, 9.0), 0.5);
        assert_eq!(warp_to_take(&v, 1000.0, 9.0), 1.0);
        assert_eq!(warp_to_take(&v, 1500.0, 9.0), 1.25);
        // Extends the first and last segment's slopes.
        assert_eq!(warp_to_take(&v, -1000.0, 9.0), -1.0);
        assert_eq!(warp_to_take(&v, 3000.0, 9.0), 2.0);
        // Zero-width segment uses the default slope.
        let z = [a(0.0, 0.0), a(1.0, 0.0)];
        assert_eq!(warp_to_take(&z, 10.0, 0.5), 5.0);
    }
}
