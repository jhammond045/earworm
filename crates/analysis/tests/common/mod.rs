//! Shared helpers for the synthetic corpora: seeded RNG, a hum synthesizer, a
//! beatbox synthesizer, the 25-float record builder and note matching.
#![allow(dead_code)]

use dsp::capture::frames_from_signal;
use dsp::frame::{DEFAULT_BANDS, frame_len};
use dsp::hz_to_cents;

pub const SR: f32 = 48_000.0;
pub use analysis::second::STRIDE;

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03)
    }
    pub fn next_u64(&mut self) -> u64 {
        // splitmix64
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    pub fn f(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f()
    }
    pub fn int(&mut self, lo: usize, hi: usize) -> usize {
        lo + (self.next_u64() % (hi - lo + 1) as u64) as usize
    }
    pub fn chance(&mut self, p: f32) -> bool {
        self.f() < p
    }
    /// Standard normal.
    pub fn normal(&mut self) -> f32 {
        let u = self.f().max(1e-7);
        let v = self.f();
        (-2.0 * u.ln()).sqrt() * (std::f32::consts::TAU * v).cos()
    }
    /// White noise in [-1, 1).
    pub fn noise(&mut self) -> f32 {
        self.f() * 2.0 - 1.0
    }
}

/// The 24-float frames from the live tier with the tuned slot appended (NaN).
pub fn records(audio: &[f32], sr: f32) -> Vec<f32> {
    let fr = frames_from_signal(audio, sr, DEFAULT_BANDS);
    let mut out = Vec::with_capacity(fr.len() * STRIDE);
    for f in &fr {
        out.extend_from_slice(&f[..frame_len(DEFAULT_BANDS)]);
        out.push(f32::NAN);
    }
    out
}

// ---------------------------------------------------------------- hums

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Vowel {
    Hum,
    Oo,
    Aa,
}

impl Vowel {
    fn formants(self) -> &'static [(f32, f32, f32)] {
        // (centre, bandwidth, gain)
        match self {
            Vowel::Hum => &[(250.0, 80.0, 1.0), (1000.0, 200.0, 0.15), (2200.0, 300.0, 0.05)],
            Vowel::Oo => &[(300.0, 70.0, 1.0), (870.0, 110.0, 0.5), (2250.0, 250.0, 0.1)],
            Vowel::Aa => &[(700.0, 100.0, 1.0), (1150.0, 120.0, 0.7), (2600.0, 250.0, 0.2)],
        }
    }
}

#[derive(Clone, Debug)]
pub struct Voice {
    pub name: &'static str,
    pub lo_hz: f32,
    pub hi_hz: f32,
    pub breath: f32,
    pub jitter: f32,
    pub shimmer: f32,
    /// Formant frequencies scale with the size of the vocal tract.
    pub formant_scale: f32,
}

pub fn voices() -> Vec<Voice> {
    let v = |name, lo_hz, hi_hz, breath, jitter, shimmer, formant_scale| Voice { name, lo_hz, hi_hz, breath, jitter, shimmer, formant_scale };
    vec![
        v("bass", 85.0, 175.0, 0.05, 0.004, 0.03, 0.9),
        v("baritone", 100.0, 200.0, 0.08, 0.004, 0.04, 0.95),
        v("tenor", 130.0, 260.0, 0.06, 0.003, 0.03, 1.0),
        v("high tenor", 160.0, 330.0, 0.10, 0.004, 0.04, 1.0),
        v("alto", 170.0, 340.0, 0.12, 0.005, 0.05, 1.1),
        v("mezzo", 200.0, 400.0, 0.10, 0.004, 0.04, 1.15),
        v("soprano", 260.0, 520.0, 0.12, 0.005, 0.05, 1.2),
        v("high female", 300.0, 640.0, 0.15, 0.006, 0.05, 1.25),
        v("child", 330.0, 700.0, 0.18, 0.008, 0.06, 1.35),
        v("breathy baritone", 105.0, 210.0, 0.45, 0.010, 0.10, 0.95),
        v("rough bass", 90.0, 180.0, 0.20, 0.020, 0.12, 0.9),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Link {
    /// First note.
    Start,
    /// Pitch glides across the boundary, no level change.
    Legato(f32),
    /// Silence between the notes (seconds).
    Detached(f32),
    /// Same sound, level dips by `db` for `len` seconds.
    Dip(f32, f32),
    /// "da": level dips, with a noise burst at the new note.
    Da(f32, f32),
    /// Long slide (seconds), no level change.
    Slide(f32),
}

#[derive(Clone, Debug)]
pub struct TrueNote {
    pub onset: f32,
    pub offset: f32,
    /// Cents above C0.
    pub cents: f32,
    pub link: Link,
    pub scoop: f32,
}

pub struct Hum {
    pub audio: Vec<f32>,
    pub notes: Vec<TrueNote>,
    pub desc: String,
}

const MAJOR: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
const MINOR: [i32; 7] = [0, 2, 3, 5, 7, 8, 10];
const PENT: [i32; 5] = [0, 2, 4, 7, 9];

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// A random hum by `voice`, seeded.
pub fn make_hum(voice: &Voice, seed: u64) -> Hum {
    let mut r = Rng::new(seed);
    let sr = SR;
    let vowel = [Vowel::Hum, Vowel::Oo, Vowel::Aa][r.int(0, 2)];
    let n_notes = r.int(3, 12);
    let scale: &[i32] = [&MAJOR[..], &MINOR[..], &PENT[..]][r.int(0, 2)];
    let tonic_hz = r.range(voice.lo_hz, voice.lo_hz * 1.15);
    // Singer's offset from concert pitch.
    let offset = r.range(-40.0, 40.0);
    let tonic_c = (hz_to_cents(tonic_hz) / 100.0).round() * 100.0 + offset;
    // Degrees the voice can sing: from a little under its low end to a little over its high end.
    let deg_c = |d: i32| tonic_c + 1200.0 * d.div_euclid(scale.len() as i32) as f32 + 100.0 * scale[d.rem_euclid(scale.len() as i32) as usize] as f32;
    let (min_deg, max_deg) = {
        let ok = |d: i32| {
            let hz = dsp::cents_to_hz(deg_c(d));
            hz >= voice.lo_hz * 0.95 && hz <= voice.hi_hz * 1.15
        };
        let lo = (-20..0).find(|&d| ok(d)).unwrap_or(0);
        let hi = (0..40).rev().find(|&d| ok(d)).unwrap_or(0);
        (lo.min(hi), hi.max(lo + 2))
    };
    let vib_depth = if r.chance(0.35) { 0.0 } else { r.range(15.0, 70.0) };
    let vib_rate = r.range(4.3, 7.0);
    let style = r.int(0, 3); // 0 legato, 1 detached, 2 mixed, 3 articulated (dips and das)
    let scoopy = r.chance(0.4);
    let amp = r.range(0.15, 0.7);

    // Note plan.
    let mut deg = r.int(0, ((max_deg - min_deg) / 2) as usize) as i32 + min_deg;
    let mut specs: Vec<(f32, f32, i32, Link, f32)> = Vec::new(); // (start, dur, degree, link, scoop depth)
    let mut t = r.range(0.15, 0.4);
    let target_total = r.range(2.0, 8.0);
    let mean_len = ((target_total - t) / n_notes as f32).clamp(0.2, 1.0);
    for i in 0..n_notes {
        let dur = (mean_len * r.range(0.6, 1.4)).clamp(0.18, 1.3);
        let same = i > 0 && r.chance(0.28);
        if i > 0 && !same {
            let step = r.int(1, 4) as i32 * if r.chance(0.5) { 1 } else { -1 };
            deg = (deg + step).clamp(min_deg, max_deg);
            // Avoid a no-op clamp turning a step into a repeat.
            if deg == specs.last().unwrap().2 {
                deg = (deg + 2).min(max_deg);
                if deg == specs.last().unwrap().2 {
                    deg -= 2;
                }
            }
        }
        let last_deg = specs.last().map(|s| s.2);
        let same = last_deg == Some(deg);
        let link = if i == 0 {
            Link::Start
        } else if same {
            // Repeated pitch needs a gap, dip or consonant.
            match r.int(0, 2) {
                0 => Link::Detached(r.range(0.07, 0.2)),
                1 => Link::Dip(r.range(8.0, 16.0), r.range(0.04, 0.08)),
                _ => Link::Da(r.range(8.0, 18.0), r.range(0.03, 0.06)),
            }
        } else {
            let pick = match style {
                0 => r.int(0, 2),
                1 => 3,
                2 => r.int(0, 4),
                _ => r.int(3, 4),
            };
            match pick {
                0 => Link::Legato(r.range(0.02, 0.06)),
                1 => Link::Slide(r.range(0.1, 0.22)),
                2 => Link::Legato(r.range(0.02, 0.05)),
                3 => Link::Detached(r.range(0.06, 0.2)),
                _ => {
                    if r.chance(0.5) {
                        Link::Dip(r.range(8.0, 16.0), r.range(0.04, 0.08))
                    } else {
                        Link::Da(r.range(8.0, 18.0), r.range(0.03, 0.06))
                    }
                }
            }
        };
        let scoop = match link {
            Link::Start | Link::Detached(_) | Link::Da(..) | Link::Dip(..) if scoopy && r.chance(0.6) => {
                r.range(60.0, 200.0) * if r.chance(0.7) { -1.0 } else { 1.0 }
            }
            _ => 0.0,
        };
        // Slides and legato glides eat into the previous note's tail.
        let start = if i == 0 {
            t
        } else {
            let (ps, pd, ..) = *specs.last().unwrap();
            ps + pd
        };
        let start = match link {
            Link::Detached(g) => start + g,
            _ => start,
        };
        specs.push((start, dur, deg, link, scoop));
        t = start + dur;
    }
    let total = t + 0.3;
    let n = (total * sr) as usize;

    // Pitch contour, per sample, in cents.
    let target = |d: i32| -> f32 {
        let octave = d.div_euclid(scale.len() as i32);
        let idx = d.rem_euclid(scale.len() as i32) as usize;
        tonic_c + 1200.0 * octave as f32 + 100.0 * scale[idx] as f32
    };
    // Per-note small intonation error and slow drift.
    let errs: Vec<f32> = specs.iter().map(|_| r.range(-12.0, 12.0)).collect();
    let drift_a: Vec<f32> = specs.iter().map(|_| r.range(-25.0, 25.0)).collect();
    let mut cents = vec![0.0f32; n];
    let mut gain = vec![0.0f32; n];
    let mut burst = vec![0.0f32; n];
    let mut truth = Vec::new();
    let att = r.range(0.006, 0.03);
    let rel = r.range(0.02, 0.06);
    for (i, &(start, dur, d, link, scoop)) in specs.iter().enumerate() {
        let c = target(d) + errs[i];
        let end = start + dur;
        let s0 = (start * sr) as usize;
        let s1 = ((end * sr) as usize).min(n);
        let next = specs.get(i + 1);
        for s in s0..s1 {
            let tt = s as f32 / sr - start;
            let mut p = c + drift_a[i] * (tt / dur - 0.5);
            if scoop != 0.0 {
                let sl = 0.09;
                p -= scoop * (1.0 - smooth(tt / sl));
            }
            // Vibrato fades in over 0.2 s.
            p += vib_depth * smooth((tt - 0.1) / 0.25) * (std::f32::consts::TAU * vib_rate * tt).sin();
            cents[s] = p;
            // Level: attack, sustain with slight swell, release.
            let a = smooth(tt / att);
            let rr = smooth((end - s as f32 / sr) / rel);
            let sw = 1.0 + 0.15 * (std::f32::consts::PI * tt / dur).sin();
            gain[s] = a * rr * sw;
        }
        // Boundary shaping for links into the next note.
        if let Some(&(ns, _, nd, nlink, _)) = next {
            let nc = target(nd) + errs[i + 1];
            let b = end; // boundary time
            match nlink {
                Link::Legato(g) | Link::Slide(g) => {
                    // Crossfade pitch over g centred at b; level untouched. Extend note i's region
                    // to the boundary, then glide.
                    let g0 = ((b - g * 0.5) * sr) as usize;
                    let g1 = (((b + g * 0.5) * sr) as usize).min(n);
                    for s in g0..g1 {
                        let x = smooth((s - g0) as f32 / (g1 - g0).max(1) as f32);
                        let from = c;
                        cents[s] = from + (nc - from) * x;
                        gain[s] = 1.0;
                    }
                    let _ = ns;
                }
                _ => {}
            }
        }
        truth.push(TrueNote { onset: start, offset: end, cents: c, link, scoop });
    }
    // Dips and das: the previous note released to zero at the boundary and the new one attacks from
    // zero, so rebuild the level around the boundary as a plateau with a dip of the stated depth.
    for i in 1..specs.len() {
        let (start, _, _, link, _) = specs[i];
        if let Link::Dip(db, w) | Link::Da(db, w) = link {
            let g = 10f32.powf(-db / 20.0);
            let s0 = (((start - w) * sr).max(0.0)) as usize;
            let s1 = (((start + w) * sr) as usize).min(n);
            for s in s0..s1 {
                let x = (s as f32 / sr - start) / w;
                let shape = 0.5 + 0.5 * (std::f32::consts::PI * x).cos();
                gain[s] = 1.0 - (1.0 - g) * shape;
            }
            if let Link::Da(..) = link {
                let b0 = (start * sr) as usize;
                let b1 = (((start + 0.014) * sr) as usize).min(n);
                for s in b0..b1 {
                    burst[s] = 0.6 * smooth((s - b0) as f32 / 40.0) * smooth((b1 - s) as f32 / 200.0);
                }
            }
        }
    }

    // Voice synthesis.
    let formants: Vec<(f32, f32, f32)> =
        vowel.formants().iter().map(|&(f, b, g)| (f * voice.formant_scale, b * voice.formant_scale, g)).collect();
    let table: Vec<f32> = (0..6000)
        .map(|hz| {
            let mut g = 0.08;
            for &(fc, bw, gn) in &formants {
                let d = (hz as f32 - fc) / bw;
                g += gn / (1.0 + d * d);
            }
            g
        })
        .collect();
    let mut audio = vec![0.0f32; n];
    let mut phase = 0.0f32;
    let mut jit = 0.0f32;
    let mut shim = 1.0f32;
    let mut cycle = 0.0f32;
    let mut breath_lp = 0.0f32;
    let mut noise_lp = 0.0f32;
    for s in 0..n {
        let hz = dsp::cents_to_hz(if cents[s] == 0.0 { target(0) } else { cents[s] });
        let f = hz * (1.0 + jit);
        phase += f / sr;
        if phase >= 1.0 {
            phase -= 1.0;
            // New cycle: new jitter and shimmer draw.
            jit = voice.jitter * r.normal() * 0.7;
            shim = 1.0 + voice.shimmer * r.normal() * 0.7;
            cycle += 1.0;
        }
        let g = gain[s];
        let mut x = 0.0;
        if g > 1e-4 {
            let mut k = 1;
            while (k as f32) * hz < 5000.0 {
                let gk = table[((k as f32 * hz) as usize).min(5999)];
                x += gk / (k as f32).powf(1.6) * (std::f32::consts::TAU * k as f32 * phase).sin();
                k += 1;
            }
            x *= g * shim;
        }
        // Breath: noise high-passed (difference of two smoothings), scaled with the level.
        let w = r.noise();
        breath_lp += 0.3 * (w - breath_lp);
        noise_lp += 0.02 * (breath_lp - noise_lp);
        let br = (breath_lp - noise_lp) * voice.breath * 0.35 * g.min(1.2);
        audio[s] = amp * (0.35 * x + br) + amp * burst[s] * w * 0.8;
    }
    let _ = cycle;
    // Mic noise floor.
    let floor = 10f32.powf(r.range(-68.0, -56.0) / 20.0);
    // Room noise: broadband floor plus a low rumble (fan, traffic) a good deal louder.
    let rumble = 10f32.powf(r.range(-58.0, -46.0) / 20.0);
    let mut lp = 0.0f32;
    for a in audio.iter_mut() {
        lp += 0.02 * (r.normal() - lp);
        *a += floor * r.normal() + rumble * lp * 4.0;
    }
    let desc = format!("{} {:?} {} notes vib {:.0}c@{:.1}Hz style {} scoop {}", voice.name, vowel, n_notes, vib_depth, vib_rate, style, scoopy);
    Hum { audio, notes: truth, desc }
}

#[allow(unused_imports)]
pub use analysis::score::{NoteScore, match_notes};

// ---------------------------------------------------------------- flex anchors

pub mod flex_map;
pub mod flex_takes;

/// How well found anchor times match true onsets: each true time is matched to at most one
/// found time within 50 ms, nearest pairs first (as the flex prototype matched them).
#[derive(Clone, Debug, Default)]
pub struct AnchorScore {
    pub truth: usize,
    /// Signed errors of the matched true times, found minus true, ms.
    pub errs: Vec<f64>,
}

impl AnchorScore {
    pub fn add(&mut self, found: &[f64], truth: &[f64]) {
        let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
        for (i, f) in found.iter().enumerate() {
            for (j, t) in truth.iter().enumerate() {
                let d = (f - t).abs();
                if d <= 0.05 {
                    pairs.push((d, i, j));
                }
            }
        }
        pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (mut uf, mut ut) = (vec![false; found.len()], vec![false; truth.len()]);
        for (_, i, j) in pairs {
            if !uf[i] && !ut[j] {
                uf[i] = true;
                ut[j] = true;
                self.errs.push((found[i] - truth[j]) * 1000.0);
            }
        }
        self.truth += truth.len();
    }
    pub fn found_share(&self) -> f64 {
        self.errs.len() as f64 / self.truth.max(1) as f64
    }
    pub fn mean_ms(&self) -> f64 {
        self.errs.iter().map(|e| e.abs()).sum::<f64>() / self.errs.len().max(1) as f64
    }
    pub fn max_ms(&self) -> f64 {
        self.errs.iter().fold(0.0f64, |m, e| m.max(e.abs()))
    }
    pub fn line(&self) -> String {
        format!("{} of {} found ({:.1}%), mean error {:.2} ms, max {:.1} ms", self.errs.len(), self.truth, 100.0 * self.found_share(), self.mean_ms(), self.max_ms())
    }
}
