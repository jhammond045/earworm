//! Poly takes (docs/poly.md, stage P3): the notes of a polyphonic take (a guitar's chords, a piano,
//! anything that plays several notes at once) and a curve per note, from the take's audio.
//!
//! - `model`: Basic Pitch's network, run over the take in two-second windows.
//! - `decode`: Basic Pitch's own note decoding (`note_creation.py`), the same steps and thresholds,
//!   so the notes are the ones the Python package gives.
//! - Here: the take is resampled to 22050 Hz as it streams in (`PolyStream::feed`), so a long take
//!   never sits in memory whole; at the end the notes are decoded, kept to the instrument's range
//!   and the layer's capture channels, and each note gets a curve: one record per 10 ms of its life
//!   (pitch offset from the note in cents, volume, intensity, hardness, onset) from the model's
//!   pitch contours and the take's constant-Q spectrum, which the model computes anyway.
//!
//! # Flat result (`PolyResult::to_floats`)
//!
//! ```text
//! header, 8 floats: version (1), n_notes, n_records, NOTE_LEN, REC_LEN, frames read, seconds, 0
//! notes, n_notes x NOTE_LEN: take seconds, duration s, cents above C0 (the semitone), velocity
//!   (0..1, the volume at the start), hardness (0..1), first record, record count, MIDI note
//! records, n_records x REC_LEN: pitch offset from the note's cents, volume (0..1), intensity
//!   (0..1, 0.5 the take's usual tilt), hardness (0..1, held), onset (1 in the first record)
//! ```

pub mod decode;
pub mod model;

use model::{Model, Outputs, KEEP, N_CONTOUR, N_NOTES, OVERLAP, OVERLAP_FRAMES, SR, WIN, WIN_HOP};

pub const VERSION: u32 = 1;
pub const HEADER_LEN: usize = 8;
pub const NOTE_LEN: usize = 8;
pub const REC_LEN: usize = 5;
/// Note curve records per second.
pub const REC_RATE: f64 = 100.0;
/// Most capture channels.
pub const MAX_CHANNELS: usize = 8;
const N_CQT: usize = 309;

/// An instrument profile: which notes a take may hold and how many channels by default
/// (docs/poly.md "Instruments": guitar E2 to E6, piano A0 to C8, other A1 to C7).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Profile {
    /// Lowest and highest MIDI note kept.
    pub lo: i32,
    pub hi: i32,
    pub channels: usize,
}

pub const GUITAR: Profile = Profile { lo: 40, hi: 88, channels: 6 };
pub const PIANO: Profile = Profile { lo: 21, hi: 108, channels: 8 };
pub const GENERIC: Profile = Profile { lo: 33, hi: 96, channels: 8 };

/// Options for a poly pass. `Options::from_floats` reads the flat form: instrument (0 guitar,
/// 1 piano, 2 other), channels (1..8; 0 the instrument's default), lowest and highest MIDI note
/// (0: the instrument's).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    pub lo: i32,
    pub hi: i32,
    pub channels: usize,
}

impl Options {
    pub fn profile(p: Profile) -> Self {
        Options { lo: p.lo, hi: p.hi, channels: p.channels }
    }

    /// No range and no channel limit: the model's own notes, for comparing with the reference.
    pub fn unlimited() -> Self {
        Options { lo: model::MIDI_OFFSET, hi: model::MIDI_OFFSET + N_NOTES as i32 - 1, channels: usize::MAX }
    }

    pub fn from_floats(f: &[f32]) -> Self {
        let get = |i: usize| f.get(i).copied().filter(|x| x.is_finite()).unwrap_or(0.0);
        let p = match get(0) as i32 {
            1 => PIANO,
            2 => GENERIC,
            _ => GUITAR,
        };
        let ch = get(1).round() as usize;
        let (lo, hi) = (get(2).round() as i32, get(3).round() as i32);
        let lo = if lo > 0 { lo.clamp(21, 108) } else { p.lo };
        let hi = if hi > 0 { hi.clamp(lo, 108) } else { p.hi.max(lo) };
        Options { lo, hi, channels: if ch == 0 { p.channels } else { ch.clamp(1, MAX_CHANNELS) } }
    }
}

/// One note of a poly take.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PolyNote {
    pub t: f64,
    pub dur: f64,
    pub midi: i32,
    /// Basic Pitch's amplitude: the mean note activation over the note.
    pub amp: f32,
    pub vel: f32,
    pub hard: f32,
    pub first: usize,
    pub n: usize,
}

impl PolyNote {
    pub fn cents(&self) -> f32 {
        ((self.midi - 12) * 100) as f32
    }
}

#[derive(Clone, Debug, Default)]
pub struct PolyResult {
    pub notes: Vec<PolyNote>,
    /// REC_LEN floats per record.
    pub records: Vec<f32>,
    /// Model frames read (86 per second).
    pub frames: usize,
    pub seconds: f64,
}

impl PolyResult {
    pub fn to_floats(&self, out: &mut Vec<f32>) {
        out.clear();
        out.extend_from_slice(&[
            VERSION as f32,
            self.notes.len() as f32,
            (self.records.len() / REC_LEN) as f32,
            NOTE_LEN as f32,
            REC_LEN as f32,
            self.frames as f32,
            self.seconds as f32,
            0.0,
        ]);
        for n in &self.notes {
            out.extend_from_slice(&[n.t as f32, n.dur as f32, n.cents(), n.vel, n.hard, n.first as f32, n.n as f32, n.midi as f32]);
        }
        out.extend_from_slice(&self.records);
    }
}

/// Streaming sinc resampler to 22050 Hz (Kaiser-windowed, cutoff 0.95 of the new Nyquist).
struct Resampler {
    /// Input samples per output, as a fraction num / den in lowest terms.
    num: u64,
    den: u64,
    half: usize,
    /// One row of 2 * half taps per phase (den phases), when den is small enough to tabulate.
    table: Vec<f32>,
    fc: f64,
    /// Input not yet consumed, from input index `base`.
    buf: Vec<f32>,
    base: u64,
    /// Next output index.
    next: u64,
}

const ZERO_CROSSINGS: f64 = 48.0;
const KAISER_BETA: f64 = 9.0;
const MAX_PHASES: u64 = 2048;

fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term, mut k) = (1.0, 1.0, 1.0);
    while term > 1e-12 * sum {
        term *= (x / (2.0 * k)) * (x / (2.0 * k));
        sum += term;
        k += 1.0;
    }
    sum
}

impl Resampler {
    fn new(sr: u32) -> Self {
        let g = gcd(sr as u64, SR as u64);
        let (num, den) = (sr as u64 / g, SR as u64 / g);
        // Cutoff, in cycles per input sample.
        let fc = 0.5 * 0.95 * (SR as f64 / sr as f64).min(1.0);
        let half = (ZERO_CROSSINGS / (2.0 * fc)).ceil() as usize;
        let mut r = Resampler { num, den, half, table: Vec::new(), fc, buf: Vec::new(), base: 0, next: 0 };
        if den <= MAX_PHASES {
            let mut row = vec![0.0f32; 2 * half];
            for p in 0..den {
                r.taps(p as f64 / den as f64, &mut row);
                r.table.extend_from_slice(&row);
            }
        }
        r
    }

    /// Taps for an output at fraction `frac` past input sample i: tap m reads input i + 1 - half + m.
    fn taps(&self, frac: f64, out: &mut [f32]) {
        let mut sum = 0.0f64;
        let mut tmp = vec![0.0f64; out.len()];
        let i0 = bessel_i0(KAISER_BETA);
        let w = self.half as f64;
        for (m, t) in tmp.iter_mut().enumerate() {
            let x = (m as f64 + 1.0 - w) - frac;
            let sinc = if x == 0.0 { 2.0 * self.fc } else { (2.0 * std::f64::consts::PI * self.fc * x).sin() / (std::f64::consts::PI * x) };
            let r = x / w;
            let win = if r.abs() >= 1.0 { 0.0 } else { bessel_i0(KAISER_BETA * (1.0 - r * r).sqrt()) / i0 };
            *t = sinc * win;
            sum += *t;
        }
        for (o, t) in out.iter_mut().zip(tmp) {
            *o = (t / sum) as f32;
        }
    }

    /// Add input; push every output whose taps are all in.
    fn feed(&mut self, x: &[f32], out: &mut Vec<f32>) {
        if self.num == self.den {
            // Already at 22050 Hz: nothing to do (as librosa does).
            out.extend(x.iter().map(|v| if v.is_finite() { *v } else { 0.0 }));
            self.next += x.len() as u64;
            return;
        }
        self.buf.extend(x.iter().map(|v| if v.is_finite() { *v } else { 0.0 }));
        self.drain(out);
    }

    /// Outputs up to `total` (the rest of the take), reading zeros past the end.
    fn finish(&mut self, total: u64, out: &mut Vec<f32>) {
        if self.num == self.den {
            return;
        }
        self.buf.extend(std::iter::repeat_n(0.0, 2 * self.half + 2));
        while self.next < total {
            let before = self.next;
            self.drain(out);
            if self.next == before {
                self.buf.extend(std::iter::repeat_n(0.0, 2 * self.half + 2));
            }
        }
        out.truncate(out.len() - (self.next - total) as usize);
    }

    fn drain(&mut self, out: &mut Vec<f32>) {
        let mut row = vec![0.0f32; 2 * self.half];
        let end = self.base + self.buf.len() as u64;
        loop {
            let pos = self.next * self.num;
            let (i, p) = (pos / self.den, pos % self.den);
            // Inputs i + 1 - half ..= i + half.
            if i + self.half as u64 >= end {
                break;
            }
            let lo = i as i64 + 1 - self.half as i64;
            let taps: &[f32] = if self.table.is_empty() {
                self.taps(p as f64 / self.den as f64, &mut row);
                &row
            } else {
                &self.table[p as usize * 2 * self.half..(p as usize + 1) * 2 * self.half]
            };
            let mut acc = [0.0f32; 4];
            for (m, &h) in taps.iter().enumerate() {
                let k = lo + m as i64;
                if k >= self.base as i64 {
                    acc[m % 4] += h * self.buf[(k - self.base as i64) as usize];
                }
            }
            out.push((acc[0] + acc[1]) + (acc[2] + acc[3]));
            self.next += 1;
        }
        // Drop input no later output needs.
        let pos = self.next * self.num / self.den;
        let keep_from = (pos as i64 + 1 - self.half as i64).max(self.base as i64) as u64;
        let drop = (keep_from - self.base) as usize;
        if drop > 65536 {
            self.buf.drain(..drop);
            self.base = keep_from;
        }
    }
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// The model's outputs for the whole take, frame by frame, kept compact: notes and onsets as f32
/// (the decoding needs them exactly), the pitch contours and the constant-Q spectrum in bytes
/// (they only shape the note curves).
#[derive(Default)]
pub struct Activations {
    pub frames: usize,
    pub note: Vec<f32>,
    pub onset: Vec<f32>,
    /// Contour activation x 255, rounded.
    pub contour: Vec<u8>,
    /// Constant-Q power in half-dB steps above -100 dB (0..255), 309 bins from 27.5 Hz.
    pub cqt: Vec<u8>,
}

impl Activations {
    pub fn contour_at(&self, t: usize, b: usize) -> f32 {
        self.contour[t * N_CONTOUR + b] as f32 / 255.0
    }

    pub fn cqt_db(&self, t: usize, b: usize) -> f32 {
        self.cqt[t * N_CQT + b] as f32 * 0.5 - 100.0
    }
}

/// A take being read: audio in, model windows run as soon as they are full.
pub struct PolyStream {
    model: Model,
    rs: Resampler,
    sr: u32,
    opts: Options,
    /// Input samples read.
    read: u64,
    /// 22050 Hz audio not yet fully used, from sample `at` of the padded take (OVERLAP / 2 zeros
    /// first, as Basic Pitch pads).
    audio: Vec<f32>,
    at: usize,
    out: Outputs,
    pub acts: Activations,
}

impl PolyStream {
    pub fn new(sr: f32, opts: Options) -> Self {
        let sr = sr.round().max(8000.0) as u32;
        PolyStream {
            model: Model::new(),
            rs: Resampler::new(sr),
            sr,
            opts,
            read: 0,
            audio: vec![0.0; OVERLAP / 2],
            at: 0,
            out: Outputs { note: Vec::new(), onset: Vec::new(), contour: Vec::new() },
            acts: Activations::default(),
        }
    }

    /// The take's next samples (mono, at the rate given to `new`).
    pub fn feed(&mut self, x: &[f32]) {
        self.read += x.len() as u64;
        self.rs.feed(x, &mut self.audio);
        while self.audio.len() >= WIN {
            self.window();
        }
    }

    /// Run one window from the start of `audio` (zero-padded at the end), keep its middle frames,
    /// and move on by WIN_HOP.
    fn window(&mut self) {
        if self.audio.len() < WIN {
            self.audio.resize(WIN, 0.0);
        }
        self.model.run(&self.audio[..WIN], &mut self.out);
        let lo = OVERLAP_FRAMES / 2;
        for t in lo..lo + KEEP {
            self.acts.note.extend_from_slice(&self.out.note[t * N_NOTES..(t + 1) * N_NOTES]);
            self.acts.onset.extend_from_slice(&self.out.onset[t * N_NOTES..(t + 1) * N_NOTES]);
            self.acts.contour.extend(self.out.contour[t * N_CONTOUR..(t + 1) * N_CONTOUR].iter().map(|v| (v * 255.0).round() as u8));
            self.acts.cqt.extend(self.model.cqt_db(t).iter().map(|&db| ((db + 100.0) * 2.0).round().clamp(0.0, 255.0) as u8));
        }
        self.acts.frames += KEEP;
        self.audio.drain(..WIN_HOP);
        self.at += WIN_HOP;
    }

    /// The take has ended: run the windows left and trim the outputs to the take's length.
    fn flush(&mut self) -> usize {
        let total = (self.read as f64 * SR as f64 / self.sr as f64).ceil() as u64;
        self.rs.finish(total, &mut self.audio);
        // Basic Pitch runs a window from every WIN_HOP of the padded take.
        let padded = OVERLAP / 2 + total as usize;
        while self.at < padded {
            self.window();
        }
        let keep = (total as f64 * (SR / model::HOP as u32) as f64 / SR as f64).floor() as usize;
        let keep = keep.min(self.acts.frames);
        self.acts.frames = keep;
        self.acts.note.truncate(keep * N_NOTES);
        self.acts.onset.truncate(keep * N_NOTES);
        self.acts.contour.truncate(keep * N_CONTOUR);
        self.acts.cqt.truncate(keep * N_CQT);
        keep
    }

    /// End the take and give the model's outputs, undecoded (for checks).
    pub fn activations_until_end(&mut self) -> &Activations {
        self.flush();
        &self.acts
    }

    /// The take has ended: decode the notes and measure their curves.
    pub fn finish(mut self) -> PolyResult {
        let keep = self.flush();
        let raw = decode::notes(&self.acts, &decode::Settings::with_range(self.opts.lo, self.opts.hi));
        let mut res = curves(&self.acts, &raw);
        res.notes = cap_channels(res.notes, &self.acts, self.opts.channels);
        // Records stay with their notes; drop those of dropped notes.
        let mut recs = Vec::with_capacity(res.records.len());
        for n in &mut res.notes {
            let first = recs.len() / REC_LEN;
            recs.extend_from_slice(&res.records[n.first * REC_LEN..(n.first + n.n) * REC_LEN]);
            n.first = first;
        }
        res.records = recs;
        res.frames = keep;
        res.seconds = self.read as f64 / self.sr as f64;
        res
    }
}

/// The whole pass over a take held in memory.
pub fn poly_pass(audio: &[f32], sr: f32, opts: Options) -> PolyResult {
    let mut s = PolyStream::new(sr, opts);
    for c in audio.chunks(1 << 16) {
        s.feed(c);
    }
    s.finish()
}

/// Seconds per model frame.
pub const FRAME_S: f64 = model::HOP as f64 / SR as f64;

/// The constant-Q bin of MIDI note `midi`'s partial `k` (1 = the fundamental), if in range.
fn partial_bin(midi: i32, k: usize) -> Option<usize> {
    let b = 3 * (midi - model::MIDI_OFFSET) as isize + (36.0 * (k as f64).log2()).round() as isize;
    (0..N_CQT as isize).contains(&b).then_some(b as usize)
}

/// Power sum (dB) of partials `ks` of a note at frame t, each read as the strongest of its bin and
/// the two beside it (a partial can sit a third of a semitone off, and strings run sharp).
fn partials_db(a: &Activations, t: usize, midi: i32, ks: std::ops::RangeInclusive<usize>) -> f32 {
    let mut p = 0.0f64;
    for k in ks {
        let Some(b) = partial_bin(midi, k) else { continue };
        let db = (b.saturating_sub(1)..=(b + 1).min(N_CQT - 1)).map(|j| a.cqt_db(t, j)).fold(f32::MIN, f32::max);
        p += 10f64.powf(db as f64 / 10.0);
    }
    if p > 0.0 { (10.0 * p.log10()) as f32 } else { -100.0 }
}

/// Mean dB of partials `ks` (each the strongest of its bin and the two beside it).
fn partials_mean_db(a: &Activations, t: usize, midi: i32, ks: std::ops::RangeInclusive<usize>) -> f32 {
    let (mut s, mut n) = (0.0f32, 0);
    for k in ks {
        let Some(b) = partial_bin(midi, k) else { continue };
        s += (b.saturating_sub(1)..=(b + 1).min(N_CQT - 1)).map(|j| a.cqt_db(t, j)).fold(f32::MIN, f32::max);
        n += 1;
    }
    if n > 0 { s / n as f32 } else { f32::NAN }
}

/// The pitch offset (cents) of a note at frame t from its contour: the strongest contour bin within
/// 25 bins of the note's, weighted by a Gaussian (sd 5 bins) as Basic Pitch's pitch bends are, then
/// refined between its neighbours by a parabola.
fn pitch_offset(a: &Activations, t: usize, midi: i32) -> f32 {
    let c0 = 3 * (midi - model::MIDI_OFFSET) as isize;
    let (mut best, mut at) = (f32::MIN, c0);
    for d in -25isize..=25 {
        let b = c0 + d;
        if !(0..N_CONTOUR as isize).contains(&b) {
            continue;
        }
        let g = (-0.5 * (d as f32 / 5.0).powi(2)).exp();
        let v = a.contour_at(t, b as usize) * g;
        if v > best {
            best = v;
            at = b;
        }
    }
    let v = |b: isize| if (0..N_CONTOUR as isize).contains(&b) { a.contour_at(t, b as usize) } else { 0.0 };
    let (l, m, r) = (v(at - 1), v(at), v(at + 1));
    let den = l - 2.0 * m + r;
    let frac = if den < 0.0 { (0.5 * (l - r) / den).clamp(-0.5, 0.5) } else { 0.0 };
    ((at - c0) as f32 + frac) * (100.0 / 3.0)
}

/// Each note's curve, and its velocity and hardness (docs/poly.md "Per-note features").
fn curves(a: &Activations, raw: &[decode::RawNote]) -> PolyResult {
    // Levels first: the take's loudest note level and usual tilt set the scales.
    let level = |t: usize, midi: i32| partials_db(a, t.min(a.frames.saturating_sub(1)), midi, 1..=8);
    let tilt = |t: usize, midi: i32| {
        let t = t.min(a.frames.saturating_sub(1));
        partials_mean_db(a, t, midi, 5..=10) - partials_mean_db(a, t, midi, 1..=4)
    };
    let mut top = f32::MIN;
    let mut tilts = Vec::new();
    for n in raw {
        for t in n.start..n.end {
            top = top.max(level(t, n.midi));
            let x = tilt(t, n.midi);
            if x.is_finite() {
                tilts.push(x);
            }
        }
    }
    tilts.sort_by(f32::total_cmp);
    let usual = tilts.get(tilts.len() / 2).copied().unwrap_or(0.0);
    let vol = |db: f32| ((db - top) / VOLUME_RANGE_DB + 1.0).clamp(0.0, 1.0);
    let mut res = PolyResult::default();
    for n in raw {
        let (t0, t1) = (decode::frame_time(n.start), decode::frame_time(n.end));
        let dur = (t1 - t0).max(FRAME_S);
        // Hardness: how fast the note's level rose into its start, dB per ms over the frames
        // around it, 0..1 over 0..HARD_DB_PER_MS.
        let before = level(n.start.saturating_sub(1), n.midi);
        let peak = (n.start..(n.start + 3).min(n.end.max(n.start + 1))).map(|t| level(t, n.midi)).fold(f32::MIN, f32::max);
        let rise_ms = (peak - before).max(0.0) / (2.0 * FRAME_S as f32 * 1000.0);
        let hard = (rise_ms / HARD_DB_PER_MS).clamp(0.0, 1.0);
        let count = ((dur * REC_RATE).round() as usize).max(1);
        let first = res.records.len() / REC_LEN;
        for r in 0..count {
            let x = n.start as f64 + (r as f64 / REC_RATE) / FRAME_S;
            let t = (x.round() as usize).min(n.end.saturating_sub(1)).max(n.start).min(a.frames.saturating_sub(1));
            let i = tilt(t, n.midi);
            res.records.extend_from_slice(&[
                pitch_offset(a, t, n.midi),
                vol(level(t, n.midi)),
                if i.is_finite() { (0.5 + (i - usual) / (2.0 * INTENSITY_DB)).clamp(0.0, 1.0) } else { 0.5 },
                hard,
                if r == 0 { 1.0 } else { 0.0 },
            ]);
        }
        let vel = res.records[first * REC_LEN + 1];
        res.notes.push(PolyNote { t: t0, dur, midi: n.midi, amp: n.amp, vel, hard, first, n: count });
    }
    res
}

/// Volume runs from 0 this far below the take's loudest note to 1 at it.
const VOLUME_RANGE_DB: f32 = 48.0;
/// Tilt either side of the take's usual one that spans intensity 0..1.
const INTENSITY_DB: f32 = 10.0;
/// A rise this fast (dB per ms) or faster is hardness 1.
const HARD_DB_PER_MS: f32 = 1.0;

/// Keep at most `n` notes sounding at once (docs/poly.md "Voice allocation"): a note takes a free
/// channel, preferring the one whose last note was nearest in pitch and ended within the last
/// second, else the one free longest. With every channel busy it takes the channel of the
/// quietest sounding note when it is at least 6 dB louder than that note is then (which ends
/// there); otherwise it is dropped. `notes` are in start order.
fn cap_channels(notes: Vec<PolyNote>, a: &Activations, n: usize) -> Vec<PolyNote> {
    if n >= notes.len() {
        return notes;
    }
    struct Ch {
        note: Option<usize>,
        end: f64,
        midi: i32,
    }
    let mut ch: Vec<Ch> = (0..n).map(|_| Ch { note: None, end: f64::NEG_INFINITY, midi: 0 }).collect();
    let mut kept: Vec<PolyNote> = Vec::with_capacity(notes.len());
    let level = |note: &PolyNote, t: f64| {
        let f = ((t / FRAME_S).round() as usize).min(a.frames.saturating_sub(1));
        partials_db(a, f, note.midi, 1..=8)
    };
    for note in notes {
        let t = note.t;
        for c in ch.iter_mut() {
            if c.note.is_some() && c.end <= t {
                c.note = None;
            }
        }
        let free = ch
            .iter()
            .enumerate()
            .filter(|(_, c)| c.note.is_none())
            .min_by(|(_, x), (_, y)| {
                let near = |c: &Ch| if t - c.end <= 1.0 { (c.midi - note.midi).abs() } else { i32::MAX };
                near(x).cmp(&near(y)).then(x.end.total_cmp(&y.end))
            })
            .map(|(i, _)| i);
        let slot = match free {
            Some(i) => i,
            None => {
                let (i, q) = ch
                    .iter()
                    .enumerate()
                    .map(|(i, c)| (i, level(&kept[c.note.unwrap()], t)))
                    .min_by(|x, y| x.1.total_cmp(&y.1))
                    .unwrap();
                if level(&note, t) < q + 6.0 {
                    continue;
                }
                let k = ch[i].note.unwrap();
                let old = &mut kept[k];
                let keep_recs = (((t - old.t) * REC_RATE).round() as usize).clamp(1, old.n);
                old.dur = (t - old.t).max(FRAME_S);
                old.n = keep_recs;
                i
            }
        };
        ch[slot] = Ch { note: Some(kept.len()), end: t + note.dur, midi: note.midi };
        kept.push(note);
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_keeps_a_sine_and_the_length() {
        for sr in [48000u32, 44100, 32000] {
            let n = sr as usize;
            let x: Vec<f32> = (0..n).map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / sr as f32).sin()).collect();
            let mut r = Resampler::new(sr);
            let mut out = Vec::new();
            for c in x.chunks(1000) {
                r.feed(c, &mut out);
            }
            r.finish(22050, &mut out);
            assert_eq!(out.len(), 22050);
            let err = (2000..20000).map(|i| (out[i] - (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 22050.0).sin()).abs()).fold(0.0f32, f32::max);
            assert!(err < 2e-3, "{sr}: {err}");
        }
    }

    #[test]
    fn options_read_the_profile_and_clamp() {
        assert_eq!(Options::from_floats(&[0.0, 0.0, 0.0, 0.0]), Options::profile(GUITAR));
        assert_eq!(Options::from_floats(&[1.0, 0.0]), Options::profile(PIANO));
        assert_eq!(Options::from_floats(&[2.0, 12.0, 35.0, 0.0]), Options { lo: 35, hi: 96, channels: 8 });
        assert_eq!(Options::from_floats(&[]).channels, 6);
    }
}
