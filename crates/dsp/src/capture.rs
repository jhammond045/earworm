//! The live capture front end (TDD "Live tier" and the fourteen Live follow
//! requirements). Runs causally inside the engine's process call, before the
//! voices, and also over files (analysis, parity tests): same code both ways.
//!
//! Per block it high-passes the input in place, updates the latest `Gesture`
//! (what the synth reads this block) and emits one feature frame per 10 ms
//! into a `FrameSink`. Stored frames are shifted so all channels line up with
//! pitch (requirement 11); the live `Gesture` uses each channel as soon as it
//! exists.

use crate::bands::BandBank;
use crate::filters::HighPass;
use crate::frame::{
    self, silent_frame, Frame, BANDS, BREATHINESS, CONFIDENCE, HARDNESS, LOUDNESS, MAX_BANDS, ONSET, OPENNESS,
    PITCH, SILENT, UNVOICED, VOICED, VOICING,
};
use crate::resample::{decimation_factor, Decimator};
use crate::yin::{Yin, YinEstimate};

/// Latest live values, refreshed every block.
#[derive(Clone, Debug)]
pub struct Gesture {
    /// Raw live pitch in cents above C0, NaN until the first voiced estimate.
    /// After the continuity guard (requirement 8).
    pub pitch_cents: f32,
    pub pitch_conf: f32,
    /// Pitch estimates produced during this block, oldest first, for the tuner
    /// (one per YIN hop, so 0..=MAX_PITCH_UPDATES per block).
    pub pitch_updates: [(f32, f32); MAX_PITCH_UPDATES],
    pub n_pitch_updates: usize,
    /// 0..1 after floor/ceiling calibration.
    pub loudness: f32,
    pub openness: f32,
    pub breathiness: f32,
    /// > 0 only in the block an onset fires (its strength 0..1).
    pub onset: f32,
    /// Hardness of the most recent onset, 0..1.
    pub hardness: f32,
    /// frame::VOICED / UNVOICED / SILENT.
    pub voicing: f32,
    /// Gate open (requirement 9: adaptive floor, hysteresis, 5 ms att / 60 ms rel).
    pub gate: bool,
    /// Gate envelope 0..1 (smoothed, so the synth can fade without clicks).
    pub gate_env: f32,
    /// Vowel shape in dB: loudness and the singer's long-term average removed
    /// (requirements 3, 4, 5). Only `n_bands` entries are meaningful.
    pub bands_db: [f32; MAX_BANDS],
    /// Input level in dBFS (for meters).
    pub input_db: f32,
    /// Current adaptive noise floor in dBFS (for meters).
    pub floor_db: f32,
}

pub const MAX_PITCH_UPDATES: usize = 8;

/// Fixed-capacity output for frames produced in one call. Never reallocates.
pub struct FrameSink {
    frames: Vec<Frame>,
    count: usize,
}

impl FrameSink {
    pub fn with_capacity(n: usize) -> Self {
        Self { frames: vec![crate::frame::silent_frame(); n], count: 0 }
    }
    pub fn clear(&mut self) {
        self.count = 0;
    }
    /// Push a frame; silently drops it if full (callers size for the worst case).
    pub fn push(&mut self, f: &Frame) {
        if self.count < self.frames.len() {
            self.frames[self.count] = *f;
            self.count += 1;
        }
    }
    pub fn frames(&self) -> &[Frame] {
        &self.frames[..self.count]
    }
}

/// Measured analysis delays of the live tier, for take alignment and for the
/// delay report. Seconds.
#[derive(Clone, Copy, Debug)]
pub struct Delays {
    pub level_s: f32,
    pub pitch_s: f32,
}

// Timing. Features are computed on a 2.5 ms tick so the level channels are
// not held back by their own update rate; the onset detector compares
// against the levels two ticks (5 ms) ago, and a frame is emitted every
// four ticks (10 ms).
const TICK_S: f64 = 0.0025;
const TICKS_PER_FRAME: u32 = 4;
const DT: f32 = TICK_S as f32;

/// Nominal band smoothing delay (5 ms attack), the level delay.
const LEVEL_DELAY_S: f32 = 0.005;
/// Largest alignment shift, in frames.
const MAX_ALIGN: usize = 3;
const RING: usize = MAX_ALIGN + 1;

const MED_LEN: usize = 40; // 200 ms of 5 ms pitch hops
const MAX_JUMP_CENTS: f32 = 700.0;
/// Confidence needed for the first estimate of a new pitch region.
const SEED_CONFIDENCE: f32 = 0.8;
const PERSIST_HOPS: u32 = 6; // 30 ms
const FLOOR_MIN_DB: f32 = -80.0;
const FLOOR_START_DB: f32 = -60.0;
const ABS_OPEN_DB: f32 = -70.0;

fn db_of_power(p: f32) -> f32 {
    10.0 * p.max(1e-12).log10()
}

pub struct LiveTier {
    n_bands: usize,
    gesture: Gesture,

    hp: HighPass,
    bank: BandBank,
    centres: [f32; MAX_BANDS],
    dec: Decimator,
    yin: Yin,
    pitch_delay_s: f32,
    align_frames: usize,

    // Tick clock.
    tick_len: f64,
    tick_acc: f64,
    tick_in_frame: u32,

    // Per-tick sample accumulators.
    zc_count: u32,
    last_sign: bool,

    // Gate, floor, ceiling.
    floor_db: f32,
    ceil_db: f32,
    gate_open: bool,
    fall_a: f32,
    env_att: f32,
    env_rel: f32,
    zcr_hz: f32,
    zcr_a: f32,
    breath: f32,
    breath_a: f32,

    // Pitch guard.
    conf: f32,
    pitch_cents: f32,
    hist: [f32; MED_LEN],
    hist_n: usize,
    hist_pos: usize,
    cand_cents: f32,
    cand_n: u32,
    unvoiced_run: u32,
    voicing: f32,

    // Singer model.
    avg: [f32; MAX_BANDS],
    n_learn: u32,
    avg_slow_a: f32,
    c_lo: f32,
    c_hi: f32,
    c_init: bool,

    // Onset detector.
    lv1: [f32; MAX_BANDS],
    lv2: [f32; MAX_BANDS],
    tot1: f32,
    tot2: f32,
    flux_mean: f32,
    flux_dev: f32,
    flux_a: f32,
    refractory: u32,
    refine_left: u32,
    refine_max_slope: f32,
    hard_stored: f32,
    onset_max: f32,

    // Frames.
    ring: [Frame; RING],
    ring_head: usize,
}

impl LiveTier {
    pub fn new(sample_rate: f32, n_bands: usize) -> Self {
        let n_bands = n_bands.clamp(1, MAX_BANDS);
        let bank = BandBank::new(sample_rate, n_bands);
        let mut centres = [0.0f32; MAX_BANDS];
        centres[..n_bands].copy_from_slice(bank.centres());
        let factor = decimation_factor(sample_rate);
        let dec = Decimator::new(factor);
        let yin = Yin::new(sample_rate / factor as f32, 70.0, 1000.0, 0.032, 0.005, 0.15);
        let pitch_delay_s = dec.delay_samples() as f32 / sample_rate + yin.latency_s();
        let align_frames = (((pitch_delay_s - LEVEL_DELAY_S) / frame::FRAME_SECONDS).round().max(0.0) as usize).min(MAX_ALIGN);
        let coef = |tau: f32| 1.0 - (-1.0 / (tau * sample_rate)).exp();
        let tick_a = |tau: f32| 1.0 - (-DT / tau).exp();

        let mut gesture = Gesture {
            pitch_cents: f32::NAN,
            pitch_conf: 0.0,
            pitch_updates: [(f32::NAN, 0.0); MAX_PITCH_UPDATES],
            n_pitch_updates: 0,
            loudness: 0.0,
            openness: 0.0,
            breathiness: 0.0,
            onset: 0.0,
            hardness: 0.0,
            voicing: SILENT,
            gate: false,
            gate_env: 0.0,
            bands_db: [0.0; MAX_BANDS],
            input_db: -120.0,
            floor_db: FLOOR_START_DB,
        };
        gesture.floor_db = FLOOR_START_DB;

        Self {
            n_bands,
            gesture,
            hp: HighPass::new(sample_rate, 60.0),
            bank,
            centres,
            dec,
            yin,
            pitch_delay_s,
            align_frames,
            tick_len: sample_rate as f64 * TICK_S,
            tick_acc: 0.0,
            tick_in_frame: 0,
            zc_count: 0,
            last_sign: false,
            floor_db: FLOOR_START_DB,
            ceil_db: FLOOR_START_DB + 30.0,
            gate_open: false,
            fall_a: tick_a(0.05),
            env_att: coef(0.005),
            env_rel: coef(0.060),
            zcr_hz: 0.0,
            zcr_a: tick_a(0.010),
            breath: 0.0,
            breath_a: tick_a(0.010),
            conf: 0.0,
            pitch_cents: f32::NAN,
            hist: [0.0; MED_LEN],
            hist_n: 0,
            hist_pos: 0,
            cand_cents: 0.0,
            cand_n: 0,
            unvoiced_run: 0,
            voicing: SILENT,
            avg: [0.0; MAX_BANDS],
            n_learn: 0,
            avg_slow_a: tick_a(4.0),
            c_lo: 0.0,
            c_hi: 0.0,
            c_init: false,
            lv1: [-120.0; MAX_BANDS],
            lv2: [-120.0; MAX_BANDS],
            tot1: -120.0,
            tot2: -120.0,
            flux_mean: 0.0,
            flux_dev: 0.0,
            flux_a: tick_a(0.3),
            refractory: 0,
            refine_left: 0,
            refine_max_slope: 0.0,
            hard_stored: 0.0,
            onset_max: 0.0,
            ring: [silent_frame(); RING],
            ring_head: 0,
        }
    }

    pub fn n_bands(&self) -> usize {
        self.n_bands
    }

    /// Process one block of mic input. `input` is high-passed in place (the
    /// caller then copies it to the mic ring). Frames completed during the
    /// block are pushed to `sink` (the caller clears it).
    pub fn process(&mut self, input: &mut [f32], sink: &mut FrameSink) {
        self.gesture.n_pitch_updates = 0;
        self.gesture.onset = 0.0;
        if input.is_empty() {
            return;
        }

        let mut sum_sq = 0.0f32;
        for s in input.iter_mut() {
            let raw = if s.is_finite() { *s } else { 0.0 };
            let x = self.hp.process(raw);
            *s = x;
            sum_sq += x * x;

            self.bank.process(x);

            // Gate envelope, per sample so the synth can fade without clicks.
            let target = if self.gate_open { 1.0 } else { 0.0 };
            let a = if target > self.gesture.gate_env { self.env_att } else { self.env_rel };
            self.gesture.gate_env += a * (target - self.gesture.gate_env);

            // Zero crossings for the voicing decision.
            let sign = x >= 0.0;
            if sign != self.last_sign {
                self.zc_count += 1;
                self.last_sign = sign;
            }

            // Pitch path.
            if let Some(d) = self.dec.process(x) {
                if let Some(est) = self.yin.push(d) {
                    self.on_pitch(est);
                }
            }

            self.tick_acc += 1.0;
            if self.tick_acc >= self.tick_len {
                self.tick_acc -= self.tick_len;
                self.tick(sink);
            }
        }
        self.gesture.input_db = db_of_power(sum_sq / input.len() as f32);
        self.gesture.floor_db = self.floor_db;
    }

    pub fn gesture(&self) -> &Gesture {
        &self.gesture
    }

    /// Forget the learned singer average, openness range and noise floor.
    pub fn reset_singer(&mut self) {
        self.avg = [0.0; MAX_BANDS];
        self.n_learn = 0;
        self.c_init = false;
        self.floor_db = FLOOR_START_DB;
        self.ceil_db = FLOOR_START_DB + 30.0;
        self.gesture.floor_db = self.floor_db;
    }

    /// How much later stored frames put every channel than the level analysis does: the frame
    /// sink shifts them to line up with pitch, by whole frames (seconds).
    pub fn align_s(&self) -> f32 {
        self.align_frames as f32 * frame::FRAME_SECONDS
    }

    /// Nominal analysis delays (constants derived from filter/window sizes).
    pub fn delays(&self) -> Delays {
        Delays { level_s: LEVEL_DELAY_S, pitch_s: self.pitch_delay_s }
    }

    /// Handle one YIN estimate (every ~5 ms). Continuity guard, requirement 8:
    /// an estimate more than 700 cents from the median of the last 200 ms is
    /// rejected unless the new region has persisted for 30 ms (6 hops in a
    /// row). Rejected and unvoiced estimates go to the tuner as NaN so it
    /// holds its last note.
    fn on_pitch(&mut self, est: YinEstimate) {
        let conf = est.confidence;
        self.conf = conf;
        self.gesture.pitch_conf = conf;
        let mut update = (f32::NAN, conf);

        // A new region (empty history) needs a confident estimate to seed the
        // guard. The first YIN windows after silence are part silence and can
        // land on a harmonic with middling confidence; seeding the median with
        // that would then reject the right pitch for the whole 30 ms.
        let min_conf = if self.hist_n == 0 { SEED_CONFIDENCE } else { 0.5 };
        if est.hz > 0.0 && conf >= min_conf {
            let cents = crate::hz_to_cents(est.hz);
            self.unvoiced_run = 0;
            let mut accept = true;
            if self.hist_n > 0 {
                let med = self.median();
                if (cents - med).abs() > MAX_JUMP_CENTS {
                    if self.cand_n > 0 && (cents - self.cand_cents).abs() < 300.0 {
                        self.cand_n += 1;
                    } else {
                        self.cand_n = 1;
                    }
                    self.cand_cents = cents;
                    if self.cand_n >= PERSIST_HOPS {
                        // The new region is real: forget the old one.
                        self.hist_n = 0;
                        self.hist_pos = 0;
                    } else {
                        accept = false;
                    }
                } else {
                    self.cand_n = 0;
                }
            }
            if accept {
                self.cand_n = 0;
                self.hist[self.hist_pos] = cents;
                self.hist_pos = (self.hist_pos + 1) % MED_LEN;
                self.hist_n = (self.hist_n + 1).min(MED_LEN);
                self.pitch_cents = cents;
                update = (cents, conf);
            }
        } else {
            self.cand_n = 0;
            self.unvoiced_run += 1;
            // After 300 of silence or noise the next note may be anywhere.
            if self.unvoiced_run > 60 {
                self.hist_n = 0;
                self.hist_pos = 0;
            }
        }

        self.gesture.pitch_cents = self.pitch_cents;
        let n = self.gesture.n_pitch_updates;
        if n < MAX_PITCH_UPDATES {
            self.gesture.pitch_updates[n] = update;
            self.gesture.n_pitch_updates = n + 1;
        }
    }

    fn median(&self) -> f32 {
        let mut tmp = [0.0f32; MED_LEN];
        let n = self.hist_n;
        tmp[..n].copy_from_slice(&self.hist[..n]);
        // Insertion sort; n <= 40.
        for i in 1..n {
            let v = tmp[i];
            let mut j = i;
            while j > 0 && tmp[j - 1] > v {
                tmp[j] = tmp[j - 1];
                j -= 1;
            }
            tmp[j] = v;
        }
        tmp[n / 2]
    }

    /// One 2.5 ms feature update.
    fn tick(&mut self, sink: &mut FrameSink) {
        let n = self.n_bands;
        let mut lv = [0.0f32; MAX_BANDS];
        self.bank.levels_db(&mut lv[..n]);
        let (total_p, hf_p) = {
            let pw = self.bank.power();
            let mut t = 0.0;
            let mut h = 0.0;
            for i in 0..n {
                t += pw[i];
                if self.centres[i] >= 4000.0 {
                    h += pw[i];
                }
            }
            (t, h)
        };
        let total_db = db_of_power(total_p);
        let hf_share = if total_p > 1e-12 { (hf_p / total_p).clamp(0.0, 1.0) } else { 0.0 };

        let zcr_inst = self.zc_count as f32 / DT;
        self.zc_count = 0;
        self.zcr_hz += self.zcr_a * (zcr_inst - self.zcr_hz);

        // Noise floor: falls quickly, rises 3 dB/s while the gate is closed
        // and only 0.3 dB/s while it is open, so a held note does not drag
        // the floor up and shut the gate on itself.
        if total_db < self.floor_db {
            self.floor_db += self.fall_a * (total_db - self.floor_db);
        } else {
            let rate = if self.gate_open { 0.3 } else { 3.0 };
            self.floor_db = (self.floor_db + rate * DT).min(total_db);
        }
        self.floor_db = self.floor_db.clamp(FLOOR_MIN_DB, -20.0);

        // Gate with hysteresis. The absolute minimum keeps converter noise
        // from opening it when the floor has fallen to digital silence.
        let open_thr = (self.floor_db + 10.0).max(ABS_OPEN_DB);
        let close_thr = (self.floor_db + 6.0).max(ABS_OPEN_DB - 4.0);
        if self.gate_open {
            if total_db < close_thr {
                self.gate_open = false;
            }
        } else if total_db > open_thr {
            self.gate_open = true;
        }
        self.gesture.gate = self.gate_open;

        // Loudness: 0 at the gate's closing level, 1 at a running ceiling
        // (peak hold, 1 dB/s decay, never closer than 30 dB, nor below -30 dBFS,
        // so one quiet take is not stretched to full scale).
        let lo = self.floor_db + 6.0;
        self.ceil_db = (self.ceil_db - 1.0 * DT).max((self.floor_db + 30.0).max(-30.0));
        if total_db > self.ceil_db {
            self.ceil_db = total_db;
        }
        let loudness = ((total_db - lo) / (self.ceil_db - lo).max(1.0)).clamp(0.0, 1.0);
        self.gesture.loudness = if self.gate_open { loudness } else { 0.0 };

        // Voicing.
        let high_zcr = self.zcr_hz > 5000.0;
        self.voicing = if !self.gate_open {
            SILENT
        } else if self.conf >= 0.5 && self.pitch_cents.is_finite() && hf_share < 0.6 {
            VOICED
        } else if hf_share >= 0.5 || high_zcr {
            UNVOICED
        } else if self.voicing == SILENT {
            // Gate just opened and nothing says "pitched" yet: noise first.
            UNVOICED
        } else {
            self.voicing
        };
        self.gesture.voicing = self.voicing;
        let voiced = self.voicing == VOICED;

        // Normalized vowel shape: band dB minus total dB, so loudness drops
        // out (requirement 4).
        let mut raw = [0.0f32; MAX_BANDS];
        for i in 0..n {
            raw[i] = lv[i] - total_db;
        }
        // Requirement 3, simple version: below 0.8 * f0 a band can only be
        // measuring the fundamental's skirt or nothing, so copy the nearest
        // band at or above f0. Bands that sit between harmonics higher up are
        // not handled; the 16 bands are wider than the harmonic spacing until
        // f0 is well above 500 Hz.
        if voiced && self.pitch_cents.is_finite() {
            let f0 = crate::cents_to_hz(self.pitch_cents);
            let cut = 0.8 * f0;
            if let Some(src) = (0..n).find(|&i| self.centres[i] >= f0) {
                for i in 0..n {
                    if self.centres[i] < cut {
                        raw[i] = raw[src];
                    }
                }
            }
        }

        // Singer average: cumulative mean for the first ~4 s (so a new
        // singer is neutral within a second or two), then a 4 s one-pole.
        // Only voiced, gated frames contribute.
        if voiced {
            self.n_learn = self.n_learn.saturating_add(1);
            let a = self.avg_slow_a.max(1.0 / (self.n_learn as f32 + 20.0));
            for i in 0..n {
                self.avg[i] += a * (raw[i] - self.avg[i]);
            }
        }
        if self.gate_open {
            for i in 0..n {
                self.gesture.bands_db[i] = raw[i] - self.avg[i];
            }
        }

        // Openness: centroid (log frequency, power weighted) of the
        // normalized bands from 300 Hz to 3 kHz, against the singer's range.
        if voiced {
            let (mut sw, mut sc) = (0.0f32, 0.0f32);
            for i in 0..n {
                let f = self.centres[i];
                if (300.0..=3000.0).contains(&f) {
                    let p = 10f32.powf(raw[i] / 10.0);
                    sw += p;
                    sc += p * f.ln();
                }
            }
            if sw > 0.0 {
                let c = sc / sw;
                if !self.c_init {
                    self.c_lo = c - 0.15;
                    self.c_hi = c + 0.15;
                    self.c_init = true;
                }
                self.c_lo = self.c_lo.min(c);
                self.c_hi = self.c_hi.max(c);
                // Range slowly narrows again (30 s), never below 0.3 in ln Hz.
                let span = self.c_hi - self.c_lo;
                if span > 0.3 {
                    let k = (DT / 30.0) * span;
                    self.c_lo += k.min(c - self.c_lo).max(0.0);
                    self.c_hi -= k.min(self.c_hi - c).max(0.0);
                }
                self.gesture.openness = ((c - self.c_lo) / (self.c_hi - self.c_lo).max(0.3)).clamp(0.0, 1.0);
            }
        }

        // Breathiness: weak periodicity at audible level plus share of high
        // band energy.
        let audible = ((total_db - (self.floor_db + 10.0)) / 20.0).clamp(0.0, 1.0);
        let braw = audible * (0.6 * (1.0 - self.conf) + 0.4 * (2.0 * hf_share).min(1.0));
        self.breath += self.breath_a * (braw - self.breath);
        self.gesture.breathiness = self.breath.clamp(0.0, 1.0);

        // Onset: summed positive band rise over 5 ms against mean + 4 dev.
        let mut flux = 0.0f32;
        for i in 0..n {
            flux += (lv[i] - self.lv2[i]).max(0.0);
        }
        let tot_now = total_db.max(-80.0);
        let rise_db = tot_now - self.tot2.max(-80.0);
        let slope = rise_db / 5.0; // dB per ms
        let thr = self.flux_mean + 4.0 * self.flux_dev + 3.0 * n as f32;
        if self.refractory > 0 {
            self.refractory -= 1;
        }
        if self.refractory == 0 && flux > thr && rise_db > 3.0 && total_db > self.floor_db + 10.0 {
            let strength = ((flux - thr) / (6.0 * n as f32)).clamp(0.05, 1.0);
            self.gesture.onset = self.gesture.onset.max(strength);
            self.onset_max = self.onset_max.max(strength);
            self.refractory = (0.040 / TICK_S).round() as u32;
            self.hard_stored = (slope / 0.5).clamp(0.0, 1.0);
            self.gesture.hardness = self.hard_stored;
            self.refine_left = 4;
            self.refine_max_slope = slope;
        } else if self.refine_left > 0 {
            self.refine_left -= 1;
            self.refine_max_slope = self.refine_max_slope.max(slope);
            if self.refine_left == 0 {
                self.hard_stored = (self.refine_max_slope / 0.5).clamp(0.0, 1.0);
                self.gesture.hardness = self.hard_stored;
                // Frames already waiting in the alignment ring get the
                // refined value; harmless for older entries.
                for f in self.ring.iter_mut() {
                    f[HARDNESS] = self.hard_stored;
                }
            }
        }
        let a = self.flux_a;
        self.flux_mean += a * (flux - self.flux_mean);
        self.flux_dev += a * ((flux - self.flux_mean).abs() - self.flux_dev);
        self.lv2 = self.lv1;
        self.lv1[..n].copy_from_slice(&lv[..n]);
        self.tot2 = self.tot1;
        self.tot1 = total_db;

        self.tick_in_frame += 1;
        if self.tick_in_frame >= TICKS_PER_FRAME {
            self.tick_in_frame = 0;
            self.emit_frame(sink);
        }
    }

    fn emit_frame(&mut self, sink: &mut FrameSink) {
        let n = self.n_bands;

        // Level group snapshot goes into the ring; the pitch group is
        // current. Requirement 11: stored frames shift the level group by
        // the pitch path's extra latency, rounded to whole frames.
        let mut cur = silent_frame();
        cur[LOUDNESS] = self.gesture.loudness;
        cur[OPENNESS] = self.gesture.openness;
        cur[ONSET] = self.onset_max;
        cur[HARDNESS] = self.hard_stored;
        cur[BANDS..BANDS + n].copy_from_slice(&self.gesture.bands_db[..n]);
        self.onset_max = 0.0;
        self.ring[self.ring_head] = cur;
        let idx = (self.ring_head + RING - self.align_frames) % RING;
        self.ring_head = (self.ring_head + 1) % RING;

        let mut out = self.ring[idx];
        let voiced = self.voicing == VOICED;
        out[PITCH] = if voiced { self.pitch_cents } else { f32::NAN };
        out[CONFIDENCE] = self.conf;
        out[BREATHINESS] = self.gesture.breathiness;
        out[VOICING] = if self.voicing == UNVOICED || voiced { self.voicing } else { SILENT };
        sink.push(&out);
    }
}

/// Convenience for files: run the live tier over a whole signal in 128-sample
/// blocks and collect every frame. Allocates; not for the audio thread.
pub fn frames_from_signal(signal: &[f32], sample_rate: f32, n_bands: usize) -> Vec<Frame> {
    let mut tier = LiveTier::new(sample_rate, n_bands);
    let mut sink = FrameSink::with_capacity(8);
    let mut out = Vec::new();
    let mut buf = [0.0f32; 128];
    for chunk in signal.chunks(128) {
        let b = &mut buf[..chunk.len()];
        b.copy_from_slice(chunk);
        sink.clear();
        tier.process(b, &mut sink);
        out.extend_from_slice(sink.frames());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{frame_len, FRAME_RATE};
    use crate::noise::Noise;

    const SR: f32 = 48_000.0;

    /// Harmonic voice with a formant envelope, built additively so pitch can
    /// move. `f0` maps sample index to Hz; formants are (centre, bandwidth).
    fn voice(n: usize, sr: f32, amp: f32, f0: impl Fn(usize) -> f32, formants: &[(f32, f32)]) -> Vec<f32> {
        let mut phase = 0.0f32;
        let mut out = vec![0.0; n];
        for (i, o) in out.iter_mut().enumerate() {
            let f = f0(i);
            phase = (phase + f / sr).fract();
            let mut s = 0.0;
            let mut k = 1;
            while (k as f32) * f < 5000.0 {
                let fk = k as f32 * f;
                let mut g = 0.02;
                for &(fc, bw) in formants {
                    let d = (fk - fc) / bw;
                    g += 1.0 / (1.0 + d * d);
                }
                s += g / k as f32 * (std::f32::consts::TAU * k as f32 * phase).sin();
                k += 1;
            }
            *o = amp * s;
        }
        out
    }

    const AH: [(f32, f32); 2] = [(700.0, 90.0), (1100.0, 100.0)];
    const EE: [(f32, f32); 2] = [(300.0, 60.0), (2300.0, 200.0)];

    fn secs(s: f32) -> usize {
        (s * SR) as usize
    }

    fn cents_err(c: f32, hz: f32) -> f32 {
        (c - crate::hz_to_cents(hz)).abs()
    }

    #[test]
    fn silence_is_silent() {
        let f = frames_from_signal(&vec![0.0; secs(1.0)], SR, 16);
        assert!(f.len() >= 99);
        for fr in &f {
            assert_eq!(fr[VOICING], SILENT);
            assert!(fr[PITCH].is_nan());
        }
        let mut t = LiveTier::new(SR, 16);
        let mut sink = FrameSink::with_capacity(8);
        let mut buf = [0.0f32; 128];
        for _ in 0..200 {
            sink.clear();
            t.process(&mut buf, &mut sink);
        }
        assert!(!t.gesture().gate);
        assert!(t.gesture().gate_env < 0.01);
    }

    #[test]
    fn nan_input_is_treated_as_silence() {
        let mut t = LiveTier::new(SR, 16);
        let mut sink = FrameSink::with_capacity(8);
        let mut buf = [f32::NAN; 128];
        buf[3] = f32::INFINITY;
        t.process(&mut buf, &mut sink);
        assert!(buf.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn steady_voice_is_voiced_and_in_tune() {
        let sig = voice(secs(1.5), SR, 0.1, |_| 220.0, &AH);
        let f = frames_from_signal(&sig, SR, 16);
        for fr in &f[40..] {
            assert_eq!(fr[VOICING], VOICED);
            assert!(cents_err(fr[PITCH], 220.0) < 10.0, "pitch {}", fr[PITCH]);
            assert!(fr[CONFIDENCE] > 0.8);
            assert!(fr[BREATHINESS] < 0.3);
        }
    }

    #[test]
    fn pitch_jump_is_followed() {
        let jump = secs(1.0);
        let sig = voice(secs(2.0), SR, 0.1, |i| if i < jump { 220.0 } else { 330.0 }, &AH);
        let f = frames_from_signal(&sig, SR, 16);
        // 330 / 220 is 702 cents: just over the guard, so the 30 ms
        // persistence rule is what lets it through.
        for fr in &f[150..] {
            assert!(cents_err(fr[PITCH], 330.0) < 10.0, "pitch {}", fr[PITCH]);
        }
        let first = f[100..].iter().position(|fr| cents_err(fr[PITCH], 330.0) < 10.0).unwrap();
        assert!(first < 12, "took {first} frames to follow");
    }

    #[test]
    fn short_octave_burst_is_not_followed() {
        let t0 = secs(1.0);
        let t1 = t0 + secs(0.015);
        let sig = voice(secs(2.0), SR, 0.1, |i| if i >= t0 && i < t1 { 440.0 } else { 220.0 }, &AH);
        let f = frames_from_signal(&sig, SR, 16);
        for fr in &f[40..] {
            assert!(fr[PITCH].is_nan() || cents_err(fr[PITCH], 220.0) < 100.0, "pitch {}", fr[PITCH]);
        }
    }

    #[test]
    fn noise_burst_is_unvoiced() {
        let mut nz = Noise::new(7);
        let mut sig = vec![0.0f32; secs(1.0)];
        for s in sig[secs(0.3)..secs(0.7)].iter_mut() {
            *s = 0.2 * nz.next();
        }
        let f = frames_from_signal(&sig, SR, 16);
        let mid: Vec<_> = f[40..65].to_vec();
        let unv = mid.iter().filter(|fr| fr[VOICING] == UNVOICED).count();
        assert!(unv >= 20, "unvoiced frames {unv}/25");
        assert!(mid.iter().all(|fr| fr[VOICING] != VOICED));
        assert!(mid.iter().all(|fr| fr[PITCH].is_nan()));
        assert_eq!(f[5][VOICING], SILENT);
        assert_eq!(f[90][VOICING], SILENT);
    }

    #[test]
    fn onset_fires_once_per_burst() {
        let mut nz = Noise::new(3);
        let mut sig = vec![0.0f32; secs(1.5)];
        let starts = [0.2f32, 0.6, 1.0];
        for st in starts {
            let a = secs(st);
            for (k, s) in sig[a..a + secs(0.05)].iter_mut().enumerate() {
                *s = 0.4 * nz.next() * (-(k as f32) / 600.0).exp();
            }
        }
        let f = frames_from_signal(&sig, SR, 16);
        for st in starts {
            let a = (st * FRAME_RATE) as usize;
            let n = f[a - 3..a + 25].iter().filter(|fr| fr[ONSET] > 0.0).count();
            assert_eq!(n, 1, "onsets near {st}s");
        }
        let total = f.iter().filter(|fr| fr[ONSET] > 0.0).count();
        assert_eq!(total, 3);
        // Hard attacks are hard.
        let h = f.iter().map(|fr| fr[HARDNESS]).fold(0.0, f32::max);
        assert!(h > 0.8, "hardness {h}");
    }

    #[test]
    fn louder_input_gives_higher_loudness() {
        let n = secs(1.0);
        let quiet = voice(n, SR, 0.02, |_| 220.0, &AH);
        let loud = voice(n, SR, 0.15, |_| 220.0, &AH);
        let mut sig = quiet.clone();
        sig.extend(&loud);
        sig.extend(&quiet);
        let f = frames_from_signal(&sig, SR, 16);
        let q = f[80][LOUDNESS];
        let l = f[180][LOUDNESS];
        let q2 = f[280][LOUDNESS];
        assert!(l > 0.8, "loud {l}");
        assert!(q > 0.05 && q < l - 0.2, "quiet {q} loud {l}");
        assert!(q2 < l - 0.2, "quiet again {q2}");
    }

    #[test]
    fn vowels_differ_in_shape_and_openness() {
        let mut sig = voice(secs(1.5), SR, 0.1, |_| 200.0, &AH);
        sig.extend(voice(secs(1.5), SR, 0.1, |_| 200.0, &EE));
        let f = frames_from_signal(&sig, SR, 16);
        let ah = f[140];
        let ee = f[290];
        assert_eq!(ah[VOICING], VOICED);
        assert_eq!(ee[VOICING], VOICED);
        let mut dist = 0.0;
        for i in 0..16 {
            dist += (ah[BANDS + i] - ee[BANDS + i]).abs();
        }
        assert!(dist / 16.0 > 3.0, "mean band difference {} dB", dist / 16.0);
        assert!(ah[OPENNESS] > ee[OPENNESS] + 0.3, "ah {} ee {}", ah[OPENNESS], ee[OPENNESS]);
        // F1 band: ee has far more energy near 300 Hz than ah, the reverse near 700 Hz.
        let mut c = [0.0f32; 16];
        crate::bands::band_centres(16, &mut c);
        let b300 = (0..16).min_by_key(|&i| ((c[i] - 300.0).abs() * 10.0) as i32).unwrap();
        let b700 = (0..16).min_by_key(|&i| ((c[i] - 700.0).abs() * 10.0) as i32).unwrap();
        assert!(ee[BANDS + b300] > ah[BANDS + b300]);
        assert!(ah[BANDS + b700] > ee[BANDS + b700]);
    }

    #[test]
    fn vowel_shape_is_independent_of_level() {
        let mut sig = voice(secs(1.5), SR, 0.1, |_| 200.0, &AH);
        sig.extend(voice(secs(1.5), SR, 0.1, |_| 200.0, &EE));
        let quiet: Vec<f32> = sig.iter().map(|x| x * 0.1).collect();
        let a = frames_from_signal(&sig, SR, 16);
        let b = frames_from_signal(&quiet, SR, 16);
        let mut d = 0.0;
        for i in 0..16 {
            d += (a[140][BANDS + i] - b[140][BANDS + i]).abs();
        }
        assert!(d / 16.0 < 1.5, "level changed the shape by {} dB", d / 16.0);
    }

    fn count_frames(sr: f32, block: usize) -> usize {
        let mut t = LiveTier::new(sr, 16);
        let mut sink = FrameSink::with_capacity(16);
        let secs_total = 2.0;
        let n = (sr * secs_total) as usize;
        let sig = voice(n, sr, 0.1, |_| 200.0, &AH);
        let mut count = 0;
        let mut buf = vec![0.0f32; block];
        for ch in sig.chunks(block) {
            let b = &mut buf[..ch.len()];
            b.copy_from_slice(ch);
            sink.clear();
            t.process(b, &mut sink);
            for fr in sink.frames() {
                assert_eq!(fr.len(), frame::MAX_FRAME_LEN);
            }
            count += sink.frames().len();
        }
        count
    }

    #[test]
    fn frame_rate_is_100_per_second() {
        for &sr in &[48_000.0f32, 44_100.0] {
            for &block in &[128usize, 300] {
                let c = count_frames(sr, block) as i32;
                assert!((c - 200).abs() <= 1, "{c} frames at {sr} Hz, block {block}");
            }
        }
    }

    #[test]
    fn frame_format() {
        assert_eq!(frame_len(16), 24);
        let sig = voice(secs(0.5), SR, 0.1, |_| 220.0, &AH);
        for nb in [16usize, 24] {
            let f = frames_from_signal(&sig, SR, nb);
            let fr = f.last().unwrap();
            assert!(fr[BANDS..BANDS + nb].iter().all(|x| x.is_finite()));
            assert!(fr[BANDS + nb..].iter().all(|&x| x == 0.0));
            for s in [LOUDNESS, OPENNESS, BREATHINESS, ONSET, HARDNESS] {
                assert!((0.0..=1.0).contains(&fr[s]));
            }
        }
    }

    /// Measures the live tier's real delays and checks them against the TDD
    /// budget (pitch under 25 ms, level under 10 ms).
    #[test]
    fn measured_delays_are_within_budget() {
        let block = 16usize; // fine resolution for the measurement
        let t_on = secs(0.5);

        // Pitch: sine onset after silence; first block whose gesture holds
        // the right pitch.
        let sine: Vec<f32> = (0..secs(1.0))
            .map(|i| if i >= t_on { 0.1 * (std::f32::consts::TAU * 440.0 * (i - t_on) as f32 / SR).sin() } else { 0.0 })
            .collect();
        let mut t = LiveTier::new(SR, 16);
        let mut sink = FrameSink::with_capacity(64);
        let mut pitch_delay = None;
        let mut pos = 0;
        for ch in sine.chunks(block) {
            let mut b = ch.to_vec();
            sink.clear();
            t.process(&mut b, &mut sink);
            pos += ch.len();
            let g = t.gesture();
            if pitch_delay.is_none() && pos > t_on && g.pitch_cents.is_finite() && cents_err(g.pitch_cents, 440.0) < 20.0 {
                pitch_delay = Some((pos - t_on) as f32 / SR);
            }
        }
        let pitch_delay = pitch_delay.expect("pitch never found");

        // Level: loudness step; time to reach half of its settled value.
        let step: Vec<f32> = (0..secs(1.0))
            .map(|i| if i >= t_on { 0.1 * (std::f32::consts::TAU * 440.0 * (i - t_on) as f32 / SR).sin() } else { 0.0 })
            .collect();
        let mut t = LiveTier::new(SR, 16);
        let mut level_delay = None;
        let mut pos = 0;
        let mut settled = 0.0;
        for ch in step.chunks(block) {
            let mut b = ch.to_vec();
            sink.clear();
            t.process(&mut b, &mut sink);
            pos += ch.len();
            settled = t.gesture().loudness;
            if level_delay.is_none() && pos > t_on && settled > 0.5 {
                level_delay = Some((pos - t_on) as f32 / SR);
            }
        }
        assert!(settled > 0.9);
        let level_delay = level_delay.expect("loudness never rose");

        let d = t.delays();
        eprintln!(
            "measured: pitch delay {:.1} ms (nominal {:.1} ms), level delay {:.1} ms (nominal {:.1} ms)",
            pitch_delay * 1000.0,
            d.pitch_s * 1000.0,
            level_delay * 1000.0,
            d.level_s * 1000.0
        );
        assert!(pitch_delay < 0.025, "pitch delay {pitch_delay}");
        assert!(level_delay < 0.010, "level delay {level_delay}");
    }
}
