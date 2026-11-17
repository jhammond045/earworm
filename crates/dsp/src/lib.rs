//! Shared DSP for the Earworm M0 prototypes.
//!
//! M0 is throwaway code. This crate holds everything the engine and the
//! analysis code share: primitives (filters, band bank, YIN, FFT, noise,
//! decimator), the feature-frame format, the parameter list, the tuning
//! function and the live capture front end. See docs/TDD.md.
//!
//! Plain `std` is used (it compiles to wasm32-unknown-unknown with no
//! imports as long as nothing touches I/O). All buffers are allocated in
//! constructors; `process`-style methods must not allocate.

pub mod bands;
pub mod capture;
pub mod fft;
pub mod filters;
pub mod frame;
pub mod noise;
pub mod params;
pub mod resample;
pub mod tune;
pub mod yin;

/// Cents above C0 for a frequency in Hz. C0 = 16.351_597_8 Hz.
#[inline]
pub fn hz_to_cents(hz: f32) -> f32 {
    1200.0 * (hz / C0_HZ).log2()
}

/// Frequency in Hz for cents above C0.
#[inline]
pub fn cents_to_hz(cents: f32) -> f32 {
    C0_HZ * (cents / 1200.0).exp2()
}

pub const C0_HZ: f32 = 16.351_598;

#[inline]
pub fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

#[inline]
pub fn lin_to_db(x: f32) -> f32 {
    20.0 * x.max(1e-12).log10()
}

/// Tiny offset added in feedback paths: wasm has no flush-to-zero.
pub const DENORMAL: f32 = 1e-20;
