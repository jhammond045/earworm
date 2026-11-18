//! The M4 gate proxy: "blind match 70% at 100%" (TDD testing table, Influence
//! row; docs/M0.md blind test).
//!
//! THIS IS NOT THE GATE. The gate is a listening test: people hear one match
//! and four candidate sources and pick the source the match was made from, at
//! least 70% of the time over 20 varied sources with Influence at 100%. Nobody
//! can listen on the build machine, so this builds an objective stand-in: 20
//! synthetic sources made by code that is not the Klont (so the engine cannot
//! trivially match itself), each matched with `match_full`, rendered through the
//! Klont at Influence 100%, and then "identified" by the TDD's refine loss
//! (L1 between log-mel spectrograms at three FFT sizes plus half-weighted L2
//! between loudness envelopes) instead of by ears. A loss is not an ear: it
//! ignores things people hear (phase, attack character, naturalness) and hears
//! things people do not. Read the number as "the match is closer to its own
//! source than to others by this measure", not as a pass of the gate.
//!
//! Prints per-source hit rates, the total against the 70% target, and the
//! score of a neutral, unmatched render (what the test gives for no matching).
//! Asserts only that the matched total beats the neutral total.
//!
//! Run: cargo test -p analysis --release --test blind_match -- --nocapture

mod klont_util;
use analysis::{FullMatch, FullOpts, match_full};
use dsp::fft::{Fft, hann};
use klont_util::*;
use params as P;
use std::f32::consts::TAU;

// ------------------------------------------------------------ tiny DSP kit

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03)
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    fn f(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    /// Uniform in [-1, 1).
    fn w(&mut self) -> f32 {
        2.0 * self.f() - 1.0
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f()
    }
}

fn n_of(secs: f32) -> usize {
    (secs * SR) as usize
}

fn noise(n: usize, r: &mut Rng) -> Vec<f32> {
    (0..n).map(|_| r.w()).collect()
}

/// RBJ biquad, direct form I. kind: 0 low-pass, 1 high-pass, 2 band-pass (0 dB peak).
fn biquad(x: &[f32], kind: u8, fc: f32, q: f32) -> Vec<f32> {
    let w0 = TAU * fc / SR;
    let (s, c) = w0.sin_cos();
    let al = s / (2.0 * q);
    let (b0, b1, b2) = match kind {
        0 => ((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0),
        1 => ((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0),
        _ => (al, 0.0, -al),
    };
    let (a0, a1, a2) = (1.0 + al, -2.0 * c, 1.0 - al);
    let (b0, b1, b2, a1, a2) = (b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0);
    let (mut x1, mut x2, mut y1, mut y2) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    x.iter()
        .map(|&v| {
            let y = b0 * v + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
            x2 = x1;
            x1 = v;
            y2 = y1;
            y1 = y;
            y
        })
        .collect()
}

/// Paul Kellet's three-pole approximation of pink noise.
fn pink(n: usize, r: &mut Rng) -> Vec<f32> {
    let (mut b0, mut b1, mut b2) = (0.0f32, 0.0f32, 0.0f32);
    (0..n)
        .map(|_| {
            let w = r.w();
            b0 = 0.99765 * b0 + w * 0.0990460;
            b1 = 0.96300 * b1 + w * 0.2965164;
            b2 = 0.57000 * b2 + w * 1.0526913;
            b0 + b1 + b2 + w * 0.1848
        })
        .collect()
}

/// Linear attack and release ramps (seconds).
fn shape(x: &mut [f32], attack: f32, release: f32) {
    let n = x.len();
    let (a, r) = (n_of(attack).max(1), n_of(release).max(1));
    for (i, v) in x.iter_mut().enumerate() {
        let g = ((i as f32 + 0.5) / a as f32).min(1.0).min(((n - i) as f32 - 0.5) / r as f32).clamp(0.0, 1.0);
        *v *= g;
    }
}

fn peak_to(x: &mut [f32], level: f32) {
    let p = x.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-9);
    for v in x.iter_mut() {
        *v *= level / p;
    }
}

/// Karplus-Strong pluck: noise burst in a delay line with an averaging loop.
fn pluck(f0: f32, secs: f32, damp: f32, r: &mut Rng) -> Vec<f32> {
    let n = n_of(secs);
    let d = (SR / f0).round() as usize;
    let mut buf: Vec<f32> = (0..d).map(|_| r.w()).collect();
    // A soft pick: smooth the burst once.
    for i in 1..d {
        buf[i] = 0.5 * (buf[i] + buf[i - 1]);
    }
    let mut out = Vec::with_capacity(n);
    let mut p = 0;
    let mut prev = 0.0f32;
    for _ in 0..n {
        let cur = buf[p];
        let y = damp * 0.5 * (cur + prev);
        prev = cur;
        buf[p] = y;
        out.push(cur);
        p = (p + 1) % d;
    }
    out
}

/// Sum of sines with the given (ratio, amp, decay-seconds or 0 for none) at f0.
fn additive(f0: f32, secs: f32, parts: &[(f32, f32, f32)]) -> Vec<f32> {
    let n = n_of(secs);
    let mut out = vec![0.0f32; n];
    for &(ratio, a, tau) in parts {
        let f = f0 * ratio;
        if f > 0.45 * SR {
            continue;
        }
        let w = TAU * f / SR;
        for (i, o) in out.iter_mut().enumerate() {
            let env = if tau > 0.0 { (-(i as f32) / SR / tau).exp() } else { 1.0 };
            *o += a * env * (w * i as f32).sin();
        }
    }
    out
}

/// Band-limited saw by additive synthesis with a per-sample pitch in Hz.
fn saw_track(hz: impl Fn(usize) -> f32, n: usize, detune_phase: f32) -> Vec<f32> {
    let mut ph = detune_phase;
    (0..n)
        .map(|i| {
            let f = hz(i);
            ph += TAU * f / SR;
            if ph > TAU {
                ph -= TAU;
            }
            let k_max = ((0.45 * SR / f) as usize).min(48);
            (1..=k_max).map(|k| (k as f32 * ph).sin() / k as f32).sum::<f32>()
        })
        .collect()
}

// ------------------------------------------------------------ the sources

const NAMES: [&str; 20] = [
    "pluck", "bowed saw", "flute", "clarinet", "organ", "voice aa", "whistle", "bell", "marimba", "gong", "kick", "snare",
    "hi-hat", "clap", "hiss", "wind", "synth pad", "dist guitar", "tremolo sine", "reversed swell",
];

fn source(i: usize, seed: u64) -> Vec<f32> {
    let mut r = Rng::new(seed * 1000 + i as u64);
    let mut x: Vec<f32> = match i {
        0 => pluck(220.0, 1.5, 0.996, &mut r),
        1 => {
            let n = n_of(2.0);
            let mut s = saw_track(|k| 196.0 * 2f32.powf(25.0 / 1200.0 * (TAU * 5.5 * k as f32 / SR).sin()), n, 0.0);
            let bow = biquad(&noise(n, &mut r), 2, 2500.0, 0.8);
            for (a, b) in s.iter_mut().zip(bow) {
                *a += 0.15 * b;
            }
            shape(&mut s, 0.15, 0.1);
            s
        }
        2 => {
            let n = n_of(1.5);
            let mut s = additive(523.0, 1.5, &[(1.0, 1.0, 0.0), (2.0, 0.15, 0.0), (3.0, 0.05, 0.0)]);
            let breath = biquad(&noise(n, &mut r), 2, 3000.0, 0.7);
            for (a, b) in s.iter_mut().zip(breath) {
                *a += 0.25 * b;
            }
            shape(&mut s, 0.12, 0.1);
            s
        }
        3 => {
            let parts: Vec<(f32, f32, f32)> = (0..8).map(|j| (2.0 * j as f32 + 1.0, 1.0 / (2.0 * j as f32 + 1.0), 0.0)).collect();
            let mut s = additive(233.0, 1.5, &parts);
            let n = s.len();
            for (a, b) in s.iter_mut().zip(noise(n, &mut r)) {
                *a += 0.02 * b;
            }
            shape(&mut s, 0.02, 0.05);
            s
        }
        4 => {
            let mut s = additive(130.8, 1.5, &[(1.0, 1.0, 0.0), (2.0, 0.8, 0.0), (3.0, 0.6, 0.0), (4.0, 0.5, 0.0), (6.0, 0.3, 0.0), (8.0, 0.2, 0.0)]);
            shape(&mut s, 0.01, 0.02);
            s
        }
        5 => {
            // Impulses at a vibrato pitch through three parallel formant resonators.
            let n = n_of(1.5);
            let mut ph = 0.0f32;
            let mut pulses = vec![0.0f32; n];
            for (k, p) in pulses.iter_mut().enumerate() {
                let f = 140.0 * 2f32.powf(10.0 / 1200.0 * (TAU * 5.0 * k as f32 / SR).sin());
                ph += f / SR;
                if ph >= 1.0 {
                    ph -= 1.0;
                    *p = 1.0 + 0.03 * r.w();
                }
            }
            let mut s = vec![0.0f32; n];
            for &(fc, bw, g) in &[(800.0f32, 80.0f32, 1.0f32), (1200.0, 90.0, 0.6), (2600.0, 120.0, 0.3)] {
                for (a, b) in s.iter_mut().zip(biquad(&pulses, 2, fc, fc / bw)) {
                    *a += g * b;
                }
            }
            shape(&mut s, 0.05, 0.08);
            s
        }
        6 => {
            let n = n_of(1.2);
            let mut ph = 0.0f32;
            let mut s: Vec<f32> = (0..n)
                .map(|k| {
                    ph += TAU * 1760.0 * 2f32.powf(15.0 / 1200.0 * (TAU * 6.0 * k as f32 / SR).sin()) / SR;
                    ph.sin() + 0.003 * r.w()
                })
                .collect();
            shape(&mut s, 0.06, 0.08);
            s
        }
        7 => {
            let ratios = [0.5, 1.0, 1.19, 1.56, 2.0, 2.51, 3.0, 4.07, 5.4];
            let parts: Vec<(f32, f32, f32)> = ratios.iter().enumerate().map(|(k, &q)| (q, 1.0 / (1.0 + 0.3 * k as f32), 1.4 / (1.0 + 0.35 * k as f32))).collect();
            let mut s = additive(392.0, 2.0, &parts);
            shape(&mut s, 0.002, 0.02);
            s
        }
        8 => {
            let mut s = additive(330.0, 1.2, &[(1.0, 1.0, 0.45), (3.99, 0.45, 0.12), (9.9, 0.12, 0.04)]);
            shape(&mut s, 0.002, 0.02);
            s
        }
        9 => {
            let mut ratios: Vec<f32> = (0..30).map(|_| r.range(1.0, 12.0)).collect();
            ratios.sort_by(|a, b| a.total_cmp(b));
            let parts: Vec<(f32, f32, f32)> = ratios.iter().map(|&q| (q, r.range(0.3, 1.0) / q.sqrt(), r.range(1.2, 3.0))).collect();
            let mut s = additive(110.0, 2.0, &parts);
            shape(&mut s, 0.04, 0.05);
            s
        }
        10 => {
            let n = n_of(0.75);
            let mut ph = 0.0f32;
            let mut s: Vec<f32> = (0..n)
                .map(|k| {
                    let t = k as f32 / SR;
                    ph += TAU * (45.0 + 105.0 * (-t / 0.03).exp()) / SR;
                    ph.sin() * (-t / 0.2).exp()
                })
                .collect();
            shape(&mut s, 0.001, 0.02);
            s
        }
        11 => {
            let n = n_of(0.75);
            let nz = biquad(&noise(n, &mut r), 1, 1000.0, 0.7);
            let mut s: Vec<f32> = (0..n)
                .map(|k| {
                    let t = k as f32 / SR;
                    (TAU * 190.0 * t).sin() * (-t / 0.06).exp() + 0.8 * nz[k] * (-t / 0.1).exp()
                })
                .collect();
            shape(&mut s, 0.001, 0.02);
            s
        }
        12 => {
            let n = n_of(0.75);
            let nz = biquad(&biquad(&noise(n, &mut r), 1, 7000.0, 0.7), 1, 7000.0, 0.7);
            let mut s: Vec<f32> = (0..n).map(|k| nz[k] * (-(k as f32) / SR / 0.04).exp()).collect();
            shape(&mut s, 0.0005, 0.02);
            s
        }
        13 => {
            let n = n_of(0.75);
            let nz = biquad(&noise(n, &mut r), 2, 1300.0, 1.5);
            let starts = [0.0f32, 0.011, 0.022, 0.034];
            let mut s: Vec<f32> = (0..n)
                .map(|k| {
                    let t = k as f32 / SR;
                    let mut e = 0.0f32;
                    for (j, &t0) in starts.iter().enumerate() {
                        if t >= t0 {
                            let tau = if j == 3 { 0.08 } else { 0.006 };
                            e = e.max((-(t - t0) / tau).exp());
                        }
                    }
                    nz[k] * e
                })
                .collect();
            shape(&mut s, 0.0005, 0.02);
            s
        }
        14 => {
            let mut s = pink(n_of(1.5), &mut r);
            shape(&mut s, 0.05, 0.05);
            s
        }
        15 => {
            // White noise through a Chamberlin state-variable low-pass whose
            // cutoff drifts slowly, with a slow level swell.
            let n = n_of(2.0);
            let (mut lo, mut bd) = (0.0f32, 0.0f32);
            let mut s: Vec<f32> = (0..n)
                .map(|k| {
                    let t = k as f32 / SR;
                    let fc = 300.0 + 600.0 * (0.5 + 0.5 * (TAU * 0.4 * t).sin());
                    let f = 2.0 * (std::f32::consts::PI * fc / SR).sin();
                    let hi = r.w() - lo - 0.7 * bd;
                    bd += f * hi;
                    lo += f * bd;
                    lo * (0.6 + 0.4 * (TAU * 0.5 * t).sin())
                })
                .collect();
            shape(&mut s, 0.2, 0.2);
            s
        }
        16 => {
            let n = n_of(2.0);
            let mut s = vec![0.0f32; n];
            for &d in &[-7.0f32, 0.0, 7.0] {
                let f = 220.0 * 2f32.powf(d / 1200.0);
                for (a, b) in s.iter_mut().zip(saw_track(|_| f, n, r.f() * TAU)) {
                    *a += b;
                }
            }
            // Cutoff sweep: blend of low-passed copies at a few cutoffs.
            let lows: Vec<Vec<f32>> = [500.0f32, 1200.0, 3000.0].iter().map(|&fc| biquad(&biquad(&s, 0, fc, 0.7), 0, fc, 0.7)).collect();
            let out: Vec<f32> = (0..n)
                .map(|k| {
                    let u = (k as f32 / n as f32 * 2.0).min(2.0);
                    let (a, b, w) = if u < 1.0 { (0, 1, u) } else { (1, 2, u - 1.0) };
                    lows[a][k] * (1.0 - w) + lows[b][k] * w
                })
                .collect();
            let mut out = out;
            shape(&mut out, 0.3, 0.3);
            out
        }
        17 => {
            let mut s = pluck(165.0, 1.5, 0.9985, &mut r);
            peak_to(&mut s, 1.0);
            for v in s.iter_mut() {
                *v = (6.0 * *v).tanh();
            }
            s
        }
        18 => {
            let n = n_of(1.5);
            let mut s: Vec<f32> = (0..n)
                .map(|k| {
                    let t = k as f32 / SR;
                    (TAU * 330.0 * t).sin() * (1.0 - 0.7 * (0.5 - 0.5 * (TAU * 6.0 * t).sin()))
                })
                .collect();
            shape(&mut s, 0.01, 0.02);
            s
        }
        _ => {
            // A decaying cluster of tones and noise, played backwards.
            let n = n_of(1.5);
            let mut s = additive(300.0, 1.5, &[(1.0, 1.0, 0.5), (1.503, 0.8, 0.5), (2.0, 0.6, 0.45), (3.01, 0.4, 0.4)]);
            let nz = biquad(&pink(n, &mut r), 0, 4000.0, 0.7);
            for (a, b) in s.iter_mut().zip(nz) {
                *a += 0.8 * b * 0.5;
            }
            s.reverse();
            shape(&mut s, 0.02, 0.005);
            s
        }
    };
    peak_to(&mut x, 0.5);
    x
}

// ------------------------------------------------------------ the loss

const SIZES: [usize; 3] = [512, 1024, 2048];
const MELS: usize = 48;
/// Floor added to mel power before the log, about 60 dB under a unit-rms
/// signal's total power, so that the digital silence between the partials of a
/// noiseless synthetic source does not outweigh everything a person can hear.
const LOG_FLOOR: f32 = 1e-6;

struct Mel {
    /// filters[b] = (first bin, weights)
    filters: Vec<(usize, Vec<f32>)>,
}

fn hz_mel(f: f32) -> f32 {
    2595.0 * (1.0 + f / 700.0).log10()
}
fn mel_hz(m: f32) -> f32 {
    700.0 * (10f32.powf(m / 2595.0) - 1.0)
}

impl Mel {
    fn new(n: usize) -> Self {
        let (lo, hi) = (hz_mel(40.0), hz_mel(16000.0));
        let pts: Vec<f32> = (0..MELS + 2).map(|i| mel_hz(lo + (hi - lo) * i as f32 / (MELS + 1) as f32)).collect();
        let bin = SR / n as f32;
        let filters = (0..MELS)
            .map(|b| {
                let (l, c, h) = (pts[b], pts[b + 1], pts[b + 2]);
                let first = (l / bin).floor() as usize;
                let last = ((h / bin).ceil() as usize).min(n / 2);
                let w: Vec<f32> = (first..=last)
                    .map(|j| {
                        let f = j as f32 * bin;
                        if f <= l || f >= h { 0.0 } else if f < c { (f - l) / (c - l) } else { (h - f) / (h - c) }
                    })
                    .collect();
                (first, w)
            })
            .collect();
        Mel { filters }
    }
}

struct Prepared {
    /// [size][frame][band] natural log of the mel power.
    logmel: Vec<Vec<[f32; MELS]>>,
    /// Loudness per 10 ms, ln of power.
    loud: Vec<f32>,
}

/// Time-align (start at the onset), level-normalise (rms 0.1) and measure.
fn prepare(x: &[f32], mels: &[Mel], ffts: &[Fft]) -> Prepared {
    let blk = n_of(0.01);
    let nb = x.len() / blk;
    let rms_b: Vec<f32> = (0..nb).map(|i| rms(&x[i * blk..(i + 1) * blk])).collect();
    let pk = rms_b.iter().cloned().fold(0.0f32, f32::max).max(1e-9);
    let on = rms_b.iter().position(|&r| r >= pk * 10f32.powf(-35.0 / 20.0)).unwrap_or(0);
    let mut y: Vec<f32> = x[on * blk..].to_vec();
    let g = 0.1 / rms(&y).max(1e-9);
    y.iter_mut().for_each(|v| *v *= g);
    let loud: Vec<f32> = (0..y.len() / blk).map(|i| (rms(&y[i * blk..(i + 1) * blk]).powi(2) + 1e-9).ln()).collect();
    let mut logmel = Vec::new();
    for (si, &n) in SIZES.iter().enumerate() {
        let mut win = vec![0.0f32; n];
        hann(&mut win);
        let wsum: f32 = win.iter().map(|w| w * w).sum();
        let hop = n / 4;
        let (mut re, mut im) = (vec![0.0f32; n], vec![0.0f32; n]);
        let mut frames = Vec::new();
        let mut s = 0;
        while s + n <= y.len() {
            for j in 0..n {
                re[j] = y[s + j] * win[j];
                im[j] = 0.0;
            }
            ffts[si].forward(&mut re, &mut im);
            let mut f = [0.0f32; MELS];
            for (b, (first, w)) in mels[si].filters.iter().enumerate() {
                let mut e = 0.0f32;
                for (k, wk) in w.iter().enumerate() {
                    let j = first + k;
                    if j <= n / 2 {
                        e += wk * (re[j] * re[j] + im[j] * im[j]);
                    }
                }
                f[b] = (e / wsum + LOG_FLOOR).ln();
            }
            frames.push(f);
            s += hop;
        }
        logmel.push(frames);
    }
    Prepared { logmel, loud }
}

/// The TDD refine loss between two prepared signals, length-trimmed.
fn loss(a: &Prepared, b: &Prepared) -> f32 {
    let mut l = 0.0f32;
    for s in 0..SIZES.len() {
        let n = a.logmel[s].len().min(b.logmel[s].len());
        if n == 0 {
            l += 20.0;
            continue;
        }
        let mut sum = 0.0f32;
        for f in 0..n {
            for k in 0..MELS {
                sum += (a.logmel[s][f][k] - b.logmel[s][f][k]).abs();
            }
        }
        l += sum / (n * MELS) as f32;
    }
    let n = a.loud.len().min(b.loud.len());
    let l2 = if n == 0 { 0.0 } else { ((0..n).map(|i| (a.loud[i] - b.loud[i]).powi(2)).sum::<f32>() / n as f32).sqrt() };
    l + 0.5 * l2
}

// ------------------------------------------------------------ the run

struct Setup {
    mels: Vec<Mel>,
    ffts: Vec<Fft>,
}

fn setup() -> Setup {
    Setup { mels: SIZES.iter().map(|&n| Mel::new(n)).collect(), ffts: SIZES.iter().map(|&n| Fft::new(n)).collect() }
}

/// Render the match of `src` (Influence 100%, full mask, neutral base), a note
/// at the matched pitch (C4 for unpitched sources) for the source's length.
fn render_match(src: &[f32], m: &FullMatch, with_grain: bool) -> Vec<f32> {
    let spec = LayerSpec { base: P::neutral_set(), influence: Some((m.target, 1.0, MASK_ALL)) };
    let total = src.len() as f32 / SR;
    let hold = m.duration_s.min(total);
    let cents = if m.pitched { m.pitch_cents } else { 4800.0 };
    let grain_bytes_v;
    let grain = if with_grain && m.use_sound {
        let g = m.grain.expect("grain suggestion");
        grain_bytes_v = grain_bytes(SR, g.ref_cents, &src[g.start..g.end]);
        Some((&grain_bytes_v[..], 4u32))
    } else {
        None
    };
    render_note(&spec, grain, cents, hold, total)
}

/// What no matching gives: the neutral layer, no Influence, a C4 note.
fn render_neutral(len: usize) -> Vec<f32> {
    let total = len as f32 / SR;
    render_note(&LayerSpec::base(P::neutral_set()), None, 4800.0, total * 0.8, total)
}

struct Score {
    per_source: Vec<usize>,
    trials: usize,
}

impl Score {
    fn pct(&self) -> f32 {
        100.0 * self.per_source.iter().sum::<usize>() as f32 / (self.trials * self.per_source.len()) as f32
    }
}

/// Distances D[i][j] from render i to source j, then the seeded four-way
/// trials: each source against 3 distractors drawn from the other 19.
fn score(renders: &[Vec<f32>], sources: &[Prepared], su: &Setup, trials: usize, seed: u64) -> Score {
    let n = sources.len();
    let prep: Vec<Prepared> = renders.iter().map(|r| prepare(r, &su.mels, &su.ffts)).collect();
    let d: Vec<Vec<f32>> = (0..n).map(|i| (0..n).map(|j| loss(&prep[i], &sources[j])).collect()).collect();
    let mut rng = Rng::new(seed);
    let mut per_source = vec![0usize; n];
    for i in 0..n {
        for _ in 0..trials {
            let mut others: Vec<usize> = (0..n).filter(|&j| j != i).collect();
            let mut pick = Vec::new();
            for _ in 0..3 {
                let k = (rng.next_u64() % others.len() as u64) as usize;
                pick.push(others.swap_remove(k));
            }
            if pick.iter().all(|&j| d[i][i] < d[i][j]) {
                per_source[i] += 1;
            }
        }
    }
    Score { per_source, trials }
}

fn report(label: &str, s: &Score) -> f32 {
    let total: usize = s.per_source.iter().sum();
    let all = s.trials * s.per_source.len();
    println!("--- {label}");
    for (i, h) in s.per_source.iter().enumerate() {
        println!("  {:<15} {}/{}  {:>3.0}%", NAMES[i], h, s.trials, 100.0 * *h as f32 / s.trials as f32);
    }
    let pct = s.pct();
    println!("  TOTAL           {total}/{all}  {pct:.1}%  (target 70%: {})", if pct >= 70.0 { "reached" } else { "not reached" });
    pct
}

struct Totals {
    synth_only: f32,
    neutral: f32,
    /// With the grain cloud: default use_sound rules, and use_sound on everywhere.
    grain_default: Option<f32>,
    grain_all: Option<f32>,
}

/// One full run: 20 sources made with `seed`, 5 trials each with `trial_seed`.
fn run(seed: u64, trial_seed: u64, verbose: bool) -> Totals {
    let su = setup();
    let sources: Vec<Vec<f32>> = (0..20).map(|i| source(i, seed)).collect();
    let prep_src: Vec<Prepared> = sources.iter().map(|s| prepare(s, &su.mels, &su.ffts)).collect();
    let quiet = |label: &str, s: &Score| {
        if verbose {
            report(label, s)
        } else {
            s.pct()
        }
    };

    // Synthesis only: use_sound off for every source.
    let off = FullOpts { decaying: false, use_sound: Some(false) };
    let ms: Vec<FullMatch> = sources.iter().map(|s| match_full(s, SR, off)).collect();
    if verbose {
        for (i, m) in ms.iter().enumerate() {
            println!(
                "  match {:<15} pitched={} cents={:>6.0} dur={:.2}s voiced={:.2} prov={}",
                NAMES[i], m.pitched, m.pitch_cents, m.duration_s, m.voiced_fraction, m.provisional
            );
        }
    }
    let renders: Vec<Vec<f32>> = sources.iter().zip(&ms).map(|(s, m)| render_match(s, m, false)).collect();
    let synth_only = quiet("matched, synthesis only (use_sound off, no grain cloud)", &score(&renders, &prep_src, &su, 5, trial_seed));

    let neutral: Vec<Vec<f32>> = sources.iter().map(|s| render_neutral(s.len())).collect();
    let neutral = quiet("neutral baseline (unmatched render, no Influence)", &score(&neutral, &prep_src, &su, 5, trial_seed));

    let (mut grain_default, mut grain_all) = (None, None);
    if engine_has_grain() {
        let ms2: Vec<FullMatch> = sources.iter().map(|s| match_full(s, SR, FullOpts::default())).collect();
        let r2: Vec<Vec<f32>> = sources.iter().zip(&ms2).map(|(s, m)| render_match(s, m, true)).collect();
        if verbose {
            let used: Vec<&str> = (0..20).filter(|&i| ms2[i].use_sound).map(|i| NAMES[i]).collect();
            println!("use_sound on by default for: {}", used.join(", "));
        }
        grain_default = Some(quiet("matched, default use_sound rules (unpitched on) with the grain cloud", &score(&r2, &prep_src, &su, 5, trial_seed)));
        let on = FullOpts { decaying: false, use_sound: Some(true) };
        let r3: Vec<Vec<f32>> = sources.iter().map(|s| render_match(s, &match_full(s, SR, on), true)).collect();
        grain_all = Some(quiet("matched, use_sound on for every source, with the grain cloud", &score(&r3, &prep_src, &su, 5, trial_seed)));
    } else if verbose {
        println!("(this build of the Klont has no grain cloud: the with-grain runs were skipped)");
    }
    Totals { synth_only, neutral, grain_default, grain_all }
}

#[test]
fn blind_match_proxy() {
    println!("\nBlind-match PROXY: an objective stand-in for the listening gate, NOT the gate.");
    println!("20 synthetic sources (not made by the Klont), Influence 100%, 5 seeded trials each, 3 distractors, refine loss as the judge.");
    println!("Klont grain cloud present in this build: {}", engine_has_grain());
    let a = run(1, 4242, true);

    // The analysis was tuned while looking at seed 1. A second set of
    // sources (same 20 recipes, different random draws and trials) is a small
    // check that the numbers are not just that one draw.
    let b = run(2, 777, false);
    println!("\nSecond draw (sources seed 2, trials seed 777): synthesis only {:.1}%, neutral {:.1}%, grain default {}, grain all {}",
        b.synth_only, b.neutral, b.grain_default.map_or("n/a".into(), |v| format!("{v:.1}%")), b.grain_all.map_or("n/a".into(), |v| format!("{v:.1}%")));
    println!(
        "\nSummary (draw 1): matched {:.1}% (synthesis only), neutral {:.1}%, target 70%, chance for four-way 25%; with grain cloud {} (default rules) / {} (use_sound everywhere)\n",
        a.synth_only, a.neutral, a.grain_default.map_or("n/a".into(), |v| format!("{v:.1}%")), a.grain_all.map_or("n/a".into(), |v| format!("{v:.1}%"))
    );
    assert!(a.synth_only > a.neutral, "matching must beat the neutral render: {} vs {}", a.synth_only, a.neutral);
    assert!(b.synth_only > b.neutral);
}
