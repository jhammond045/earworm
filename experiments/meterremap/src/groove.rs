//! Synthetic grooves with known bars: drums, bass and keys in any meter, each event rendered from
//! its own random stream so the same event sounds the same wherever it is placed. A pattern
//! re-sequences the events beat by beat into the "ideal": the band playing the new meter, notes
//! released where the new order breaks them, the room reverb running on.

pub const SR: f32 = 48_000.0;
const TAU: f32 = std::f32::consts::TAU;

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn f(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    pub fn noise(&mut self) -> f32 {
        self.f() * 2.0 - 1.0
    }
    pub fn normal(&mut self) -> f32 {
        let u = self.f().max(1e-7);
        let v = self.f();
        (-2.0 * u.ln()).sqrt() * (TAU * v).cos()
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Inst {
    Kick,
    Snare,
    Hat,
    Ride,
    Rim,
    Bass,
    Keys,
}

/// Stems: 0 drums, 1 bass, 2 keys.
pub const STEMS: usize = 3;

pub fn stem(i: Inst) -> usize {
    match i {
        Inst::Bass => 1,
        Inst::Keys => 2,
        _ => 0,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Ev {
    pub inst: Inst,
    pub bar: usize,
    /// Written position in beats from the start of its bar (swung notes included).
    pub beat: f64,
    /// Length in beats (bass and keys; drums ring out).
    pub len: f64,
    /// Bass: the note. Keys: the chord root; `minor` picks the third.
    pub midi: f32,
    pub minor: bool,
    pub vel: f32,
    /// How far from its written time it was played, seconds.
    pub off: f64,
    pub seed: u64,
}

pub struct Groove {
    pub name: String,
    pub bpm: f64,
    pub meter: usize,
    pub bars: usize,
    /// Seconds before the first beat.
    pub lead: f64,
    pub events: Vec<Ev>,
}

impl Groove {
    pub fn spb(&self) -> f64 {
        60.0 / self.bpm
    }
    /// Seconds of a written beat position (global beats from the first downbeat).
    pub fn at(&self, beats: f64) -> f64 {
        self.lead + beats * self.spb()
    }
    pub fn beats(&self) -> usize {
        self.bars * self.meter
    }
}

// ---------------------------------------------------------------- sounds

fn kick(r: &mut Rng) -> Vec<f32> {
    let n = (0.45 * SR) as usize;
    let mut ph = 0.0f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / SR;
            ph += (48.0 + 110.0 * (-t / 0.035).exp()) / SR;
            let click = if t < 0.003 { 0.4 * r.noise() } else { 0.0 };
            0.8 * (TAU * ph).sin() * (-t / 0.13).exp() + click
        })
        .collect()
}

fn snare(r: &mut Rng) -> Vec<f32> {
    let n = (0.3 * SR) as usize;
    let (mut a, mut b) = (0.0f32, 0.0f32);
    (0..n)
        .map(|i| {
            let t = i as f32 / SR;
            let w = r.noise();
            a += 0.25 * (w - a);
            b += 0.03 * (a - b);
            1.4 * (a - b) * (-t / 0.07).exp() + 0.3 * (TAU * 190.0 * t).sin() * (-t / 0.04).exp()
        })
        .collect()
}

fn hat(r: &mut Rng) -> Vec<f32> {
    let n = (0.12 * SR) as usize;
    let (mut x1, mut x2) = (0.0f32, 0.0f32);
    (0..n)
        .map(|i| {
            let t = i as f32 / SR;
            let w = r.noise();
            let y = w - 2.0 * x1 + x2;
            x2 = x1;
            x1 = w;
            0.3 * y * (-t / 0.018).exp()
        })
        .collect()
}

/// A ride cymbal: inharmonic partials with a long decay, plus a bright tick.
fn ride(r: &mut Rng) -> Vec<f32> {
    let n = (1.4 * SR) as usize;
    let parts: Vec<(f32, f32)> = (0..8).map(|k| (2900.0 * (1.0 + 0.37 * k as f32 + 0.05 * r.f()), r.f() * TAU)).collect();
    let (mut x1, mut x2) = (0.0f32, 0.0f32);
    (0..n)
        .map(|i| {
            let t = i as f32 / SR;
            let mut v = 0.0;
            for (k, &(hz, ph)) in parts.iter().enumerate() {
                v += (TAU * hz * t + ph).sin() * (-t / (0.5 - 0.04 * k as f32)).exp();
            }
            let w = r.noise();
            let y = w - 2.0 * x1 + x2;
            x2 = x1;
            x1 = w;
            0.035 * v + 0.25 * y * (-t / 0.01).exp()
        })
        .collect()
}

fn rim(r: &mut Rng) -> Vec<f32> {
    let n = (0.08 * SR) as usize;
    (0..n)
        .map(|i| {
            let t = i as f32 / SR;
            0.6 * (TAU * 1700.0 * t).sin() * (-t / 0.008).exp() + 0.2 * r.noise() * (-t / 0.004).exp()
        })
        .collect()
}

fn hz(midi: f32) -> f32 {
    440.0 * 2f32.powf((midi - 69.0) / 12.0)
}

/// A note with an attack, a decay toward `sustain`, and a release from `dur` on.
fn env(t: f32, dur: f32, attack: f32, decay: f32, release: f32) -> f32 {
    let a = (t / attack).min(1.0);
    let d = (-t / decay).exp();
    let r = if t > dur { (-(t - dur) / release).exp() } else { 1.0 };
    a * d * r
}

fn bass(r: &mut Rng, midi: f32, dur: f32) -> Vec<f32> {
    let n = ((dur + 0.3) * SR) as usize;
    let f = hz(midi);
    let ph0 = r.f();
    (0..n)
        .map(|i| {
            let t = i as f32 / SR;
            let mut v = 0.0;
            for k in 1..=6 {
                let bright = (-t * k as f32 / 0.6).exp() * 0.6 + 0.4;
                v += (TAU * (f * k as f32 * t + ph0)).sin() / (k * k) as f32 * bright;
            }
            0.55 * v * env(t, dur, 0.004, 1.8, 0.035)
        })
        .collect()
}

/// An electric-piano chord: root, third, fifth and octave, bright partials dying first.
fn keys(r: &mut Rng, root: f32, minor: bool, dur: f32) -> Vec<f32> {
    let n = ((dur + 0.4) * SR) as usize;
    let notes = [root, root + if minor { 3.0 } else { 4.0 }, root + 7.0, root + 12.0];
    let phs: Vec<f32> = (0..16).map(|_| r.f()).collect();
    let mut out: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / SR;
            let mut v = 0.0;
            for (j, &m) in notes.iter().enumerate() {
                let f = hz(m);
                for k in 1..=4 {
                    v += (TAU * (f * k as f32 * t + phs[j * 4 + k - 1])).sin() * (-t * k as f32 / 0.9).exp() / k as f32;
                }
            }
            0.11 * v * env(t, dur, 0.003, 2.5, 0.06)
        })
        .collect();
    // Hammer noise.
    for (i, v) in out.iter_mut().take((0.01 * SR) as usize).enumerate() {
        *v += 0.05 * r.noise() * (1.0 - i as f32 / (0.01 * SR));
    }
    out
}

fn sound(e: &Ev, len_s: f32) -> Vec<f32> {
    let mut r = Rng::new(e.seed);
    let mut w = match e.inst {
        Inst::Kick => kick(&mut r),
        Inst::Snare => snare(&mut r),
        Inst::Hat => hat(&mut r),
        Inst::Ride => ride(&mut r),
        Inst::Rim => rim(&mut r),
        Inst::Bass => bass(&mut r, e.midi, len_s),
        Inst::Keys => keys(&mut r, e.midi, e.minor, len_s),
    };
    for v in w.iter_mut() {
        *v *= e.vel;
    }
    w
}

fn mix_at(out: &mut [f32], t: f64, w: &[f32]) {
    let a = (t * SR as f64).round() as i64;
    for (i, v) in w.iter().enumerate() {
        let k = a + i as i64;
        if k >= 0 {
            if let Some(o) = out.get_mut(k as usize) {
                *o += v;
            }
        }
    }
}

/// A small room: four combs and two allpasses (Schroeder), about 0.9 s to die away.
fn reverb(x: &[f32]) -> Vec<f32> {
    let combs = [0.0297f32, 0.0371, 0.0411, 0.0437];
    let mut wet = vec![0.0f32; x.len()];
    for &d in &combs {
        let n = (d * SR) as usize;
        let g = 10f32.powf(-3.0 * d / 0.9);
        let mut buf = vec![0.0f32; n];
        let mut p = 0;
        for (i, &v) in x.iter().enumerate() {
            let y = buf[p];
            buf[p] = v + g * y;
            p = (p + 1) % n;
            wet[i] += y * 0.25;
        }
    }
    for &(d, g) in &[(0.005f32, 0.7f32), (0.0017, 0.7)] {
        let n = (d * SR) as usize;
        let mut buf = vec![0.0f32; n];
        let mut p = 0;
        for v in wet.iter_mut() {
            let b = buf[p];
            let y = -g * *v + b;
            buf[p] = *v + g * y;
            p = (p + 1) % n;
            *v = y;
        }
    }
    x.iter().zip(&wet).map(|(a, b)| a + 0.22 * b).collect()
}

/// One stem per instrument group, each with its own (linear) room, so the stems sum to the mix.
pub struct Rendered {
    pub stems: [Vec<f32>; STEMS],
    pub mix: Vec<f32>,
}

/// Render events placed at `(event, start seconds, length seconds)`, `total` seconds long.
pub fn render(placed: &[(Ev, f64, f64)], total: f64) -> Rendered {
    let n = (total * SR as f64) as usize;
    let mut stems: [Vec<f32>; STEMS] = std::array::from_fn(|_| vec![0.0f32; n]);
    for (e, t, len) in placed {
        mix_at(&mut stems[stem(e.inst)], *t, &sound(e, *len as f32));
    }
    // Room noise at about -66 dB, as a real take has some.
    let mut nr = Rng::new(0x5eed);
    for s in stems.iter_mut() {
        *s = reverb(s);
    }
    for v in stems[0].iter_mut() {
        *v += 0.0005 * nr.normal();
    }
    let mix = (0..n).map(|i| stems.iter().map(|s| s[i]).sum()).collect();
    Rendered { stems, mix }
}

/// The groove as performed.
pub fn performed(g: &Groove) -> (Rendered, f64) {
    let spb = g.spb();
    let placed: Vec<(Ev, f64, f64)> = g
        .events
        .iter()
        .map(|e| (*e, g.at((e.bar * g.meter) as f64 + e.beat) + e.off, e.len * spb))
        .collect();
    let total = g.at(g.beats() as f64) + 1.2;
    (render(&placed, total), total)
}

/// Global source beat for each output beat: bar `i` of the output plays, at slot `j`, beat
/// `pattern[j]` (1-based) of source bar `i`.
pub fn source_beats(meter: usize, bars: usize, pattern: &[usize]) -> Vec<usize> {
    (0..bars).flat_map(|i| pattern.iter().map(move |&p| i * meter + p - 1)).collect()
}

/// For each output beat, how many beats after it the source still runs on unbroken (itself
/// included): where a held note has to stop.
pub fn run_left(src: &[usize]) -> Vec<usize> {
    let mut left = vec![1usize; src.len()];
    for j in (0..src.len().saturating_sub(1)).rev() {
        if src[j + 1] == src[j] + 1 {
            left[j] = left[j + 1] + 1;
        }
    }
    left
}

/// The ideal: the events re-sequenced beat by beat by `pattern`, each keeping its played offset;
/// a bass or keys note is released where its beat's run ends.
pub fn ideal(g: &Groove, pattern: &[usize]) -> (Rendered, f64) {
    let spb = g.spb();
    let src = source_beats(g.meter, g.bars, pattern);
    let left = run_left(&src);
    let mut placed = Vec::new();
    for (j, &k) in src.iter().enumerate() {
        for e in &g.events {
            let gb = (e.bar * g.meter) as f64 + e.beat;
            if gb.floor() as usize != k {
                continue;
            }
            let pos = j as f64 + (gb - k as f64);
            let room = (j + left[j]) as f64 - pos;
            placed.push((*e, g.at(pos) + e.off, e.len.min(room) * spb));
        }
    }
    let total = g.at(src.len() as f64) + 1.2;
    (render(&placed, total), total)
}

// ---------------------------------------------------------------- the named grooves

struct Plan {
    seed: u64,
    jitter_ms: f64,
    events: Vec<Ev>,
}

impl Plan {
    fn new(seed: u64, jitter_ms: f64) -> Self {
        Plan { seed, jitter_ms, events: Vec::new() }
    }
    fn add(&mut self, inst: Inst, bar: usize, beat: f64, len: f64, midi: f32, minor: bool, vel: f32, bias_ms: f64) {
        let n = self.events.len() as u64;
        let seed = self.seed.wrapping_mul(7919).wrapping_add(n);
        let mut r = Rng::new(seed ^ 0xabc);
        let off = (bias_ms + self.jitter_ms * r.normal() as f64) / 1000.0;
        self.events.push(Ev { inst, bar, beat, len, midi, minor, vel, off, seed });
    }
}

/// 4/4 at 100 bpm: kick on 1, the "and" of 2 and 3, a late snare on 2 and 4, eighth hats; bass and
/// held electric-piano chords that cross beats, I-vi-IV-V.
pub fn four(bars: usize) -> Groove {
    let mut p = Plan::new(44, 5.0);
    let prog: [(f32, bool); 4] = [(60.0, false), (57.0, true), (53.0, false), (55.0, false)];
    for b in 0..bars {
        let (root, minor) = prog[b % 4];
        let bass_root = root - 24.0;
        for (beat, v) in [(0.0, 1.0), (1.5, 0.7), (2.0, 0.9)] {
            p.add(Inst::Kick, b, beat, 0.0, 0.0, false, v, -2.0);
        }
        for beat in [1.0, 3.0] {
            p.add(Inst::Snare, b, beat, 0.0, 0.0, false, 0.9, 7.0);
        }
        for k in 0..8 {
            p.add(Inst::Hat, b, k as f64 * 0.5, 0.0, 0.0, false, if k % 2 == 0 { 0.55 } else { 0.3 }, 0.0);
        }
        for (beat, len, d) in [(0.0, 1.0, 0.0), (1.5, 0.5, 0.0), (2.0, 1.0, 0.0), (3.0, 0.5, 12.0), (3.5, 0.5, 7.0)] {
            p.add(Inst::Bass, b, beat, len * 0.9, bass_root + d, false, 0.9, 4.0);
        }
        p.add(Inst::Keys, b, 0.0, 1.9, root, minor, 0.8, 3.0);
        p.add(Inst::Keys, b, 2.5, 1.3, root, minor, 0.6, 3.0);
    }
    Groove { name: "four".into(), bpm: 100.0, meter: 4, bars, lead: 0.3, events: p.events }
}

/// 5/4 at 160 bpm, swung, grouped 3 + 2: a ride on every beat with a swung skip, a soft kick on 1
/// and 4, rim clicks on 3 and 5; a bass on every beat; comping chords on 1, the skip of 2, and 4.
/// A generic jazz figure in five, not any one tune.
pub fn five(bars: usize) -> Groove {
    let mut p = Plan::new(55, 6.0);
    let sw = 2.0 / 3.0;
    let prog: [(f32, bool); 2] = [(62.0, true), (55.0, true)];
    for b in 0..bars {
        let (root, minor) = prog[b % 2];
        for beat in [0.0f64, 1.0, 1.0 + sw, 2.0, 3.0, 3.0 + sw, 4.0] {
            let v = if beat == 0.0 || beat == 3.0 { 0.9 } else if beat.fract() > 0.0 { 0.5 } else { 0.7 };
            p.add(Inst::Ride, b, beat, 0.0, 0.0, false, v, -4.0);
        }
        for beat in [0.0, 3.0] {
            p.add(Inst::Kick, b, beat, 0.0, 0.0, false, 0.6, 0.0);
        }
        for beat in [2.0, 4.0] {
            p.add(Inst::Rim, b, beat, 0.0, 0.0, false, 0.7, 5.0);
        }
        let line = [0.0f32, 7.0, 5.0, 3.0, 2.0];
        for (k, d) in line.iter().enumerate() {
            p.add(Inst::Bass, b, k as f64, 0.85, root - 24.0 + d, false, 0.9, 3.0);
        }
        p.add(Inst::Keys, b, 0.0, 1.4, root, minor, 0.7, 2.0);
        p.add(Inst::Keys, b, 1.0 + sw, 1.0, root + 5.0, minor, 0.55, 2.0);
        p.add(Inst::Keys, b, 3.0, 1.8, root, minor, 0.65, 2.0);
    }
    Groove { name: "five".into(), bpm: 160.0, meter: 5, bars, lead: 0.3, events: p.events }
}

/// 3/4 at 144 bpm, a waltz: kick and bass on 1, chord stabs on 2 and 3 with light snares, eighth
/// hats; C F G C.
pub fn three(bars: usize) -> Groove {
    let mut p = Plan::new(33, 5.0);
    let prog: [f32; 4] = [60.0, 53.0, 55.0, 60.0];
    for b in 0..bars {
        let root = prog[b % 4];
        p.add(Inst::Kick, b, 0.0, 0.0, 0.0, false, 1.0, -2.0);
        for beat in [1.0, 2.0] {
            p.add(Inst::Snare, b, beat, 0.0, 0.0, false, 0.45, 5.0);
            p.add(Inst::Keys, b, beat, 0.7, root, false, 0.6, 3.0);
        }
        for k in 0..6 {
            p.add(Inst::Hat, b, k as f64 * 0.5, 0.0, 0.0, false, if k % 2 == 0 { 0.5 } else { 0.28 }, 0.0);
        }
        p.add(Inst::Bass, b, 0.0, 1.6, root - 24.0, false, 1.0, 3.0);
        p.add(Inst::Bass, b, 2.0, 0.8, root - 24.0 + 7.0, false, 0.7, 3.0);
    }
    Groove { name: "three".into(), bpm: 144.0, meter: 3, bars, lead: 0.3, events: p.events }
}

// ---------------------------------------------------------------- random grooves, for detection

/// Which instruments a random groove has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Band {
    Full,
    DrumsOnly,
    NoDrums,
    KeysOnly,
}

/// A random groove in `meter`: a kick on 1 most of the time, backbeats where that meter puts them,
/// random extra hits, a bass on 1 and elsewhere, chords from a 2- or 4-bar progression.
pub fn random(seed: u64, meter: usize, bars: usize, band: Band) -> Groove {
    let mut r = Rng::new(seed);
    let bpm = 80.0 + 90.0 * r.f() as f64;
    let mut p = Plan::new(seed, 4.0 + 4.0 * r.f() as f64);
    let backbeats: &[usize] = match meter {
        2 => &[1],
        3 => &[1, 2],
        4 => &[1, 3],
        5 => &[2, 4],
        6 => &[3],
        _ => &[2, 4, 6],
    };
    let kicks: Vec<usize> = (0..meter * 2).filter(|&k| k == 0 || (!backbeats.contains(&(k / 2)) && r.f() < 0.22)).collect();
    let bass: Vec<usize> = (0..meter * 2).filter(|&k| k == 0 || r.f() < 0.3).collect();
    let stabs: Vec<usize> = (1..meter * 2).filter(|_| r.f() < 0.18).collect();
    let roots = [60.0f32, 57.0, 53.0, 55.0, 62.0, 64.0];
    let prog_len = if r.f() < 0.5 { 2 } else { 4 };
    let prog: Vec<(f32, bool)> = (0..prog_len).map(|_| (roots[r.below(roots.len())], r.f() < 0.4)).collect();
    let drums = matches!(band, Band::Full | Band::DrumsOnly);
    let has_bass = matches!(band, Band::Full | Band::NoDrums);
    let has_keys = band != Band::DrumsOnly;
    for b in 0..bars {
        let (root, minor) = prog[b % prog_len];
        if drums {
            for &k in &kicks {
                p.add(Inst::Kick, b, k as f64 * 0.5, 0.0, 0.0, false, if k == 0 { 1.0 } else { 0.75 }, -2.0);
            }
            for &k in backbeats {
                p.add(Inst::Snare, b, k as f64, 0.0, 0.0, false, 0.85, 5.0);
            }
            for k in 0..meter * 2 {
                p.add(Inst::Hat, b, k as f64 * 0.5, 0.0, 0.0, false, if k % 2 == 0 { 0.5 } else { 0.3 }, 0.0);
            }
        }
        if has_bass {
            for (i, &k) in bass.iter().enumerate() {
                let next = bass.get(i + 1).copied().unwrap_or(meter * 2);
                let d = if k == 0 { 0.0 } else { [7.0, 5.0, 12.0, 3.0][k % 4] };
                p.add(Inst::Bass, b, k as f64 * 0.5, (next - k) as f64 * 0.45, root - 24.0 + d, false, if k == 0 { 1.0 } else { 0.7 }, 3.0);
            }
        }
        if has_keys {
            p.add(Inst::Keys, b, 0.0, meter as f64 * 0.45, root, minor, 0.75, 2.0);
            for &k in &stabs {
                p.add(Inst::Keys, b, k as f64 * 0.5, 0.4, root, minor, 0.5, 2.0);
            }
        }
    }
    Groove { name: format!("random-{meter}-{seed}"), bpm, meter, bars, lead: 0.3, events: p.events }
}
