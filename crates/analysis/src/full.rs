//! Influence match for the M1 engine (TDD "Sound Influence" / "Instant match").
//!
//! `match_full` measures a mono source and returns a target over the whole
//! M1 parameter table (96 normalized values, `params::to_norm`, clamped), plus
//! what the host needs around it: pitch, length, whether the source is
//! pitched, a grain-source suggestion and a "provisional" flag for the motion
//! rows. The M0 measurement (`instant_inner`) is reused for every row it
//! already covers; this file adds the rows the M0 engine did not have and
//! converts the level relationships to what the Klont engine does.
//!
//! Mapping choices (each one was checked by rendering known Klont parameter
//! sets and measuring them again, see tests/round_trip.rs):
//!
//! * Source rows come straight from the M0 measurement (tilt, oddeven, stretch,
//!   modal, damp, f1/f2, tone and noise bands, envelope, noise decay, pitch
//!   drop). The M0 tables for drumhead and bar partials equal the Klont's.
//!   `damp` is only measured for sounds that decay (sustain under 0.3) or when
//!   `decaying` says the audio is one hit for a kit slot.
//! * harm_mix / noise_mix: the Klont normalises its partial bank to a fixed
//!   power (rms 0.173 at harm_mix 1) and its shaped noise to rms 0.35 at
//!   noise_mix 1 with 0 dB bands, so the pair is `NOISE_REL` times the
//!   measured rms ratio, scaled so the larger is 1. Noise bands are rescaled
//!   so their power, weighted by band width (the Klont's band-passes are
//!   constant-Q, so a band's power follows its centre frequency), averages to
//!   0 dB; then the band shape changes the colour and not the level.
//! * Unpitched sources (under 30% voiced) skip the harmonic rows: harm_mix 0,
//!   noise only, and `use_sound` defaults to on.
//! * Motion: the pitch, loudness and spectral-centroid tracks (10 ms hop) are
//!   detrended with a quadratic and scanned for a sinusoid between 0.5 and
//!   20 Hz. A peak needs three times the median level of the scan. Pitch gives
//!   lfo1.rate and lfo1.to_pitch (peak cents), loudness gives lfo1.to_level
//!   (tremolo depth from the peak-to-peak dB), centroid gives to_filter (peak
//!   octaves; with a filt.cutoff of 3 times the median centroid so the
//!   low-pass is in range). A peak whose rate differs from lfo1's by more than
//!   25% goes to lfo2 (pitch and filter only; lfo2 has no level). With no pitch
//!   peak but a wandering pitch (rms over 10 cents) the row is chaos. Under two
//!   cycles seen, or under 2 s of steady sound, the result is `provisional`.
//! * strike.click: energy of the first 5 ms above 2 kHz against the body
//!   (10 to 40 ms later), above a threshold, as a fraction of the body rms;
//!   strike.color is the centroid of that first 5 ms.
//! * fold.amt: ripple of the partial levels around their straight line (dB
//!   against log2 of the partial number), for flat-topped waves (crest under
//!   1.8) with at least 20 audible partials. Pitched sources only. The
//!   Klont's folder at typical levels is mild, so this reads it low (about 0.25
//!   at fold.amt 1) and a strongly formant-shaped sound can read a little
//!   fold; it is the weakest of the grit rows.
//! * crush.rate: only for unpitched sources whose spectrum ends in a brick
//!   wall under 6 kHz (a low-rate recording). A smooth roll-off is the filter's
//!   job and stays with the tone rows. The Klont's own crush shows up as
//!   images, not a wall, so it is not read back.
//! * verb.size / verb.mix: when the loudness falls at least 15 dB in 40 ms
//!   from near its peak and then keeps falling at less than a third of that
//!   speed for over 300 ms, the slow part is a reverb tail: RT60 = 0.3 s *
//!   40^size and the tail level at the stop is the wet mix.
//! * Grain: the steady region is the longest stretch within 6 dB of the median
//!   loudness of the loud part (frames within 20 dB of the peak), 100 ms to
//!   10 s; its reference pitch is the median voiced pitch in it, or none.
//!   `use_sound` sets src.grain_mix to `GRAIN_MIX_ON` and scales harm_mix and
//!   noise_mix by what is left, so the grain cloud carries the sound's own
//!   colour while the synth keeps its envelope.

use crate::influence::{VOICED_CONF, instant_inner, line_fit, track_pitch};
use dsp::fft::{Fft, hann};
use dsp::{cents_to_hz, hz_to_cents};
use params as P;

/// Floats in the flat result of `an_match_full` (and `FullMatch::to_floats`):
/// the 96 normalized target values, then `FULL_EXTRA` floats:
///
/// | offset from 96 | value |
/// | --- | --- |
/// | 0 | pitched (0 or 1) |
/// | 1 | pitch_cents, cents above C0 to play the match at |
/// | 2 | duration_s, how long to hold the audition note |
/// | 3 | voiced_fraction |
/// | 4 | provisional (0 or 1): motion rows not yet reliable |
/// | 5 | use_sound (0 or 1): src.grain_mix was raised |
/// | 6 | grain_start, samples (0 when none) |
/// | 7 | grain_end, samples (0 when none) |
/// | 8 | grain_ref_cents, NaN for none (unpitched) |
/// | 9 | grain_valid (0 or 1) |
///
/// Mirrored as FULL_MATCH_LEN in web/src/m4/match.ts.
pub const FULL_MATCH_LEN: usize = P::COUNT + FULL_EXTRA;
pub const FULL_EXTRA: usize = 10;

/// `flags` bits of `an_match_full`.
pub const FLAG_DECAYING: u32 = 1;
/// Force `use_sound` on.
pub const FLAG_USE_SOUND: u32 = 2;
/// Force `use_sound` off (wins over FLAG_USE_SOUND).
pub const FLAG_NO_SOUND: u32 = 4;

/// grain_mix when "use my sound" is on.
pub const GRAIN_MIX_ON: f32 = 0.75;
/// noise_mix over harm_mix per unit of measured noise-to-partials rms ratio. Fitted
/// against the Klont's own render (tests/round_trip.rs): the nominal rms figures
/// (partials 0.173, noise 0.35) would give 0.49, but the band-pass bank passes
/// about 2.3 times less than its nominal level.
const NOISE_REL: f32 = 1.12;
/// noise_mix below this (against harm_mix 1) is taken as zero.
const NOISE_FLOOR: f32 = 0.05;
const HOP_S: f32 = 0.01;
const MAX_GRAIN_S: f32 = 10.0;
const MIN_GRAIN_S: f32 = 0.1;

#[derive(Clone, Copy, Debug, Default)]
pub struct FullOpts {
    /// The audio is one hit for a kit slot: decaying-sound rows on.
    pub decaying: bool,
    /// `None`: on for unpitched sources only.
    pub use_sound: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GrainSuggestion {
    /// Sample range of the steady region in the analysed signal.
    pub start: usize,
    pub end: usize,
    /// Cents above C0 of the region's pitch; None when it is unpitched.
    pub ref_cents: Option<f32>,
}

#[derive(Clone, Debug)]
pub struct FullMatch {
    /// Normalized values in `params` order (96).
    pub target: [f32; P::COUNT],
    pub pitched: bool,
    pub pitch_cents: f32,
    pub duration_s: f32,
    pub voiced_fraction: f32,
    /// Motion rows (LFO, chaos) rest on under two cycles or under 2 s.
    pub provisional: bool,
    /// src.grain_mix was raised in `target`.
    pub use_sound: bool,
    pub grain: Option<GrainSuggestion>,
}

impl FullMatch {
    /// Value of parameter `i` in real units.
    pub fn real(&self, i: usize) -> f32 {
        P::to_real(i, self.target[i])
    }

    pub fn to_floats(&self, out: &mut [f32]) {
        out[..P::COUNT].copy_from_slice(&self.target);
        let b = P::COUNT;
        out[b] = self.pitched as u8 as f32;
        out[b + 1] = self.pitch_cents;
        out[b + 2] = self.duration_s;
        out[b + 3] = self.voiced_fraction;
        out[b + 4] = self.provisional as u8 as f32;
        out[b + 5] = self.use_sound as u8 as f32;
        match self.grain {
            Some(g) => {
                out[b + 6] = g.start as f32;
                out[b + 7] = g.end as f32;
                out[b + 8] = g.ref_cents.unwrap_or(f32::NAN);
                out[b + 9] = 1.0;
            }
            None => {
                out[b + 6] = 0.0;
                out[b + 7] = 0.0;
                out[b + 8] = f32::NAN;
                out[b + 9] = 0.0;
            }
        }
    }
}

fn neutral_result() -> FullMatch {
    FullMatch {
        target: P::neutral_set(),
        pitched: false,
        pitch_cents: 4800.0,
        duration_s: 0.2,
        voiced_fraction: 0.0,
        provisional: true,
        use_sound: false,
        grain: None,
    }
}

fn set(t: &mut [f32; P::COUNT], i: usize, real: f32) {
    if real.is_finite() {
        t[i] = P::to_norm(i, real);
    }
}

fn median(v: &mut [f32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

/// Measure `signal` for the M1 engine.
pub fn match_full(signal: &[f32], sample_rate: f32, opts: FullOpts) -> FullMatch {
    let mut out = neutral_result();
    if !(sample_rate.is_finite() && (4000.0..=400_000.0).contains(&sample_rate)) {
        return out;
    }
    let sr = sample_rate;
    let hop = ((sr * HOP_S).round() as usize).max(1);
    if signal.len() < hop * 5 {
        return out;
    }
    let x: Vec<f32> = signal.iter().map(|&v| if v.is_finite() { v } else { 0.0 }).collect();
    let nf = x.len() / hop;
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
        return out;
    }
    let db: Vec<f32> = amp.iter().map(|&a| 20.0 * (a.max(1e-9) / pk).log10()).collect();

    let base = instant_inner(&x, sr, 16, opts.decaying);
    let mut t = P::neutral_set();
    let pitched = base.pitched;
    out.pitched = pitched;
    out.pitch_cents = base.pitch_cents;
    out.duration_s = base.duration_s;
    out.voiced_fraction = base.voiced_fraction;
    let m = &base.params.scalars;

    // ---- source rows from the M0 measurement ----
    use dsp::params as m0;
    if pitched {
        // A sound with one or two partials (a sine, a whistle) has no tilt to
        // measure; the M0 fallback of -6 dB/oct would add partials it lacks.
        let sparse = base.partials <= 2.0;
        set(&mut t, P::SRC_TILT, if sparse { -24.0 } else { m[m0::TILT] });
        set(&mut t, P::SRC_ODDEVEN, if sparse { 0.0 } else { m[m0::ODDEVEN] });
        set(&mut t, P::SRC_STRETCH, m[m0::STRETCH]);
        set(&mut t, P::SRC_MODAL, m[m0::MODAL]);
        set(&mut t, P::SRC_DAMP, m[m0::DAMP]);
        if m[m0::F1_GAIN] > 0.0 {
            set(&mut t, P::SRC_F1_FREQ, m[m0::F1_FREQ]);
            set(&mut t, P::SRC_F1_GAIN, m[m0::F1_GAIN]);
        }
        if m[m0::F2_GAIN] > 0.0 {
            set(&mut t, P::SRC_F2_FREQ, m[m0::F2_FREQ]);
            set(&mut t, P::SRC_F2_GAIN, m[m0::F2_GAIN]);
        }
        if sparse {
            // Nothing above the second partial: cut the bands from 1.2 to
            // 1.8 times the fundamental down by 30 dB, so the bank stays a
            // sine and not a sine with a -24 dB/oct skirt of partials.
            let mut c = [0.0f32; dsp::frame::MAX_BANDS];
            dsp::bands::band_centres(P::N_BANDS, &mut c);
            let f0 = cents_to_hz(base.pitch_cents);
            for b in 0..P::N_BANDS {
                let u = ((c[b] / f0).log2() - 1.2f32.log2()) / (1.8f32.log2() - 1.2f32.log2());
                t[P::TONE_BANDS + b] = P::to_norm(P::TONE_BANDS + b, -30.0 * u.clamp(0.0, 1.0));
            }
        } else {
            // Bands more than half an octave above the highest partial that
            // was found are cut (up to -30 dB): the sound has nothing there,
            // and a flat band shape would let the bank add partials it lacks.
            let mut c = [0.0f32; dsp::frame::MAX_BANDS];
            dsp::bands::band_centres(P::N_BANDS, &mut c);
            let top = base.partial_top_hz;
            for b in 0..P::N_BANDS {
                let cut = if top > 0.0 { 30.0 * ((c[b] / top).log2() / 0.5).clamp(0.0, 1.0) } else { 0.0 };
                t[P::TONE_BANDS + b] = P::to_norm(P::TONE_BANDS + b, base.params.tone_bands[b] - cut);
            }
        }
        set(&mut t, P::ENV_PITCH_AMT, m[m0::ENV_PITCH_AMT]);
        set(&mut t, P::ENV_PITCH_TIME, m[m0::ENV_PITCH_TIME]);
        // Level relation between the partial bank and the noise.
        let r = base.noise_to_harm * NOISE_REL; // noise_mix / harm_mix
        let (h, nz) = if !r.is_finite() || r >= 1e3 {
            (0.0, 1.0)
        } else if r < 1.0 {
            (1.0, r)
        } else {
            (1.0 / r, 1.0)
        };
        t[P::SRC_HARM_MIX] = h.clamp(0.0, 1.0);
        // Under -26 dB against the partials the residual is leakage from the
        // partials themselves (sidebands, window skirts), not noise.
        t[P::SRC_NOISE_MIX] = if nz < NOISE_FLOOR { 0.0 } else { nz.clamp(0.0, 1.0) };
    } else {
        t[P::SRC_HARM_MIX] = 0.0;
        t[P::SRC_NOISE_MIX] = 1.0;
    }
    // Noise bands: shape from the residual, level fixed by the mixes above.
    {
        let mut c = [0.0f32; dsp::frame::MAX_BANDS];
        dsp::bands::band_centres(P::N_BANDS, &mut c);
        let s = &base.params.noise_bands;
        let (mut wsum, mut psum) = (0.0f32, 0.0f32);
        for b in 0..P::N_BANDS {
            wsum += c[b];
            psum += c[b] * 10f32.powf(0.1 * s[b]);
        }
        let off = -10.0 * (psum / wsum).max(1e-12).log10();
        for b in 0..P::N_BANDS {
            t[P::NOISE_BANDS + b] = P::to_norm(P::NOISE_BANDS + b, s[b] + off);
        }
    }
    // Envelope. The Klont's noise has no attack stage (only noise_decay and
    // release), so a noise-only source keeps these times for its partials-free
    // render; they still set the release.
    // The measured attack is the 10% to 90% time of the loudness; the Klont's
    // attack is a linear ramp, whose 10-90% time is 0.8 of it. The measured
    // release is the time to -40 dB; the Klont's is a time constant, and -40 dB
    // is ln(100) = 4.6 of them.
    set(&mut t, P::ENV_ATTACK, m[m0::ENV_ATTACK] / 0.8);
    set(&mut t, P::ENV_HOLD, m[m0::ENV_HOLD]);
    set(&mut t, P::ENV_DECAY, m[m0::ENV_DECAY]);
    set(&mut t, P::ENV_SUSTAIN, m[m0::ENV_SUSTAIN]);
    set(&mut t, P::ENV_RELEASE, m[m0::ENV_RELEASE] / 4.605);
    set(&mut t, P::ENV_NOISE_DECAY, m[m0::ENV_NOISE_DECAY]);

    // ---- tracks ----
    let track = track_pitch(&x, sr, hop, &db);
    let voiced = |i: usize| track[i].0 > 0.0 && track[i].1 >= VOICED_CONF;

    // Click and colour from the first milliseconds.
    click_rows(&x, sr, &db, &amp, &mut t);

    // Motion.
    let region = longest_run(&db, |d| d >= -20.0).unwrap_or((p, p + 1));
    let provisional;
    if region.1 - region.0 >= 20 {
        provisional = motion_rows(&x, sr, hop, &db, &track, region, pitched, m[m0::ENV_PITCH_AMT] != 0.0, &mut t)
            || ((region.1 - region.0) as f32 * HOP_S) < 2.0;
    } else {
        provisional = true;
    }
    out.provisional = provisional;

    // Grit.
    if pitched {
        fold_row(&x, sr, hop, &db, p, base.pitch_cents, &mut t);
    } else {
        crush_row(&x, sr, hop, &db, region, &mut t);
    }
    // Space. A tremolo makes drops and rises that are not a stop and a tail.
    if t[P::LFO1_TO_LEVEL] == P::to_norm(P::LFO1_TO_LEVEL, 0.0) {
        verb_rows(&db, &mut t);
    }

    // ---- grain source ----
    let grain = grain_suggestion(&x, hop, &db, &track, &voiced);
    out.grain = Some(grain);
    let use_sound = opts.use_sound.unwrap_or(!pitched);
    out.use_sound = use_sound;
    if use_sound {
        t[P::SRC_GRAIN_MIX] = P::to_norm(P::SRC_GRAIN_MIX, GRAIN_MIX_ON);
        let keep = 1.0 - GRAIN_MIX_ON;
        t[P::SRC_HARM_MIX] *= keep;
        t[P::SRC_NOISE_MIX] *= keep;
    }

    for v in t.iter_mut() {
        if !v.is_finite() {
            *v = 0.0;
        }
        *v = v.clamp(0.0, 1.0);
    }
    out.target = t;
    out
}

// ---------------------------------------------------------------- helpers

/// Longest run of frames satisfying `f` (half-open range).
fn longest_run(db: &[f32], f: impl Fn(f32) -> bool) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    let mut i = 0;
    while i < db.len() {
        if f(db[i]) {
            let s = i;
            while i < db.len() && f(db[i]) {
                i += 1;
            }
            if best.is_none_or(|b| i - s > b.1 - b.0) {
                best = Some((s, i));
            }
        } else {
            i += 1;
        }
    }
    best
}

fn hann_frame(x: &[f32], centre: isize, n: usize, win: &[f32], re: &mut [f32], im: &mut [f32]) {
    for j in 0..n {
        let s = centre - (n / 2) as isize + j as isize;
        re[j] = if s >= 0 && (s as usize) < x.len() { x[s as usize] * win[j] } else { 0.0 };
        im[j] = 0.0;
    }
}

// ---------------------------------------------------------------- click

fn click_rows(x: &[f32], sr: f32, db: &[f32], amp: &[f32], t: &mut [f32; P::COUNT]) {
    let Some(onset) = db.iter().position(|&d| d >= -30.0) else { return };
    let gpk = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    // First sample of the onset block that is clearly above the floor.
    let blk = (sr * HOP_S) as usize;
    let a = onset * blk;
    let s0 = (a..(a + 2 * blk).min(x.len())).find(|&i| x[i].abs() >= 0.05 * gpk).unwrap_or(a);
    let n5 = (0.005 * sr) as usize;
    let (b0, b1) = (s0 + (0.010 * sr) as usize, s0 + (0.040 * sr) as usize);
    if b1 + 1 > x.len() || n5 < 8 {
        return;
    }
    // Two one-pole high-passes near 2 kHz (12 dB/oct).
    let a_hp = (-std::f32::consts::TAU * 2000.0 / sr).exp();
    let mut hp = vec![0.0f32; b1 - s0 + 1];
    let (mut y1, mut y2, mut p1, mut p2) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for (k, h) in hp.iter_mut().enumerate() {
        let v = x[s0 + k];
        y1 = a_hp * (y1 + v - p1);
        p1 = v;
        y2 = a_hp * (y2 + y1 - p2);
        p2 = y1;
        *h = y2;
    }
    let rms = |s: &[f32]| (s.iter().map(|v| v * v).sum::<f32>() / s.len().max(1) as f32).sqrt();
    let e5 = rms(&hp[..n5]);
    let eb = rms(&hp[b0 - s0..b1 - s0]);
    let body = {
        let lo = s0 + (0.010 * sr) as usize;
        rms(&x[lo..(lo + 2 * n5 * 6).min(x.len())])
    };
    let _ = amp;
    let excess = (e5 * e5 - eb * eb).max(0.0).sqrt();
    if !(body > 1e-6) || e5 < 1.6 * eb {
        return;
    }
    let click = (excess / body / CLICK_PER_RATIO).clamp(0.0, 1.0);
    if click < 0.08 {
        return;
    }
    set(t, P::STRIKE_CLICK, click);
    // Colour: centroid of the first 5 ms above 500 Hz.
    let n = 512usize;
    let fft = Fft::new(n);
    let mut win = vec![0.0; n];
    hann(&mut win);
    let (mut re, mut im) = (vec![0.0; n], vec![0.0; n]);
    let w5: Vec<f32> = (0..n5).map(|j| 0.5 - 0.5 * (std::f32::consts::TAU * (j as f32 + 0.5) / n5 as f32).cos()).collect();
    for j in 0..n {
        re[j] = if j < n5 { x[s0 + j] * w5[j] } else { 0.0 };
        im[j] = 0.0;
    }
    fft.forward(&mut re, &mut im);
    let bin = sr / n as f32;
    let (mut sw, mut sf) = (0.0f32, 0.0f32);
    for j in (500.0 / bin).ceil() as usize..n / 2 {
        let w = re[j] * re[j] + im[j] * im[j];
        sw += w;
        sf += w * j as f32 * bin;
    }
    if sw > 0.0 {
        set(t, P::STRIKE_COLOR, sf / sw);
    }
}

/// strike.click per unit of (first-5ms excess rms over body rms). Fitted
/// against the Klont's own click, see tests/round_trip.rs.
const CLICK_PER_RATIO: f32 = 0.33;

// ---------------------------------------------------------------- motion

/// Least-squares polynomial of order `ord` (0..=2) removed from `y` in place.
fn detrend(y: &mut [f32]) {
    let n = y.len();
    if n < 4 {
        return;
    }
    // Normal equations for 1, u, u^2 with u in -1..1.
    let mut a = [[0.0f64; 3]; 3];
    let mut b = [0.0f64; 3];
    for (i, &v) in y.iter().enumerate() {
        let u = 2.0 * i as f64 / (n - 1) as f64 - 1.0;
        let f = [1.0, u, u * u];
        for r in 0..3 {
            for c in 0..3 {
                a[r][c] += f[r] * f[c];
            }
            b[r] += f[r] * v as f64;
        }
    }
    // Gauss-Jordan on a 3x3.
    for c in 0..3 {
        let mut piv = c;
        for r in c + 1..3 {
            if a[r][c].abs() > a[piv][c].abs() {
                piv = r;
            }
        }
        if a[piv][c].abs() < 1e-12 {
            return;
        }
        a.swap(c, piv);
        b.swap(c, piv);
        for r in 0..3 {
            if r != c {
                let k = a[r][c] / a[c][c];
                for cc in 0..3 {
                    a[r][cc] -= k * a[c][cc];
                }
                b[r] -= k * b[c];
            }
        }
    }
    let co = [b[0] / a[0][0], b[1] / a[1][1], b[2] / a[2][2]];
    for (i, v) in y.iter_mut().enumerate() {
        let u = 2.0 * i as f64 / (n - 1) as f64 - 1.0;
        *v -= (co[0] + co[1] * u + co[2] * u * u) as f32;
    }
}

#[derive(Clone, Copy, Debug)]
struct Tone {
    rate: f32,
    amp: f32,
    cycles: f32,
}

/// Strongest sinusoid between 0.5 and 20 Hz in an evenly sampled series that
/// has been detrended. None unless it stands three times above the median of
/// the scan and has at least one cycle in the series.
fn scan_tone(y: &[f32], dt: f32) -> Option<Tone> {
    let n = y.len();
    let dur = n as f32 * dt;
    let nyq = 0.5 / dt;
    let grid: Vec<f32> = (0..80).map(|i| 0.5 * (20.0f32 / 0.5).powf(i as f32 / 79.0)).filter(|&r| r * dur >= 1.0 && r < nyq * 0.9).collect();
    if grid.len() < 6 {
        return None;
    }
    let w: Vec<f32> = (0..n).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * (i as f32 + 0.5) / n as f32).cos()).collect();
    let wsum: f32 = w.iter().sum();
    let amps: Vec<f32> = grid
        .iter()
        .map(|&r| {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for i in 0..n {
                let ph = std::f64::consts::TAU * r as f64 * i as f64 * dt as f64;
                let v = (w[i] * y[i]) as f64;
                re += v * ph.cos();
                im += v * ph.sin();
            }
            (2.0 * (re * re + im * im).sqrt() / wsum as f64) as f32
        })
        .collect();
    let mut best = 0;
    for i in 0..amps.len() {
        if amps[i] > amps[best] {
            best = i;
        }
    }
    let med = median(&mut amps.clone());
    if !(amps[best] > 3.0 * med.max(1e-9)) {
        return None;
    }
    // A sinusoid explains most of the variance; a wandering signal does not.
    let var = y.iter().map(|v| v * v).sum::<f32>() / n as f32;
    if !(0.5 * amps[best] * amps[best] >= 0.6 * var) {
        return None;
    }
    Some(Tone { rate: grid[best], amp: amps[best], cycles: grid[best] * dur })
}

fn rms_of(y: &[f32]) -> f32 {
    (y.iter().map(|v| v * v).sum::<f32>() / y.len().max(1) as f32).sqrt()
}

/// Fills the LFO, chaos and filter rows. Returns true when what it found is
/// provisional (under two cycles seen).
#[allow(clippy::too_many_arguments)]
fn motion_rows(
    x: &[f32],
    sr: f32,
    hop: usize,
    db: &[f32],
    track: &[(f32, f32)],
    region: (usize, usize),
    pitched: bool,
    pitch_drop: bool,
    t: &mut [f32; P::COUNT],
) -> bool {
    let (r0, r1) = region;
    let mut prov = false;

    // Pitch track: longest voiced stretch in the region (gaps of two frames closed).
    let mut pitch_tone: Option<Tone> = None;
    let mut pitch_rms = 0.0f32;
    let mut pitch_len = 0usize;
    let mut pitch_zc = 0.0f32;
    if pitched {
        let skip = if pitch_drop { 15 } else { 0 };
        let (mut best, mut cur): (Vec<f32>, Vec<f32>) = (Vec::new(), Vec::new());
        let mut gap = 0;
        for i in (r0 + skip).min(r1)..r1 {
            let (hz, conf) = track[i];
            if hz > 0.0 && conf >= VOICED_CONF {
                let c = hz_to_cents(hz);
                // Close a short gap by repeating the last value.
                if gap > 0 && gap <= 2 {
                    let last = *cur.last().unwrap_or(&c);
                    for _ in 0..gap {
                        cur.push(last);
                    }
                }
                gap = 0;
                cur.push(c);
            } else {
                gap += 1;
                if gap > 2 && !cur.is_empty() {
                    if cur.len() > best.len() {
                        best = std::mem::take(&mut cur);
                    } else {
                        cur.clear();
                    }
                }
            }
        }
        if cur.len() > best.len() {
            best = cur;
        }
        let mut cents = best;
        if cents.len() >= 40 {
            let med = median(&mut cents.clone());
            // Octave slips end the run.
            if let Some(cut) = cents.iter().position(|&c| (c - med).abs() > 300.0) {
                cents.truncate(cut);
            }
            if cents.len() >= 40 {
                detrend(&mut cents);
                pitch_rms = rms_of(&cents);
                pitch_zc = cents.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count() as f32 / (cents.len() as f32 * HOP_S);
                pitch_len = cents.len();
                pitch_tone = scan_tone(&cents, HOP_S).filter(|t| t.amp >= 6.0);
            }
        }
    }

    // Loudness track in dB over the region.
    let mut loud: Vec<f32> = db[r0..r1].iter().map(|&d| d.max(-40.0)).collect();
    detrend(&mut loud);
    let loud_tone = scan_tone(&loud, HOP_S).filter(|t| t.amp >= 0.8);

    // Centroid track over the region, at most 300 frames.
    let stride = ((r1 - r0) as f32 / 300.0).ceil().max(1.0) as usize;
    let n = 1024usize;
    let fft = Fft::new(n);
    let mut win = vec![0.0; n];
    hann(&mut win);
    let (mut re, mut im) = (vec![0.0f32; n], vec![0.0f32; n]);
    let bin = sr / n as f32;
    let lo = (200.0 / bin).ceil() as usize;
    let hi = ((0.45 * sr) / bin) as usize;
    let mut cen: Vec<f32> = Vec::new();
    let mut cen_hz: Vec<f32> = Vec::new();
    for i in (r0..r1).step_by(stride) {
        hann_frame(x, (i * hop + hop / 2) as isize, n, &win, &mut re, &mut im);
        fft.forward(&mut re, &mut im);
        let (mut sw, mut sf) = (0.0f32, 0.0f32);
        for j in lo..hi.min(n / 2) {
            let m = re[j].hypot(im[j]);
            sw += m;
            sf += m * j as f32 * bin;
        }
        let c = if sw > 0.0 { sf / sw } else { 1000.0 };
        cen_hz.push(c);
        cen.push(c.max(50.0).log2());
    }
    let cen_dt = HOP_S * stride as f32;
    let cen_tone = if cen.len() >= 20 {
        detrend(&mut cen);
        scan_tone(&cen, cen_dt).filter(|t| t.amp >= 0.12)
    } else {
        None
    };

    // Assign the peaks to lfo1 and lfo2.
    #[derive(Clone, Copy, PartialEq)]
    enum Kind {
        Pitch,
        Level,
        Filter,
    }
    let mut cands: Vec<(Kind, Tone)> = Vec::new();
    if let Some(tn) = pitch_tone {
        cands.push((Kind::Pitch, tn));
    }
    if let Some(tn) = loud_tone {
        cands.push((Kind::Level, tn));
    }
    if let Some(tn) = cen_tone {
        cands.push((Kind::Filter, tn));
    }
    let agrees = |a: f32, b: f32| (a / b).log2().abs() < 0.32;
    let mut r1_rate: Option<f32> = None;
    let mut r2_rate: Option<f32> = None;
    let mut filter_depth = 0.0f32;
    for (k, tn) in &cands {
        let slot = match (r1_rate, r2_rate) {
            (None, _) => {
                r1_rate = Some(tn.rate);
                1
            }
            (Some(a), _) if agrees(a, tn.rate) => 1,
            (Some(_), None) if *k != Kind::Level => {
                r2_rate = Some(tn.rate);
                2
            }
            (Some(_), Some(b)) if *k != Kind::Level && agrees(b, tn.rate) => 2,
            _ => 0,
        };
        if slot == 0 {
            continue;
        }
        if tn.cycles < 2.0 {
            prov = true;
        }
        match (*k, slot) {
            (Kind::Pitch, 1) => set(t, P::LFO1_TO_PITCH, tn.amp),
            (Kind::Pitch, _) => set(t, P::LFO2_TO_PITCH, tn.amp),
            (Kind::Level, _) => set(t, P::LFO1_TO_LEVEL, 1.0 - 10f32.powf(-2.0 * tn.amp / 20.0)),
            (Kind::Filter, 1) => {
                set(t, P::LFO1_TO_FILTER, tn.amp);
                filter_depth = tn.amp;
            }
            (Kind::Filter, _) => {
                set(t, P::LFO2_TO_FILTER, tn.amp);
                filter_depth = tn.amp;
            }
        }
    }
    if let Some(a) = r1_rate {
        set(t, P::LFO1_RATE, a);
    }
    if let Some(b) = r2_rate {
        set(t, P::LFO2_RATE, b);
    }
    if filter_depth > 0.0 {
        let mut c = cen_hz.clone();
        set(t, P::FILT_CUTOFF, 3.0 * median(&mut c));
    }
    // Wandering pitch without a peak: chaos.
    if pitch_tone.is_none() && pitch_len >= 40 && pitch_rms >= 10.0 {
        set(t, P::CHAOS_AMT, pitch_rms / CHAOS_RMS_CENTS);
        // A new random target every 1/rate s flips the sign half of the time.
        set(t, P::CHAOS_RATE, 2.0 * pitch_zc);
    }
    prov
}

/// Cents rms of the Klont's chaos source at chaos.amt 1: 50 cents times a
/// smoothed uniform random of rms about 0.5.
const CHAOS_RMS_CENTS: f32 = 25.0;

// ---------------------------------------------------------------- grit

fn fold_row(x: &[f32], sr: f32, hop: usize, db: &[f32], p: usize, pitch_cents: f32, t: &mut [f32; P::COUNT]) {
    let f0 = cents_to_hz(pitch_cents);
    if !(f0 > 40.0) {
        return;
    }
    // Crest factor over 4-period windows of the loud frames, median.
    let win = ((4.0 * sr / f0) as usize).clamp(256, 4096);
    let loud: Vec<usize> = (0..db.len()).filter(|&i| db[i] >= -12.0).collect();
    if loud.is_empty() {
        return;
    }
    let step = (loud.len() / 12).max(1);
    let mut crests: Vec<f32> = Vec::new();
    for &i in loud.iter().step_by(step) {
        let c = i * hop + hop / 2;
        if c < win / 2 || c + win / 2 > x.len() {
            continue;
        }
        let w = &x[c - win / 2..c + win / 2];
        let r = rms_of(w);
        let pkv = w.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        if r > 1e-6 {
            crests.push(pkv / r);
        }
    }
    if crests.is_empty() {
        return;
    }
    let crest = median(&mut crests);
    // Upper harmonics: share of power above 4 f0 in the loudest frame.
    let n = 4096usize;
    let fft = Fft::new(n);
    let mut wn = vec![0.0; n];
    hann(&mut wn);
    let (mut re, mut im) = (vec![0.0f32; n], vec![0.0f32; n]);
    hann_frame(x, (p * hop + hop / 2) as isize, n, &wn, &mut re, &mut im);
    fft.forward(&mut re, &mut im);
    let bin = sr / n as f32;
    // Partial levels of the loudest frame, in dB against log2 of the partial
    // number. Clean bright sounds fall on a smooth line; a wavefolder leaves
    // a rippled, flat-topped result (Bessel-like peaks and notches), so the
    // measure is the ripple around the fitted line, only where many partials
    // are audible and the waveform is flat-topped (crest under 1.8).
    let mut pts: Vec<(f32, f32)> = Vec::new();
    let mag = |j: usize| re[j].hypot(im[j]);
    let mut top = 0.0f32;
    let mut raw = [0.0f32; 40];
    for k in 1..=40usize {
        let fk = k as f32 * f0;
        if fk > 0.45 * sr {
            break;
        }
        let (a, b) = (((fk * 0.97 / bin) as usize).max(1), ((fk * 1.03 / bin).ceil() as usize + 1).min(n / 2 - 1));
        raw[k - 1] = (a..=b).map(mag).fold(0.0f32, f32::max);
        top = top.max(raw[k - 1]);
    }
    for (k, &a) in raw.iter().enumerate() {
        if a >= top * 0.003 {
            pts.push((((k + 1) as f32).log2(), 20.0 * a.log10()));
        }
    }
    if pts.len() < 20 || crest >= 1.8 {
        return;
    }
    let (ic, sl) = line_fit(pts.iter().copied());
    let ripple = (pts.iter().map(|q| (q.1 - ic - sl * q.0).powi(2)).sum::<f32>() / pts.len() as f32).sqrt();
    let fold = ((ripple - FOLD_RIPPLE) / FOLD_SPAN).clamp(0.0, 1.0);
    if fold > 0.05 {
        set(t, P::FOLD_AMT, fold);
    }
}

/// Partial-level ripple (dB rms) tolerated before it counts as fold, and the
/// ripple at which fold reads 1.
const FOLD_RIPPLE: f32 = 5.0;
const FOLD_SPAN: f32 = 5.0;

fn crush_row(x: &[f32], sr: f32, hop: usize, db: &[f32], region: (usize, usize), t: &mut [f32; P::COUNT]) {
    // Average power spectrum over up to 16 loud frames.
    let n = 4096usize;
    let fft = Fft::new(n);
    let mut wn = vec![0.0; n];
    hann(&mut wn);
    let (mut re, mut im) = (vec![0.0f32; n], vec![0.0f32; n]);
    let mut acc = vec![0.0f64; n / 2 + 1];
    let step = ((region.1 - region.0) / 16).max(1);
    let mut cnt = 0;
    for i in (region.0..region.1).step_by(step) {
        if db[i] < -20.0 {
            continue;
        }
        hann_frame(x, (i * hop + hop / 2) as isize, n, &wn, &mut re, &mut im);
        fft.forward(&mut re, &mut im);
        for j in 0..=n / 2 {
            acc[j] += (re[j] * re[j] + im[j] * im[j]) as f64;
        }
        cnt += 1;
    }
    if cnt == 0 {
        return;
    }
    let bin = sr / n as f32;
    // Level of a frequency span in dB (mean power).
    let level = |f0: f32, f1: f32| -> f32 {
        let (a, b) = (((f0 / bin) as usize).max(1), ((f1 / bin) as usize).min(n / 2));
        if b <= a {
            return -200.0;
        }
        let m = acc[a..b].iter().sum::<f64>() / (b - a) as f64 / cnt as f64;
        10.0 * (m.max(1e-30) as f32).log10()
    };
    let nyq = 0.5 * sr;
    // Third-octave steps from 150 Hz.
    let mut top = f32::MIN;
    let mut fc = 0.0f32;
    let mut f = 150.0f32;
    let mut bands: Vec<(f32, f32)> = Vec::new();
    while f * 1.26 < nyq {
        let l = level(f, f * 1.26);
        bands.push((f, l));
        top = top.max(l);
        f *= 1.26;
    }
    for &(f, l) in &bands {
        if l >= top - 35.0 {
            fc = f * 1.26;
        }
    }
    if !(fc > 0.0) || fc >= 6000.0 || fc * 1.4 >= nyq {
        return;
    }
    let below = level(fc * 0.5, fc);
    let above = level(fc * 1.4, (fc * 2.8).min(nyq));
    if below - above >= 40.0 {
        set(t, P::CRUSH_RATE, 2.2 * fc);
    }
}

// ---------------------------------------------------------------- reverb

fn verb_rows(db: &[f32], t: &mut [f32; P::COUNT]) {
    let n = db.len();
    let drop_f = 4usize; // 40 ms
    if n < 60 {
        return;
    }
    // The stop is where the loudness falls fastest over 40 ms while still
    // loud; a smooth decay has its fastest fall at the start and fails below.
    let mut best: Option<(usize, f32)> = None;
    for i in 5..n - drop_f {
        if db[i] < -20.0 {
            continue;
        }
        let d = db[i] - db[i + drop_f];
        if best.is_none_or(|b| d > b.1) {
            best = Some((i, d));
        }
    }
    let Some((q, drop_db)) = best else { return };
    if drop_db < 6.0 {
        return;
    }
    let start = q + drop_f;
    let pts: Vec<(f32, f32)> = (start..n).take_while(|&i| db[i] >= -55.0).map(|i| ((i - start) as f32 * HOP_S, db[i])).collect();
    // Longer than 300 ms of tail above the floor.
    if pts.len() < 31 {
        return;
    }
    let (icpt, slope) = line_fit(pts.iter().copied());
    // A tail only falls and is a straight line in dB: judged on a 5-frame
    // average, because the reverb's own tail wobbles by a few dB.
    let sm: Vec<f32> = (0..pts.len())
        .map(|i| {
            let (a, b) = (i.saturating_sub(2), (i + 3).min(pts.len()));
            pts[a..b].iter().map(|q| q.1).sum::<f32>() / (b - a) as f32
        })
        .collect();
    if sm.iter().any(|&v| v > sm[0] + 5.0) {
        return;
    }
    let resid = (pts.iter().zip(&sm).map(|(q, &v)| (v - icpt - slope * q.0).powi(2)).sum::<f32>() / pts.len() as f32).sqrt();
    if resid > 4.0 {
        return;
    }
    let fast = drop_db / (drop_f as f32 * HOP_S);
    if !(slope < -1.0) || -slope > fast / 3.0 {
        return;
    }
    let rt60 = 60.0 / -slope;
    if rt60 < 0.3 {
        return;
    }
    let pre = db[q.saturating_sub(4)..=q].iter().sum::<f32>() / (q - q.saturating_sub(4) + 1) as f32;
    let r = 10f32.powf((icpt - pre) / 20.0);
    set(t, P::VERB_SIZE, (rt60 / 0.3).ln() / 40f32.ln());
    set(t, P::VERB_MIX, r * VERB_MIX_PER_RATIO);
}

/// verb.mix per unit of (tail level at the stop over the level before it).
const VERB_MIX_PER_RATIO: f32 = 1.0;

// ---------------------------------------------------------------- grain

fn grain_suggestion(x: &[f32], hop: usize, db: &[f32], track: &[(f32, f32)], voiced: &impl Fn(usize) -> bool) -> GrainSuggestion {
    let nf = db.len();
    let min_f = (MIN_GRAIN_S / HOP_S) as usize;
    let max_f = (MAX_GRAIN_S / HOP_S) as usize;
    let mut loud: Vec<f32> = db.iter().copied().filter(|&d| d >= -20.0).collect();
    let med = if loud.is_empty() { 0.0 } else { median(&mut loud) };
    let (mut i0, mut i1) = longest_run(db, |d| (d - med).abs() <= 6.0).unwrap_or((0, nf));
    if i1 - i0 < min_f {
        // Too short: widen around its middle.
        let mid = (i0 + i1) / 2;
        i0 = mid.saturating_sub(min_f / 2);
        i1 = (i0 + min_f).min(nf);
        i0 = i1.saturating_sub(min_f);
    }
    if i1 - i0 > max_f {
        let mid = (i0 + i1) / 2;
        i0 = mid - max_f / 2;
        i1 = i0 + max_f;
    }
    let start = (i0 * hop).min(x.len());
    let end = (i1 * hop).min(x.len());
    let mut hz: Vec<f32> = (i0..i1).filter(|&i| voiced(i)).map(|i| track[i].0).collect();
    let ref_cents = if hz.len() as f32 >= 0.3 * (i1 - i0) as f32 && !hz.is_empty() { Some(hz_to_cents(median(&mut hz))) } else { None };
    GrainSuggestion { start, end, ref_cents }
}
