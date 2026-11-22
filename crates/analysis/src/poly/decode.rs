//! Basic Pitch's note decoding (`basic_pitch/note_creation.py`, `output_to_notes_polyphonic` with
//! the defaults `predict` uses: onset threshold 0.5, frame threshold 0.3, notes longer than 11
//! frames, inferred onsets, the "melodia trick", energy tolerance 11), step for step, so the notes
//! match the Python package's. Values the Python code holds in float64 are float64 here too.
//!
//! The one change is speed, not result: the melodia step repeatedly takes the largest remaining
//! activation over the whole take. Activations only ever drop to zero, so visiting the cells in
//! one sorted order (largest first, ties in row-major order, as `np.argmax` breaks them) and
//! skipping the ones already zeroed gives the same sequence without rescanning the matrix.

use super::model::{HOP, MIDI_OFFSET, N_NOTES, SR};
use super::Activations;

pub struct Settings {
    pub onset_thresh: f64,
    pub frame_thresh: f64,
    pub min_note_len: usize,
    pub energy_tol: usize,
    /// Semitones (0..88) kept: activations outside [lo, hi) are zeroed first.
    pub lo: usize,
    pub hi: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { onset_thresh: 0.5, frame_thresh: 0.3, min_note_len: 11, energy_tol: 11, lo: 0, hi: N_NOTES }
    }
}

impl Settings {
    /// Keep MIDI notes lo..=hi.
    pub fn with_range(lo: i32, hi: i32) -> Self {
        let s = |m: i32| (m - MIDI_OFFSET).clamp(0, N_NOTES as i32) as usize;
        Settings { lo: s(lo), hi: s(hi + 1), ..Settings::default() }
    }
}

/// A decoded note: model frames [start, end), MIDI pitch, mean note activation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RawNote {
    pub start: usize,
    pub end: usize,
    pub midi: i32,
    pub amp: f32,
}

/// Basic Pitch's `model_frames_to_time`: frame times, with its correction per two-second window.
pub fn frame_time(i: usize) -> f64 {
    const ANNOT_N_FRAMES: f64 = 172.0; // ANNOTATIONS_FPS (86) x 2 s
    let hop = HOP as f64 / SR as f64;
    let offset = hop * (ANNOT_N_FRAMES - super::model::WIN as f64 / HOP as f64) + 0.0018;
    i as f64 * hop - offset * (i as f64 / ANNOT_N_FRAMES).floor()
}

struct Bits(Vec<u64>);

impl Bits {
    fn new(n: usize) -> Self {
        Bits(vec![0; n.div_ceil(64)])
    }
    #[inline]
    fn get(&self, i: usize) -> bool {
        self.0[i / 64] >> (i % 64) & 1 == 1
    }
    #[inline]
    fn set(&mut self, i: usize) {
        self.0[i / 64] |= 1 << (i % 64);
    }
}

pub fn notes(a: &Activations, s: &Settings) -> Vec<RawNote> {
    let n = a.frames;
    let f = N_NOTES;
    if n < 3 {
        return Vec::new();
    }
    // constrain_frequency: outside the range, frames and onsets are zero.
    let inside = |j: usize| j >= s.lo && j < s.hi;
    let frame = |i: usize, j: usize| if inside(j) { a.note[i * f + j] as f64 } else { 0.0 };
    let onset = |i: usize, j: usize| if inside(j) { a.onset[i * f + j] as f64 } else { 0.0 };

    // get_infered_onsets (n_diff 2).
    let diff = |i: usize, j: usize| {
        let prev = |k: usize| if i >= k { frame(i - k, j) } else { 0.0 };
        let d = (frame(i, j) - prev(1)).min(frame(i, j) - prev(2));
        if i < 2 || d < 0.0 { 0.0 } else { d }
    };
    let (mut max_on, mut max_diff) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for i in 0..n {
        for j in 0..f {
            max_on = max_on.max(onset(i, j));
            max_diff = max_diff.max(diff(i, j));
        }
    }
    // Recomputed where needed rather than stored: a five-minute take has 2.3 million cells.
    let peak = |i: usize, j: usize| onset(i, j).max(if max_diff > 0.0 { max_on * diff(i, j) / max_diff } else { 0.0 });

    // Peaks in time (scipy argrelmax, order 1) at or above the onset threshold, latest first.
    let mut starts = Vec::new();
    for i in 1..n - 1 {
        for j in 0..f {
            let v = peak(i, j);
            if v > peak(i - 1, j) && v > peak(i + 1, j) && v >= s.onset_thresh {
                starts.push((i, j));
            }
        }
    }
    starts.reverse();

    let mut zero = Bits::new(n * f);
    let rem = |zero: &Bits, i: usize, j: usize| if zero.get(i * f + j) { 0.0 } else { frame(i, j) };
    let mean = |i0: usize, i1: usize, j: usize| {
        let sum: f64 = (i0..i1).map(|i| a.note[i * f + j] as f64 * if inside(j) { 1.0 } else { 0.0 }).sum();
        (sum / (i1 - i0) as f64) as f32
    };
    let mut out = Vec::new();
    for (start, j) in starts {
        if start >= n - 1 {
            continue;
        }
        let mut i = start + 1;
        let mut k = 0;
        while i < n - 1 && k < s.energy_tol {
            if rem(&zero, i, j) < s.frame_thresh {
                k += 1;
            } else {
                k = 0;
            }
            i += 1;
        }
        i -= k;
        if i - start <= s.min_note_len {
            continue;
        }
        for t in start..i {
            zero.set(t * f + j);
            if j < f - 1 {
                zero.set(t * f + j + 1);
            }
            if j > 0 {
                zero.set(t * f + j - 1);
            }
        }
        out.push(RawNote { start, end: i, midi: j as i32 + MIDI_OFFSET, amp: mean(start, i, j) });
    }

    // The melodia trick: notes grown out of the largest activations left.
    let mut order: Vec<(f32, u32)> = (0..n * f)
        .filter(|&c| !zero.get(c) && frame(c / f, c % f) > s.frame_thresh)
        .map(|c| (a.note[c], c as u32))
        .collect();
    order.sort_by(|x, y| y.0.total_cmp(&x.0).then(x.1.cmp(&y.1)));
    for &(_, c) in &order {
        let c = c as usize;
        if zero.get(c) {
            continue;
        }
        let (mid, j) = (c / f, c % f);
        zero.set(c);
        let clear = |zero: &mut Bits, i: usize| {
            zero.set(i * f + j);
            if j < f - 1 {
                zero.set(i * f + j + 1);
            }
            if j > 0 {
                zero.set(i * f + j - 1);
            }
        };
        let mut i = mid + 1;
        let mut k = 0;
        while i < n - 1 && k < s.energy_tol {
            if rem(&zero, i, j) < s.frame_thresh {
                k += 1;
            } else {
                k = 0;
            }
            clear(&mut zero, i);
            i += 1;
        }
        let end = i - 1 - k;
        let mut i = mid as isize - 1;
        let mut k = 0;
        while i > 0 && k < s.energy_tol {
            if rem(&zero, i as usize, j) < s.frame_thresh {
                k += 1;
            } else {
                k = 0;
            }
            clear(&mut zero, i as usize);
            i -= 1;
        }
        let start = (i + 1 + k as isize) as usize;
        if end as isize - (start as isize) <= s.min_note_len as isize {
            continue;
        }
        out.push(RawNote { start, end, midi: j as i32 + MIDI_OFFSET, amp: mean(start, end, j) });
    }
    out.sort_by_key(|x| (x.start, x.midi));
    out
}
