//! Influence instant match (TDD "Sound Influence" / "Instant match"), M0
//! subset: only the parameters the M0 engine has (sources and envelopes).

use dsp::fft::{Fft, hann};
use dsp::frame::MAX_BANDS;
use dsp::hz_to_cents;
use dsp::params::*;
use dsp::resample::{Decimator, decimation_factor};
use dsp::{bands, yin};

/// Floats in the flat result: params, tone bands, noise bands, then
/// pitch_cents, duration_s, voiced_fraction, pitched (0/1).
/// Mirrored as MATCH_* in web/src/host/protocol.ts.
pub const MATCH_LEN: usize = COUNT + 2 * MAX_BANDS + 4;

#[derive(Clone, Debug)]
pub struct MatchResult {
    /// Target parameters in real units. Gesture depths and tuning keep defaults.
    pub params: ParamSet,
    /// Pitch to audition the match at (median voiced pitch, cents above C0);
    /// for unpitched sources a neutral 4800 (C4).
    pub pitch_cents: f32,
    /// How long to hold the audition note (source sound length up to its offset).
    pub duration_s: f32,
    pub voiced_fraction: f32,
    pub pitched: bool,
    /// RMS of the residual (everything between the partials) over the RMS of
    /// the partials, in the frames near the peak. NaN for unpitched sources.
    /// `match_full` turns it into harm_mix and noise_mix for the M1 engine.
    pub noise_to_harm: f32,
    /// Median number of partials found per loud frame (0 when unpitched).
    pub partials: f32,
    /// Median over loud frames of the highest partial found, Hz (0 when unpitched).
    pub partial_top_hz: f32,
}

impl MatchResult {
    pub fn to_floats(&self, out: &mut [f32]) {
        out[..COUNT].copy_from_slice(&self.params.scalars);
        out[COUNT..COUNT + MAX_BANDS].copy_from_slice(&self.params.tone_bands);
        out[COUNT + MAX_BANDS..COUNT + 2 * MAX_BANDS].copy_from_slice(&self.params.noise_bands);
        let b = COUNT + 2 * MAX_BANDS;
        out[b] = self.pitch_cents;
        out[b + 1] = self.duration_s;
        out[b + 2] = self.voiced_fraction;
        out[b + 3] = self.pitched as u8 as f32;
    }
}


// ---------------------------------------------------------------------------
// How the match is built (M0 approximations, written down so the synth side
// can be checked against them):
//
// * Loudness is RMS per 10 ms block. The spectral work (4096-point Hann FFT)
//   runs on at most MAX_SPEC_FRAMES blocks spread evenly over the part of the
//   sound within 30 dB of its peak, not on every 10 ms hop, to keep ten
//   seconds of audio well under a second in wasm.
// * Pitch is YIN (dsp::yin) on a 16 kHz copy, one estimate per 10 ms block.
//   A block counts as voiced when confidence >= 0.7. voiced_fraction is voiced
//   blocks over blocks within 35 dB of the peak.
// * The synth's partial k sits at f0 * ratio_k * sqrt(1 + B k^2) with ratio
//   tables harmonic / drumhead / bar and `modal` interpolating between
//   adjacent tables. Tilt is measured against log2(ratio_k), not log2(k).
//   Beyond the tabulated values the drumhead table grows as sqrt(k) and the
//   bar table follows ((k + 0.5) pi)^2 / 22.373 (free-free bar).
// * harm_mix / noise_mix are the RMS amplitude ratio of harmonic peaks to the
//   residual, scaled so the larger is 1. The M0 synth's absolute partial and
//   noise levels are not calibrated against each other, so this is the part
//   most likely to need a fudge factor by ear.
// * tone_bands / noise_bands are shapes in dB with their mean removed, clamped
//   to +-40 dB. noise_bands is mean power per FFT bin in each band (the band
//   bank's sum of band-passes is roughly flat for white noise, so bin power
//   maps straight onto the synth's gains).
// * f1/f2 are modelled as Gaussian bells of 1/3 octave full width at half
//   maximum; they are subtracted from the tone_bands pooling so the same
//   resonance is not counted twice.
// * env.attack is the literal 10% to 90% rise time of the RMS curve.
// ---------------------------------------------------------------------------

const FFT_N: usize = 4096;
const HOP_S: f32 = 0.01;
const MAX_SPEC_FRAMES: usize = 120;
const MAX_PARTIALS: usize = 64;
pub(crate) const VOICED_CONF: f32 = 0.7;
const BELL_SIGMA_OCT: f32 = 0.1416; // 1/3 octave FWHM
const BAND_CLAMP_DB: f32 = 40.0;

fn default_result() -> MatchResult {
    MatchResult {
        params: ParamSet::default(),
        pitch_cents: 4800.0,
        duration_s: 0.2,
        voiced_fraction: 0.0,
        pitched: false,
        noise_to_harm: f32::NAN,
        partials: 0.0,
        partial_top_hz: 0.0,
    }
}

/// Measure `signal` and return the target parameter set.
pub fn instant_match(signal: &[f32], sample_rate: f32, n_bands: usize) -> MatchResult {
    instant_inner(signal, sample_rate, n_bands, false)
}

/// The measurement behind `instant_match` and `match_full`. `decaying` turns
/// the decay-slope row (damp) on for a sound that has a plateau, as when the
/// audio is one hit for a kit slot.
pub(crate) fn instant_inner(signal: &[f32], sample_rate: f32, n_bands: usize, decaying: bool) -> MatchResult {
    let mut res = default_result();
    if !(sample_rate.is_finite() && (4000.0..=400_000.0).contains(&sample_rate)) {
        return res;
    }
    let sr = sample_rate;
    let hop = ((sr * HOP_S).round() as usize).max(1);
    if signal.len() < hop * 5 {
        return res;
    }
    let n = n_bands.clamp(2, MAX_BANDS);
    let x: Vec<f32> = signal.iter().map(|&v| if v.is_finite() { v } else { 0.0 }).collect();
    let hop_s = hop as f32 / sr;
    let nf = x.len() / hop;

    // Loudness curve.
    let amp: Vec<f32> = (0..nf)
        .map(|i| {
            let b = &x[i * hop..(i + 1) * hop];
            (b.iter().map(|v| v * v).sum::<f32>() / hop as f32).sqrt()
        })
        .collect();
    let mut p = 0;
    for i in 0..nf {
        if amp[i] > amp[p] {
            p = i;
        }
    }
    let pk = amp[p];
    if !(pk > 1e-6) {
        return res;
    }
    let db: Vec<f32> = amp.iter().map(|&a| 20.0 * (a.max(1e-9) / pk).log10()).collect();
    let env = fit_envelope(&amp, &db, p, hop_s);

    // Pitch per block.
    let track = track_pitch(&x, sr, hop, &db);
    let considered = db.iter().filter(|&&d| d >= -35.0).count().max(1);
    let voiced: Vec<usize> = (0..nf).filter(|&i| track[i].0 > 0.0 && track[i].1 >= VOICED_CONF).collect();
    let vfrac = voiced.len() as f32 / considered as f32;
    let pitched = vfrac >= 0.3;
    res.voiced_fraction = vfrac.clamp(0.0, 1.0);
    res.pitched = pitched;
    let mut f0_global = 0.0;
    let mut cents_by_frame = vec![f32::NAN; nf];
    if pitched {
        let mut hz: Vec<f32> = voiced.iter().map(|&i| track[i].0).collect();
        hz.sort_by(|a, b| a.total_cmp(b));
        f0_global = hz[hz.len() / 2];
        for &i in &voiced {
            cents_by_frame[i] = hz_to_cents(track[i].0);
        }
        res.pitch_cents = hz_to_cents(f0_global);
    }

    // Spectral frames.
    let mut act: Vec<usize> = (env.onset..env.off.min(nf)).filter(|&i| db[i] >= -30.0).collect();
    if act.is_empty() {
        act.push(p);
    }
    let stride = act.len().div_ceil(MAX_SPEC_FRAMES).max(1);
    let mut idx: Vec<usize> = act.iter().copied().step_by(stride).collect();
    if !idx.contains(&p) {
        idx.push(p);
        idx.sort_unstable();
    }
    let fft = Fft::new(FFT_N);
    let mut win = vec![0.0f32; FFT_N];
    hann(&mut win);
    let nh = FFT_N / 2 + 1;
    let mut mags: Vec<Vec<f32>> = Vec::with_capacity(idx.len());
    let (mut re, mut im) = (vec![0.0f32; FFT_N], vec![0.0f32; FFT_N]);
    for &i in &idx {
        let c = (i * hop + hop / 2) as isize - (FFT_N / 2) as isize;
        for j in 0..FFT_N {
            let s = c + j as isize;
            re[j] = if s >= 0 && (s as usize) < x.len() { x[s as usize] * win[j] } else { 0.0 };
            im[j] = 0.0;
        }
        fft.forward(&mut re, &mut im);
        mags.push((0..nh).map(|j| re[j].hypot(im[j])).collect());
    }
    let bin_hz = sr / FFT_N as f32;

    // Bands.
    let mut centres = [0.0f32; MAX_BANDS];
    bands::band_centres(n, &mut centres);
    let centres = &centres[..n];
    let edges = band_edges(centres);
    let bin_lo = (edges[0] / bin_hz).ceil() as usize;
    let band_bins: Vec<(usize, usize)> = (0..n)
        .map(|b| {
            let lo = ((edges[b] / bin_hz).ceil() as usize).max(bin_lo).min(nh - 1);
            let hi = ((edges[b + 1] / bin_hz).ceil() as usize).min(nh - 1);
            (lo, hi.max(lo))
        })
        .collect();

    let tone_frame: Vec<bool> = idx.iter().map(|&i| db[i] >= -12.0).collect();
    let tone_frame = if tone_frame.iter().any(|&t| t) {
        tone_frame
    } else {
        idx.iter().map(|&i| i == p).collect()
    };

    // Harmonic structure of the sound: modal table and stretch.
    let (mut modal, mut stretch) = (0.0f32, 0.0f32);
    let mut tau1: Option<f32> = None;
    if pitched {
        let ref_f = idx.iter().enumerate().filter(|(fi, _)| tone_frame[*fi]).map(|(fi, _)| fi).max_by(|&a, &b| {
            let ea: f32 = mags[a].iter().map(|v| v * v).sum();
            let eb: f32 = mags[b].iter().map(|v| v * v).sum();
            ea.total_cmp(&eb)
        });
        if let Some(rf) = ref_f {
            let (m, b) = fit_modal(&mags[rf], sr, f0_global);
            modal = m;
            stretch = b;
        }
    }

    // Per-frame harmonic extraction and residual.
    let mut f0_fill = vec![f0_global; nf];
    if pitched {
        let mut last = 0.0;
        for i in 0..nf {
            if track[i].0 > 0.0 && track[i].1 >= VOICED_CONF {
                last = track[i].0;
            }
            if last > 0.0 {
                f0_fill[i] = last;
            }
        }
        // Frames before the first voiced one take the first voiced pitch.
        if let Some(&first) = voiced.first() {
            for i in 0..first {
                f0_fill[i] = track[first].0;
            }
        }
    }
    let mut mask = vec![false; nh];
    let mut frame_parts: Vec<Vec<Partial>> = Vec::with_capacity(idx.len());
    let mut resid_series: Vec<(f32, f32)> = Vec::new(); // (time s, residual power)
    let mut e_harm = 0.0f64;
    let mut e_resid = 0.0f64;
    let mut nb_sum = vec![0.0f64; n];
    let mut nb_cnt = vec![0usize; n];
    for (fi, mag) in mags.iter().enumerate() {
        let i = idx[fi];
        mask.iter_mut().for_each(|m| *m = false);
        let parts = if pitched { extract_partials(mag, sr, f0_fill[i], modal, stretch) } else { Vec::new() };
        for pt in &parts {
            for j in pt.bin.saturating_sub(2)..=(pt.bin + 2).min(nh - 1) {
                mask[j] = true;
            }
        }
        let mut r_pow = 0.0f64;
        for j in bin_lo..nh {
            if !mask[j] {
                r_pow += (mag[j] as f64) * (mag[j] as f64);
            }
        }
        resid_series.push(((i as f32 + 0.5) * hop_s, r_pow as f32));
        if tone_frame[fi] {
            e_resid += r_pow;
            e_harm += parts.iter().map(|pt| pt.energy as f64).sum::<f64>();
            for b in 0..n {
                let (lo, hi) = band_bins[b];
                for j in lo..hi.max(lo + 1).min(nh) {
                    if !mask[j] {
                        nb_sum[b] += (mag[j] as f64) * (mag[j] as f64);
                        nb_cnt[b] += 1;
                    }
                }
            }
        }
        frame_parts.push(parts);
    }

    // noise_bands: mean power per bin in dB, missing bands interpolated, mean removed.
    let mut nb: Vec<Option<f32>> = (0..n)
        .map(|b| {
            if nb_cnt[b] >= 2 && nb_sum[b] > 0.0 {
                Some(10.0 * ((nb_sum[b] / nb_cnt[b] as f64) as f32).max(1e-20).log10())
            } else {
                None
            }
        })
        .collect();
    let noise_shape = finish_shape(&mut nb);

    let ps = &mut res.params;
    for (k, v) in noise_shape.iter().enumerate() {
        ps.noise_bands[k] = *v;
    }

    if !pitched {
        ps.scalars[HARM_MIX] = clamp(HARM_MIX, 0.1);
        ps.scalars[NOISE_MIX] = clamp(NOISE_MIX, 1.0);
    } else {
        // Mix.
        let ratio = if e_harm > 0.0 { (e_resid / e_harm).sqrt() as f32 } else { 1e3 };
        let (h, nz) = if ratio < 1.0 { (1.0, ratio) } else { (1.0 / ratio, 1.0) };
        ps.scalars[HARM_MIX] = clamp(HARM_MIX, h);
        ps.scalars[NOISE_MIX] = clamp(NOISE_MIX, nz);
        res.noise_to_harm = ratio;
        let mut counts: Vec<usize> = frame_parts.iter().enumerate().filter(|(fi, _)| tone_frame[*fi]).map(|(_, q)| q.len()).collect();
        counts.sort_unstable();
        res.partials = counts.get(counts.len() / 2).copied().unwrap_or(0) as f32;
        let mut tops: Vec<f32> = frame_parts.iter().enumerate().filter(|(fi, q)| tone_frame[*fi] && !q.is_empty()).map(|(_, q)| q.iter().map(|x| x.f).fold(0.0f32, f32::max)).collect();
        tops.sort_by(|a, b| a.total_cmp(b));
        res.partial_top_hz = tops.get(tops.len() / 2).copied().unwrap_or(0.0);
        let ps = &mut res.params;
        ps.scalars[MODAL] = clamp(MODAL, modal);

        // Stretch: harmonic sounds get a per-frame fit of f_k^2/k^2 against k^2;
        // modal sounds keep the grid value from the table fit.
        if modal < 0.25 {
            let mut num = 0.0f32;
            let mut den = 0.0f32;
            for (fi, parts) in frame_parts.iter().enumerate() {
                if !tone_frame[fi] || parts.len() < 4 {
                    continue;
                }
                let (a1, b1) = line_fit(parts.iter().map(|q| ((q.k * q.k) as f32, (q.f / q.k as f32).powi(2))));
                if a1 > 0.0 {
                    num += (b1 / a1) * parts.len() as f32;
                    den += parts.len() as f32;
                }
            }
            if den > 0.0 {
                stretch = (num / den).clamp(0.0, 0.01);
            } else {
                stretch = 0.0;
            }
        }
        ps.scalars[STRETCH] = clamp(STRETCH, stretch);

        // Tilt and odd/even by least squares with a free level per frame.
        let (mut sxx, mut sxs, mut sss, mut sxl, mut ssl) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
        let (mut odd_n, mut even_n, mut odd_slots, mut even_slots) = (0usize, 0usize, 0usize, 0usize);
        for (fi, parts) in frame_parts.iter().enumerate() {
            if !tone_frame[fi] || parts.is_empty() {
                continue;
            }
            let kmax = parts.iter().map(|q| q.k).max().unwrap_or(0);
            odd_slots += kmax.div_ceil(2);
            even_slots += kmax / 2;
            for q in parts {
                if q.k % 2 == 1 {
                    odd_n += 1
                } else {
                    even_n += 1
                }
            }
            if parts.len() < 2 {
                continue;
            }
            let m = parts.len() as f32;
            let xs: Vec<f32> = parts.iter().map(|q| ratio_k(modal, q.k).log2()).collect();
            let ss: Vec<f32> = parts.iter().map(|q| if q.k % 2 == 1 { 1.0 } else { -1.0 }).collect();
            let mx = xs.iter().sum::<f32>() / m;
            let ms = ss.iter().sum::<f32>() / m;
            let ml = parts.iter().map(|q| q.db).sum::<f32>() / m;
            for (j, q) in parts.iter().enumerate() {
                let (a, s, l) = ((xs[j] - mx) as f64, (ss[j] - ms) as f64, (q.db - ml) as f64);
                sxx += a * a;
                sxs += a * s;
                sss += s * s;
                sxl += a * l;
                ssl += s * l;
            }
        }
        let odd_rate = odd_n as f32 / odd_slots.max(1) as f32;
        let even_rate = even_n as f32 / even_slots.max(1) as f32;
        let (tilt, c_oe);
        let det = sxx * sss - sxs * sxs;
        if odd_rate >= 0.6 && even_rate <= 0.2 {
            tilt = if sxx > 1e-9 { (sxl / sxx) as f32 } else { -6.0 };
            c_oe = 6.0;
        } else if even_rate >= 0.6 && odd_rate <= 0.2 {
            tilt = if sxx > 1e-9 { (sxl / sxx) as f32 } else { -6.0 };
            c_oe = -6.0;
        } else if det > 1e-6 * (sxx * sss).max(1e-12) {
            tilt = ((sxl * sss - ssl * sxs) / det) as f32;
            c_oe = (((ssl * sxx - sxl * sxs) / det) as f32).clamp(-6.0, 6.0);
        } else {
            tilt = if sxx > 1e-9 { (sxl / sxx) as f32 } else { -6.0 };
            c_oe = 0.0;
        }
        ps.scalars[TILT] = clamp(TILT, tilt);
        ps.scalars[ODDEVEN] = clamp(ODDEVEN, c_oe / 6.0);
        let (tilt, c_oe) = (ps.scalars[TILT], ps.scalars[ODDEVEN] * 6.0);

        // Tilt- and parity-removed harmonic levels, one level per frame removed.
        let mut samples: Vec<(f32, f32)> = Vec::new(); // (freq, dB)
        for (fi, parts) in frame_parts.iter().enumerate() {
            if !tone_frame[fi] || parts.is_empty() {
                continue;
            }
            let adj: Vec<f32> = parts
                .iter()
                .map(|q| q.db - tilt * ratio_k(modal, q.k).log2() - if q.k % 2 == 1 { c_oe } else { -c_oe })
                .collect();
            let mean = adj.iter().sum::<f32>() / adj.len() as f32;
            for (q, a) in parts.iter().zip(adj) {
                samples.push((q.f, a - mean));
            }
        }

        // Formants from the smoothed envelope between 200 Hz and 3.5 kHz.
        let formants = find_formants(&samples);
        let mut f_slots = [(0.0f32, 0.0f32); 2]; // (freq, gain), gain 0 = unused
        match formants.len() {
            0 => {}
            1 => {
                let (f, g) = formants[0];
                if f <= 900.0 {
                    f_slots[0] = (f, g)
                } else {
                    f_slots[1] = (f, g)
                }
            }
            _ => {
                let (a, b) = (formants[0], formants[1]);
                let (lo, hi) = if a.0 <= b.0 { (a, b) } else { (b, a) };
                if lo.0 > 1200.0 {
                    f_slots[1] = if lo.1 >= hi.1 { lo } else { hi };
                } else {
                    f_slots[0] = lo;
                    f_slots[1] = hi;
                }
            }
        }
        if f_slots[0].1 > 0.0 {
            ps.scalars[F1_FREQ] = clamp(F1_FREQ, f_slots[0].0);
            ps.scalars[F1_GAIN] = clamp(F1_GAIN, f_slots[0].1);
        }
        if f_slots[1].1 > 0.0 {
            ps.scalars[F2_FREQ] = clamp(F2_FREQ, f_slots[1].0);
            ps.scalars[F2_GAIN] = clamp(F2_GAIN, f_slots[1].1);
        }
        let bells: Vec<(f32, f32)> = f_slots.iter().filter(|s| s.1 > 0.0).copied().collect();

        // tone_bands: pooled mean of the leftover levels.
        let mut tsum = vec![0.0f32; n];
        let mut tcnt = vec![0usize; n];
        for &(f, l) in &samples {
            let mut v = l;
            for &(bf, bg) in &bells {
                let o = (f / bf).log2() / BELL_SIGMA_OCT;
                v -= bg * (-0.5 * o * o).exp();
            }
            let b = edges[1..n].partition_point(|&e| e <= f);
            tsum[b] += v;
            tcnt[b] += 1;
        }
        let mut tb: Vec<Option<f32>> = (0..n).map(|b| if tcnt[b] > 0 { Some(tsum[b] / tcnt[b] as f32) } else { None }).collect();
        let tone_shape = finish_shape(&mut tb);
        for (k, v) in tone_shape.iter().enumerate() {
            ps.tone_bands[k] = *v;
        }

        // damp from how fast each partial dies against its ratio.
        if env.sustain < 0.3 || decaying {
            let mut series: Vec<Vec<(f32, f32)>> = vec![Vec::new(); MAX_PARTIALS + 1];
            for (fi, parts) in frame_parts.iter().enumerate() {
                let t = (idx[fi] as f32 + 0.5) * hop_s;
                if idx[fi] < p {
                    continue;
                }
                for q in parts {
                    series[q.k].push((t, q.db));
                }
            }
            let mut pts: Vec<(f32, f32)> = Vec::new(); // (ln ratio, ln tau)
            for k in 1..=MAX_PARTIALS {
                if series[k].len() < 5 {
                    continue;
                }
                let (_, slope) = line_fit(series[k].iter().copied());
                if slope < -2.0 {
                    let tau = -8.686 / slope;
                    pts.push((ratio_k(modal, k).ln(), tau.ln()));
                }
            }
            if pts.len() >= 3 {
                let spread = pts.iter().map(|q| q.0).fold(f32::MIN, f32::max) - pts.iter().map(|q| q.0).fold(f32::MAX, f32::min);
                if spread > 0.5 {
                    let (icpt, slope) = line_fit(pts.iter().copied());
                    ps.scalars[DAMP] = clamp(DAMP, -slope / 2.0);
                    // The Klont's partial k decays with decay * ratio^(-2 damp),
                    // so env.decay is the fundamental's own time constant (the
                    // fit's value at ratio 1), not that of the summed loudness.
                    tau1 = Some(icpt.exp());
                }
            }
        }

        // Pitch drop over the first 150 ms.
        let settle = {
            let late: Vec<f32> = (env.onset + 15..nf).filter_map(|i| if cents_by_frame[i].is_finite() { Some(cents_by_frame[i]) } else { None }).collect();
            if late.len() >= 5 {
                let mut l = late;
                l.sort_by(|a, b| a.total_cmp(b));
                l[l.len() / 2]
            } else {
                res.pitch_cents
            }
        };
        if let Some((amt, time)) = pitch_drop(&cents_by_frame, env.onset, settle, hop_s) {
            res.params.scalars[ENV_PITCH_AMT] = clamp(ENV_PITCH_AMT, amt);
            res.params.scalars[ENV_PITCH_TIME] = clamp(ENV_PITCH_TIME, time);
            res.pitch_cents = settle;
        }
    }

    // Envelope parameters.
    let ps = &mut res.params;
    ps.scalars[ENV_ATTACK] = clamp(ENV_ATTACK, env.attack);
    ps.scalars[ENV_HOLD] = clamp(ENV_HOLD, env.hold);
    ps.scalars[ENV_DECAY] = clamp(ENV_DECAY, tau1.unwrap_or(env.decay));
    ps.scalars[ENV_SUSTAIN] = clamp(ENV_SUSTAIN, env.sustain);
    ps.scalars[ENV_RELEASE] = clamp(ENV_RELEASE, env.release);
    ps.scalars[ENV_NOISE_DECAY] = clamp(ENV_NOISE_DECAY, noise_decay(&resid_series, p, hop_s));
    res.duration_s = env.duration.clamp(0.2, 4.0);
    sanitize(&mut res);
    res
}

fn sanitize(r: &mut MatchResult) {
    for (i, v) in r.params.scalars.iter_mut().enumerate() {
        if !v.is_finite() {
            *v = PARAMS[i].default;
        }
        *v = clamp(i, *v);
    }
    for v in r.params.tone_bands.iter_mut().chain(r.params.noise_bands.iter_mut()) {
        *v = if v.is_finite() { v.clamp(-BAND_CLAMP_DB, BAND_CLAMP_DB) } else { 0.0 };
    }
    if !r.pitch_cents.is_finite() {
        r.pitch_cents = 4800.0;
    }
    if !r.duration_s.is_finite() {
        r.duration_s = 0.2;
    }
    if !r.voiced_fraction.is_finite() {
        r.voiced_fraction = 0.0;
    }
}

// ---- envelope ----

struct EnvFit {
    attack: f32,
    hold: f32,
    decay: f32,
    sustain: f32,
    release: f32,
    onset: usize,
    off: usize,
    duration: f32,
}

/// Time in blocks (block centres) where `amp` first reaches `thr`, interpolated.
fn rise_cross(amp: &[f32], thr: f32) -> f32 {
    for i in 0..amp.len() {
        if amp[i] >= thr {
            if i == 0 {
                return 0.5;
            }
            let (a, b) = (amp[i - 1], amp[i]);
            let f = if b > a { (thr - a) / (b - a) } else { 1.0 };
            return i as f32 - 0.5 + f;
        }
    }
    amp.len() as f32
}

fn fit_envelope(amp: &[f32], db: &[f32], p: usize, hop_s: f32) -> EnvFit {
    let nf = amp.len();
    let pk = amp[p];
    let onset = db.iter().position(|&d| d >= -30.0).unwrap_or(0).min(p);
    let off = (p + 1..nf).find(|&i| db[i] < -30.0).unwrap_or(nf);
    let duration = (off - onset) as f32 * hop_s;

    // The attack ends where the loudness first reaches 90% of its maximum. A
    // sustained sound that ripples (tremolo, vibrato) can have its maximum
    // anywhere, so the decay fit starts there and not at the maximum.
    let pe = (0..=p).find(|&i| amp[i] >= 0.9 * pk).unwrap_or(p);
    let attack = ((rise_cross(&amp[..=pe], 0.9 * pk) - rise_cross(&amp[..=pe], 0.1 * pk)) * hop_s).max(0.002);

    // Decay to a plateau over the sounding part, fitted on the amplitude.
    let pk_fit = amp[pe].max(1e-9);
    let norm = |range: std::ops::Range<usize>| -> Vec<f32> { amp[range].iter().map(|a| a / pk_fit).collect() };
    let mut region_end = off.max(pe + 1).min(nf);
    // A plateau followed by a release: the decay-to-plateau fit must not see
    // the release, or it reads the whole thing as one long decay.
    if let Some(b) = plateau_break(db, pe) {
        region_end = region_end.min(b + 1).max(pe + 1);
    }
    let mut start = pe;
    let mut a = norm(start..region_end);
    let (mut s, mut tau) = fit_decay(&a, hop_s);
    let mut hold = 0.0;
    if s < 0.9 {
        let flat = (p..nf).take_while(|&i| db[i] >= -0.3).count();
        if flat >= 10 {
            hold = (flat - 1) as f32 * hop_s;
            start = p + flat - 1;
            a = norm(start..region_end);
            (s, tau) = fit_decay(&a, hop_s);
        }
    }
    let (sustain, decay) = if s >= 0.9 {
        (s, 0.3)
    } else if s < 0.05 {
        (0.0, tau)
    } else {
        (s, tau)
    };
    let plateau = sustain >= 0.3;

    let release = if plateau {
        // Loud part ends at the last block within 6 dB of the peak.
        // (Within 3 dB of the plateau level, which is under the peak when
        // the sound decays to its plateau, and -40 dB is taken from there.)
        let plat_db = 20.0 * sustain.max(1e-3).log10();
        let r0 = (p..nf).rev().find(|&i| db[i] >= plat_db - 3.0).unwrap_or(p);
        if r0 + 1 >= nf {
            0.15
        } else {
            let t_end = match (r0..nf).find(|&i| db[i] <= db[r0] - 40.0) {
                Some(j) => j as f32,
                None => {
                    // Not reached before the signal ends: extrapolate the slope so far.
                    let drop = (-db[nf - 1] + db[r0]).max(1.0);
                    r0 as f32 + (nf - 1 - r0) as f32 * 40.0 / drop
                }
            };
            (t_end - r0 as f32) * hop_s
        }
    } else {
        // No plateau: time from the 30 dB offset to -40 dB.
        match (off.min(nf - 1)..nf).find(|&i| db[i] <= -40.0) {
            Some(j) => (j as f32 - off as f32) * hop_s,
            None => 1.15 * tau,
        }
    };
    EnvFit { attack, hold, decay, sustain, release: release.max(0.005), onset, off, duration }
}

/// Where a plateau ends (the frame the release starts at): the end of the
/// longest stretch after the peak where the loudness, smoothed over 250 ms,
/// moves under 4 dB per second and stays within 12 dB of the peak. None when
/// there is no such stretch of at least 150 ms.
fn plateau_break(db: &[f32], p: usize) -> Option<usize> {
    let nf = db.len();
    if nf < p + 40 {
        return None;
    }
    let sm: Vec<f32> = (0..nf)
        .map(|i| {
            let (a, b) = (i.saturating_sub(12), (i + 13).min(nf));
            db[a..b].iter().sum::<f32>() / (b - a) as f32
        })
        .collect();
    let flat = |i: usize| i >= p && i >= 10 && i + 10 < nf && sm[i] > -12.0 && ((sm[i + 10] - sm[i - 10]) / 0.2).abs() < 4.0;
    let mut best: Option<(usize, usize)> = None;
    let mut i = p;
    while i < nf {
        if flat(i) {
            let s = i;
            while i < nf && flat(i) {
                i += 1;
            }
            if i - s >= 15 && best.is_none_or(|b| i - s > b.1 - b.0) {
                best = Some((s, i));
            }
        } else {
            i += 1;
        }
    }
    // The windowed slope turns over about ten frames before the real break.
    best.map(|(_, e)| (e + 10).min(nf - 1))
}

/// Fit s + (1 - s) exp(-t / tau) to amplitudes (peak = 1 at index 0), L1 error.
fn fit_decay(a: &[f32], hop_s: f32) -> (f32, f32) {
    if a.len() < 3 {
        return (0.0, 0.3);
    }
    let l1 = |s: f32, tau: f32| -> f32 {
        let mut e = 0.0;
        for (j, &v) in a.iter().enumerate() {
            e += (v - (s + (1.0 - s) * (-(j as f32) * hop_s / tau).exp())).abs();
        }
        e
    };
    let mut best = (f32::MAX, 0.0f32, 0.3f32);
    for si in 0..=20 {
        let s = si as f32 * 0.05;
        for ti in 0..48 {
            let tau = 0.01 * (400.0f32).powf(ti as f32 / 47.0);
            let e = l1(s, tau);
            if e < best.0 {
                best = (e, s, tau);
            }
        }
    }
    // Refine tau and s around the grid winner.
    let (mut e0, mut s0, mut t0) = best;
    for step in 0..2 {
        let sf: f32 = if step == 0 { 1.14 } else { 1.04 };
        let sd = if step == 0 { 0.05 } else { 0.02 };
        for ti in -4i32..=4 {
            for si in -2i32..=2 {
                let (s, tau) = ((s0 + si as f32 * sd / 2.0).clamp(0.0, 1.0), t0 * sf.powf(ti as f32 / 4.0 * 2.0));
                let e = l1(s, tau);
                if e < e0 {
                    (e0, s0, t0) = (e, s, tau);
                }
            }
        }
    }
    (s0, t0.clamp(0.01, 4.0))
}

/// Decay time constant of the residual alone, from the slope of its dB curve
/// between its peak and 30 dB below. 4 s (the maximum) when it does not fall.
fn noise_decay(series: &[(f32, f32)], p: usize, hop_s: f32) -> f32 {
    if series.len() < 4 {
        return 4.0;
    }
    let _ = (p, hop_s);
    let dbv: Vec<(f32, f32)> = series.iter().map(|&(t, e)| (t, 10.0 * e.max(1e-20).log10())).collect();
    let mut ip = 0;
    for i in 0..dbv.len() {
        if dbv[i].1 > dbv[ip].1 {
            ip = i;
        }
    }
    let pts: Vec<(f32, f32)> = dbv[ip..].iter().copied().take_while(|q| q.1 >= dbv[ip].1 - 30.0).collect();
    if pts.len() < 4 {
        return 4.0;
    }
    let (_, slope) = line_fit(pts.iter().copied());
    if slope > -1.0 { 4.0 } else { (-8.686 / slope).clamp(0.005, 4.0) }
}

// ---- pitch ----

/// (hz, confidence) per block; (0, 0) for blocks skipped as too quiet.
pub(crate) fn track_pitch(x: &[f32], sr: f32, hop: usize, db: &[f32]) -> Vec<(f32, f32)> {
    let nf = db.len();
    let factor = decimation_factor(sr);
    let mut dec = Decimator::new(factor);
    let delay = dec.delay_samples();
    let xd: Vec<f32> = x.iter().filter_map(|&v| dec.process(v)).collect();
    let sr_d = sr / factor as f32;
    let w = ((0.024 * sr_d).round() as usize).max(32);
    let l = (sr_d / 70.0).ceil() as usize + 2;
    let flen = w + l;
    let mut buf = vec![0.0f32; flen];
    let mut scratch = vec![0.0f32; l];
    let mut out = vec![(0.0f32, 0.0f32); nf];
    for i in 0..nf {
        if db[i] < -35.0 {
            continue;
        }
        let c = ((i * hop + hop / 2 + delay) / factor) as isize;
        let start = c - (flen / 2) as isize;
        for (j, b) in buf.iter_mut().enumerate() {
            let s = start + j as isize;
            *b = if s >= 0 && (s as usize) < xd.len() { xd[s as usize] } else { 0.0 };
        }
        let e = yin::yin(&buf, sr_d, 70.0, 1000.0, 0.15, &mut scratch);
        if e.hz.is_finite() && e.confidence.is_finite() {
            out[i] = (e.hz, e.confidence);
        }
    }
    out
}

/// Exponential fit of pitch over the 150 ms after the onset: returns
/// (semitones at the start relative to the settled pitch, time constant s).
fn pitch_drop(cents: &[f32], onset: usize, settle: f32, hop_s: f32) -> Option<(f32, f32)> {
    let end = (onset + 15).min(cents.len());
    let pts: Vec<(usize, f32)> = (onset..end).filter(|&i| cents[i].is_finite()).map(|i| (i, cents[i] - settle)).collect();
    let first = pts.first()?;
    if pts.len() < 4 || first.1.abs() < 80.0 {
        return None;
    }
    let sign = first.1.signum();
    let fit: Vec<(f32, f32)> = pts
        .iter()
        .filter(|q| q.1 * sign > 15.0)
        .map(|q| ((q.0 - onset) as f32 * hop_s, (q.1 * sign).ln()))
        .collect();
    if fit.len() < 3 {
        return None;
    }
    let (icpt, slope) = line_fit(fit.iter().copied());
    if !(slope < -1.0) {
        return None;
    }
    let amt = sign * icpt.exp() / 100.0;
    Some((amt.clamp(-24.0, 48.0), (-1.0 / slope).clamp(0.001, 0.5)))
}

// ---- partials ----

fn drum_ratio(k: usize) -> f32 {
    const T: [f32; 16] =
        [1.0, 1.593, 2.135, 2.295, 2.653, 2.917, 3.155, 3.500, 3.598, 3.652, 4.060, 4.154, 4.231, 4.601, 4.612, 4.903];
    if k <= 16 { T[k - 1] } else { 4.903 * (k as f32 / 16.0).sqrt() }
}

fn bar_ratio(k: usize) -> f32 {
    if k == 1 {
        1.0
    } else {
        let b = (k as f32 + 0.5) * std::f32::consts::PI;
        b * b / 22.373
    }
}

fn table_ratio(t: usize, k: usize) -> f32 {
    match t {
        0 => k as f32,
        1 => drum_ratio(k),
        _ => bar_ratio(k),
    }
}

/// Ratio of partial k (k >= 1) for modal position m in 0..=2.
fn ratio_k(m: f32, k: usize) -> f32 {
    let m = m.clamp(0.0, 2.0);
    let a = (m.floor() as usize).min(1);
    let f = m - a as f32;
    (1.0 - f) * table_ratio(a, k) + f * table_ratio(a + 1, k)
}

struct Partial {
    k: usize,
    f: f32,
    db: f32,
    energy: f32,
    bin: usize,
}

/// Peak position (bins) and amplitude from log-parabolic interpolation around bin j.
fn parab_peak(mag: &[f32], j: usize) -> (f32, f32) {
    let a = mag[j - 1].max(1e-12).ln();
    let b = mag[j].max(1e-12).ln();
    let c = mag[j + 1].max(1e-12).ln();
    let den = a - 2.0 * b + c;
    let d = if den.abs() < 1e-9 { 0.0 } else { (0.5 * (a - c) / den).clamp(-0.5, 0.5) };
    (j as f32 + d, (b - 0.25 * (a - c) * d).exp())
}

/// Find partial k of f0 * ratio_k(m) * sqrt(1 + B k^2) in a magnitude spectrum.
/// The search window follows the last peak found (so stretch is tracked) and a
/// partial counts only if it is a local maximum with a clear valley beside it.
fn extract_partials(mag: &[f32], sr: f32, f0: f32, m: f32, stretch: f32) -> Vec<Partial> {
    let mut out = Vec::new();
    if !(f0 > 20.0) {
        return out;
    }
    let nh = mag.len();
    let bin_hz = sr / FFT_N as f32;
    let max_amp = mag.iter().copied().fold(0.0f32, f32::max);
    if !(max_amp > 0.0) {
        return out;
    }
    let floor_amp = max_amp * 10f32.powf(-50.0 / 20.0);
    let base = |k: usize| f0 * ratio_k(m, k) * (1.0 + stretch * (k * k) as f32).sqrt();
    let limit = 0.45 * sr;
    let mut last: Option<(f32, f32)> = None; // (base, measured)
    let mut g = 1.0f32;
    let mut missing = 0;
    let mut last_bin = 0usize;
    for k in 1..=MAX_PARTIALS {
        let bk = base(k);
        if bk > limit {
            break;
        }
        let b_prev = if k == 1 { 0.0 } else { base(k - 1) };
        let spc = (bk - b_prev).min(base(k + 1) - bk).max(1.5 * bin_hz);
        let pred = match last {
            Some((bl, fl)) => fl + (bk - bl) * g,
            None => bk,
        };
        if pred > limit {
            break;
        }
        let hw = (0.4 * spc * g).max(1.5 * bin_hz);
        let lo = (((pred - hw) / bin_hz).floor().max(2.0)) as usize;
        let hi = (((pred + hw) / bin_hz).ceil() as usize).min(nh - 3);
        let mut found = false;
        if lo <= hi {
            let mut j = lo;
            for q in lo..=hi {
                if mag[q] > mag[j] {
                    j = q;
                }
            }
            if mag[j] >= mag[j - 1] && mag[j] >= mag[j + 1] && j.abs_diff(last_bin) > 2 {
                let (pos, amp) = parab_peak(mag, j);
                let mut ok = amp >= floor_amp;
                let hs = (0.5 * spc / bin_hz).floor() as usize;
                if ok && hs >= 4 {
                    let lmin = (j.saturating_sub(hs)..=j - 3).map(|q| mag[q]).fold(f32::MAX, f32::min);
                    let rmin = (j + 3..=(j + hs).min(nh - 1)).map(|q| mag[q]).fold(f32::MAX, f32::min);
                    let valley = lmin.max(rmin);
                    ok = mag[j] >= 5.0 * valley;
                }
                if ok {
                    let f = pos * bin_hz;
                    let energy: f32 = (j.saturating_sub(2)..=(j + 2).min(nh - 1)).map(|q| mag[q] * mag[q]).sum();
                    out.push(Partial { k, f, db: 20.0 * amp.log10(), energy, bin: j });
                    if let Some((bp, fp)) = last {
                        if bk - bp > 1e-3 {
                            g = ((f - fp) / (bk - bp)).clamp(0.7, 2.0);
                        }
                    }
                    last = Some((bk, f));
                    last_bin = j;
                    found = true;
                }
            }
        }
        if found {
            missing = 0;
        } else {
            missing += 1;
            if missing >= 8 {
                break;
            }
        }
    }
    out
}

/// Strongest spectral peaks (frequency, amplitude), at most `max_n`, within 28 dB of the top.
fn generic_peaks(mag: &[f32], bin_hz: f32, max_n: usize) -> Vec<(f32, f32)> {
    let nh = mag.len();
    let top = mag.iter().copied().fold(0.0f32, f32::max);
    let thr = top * 10f32.powf(-28.0 / 20.0);
    let lo = ((40.0 / bin_hz).ceil() as usize).max(2);
    let mut cand: Vec<usize> = (lo..nh - 2).filter(|&j| mag[j] >= thr && mag[j] > mag[j - 1] && mag[j] >= mag[j + 1]).collect();
    cand.sort_by(|&a, &b| mag[b].total_cmp(&mag[a]));
    let mut out: Vec<usize> = Vec::new();
    for j in cand {
        if out.iter().all(|&o| o.abs_diff(j) >= 3) {
            out.push(j);
            if out.len() == max_n {
                break;
            }
        }
    }
    out.iter()
        .map(|&j| {
            let (pos, amp) = parab_peak(mag, j);
            (pos * bin_hz, amp)
        })
        .collect()
}

/// Choose the modal table position and stretch that best explain the strongest
/// peaks of one spectrum, by grid search over (m, B).
fn fit_modal(mag: &[f32], sr: f32, f0: f32) -> (f32, f32) {
    let bin_hz = sr / FFT_N as f32;
    let peaks = generic_peaks(mag, bin_hz, 10);
    if peaks.len() < 3 || !(f0 > 0.0) {
        return (0.0, 0.0);
    }
    let f_ref = peaks.iter().map(|p| p.0).filter(|&f| (f / f0 - 1.0).abs() < 0.06).min_by(|a, b| (a / f0 - 1.0).abs().total_cmp(&(b / f0 - 1.0).abs())).unwrap_or(f0);
    let pr: Vec<(f32, f32)> = peaks.iter().map(|p| (p.0 / f_ref, p.1)).filter(|p| p.0 >= 0.8 && p.0 <= 14.0).collect();
    if pr.len() < 3 {
        return (0.0, 0.0);
    }
    let wsum: f32 = pr.iter().map(|p| p.1).sum();
    const BS: [f32; 8] = [0.0, 0.0005, 0.001, 0.002, 0.004, 0.006, 0.008, 0.01];
    let mut best = (f32::MAX, 0.0f32, 0.0f32);
    for mi in 0..=8 {
        let m = mi as f32 * 0.25;
        for &bs in &BS {
            let pred: Vec<f32> = (1..=16).map(|k| ratio_k(m, k) * (1.0 + bs * (k * k) as f32).sqrt()).collect();
            let mut cost = 0.0;
            for &(r, w) in &pr {
                let e = pred.iter().map(|&pv| (r / pv).ln().abs()).fold(f32::MAX, f32::min);
                cost += w * e.min(0.3);
            }
            cost = cost / wsum + 0.5 * bs;
            // Prefer the simpler table on a near tie.
            cost += 1e-4 * m;
            if cost < best.0 {
                best = (cost, m, bs);
            }
        }
    }
    (best.1, best.2)
}

// ---- formants, shapes, small numerics ----

/// Up to two (freq, gain dB) resonances from tilt-removed harmonic levels.
fn find_formants(samples: &[(f32, f32)]) -> Vec<(f32, f32)> {
    const SIGMA: f32 = 0.15; // octaves
    // Smoothing flattens a bell of sigma BELL_SIGMA_OCT by this factor; undo it.
    let boost = (BELL_SIGMA_OCT * BELL_SIGMA_OCT + SIGMA * SIGMA).sqrt() / BELL_SIGMA_OCT;
    let steps = (((3500.0f32 / 200.0).log2()) * 12.0).round() as usize + 1;
    let mut grid: Vec<Option<(f32, f32)>> = Vec::with_capacity(steps); // (freq, value)
    for g in 0..steps {
        let f = 200.0 * 2f32.powf(g as f32 / 12.0);
        let (mut sw, mut sv) = (0.0f32, 0.0f32);
        for &(sf, l) in samples {
            if sf <= 0.0 {
                continue;
            }
            let o = (sf / f).log2() / SIGMA;
            if o.abs() > 3.0 {
                continue;
            }
            let w = (-0.5 * o * o).exp();
            sw += w;
            sv += w * l;
        }
        grid.push(if sw >= 0.5 { Some((f, sv / sw)) } else { None });
    }
    let mut vals: Vec<f32> = grid.iter().flatten().map(|q| q.1).collect();
    if vals.len() < 6 {
        return Vec::new();
    }
    vals.sort_by(|a, b| a.total_cmp(b));
    let base = vals[vals.len() / 2];
    let mut peaks: Vec<(f32, f32)> = Vec::new();
    for g in 1..steps - 1 {
        if let (Some(a), Some(b), Some(c)) = (grid[g - 1], grid[g], grid[g + 1]) {
            if b.1 >= a.1 && b.1 > c.1 {
                let gain = (b.1 - base) * boost;
                if gain >= 3.0 {
                    peaks.push((b.0, gain));
                }
            }
        }
    }
    peaks.sort_by(|a, b| b.1.total_cmp(&a.1));
    // Two resonances closer than half an octave are one.
    let mut out: Vec<(f32, f32)> = Vec::new();
    for pk in peaks {
        if out.iter().all(|o| (o.0 / pk.0).log2().abs() > 0.5) {
            out.push(pk);
            if out.len() == 2 {
                break;
            }
        }
    }
    out
}

pub(crate) fn band_edges(c: &[f32]) -> Vec<f32> {
    let n = c.len();
    let r = (c[n - 1] / c[0]).powf(1.0 / (n - 1) as f32).max(1.01);
    let sr = r.sqrt();
    let mut e = Vec::with_capacity(n + 1);
    e.push(c[0] / sr);
    for i in 1..n {
        e.push((c[i - 1] * c[i]).sqrt());
    }
    e.push(c[n - 1] * sr);
    e
}

/// Fill gaps by linear interpolation over band index (ends hold), remove the
/// mean, clamp. Returns zeros if no band has data.
fn finish_shape(v: &mut [Option<f32>]) -> Vec<f32> {
    let n = v.len();
    let known: Vec<usize> = (0..n).filter(|&i| v[i].is_some()).collect();
    if known.is_empty() {
        return vec![0.0; n];
    }
    let mut out = vec![0.0f32; n];
    for i in 0..n {
        out[i] = match v[i] {
            Some(x) => x,
            None => {
                let prev = known.iter().rev().find(|&&k| k < i).copied();
                let next = known.iter().find(|&&k| k > i).copied();
                match (prev, next) {
                    (Some(a), Some(b)) => {
                        let t = (i - a) as f32 / (b - a) as f32;
                        v[a].unwrap() * (1.0 - t) + v[b].unwrap() * t
                    }
                    (Some(a), None) => v[a].unwrap(),
                    (None, Some(b)) => v[b].unwrap(),
                    (None, None) => 0.0,
                }
            }
        };
    }
    let mean = out.iter().sum::<f32>() / n as f32;
    out.iter().map(|&x| (x - mean).clamp(-BAND_CLAMP_DB, BAND_CLAMP_DB)).collect()
}

/// Least-squares line y = a + b x. Returns (a, b); (mean y, 0) if x has no spread.
pub(crate) fn line_fit(pts: impl Iterator<Item = (f32, f32)>) -> (f32, f32) {
    let (mut n, mut sx, mut sy, mut sxx, mut sxy) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for (x, y) in pts {
        let (x, y) = (x as f64, y as f64);
        n += 1.0;
        sx += x;
        sy += y;
        sxx += x * x;
        sxy += x * y;
    }
    if n < 2.0 {
        return (if n > 0.0 { (sy / n) as f32 } else { 0.0 }, 0.0);
    }
    let den = n * sxx - sx * sx;
    if den.abs() < 1e-18 {
        return ((sy / n) as f32, 0.0);
    }
    let b = (n * sxy - sx * sy) / den;
    (((sy - b * sx) / n) as f32, b as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    const SR: f32 = 48_000.0;

    struct Lcg(u32);
    impl Lcg {
        fn next(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
            (self.0 >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        }
    }

    /// Sum of partials (freq, amp, decay tau or 0) with a linear attack and amplitude envelope.
    fn additive(dur: f32, parts: &[(f32, f32, f32)], attack: f32, release: f32) -> Vec<f32> {
        let n = (dur * SR) as usize;
        let mut out = vec![0.0f32; n];
        for &(f, a, tau) in parts {
            let w = 2.0 * PI * f / SR;
            for (i, o) in out.iter_mut().enumerate() {
                let t = i as f32 / SR;
                let d = if tau > 0.0 { (-t / tau).exp() } else { 1.0 };
                *o += a * d * (w * i as f32).sin();
            }
        }
        for (i, o) in out.iter_mut().enumerate() {
            let t = i as f32 / SR;
            let att = if attack > 0.0 { (t / attack).min(1.0) } else { 1.0 };
            let rel = ((dur - t) / release.max(1e-6)).clamp(0.0, 1.0);
            *o *= 0.3 * att * rel;
        }
        out
    }

    fn band_of(freq: f32, n: usize) -> usize {
        let mut c = [0.0f32; MAX_BANDS];
        bands::band_centres(n, &mut c);
        (0..n).min_by(|&a, &b| (c[a] / freq).ln().abs().total_cmp(&(c[b] / freq).ln().abs())).unwrap()
    }

    #[test]
    fn layout_length_and_order() {
        assert_eq!(MATCH_LEN, COUNT + 2 * MAX_BANDS + 4);
        let mut r = default_result();
        r.pitch_cents = 1.0;
        r.duration_s = 2.0;
        r.voiced_fraction = 3.0;
        r.pitched = true;
        r.params.tone_bands[0] = 7.0;
        r.params.noise_bands[MAX_BANDS - 1] = 9.0;
        let mut out = vec![0.0f32; MATCH_LEN];
        r.to_floats(&mut out);
        assert_eq!(out[COUNT], 7.0);
        assert_eq!(out[COUNT + 2 * MAX_BANDS - 1], 9.0);
        assert_eq!(&out[MATCH_LEN - 4..], &[1.0, 2.0, 3.0, 1.0]);
    }

    #[test]
    fn degenerate_inputs_return_defaults() {
        let d = default_result();
        for sig in [vec![], vec![0.5; 100], vec![0.0; 48_000], vec![f32::NAN; 48_000]] {
            let r = instant_match(&sig, SR, 16);
            assert!(!r.pitched);
            assert_eq!(r.pitch_cents, 4800.0);
            assert_eq!(r.params.scalars, d.params.scalars);
        }
        let r = instant_match(&vec![0.5; 48_000], 0.0, 16);
        assert!(!r.pitched);
        // Odd band counts still give finite output.
        let sig = additive(0.5, &[(300.0, 1.0, 0.0)], 0.01, 0.05);
        for nb in [0, 1, 2, 16, 24, 99] {
            let r = instant_match(&sig, SR, nb);
            let mut out = vec![0.0f32; MATCH_LEN];
            r.to_floats(&mut out);
            assert!(out.iter().all(|v| v.is_finite()), "n_bands {nb}");
        }
    }

    #[test]
    fn recovers_tilt_and_pitch() {
        let f0 = 220.0;
        let parts: Vec<_> = (1..=40).map(|k| (f0 * k as f32, 10f32.powf(-9.0 * (k as f32).log2() / 20.0), 0.0)).collect();
        let r = instant_match(&additive(1.5, &parts, 0.02, 0.05), SR, 16);
        assert!(r.pitched, "voiced {}", r.voiced_fraction);
        let tilt = r.params.scalars[TILT];
        assert!((tilt + 9.0).abs() < 2.0, "tilt {tilt}");
        assert!(r.params.scalars[ODDEVEN].abs() < 0.3, "oddeven {}", r.params.scalars[ODDEVEN]);
        assert!(r.params.scalars[STRETCH] < 0.0015, "stretch {}", r.params.scalars[STRETCH]);
        assert!(r.params.scalars[MODAL] < 0.5, "modal {}", r.params.scalars[MODAL]);
        assert!((r.pitch_cents - hz_to_cents(f0)).abs() < 20.0);
        assert!(r.params.scalars[HARM_MIX] == 1.0 && r.params.scalars[NOISE_MIX] < 0.1);
        assert!(r.duration_s > 1.0);
        // Shape of a clean harmonic tone is flat once the tilt is removed.
        let tb = &r.params.tone_bands[..16];
        assert!(tb.iter().all(|v| v.abs() < 6.0), "{tb:?}");
    }

    #[test]
    fn odd_only_wave_has_positive_oddeven() {
        let parts: Vec<_> = (0..20).map(|j| 2 * j + 1).map(|k| (200.0 * k as f32, 1.0 / k as f32, 0.0)).collect();
        let r = instant_match(&additive(1.0, &parts, 0.02, 0.05), SR, 16);
        assert!(r.pitched);
        assert!(r.params.scalars[ODDEVEN] > 0.5, "oddeven {}", r.params.scalars[ODDEVEN]);
    }

    #[test]
    fn stretched_partials_give_stretch() {
        let (f0, b) = (150.0f32, 0.002f32);
        let parts: Vec<_> =
            (1..=30).map(|k| (f0 * k as f32 * (1.0 + b * (k * k) as f32).sqrt(), 1.0 / k as f32, 0.0)).collect();
        let r = instant_match(&additive(1.2, &parts, 0.02, 0.05), SR, 16);
        assert!(r.pitched);
        let s = r.params.scalars[STRETCH];
        assert!(s > 0.0005 && s < 0.006, "stretch {s}");
        assert!(r.params.scalars[MODAL] < 0.5, "modal {}", r.params.scalars[MODAL]);
    }

    #[test]
    fn bar_strike_reads_as_modal_two_with_damping() {
        let f0 = 300.0f32;
        let damp = 0.5f32;
        let parts: Vec<_> = (1..=6)
            .map(|k| {
                let r = bar_ratio(k);
                (f0 * r, r.powf(-1.5), 0.6 * r.powf(-2.0 * damp))
            })
            .collect();
        let r = instant_match(&additive(2.0, &parts, 0.0, 0.05), SR, 16);
        assert!(r.pitched, "voiced {}", r.voiced_fraction);
        assert!(r.params.scalars[MODAL] > 1.5, "modal {}", r.params.scalars[MODAL]);
        assert_eq!(r.params.scalars[ENV_SUSTAIN], 0.0);
        let d = r.params.scalars[DAMP];
        assert!(d > 0.2 && d < 0.9, "damp {d}");
    }

    #[test]
    fn band_passed_noise_is_unpitched_with_peak_in_band() {
        let mut rng = Lcg(7);
        // RBJ band-pass at 2 kHz, Q 4.
        let (f, q) = (2000.0f32, 4.0f32);
        let w = 2.0 * PI * f / SR;
        let alpha = w.sin() / (2.0 * q);
        let (b0, b2, a0, a1, a2) = (alpha, -alpha, 1.0 + alpha, -2.0 * w.cos(), 1.0 - alpha);
        let (mut x1, mut x2, mut y1, mut y2) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        let sig: Vec<f32> = (0..(2.0 * SR) as usize)
            .map(|_| {
                let x = rng.next();
                let y = (b0 * x + b2 * x2 - a1 * y1 - a2 * y2) / a0;
                (x2, x1, y2, y1) = (x1, x, y1, y);
                y * 0.5
            })
            .collect();
        let r = instant_match(&sig, SR, 16);
        assert!(!r.pitched, "voiced {}", r.voiced_fraction);
        assert_eq!(r.pitch_cents, 4800.0);
        assert_eq!(r.params.scalars[NOISE_MIX], 1.0);
        assert!(r.params.scalars[HARM_MIX] <= 0.2);
        let nb = &r.params.noise_bands[..16];
        let top = (0..16).max_by(|&a, &b| nb[a].total_cmp(&nb[b])).unwrap();
        let want = band_of(2000.0, 16);
        assert!(top.abs_diff(want) <= 1, "peak band {top}, want {want}: {nb:?}");
        assert!(nb[top] > 6.0);
    }

    #[test]
    fn white_noise_has_flat_shape() {
        let mut rng = Lcg(99);
        let sig: Vec<f32> = (0..SR as usize).map(|_| 0.3 * rng.next()).collect();
        let r = instant_match(&sig, SR, 16);
        assert!(!r.pitched);
        assert!(r.params.noise_bands[..16].iter().all(|v| v.abs() < 3.0), "{:?}", &r.params.noise_bands[..16]);
    }

    #[test]
    fn envelope_attack_decay_sustain() {
        // Linear ramp over 0.15 s (10% to 90% takes 0.12 s), then exp decay tau 0.25 s.
        let (ramp, tau) = (0.15f32, 0.25f32);
        let n = (2.0 * SR) as usize;
        let sig: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / SR;
                let a = if t < ramp { t / ramp } else { (-(t - ramp) / tau).exp() };
                0.5 * a * (2.0 * PI * 440.0 * t).sin() + 0.25 * a * (2.0 * PI * 880.0 * t).sin()
            })
            .collect();
        let r = instant_match(&sig, SR, 16);
        let ps = &r.params.scalars;
        assert!((ps[ENV_ATTACK] - 0.12).abs() < 0.024, "attack {}", ps[ENV_ATTACK]);
        assert!(ps[ENV_DECAY] > 0.175 && ps[ENV_DECAY] < 0.325, "decay {}", ps[ENV_DECAY]);
        assert!(ps[ENV_SUSTAIN] < 0.1, "sustain {}", ps[ENV_SUSTAIN]);
        let want_dur = ramp + tau * (30.0f32 / 8.686).max(0.0);
        assert!((r.duration_s - want_dur).abs() < 0.2, "duration {} want {want_dur}", r.duration_s);
        assert!(r.pitched);
    }

    #[test]
    fn plateau_gives_sustain_and_release() {
        // 0.5 s sine with fast attack, a hold at a lower plateau after a decay, then a 0.2 s linear fall.
        let n = (1.5 * SR) as usize;
        let sig: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / SR;
                let a = if t < 0.02 {
                    t / 0.02
                } else if t < 0.3 {
                    0.5 + 0.5 * (-(t - 0.02) / 0.05).exp()
                } else if t < 1.2 {
                    0.5
                } else {
                    (0.5 * (1.0 - (t - 1.2) / 0.3)).max(0.0)
                };
                0.6 * a * (2.0 * PI * 330.0 * t).sin()
            })
            .collect();
        let r = instant_match(&sig, SR, 16);
        let ps = &r.params.scalars;
        assert!(ps[ENV_SUSTAIN] > 0.4 && ps[ENV_SUSTAIN] < 0.6, "sustain {}", ps[ENV_SUSTAIN]);
        assert!(ps[ENV_RELEASE] > 0.1 && ps[ENV_RELEASE] < 0.4, "release {}", ps[ENV_RELEASE]);
    }

    #[test]
    fn pitch_drop_is_found() {
        // Kick-like: starts 12 semitones up, falls with tau 50 ms to 110 Hz.
        let n = (0.8 * SR) as usize;
        let mut ph = 0.0f64;
        let sig: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / SR;
                let semis = 12.0 * (-t / 0.05).exp();
                let f = 110.0 * 2f32.powf(semis / 12.0);
                ph += 2.0 * std::f64::consts::PI * f as f64 / SR as f64;
                0.6 * (-t / 0.4).exp() * (ph.sin() as f32)
            })
            .collect();
        let r = instant_match(&sig, SR, 16);
        assert!(r.pitched, "voiced {}", r.voiced_fraction);
        let amt = r.params.scalars[ENV_PITCH_AMT];
        let time = r.params.scalars[ENV_PITCH_TIME];
        assert!(amt > 4.0 && amt < 20.0, "amt {amt}");
        assert!(time > 0.02 && time < 0.15, "time {time}");
        assert!((r.pitch_cents - hz_to_cents(110.0)).abs() < 100.0, "pitch {}", r.pitch_cents);
    }

    #[test]
    fn formant_bump_is_found() {
        // 150 Hz harmonics with a +15 dB bump around 1 kHz.
        let f0 = 150.0f32;
        let parts: Vec<_> = (1..=40)
            .map(|k| {
                let f = f0 * k as f32;
                let bump = 15.0 * (-0.5 * ((f / 1000.0).log2() / 0.14).powi(2)).exp();
                (f, 10f32.powf((-6.0 * (k as f32).log2() + bump) / 20.0), 0.0)
            })
            .collect();
        let r = instant_match(&additive(1.0, &parts, 0.02, 0.05), SR, 16);
        let ps = &r.params.scalars;
        let (f1, g1, f2, g2) = (ps[F1_FREQ], ps[F1_GAIN], ps[F2_FREQ], ps[F2_GAIN]);
        let hit = (g1 > 5.0 && (f1 / 1000.0).log2().abs() < 0.3) || (g2 > 5.0 && (f2 / 1000.0).log2().abs() < 0.3);
        assert!(hit, "f1 {f1} {g1} f2 {f2} {g2}");
        assert!((ps[TILT] + 6.0).abs() < 2.5, "tilt {}", ps[TILT]);
    }

    #[test]
    fn ten_seconds_is_quick() {
        let parts: Vec<_> = (1..=30).map(|k| (180.0 * k as f32, 1.0 / k as f32, 0.0)).collect();
        let sig = additive(10.0, &parts, 0.05, 0.1);
        let t = std::time::Instant::now();
        let r = instant_match(&sig, SR, 16);
        let el = t.elapsed().as_secs_f32();
        eprintln!("10 s of audio matched in {el:.3} s");
        assert!(r.pitched);
        assert!(el < 5.0, "{el}");
    }
}
