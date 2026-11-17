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

fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

/// Normalized 0..1 to real units (lin, db and bipolar map linearly; log maps
/// to min * (max/min)^n). The input is clamped to 0..1; NaN is treated as the
/// parameter's neutral value. The result is always within min..max.
pub fn to_real(i: usize, n: f32) -> f32 {
    let d = &DESCS[i];
    if n.is_nan() {
        return d.neutral;
    }
    let n = clamp01(n);
    let r = match d.scale {
        Scale::Log => d.min * (d.max / d.min).powf(n),
        _ => d.min + (d.max - d.min) * n,
    };
    r.clamp(d.min, d.max)
}

/// Real units to normalized, clamped to 0..1. Inverse of `to_real`. NaN is
/// treated as the parameter's neutral value.
pub fn to_norm(i: usize, real: f32) -> f32 {
    let d = &DESCS[i];
    let real = if real.is_nan() { d.neutral } else { real };
    let n = match d.scale {
        Scale::Log => {
            if real <= 0.0 {
                0.0
            } else {
                (real / d.min).ln() / (d.max / d.min).ln()
            }
        }
        _ => (real - d.min) / (d.max - d.min),
    };
    if n.is_nan() { 0.0 } else { clamp01(n) }
}

/// Every parameter at its neutral value, normalized.
pub fn neutral_set() -> ParamSet {
    let mut s = [0.0; COUNT];
    for (i, v) in s.iter_mut().enumerate() {
        *v = to_norm(i, DESCS[i].neutral);
    }
    s
}

/// Non-finite values count as 0.
fn finite_or_zero(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

/// Resolution step 2: p = (1 - a m) b + a m t, with m = 1 when the
/// parameter's group bit is set in `mask`. `amount` is clamped to 0..1 (NaN
/// counts as 0); non-finite base or target values count as 0; the output is
/// clamped to 0..1.
pub fn influence_blend(base: &ParamSet, target: &ParamSet, amount: f32, mask: u32, out: &mut ParamSet) {
    let a = if amount.is_nan() { 0.0 } else { clamp01(amount) };
    for i in 0..COUNT {
        let b = finite_or_zero(base[i]);
        let on = mask & (1u32 << (DESCS[i].group as u32)) != 0;
        out[i] = if on {
            clamp01((1.0 - a) * b + a * finite_or_zero(target[i]))
        } else {
            clamp01(b)
        };
    }
}

/// Resolution step 3: p = clamp(p + sum_k W[k][i] (2 u_k - 1), 0, 1).
/// Macro values are clamped to 0..1 (NaN counts as 0.5, no offset); a
/// non-finite p counts as 0.
pub fn apply_macros(p: &mut ParamSet, macros: &[f32; N_MACROS]) {
    let mut d = [0.0f32; N_MACROS];
    for k in 0..N_MACROS {
        let u = if macros[k].is_nan() { 0.5 } else { clamp01(macros[k]) };
        d[k] = 2.0 * u - 1.0;
    }
    for i in 0..COUNT {
        let mut v = finite_or_zero(p[i]);
        for k in 0..N_MACROS {
            v += MACRO_W[k][i] * d[k];
        }
        p[i] = clamp01(v);
    }
}

/// TDD "What the sliders show": u_live_k = u_k + W_k . (live - stored) / (2 |W_k|^2).
/// Macros with an all-zero row keep u_k. Output is clamped to 0..1; non-finite
/// inputs count as 0 (u as 0.5).
pub fn project_macros(stored: &ParamSet, live: &ParamSet, macros: &[f32; N_MACROS], out: &mut [f32; N_MACROS]) {
    for k in 0..N_MACROS {
        let u = if macros[k].is_nan() { 0.5 } else { clamp01(macros[k]) };
        let mut dot = 0.0f32;
        let mut n2 = 0.0f32;
        for i in 0..COUNT {
            let w = MACRO_W[k][i];
            dot += w * (finite_or_zero(live[i]) - finite_or_zero(stored[i]));
            n2 += w * w;
        }
        out[k] = if n2 > 0.0 { clamp01(u + dot / (2.0 * n2)) } else { u };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn unit(&mut self) -> f32 {
            (self.next() >> 40) as f32 / (1u64 << 24) as f32
        }
    }

    fn random_set(r: &mut Rng) -> ParamSet {
        let mut s = [0.0; COUNT];
        for v in s.iter_mut() {
            *v = r.unit();
        }
        s
    }

    #[test]
    fn round_trip() {
        for i in 0..COUNT {
            for &n in &[0.0, 0.01, 0.25, 0.5, 0.77, 0.99, 1.0] {
                let back = to_norm(i, to_real(i, n));
                assert!((back - n).abs() < 1e-4, "{} n={n} back={back}", DESCS[i].id);
            }
        }
    }

    #[test]
    fn to_real_stays_in_range_for_odd_inputs() {
        for i in 0..COUNT {
            let d = &DESCS[i];
            for &n in &[f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -5.0, 5.0] {
                let r = to_real(i, n);
                assert!(r.is_finite() && r >= d.min && r <= d.max, "{}", d.id);
            }
            for &r in &[f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1e30, 1e30, 0.0] {
                let n = to_norm(i, r);
                assert!((0.0..=1.0).contains(&n), "{}", d.id);
            }
        }
    }

    #[test]
    fn neutral_set_matches_table() {
        let s = neutral_set();
        for i in 0..COUNT {
            assert!((0.0..=1.0).contains(&s[i]));
            let d = &DESCS[i];
            let tol = 1e-4 * (d.max - d.min).abs().max(1.0);
            assert!((to_real(i, s[i]) - d.neutral).abs() <= tol, "{}", d.id);
        }
    }

    #[test]
    fn named_constants_point_at_their_ids() {
        assert_eq!(DESCS[SRC_TILT].id, "src.tilt");
        assert_eq!(DESCS[WIDTH].id, "width");
        assert_eq!(DESCS[TONE_BANDS + 3].id, "tone_bands.3");
        assert_eq!(DESCS[NOISE_BANDS + 15].id, "noise_bands.15");
    }

    #[test]
    fn blend_endpoints_and_mask() {
        let mut r = Rng(0x9e3779b97f4a7c15);
        let b = random_set(&mut r);
        let t = random_set(&mut r);
        let mut out = [0.0; COUNT];
        influence_blend(&b, &t, 0.0, 0b11111, &mut out);
        assert_eq!(out, b);
        influence_blend(&b, &t, 1.0, 0b11111, &mut out);
        for i in 0..COUNT {
            assert!((out[i] - t[i]).abs() < 1e-6);
        }
        // only the Grit group
        let mask = 1u32 << (Group::Grit as u32);
        influence_blend(&b, &t, 1.0, mask, &mut out);
        let mut grit = 0;
        for i in 0..COUNT {
            if DESCS[i].group == Group::Grit {
                grit += 1;
                assert!((out[i] - t[i]).abs() < 1e-6);
            } else {
                assert_eq!(out[i], b[i]);
            }
        }
        assert!(grit > 0);
        influence_blend(&b, &t, 1.0, 0, &mut out);
        assert_eq!(out, b);
        influence_blend(&b, &t, 0.5, 0b11111, &mut out);
        assert!((out[3] - 0.5 * (b[3] + t[3])).abs() < 1e-6);
    }

    #[test]
    fn macros_at_half_are_identity() {
        let mut r = Rng(12345);
        for _ in 0..50 {
            let p = random_set(&mut r);
            let mut q = p;
            apply_macros(&mut q, &[0.5; N_MACROS]);
            assert_eq!(p, q);
        }
    }

    #[test]
    fn macros_stay_in_range() {
        let mut r = Rng(0xdeadbeef12345);
        for round in 0..2000 {
            let mut p = random_set(&mut r);
            let mut m = [0.0; N_MACROS];
            for v in m.iter_mut() {
                *v = match round % 4 {
                    0 => r.unit(),
                    1 => if r.next() & 1 == 0 { 0.0 } else { 1.0 },
                    2 => r.unit() * 20.0 - 10.0,
                    _ => [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.3][(r.next() % 4) as usize],
                };
            }
            apply_macros(&mut p, &m);
            assert!(p.iter().all(|v| (0.0..=1.0).contains(v)));
        }
    }

    #[test]
    fn project_returns_u_when_unchanged() {
        let mut r = Rng(777);
        let p = random_set(&mut r);
        let m = [0.1, 0.9, 0.5, 0.3, 0.7, 0.0, 1.0, 0.45];
        let mut out = [0.0; N_MACROS];
        project_macros(&p, &p, &m, &mut out);
        assert_eq!(out, m);
    }

    #[test]
    fn project_recovers_a_macro_move() {
        // Moving macro k by du shifts live by W_k * 2du (before clamping), so
        // projecting that back returns u + du when the rows are independent
        // enough that cross terms are small; check the own-row term only.
        let stored = [0.5; COUNT];
        let mut live = stored;
        for i in 0..COUNT {
            live[i] += MACRO_W[0][i] * 0.2;
        }
        let mut out = [0.0; N_MACROS];
        project_macros(&stored, &live, &[0.5; N_MACROS], &mut out);
        assert!(out[0] > 0.5);
    }

    #[test]
    fn table_is_consistent() {
        for (a, da) in DESCS.iter().enumerate() {
            for db in DESCS.iter().skip(a + 1) {
                assert_ne!(da.id, db.id);
            }
            assert!(da.min < da.max && da.min <= da.neutral && da.neutral <= da.max);
        }
        assert_eq!(MACRO_W.len(), N_MACROS);
        assert_eq!(MACRO_NAMES.len(), N_MACROS);
        assert!(MACRO_W.iter().all(|r| r.len() == COUNT));
        assert!(MACRO_W.iter().all(|r| r.iter().any(|w| *w != 0.0)));
        assert_eq!(TONE_BANDS, SCALARS);
        assert_eq!(NOISE_BANDS, SCALARS + N_BANDS);
        assert_eq!(COUNT, SCALARS + 2 * N_BANDS);
    }
}
