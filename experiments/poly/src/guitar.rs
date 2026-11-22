//! A synthetic guitar with an exact list of the notes it plays.
//!
//! Each pluck is an additive modal string: inharmonic partials
//! (f_m = m f0 sqrt(1 + B m^2)), the comb of the pluck position, a pick
//! low-pass that opens with velocity, two polarizations that beat and decay
//! at different rates, decay that grows with frequency, a small pitch glide at
//! the attack, pick noise, and either an acoustic body or an electric pickup.
//! A string sounds one note at a time: a new pluck or a damp cuts the last.

use std::f64::consts::PI;

pub const SR: f64 = 48_000.0;
/// Open strings, low to high: E2 A2 D3 G3 B3 E4.
pub const OPEN: [u8; 6] = [40, 45, 50, 55, 59, 64];
/// Inharmonicity of each open string; fretting shortens it and raises B.
const B_OPEN: [f64; 6] = [6e-5, 5e-5, 4e-5, 8e-5, 1e-4, 8e-5];
/// Frequency-independent decay of each open string, 1/s.
const SUSTAIN: [f64; 6] = [0.45, 0.5, 0.6, 0.7, 0.8, 0.9];

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn f(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.f()
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Acoustic,
    Electric,
}

/// One note as played: the truth the detectors are scored against.
#[derive(Clone, Copy, Debug)]
pub struct Note {
    pub midi: u8,
    pub hz: f64,
    pub on: f64,
    pub off: f64,
    pub vel_db: f64,
}

#[derive(Clone, Copy)]
struct Pluck {
    string: usize,
    fret: u8,
    t: f64,
    vel_db: f64,
}

/// Fret per string, low E first; -1 is a string not played.
pub type Shape = [i8; 6];

pub struct Score {
    plucks: Vec<Pluck>,
    damps: Vec<(f64, u8)>,
    pub len: f64,
    /// Start time and name of each section, for per-section figures.
    pub sections: Vec<(f64, &'static str)>,
}

const ALL: u8 = 0b11_1111;

impl Score {
    fn pluck(&mut self, string: usize, fret: u8, t: f64, vel_db: f64) {
        self.plucks.push(Pluck { string, fret, t, vel_db });
    }

    /// A strum across the shape's strings. `top` limits it to the highest
    /// `top` strings (an up-strum that catches only the treble).
    fn strum(&mut self, shape: &Shape, t: f64, down: bool, top: Option<usize>, base_db: f64, rng: &mut Rng) {
        let mut strings: Vec<usize> = (0..6).filter(|&s| shape[s] >= 0).collect();
        if let Some(k) = top {
            strings = strings[strings.len().saturating_sub(k)..].to_vec();
        }
        if !down {
            strings.reverse();
        }
        let gap = rng.range(0.003, 0.008);
        for (i, &s) in strings.iter().enumerate() {
            let jitter = rng.range(-0.0015, 0.0015);
            let db = base_db + rng.range(-3.0, 3.0) - if down { 0.0 } else { 2.0 };
            self.pluck(s, shape[s] as u8, t + i as f64 * gap + jitter.max(0.0), db);
        }
    }

    fn damp(&mut self, t: f64, mask: u8) {
        self.damps.push((t, mask));
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Set {
    /// Material the thresholds were tuned on.
    Dev,
    /// Held-out material: other chords, other lines, other random draws.
    Test,
}

struct Material {
    chords: Vec<Shape>,
    progression: Vec<Shape>,
    arpeggios: Vec<(Shape, [usize; 8])>,
    /// Steps of a line: the (string, fret) pairs struck together, and how long they ring.
    line: Vec<(Vec<(usize, u8)>, f64)>,
    chug: Shape,
}

fn material(set: Set) -> Material {
    match set {
        Set::Dev => Material {
            chords: vec![
                [0, 2, 2, 1, 0, 0],     // E
                [-1, 0, 2, 2, 1, 0],    // Am
                [-1, 3, 2, 0, 1, 0],    // C
                [3, 2, 0, 0, 0, 3],     // G
                [-1, -1, 0, 2, 3, 2],   // D
                [1, 3, 3, 2, 1, 1],     // F barre
                [-1, 2, 4, 4, 3, 2],    // Bm barre
                [0, 2, 2, 0, 0, 0],     // Em
                [0, 2, 2, -1, -1, -1],  // E5
                [-1, 0, 2, -1, -1, -1], // A5
                [-1, -1, -1, 4, 5, -1], // B3 E4
                [-1, -1, -1, 7, 8, 7],  // D4 G4 B4
            ],
            progression: vec![[3, 2, 0, 0, 0, 3], [-1, 3, 2, 0, 1, 0], [-1, -1, 0, 2, 3, 2], [0, 2, 2, 0, 0, 0]],
            arpeggios: vec![
                ([-1, 0, 2, 2, 1, 0], [1, 3, 4, 5, 4, 3, 2, 3]),
                ([-1, 3, 2, 0, 1, 0], [1, 3, 4, 5, 4, 3, 2, 3]),
                ([3, 2, 0, 0, 0, 3], [0, 3, 4, 5, 4, 3, 1, 3]),
            ],
            line: vec![
                (vec![(3, 2)], 0.25),
                (vec![(3, 4)], 0.25),
                (vec![(4, 1)], 0.25),
                (vec![(4, 3)], 0.25),
                (vec![(5, 0)], 0.25),
                (vec![(5, 3)], 0.5),
                (vec![(3, 4), (4, 5)], 0.4),
                (vec![(3, 2), (4, 3)], 0.4),
                (vec![(2, 2), (3, 2)], 0.4),
                (vec![(1, 2)], 0.25),
                (vec![(1, 0)], 0.25),
                (vec![(0, 3)], 0.25),
                (vec![(0, 0)], 0.6),
            ],
            chug: [0, 2, 2, -1, -1, -1],
        },
        Set::Test => Material {
            chords: vec![
                [-1, 0, 2, 2, 2, 0],    // A
                [-1, -1, 0, 2, 3, 1],   // Dm
                [0, 2, 0, 1, 0, 0],     // E7
                [-1, 3, 2, 0, 0, 0],    // Cmaj7
                [3, 2, 0, 0, 0, 1],     // G7
                [5, 7, 7, 6, 5, 5],     // A barre at 5
                [-1, 3, 5, 5, 4, 3],    // Cm barre at 3
                [-1, 1, 3, 3, 3, 1],    // Bb
                [2, 4, 4, 2, 2, 2],     // F#m barre
                [2, 0, 0, 2, 3, 2],     // D/F#
                [3, 5, 5, -1, -1, -1],  // G5
                [-1, -1, 4, -1, 5, -1], // F#3 E4
                [-1, -1, -1, 9, -1, 8], // E4 C5
            ],
            progression: vec![[-1, 0, 2, 2, 1, 0], [1, 3, 3, 2, 1, 1], [-1, 3, 2, 0, 1, 0], [3, 2, 0, 0, 0, 3]],
            arpeggios: vec![
                ([-1, -1, 0, 2, 3, 1], [2, 3, 4, 5, 4, 3, 4, 5]),
                ([0, 2, 0, 1, 0, 0], [0, 2, 3, 4, 5, 4, 3, 2]),
                ([-1, 0, 2, 2, 1, 0], [1, 2, 3, 4, 5, 4, 3, 2]),
            ],
            line: vec![
                (vec![(2, 2)], 0.25),
                (vec![(2, 4)], 0.25),
                (vec![(3, 2)], 0.25),
                (vec![(3, 4)], 0.25),
                (vec![(4, 3)], 0.5),
                (vec![(4, 5)], 0.25),
                (vec![(5, 2)], 0.5),
                (vec![(4, 3), (5, 3)], 0.4),
                (vec![(3, 4), (4, 3)], 0.4),
                (vec![(2, 4), (3, 2)], 0.4),
                (vec![(1, 3)], 0.25),
                (vec![(1, 5)], 0.25),
                (vec![(0, 5)], 0.6),
            ],
            chug: [-1, 0, 2, 2, -1, -1],
        },
    }
}

/// The score for one set: single strums, a strummed progression, arpeggios,
/// a line with double stops, and palm-muted chugs.
pub fn score(set: Set, seed: u64) -> Score {
    let mut rng = Rng::new(seed);
    let m = material(set);
    let mut sc = Score { plucks: vec![], damps: vec![], len: 0.0, sections: vec![] };
    let mut t = 0.3;

    sc.sections.push((t, "single strums"));
    for shape in &m.chords {
        let down = rng.f() < 0.75;
        sc.strum(shape, t, down, None, rng.range(-6.0, 0.0), &mut rng);
        sc.damp(t + 1.6, ALL);
        t += 2.0;
    }

    sc.sections.push((t, "strummed"));
    for _ in 0..2 {
        for shape in &m.progression {
            for i in 0..4 {
                let down = i % 2 == 0;
                let top = if down { None } else { Some(3) };
                sc.strum(shape, t + 0.3 * i as f64, down, top, rng.range(-5.0, 0.0), &mut rng);
            }
            sc.damp(t + 1.18, ALL);
            t += 1.2;
        }
    }
    t += 0.5;

    sc.sections.push((t, "arpeggio"));
    for (shape, order) in &m.arpeggios {
        for (i, &s) in order.iter().enumerate() {
            sc.pluck(s, shape[s].max(0) as u8, t + 0.25 * i as f64 + rng.range(0.0, 0.01), rng.range(-6.0, 0.0));
        }
        sc.damp(t + 1.98, ALL);
        t += 2.0;
    }
    t += 0.5;

    sc.sections.push((t, "line"));
    for (stop, dur) in &m.line {
        let mut mask = 0u8;
        for &(s, f) in stop {
            sc.pluck(s, f, t + rng.range(0.0, 0.004), rng.range(-6.0, 0.0));
            mask |= 1 << s;
        }
        sc.damp(t + dur - 0.02, mask);
        t += dur;
    }
    t += 0.5;

    sc.sections.push((t, "palm mute"));
    for _ in 0..8 {
        sc.strum(&m.chug, t, true, None, rng.range(-8.0, -3.0), &mut rng);
        sc.damp(t + 0.09, ALL);
        t += 0.18;
    }
    sc.len = t + 0.8;
    sc
}

/// Acoustic body (a few broad resonances) or electric pickup (a comb at the
/// pickup position and a resonant peak), as a gain at frequency `f`.
fn response(kind: Kind, f: f64, m: usize, pickup: f64) -> f64 {
    match kind {
        Kind::Acoustic => {
            let mut g = 0.35;
            for &(fr, a, q) in &[(100.0, 1.0, 6.0), (195.0, 0.9, 8.0), (380.0, 0.6, 5.0), (720.0, 0.35, 4.0), (1500.0, 0.25, 3.0)] {
                let x: f64 = q * (f / fr - fr / f);
                g += a / (1.0 + x * x).sqrt();
            }
            g / (1.0 + (f / 4500.0).powi(2)).sqrt()
        }
        Kind::Electric => {
            let comb = (PI * m as f64 * pickup).sin().abs();
            let x = 1.5 * (f / 2800.0 - 2800.0 / f);
            comb * (1.0 + 1.0 / (1.0 + x * x).sqrt()) / (1.0 + (f / 5000.0).powi(4)).sqrt()
        }
    }
}

fn render_pluck(out: &mut [f64], p: &Pluck, f0: f64, cut: f64, kind: Kind, pickup: f64, rng: &mut Rng) {
    let s = p.string;
    let b = B_OPEN[s] * 2f64.powf(p.fret as f64 / 6.0);
    let vel = 10f64.powf(p.vel_db / 20.0);
    let pos = match kind {
        Kind::Acoustic => rng.range(0.12, 0.22),
        Kind::Electric => rng.range(0.07, 0.18),
    };
    let fc = 1200.0 + 3800.0 * vel;
    let fret_k = 1.0 + p.fret as f64 / 24.0;
    let (b1, b3) = match kind {
        Kind::Acoustic => (SUSTAIN[s] * fret_k, 1.6e-6),
        Kind::Electric => (0.6 * SUSTAIN[s] * fret_k, 0.8e-6),
    };
    let glide = 0.002 * vel;

    let mut parts: Vec<(f64, f64, f64)> = vec![];
    for m in 1..=120usize {
        let fm = m as f64 * f0 * (1.0 + b * (m * m) as f64).sqrt();
        if fm > 10_000.0 {
            break;
        }
        let mut a = (PI * m as f64 * pos).sin().abs() / m as f64;
        a /= (1.0 + (fm / fc).powi(2)).powf(0.75);
        a *= response(kind, fm, m, pickup);
        parts.push((fm, a, b1 + b3 * fm * fm));
    }
    // RMS of the note at its start is 0.07 * velocity.
    let rms = (parts.iter().map(|x| x.1 * x.1).sum::<f64>() / 2.0).sqrt();
    let scale = 0.07 * vel / rms.max(1e-12);

    let start = (p.t * SR) as usize;
    let cut_i = (cut * SR) as usize;
    let end = out.len().min(cut_i + (0.1 * SR) as usize);
    let damp_sigma = 1.0 / 0.012;
    for (m, &(fm, a, sigma)) in parts.iter().enumerate() {
        let phase = rng.range(0.0, 2.0 * PI);
        let df = 0.3 * ((m + 1) as f64).sqrt();
        for &(w, dfreq, smul) in &[(0.75, 0.0, 1.0), (0.25, df, 1.8)] {
            let amp = a * scale * w;
            let (mut re, mut im) = (phase.cos(), phase.sin());
            let mut i = start;
            while i < end {
                let t = (i - start) as f64 / SR;
                let freq = fm * (1.0 + glide * (-t / 0.06).exp()) + dfreq;
                let sig = sigma * smul + if i >= cut_i { damp_sigma } else { 0.0 };
                let r = (-sig / SR).exp();
                let wv = 2.0 * PI * freq / SR;
                let (c, sn) = (r * wv.cos(), r * wv.sin());
                let stop = end.min(i + 32);
                while i < stop {
                    out[i] += amp * im;
                    let nr = re * c - im * sn;
                    im = re * sn + im * c;
                    re = nr;
                    i += 1;
                }
                if amp * (re * re + im * im).sqrt() < 1e-7 {
                    break;
                }
            }
        }
    }

    // Pick noise: a short high-passed burst.
    let nlen = (0.02 * SR) as usize;
    let mut last = 0.0;
    for i in 0..nlen.min(out.len().saturating_sub(start)) {
        let x = rng.range(-1.0, 1.0);
        out[start + i] += 0.03 * vel * (-(i as f64) / (0.003 * SR)).exp() * (x - last);
        last = x;
    }
    // The acoustic top's thump.
    if kind == Kind::Acoustic {
        let tl = (0.2 * SR) as usize;
        for i in 0..tl.min(out.len().saturating_sub(start)) {
            let t = i as f64 / SR;
            out[start + i] += 0.006 * vel * (2.0 * PI * 105.0 * t).sin() * (-t / 0.05).exp();
        }
    }
}

/// Render a score. Returns the signal and the notes played, in onset order.
pub fn render(sc: &Score, kind: Kind, seed: u64) -> (Vec<f32>, Vec<Note>) {
    let mut rng = Rng::new(seed ^ 0xABCD);
    let n = (sc.len * SR) as usize;
    let mut out = vec![0.0f64; n];
    let detune: Vec<f64> = (0..6).map(|_| rng.range(-4.0, 4.0)).collect();
    let pickup = rng.range(0.1, 0.2);
    let mut plucks = sc.plucks.clone();
    plucks.sort_by(|a, b| a.t.total_cmp(&b.t));
    let mut notes = vec![];
    for (i, p) in plucks.iter().enumerate() {
        let mut cut = sc.len;
        if let Some(q) = plucks[i + 1..].iter().find(|q| q.string == p.string) {
            cut = q.t;
        }
        for &(t, mask) in &sc.damps {
            if t > p.t && mask & (1 << p.string) != 0 {
                cut = cut.min(t);
            }
        }
        let midi = OPEN[p.string] + p.fret;
        let hz = 440.0 * 2f64.powf((midi as f64 - 69.0) / 12.0) * 2f64.powf(detune[p.string] / 1200.0);
        notes.push(Note { midi, hz, on: p.t, off: cut, vel_db: p.vel_db });
        render_pluck(&mut out, p, hz, cut, kind, pickup, &mut rng);
    }
    // Room and converter noise, -66 dBFS RMS.
    let a = 5e-4 * 3f64.sqrt();
    let sig = out.iter().map(|&x| (x + rng.range(-a, a)) as f32).collect();
    (sig, notes)
}
