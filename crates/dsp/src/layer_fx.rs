//! Per-layer effects for the Klont (TDD "Klont DSP" table): crush, tape,
//! delay, reverb and output width.
//!
//! Same conventions as `fx`: `set` at control rate, `process` per sample,
//! no allocation after `new`, finite output for any input, `DENORMAL` in
//! every feedback path, seeded randomness.

use crate::delay::DelayLine;
use crate::filters::OnePole;
use crate::noise::Noise;
use crate::DENORMAL;
use std::f32::consts::TAU;

/// Input guard: NaN/inf become 0 and huge values are clamped so no state can
/// overflow.
#[inline]
fn clean(x: f32) -> f32 {
    if x.is_finite() {
        x.clamp(-1e6, 1e6)
    } else {
        0.0
    }
}

#[inline]
fn fin(x: f32, default: f32) -> f32 {
    if x.is_finite() {
        x
    } else {
        default
    }
}

/// Crush: fractional sample-and-hold at `rate_hz` plus a bit quantiser with
/// TPDF dither, for signals in -1..1. At bits >= 16 and rate >= the sample
/// rate `process` returns x unchanged.
///
/// The quantiser is mid-tread with a step of 2^(1-bits) and +-1 step TPDF
/// dither, applied when a value is captured, so held values stay constant.
pub struct Crush {
    sr: f32,
    noise: Noise,
    quant: bool,
    hold: bool,
    step: f32,
    inv_step: f32,
    inc: f32,
    phase: f32,
    held: f32,
}

impl Crush {
    pub fn new(sample_rate: f32, seed: u32) -> Self {
        Self {
            sr: sample_rate.max(1.0),
            noise: Noise::new(seed),
            quant: false,
            hold: false,
            step: 1.0,
            inv_step: 1.0,
            inc: 1.0,
            phase: 1.0,
            held: 0.0,
        }
    }
    pub fn set(&mut self, bits: f32, rate_hz: f32) {
        let bits = fin(bits, 16.0).clamp(1.0, 16.0);
        let rate = fin(rate_hz, self.sr).clamp(1.0, self.sr);
        self.quant = bits < 16.0;
        self.step = (1.0 - bits).exp2();
        self.inv_step = 1.0 / self.step;
        self.hold = rate < self.sr;
        self.inc = rate / self.sr;
    }
    #[inline]
    fn q(&mut self, x: f32) -> f32 {
        if !self.quant {
            return x;
        }
        let d = 0.5 * (self.noise.next() + self.noise.next());
        (x * self.inv_step + d).round() * self.step
    }
    pub fn process(&mut self, x: f32) -> f32 {
        let x = if x.is_finite() { x } else { 0.0 };
        if self.hold {
            self.phase += self.inc;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
                self.held = self.q(x);
            }
            self.held
        } else {
            self.q(x)
        }
    }
}

/// Tape: a modulated short delay (about 5 ms centre) with slow wow (~0.5 Hz),
/// fast flutter (~7 Hz, partly random) and random short dropouts (gain dips
/// of a few ms), all scaled by `amt` 0..1. At amt 0 `process` returns x
/// unchanged (no added latency). Depth at amt 1: wow about 30 cents, flutter
/// about 8 cents, a dropout every second or so.
///
/// Implementation notes: wow at 30 cents peak needs 5.5 ms of delay swing, so
/// the centre is 6.5 ms. Between amt 0 and about 0.125 the output crossfades
/// from dry to the modulated delay, so turning it on does not click. Below
/// that crossfade the amount is smoothed (30 ms).
pub struct Tape {
    sr: f32,
    line: DelayLine,
    noise: Noise,
    amt_t: f32,
    amt: f32,
    a_amt: f32,
    wow_ph: f32,
    wow_inc: f32,
    fl_ph: f32,
    fl_inc: f32,
    rnd: f32,
    rnd2: f32,
    rnd_a: f32,
    tick: u32,
    drop_pos: u32,
    drop_len: u32,
    drop_depth: f32,
}

const TAPE_CENTRE_S: f32 = 0.0065;
const TAPE_WOW_S: f32 = 0.0055;
const TAPE_FLUTTER_S: f32 = 0.000_072;
const TAPE_RANDOM_S: f32 = 0.0008;

impl Tape {
    pub fn new(sample_rate: f32, seed: u32) -> Self {
        let sr = sample_rate.max(1.0);
        let mut noise = Noise::new(seed);
        let wow_ph = 0.5 * (noise.next() + 1.0);
        let fl_ph = 0.5 * (noise.next() + 1.0);
        Self {
            sr,
            line: DelayLine::new((0.02 * sr) as usize + 8),
            noise,
            amt_t: 0.0,
            amt: 0.0,
            a_amt: OnePole::coef_tau(0.03, sr),
            wow_ph,
            wow_inc: 0.5 / sr,
            fl_ph,
            fl_inc: 7.0 / sr,
            rnd: 0.0,
            rnd2: 0.0,
            rnd_a: OnePole::coef_hz(12.0, sr),
            tick: 0,
            drop_pos: 0,
            drop_len: 0,
            drop_depth: 0.0,
        }
    }
    /// Back to the state `new(sample_rate, seed)` leaves it in, without allocating.
    pub fn reseed(&mut self, seed: u32) {
        let mut noise = Noise::new(seed);
        self.wow_ph = 0.5 * (noise.next() + 1.0);
        self.fl_ph = 0.5 * (noise.next() + 1.0);
        self.noise = noise;
        self.line.clear();
        self.amt_t = 0.0;
        self.amt = 0.0;
        self.rnd = 0.0;
        self.rnd2 = 0.0;
        self.tick = 0;
        self.drop_pos = 0;
        self.drop_len = 0;
        self.drop_depth = 0.0;
    }
    pub fn set(&mut self, amt: f32) {
        self.amt_t = fin(amt, 0.0).clamp(0.0, 1.0);
    }
    pub fn process(&mut self, x: f32) -> f32 {
        let x = clean(x);
        if self.amt != self.amt_t {
            self.amt += self.a_amt * (self.amt_t - self.amt);
            if (self.amt_t - self.amt).abs() < 1e-4 {
                self.amt = self.amt_t;
            }
        }
        // Always feed the line so switching on has history.
        self.line.push(x);
        if self.amt == 0.0 {
            self.drop_pos = 0;
            return x;
        }
        let a = self.amt;
        self.wow_ph += self.wow_inc;
        if self.wow_ph >= 1.0 {
            self.wow_ph -= 1.0;
        }
        self.fl_ph += self.fl_inc;
        if self.fl_ph >= 1.0 {
            self.fl_ph -= 1.0;
        }
        // Two poles so the random part is smooth (bounded pitch rate).
        self.rnd += self.rnd_a * (self.noise.next() - self.rnd);
        self.rnd2 += self.rnd_a * (self.rnd - self.rnd2);
        let d_s = TAPE_CENTRE_S
            + a * (TAPE_WOW_S * (TAU * self.wow_ph).sin()
                + TAPE_FLUTTER_S * (TAU * self.fl_ph).sin()
                + TAPE_RANDOM_S * self.rnd2);
        let wet = self.line.read_cubic(d_s * self.sr);
        let c = (a * 8.0).min(1.0);
        let mut y = x + c * (wet - x);
        // Dropouts: once per 32 samples, chance scaled so about one a second at amt 1.
        self.tick += 1;
        if self.tick >= 32 {
            self.tick = 0;
            if self.drop_pos >= self.drop_len {
                let u = 0.5 * (self.noise.next() + 1.0);
                if u < a * 32.0 / self.sr {
                    let v = 0.5 * (self.noise.next() + 1.0);
                    self.drop_len = ((0.003 + 0.003 * v) * self.sr) as u32 + 2;
                    self.drop_pos = 0;
                    self.drop_depth = a * (0.3 + 0.5 * v);
                    // pos == len means idle, so start one sample in.
                } else {
                    self.drop_len = 0;
                    self.drop_pos = 0;
                }
            }
        }
        if self.drop_pos < self.drop_len {
            let ph = self.drop_pos as f32 / self.drop_len as f32;
            y *= 1.0 - self.drop_depth * 0.5 * (1.0 - (TAU * ph).cos());
            self.drop_pos += 1;
        }
        y
    }
}

/// Stereo delay up to 2 s. The feedback path has a one-pole low-pass (about
/// 6 kHz) and a two-tap crossfading pitch shifter (`shift` semitones, -12..12;
/// two read heads sweeping a ~50 ms window with Hann crossfades; bypassed
/// exactly at 0). Feedback crosses channels lightly (ping-pong feel is not
/// required). `time_s` changes glide (no zipper, no clicks): smooth the read
/// position. `mix` 0..1 is a dry/wet crossfade (linear or equal power); at
/// mix 0 the output is exactly the input.
///
/// Notes: the first repeat is the raw input at `time_s`; the shifter acts on
/// the signal fed back, so repeat 2 is shifted once, repeat 3 twice, and so
/// on. Each shifted pass also adds about 25 ms (half the window) to the loop,
/// so shifted repeats arrive a little late. Feedback is limited to 0.97.
pub struct StereoDelay {
    sr: f32,
    lines: [DelayLine; 2],
    pbuf: [DelayLine; 2],
    lp: [OnePole; 2],
    min_time: f32,
    max_time: f32,
    time_t: f32,
    time: f32,
    a_time: f32,
    fb_t: f32,
    fb: f32,
    mix_t: f32,
    mix: f32,
    a_fast: f32,
    shift: f32,
    ratio: f32,
    eng: f32,
    eng_inc: f32,
    win: f32,
    pp: f32,
    peak_cur: f32,
    peak_prev: f32,
    win_left: u32,
    first: bool,
}

const DELAY_CROSS: f32 = 0.15;
const TAIL_FLOOR: f32 = 1e-5;

impl StereoDelay {
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(1.0);
        let n = (2.0 * sr) as usize + 8;
        let win = (0.05 * sr).round();
        Self {
            sr,
            lines: [DelayLine::new(n), DelayLine::new(n)],
            pbuf: [DelayLine::new(win as usize + 8), DelayLine::new(win as usize + 8)],
            lp: [OnePole::new(OnePole::coef_hz(6000.0, sr)); 2],
            min_time: 3.0,
            max_time: 2.0 * sr,
            time_t: 0.25 * sr,
            time: 0.25 * sr,
            a_time: OnePole::coef_tau(0.08, sr),
            fb_t: 0.0,
            fb: 0.0,
            mix_t: 0.0,
            mix: 0.0,
            a_fast: OnePole::coef_tau(0.01, sr),
            shift: 0.0,
            ratio: 1.0,
            eng: 0.0,
            eng_inc: 1.0 / (0.005 * sr),
            win,
            pp: 0.0,
            peak_cur: 0.0,
            peak_prev: 0.0,
            win_left: 1,
            first: true,
        }
    }
    /// Back to the state `new` leaves it in, without allocating (play from a stop).
    pub fn restart(&mut self) {
        for i in 0..2 {
            self.lines[i].clear();
            self.pbuf[i].clear();
            self.lp[i] = OnePole::new(OnePole::coef_hz(6000.0, self.sr));
        }
        self.time_t = 0.25 * self.sr;
        self.time = 0.25 * self.sr;
        self.fb_t = 0.0;
        self.fb = 0.0;
        self.mix_t = 0.0;
        self.mix = 0.0;
        self.shift = 0.0;
        self.ratio = 1.0;
        self.eng = 0.0;
        self.pp = 0.0;
        self.peak_cur = 0.0;
        self.peak_prev = 0.0;
        self.win_left = 1;
        self.first = true;
    }
    pub fn set(&mut self, time_s: f32, feedback: f32, shift_semitones: f32, mix: f32) {
        self.time_t = (fin(time_s, 0.25) * self.sr).clamp(self.min_time, self.max_time);
        self.fb_t = fin(feedback, 0.0).clamp(0.0, 0.97);
        self.mix_t = fin(mix, 0.0).clamp(0.0, 1.0);
        let s = fin(shift_semitones, 0.0).clamp(-12.0, 12.0);
        if s != self.shift {
            self.shift = s;
            self.ratio = (s / 12.0).exp2();
        }
        if self.first {
            self.first = false;
            self.time = self.time_t;
            self.fb = self.fb_t;
            self.mix = self.mix_t;
        }
    }
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let l = clean(l);
        let r = clean(r);
        if self.time != self.time_t {
            self.time += self.a_time * (self.time_t - self.time);
            if (self.time_t - self.time).abs() < 0.01 {
                self.time = self.time_t;
            }
        }
        if self.fb != self.fb_t {
            self.fb += self.a_fast * (self.fb_t - self.fb);
            if (self.fb_t - self.fb).abs() < 1e-5 {
                self.fb = self.fb_t;
            }
        }
        if self.mix != self.mix_t {
            self.mix += self.a_fast * (self.mix_t - self.mix);
            if (self.mix_t - self.mix).abs() < 1e-5 {
                self.mix = self.mix_t;
            }
        }
        // Read before the push: delay d samples means tap(d - 1).
        let rd = self.time - 1.0;
        let wl = self.lines[0].read_cubic(rd);
        let wr = self.lines[1].read_cubic(rd);
        let fl = self.lp[0].process(wl);
        let fr = self.lp[1].process(wr);
        self.pbuf[0].push(fl);
        self.pbuf[1].push(fr);

        // Shifter engage ramp (0 = exact bypass).
        let target = if self.shift != 0.0 { 1.0 } else { 0.0 };
        if self.eng < target {
            self.eng = (self.eng + self.eng_inc).min(target);
        } else if self.eng > target {
            self.eng = (self.eng - self.eng_inc).max(target);
        }
        let (sl, sr) = if self.eng > 0.0 {
            let p1 = self.pp;
            let p2 = if p1 >= 0.5 { p1 - 0.5 } else { p1 + 0.5 };
            let w1 = 0.5 * (1.0 - (TAU * p1).cos());
            let w2 = 1.0 - w1;
            let (d1, d2) = (p1 * self.win, p2 * self.win);
            let ml = w1 * self.pbuf[0].read_linear(d1) + w2 * self.pbuf[0].read_linear(d2);
            let mr = w1 * self.pbuf[1].read_linear(d1) + w2 * self.pbuf[1].read_linear(d2);
            self.pp += (1.0 - self.ratio) / self.win;
            if self.pp >= 1.0 {
                self.pp -= 1.0;
            } else if self.pp < 0.0 {
                self.pp += 1.0;
            }
            (fl + self.eng * (ml - fl), fr + self.eng * (mr - fr))
        } else {
            (fl, fr)
        };
        let g = self.fb;
        let il = l + g * ((1.0 - DELAY_CROSS) * sl + DELAY_CROSS * sr) + DENORMAL;
        let ir = r + g * ((1.0 - DELAY_CROSS) * sr + DELAY_CROSS * sl) + DENORMAL;
        self.lines[0].push(il);
        self.lines[1].push(ir);

        // Tail tracking: peak of what is written, over windows as long as the loop.
        let a = il.abs().max(ir.abs());
        if a > self.peak_cur {
            self.peak_cur = a;
        }
        self.win_left -= 1;
        if self.win_left == 0 {
            self.peak_prev = self.peak_cur;
            self.peak_cur = 0.0;
            self.win_left = (self.time + self.win + 8.0) as u32;
        }
        let m = self.mix;
        (l + m * (wl - l), r + m * (wr - r))
    }
    /// False once mix is 0 and the loop has decayed below -100 dB, so the
    /// layer can skip calling `process`.
    pub fn active(&self) -> bool {
        self.mix_t > 0.0
            || self.mix > 0.0
            || self.peak_cur.max(self.peak_prev) > TAIL_FLOOR
    }
    pub fn reset(&mut self) {
        for i in 0..2 {
            self.lines[i].clear();
            self.pbuf[i].clear();
            self.lp[i].y = 0.0;
        }
        self.pp = 0.0;
        self.eng = 0.0;
        self.peak_cur = 0.0;
        self.peak_prev = 0.0;
        self.win_left = 1;
        self.time = self.time_t;
        self.fb = self.fb_t;
        self.mix = self.mix_t;
    }
}

const C8: f32 = 0.353_553_4; // 1/sqrt(8)
/// Relative line lengths (times 10 ms * 10^size), spread so the lines do not
/// share common factors once rounded to primes.
const REVERB_RATIOS: [f32; 8] = [0.80, 0.89, 0.97, 1.07, 1.17, 1.28, 1.40, 1.55];
/// Modulation rates in Hz, one per line.
const REVERB_MOD_HZ: [f32; 8] = [0.31, 0.43, 0.57, 0.71, 0.89, 1.07, 1.31, 1.57];
/// Signs for the line outputs: left and right are orthogonal combinations.
const OUT_L: [f32; 8] = [1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, -1.0];
const OUT_R: [f32; 8] = [1.0, -1.0, 1.0, -1.0, -1.0, 1.0, -1.0, 1.0];
const IN_SIGN: [f32; 8] = [1.0, 1.0, -1.0, 1.0, -1.0, -1.0, -1.0, 1.0];

#[inline]
fn hadamard8(z: &mut [f32; 8]) {
    let (a0, a1) = (z[0] + z[1], z[0] - z[1]);
    let (a2, a3) = (z[2] + z[3], z[2] - z[3]);
    let (a4, a5) = (z[4] + z[5], z[4] - z[5]);
    let (a6, a7) = (z[6] + z[7], z[6] - z[7]);
    let (b0, b2) = (a0 + a2, a0 - a2);
    let (b1, b3) = (a1 + a3, a1 - a3);
    let (b4, b6) = (a4 + a6, a4 - a6);
    let (b5, b7) = (a5 + a7, a5 - a7);
    *z = [b0 + b4, b1 + b5, b2 + b6, b3 + b7, b0 - b4, b1 - b5, b2 - b6, b3 - b7];
}

/// Eight-line feedback delay network with Hadamard mixing, a one-pole
/// damping low-pass per line, and slow modulation of line lengths (< 1 ms,
/// different rate per line) to avoid metallic ringing.
/// `size` 0..1 scales line lengths (roughly 10 ms to 100 ms, mutually prime
/// in samples) and RT60 (about 0.3 s to 12 s, exponential in size).
/// `damp` 0..1: higher means darker tails. `mix` 0..1 dry/wet (exact dry at
/// 0). `freeze` 0..1: at 1 the loop gain is unity, damping is off and the
/// input is muted, so the tail holds; crossfade smoothly in between.
///
/// Notes: line i has length 10 ms * 10^size * ratio_i (ratios 0.8..1.55),
/// rounded to the nearest prime in samples, so the lines span about 8 ms at
/// size 0 to 155 ms at size 1. RT60 is 0.3 s * 40^size, set per line from its
/// own length so every line decays at the same dB per second. The input level
/// is scaled so the wet signal has about the input's level for any size.
/// Size, damp and freeze changes glide (line lengths with a 150 ms time
/// constant, which bends pitch while moving). At freeze 1 modulation is off
/// so reads are on whole samples and the loop loses no energy.
pub struct Reverb {
    sr: f32,
    lines: [DelayLine; 8],
    primes: Vec<u32>,
    lp: [f32; 8],
    len: [f32; 8],
    len_t: [f32; 8],
    g: [f32; 8],
    g_t: [f32; 8],
    a: f32,
    a_t: f32,
    kin: f32,
    kin_t: f32,
    modamp: f32,
    mod_t: f32,
    depth_cap: f32,
    rc: [f32; 8],
    rs: [f32; 8],
    rcw: [f32; 8],
    rsw: [f32; 8],
    inl: [f32; 8],
    inr: [f32; 8],
    p_size: f32,
    p_damp: f32,
    p_freeze: f32,
    glide: u32,
    glide_len: u32,
    a_len: f32,
    a_fast: f32,
    mix_t: f32,
    mix: f32,
    a_mix: f32,
    peak: f32,
    first: bool,
}

impl Reverb {
    pub fn new(sample_rate: f32, seed: u32) -> Self {
        let sr = sample_rate.max(1.0);
        let max_len = (0.01 * 10.0 * 1.55 * sr) as usize + 64;
        let n = max_len + (0.001 * sr) as usize + 8;
        // Sieve of primes up to max_len.
        let mut is = vec![true; max_len + 1];
        let mut primes = Vec::new();
        for i in 2..=max_len {
            if is[i] {
                primes.push(i as u32);
                let mut j = i * i;
                while j <= max_len {
                    is[j] = false;
                    j += i;
                }
            }
        }
        let mut s = Self {
            sr,
            lines: std::array::from_fn(|_| DelayLine::new(n)),
            primes,
            lp: [0.0; 8],
            len: [0.0; 8],
            len_t: [0.0; 8],
            g: [0.0; 8],
            g_t: [0.0; 8],
            a: 1.0,
            a_t: 1.0,
            kin: 0.0,
            kin_t: 0.0,
            modamp: 1.0,
            mod_t: 1.0,
            depth_cap: 0.0008 * sr,
            rc: [0.0; 8],
            rs: [0.0; 8],
            rcw: [0.0; 8],
            rsw: [0.0; 8],
            inl: [0.0; 8],
            inr: [0.0; 8],
            p_size: f32::NAN,
            p_damp: f32::NAN,
            p_freeze: f32::NAN,
            glide: 0,
            glide_len: (1.2 * sr) as u32,
            a_len: OnePole::coef_tau(0.15, sr),
            a_fast: OnePole::coef_tau(0.03, sr),
            mix_t: 0.0,
            mix: 0.0,
            a_mix: OnePole::coef_tau(0.01, sr),
            peak: 0.0,
            first: true,
        };
        s.reseed(seed);
        s
    }

    /// The state `new(sample_rate, seed)` leaves it in, without allocating: the modulation
    /// phases from the seed, empty lines, the starting size (play from a stop).
    pub fn reseed(&mut self, seed: u32) {
        let sr = self.sr;
        let mut noise = Noise::new(seed);
        for i in 0..8 {
            let ph = std::f32::consts::PI * (noise.next() + 1.0);
            self.rc[i] = ph.cos();
            self.rs[i] = ph.sin();
            let w = TAU * REVERB_MOD_HZ[i] / sr;
            self.rcw[i] = w.cos();
            self.rsw[i] = w.sin();
            let th = std::f32::consts::FRAC_PI_2 * (i as f32 + 0.5) / 8.0;
            self.inl[i] = IN_SIGN[i] * th.cos();
            self.inr[i] = IN_SIGN[i] * th.sin();
            self.lines[i].clear();
        }
        self.lp = [0.0; 8];
        self.len = [0.0; 8];
        self.len_t = [0.0; 8];
        self.g = [0.0; 8];
        self.g_t = [0.0; 8];
        self.a = 1.0;
        self.a_t = 1.0;
        self.kin = 0.0;
        self.kin_t = 0.0;
        self.modamp = 1.0;
        self.mod_t = 1.0;
        self.p_size = f32::NAN;
        self.p_damp = f32::NAN;
        self.p_freeze = f32::NAN;
        self.glide = 0;
        self.mix_t = 0.0;
        self.mix = 0.0;
        self.peak = 0.0;
        self.first = true;
        self.retarget(0.5, 0.3, 0.0, true);
    }

    fn nearest_prime(&self, x: f32) -> f32 {
        let v = x.round().max(2.0) as u32;
        match self.primes.binary_search(&v) {
            Ok(_) => v as f32,
            Err(i) => {
                let hi = self.primes.get(i).copied();
                let lo = if i > 0 { Some(self.primes[i - 1]) } else { None };
                match (lo, hi) {
                    (Some(a), Some(b)) => (if v - a <= b - v { a } else { b }) as f32,
                    (Some(a), None) => a as f32,
                    (None, Some(b)) => b as f32,
                    _ => v as f32,
                }
            }
        }
    }

    fn retarget(&mut self, size: f32, damp: f32, freeze: f32, snap: bool) {
        self.p_size = size;
        self.p_damp = damp;
        self.p_freeze = freeze;
        let rt60 = 0.3 * 40f32.powf(size);
        let mut gsum = 0.0;
        let mut gt = [0.0f32; 8];
        for i in 0..8 {
            let l = self.nearest_prime(0.01 * 10f32.powf(size) * REVERB_RATIOS[i] * self.sr);
            self.len_t[i] = l;
            let g = (-6.907_755 * l / (self.sr * rt60)).exp();
            gt[i] = g;
            gsum += g;
        }
        let gm = gsum / 8.0;
        let fc = 40_000.0 * (1200.0f32 / 40_000.0).powf(damp);
        let a = OnePole::coef_hz(fc, self.sr);
        for i in 0..8 {
            self.g_t[i] = if freeze >= 1.0 { 1.0 } else { gt[i] + freeze * (1.0 - gt[i]) };
        }
        self.a_t = if freeze >= 1.0 { 1.0 } else { a + freeze * (1.0 - a) };
        self.kin_t = (1.0 - gm * gm).max(0.003).sqrt() * (1.0 - freeze);
        self.mod_t = 1.0 - freeze;
        if snap {
            self.snap();
        } else {
            self.glide = self.glide_len;
        }
    }

    fn snap(&mut self) {
        self.len = self.len_t;
        self.g = self.g_t;
        self.a = self.a_t;
        self.kin = self.kin_t;
        self.modamp = self.mod_t;
        self.glide = 0;
    }

    pub fn set(&mut self, size: f32, damp: f32, mix: f32, freeze: f32) {
        let size = fin(size, 0.5).clamp(0.0, 1.0);
        let damp = fin(damp, 0.0).clamp(0.0, 1.0);
        let freeze = fin(freeze, 0.0).clamp(0.0, 1.0);
        self.mix_t = fin(mix, 0.0).clamp(0.0, 1.0);
        if size != self.p_size || damp != self.p_damp || freeze != self.p_freeze || self.first {
            let snap = self.first;
            self.retarget(size, damp, freeze, snap);
        }
        if self.first {
            self.mix = self.mix_t;
            self.first = false;
        }
        // Keep the rotators on the unit circle.
        for i in 0..8 {
            let r = 1.5 - 0.5 * (self.rc[i] * self.rc[i] + self.rs[i] * self.rs[i]);
            self.rc[i] *= r;
            self.rs[i] *= r;
        }
    }

    fn glide_step(&mut self) {
        for i in 0..8 {
            self.len[i] += self.a_len * (self.len_t[i] - self.len[i]);
            self.g[i] += self.a_fast * (self.g_t[i] - self.g[i]);
        }
        self.a += self.a_fast * (self.a_t - self.a);
        self.kin += self.a_fast * (self.kin_t - self.kin);
        self.modamp += self.a_fast * (self.mod_t - self.modamp);
        self.glide -= 1;
        if self.glide == 0 {
            self.snap();
        }
    }

    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let l = clean(l);
        let r = clean(r);
        if self.glide > 0 {
            self.glide_step();
        }
        let mut y = [0.0f32; 8];
        for i in 0..8 {
            let (c, s) = (self.rc[i], self.rs[i]);
            self.rc[i] = c * self.rcw[i] - s * self.rsw[i];
            self.rs[i] = s * self.rcw[i] + c * self.rsw[i];
            let len = self.len[i];
            let depth = (0.03 * len).min(self.depth_cap) * self.modamp;
            y[i] = self.lines[i].read_linear(len + depth * s - 1.0);
        }
        let mut wl = 0.0;
        let mut wr = 0.0;
        for i in 0..8 {
            wl += OUT_L[i] * y[i];
            wr += OUT_R[i] * y[i];
        }
        wl *= C8;
        wr *= C8;

        let mut z = [0.0f32; 8];
        let a = self.a;
        for i in 0..8 {
            let v = if a >= 1.0 {
                self.lp[i] = y[i];
                y[i]
            } else {
                self.lp[i] += a * (y[i] - self.lp[i]);
                self.lp[i]
            };
            z[i] = v * self.g[i];
        }
        hadamard8(&mut z);
        for i in 0..8 {
            let inp = (l * self.inl[i] + r * self.inr[i]) * self.kin;
            self.lines[i].push(z[i] * C8 + inp + DENORMAL);
        }

        let pk = wl.abs().max(wr.abs()).max(l.abs()).max(r.abs());
        self.peak = pk.max(self.peak * 0.9999);

        if self.mix != self.mix_t {
            self.mix += self.a_mix * (self.mix_t - self.mix);
            if (self.mix_t - self.mix).abs() < 1e-5 {
                self.mix = self.mix_t;
            }
        }
        let m = self.mix;
        (l + m * (wl - l), r + m * (wr - r))
    }
    /// False once mix is 0 and the tail has decayed below -100 dB.
    pub fn active(&self) -> bool {
        self.mix_t > 0.0 || self.mix > 0.0 || self.peak > TAIL_FLOOR
    }
    pub fn reset(&mut self) {
        for i in 0..8 {
            self.lines[i].clear();
            self.lp[i] = 0.0;
        }
        self.peak = 0.0;
        self.mix = self.mix_t;
    }
}

/// Mid-side width: 0 mono, 1 unchanged, 2 doubles the side signal.
#[inline]
pub fn width(l: f32, r: f32, w: f32) -> (f32, f32) {
    let m = 0.5 * (l + r);
    let s = 0.5 * (l - r) * w;
    (m + s, m - s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    /// Positive-going zero-crossing frequency of `x[a..b]` in Hz.
    fn zc_freq(x: &[f32], a: usize, b: usize) -> f32 {
        let mut first = None;
        let mut last = 0.0f32;
        let mut n = 0;
        for i in a + 1..b {
            if x[i - 1] < 0.0 && x[i] >= 0.0 {
                let t = i as f32 - 1.0 + (-x[i - 1]) / (x[i] - x[i - 1]);
                if first.is_none() {
                    first = Some(t);
                } else {
                    n += 1;
                }
                last = t;
            }
        }
        n as f32 * SR / (last - first.unwrap())
    }

    fn sine(hz: f32, i: usize) -> f32 {
        (TAU * hz * i as f32 / SR).sin()
    }

    #[test]
    fn crush_bypass() {
        let mut c = Crush::new(SR, 1);
        c.set(16.0, SR);
        for i in 0..1000 {
            let x = 0.9 * sine(440.0, i);
            assert_eq!(c.process(x), x);
        }
    }

    #[test]
    fn crush_four_bits_has_few_levels() {
        let mut c = Crush::new(SR, 1);
        c.set(4.0, SR);
        let mut lv: Vec<i32> = Vec::new();
        for i in 0..48_000 {
            let y = c.process(0.95 * sine(97.0, i));
            let k = (y * 1e5).round() as i32;
            if !lv.contains(&k) {
                lv.push(k);
            }
        }
        assert!(lv.len() >= 8 && lv.len() <= 19, "{}", lv.len());
    }

    #[test]
    fn crush_holds_at_1khz() {
        let mut c = Crush::new(SR, 1);
        c.set(16.0, 1000.0);
        let mut changes = 0;
        let mut prev = f32::NAN;
        for i in 0..4800 {
            let y = c.process(0.8 * sine(333.0, i));
            if y != prev {
                changes += 1;
            }
            prev = y;
        }
        assert!((98..=102).contains(&changes), "{changes}");
    }

    #[test]
    fn crush_nan_is_finite() {
        let mut c = Crush::new(SR, 1);
        c.set(f32::NAN, 3000.0);
        c.set(4.0, 3000.0);
        assert!(c.process(f32::NAN).is_finite());
        assert!(c.process(f32::INFINITY).is_finite());
    }

    #[test]
    fn tape_bypass_at_zero() {
        let mut t = Tape::new(SR, 3);
        t.set(0.0);
        for i in 0..2000 {
            let x = sine(300.0, i);
            assert_eq!(t.process(x), x);
        }
    }

    #[test]
    fn tape_pitch_deviation_is_bounded() {
        let mut t = Tape::new(SR, 3);
        t.set(1.0);
        let n = (20.0 * SR) as usize;
        let y: Vec<f32> = (0..n).map(|i| t.process(sine(1000.0, i))).collect();
        // Per-cycle period -> cents.
        let mut crossings = Vec::new();
        for i in (SR as usize)..n {
            if y[i - 1] < 0.0 && y[i] >= 0.0 {
                crossings.push(i as f32 - 1.0 + (-y[i - 1]) / (y[i] - y[i - 1]));
            }
        }
        let mut worst = 0.0f32;
        let mut ratios = Vec::new();
        for w in crossings.windows(2) {
            let c = 1200.0 * (SR / 1000.0 / (w[1] - w[0])).log2();
            ratios.push(c);
            worst = worst.max(c.abs());
        }
        let spread = ratios.iter().cloned().fold(f32::MIN, f32::max)
            - ratios.iter().cloned().fold(f32::MAX, f32::min);
        let rms = (ratios.iter().map(|c| c * c).sum::<f32>() / ratios.len() as f32).sqrt();
        assert!(worst < 45.0, "worst {worst} rms {rms}");
        assert!(spread > 30.0, "not moving enough: {spread}");
    }

    #[test]
    fn tape_dropouts_happen_and_output_is_finite() {
        let mut t = Tape::new(SR, 9);
        t.set(1.0);
        let mut min_env = 1.0f32;
        let mut env = 0.0f32;
        for i in 0..(10.0 * SR) as usize {
            let y = t.process(if i == 5 { f32::NAN } else { sine(200.0, i) });
            assert!(y.is_finite());
            env = env.max(y.abs()).min(1.0) * 0.999;
            if i > 4800 {
                min_env = min_env.min(env);
            }
        }
        assert!(min_env < 0.85, "no dropout seen {min_env}");
    }

    fn run_delay(
        d: &mut StereoDelay,
        n: usize,
        f: impl Fn(usize) -> f32,
    ) -> Vec<f32> {
        (0..n).map(|i| d.process(f(i), f(i)).0).collect()
    }

    #[test]
    fn delay_impulse_returns_at_time() {
        for &t in &[0.1f32, 0.1013, 0.5] {
            let mut d = StereoDelay::new(SR);
            d.set(t, 0.0, 0.0, 1.0);
            let n = (t * SR) as usize + 200;
            let y = run_delay(&mut d, n, |i| if i == 0 { 1.0 } else { 0.0 });
            let (pk, _) = y
                .iter()
                .enumerate()
                .skip(10)
                .fold((0, 0.0f32), |a, (i, &v)| if v.abs() > a.1 { (i, v.abs()) } else { a });
            assert!((pk as f32 - t * SR).abs() <= 1.0, "t {t} peak at {pk}");
        }
    }

    #[test]
    fn delay_repeats_decay_by_feedback() {
        let mut d = StereoDelay::new(SR);
        let t = 0.05;
        d.set(t, 0.5, 0.0, 1.0);
        let dn = (t * SR) as usize;
        let y = run_delay(&mut d, dn * 4 + 400, |i| if i == 0 { 1.0 } else { 0.0 });
        let sum = |k: usize| -> f32 { y[k * dn - 4..k * dn + 80].iter().sum() };
        let (s1, s2, s3) = (sum(1), sum(2), sum(3));
        assert!((s1 - 1.0).abs() < 0.01, "{s1}");
        assert!((s2 / s1 - 0.5).abs() < 0.01, "{}", s2 / s1);
        assert!((s3 / s2 - 0.5).abs() < 0.01, "{}", s3 / s2);
    }

    #[test]
    fn delay_mix_zero_is_exact_dry() {
        let mut d = StereoDelay::new(SR);
        d.set(0.1, 0.6, 5.0, 0.0);
        for i in 0..20_000 {
            let x = sine(300.0, i);
            assert_eq!(d.process(x, -x), (x, -x));
        }
    }

    #[test]
    fn delay_shift_12_doubles_second_repeat() {
        let mut d = StereoDelay::new(SR);
        d.set(0.25, 0.8, 12.0, 1.0);
        let n = (0.9 * SR) as usize;
        let y = run_delay(&mut d, n, |i| if i < 7200 { sine(480.0, i) } else { 0.0 });
        let f1 = zc_freq(&y, (0.28 * SR) as usize, (0.38 * SR) as usize);
        let f2 = zc_freq(&y, (0.56 * SR) as usize, (0.62 * SR) as usize);
        assert!((f1 - 480.0).abs() < 8.0, "first {f1}");
        assert!((f2 - 960.0).abs() < 16.0, "second {f2}");
    }

    #[test]
    fn delay_shift_zero_keeps_pitch_and_timing() {
        let mut d = StereoDelay::new(SR);
        d.set(0.25, 0.8, 0.0, 1.0);
        let n = (0.9 * SR) as usize;
        let y = run_delay(&mut d, n, |i| if i < 7200 { sine(480.0, i) } else { 0.0 });
        let f2 = zc_freq(&y, (0.52 * SR) as usize, (0.62 * SR) as usize);
        assert!((f2 - 480.0).abs() < 8.0, "{f2}");
    }

    #[test]
    fn delay_time_change_glides_without_clicks() {
        let mut d = StereoDelay::new(SR);
        d.set(0.1, 0.0, 0.0, 1.0);
        let mut prev = 0.0;
        let mut worst = 0.0f32;
        for i in 0..48_000 {
            if i == 20_000 {
                d.set(0.15, 0.0, 0.0, 1.0);
            }
            let y = d.process(sine(200.0, i), 0.0).0;
            if i > 10_000 {
                worst = worst.max((y - prev).abs());
            }
            prev = y;
        }
        assert!(worst < 0.1, "{worst}");
    }

    #[test]
    fn delay_active_and_nan() {
        let mut d = StereoDelay::new(SR);
        d.set(0.05, 0.5, 0.0, 0.0);
        assert!(!d.active());
        d.process(1.0, 1.0);
        assert!(d.active());
        let mut n = 0;
        while d.active() && n < 20 * 48_000 {
            d.process(0.0, 0.0);
            n += 1;
        }
        assert!(!d.active(), "never went inactive");
        // 0.5^k < 1e-5 needs 17 passes of 50 ms, plus up to one window of slack.
        assert!(n > 30_000 && n < 90_000, "{n}");
        d.set(0.05, 0.9, 7.0, 1.0);
        assert!(d.active());
        for i in 0..5000 {
            let x = match i % 3 {
                0 => f32::NAN,
                1 => f32::INFINITY,
                _ => 0.3,
            };
            let (a, b) = d.process(x, x);
            assert!(a.is_finite() && b.is_finite());
        }
        for _ in 0..100_000 {
            let (a, b) = d.process(0.0, 0.0);
            assert!(a.is_finite() && b.is_finite());
        }
    }

    fn rev_ir(size: f32, damp: f32, secs: f32) -> Vec<(f32, f32)> {
        let mut r = Reverb::new(SR, 5);
        r.set(size, damp, 1.0, 0.0);
        (0..(secs * SR) as usize)
            .map(|i| {
                let x = if i == 0 { 1.0 } else { 0.0 };
                r.process(x, 0.0)
            })
            .collect()
    }

    /// RT60 from the Schroeder decay curve: slope between -5 and -35 dB, doubled.
    fn rt60(ir: &[(f32, f32)]) -> f32 {
        let mut edc = vec![0.0f64; ir.len()];
        let mut acc = 0.0f64;
        for i in (0..ir.len()).rev() {
            acc += (ir[i].0 as f64).powi(2) + (ir[i].1 as f64).powi(2);
            edc[i] = acc;
        }
        let db = |v: f64| 10.0 * (v / edc[0]).log10();
        let t5 = edc.iter().position(|&v| db(v) < -5.0).unwrap();
        let t35 = edc.iter().position(|&v| db(v) < -35.0).unwrap();
        2.0 * (t35 - t5) as f32 / SR
    }

    #[test]
    fn reverb_rt60_matches_size() {
        for &(size, want) in &[(0.0f32, 0.3f32), (0.5, 1.9), (1.0, 12.0)] {
            let want_exact = 0.3 * 40f32.powf(size);
            assert!((want_exact - want).abs() / want < 0.05);
            let ir = rev_ir(size, 0.0, want * 1.4 + 1.0);
            let got = rt60(&ir);
            assert!((got - want).abs() / want < 0.3, "size {size}: {got} vs {want}");
        }
    }

    #[test]
    fn reverb_damping_darkens_tail() {
        let hf = |ir: &[(f32, f32)]| -> f64 {
            let (a, b) = ((0.3 * SR) as usize, (1.0 * SR) as usize);
            let (mut d, mut t) = (0.0f64, 0.0f64);
            for i in a..b {
                let x = ir[i].0 as f64;
                let df = x - ir[i - 1].0 as f64;
                d += df * df;
                t += x * x;
            }
            d / t
        };
        let bright = hf(&rev_ir(0.6, 0.0, 1.1));
        let dark = hf(&rev_ir(0.6, 1.0, 1.1));
        assert!(dark < 0.5 * bright, "{dark} vs {bright}");
    }

    #[test]
    fn reverb_freeze_holds_level() {
        let mut r = Reverb::new(SR, 5);
        let mut nz = Noise::new(77);
        r.set(0.5, 0.5, 1.0, 0.0);
        for _ in 0..(0.4 * SR) as usize {
            r.process(0.5 * nz.next(), 0.5 * nz.next());
        }
        r.set(0.5, 0.5, 1.0, 1.0);
        for _ in 0..(1.5 * SR) as usize {
            r.process(0.5 * nz.next(), 0.5 * nz.next());
        }
        let block = (0.25 * SR) as usize;
        let mut levels = Vec::new();
        for _ in 0..20 {
            let mut e = 0.0f64;
            for _ in 0..block {
                let (a, b) = r.process(0.0, 0.0);
                e += (a as f64).powi(2) + (b as f64).powi(2);
            }
            levels.push(10.0 * (e / block as f64).log10());
        }
        let first = levels[0];
        assert!(first > -60.0, "tail too quiet: {first}");
        for (k, &lv) in levels.iter().enumerate() {
            assert!((lv - first).abs() < 1.0, "block {k}: {lv} vs {first}");
        }
    }

    #[test]
    fn reverb_mix_zero_is_exact_dry() {
        let mut r = Reverb::new(SR, 5);
        r.set(0.7, 0.2, 0.0, 0.0);
        for i in 0..20_000 {
            let x = sine(300.0, i);
            assert_eq!(r.process(x, 0.5 * x), (x, 0.5 * x));
        }
    }

    #[test]
    fn reverb_goes_inactive_after_tail() {
        let mut r = Reverb::new(SR, 5);
        r.set(0.0, 0.0, 0.0, 0.0);
        assert!(!r.active());
        r.process(1.0, 1.0);
        assert!(r.active());
        let mut n = 1;
        while r.active() && n < 10 * 48_000 {
            r.process(0.0, 0.0);
            n += 1;
        }
        assert!(!r.active());
        // 100 dB at RT60 0.3 s is 0.5 s.
        assert!(n > 20_000 && n < 3 * 48_000, "{n}");
        r.set(0.0, 0.0, 0.5, 0.0);
        assert!(r.active());
    }

    #[test]
    fn reverb_survives_nan_and_inf() {
        let mut r = Reverb::new(SR, 5);
        r.set(0.8, 0.3, 0.7, 0.0);
        for i in 0..20_000 {
            let x = match i % 4 {
                0 => f32::NAN,
                1 => f32::INFINITY,
                2 => f32::NEG_INFINITY,
                _ => 1e30,
            };
            let (a, b) = r.process(x, x);
            assert!(a.is_finite() && b.is_finite());
        }
        r.set(f32::NAN, f32::NAN, f32::NAN, f32::NAN);
        let mut e = 0.0;
        for i in 0..96_000 {
            let (a, b) = r.process(if i == 0 { 1.0 } else { 0.0 }, 0.0);
            assert!(a.is_finite() && b.is_finite());
            e += a * a + b * b;
        }
        assert!(e > 0.0);
    }

    #[test]
    fn reverb_is_stereo_and_decorrelated() {
        let ir = rev_ir(0.5, 0.2, 1.0);
        let (mut ll, mut rr, mut lr) = (0.0f64, 0.0f64, 0.0f64);
        for &(a, b) in &ir[(0.1 * SR) as usize..] {
            ll += (a * a) as f64;
            rr += (b * b) as f64;
            lr += (a * b) as f64;
        }
        assert!((ll / rr).log10().abs() < 0.3, "{}", ll / rr);
        assert!((lr / (ll * rr).sqrt()).abs() < 0.3, "{}", lr / (ll * rr).sqrt());
    }

    #[test]
    fn reverb_size_change_is_smooth_and_settles() {
        let mut r = Reverb::new(SR, 5);
        r.set(0.3, 0.2, 1.0, 0.0);
        let mut worst = 0.0f32;
        let mut prev = 0.0;
        for i in 0..96_000 {
            if i == 20_000 {
                r.set(0.7, 0.2, 1.0, 0.0);
            }
            let (a, _) = r.process(0.3 * sine(220.0, i), 0.0);
            worst = worst.max((a - prev).abs());
            prev = a;
        }
        assert!(worst < 0.6, "{worst}");
    }

    #[test]
    fn determinism() {
        let run = || {
            let mut r = Reverb::new(SR, 42);
            let mut t = Tape::new(SR, 42);
            let mut c = Crush::new(SR, 42);
            let mut d = StereoDelay::new(SR);
            r.set(0.6, 0.4, 0.5, 0.0);
            t.set(1.0);
            c.set(6.0, 8000.0);
            d.set(0.12, 0.5, 3.0, 0.5);
            let mut nz = Noise::new(3);
            let mut out = Vec::new();
            for i in 0..30_000 {
                if i % 32 == 0 {
                    r.set(0.6, 0.4, 0.5, 0.0);
                }
                let x = nz.next() * 0.3;
                let (a, b) = r.process(x, -x);
                let (c1, c2) = d.process(a, b);
                out.push(t.process(c.process(c1)) + c2);
            }
            out
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn width_unchanged_at_one() {
        let (a, b) = width(0.3, -0.2, 1.0);
        assert!((a - 0.3).abs() < 1e-6 && (b + 0.2).abs() < 1e-6);
    }

    #[test]
    #[ignore]
    fn bench() {
        let n = 2_000_000;
        let mut nz = Noise::new(1);
        let xs: Vec<f32> = (0..4096).map(|_| nz.next() * 0.3).collect();
        let mut sink = 0.0f32;

        let mut d = StereoDelay::new(SR);
        d.set(0.3, 0.5, 0.0, 0.5);
        let t0 = std::time::Instant::now();
        for i in 0..n {
            let x = xs[i & 4095];
            let (a, b) = d.process(x, x);
            sink += a + b;
        }
        println!("delay (no shift): {:.1} ns / stereo sample", t0.elapsed().as_nanos() as f64 / n as f64);

        d.set(0.3, 0.5, 7.0, 0.5);
        let t0 = std::time::Instant::now();
        for i in 0..n {
            let x = xs[i & 4095];
            let (a, b) = d.process(x, x);
            sink += a + b;
        }
        println!("delay (shift 7):  {:.1} ns / stereo sample", t0.elapsed().as_nanos() as f64 / n as f64);

        let mut r = Reverb::new(SR, 1);
        r.set(0.6, 0.4, 0.5, 0.0);
        let t0 = std::time::Instant::now();
        for i in 0..n {
            let x = xs[i & 4095];
            let (a, b) = r.process(x, x);
            sink += a + b;
        }
        println!("reverb:           {:.1} ns / stereo sample", t0.elapsed().as_nanos() as f64 / n as f64);

        let mut t = Tape::new(SR, 1);
        t.set(1.0);
        let t0 = std::time::Instant::now();
        for i in 0..n {
            sink += t.process(xs[i & 4095]);
        }
        println!("tape:             {:.1} ns / sample", t0.elapsed().as_nanos() as f64 / n as f64);
        assert!(sink.is_finite());
    }
}
