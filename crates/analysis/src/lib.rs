//! M0 analysis: Influence instant match and the pYIN-style second-pass pitch
//! tracker. Compiled natively (tests, the pitch_eval tool) and to wasm for the
//! analysis worker.

pub mod influence;
pub mod pyin;

pub use influence::{MATCH_LEN, MatchResult, instant_match};

// ---- C ABI for the analysis worker ----

/// Allocate `len` f32s in wasm memory for the host to fill. Leaked until `an_free`.
#[unsafe(no_mangle)]
pub extern "C" fn an_alloc(len: u32) -> *mut f32 {
    let mut v = vec![0.0f32; len as usize].into_boxed_slice();
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// # Safety
/// `ptr`/`len` must come from `an_alloc`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn an_free(ptr: *mut f32, len: u32) {
    if !ptr.is_null() {
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len as usize)) });
    }
}

static mut MATCH_OUT: [f32; MATCH_LEN] = [0.0; MATCH_LEN];

/// Run the instant match over `len` mono samples at `ptr`. Returns a pointer to
/// MATCH_LEN floats (layout in `MatchResult::to_floats`), valid until the next call.
///
/// # Safety
/// `ptr`/`len` must describe readable f32s in wasm memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn an_match(ptr: *const f32, len: u32, sample_rate: f32, n_bands: u32) -> *const f32 {
    let signal = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
    let r = instant_match(signal, sample_rate, n_bands as usize);
    let out = unsafe { &mut *std::ptr::addr_of_mut!(MATCH_OUT) };
    r.to_floats(out);
    out.as_ptr()
}
