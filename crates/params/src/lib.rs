//! The parameter table and the pure parts of resolution (TDD "Parameter
//! system"). `table.rs` is generated from `params.toml` by `pnpm gen:params`;
//! never edit it by hand.
//!
//! A ParamSet is 96 normalized values: the 64 scalars in table order, then
//! 16 tone bands (index TONE_BANDS..), then 16 noise bands (NOISE_BANDS..).

mod table;
pub use table::*;

/// A full sound, normalized 0..1 per parameter.
pub type ParamSet = [f32; COUNT];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scale {
    Lin,
    Log,
    Db,
    Bipolar,
}

/// Influence aspect groups; the discriminant is the mask bit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Tone = 0,
    Shape = 1,
    Motion = 2,
    Grit = 3,
    Space = 4,
}

#[derive(Clone, Copy, Debug)]
pub struct Desc {
    pub id: &'static str,
    pub group: Group,
    pub scale: Scale,
    pub min: f32,
    pub max: f32,
    /// Real units.
    pub neutral: f32,
    pub unit: &'static str,
    pub smooth_ms: f32,
    pub doc: &'static str,
}

/// Normalized 0..1 to real units (lin, db and bipolar map linearly; log maps
/// to min * (max/min)^n). The input is clamped to 0..1.
pub fn to_real(i: usize, n: f32) -> f32 {
    let _ = (i, n);
    todo!()
}

/// Real units to normalized, clamped to 0..1. Inverse of `to_real`.
pub fn to_norm(i: usize, real: f32) -> f32 {
    let _ = (i, real);
    todo!()
}

/// Every parameter at its neutral value, normalized.
pub fn neutral_set() -> ParamSet {
    todo!()
}

/// Resolution step 2: p = (1 - a m) b + a m t, with m = 1 when the
/// parameter's group bit is set in `mask`.
pub fn influence_blend(base: &ParamSet, target: &ParamSet, amount: f32, mask: u32, out: &mut ParamSet) {
    let _ = (base, target, amount, mask, out);
    todo!()
}

/// Resolution step 3: p = clamp(p + sum_k W[k][i] (2 u_k - 1), 0, 1).
pub fn apply_macros(p: &mut ParamSet, macros: &[f32; N_MACROS]) {
    let _ = (p, macros);
    todo!()
}

/// TDD "What the sliders show": u_live_k = u_k + W_k . (live - stored) / (2 |W_k|^2).
/// Macros with an all-zero row keep u_k.
pub fn project_macros(stored: &ParamSet, live: &ParamSet, macros: &[f32; N_MACROS], out: &mut [f32; N_MACROS]) {
    let _ = (stored, live, macros, out);
    todo!()
}
