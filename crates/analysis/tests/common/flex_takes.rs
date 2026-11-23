//! The flex prototype's synthetic takes (experiments/flexquant, docs/quant.md "Prototype"),
//! with known timing: a drum loop and a sung line. Moved here as the prototype wrote them. Each
//! take is rendered twice from the same per-event random streams: as performed, and with every
//! event exactly on its grid slot (`ideal`, what a perfect quantize would sound like).

use super::SR;
const TAU: f32 = std::f32::consts::TAU;

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03)
    }
    fn next_u64(&mut self) -> u64 {
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
}

/// One event of a take: where it was played and which grid slot it belongs to.
#[derive(Clone, Copy, Debug)]
pub struct Event {
    /// Performed onset, seconds from the start of the take.
    pub t: f64,
    /// Grid position in beats.
    pub beat: f64,
    /// Drums: bit 0 kick, bit 1 snare, bit 2 hat. Sung: 0 detached, 1 "da", 2 legato.
    pub class: u8,
    /// Sung only: the note's pitch in cents above C0.
    pub cents: f32,
}

pub struct Take {
    pub audio: Vec<f32>,
    pub ideal: Vec<f32>,
    pub events: Vec<Event>,
    pub bpm: f64,
    /// Seconds before beat 0.
    pub lead: f64,
}

// ---------------------------------------------------------------- drums

/// How a drummer plays the loop.
pub struct Feel {
    /// MPC-style swing on 16ths: where the off 16th falls inside its 8th (0.5 straight).
    pub swing: f64,
    /// Extra lateness of the snare, ms.
    pub snare_ms: f64,
    /// Random timing per step, standard deviation in ms.
    pub jitter_ms: f64,
    /// A slow push and drag over the loop: peak ms, one cycle per loop.
    pub drift_ms: f64,
}

fn kick(r: &mut Rng) -> Vec<f32> {
    let n = (0.4 * SR) as usize;
    let mut ph = 0.0f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / SR;
            ph += (50.0 + 110.0 * (-t / 0.035).exp()) / SR;
            let click = if t < 0.003 { 0.4 * r.noise() } else { 0.0 };
            0.8 * (TAU * ph).sin() * (-t / 0.11).exp() + click
        })
        .collect()
}

fn snare(r: &mut Rng) -> Vec<f32> {
    let n = (0.25 * SR) as usize;
    let (mut a, mut b) = (0.0f32, 0.0f32);
    (0..n)
        .map(|i| {
            let t = i as f32 / SR;
            let w = r.noise();
            a += 0.25 * (w - a);
            b += 0.03 * (a - b);
            1.4 * (a - b) * (-t / 0.06).exp() + 0.3 * (TAU * 200.0 * t).sin() * (-t / 0.04).exp()
        })
        .collect()
}

fn hat(r: &mut Rng) -> Vec<f32> {
    let n = (0.1 * SR) as usize;
    let (mut x1, mut x2) = (0.0f32, 0.0f32);
    (0..n)
        .map(|i| {
            let t = i as f32 / SR;
            let w = r.noise();
            let y = w - 2.0 * x1 + x2;
            x2 = x1;
            x1 = w;
            0.3 * y * (-t / 0.015).exp()
        })
        .collect()
}

fn mix_at(out: &mut [f32], t: f64, w: &[f32], gain: f32) {
    let a = (t * SR as f64).round() as usize;
    for (i, v) in w.iter().enumerate() {
        if let Some(o) = out.get_mut(a + i) {
            *o += gain * v;
        }
    }
}

/// A 16-step loop: kick on 1, 3 and the "and" of 3, snare on 2 and 4, hats on every 16th.
pub fn drums(seed: u64, bpm: f64, bars: usize, feel: &Feel) -> Take {
    let lead = 0.25;
    let spb = 60.0 / bpm;
    let total = lead + (bars * 4) as f64 * spb + 0.6;
    let n = (total * SR as f64) as usize;
    let (mut audio, mut ideal) = (vec![0.0f32; n], vec![0.0f32; n]);
    let mut r = Rng::new(seed);
    let mut events = Vec::new();
    let steps = bars * 16;
    for s in 0..steps {
        let step = s % 16;
        let mut class = 0u8;
        if matches!(step, 0 | 8 | 10) {
            class |= 1;
        }
        if matches!(step, 4 | 12) {
            class |= 2;
        }
        class |= 4;
        let beat = s as f64 * 0.25;
        // The second 16th of each 8th falls at `swing` of the 8th (spb / 2).
        let swing = if step % 2 == 1 { (feel.swing - 0.5) * spb / 2.0 } else { 0.0 };
        let late = if class & 2 != 0 { feel.snare_ms / 1000.0 } else { 0.0 };
        let drift = feel.drift_ms / 1000.0 * (std::f64::consts::TAU * s as f64 / steps as f64).sin();
        let jit = feel.jitter_ms / 1000.0 * r.normal() as f64;
        let t = lead + beat * spb + swing + late + drift + jit;
        let grid_t = lead + beat * spb;
        // One random stream per event, so the performed and the ideal render use the same waveform.
        let ev_seed = seed.wrapping_mul(1000).wrapping_add(s as u64);
        let accent = if step % 2 == 0 { 1.0 } else { 0.45 };
        for (bit, make) in [(1u8, kick as fn(&mut Rng) -> Vec<f32>), (2, snare), (4, hat)] {
            if class & bit != 0 {
                let w = make(&mut Rng::new(ev_seed ^ bit as u64));
                let g = if bit == 4 { accent } else { 1.0 } * 0.5;
                mix_at(&mut audio, t, &w, g);
                mix_at(&mut ideal, grid_t, &w, g);
            }
        }
        events.push(Event { t, beat, class, cents: 0.0 });
    }
    // Room noise at about -66 dB, the same in both renders, as a real take always has some.
    let mut nr = Rng::new(seed ^ 0x5eed);
    for (a, b) in audio.iter_mut().zip(ideal.iter_mut()) {
        let v = 0.0005 * nr.normal();
        *a += v;
        *b += v;
    }
    Take { audio, ideal, events, bpm, lead }
}

// ---------------------------------------------------------------- sung line

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// (beat, length in beats, semitones above A3, link into the note: 0 detached, 1 "da", 2 legato)
const MELODY: [(f64, f64, f32, u8); 18] = [
    (0.0, 1.0, 7.0, 0),
    (1.0, 0.5, 9.0, 1),
    (1.5, 0.5, 11.0, 2),
    (2.0, 1.5, 12.0, 1),
    (4.0, 0.5, 12.0, 0),
    (4.5, 0.5, 11.0, 1),
    (5.0, 1.0, 9.0, 2),
    (6.0, 2.0, 7.0, 1),
    (8.0, 1.0, 4.0, 0),
    (9.0, 0.5, 5.0, 1),
    (9.5, 0.5, 7.0, 1),
    (10.0, 1.0, 9.0, 2),
    (11.0, 1.0, 7.0, 1),
    (12.0, 0.5, 5.0, 0),
    (12.5, 0.5, 4.0, 1),
    (13.0, 0.5, 2.0, 2),
    (13.5, 0.5, 4.0, 1),
    (14.0, 2.0, 0.0, 0),
];

/// A note as rendered: onset and end in seconds, pitch, link into it.
#[derive(Clone, Copy)]
struct Sung {
    on: f64,
    end: f64,
    cents: f32,
    link: u8,
}

/// Additive voice ("ah") with vibrato, breath, consonant bursts on "da" and pitch glides on
/// legato links. `seed` drives the breath and the bursts.
fn voice(notes: &[Sung], total: f64, seed: u64) -> Vec<f32> {
    let n = (total * SR as f64) as usize;
    let mut cents = vec![f32::NAN; n];
    let mut gain = vec![0.0f32; n];
    let mut burst = vec![0.0f32; n];
    // Breath noise runs from each onset with its own stream, so a note moved in time keeps the
    // same noise and the ideal render can be compared sample by sample.
    let mut noise = vec![0.0f32; n];
    for (i, nt) in notes.iter().enumerate() {
        let mut r = Rng::new(seed ^ (i as u64 + 1) * 7919);
        let s0 = ((nt.on - 0.05).max(0.0) * SR as f64) as usize;
        let s1 = (((nt.end + 0.05) * SR as f64) as usize).min(n);
        for v in noise[s0..s1].iter_mut() {
            *v = r.noise();
        }
    }
    for (i, nt) in notes.iter().enumerate() {
        let next_link = notes.get(i + 1).map(|x| x.link);
        let joined_in = nt.link != 0 && i > 0;
        let joined_out = matches!(next_link, Some(1) | Some(2));
        let s0 = (nt.on * SR as f64) as usize;
        let s1 = (((nt.end + 0.04) * SR as f64) as usize).min(n);
        for s in s0..s1 {
            let t = s as f64 / SR as f64;
            let tt = (t - nt.on) as f32;
            let left = (nt.end - t) as f32;
            if joined_out && left < 0.0 {
                break;
            }
            let a = if joined_in { 1.0 } else { smooth(tt / 0.012) };
            let rel = if joined_out { 1.0 } else { smooth(left / 0.03 + 1.0) };
            let swell = 1.0 + 0.12 * (std::f32::consts::PI * (tt / (nt.end - nt.on) as f32).min(1.0)).sin();
            gain[s] = a * rel * swell;
            let vib = 35.0 * smooth((tt - 0.12) / 0.2) * (TAU * 5.5 * tt).sin();
            cents[s] = nt.cents + vib;
        }
        if nt.link == 2 && i > 0 {
            // Legato: glide over 40 ms around the onset, level untouched.
            let prev = notes[i - 1].cents;
            let g0 = ((nt.on - 0.02) * SR as f64) as usize;
            let g1 = (((nt.on + 0.02) * SR as f64) as usize).min(n);
            for s in g0..g1 {
                let x = smooth((s - g0) as f32 / (g1 - g0) as f32);
                cents[s] = prev + (nt.cents - prev) * x;
                gain[s] = gain[s].max(1.0);
            }
        }
        if nt.link == 1 && i > 0 {
            // "da": a level dip to -14 dB around the onset and a short burst.
            let w = 0.035;
            let s0 = (((nt.on - w) * SR as f64).max(0.0)) as usize;
            let s1 = (((nt.on + w) * SR as f64) as usize).min(n);
            for s in s0..s1 {
                let x = ((s as f64 / SR as f64 - nt.on) / w) as f32;
                let shape = 0.5 + 0.5 * (std::f32::consts::PI * x).cos();
                gain[s] *= 1.0 - (1.0 - 0.2) * shape;
            }
            let b0 = (nt.on * SR as f64) as usize;
            let b1 = (((nt.on + 0.012) * SR as f64) as usize).min(n);
            for s in b0..b1 {
                burst[s] = 0.5 * smooth((s - b0) as f32 / 30.0) * smooth((b1 - s) as f32 / 150.0);
            }
        }
    }
    // Hold the pitch through gaps so the oscillator never sees NaN.
    let mut last = notes.first().map(|n| n.cents).unwrap_or(4000.0);
    for c in cents.iter_mut() {
        if c.is_nan() {
            *c = last;
        } else {
            last = *c;
        }
    }
    let formant = |hz: f32| -> f32 {
        let mut g = 0.05;
        for (fc, bw, gn) in [(700.0f32, 110.0f32, 1.0f32), (1150.0, 130.0, 0.6), (2600.0, 260.0, 0.2)] {
            let d = (hz - fc) / bw;
            g += gn / (1.0 + d * d);
        }
        g
    };
    let mut r = Rng::new(seed);
    let mut out = vec![0.0f32; n];
    let mut phase = 0.0f32;
    let (mut lp1, mut lp2) = (0.0f32, 0.0f32);
    for s in 0..n {
        let hz = dsp::cents_to_hz(cents[s]);
        phase = (phase + hz / SR).fract();
        let g = gain[s];
        let mut x = 0.0;
        if g > 1e-4 {
            let mut k = 1;
            while (k as f32) * hz < 6000.0 {
                x += formant(k as f32 * hz) / (k as f32).powf(1.3) * (TAU * k as f32 * phase).sin();
                k += 1;
            }
        }
        let w = noise[s];
        lp1 += 0.3 * (w - lp1);
        lp2 += 0.02 * (lp1 - lp2);
        out[s] = 0.25 * (g * x + 0.08 * g * (lp1 - lp2)) + 0.3 * burst[s] * w + 0.0003 * r.normal();
    }
    out
}

/// The melody sung against a 100 bpm grid: each onset late by `lag_ms` on average, with
/// `jitter_ms` of random timing. The ideal render puts every onset and end on the grid.
pub fn sung(seed: u64, bpm: f64, lag_ms: f64, jitter_ms: f64) -> Take {
    let lead = 0.3;
    let spb = 60.0 / bpm;
    let mut r = Rng::new(seed);
    let base = 4500.0; // A3, 220 Hz, in cents above C0
    let offs: Vec<f64> = MELODY.iter().map(|_| (lag_ms + jitter_ms * r.normal() as f64) / 1000.0).collect();
    let plan = |shift: bool| -> Vec<Sung> {
        MELODY
            .iter()
            .enumerate()
            .map(|(i, &(beat, len, semi, link))| {
                let off = |j: usize| if shift { offs[j] } else { 0.0 };
                let on = lead + beat * spb + off(i);
                let next = MELODY.get(i + 1);
                // A note runs to the next onset when the next note joins it; a detached note
                // leaves a gap of a fifth of its length.
                let end = match next {
                    Some(&(nb, _, _, nl)) if nl != 0 && (nb - (beat + len)).abs() < 1e-9 => lead + nb * spb + off(i + 1),
                    _ => lead + (beat + len * 0.8) * spb + off(i),
                };
                Sung { on, end, cents: base + 100.0 * semi, link }
            })
            .collect()
    };
    let total = lead + 16.0 * spb + 0.6;
    let performed = plan(true);
    let audio = voice(&performed, total, seed);
    let ideal = voice(&plan(false), total, seed);
    let events = performed
        .iter()
        .zip(MELODY.iter())
        .map(|(p, &(beat, _, _, link))| Event { t: p.on, beat, class: link, cents: p.cents })
        .collect();
    Take { audio, ideal, events, bpm, lead }
}
