//! Tempo and loop length (TDD "Second pass"). An inter-onset histogram
//! proposes periods for tempos from 60 to 180 bpm; the winner is the one whose
//! half-beat grid the onsets snap to best; the loop is a whole number of beats.

/// Slowest and fastest tempo considered.
pub const BPM_MIN: f32 = 60.0;
pub const BPM_MAX: f32 = 180.0;
/// Fewer onsets than this give no tempo.
pub const MIN_ONSETS: usize = 4;
/// Mean snap error, as a share of the half-beat grid step, above which no tempo is claimed.
/// Onsets with no relation to a grid score about 0.25 (spread 0.144 / sqrt(n)), and many
/// periods are tried, so the limit also has to sit `SIGNIFICANCE` spreads below that.
pub const MAX_SNAP_ERROR: f32 = 0.15;
pub const SIGNIFICANCE: f32 = 3.5;
/// Tempo the tie-break prefers when two fit equally (octave ambiguity).
const PREFERRED_BPM: f32 = 110.0;

#[derive(Clone, Copy, Debug)]
pub struct Tempo {
    pub bpm: f32,
    /// Mean distance from the nearest half-beat line as a share of the step (0 to 0.5).
    pub snap_error: f32,
    /// Time of a grid line, seconds.
    pub phase: f32,
}

/// Mean snap error of `onsets` to a half-beat grid for beat period `p`, and the best phase.
fn snap_error(onsets: &[f32], p: f32) -> (f32, f32) {
    let g = p / 2.0;
    let (mut c, mut s) = (0.0f64, 0.0f64);
    for &t in onsets {
        let a = (t as f64 / g as f64).fract() * std::f64::consts::TAU;
        c += a.cos();
        s += a.sin();
    }
    let phase = (s.atan2(c) / std::f64::consts::TAU).rem_euclid(1.0) as f32 * g;
    let err = onsets
        .iter()
        .map(|&t| {
            let x = ((t - phase) / g).rem_euclid(1.0);
            x.min(1.0 - x)
        })
        .sum::<f32>()
        / onsets.len() as f32;
    (err, phase)
}

/// Tempo of a set of onset times (seconds, ascending), or None when there are
/// too few onsets or none of the proposals fit.
pub fn find_tempo(onsets: &[f32]) -> Option<Tempo> {
    if onsets.len() < MIN_ONSETS {
        return None;
    }
    let (p_lo, p_hi) = (60.0 / BPM_MAX, 60.0 / BPM_MIN);
    // Histogram over log period, 2% bins.
    let bin_w = 0.02f32;
    let lo_bin = (p_lo.ln() / bin_w).floor() as i32 - 2;
    let hi_bin = (p_hi.ln() / bin_w).ceil() as i32 + 2;
    let nbins = (hi_bin - lo_bin + 1) as usize;
    let mut hist = vec![0.0f32; nbins];
    const RATIOS: [(f32, f32); 7] = [(0.25, 0.4), (1.0 / 3.0, 0.5), (0.5, 0.8), (1.0, 1.0), (2.0, 0.8), (3.0, 0.5), (4.0, 0.4)];
    for i in 0..onsets.len() {
        for j in i + 1..onsets.len().min(i + 4) {
            let d = onsets[j] - onsets[i];
            if !(0.1..=8.0).contains(&d) {
                continue;
            }
            for &(r, w) in &RATIOS {
                let p = d * r;
                if !(p_lo..=p_hi).contains(&p) {
                    continue;
                }
                let b = (p.ln() / bin_w).round() as i32 - lo_bin;
                let w = w / (j - i) as f32;
                for (db, k) in [(-1i32, 0.5f32), (0, 1.0), (1, 0.5)] {
                    let idx = b + db;
                    if idx >= 0 && (idx as usize) < nbins {
                        hist[idx as usize] += w * k;
                    }
                }
            }
        }
    }
    // Top peaks as candidates.
    let mut peaks: Vec<(usize, f32)> = (1..nbins.saturating_sub(1))
        .filter(|&i| hist[i] > 0.0 && hist[i] >= hist[i - 1] && hist[i] > hist[i + 1])
        .map(|i| (i, hist[i]))
        .collect();
    peaks.sort_by(|a, b| b.1.total_cmp(&a.1));
    peaks.truncate(8);
    let mut best: Option<(f32, Tempo)> = None;
    for (i, _) in peaks {
        let p0 = ((i as i32 + lo_bin) as f32 * bin_w).exp();
        // Refine the period around the peak.
        for step in -10..=10 {
            let p = p0 * (1.0 + 0.004 * step as f32);
            if !(p_lo..=p_hi).contains(&p) {
                continue;
            }
            let (err, phase) = snap_error(onsets, p);
            let bpm = 60.0 / p;
            let score = err + 0.03 * (bpm / PREFERRED_BPM).log2().abs();
            if best.as_ref().is_none_or(|b| score < b.0) {
                best = Some((score, Tempo { bpm, snap_error: err, phase }));
            }
        }
    }
    let limit = MAX_SNAP_ERROR.min(0.25 - SIGNIFICANCE * 0.144 / (onsets.len() as f32).sqrt());
    best.map(|b| b.1).filter(|t| t.snap_error <= limit)
}

/// Whole beats in a loop of `len` seconds at `bpm`, at least 1.
pub fn loop_beats(len: f32, bpm: f32) -> u32 {
    ((len * bpm / 60.0).round() as i64).max(1) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(bpm: f32, pattern: &[f32], bars: usize, jitter: f32) -> Vec<f32> {
        let beat = 60.0 / bpm;
        let mut s = 99u32;
        let mut out = Vec::new();
        for b in 0..bars {
            for &x in pattern {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                let j = ((s >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 2.0 * jitter;
                out.push(0.3 + (b as f32 * 4.0 + x) * beat + j);
            }
        }
        out
    }

    #[test]
    fn finds_tempos_across_the_range() {
        let pattern = [0.0, 0.5, 1.0, 1.5, 2.0, 3.0, 3.5];
        for bpm in [64.0f32, 80.0, 100.0, 120.0, 150.0, 172.0] {
            let t = find_tempo(&grid(bpm, &pattern, 3, 0.012)).unwrap_or_else(|| panic!("no tempo at {bpm}"));
            // Half or double the truth fits an eighth-note pattern just as well; accept those too.
            let ok = [1.0f32, 2.0, 0.5].iter().any(|k| (t.bpm / bpm / k - 1.0).abs() < 0.02);
            assert!(ok, "{bpm}: got {}", t.bpm);
        }
    }

    #[test]
    fn quarter_notes_only_settles_on_an_octave_of_the_truth() {
        let t = find_tempo(&grid(90.0, &[0.0, 1.0, 2.0, 3.0], 3, 0.01)).unwrap();
        let r = t.bpm / 90.0;
        assert!([1.0f32, 2.0].iter().any(|k| (r / k - 1.0).abs() < 0.03) || (r - 0.5).abs() < 0.02, "{}", t.bpm);
    }

    #[test]
    fn too_few_or_unrelated_onsets_give_none() {
        assert!(find_tempo(&[]).is_none());
        assert!(find_tempo(&[0.5, 1.0, 1.5]).is_none());
        let odd = [0.31f32, 0.97, 1.13, 2.41, 2.62, 3.9, 4.04, 5.37, 5.5, 6.93];
        assert!(find_tempo(&odd).is_none(), "{:?}", find_tempo(&odd));
    }

    #[test]
    fn loop_rounds_to_whole_beats() {
        assert_eq!(loop_beats(8.1, 120.0), 16);
        assert_eq!(loop_beats(7.8, 120.0), 16);
        assert_eq!(loop_beats(0.1, 100.0), 1);
    }
}
