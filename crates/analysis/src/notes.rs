//! Notes, tuning reference and scale (TDD "Second pass").
//!
//! A voiced run is split where the slow pitch (3 Hz low-pass, as in the
//! tuner) leaves the note's level by more than 80 cents for 60 ms, where the
//! level dips by 6 dB and comes back, and at onsets. Pieces under 80 ms are
//! merged into their neighbour or dropped.

use crate::signal::{FPS, Levels, Onset, refine_onset};
use dsp::tune::{SCALES, SPLIT_HZ};

/// Shortest note, frames (80 ms).
pub const MIN_NOTE: usize = 8;
/// Unvoiced gaps up to this many frames inside a run are bridged.
pub const BRIDGE: usize = 3;
/// Slow pitch this far from the note's level counts as a move, cents ...
pub const MOVE_CENTS: f32 = 80.0;
/// ... for this many frames (60 ms).
pub const MOVE_FRAMES: usize = 6;
/// A dip in level of this many dB, and back up, splits a note.
pub const DIP_DB: f32 = 6.0;
/// A pitch cut this close (frames) after an onset or dip cut is part of the same boundary.
pub const PITCH_NEAR: usize = 10;
/// An onset inside a note splits it only when the level rises this much, dB.
pub const ONSET_SPLIT_DB: f32 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Note {
    pub t: f32,
    pub dur: f32,
    pub cents: f32,
    pub hard: f32,
}

/// Frame range of a note, `start..end`.
#[derive(Clone, Copy, Debug)]
pub struct Seg {
    pub start: usize,
    pub end: usize,
}

fn median(v: &mut [f32]) -> f32 {
    if v.is_empty() {
        return f32::NAN;
    }
    let mid = v.len() / 2;
    *v.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1
}

fn median_of(p: &[f32]) -> f32 {
    let mut v: Vec<f32> = p.iter().copied().filter(|x| x.is_finite()).collect();
    median(&mut v)
}

/// Median filter with an odd window, NaN-aware (NaN frames keep NaN).
fn median_filter(p: &[f32], w: usize) -> Vec<f32> {
    let h = w / 2;
    (0..p.len())
        .map(|i| {
            if !p[i].is_finite() {
                return f32::NAN;
            }
            median_of(&p[i.saturating_sub(h)..(i + h + 1).min(p.len())])
        })
        .collect()
}

/// Voiced runs of `pitch` as frame ranges, bridging short gaps.
pub fn voiced_runs(pitch: &[f32]) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let n = pitch.len();
    let mut i = 0;
    while i < n {
        if !pitch[i].is_finite() {
            i += 1;
            continue;
        }
        let start = i;
        let mut last = i;
        loop {
            let mut j = last + 1;
            while j < n && !pitch[j].is_finite() {
                j += 1;
            }
            if j < n && j - last - 1 <= BRIDGE {
                last = j;
            } else {
                break;
            }
        }
        runs.push((start, last + 1));
        i = last + 1;
    }
    runs
}

/// Boundaries inside one run from slow pitch moves, as frame indices.
fn pitch_splits(pitch: &[f32], a: usize, b: usize) -> Vec<usize> {
    let mut out = Vec::new();
    // Hold through bridged gaps so the filters see a continuous curve.
    let mut raw: Vec<f32> = Vec::with_capacity(b - a);
    let mut last = pitch[a];
    for &p in &pitch[a..b] {
        if p.is_finite() {
            last = p;
        }
        raw.push(last);
    }
    let n = raw.len();
    if n < 2 * MIN_NOTE {
        return out;
    }
    let mf = median_filter(&raw, 5);
    let alpha = 1.0 - (-2.0 * std::f32::consts::PI * SPLIT_HZ / FPS).exp();
    let mut anchor = median_of(&raw[..5.min(n)]);
    let mut slow = anchor;
    let mut seg_start = 0usize;
    let mut cnt = 0usize;
    let mut k = 1usize;
    while k < n {
        slow += alpha * (raw[k] - slow);
        let dev = slow - anchor;
        if dev.abs() > MOVE_CENTS {
            cnt += 1;
        } else {
            cnt = 0;
            if dev.abs() < 50.0 {
                anchor += 0.05 * dev;
            }
        }
        if cnt >= MOVE_FRAMES {
            // Wait for the move to settle, then take its end as the new level.
            let mut j = k;
            let mut level = f32::NAN;
            while j < (k + 25).min(n.saturating_sub(8)) {
                let w = &mf[j..j + 8];
                let (lo, hi) = w.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(l, h), &x| (l.min(x), h.max(x)));
                if hi - lo < 40.0 {
                    level = median_of(&raw[j..j + 8]);
                    break;
                }
                j += 1;
            }
            if !level.is_finite() {
                j = k;
                level = median_of(&raw[k..(k + 8).min(n)]);
            }
            // The boundary sits where the curve is first nearer the new level.
            let mut bnd = k;
            while bnd - 1 > seg_start && (mf[bnd - 1] - level).abs() < (mf[bnd - 1] - anchor).abs() {
                bnd -= 1;
            }
            if bnd > seg_start {
                out.push(a + bnd);
                seg_start = bnd;
            }
            anchor = level;
            slow = level;
            cnt = 0;
            k = (j + 8).max(k + 1);
            continue;
        }
        k += 1;
    }
    out
}

/// Boundaries from level dips: the level falls `DIP_DB` below its peak since
/// the last boundary and then climbs back `DIP_DB` above the trough.
fn dip_splits(level: &[f32], a: usize, b: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut pk = level[a];
    let mut trough = f32::INFINITY;
    let mut ti = a;
    for k in a..b {
        let l = level[k];
        if l > pk {
            pk = l;
            trough = f32::INFINITY;
        }
        if l < pk - DIP_DB && l < trough {
            trough = l;
            ti = k;
        }
        if trough.is_finite() && l > trough + DIP_DB {
            out.push(ti);
            pk = l;
            trough = f32::INFINITY;
        }
    }
    out
}

/// Notes from the pitch curve (repaired, cents, NaN unvoiced), the level curve
/// and the onsets. Returns each note with its frame range and median pitch.
pub fn find_segments(pitch: &[f32], lv: &Levels, onsets: &[Onset]) -> Vec<(Seg, f32)> {
    let n = pitch.len().min(lv.n);
    let mut segs: Vec<(Seg, f32)> = Vec::new();
    for (a, b) in voiced_runs(&pitch[..n]) {
        if b - a < MIN_NOTE {
            continue;
        }
        let mut cuts: Vec<(usize, u8)> = Vec::new();
        for c in pitch_splits(pitch, a, b) {
            cuts.push((c, 1));
        }
        for c in dip_splits(&lv.frame_db, a, b) {
            cuts.push((c, 2));
        }
        for o in onsets {
            let k = o.frame;
            if k >= a + 3 && k + 3 < b {
                let before = (k.saturating_sub(6)..k).map(|j| lv.frame_db[j]).fold(f32::INFINITY, f32::min);
                let after = lv.frame_db[k..(k + 5).min(n)].iter().copied().fold(f32::NEG_INFINITY, f32::max);
                if after - before >= ONSET_SPLIT_DB {
                    cuts.push((k, 0));
                }
            }
        }
        // Articulation cuts (onset, dip) beat pitch cuts: a pitch change within
        // `PITCH_NEAR` frames of an articulation belongs to the same boundary.
        cuts.sort_by_key(|c| (if c.1 == 1 { 2 } else if c.1 == 2 { 1 } else { 0 }, c.0));
        let mut kept: Vec<(usize, u8)> = Vec::new();
        for (c, ty) in cuts {
            let near = |k: usize, kt: u8| {
                let gap = if ty == 1 && kt != 1 { PITCH_NEAR } else { 4 };
                k.abs_diff(c) < gap
            };
            if !kept.iter().any(|&(k, kt)| near(k, kt)) {
                kept.push((c, ty));
            }
        }
        let mut kept: Vec<usize> = kept.into_iter().map(|k| k.0).collect();
        kept.sort_unstable();
        let mut bounds = vec![a];
        bounds.extend(kept);
        bounds.push(b);
        let mut pieces: Vec<Seg> = bounds.windows(2).map(|w| Seg { start: w[0], end: w[1] }).collect();
        merge_fragments(&mut pieces, pitch);
        for s in pieces {
            let m = median_of(&pitch[s.start..s.end]);
            if m.is_finite() {
                segs.push((s, m));
            }
        }
    }
    segs
}

/// Merge pieces shorter than `MIN_NOTE` into a neighbour (the closer in pitch
/// when there are two); a short piece alone is dropped.
fn merge_fragments(pieces: &mut Vec<Seg>, pitch: &[f32]) {
    loop {
        // Shortest first.
        let Some(i) = (0..pieces.len()).filter(|&j| pieces[j].end - pieces[j].start < MIN_NOTE).min_by_key(|&j| pieces[j].end - pieces[j].start) else { return };
        if pieces.len() == 1 {
            pieces.clear();
            return;
        }
        let into_left = if i == 0 {
            false
        } else if i + 1 == pieces.len() {
            true
        } else {
            let me = median_of(&pitch[pieces[i].start..pieces[i].end]);
            let l = median_of(&pitch[pieces[i - 1].start..pieces[i - 1].end]);
            let r = median_of(&pitch[pieces[i + 1].start..pieces[i + 1].end]);
            (me - l).abs() <= (me - r).abs()
        };
        if into_left {
            pieces[i - 1].end = pieces[i].end;
        } else {
            pieces[i + 1].start = pieces[i].start;
        }
        pieces.remove(i);
    }
}

/// Attack start for a note starting at `frame`: the onset detector's time if
/// it found one close by, else the frame time walked back up the rise.
pub fn note_start_time(lv: &Levels, onsets: &[Onset], frame: usize, is_run_start: bool) -> f32 {
    let reach = if is_run_start { 8 } else { 5 };
    if let Some(o) = onsets
        .iter()
        .filter(|o| o.frame + reach >= frame && o.frame <= frame + 3)
        .min_by_key(|o| o.frame.abs_diff(frame))
    {
        return o.t;
    }
    if is_run_start {
        // Walk back up the rise to the first audible frame, then to the attack on the fine envelope.
        let mut j = frame;
        while j > 0 && frame - j < 15 && lv.frame_db[j - 1] > lv.floor_db + 8.0 {
            j -= 1;
        }
        return refine_onset(lv, j as f32 / FPS).min(frame as f32 / FPS);
    }
    frame as f32 / FPS
}

/// 10% to 90% rise time (seconds) of the fine envelope around `t`, looking
/// 20 ms back and `span` ahead. None when the envelope does not rise.
pub fn rise_time(lv: &Levels, t: f32, span: f32) -> Option<f32> {
    if lv.fine.is_empty() {
        return None;
    }
    let a = lv.fine_index((t - 0.020).max(0.0));
    let b = lv.fine_index(t + span);
    if b <= a + 2 {
        return None;
    }
    let lo = lv.fine[a..=b].iter().cloned().fold(f32::INFINITY, f32::min);
    let (pi, hi) = (a..=b).map(|j| (j, lv.fine[j])).fold((a, 0.0f32), |m, x| if x.1 > m.1 { x } else { m });
    // The envelope must rise by at least 3 dB to have an attack.
    if hi < lo * 1.41 || hi <= 0.0 {
        return None;
    }
    let lo_i = (a..=pi).rev().find(|&j| lv.fine[j] <= lo * 1.0001 + 1e-9).unwrap_or(a);
    let level = |frac: f32| lo + frac * (hi - lo);
    let t10 = (lo_i..=pi).find(|&j| lv.fine[j] >= level(0.1))?;
    let t90 = (t10..=pi).find(|&j| lv.fine[j] >= level(0.9))?;
    Some((t90 - t10).max(1) as f32 * lv.block as f32 / lv.sr)
}

/// Hardness 0..1 from an attack time: 5 ms or faster is 1, 80 ms or slower
/// (or no rise at all) is 0, log-spaced between.
pub fn hardness_from_attack(attack_s: Option<f32>) -> f32 {
    match attack_s {
        None => 0.0,
        Some(a) => (1.0 - (a / 0.005).max(1.0).ln() / 16f32.ln()).clamp(0.0, 1.0),
    }
}

/// The notes of a take.
pub fn find_notes(pitch: &[f32], lv: &Levels, onsets: &[Onset], stored_hard: &[f32], stored_onset: &[f32]) -> Vec<Note> {
    let segs = find_segments(pitch, lv, onsets);
    let n = pitch.len().min(lv.n);
    // Run starts: segments not immediately following another one.
    let mut notes = Vec::with_capacity(segs.len());
    for (i, (s, m)) in segs.iter().enumerate() {
        let run_start = i == 0 || segs[i - 1].0.end < s.start;
        let t = note_start_time(lv, onsets, s.start, run_start);
        let end_t = s.end as f32 / FPS;
        let attack = rise_time(lv, t, 0.12);
        let mut hard = hardness_from_attack(attack);
        // Live hardness is refined 10 ms after its onset; keep the larger reading when the
        // live tier saw an onset at this note's start.
        let lo = s.start.saturating_sub(5);
        let hi = (s.start + 4).min(n);
        if (lo..hi).any(|k| stored_onset.get(k).is_some_and(|&v| v > 0.0)) {
            let live = (lo..hi).filter_map(|k| stored_hard.get(k).copied()).fold(0.0f32, f32::max);
            hard = hard.max(live.clamp(0.0, 1.0));
        }
        notes.push(Note { t, dur: (end_t - t).max(0.0), cents: *m, hard });
    }
    notes
}

/// Circular mean of pitch modulo 100 cents, duration weighted over notes, in -50..50.
pub fn tuning_reference(items: &[(f32, f32)]) -> f32 {
    let (mut c, mut s, mut w) = (0.0f64, 0.0f64, 0.0f64);
    for &(cents, weight) in items {
        if !cents.is_finite() || !(weight > 0.0) {
            continue;
        }
        let a = (cents as f64).rem_euclid(100.0) / 100.0 * std::f64::consts::TAU;
        c += weight as f64 * a.cos();
        s += weight as f64 * a.sin();
        w += weight as f64;
    }
    if w <= 0.0 || (c * c + s * s).sqrt() < 1e-3 * w {
        return 0.0;
    }
    let mut r = s.atan2(c) / std::f64::consts::TAU * 100.0; // -50..50
    if r >= 50.0 {
        r -= 100.0;
    }
    r as f32
}

/// Scale detection: duration-weighted pitch-class histogram (relative to the
/// tuning reference) correlated against every template at 12 roots. Returns
/// (root pitch class, scale index). Ties go to the scale with fewer notes, then
/// to the root with more weight in the histogram, then to the lower index.
pub fn detect_scale(items: &[(f32, f32)], ref_cents: f32) -> (u8, usize) {
    let mut hist = [0.0f64; 12];
    let mut total = 0.0;
    for &(cents, w) in items {
        if !cents.is_finite() || !(w > 0.0) {
            continue;
        }
        let pc = (((cents - ref_cents) / 100.0).round() as i64).rem_euclid(12) as usize;
        hist[pc] += w as f64;
        total += w as f64;
    }
    if total <= 0.0 {
        return (0, 0);
    }
    let mean_h = total / 12.0;
    let var_h: f64 = hist.iter().map(|h| (h - mean_h).powi(2)).sum();
    if var_h <= 0.0 {
        return (0, 0);
    }
    // Chromatic has a flat template, so its correlation is 0: it wins when nothing else correlates.
    let mut best = (0u8, 0usize);
    let mut best_key = (0.0f64, 12u32, 0.0f64);
    let mut first = true;
    for (si, &(_, mask)) in SCALES.iter().enumerate().skip(1) {
        let count = mask.count_ones();
        let mean_t = count as f64 / 12.0;
        let var_t: f64 = (0..12).map(|i| (((mask >> i) & 1) as f64 - mean_t).powi(2)).sum();
        for root in 0..12u8 {
            let mut cov = 0.0;
            for pc in 0..12usize {
                let t = ((mask >> ((pc + 12 - root as usize) % 12)) & 1) as f64;
                cov += (hist[pc] - mean_h) * (t - mean_t);
            }
            let r = cov / (var_h * var_t).sqrt();
            let key = (r, count, hist[root as usize]);
            let better = first
                || key.0 > best_key.0 + 1e-3
                || ((key.0 - best_key.0).abs() <= 1e-3 && (key.1 < best_key.1 || (key.1 == best_key.1 && key.2 > best_key.2 + 1e-9)));
            if better {
                best = (root, si);
                best_key = key;
                first = false;
            }
        }
    }
    if best_key.0 < MIN_SCALE_CORR { (0, 0) } else { best }
}

/// Correlation needed for a scale other than chromatic.
pub const MIN_SCALE_CORR: f64 = 0.3;

#[cfg(test)]
mod tests {
    use super::*;

    fn lv_flat(n: usize, db: f32) -> Levels {
        let mut lv = Levels::new(&vec![0.0; n * 480], 48_000.0, n);
        lv.frame_db = vec![db; n];
        lv
    }

    fn curve(parts: &[(usize, f32)]) -> Vec<f32> {
        parts.iter().flat_map(|&(n, c)| std::iter::repeat(c).take(n)).collect()
    }

    #[test]
    fn tuning_reference_is_circular_mean() {
        let r = tuning_reference(&[(4830.0, 1.0), (5230.0, 1.0), (6130.0, 2.0)]);
        assert!((r - 30.0).abs() < 0.5, "{r}");
        let r = tuning_reference(&[(4770.0, 1.0), (5170.0, 1.0)]);
        assert!((r + 30.0).abs() < 0.5, "{r}");
        // Wraps: +10 and -10 around the same step.
        let r = tuning_reference(&[(4990.0, 1.0), (5010.0, 1.0)]);
        assert!(r.abs() < 0.5, "{r}");
        assert_eq!(tuning_reference(&[]), 0.0);
        // 49.9 stays near +50 side without flipping to nonsense.
        assert!(tuning_reference(&[(4849.0, 1.0)]).abs() <= 50.0);
    }

    #[test]
    fn scale_finds_major_and_minor_and_roots() {
        // C major scale notes with C heavier.
        let c = 4800.0f32; // C4-ish pitch class 0 modulo 1200: 4800/100 = 48 -> pc 0
        let major = [0, 2, 4, 5, 7, 9, 11];
        let items: Vec<(f32, f32)> = major.iter().map(|&s| (c + 100.0 * s as f32, if s == 0 { 2.0 } else { 1.0 })).collect();
        let (root, scale) = detect_scale(&items, 0.0);
        assert_eq!((root, SCALES[scale].0), (0, "major"), "{root} {scale}");
        // D major: shift by 2 semitones.
        let items: Vec<(f32, f32)> = items.iter().map(|&(p, w)| (p + 200.0, w)).collect();
        let (root, scale) = detect_scale(&items, 0.0);
        assert_eq!((root, SCALES[scale].0), (2, "major"));
        // A natural minor with A heaviest: same notes as C major, root A.
        let items: Vec<(f32, f32)> = major.iter().map(|&s| (c + 100.0 * s as f32, if s == 9 { 3.0 } else { 1.0 })).collect();
        let (root, scale) = detect_scale(&items, 0.0);
        assert_eq!((root, SCALES[scale].0), (9, "minor"));
    }

    #[test]
    fn scale_ties_go_to_the_simpler_scale() {
        // C D E G A equally: pentatonic (5 notes) beats major (7) on ties.
        let items: Vec<(f32, f32)> = [0, 2, 4, 7, 9].iter().map(|&s| (4800.0 + 100.0 * s as f32, 1.0)).collect();
        let (root, scale) = detect_scale(&items, 0.0);
        assert_eq!(SCALES[scale].1.count_ones(), 5, "{root} {}", SCALES[scale].0);
        // A flat histogram has no scale.
        let items: Vec<(f32, f32)> = (0..12).map(|s| (4800.0 + 100.0 * s as f32, 1.0)).collect();
        assert_eq!(detect_scale(&items, 0.0), (0, 0));
        assert_eq!(detect_scale(&[], 0.0), (0, 0));
    }

    #[test]
    fn scale_follows_the_tuning_reference() {
        let items: Vec<(f32, f32)> = [0, 2, 4, 5, 7, 9, 11].iter().map(|&s| (4800.0 + 100.0 * s as f32 + 35.0, 1.0)).collect();
        assert_eq!(SCALES[detect_scale(&items, 35.0).1].0, "major");
        assert_eq!(detect_scale(&items, 35.0).0, 0);
    }

    #[test]
    fn splits_a_step_and_keeps_a_steady_note() {
        let n = 100;
        let lv = lv_flat(n, -30.0);
        let pitch = curve(&[(10, f32::NAN), (35, 5000.0), (40, 5300.0), (15, f32::NAN)]);
        let segs = find_segments(&pitch, &lv, &[]);
        assert_eq!(segs.len(), 2, "{segs:?}");
        assert!((segs[0].1 - 5000.0).abs() < 1.0 && (segs[1].1 - 5300.0).abs() < 1.0);
        assert!(segs[0].0.end.abs_diff(45) <= 3, "{:?}", segs[0].0);
        let steady = curve(&[(10, f32::NAN), (80, 5000.0), (10, f32::NAN)]);
        assert_eq!(find_segments(&steady, &lv, &[]).len(), 1);
    }

    #[test]
    fn splits_a_semitone_step() {
        let lv = lv_flat(120, -30.0);
        let pitch = curve(&[(50, 5000.0), (60, 5100.0), (10, f32::NAN)]);
        let segs = find_segments(&pitch, &lv, &[]);
        assert_eq!(segs.len(), 2, "{segs:?}");
        assert!(segs[0].0.end.abs_diff(50) <= 5);
    }

    #[test]
    fn vibrato_does_not_split() {
        let lv = lv_flat(150, -30.0);
        let pitch: Vec<f32> = (0..150).map(|i| 5000.0 + 60.0 * (std::f32::consts::TAU * 5.5 * i as f32 / 100.0).sin()).collect();
        assert_eq!(find_segments(&pitch, &lv, &[]).len(), 1);
    }

    #[test]
    fn a_slide_is_two_notes_not_three() {
        let lv = lv_flat(150, -30.0);
        let mut pitch = vec![5000.0f32; 50];
        pitch.extend((0..20).map(|i| 5000.0 + 300.0 * i as f32 / 19.0));
        pitch.extend(vec![5300.0f32; 60]);
        let segs = find_segments(&pitch, &lv, &[]);
        assert_eq!(segs.len(), 2, "{segs:?}");
        let mid = segs[0].0.end;
        assert!((50..=70).contains(&mid), "{mid}");
    }

    #[test]
    fn a_level_dip_splits_repeated_pitch_and_a_swell_does_not() {
        let mut lv = lv_flat(150, -20.0);
        for k in 70..80 {
            lv.frame_db[k] = -32.0;
        }
        let pitch = curve(&[(10, f32::NAN), (130, 5000.0), (10, f32::NAN)]);
        let segs = find_segments(&pitch, &lv, &[]);
        assert_eq!(segs.len(), 2, "{segs:?}");
        assert!(segs[0].0.end.abs_diff(75) <= 6);
        // A 5 dB dip is not enough; a swell up from quiet is not a dip.
        let mut lv = lv_flat(150, -20.0);
        for k in 70..80 {
            lv.frame_db[k] = -25.0;
        }
        assert_eq!(find_segments(&pitch, &lv, &[]).len(), 1);
        let mut lv = lv_flat(150, -20.0);
        for k in 10..60 {
            lv.frame_db[k] = -20.0 - (60 - k) as f32 * 0.5;
        }
        assert_eq!(find_segments(&pitch, &lv, &[]).len(), 1);
    }

    #[test]
    fn short_pieces_are_merged_or_dropped() {
        let lv = lv_flat(120, -30.0);
        // A 50 ms blip is dropped; a short leading scoop joins the note after it.
        let pitch = curve(&[(5, 5000.0), (30, f32::NAN), (5, 5200.0), (5, 5000.0), (60, 5000.0), (10, f32::NAN)]);
        let segs = find_segments(&pitch, &lv, &[]);
        assert_eq!(segs.len(), 1, "{segs:?}");
        let scoop = curve(&[(10, f32::NAN), (4, 4850.0), (3, 4930.0), (70, 5000.0), (10, f32::NAN)]);
        let segs = find_segments(&scoop, &lv, &[]);
        assert_eq!(segs.len(), 1);
        assert!((segs[0].1 - 5000.0).abs() < 1.0);
        assert_eq!(segs[0].0.start, 10);
    }

    #[test]
    fn gaps_up_to_three_frames_are_bridged() {
        let p = curve(&[(20, 5000.0), (3, f32::NAN), (20, 5000.0), (4, f32::NAN), (20, 5000.0)]);
        let runs = voiced_runs(&p);
        assert_eq!(runs, vec![(0, 43), (47, 67)]);
    }

    #[test]
    fn hardness_follows_attack_time() {
        assert_eq!(hardness_from_attack(None), 0.0);
        assert_eq!(hardness_from_attack(Some(0.002)), 1.0);
        assert!(hardness_from_attack(Some(0.02)) > 0.3 && hardness_from_attack(Some(0.02)) < 0.7);
        assert!(hardness_from_attack(Some(0.2)) < 0.05);
    }
}
