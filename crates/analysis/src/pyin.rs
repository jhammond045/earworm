//! pYIN-style second-pass pitch tracker (TDD "Second pass", Pitch repair):
//! several YIN candidates per frame, a Viterbi path over them, then repair of
//! the live curve where the two disagree by > 80 cents for >= 30 ms.

use dsp::resample::{Decimator, decimation_factor};
use dsp::yin::{cmnd, parabolic};
use dsp::hz_to_cents;

const FMIN: f32 = 70.0;
const FMAX: f32 = 1000.0;
const BIN_CENTS: f32 = 20.0;
/// Pitch may move this many bins per frame (12 * 20 = 240 cents).
const JUMP_BINS: usize = 12;
/// Chance per frame of switching between voiced and unvoiced.
const SWITCH_P: f32 = 0.01;
const N_THRESH: usize = 100;
/// Frames quieter than this (RMS) are forced unvoiced.
const SILENCE_RMS: f32 = 1e-4;

/// Pitch curve at 100 frames per second (cents above C0, NaN unvoiced),
/// frame i centred on time i * 0.01 s to match the live tier's stored frames.
pub fn pyin(signal: &[f32], sample_rate: f32) -> Vec<f32> {
    if !(sample_rate.is_finite() && sample_rate >= 4000.0) || signal.is_empty() {
        return Vec::new();
    }
    let n_frames = (signal.len() as f64 * 100.0 / sample_rate as f64).floor() as usize;
    if n_frames == 0 {
        return Vec::new();
    }

    // Decimate to about 16 kHz. The FIR delays the signal by 15 input samples.
    let factor = decimation_factor(sample_rate);
    let mut dec = Decimator::new(factor);
    let delay = dec.delay_samples();
    let xd: Vec<f32> = signal.iter().filter_map(|&v| dec.process(if v.is_finite() { v } else { 0.0 })).collect();
    let sr_d = sample_rate / factor as f32;

    let win = ((0.04 * sr_d).round() as usize).max(64);
    let max_lag = (sr_d / FMIN).ceil() as usize + 2;
    let min_lag = ((sr_d / FMAX).floor() as usize).max(2);
    let flen = win + max_lag;
    let mut buf = vec![0.0f32; flen];
    let mut d = vec![0.0f32; max_lag];

    let c70 = hz_to_cents(FMIN);
    let n_bins = ((hz_to_cents(FMAX) - c70) / BIN_CENTS).ceil() as usize + 1;
    let bin_of = |cents: f32| ((cents - c70) / BIN_CENTS).round() as isize;

    // Beta(2, 18) prior over thresholds 0.01..1: pdf ~ x (1 - x)^17.
    let mut thresholds = [0.0f32; N_THRESH];
    let mut weights = [0.0f32; N_THRESH];
    let mut wsum = 0.0;
    for j in 0..N_THRESH {
        let t = 0.01 * (j + 1) as f32;
        thresholds[j] = t;
        weights[j] = t * (1.0 - t).max(0.0).powi(17);
        wsum += weights[j];
    }
    for w in &mut weights {
        *w /= wsum;
    }

    // Per frame: voiced bin masses, refined cents per candidate, unvoiced mass.
    let mut obs_bins: Vec<Vec<(usize, f32, f32)>> = Vec::with_capacity(n_frames); // (bin, mass, cents)
    let mut obs_unv: Vec<f32> = Vec::with_capacity(n_frames);
    let mut minima: Vec<(usize, f32)> = Vec::new();
    let mut mass: Vec<f32> = Vec::new();
    for i in 0..n_frames {
        let centre = ((i as f64 * 0.01 * sample_rate as f64) as usize + delay) / factor;
        let start = centre as isize - (flen / 2) as isize;
        let mut energy = 0.0f32;
        for (j, b) in buf.iter_mut().enumerate() {
            let idx = start + j as isize;
            *b = if idx >= 0 && (idx as usize) < xd.len() { xd[idx as usize] } else { 0.0 };
        }
        for &v in &buf[..win] {
            energy += v * v;
        }
        if (energy / win as f32).sqrt() < SILENCE_RMS {
            obs_bins.push(Vec::new());
            obs_unv.push(1.0);
            continue;
        }
        cmnd(&buf, max_lag, &mut d);

        // Local minima of cmnd in range, in lag order.
        minima.clear();
        for tau in min_lag.max(2)..max_lag - 1 {
            if d[tau].is_finite() && d[tau] < d[tau - 1] && d[tau] <= d[tau + 1] && d[tau] < 1.0 {
                minima.push((tau, d[tau]));
            }
        }
        // For each threshold the first dip below it takes that threshold's weight.
        mass.clear();
        mass.resize(minima.len(), 0.0);
        let mut total = 0.0;
        for j in 0..N_THRESH {
            if let Some(m) = minima.iter().position(|&(_, v)| v < thresholds[j]) {
                mass[m] += weights[j];
                total += weights[j];
            }
        }
        let mut cands = Vec::new();
        for (m, &(tau, _)) in minima.iter().enumerate() {
            if mass[m] <= 0.0 {
                continue;
            }
            let lag = parabolic(&d, tau);
            if !(lag.is_finite() && lag > 0.0) {
                continue;
            }
            let hz = sr_d / lag;
            if !(FMIN..=FMAX).contains(&hz) {
                continue;
            }
            let cents = hz_to_cents(hz);
            let b = bin_of(cents);
            if b >= 0 && (b as usize) < n_bins {
                cands.push((b as usize, mass[m], cents));
            }
        }
        obs_bins.push(cands);
        obs_unv.push((1.0 - total).clamp(0.0, 1.0));
    }

    // Viterbi. States 0..n_bins are voiced pitch bins, n_bins is the single unvoiced state.
    let ln_floor = 1e-4f32.ln();
    let ln_stay = (1.0 - SWITCH_P).ln();
    let ln_sw = SWITCH_P.ln();
    let ln_enter = (SWITCH_P / n_bins as f32).ln();
    let w1 = (JUMP_BINS + 1) as f32;
    let tri: Vec<f32> = (0..=JUMP_BINS).map(|dlt| ((w1 - dlt as f32) / (w1 * w1)).ln() + ln_stay).collect();
    let ns = n_bins + 1;
    let mut score = vec![f32::NEG_INFINITY; ns];
    let mut next = vec![f32::NEG_INFINITY; ns];
    let mut back = vec![0u16; n_frames * ns];
    let mut em = vec![0.0f32; n_bins];

    for i in 0..n_frames {
        em.iter_mut().for_each(|e| *e = 0.0);
        for &(b, m, _) in &obs_bins[i] {
            em[b] += m;
            if b > 0 {
                em[b - 1] += 0.3 * m;
            }
            if b + 1 < n_bins {
                em[b + 1] += 0.3 * m;
            }
        }
        let unv_ln = obs_unv[i].max(1e-4).ln();
        if i == 0 {
            for b in 0..n_bins {
                score[b] = -(n_bins as f32).ln() + em[b].max(1e-4).ln();
            }
            score[n_bins] = unv_ln.max(ln_floor);
            continue;
        }
        let bk = &mut back[i * ns..(i + 1) * ns];
        for b in 0..n_bins {
            let lo = b.saturating_sub(JUMP_BINS);
            let hi = (b + JUMP_BINS).min(n_bins - 1);
            let mut best = score[n_bins] + ln_enter;
            let mut arg = n_bins;
            for p in lo..=hi {
                let v = score[p] + tri[p.abs_diff(b)];
                if v > best {
                    best = v;
                    arg = p;
                }
            }
            next[b] = best + em[b].max(1e-4).ln();
            bk[b] = arg as u16;
        }
        // Into unvoiced: from unvoiced (stay) or any voiced bin (switch).
        let mut best = score[n_bins] + ln_stay;
        let mut arg = n_bins;
        for p in 0..n_bins {
            let v = score[p] + ln_sw;
            if v > best {
                best = v;
                arg = p;
            }
        }
        next[n_bins] = best + unv_ln;
        bk[n_bins] = arg as u16;
        std::mem::swap(&mut score, &mut next);
    }

    let mut state = (0..ns).max_by(|&a, &b| score[a].total_cmp(&score[b])).unwrap_or(n_bins);
    let mut out = vec![f32::NAN; n_frames];
    for i in (0..n_frames).rev() {
        if state < n_bins {
            let centre_cents = c70 + state as f32 * BIN_CENTS;
            // Use the strongest candidate near the chosen bin for sub-bin accuracy.
            let mut best = (0.0f32, centre_cents);
            for &(b, m, c) in &obs_bins[i] {
                if b.abs_diff(state) <= 1 && m > best.0 {
                    best = (m, c);
                }
            }
            out[i] = best.1;
        }
        if i > 0 {
            state = back[i * ns + state] as usize;
        }
    }
    out
}

/// Replace `live[i]` with `second[i]` where they differ by more than
/// `REPAIR_CENTS` for at least `REPAIR_MIN_FRAMES` consecutive frames.
/// Returns the repaired curve and the number of frames replaced.
///
/// Only voiced disagreements count: a frame where either curve is NaN breaks
/// a run and is left alone, so repair never turns an unvoiced live frame into
/// a voiced one or the other way round. Frames past the end of the shorter
/// curve are left as in `live`.
pub fn repair(live: &[f32], second: &[f32]) -> (Vec<f32>, usize) {
    let mut out = live.to_vec();
    let n = live.len().min(second.len());
    let bad = |i: usize| {
        let (a, b) = (live[i], second[i]);
        a.is_finite() && b.is_finite() && (a - b).abs() > REPAIR_CENTS
    };
    let mut replaced = 0;
    let mut i = 0;
    while i < n {
        if !bad(i) {
            i += 1;
            continue;
        }
        let start = i;
        while i < n && bad(i) {
            i += 1;
        }
        if i - start >= REPAIR_MIN_FRAMES {
            out[start..i].copy_from_slice(&second[start..i]);
            replaced += i - start;
        }
    }
    (out, replaced)
}

pub const REPAIR_CENTS: f32 = 80.0;
pub const REPAIR_MIN_FRAMES: usize = 3;

#[cfg(test)]
mod tests {
    use super::*;

    fn glide(sr: f32, dur: f32, f_a: f32, f_b: f32) -> Vec<f32> {
        let n = (sr * dur) as usize;
        let mut phase = 0.0f64;
        (0..n)
            .map(|i| {
                let t = i as f32 / n as f32;
                let f = f_a * (f_b / f_a).powf(t);
                phase += 2.0 * std::f64::consts::PI * f as f64 / sr as f64;
                (0.5 * phase.sin()) as f32
            })
            .collect()
    }

    #[test]
    fn tracks_sine_glide_within_20_cents() {
        for sr in [48_000.0f32, 16_000.0] {
            let x = glide(sr, 1.0, 200.0, 400.0);
            let c = pyin(&x, sr);
            assert!((c.len() as i32 - 100).abs() <= 1, "{}", c.len());
            let mut worst = 0.0f32;
            for (i, &v) in c.iter().enumerate().skip(5).take(c.len() - 10) {
                assert!(v.is_finite(), "frame {i} unvoiced at {sr}");
                let t = i as f32 * 0.01;
                let want = hz_to_cents(200.0 * 2f32.powf(t));
                worst = worst.max((v - want).abs());
            }
            assert!(worst < 20.0, "worst error {worst} cents at {sr}");
        }
    }

    #[test]
    fn silence_and_noise_are_unvoiced_and_short_input_is_safe() {
        assert!(pyin(&[], 48_000.0).is_empty());
        assert!(pyin(&[0.1; 10], 48_000.0).is_empty());
        let c = pyin(&vec![0.0; 24_000], 48_000.0);
        assert_eq!(c.len(), 50);
        assert!(c.iter().all(|v| v.is_nan()));
        let mut s = 12345u32;
        let noise: Vec<f32> = (0..48_000)
            .map(|_| {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                (s >> 8) as f32 / (1u32 << 24) as f32 - 0.5
            })
            .collect();
        let c = pyin(&noise, 48_000.0);
        let voiced = c.iter().filter(|v| v.is_finite()).count();
        assert!(voiced * 5 < c.len(), "{voiced} voiced frames in noise");
    }

    #[test]
    fn repair_fixes_octave_error() {
        let second: Vec<f32> = (0..50).map(|i| 6000.0 + i as f32).collect();
        let mut live = second.clone();
        for v in &mut live[10..20] {
            *v += 1200.0;
        }
        let (fixed, n) = repair(&live, &second);
        assert_eq!(n, 10);
        assert_eq!(fixed, second);
    }

    #[test]
    fn repair_keeps_agreement_and_short_blips() {
        let second = vec![6000.0f32; 30];
        let mut live = second.clone();
        live[5] += 40.0; // below the threshold
        live[10] += 1200.0; // one frame
        live[20] += 900.0; // two frames
        live[21] += 900.0;
        let (fixed, n) = repair(&live, &second);
        assert_eq!(n, 0);
        assert_eq!(fixed, live);
    }

    #[test]
    fn repair_leaves_nan_frames_alone() {
        let second = vec![6000.0f32; 12];
        let mut live = second.clone();
        for v in &mut live[2..8] {
            *v = f32::NAN;
        }
        for v in &mut live[8..12] {
            *v += 1200.0;
        }
        let (fixed, n) = repair(&live, &second);
        assert_eq!(n, 4);
        assert!(fixed[2..8].iter().all(|v| v.is_nan()));
        assert_eq!(&fixed[8..], &second[8..]);
    }
}
