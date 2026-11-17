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

/// All curves live in one pool allocated in `new` (CURVE_POOL_FLOATS).
/// Loading into a slot reuses that slot's space when the new curve fits,
/// otherwise appends at the end of the used part; when the pool is full it
/// compacts the live curves to the front first (memmove, no allocation) and
/// fails with TooBig only if they still do not fit.
pub struct CurvePool {}

impl CurvePool {
    pub fn new() -> Self {
        let _ = (CURVE_POOL_FLOATS, MAX_CURVES);
        todo!()
    }
    pub fn load(&mut self, slot: usize, bytes: &[u8]) -> Result<(), CurveError> {
        let _ = (slot, bytes);
        todo!()
    }
    /// Frames in a slot (0 when empty or out of range).
    pub fn len(&self, slot: usize) -> usize {
        let _ = slot;
        todo!()
    }
    /// The record at take time `sec`, by linear interpolation between the two
    /// nearest frames. Band levels and loudness interpolate linearly (bands
    /// are already dB). Pitch and tuned pitch interpolate in cents when both
    /// sides are finite, else take the nearer side. Voicing and onset take
    /// the nearer frame. Before the first frame or after the last, hold the
    /// end frame. Returns false (and leaves `out` alone) for an empty slot.
    pub fn read(&self, slot: usize, sec: f64, out: &mut [f32; CURVE_STRIDE]) -> bool {
        let _ = (slot, sec, out);
        todo!()
    }
}

/// Map clip time to take seconds through warp anchors (sorted by clip_t),
/// piecewise linear. Outside the anchors, extend the nearest segment's
/// slope. With a single anchor, assume slope `default_slope` (take seconds
/// per clip unit) through it; with none, return `clip_t * default_slope`.
pub fn warp_to_take(anchors: &[WarpAnchor], clip_t: f64, default_slope: f64) -> f64 {
    let _ = (anchors, clip_t, default_slope);
    todo!()
}
