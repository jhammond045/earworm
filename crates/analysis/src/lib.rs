//! Analysis: the M0 Influence instant match and pYIN-style pitch tracker, and
//! the M2 capture second pass (notes, hits, tempo, scale, kit slots) with the
//! stored-take tune call. Compiled natively (tests, the eval tools) and to
//! wasm for the analysis workers.

pub mod anchors;
pub mod beats;
pub mod full;
pub mod influence;
pub mod notes;
pub mod poly;
pub mod pyin;
pub mod score;
pub mod second;
pub mod signal;
pub mod tempo;

pub use full::{FULL_MATCH_LEN, FullMatch, FullOpts, GrainSuggestion, match_full};
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

static mut FULL_OUT: [f32; FULL_MATCH_LEN] = [0.0; FULL_MATCH_LEN];

/// Run `match_full` over `len` mono samples at `ptr`. `flags` is a bit set:
/// `full::FLAG_DECAYING` (1), `FLAG_USE_SOUND` (2), `FLAG_NO_SOUND` (4).
/// Returns a pointer to FULL_MATCH_LEN floats (layout documented at
/// `full::FULL_MATCH_LEN`), valid until the next call.
///
/// # Safety
/// `ptr`/`len` must describe readable f32s in wasm memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn an_match_full(ptr: *const f32, len: u32, sample_rate: f32, flags: u32) -> *const f32 {
    let signal = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
    let opts = FullOpts {
        decaying: flags & full::FLAG_DECAYING != 0,
        use_sound: if flags & full::FLAG_NO_SOUND != 0 {
            Some(false)
        } else if flags & full::FLAG_USE_SOUND != 0 {
            Some(true)
        } else {
            None
        },
    };
    let r = match_full(signal, sample_rate, opts);
    let out = unsafe { &mut *std::ptr::addr_of_mut!(FULL_OUT) };
    r.to_floats(out);
    out.as_ptr()
}

// ---- Poly takes (docs/poly.md, P3) ----

thread_local! {
    static POLY: std::cell::RefCell<Option<poly::PolyStream>> = const { std::cell::RefCell::new(None) };
}

/// Start a poly pass over a take at `sr` Hz, `samples` long (0: not known; the outputs then grow
/// as it comes, which costs more memory on a long take): `opts_len` floats of options (layout in
/// `poly::Options::from_floats`). Feed the take with `an_poly_feed`, then `an_poly_end`. A pass
/// already started is dropped.
///
/// # Safety
/// `opts`/`opts_len` must describe readable f32s in wasm memory (or be 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn an_poly_begin(sr: f32, samples: f64, opts: *const f32, opts_len: u32) {
    let opts = if opts_len == 0 { &[][..] } else { unsafe { std::slice::from_raw_parts(opts, opts_len as usize) } };
    // Drop the last pass before making room for this one.
    POLY.with(|p| *p.borrow_mut() = None);
    let mut s = poly::PolyStream::new(sr, poly::Options::from_floats(opts));
    if samples > 0.0 {
        s.expect(samples as u64);
    }
    POLY.with(|p| *p.borrow_mut() = Some(s));
}

/// The take's next `len` samples (mono). Model windows run as they fill, so a long take is read
/// in pieces and never held whole.
///
/// # Safety
/// `ptr`/`len` must describe readable f32s in wasm memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn an_poly_feed(ptr: *const f32, len: u32) {
    let x = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
    POLY.with(|p| {
        if let Some(s) = p.borrow_mut().as_mut() {
            s.feed(x);
        }
    });
}

/// End the pass: decode the notes and their curves. Returns a pointer to `an_result_len()` floats
/// (layout in `poly::PolyResult::to_floats`), valid until the next call; an empty result when no
/// pass was started.
#[unsafe(no_mangle)]
pub extern "C" fn an_poly_end() -> *const f32 {
    let r = POLY.with(|p| p.borrow_mut().take()).map(|s| s.finish()).unwrap_or_default();
    let mut out = Vec::new();
    r.to_floats(&mut out);
    publish(out)
}

// ---- Second pass and tune (M2) ----

thread_local! {
    static RESULT: std::cell::RefCell<Vec<f32>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn publish(v: Vec<f32>) -> *const f32 {
    RESULT.with(|r| {
        *r.borrow_mut() = v;
        r.borrow().as_ptr()
    })
}

/// Floats in the last `an_second` / `an_tune` result.
#[unsafe(no_mangle)]
pub extern "C" fn an_result_len() -> u32 {
    RESULT.with(|r| r.borrow().len() as u32)
}

/// Run the second pass over a take: `audio_len` mono samples at `sr`, `frames_len`
/// floats of 25-float frame records, `opts_len` floats of options (layout in
/// `second::Options::from_floats`). Returns a pointer to `an_result_len()` floats
/// (layout in `second::SecondPassResult::to_floats`), valid until the next call.
///
/// # Safety
/// Every pointer/length pair must describe readable f32s in wasm memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn an_second(
    audio: *const f32,
    audio_len: u32,
    sr: f32,
    frames: *const f32,
    frames_len: u32,
    opts: *const f32,
    opts_len: u32,
) -> *const f32 {
    let audio = unsafe { std::slice::from_raw_parts(audio, audio_len as usize) };
    let frames = unsafe { std::slice::from_raw_parts(frames, frames_len as usize) };
    let opts = if opts_len == 0 { &[][..] } else { unsafe { std::slice::from_raw_parts(opts, opts_len as usize) } };
    let r = second::second_pass(audio, sr, frames, &second::Options::from_floats(opts));
    let mut out = Vec::new();
    r.to_floats(&mut out);
    publish(out)
}

/// Floats in the flat tune settings: snap, speed ms, slides (0 glide, 1 gliss, 2 jump),
/// vibrato, root, scale index, reference cents, level out.
pub const TUNE_SETTINGS_LEN: usize = 8;

/// Tune a stored take: `frames_len` floats of 25-float records and `pitch` (one
/// repaired pitch per frame, cents, NaN unvoiced). Returns `2 * n` floats, the
/// tuned pitch per frame and then the loudness per frame after Level out.
///
/// Frames the second pass filled in (live pitch NaN, repaired pitch finite) count as
/// confident; the others use the live confidence.
///
/// # Safety
/// Every pointer/length pair must describe readable f32s in wasm memory;
/// `pitch` must hold `frames_len / 25` floats and `settings` `TUNE_SETTINGS_LEN`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn an_tune(frames: *const f32, frames_len: u32, pitch: *const f32, settings: *const f32) -> *const f32 {
    let frames = unsafe { std::slice::from_raw_parts(frames, frames_len as usize) };
    let n = frames.len() / second::STRIDE;
    let pitch = unsafe { std::slice::from_raw_parts(pitch, n) };
    let s = unsafe { std::slice::from_raw_parts(settings, TUNE_SETTINGS_LEN) };
    publish(tune_take(frames, pitch, s))
}

/// The work of `an_tune`, callable natively.
pub fn tune_take(frames: &[f32], pitch: &[f32], s: &[f32]) -> Vec<f32> {
    use dsp::tune::{SCALES, Slides, TuneSettings, level_out, tune_curve};
    let n = (frames.len() / second::STRIDE).min(pitch.len());
    let get = |i: usize, d: f32| s.get(i).copied().filter(|v| v.is_finite()).unwrap_or(d);
    let scale = (get(5, 0.0).round().max(0.0) as usize).min(SCALES.len() - 1);
    let settings = TuneSettings {
        snap: get(0, 0.0).clamp(0.0, 1.0),
        speed_ms: get(1, 60.0).clamp(0.0, 250.0),
        slides: Slides::from_index(get(2, 0.0)),
        vibrato: get(3, 1.0).clamp(0.0, 2.0),
        root: (get(4, 0.0).round() as i64).rem_euclid(12) as u8,
        mask: SCALES[scale].1,
        ref_cents: get(6, 0.0),
    };
    let rec = |k: usize| &frames[k * second::STRIDE..(k + 1) * second::STRIDE];
    let conf: Vec<f32> = (0..n)
        .map(|k| {
            let f = rec(k);
            if !f[dsp::frame::PITCH].is_finite() && pitch[k].is_finite() { 0.9 } else { f[dsp::frame::CONFIDENCE] }
        })
        .collect();
    let loud: Vec<f32> = (0..n).map(|k| rec(k)[dsp::frame::LOUDNESS]).collect();
    let mut out = vec![0.0f32; 2 * n];
    let (tuned, leveled) = out.split_at_mut(n);
    tune_curve(settings, &pitch[..n], &conf, tuned);
    level_out(&loud, get(7, 0.0), leveled);
    out
}
