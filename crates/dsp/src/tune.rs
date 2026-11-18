//! The tuning function (TDD "Tuning"). Causal, frame by frame: the same code
//! runs during Live follow (per YIN hop) and over stored takes (per frame).
//!
//! Steps: split slow pitch (3 Hz one-pole) from vibrato; pick the nearest scale
//! degree with 60% hysteresis; pull by snap then lag by speed; slide handling
//! (glide / gliss / jump); add vibrato back scaled; hold through gaps.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slides {
    Glide,
    Gliss,
    Jump,
}

impl Slides {
    pub fn from_index(i: f32) -> Self {
        match i.round() as i32 {
            1 => Slides::Gliss,
            2 => Slides::Jump,
            _ => Slides::Glide,
        }
    }
}

/// Scales as 12-bit pitch-class masks relative to the root (bit 0 = root).
/// Index order is the `tune.scale` parameter.
pub const SCALES: [(&str, u16); 6] = [
    ("chromatic", 0b1111_1111_1111),
    ("major", 0b1010_1011_0101),
    ("minor", 0b0101_1010_1101),
    ("major pentatonic", 0b0010_1001_0101),
    ("minor pentatonic", 0b0100_1010_1001),
    ("blues", 0b0100_1110_1001),
];

#[derive(Clone, Copy, Debug)]
pub struct TuneSettings {
    /// Pitch Snap, 0..1.
    pub snap: f32,
    /// Tune speed: lag time constant of the snap correction in ms, 0..250. 0 = instant.
    pub speed_ms: f32,
    pub slides: Slides,
    /// 0 flatten, 1 keep, 2 exaggerate.
    pub vibrato: f32,
    /// Pitch class of the root, 0 = C.
    pub root: u8,
    /// Pitch-class mask relative to root (see SCALES).
    pub mask: u16,
    /// Singer's offset from concert pitch in cents; scale degrees shift by this.
    pub ref_cents: f32,
}

impl Default for TuneSettings {
    fn default() -> Self {
        Self { snap: 0.0, speed_ms: 60.0, slides: Slides::Glide, vibrato: 1.0, root: 0, mask: SCALES[0].1, ref_cents: 0.0 }
    }
}

/// Frames with confidence below this, or NaN pitch, are gaps.
pub const MIN_CONFIDENCE: f32 = 0.5;
/// Slow-pitch split cutoff.
pub const SPLIT_HZ: f32 = 3.0;
/// Hysteresis: target moves once pitch is this share of the way to the next degree.
pub const HYSTERESIS: f32 = 0.6;
/// A slide: slow-pitch rate above this (cents/s) ...
pub const SLIDE_RATE: f32 = 400.0;
/// ... for longer than this (s).
pub const SLIDE_MIN_S: f32 = 0.040;
/// Jump: switch once within this many cents of a new degree ...
pub const JUMP_WINDOW_CENTS: f32 = 50.0;
/// ... for this long (s).
pub const JUMP_HOLD_S: f32 = 0.060;

/// Pitch is clamped to this range so degree arithmetic cannot overflow.
const PITCH_LIMIT: f32 = 100_000.0;

pub struct Tuner {
    dt: f32,
    settings: TuneSettings,
    /// One-pole coefficient for the 3 Hz split at this `dt`.
    split_a: f32,
    /// False after new/reset/gap until the next voiced frame re-seeds the state.
    seeded: bool,
    /// False until the first voiced frame since new/reset (output is NaN before).
    ever_voiced: bool,
    last_out: f32,
    slow: f32,
    prev_slow: f32,
    /// Lagged snap correction (pulled minus slow pitch). Tune speed lags the
    /// correction, not the sung pitch, so at Snap 0 nothing is delayed.
    corr: f32,
    /// Current target degree as an integer step k: cents = 100*k + ref_cents.
    target: i32,
    /// Note Jump mode is holding (same index scheme).
    held: i32,
    /// Time the slow pitch has been moving faster than SLIDE_RATE.
    slide_t: f32,
    /// Time the slow pitch has sat near a not-yet-adopted target (Jump).
    jump_t: f32,
}

impl Tuner {
    /// `dt`: seconds between calls (0.005 live per YIN hop, 0.01 for stored frames).
    pub fn new(dt: f32) -> Self {
        Self {
            dt,
            settings: TuneSettings::default(),
            split_a: 1.0 - (-2.0 * std::f32::consts::PI * SPLIT_HZ * dt).exp(),
            seeded: false,
            ever_voiced: false,
            last_out: f32::NAN,
            slow: 0.0,
            prev_slow: 0.0,
            corr: 0.0,
            target: 0,
            held: 0,
            slide_t: 0.0,
            jump_t: 0.0,
        }
    }
    pub fn set(&mut self, s: TuneSettings) {
        self.settings = s;
    }
    pub fn settings(&self) -> &TuneSettings {
        &self.settings
    }

    /// Scale mask limited to 12 bits; an empty mask is treated as chromatic
    /// so there is always a degree to snap to.
    fn mask(&self) -> u16 {
        let m = self.settings.mask & 0x0FFF;
        if m == 0 { 0x0FFF } else { m }
    }
    fn allowed(&self, k: i32) -> bool {
        let pc = (k - (self.settings.root % 12) as i32).rem_euclid(12);
        (self.mask() >> pc) & 1 == 1
    }
    fn degree_cents(&self, k: i32) -> f32 {
        100.0 * k as f32 + self.settings.ref_cents
    }
    /// Nearest allowed degree to `p`.
    fn nearest(&self, p: f32) -> i32 {
        let base = ((p - self.settings.ref_cents) / 100.0).round() as i32;
        let mut best = base;
        let mut best_d = f32::INFINITY;
        for k in base - 12..=base + 12 {
            if !self.allowed(k) {
                continue;
            }
            let d = (self.degree_cents(k) - p).abs();
            if d < best_d {
                best_d = d;
                best = k;
            }
        }
        best
    }
    /// Next allowed degree from `k` in direction `dir` (+1 or -1); `k` if none.
    fn next_allowed(&self, k: i32, dir: i32) -> i32 {
        for i in 1..=12 {
            if self.allowed(k + dir * i) {
                return k + dir * i;
            }
        }
        k
    }
    /// Hysteresis: the target only changes once the slow pitch is HYSTERESIS of
    /// the way from the current target to the next degree in its direction.
    fn update_target(&mut self) {
        if !self.allowed(self.target) {
            self.target = self.nearest(self.slow);
            return;
        }
        let tc = self.degree_cents(self.target);
        let dir = if self.slow > tc { 1 } else if self.slow < tc { -1 } else { return };
        let nk = self.next_allowed(self.target, dir);
        if nk == self.target {
            return;
        }
        let limit = tc + HYSTERESIS * (self.degree_cents(nk) - tc);
        let crossed = if dir > 0 { self.slow >= limit } else { self.slow <= limit };
        if crossed {
            self.target = self.nearest(self.slow);
        }
    }

    /// One step. `pitch_cents` is cents above C0 or NaN. Returns tuned cents,
    /// or NaN if nothing voiced has been seen since the last reset.
    pub fn process(&mut self, pitch_cents: f32, confidence: f32) -> f32 {
        // `!(c >= MIN)` also catches a NaN confidence.
        if !pitch_cents.is_finite() || !(confidence >= MIN_CONFIDENCE) {
            // Hold. The next voiced frame starts fresh so no glide comes from
            // a stale filter state.
            self.seeded = false;
            return self.last_out;
        }
        let pitch = pitch_cents.clamp(-PITCH_LIMIT, PITCH_LIMIT);
        let s = self.settings;

        let fresh = !self.seeded;
        if fresh {
            self.seeded = true;
            self.ever_voiced = true;
            self.slow = pitch;
            self.prev_slow = pitch;
            self.slide_t = 0.0;
            self.jump_t = 0.0;
            self.target = self.nearest(pitch);
            self.held = self.target;
        } else {
            self.slow += self.split_a * (pitch - self.slow);
        }
        let remainder = pitch - self.slow;

        // Slide detection on the slow pitch.
        let rate = (self.slow - self.prev_slow).abs() / self.dt;
        self.prev_slow = self.slow;
        if rate > SLIDE_RATE {
            self.slide_t += self.dt;
        } else {
            self.slide_t = 0.0;
        }
        let sliding = self.slide_t > SLIDE_MIN_S;

        self.update_target();

        // Jump: the note actually sung changes only after the pitch has sat
        // within JUMP_WINDOW_CENTS of the new degree for JUMP_HOLD_S. This
        // applies to every note change, slide or not, which is why Jump costs
        // 60 ms. Outside Jump mode `held` follows the target so switching modes
        // mid-stream is clean.
        if s.slides == Slides::Jump {
            if !self.allowed(self.held) {
                self.held = self.target;
            }
            if self.target != self.held
                && (self.slow - self.degree_cents(self.target)).abs() <= JUMP_WINDOW_CENTS
            {
                self.jump_t += self.dt;
                // Half a step of slack so float accumulation never costs a frame.
                if self.jump_t >= JUMP_HOLD_S - 0.5 * self.dt {
                    self.held = self.target;
                    self.jump_t = 0.0;
                }
            } else {
                self.jump_t = 0.0;
            }
        } else {
            self.held = self.target;
            self.jump_t = 0.0;
        }

        // Pull. Glide drops the pull during a slide; Gliss forces full snap
        // and no lag so the output steps through each degree the slide crosses.
        let mut snap = s.snap.clamp(0.0, 1.0);
        let mut speed_ms = s.speed_ms.max(0.0);
        let note = if s.slides == Slides::Jump { self.held } else { self.target };
        if sliding {
            match s.slides {
                Slides::Glide => snap = 0.0,
                Slides::Gliss => {
                    snap = 1.0;
                    speed_ms = 0.0;
                }
                Slides::Jump => {}
            }
        }
        let pulled = self.slow + snap * (self.degree_cents(note) - self.slow);

        // Lag the correction only. Lagging the whole pitch would also delay
        // the singer's own pitch changes (and a wrong first estimate at note
        // start) by Tune speed even at Snap 0.
        let target_corr = pulled - self.slow;
        if fresh || speed_ms <= 0.0 {
            self.corr = target_corr;
        } else {
            let a = 1.0 - (-self.dt / (speed_ms * 0.001)).exp();
            self.corr += a * (target_corr - self.corr);
        }

        let out = self.slow + self.corr + s.vibrato.clamp(0.0, 2.0) * remainder;
        self.last_out = out;
        out
    }

    pub fn reset(&mut self) {
        self.seeded = false;
        self.ever_voiced = false;
        self.last_out = f32::NAN;
        self.slow = 0.0;
        self.prev_slow = 0.0;
        self.corr = 0.0;
        self.target = 0;
        self.held = 0;
        self.slide_t = 0.0;
        self.jump_t = 0.0;
    }
}

/// Run the tuner over a stored pitch curve (one value per 10 ms frame).
/// Writes `out[i]` for each input. Used by tests and by the live/stored parity check.
pub fn tune_curve(settings: TuneSettings, pitch: &[f32], conf: &[f32], out: &mut [f32]) {
    let mut t = Tuner::new(crate::frame::FRAME_SECONDS);
    t.set(settings);
    for i in 0..pitch.len() {
        out[i] = t.process(pitch[i], conf[i]);
    }
}

/// Half-width of the running-median window for Level out, in frames (1 s each side).
pub const LEVEL_WINDOW: usize = 100;

/// Level out for stored takes (TDD "Tuning", last paragraph): pull loudness
/// toward its running median by `amount` (0 = unchanged, 1 = flat at the
/// median). The median runs over the sounding frames (loudness > 0) within
/// `LEVEL_WINDOW` frames either side, so a quiet passage is lifted and a loud
/// one tamed. Silent frames stay 0 and the result stays in 0..1.
pub fn level_out(loudness: &[f32], amount: f32, out: &mut [f32]) {
    let n = loudness.len().min(out.len());
    let amount = if amount.is_finite() { amount.clamp(0.0, 1.0) } else { 0.0 };
    if amount == 0.0 {
        out[..n].copy_from_slice(&loudness[..n]);
        return;
    }
    let mut win: Vec<f32> = Vec::with_capacity(2 * LEVEL_WINDOW + 1);
    for i in 0..n {
        let x = loudness[i];
        if !(x > 0.0) {
            out[i] = if x.is_finite() { x.max(0.0) } else { 0.0 };
            continue;
        }
        win.clear();
        let lo = i.saturating_sub(LEVEL_WINDOW);
        let hi = (i + LEVEL_WINDOW + 1).min(n);
        win.extend(loudness[lo..hi].iter().copied().filter(|v| *v > 0.0));
        let mid = win.len() / 2;
        let med = *win.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1;
        out[i] = (med + (1.0 - amount) * (x - med)).clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DTS: [f32; 2] = [0.005, 0.01];

    fn settings() -> TuneSettings {
        TuneSettings { snap: 1.0, speed_ms: 0.0, slides: Slides::Glide, vibrato: 0.0, ..TuneSettings::default() }
    }

    fn run(dt: f32, s: TuneSettings, input: &[f32]) -> Vec<f32> {
        let mut t = Tuner::new(dt);
        t.set(s);
        input.iter().map(|&p| t.process(p, 1.0)).collect()
    }

    fn curve(dt: f32, secs: f32, f: impl Fn(f32) -> f32) -> Vec<f32> {
        let n = (secs / dt).round() as usize;
        (0..n).map(|i| f(i as f32 * dt)).collect()
    }

    #[test]
    fn nan_before_voiced() {
        let mut t = Tuner::new(0.01);
        assert!(t.process(f32::NAN, 1.0).is_nan());
        assert!(t.process(4800.0, 0.1).is_nan());
        assert!(!t.process(4800.0, 0.9).is_nan());
    }

    #[test]
    fn passthrough_constant_is_exact() {
        for dt in DTS {
            let s = TuneSettings { snap: 0.0, vibrato: 1.0, ..TuneSettings::default() };
            let out = run(dt, s, &curve(dt, 1.0, |_| 4837.3));
            assert!(out.iter().all(|&o| o == 4837.3));
        }
    }

    #[test]
    fn passthrough_varying_input() {
        for dt in DTS {
            let s = TuneSettings { snap: 0.0, vibrato: 1.0, speed_ms: 0.0, ..TuneSettings::default() };
            let inp = curve(dt, 2.0, |t| 4800.0 + 80.0 * t + 40.0 * (2.0 * std::f32::consts::PI * 5.0 * t).sin());
            let out = run(dt, s, &inp);
            for (a, b) in inp.iter().zip(&out) {
                assert!((a - b).abs() < 0.02, "{a} vs {b}");
            }
        }
    }

    #[test]
    fn snap_one_lands_on_degree() {
        for dt in DTS {
            let out = run(dt, settings(), &curve(dt, 0.5, |_| 4830.0));
            assert!(out.iter().all(|&o| (o - 4800.0).abs() < 1e-3), "{:?}", &out[..3]);
        }
    }

    #[test]
    fn partial_snap_pulls_partway() {
        let s = TuneSettings { snap: 0.5, ..settings() };
        let out = run(0.01, s, &curve(0.01, 0.5, |_| 4830.0));
        assert!((out[10] - 4815.0).abs() < 1e-3);
    }

    #[test]
    fn hysteresis_up_and_down() {
        for dt in DTS {
            // Rate 20 c/s is below the slide threshold; slow lags input by ~1 cent.
            let up = curve(dt, 5.0, |t| 4800.0 + 20.0 * t);
            let out = run(dt, settings(), &up);
            let first = out.iter().position(|&o| o > 4850.0).unwrap();
            assert!(out[..first].iter().all(|&o| (o - 4800.0).abs() < 1e-3));
            assert!((out[first] - 4900.0).abs() < 1e-3);
            assert!(up[first] > 4859.0 && up[first] < 4863.0, "switched at {}", up[first]);

            let down = curve(dt, 5.0, |t| 4900.0 - 20.0 * t);
            let out = run(dt, settings(), &down);
            let first = out.iter().position(|&o| o < 4850.0).unwrap();
            assert!(out[..first].iter().all(|&o| (o - 4900.0).abs() < 1e-3));
            assert!((out[first] - 4800.0).abs() < 1e-3);
            assert!(down[first] < 4841.0 && down[first] > 4837.0, "switched at {}", down[first]);
        }
    }

    #[test]
    fn hysteresis_wobble_does_not_flip() {
        // Wobble across the 50 cent midpoint but never reach 60%.
        let dt = 0.01;
        let mut inp = vec![4800.0; 50];
        // Ease up to the midpoint so the approach is not itself a slide.
        inp.extend(curve(dt, 1.0, |t| 4800.0 + 50.0 * (t / 1.0)));
        inp.extend(curve(dt, 3.0, |t| 4850.0 + 5.0 * (2.0 * std::f32::consts::PI * 1.0 * t).sin()));
        let out = run(dt, settings(), &inp);
        assert!(out.iter().all(|&o| (o - 4800.0).abs() < 1e-3));
    }

    #[test]
    fn major_scale_non_c_root() {
        // D major: D E F# G A B C#. C is not in it.
        let s = TuneSettings { root: 2, mask: SCALES[1].1, ..settings() };
        for (p, want) in [(4830.0, 4900.0), (4760.0, 4700.0), (5000.0, 5000.0), (5090.0, 5000.0), (5130.0, 5200.0)] {
            let out = run(0.01, s, &curve(0.01, 0.2, |_| p));
            assert!((out[5] - want).abs() < 1e-3, "{p} -> {} want {want}", out[5]);
        }
    }

    #[test]
    fn pentatonic_skips_gaps() {
        // C major pentatonic: C D E G A. 5300 (F4) is between E (5200) and G (5500).
        let s = TuneSettings { mask: SCALES[3].1, ..settings() };
        let out = run(0.01, s, &curve(0.01, 0.2, |_| 5300.0));
        assert!((out[5] - 5200.0).abs() < 1e-3);
    }

    #[test]
    fn ref_cents_shifts_degrees() {
        let s = TuneSettings { ref_cents: 30.0, ..settings() };
        let out = run(0.01, s, &curve(0.01, 0.2, |_| 4845.0));
        assert!((out[5] - 4830.0).abs() < 1e-3);
        // Major: degrees are C=4830, D=5030; 4905 is nearer C than D (C# is out).
        let s = TuneSettings { ref_cents: 30.0, mask: SCALES[1].1, ..settings() };
        let out = run(0.01, s, &curve(0.01, 0.2, |_| 4905.0));
        assert!((out[5] - 4830.0).abs() < 1e-3);
    }

    fn peak_to_peak(x: &[f32]) -> f32 {
        let mx = x.iter().cloned().fold(f32::MIN, f32::max);
        let mn = x.iter().cloned().fold(f32::MAX, f32::min);
        mx - mn
    }

    #[test]
    fn vibrato_flatten_and_exaggerate() {
        // Jump is used because a +-50 cent 5 Hz vibrato leaks through the 3 Hz
        // split fast enough (about 800 c/s peak) to count as a slide, and Glide
        // would then suspend the pull. See the report note on slide detection.
        for dt in DTS {
            let inp = curve(dt, 4.0, |t| 4800.0 + 50.0 * (2.0 * std::f32::consts::PI * 5.0 * t).sin());
            let settle = (1.0 / dt) as usize;
            let jump = TuneSettings { slides: Slides::Jump, ..settings() };
            let flat = run(dt, TuneSettings { vibrato: 0.0, ..jump }, &inp);
            assert!(peak_to_peak(&flat[settle..]) < 1.0, "flat p2p {}", peak_to_peak(&flat[settle..]));
            let one = run(dt, TuneSettings { vibrato: 1.0, ..jump }, &inp);
            let two = run(dt, TuneSettings { vibrato: 2.0, ..jump }, &inp);
            let r = peak_to_peak(&two[settle..]) / peak_to_peak(&one[settle..]);
            assert!((r - 2.0).abs() < 0.05, "ratio {r}");
        }
    }

    #[test]
    fn vibrato_flatten_in_glide_for_modest_depth() {
        let dt = 0.005;
        let inp = curve(dt, 4.0, |t| 4800.0 + 20.0 * (2.0 * std::f32::consts::PI * 5.0 * t).sin());
        let out = run(dt, settings(), &inp);
        assert!(peak_to_peak(&out[200..]) < 1.0);
    }

    #[test]
    fn speed_lag_reaches_63_percent() {
        for dt in DTS {
            let s = TuneSettings { speed_ms: 100.0, ..settings() };
            let mut t = Tuner::new(dt);
            t.set(s);
            for _ in 0..50 {
                assert!((t.process(4800.0, 1.0) - 4800.0).abs() < 1e-3);
            }
            // Moving the reference moves the target by 50 cents at once.
            t.set(TuneSettings { ref_cents: 50.0, ..s });
            let n = (0.1 / dt).round() as usize;
            let mut o = 0.0;
            for _ in 0..n {
                o = t.process(4800.0, 1.0);
            }
            let frac = (o - 4800.0) / 50.0;
            assert!((frac - 0.632).abs() < 0.01, "frac {frac}");
        }
    }

    #[test]
    fn speed_zero_is_instant() {
        let mut t = Tuner::new(0.01);
        t.set(settings());
        t.process(4800.0, 1.0);
        t.set(TuneSettings { ref_cents: 50.0, ..settings() });
        assert!((t.process(4800.0, 1.0) - 4850.0).abs() < 1e-3);
    }

    /// 600 cents up at 1200 c/s after 0.3 s at C4.
    fn ramp(dt: f32, end: f32) -> Vec<f32> {
        curve(dt, 0.3 + (end - 4800.0) / 1200.0 + 0.3, |t| (4800.0 + 1200.0 * (t - 0.3).max(0.0)).min(end))
    }

    #[test]
    fn gliss_steps_through_each_degree() {
        let major = [0, 200, 400, 500, 700, 900, 1100, 1200];
        for dt in DTS {
            // Snap 0 and slow lag: Gliss must override both during the slide.
            let s = TuneSettings { snap: 0.0, speed_ms: 100.0, slides: Slides::Gliss, mask: SCALES[1].1, ..settings() };
            let inp = ramp(dt, 6000.0);
            let out = run(dt, s, &inp);
            let on_degree = |o: f32| major.iter().any(|d| (o - (4800.0 + *d as f32)).abs() < 1e-3);
            let mut seq: Vec<f32> = Vec::new();
            for (i, &o) in out.iter().enumerate() {
                if inp[i] >= 5000.0 && inp[i] <= 5900.0 {
                    assert!(on_degree(o), "off-degree {o} at input {}", inp[i]);
                    if seq.last() != Some(&o) {
                        seq.push(o);
                    }
                }
            }
            for want in [5000.0, 5200.0, 5300.0, 5500.0, 5700.0] {
                assert!(seq.iter().any(|&v| (v - want).abs() < 1e-3), "missed {want}: {seq:?}");
            }
            // Ascending, one degree at a time.
            for w in seq.windows(2) {
                assert!(w[1] > w[0]);
                let k0 = major.iter().position(|d| (w[0] - 4800.0 - *d as f32).abs() < 1e-3).unwrap();
                assert!((w[1] - 4800.0 - major[k0 + 1] as f32).abs() < 1e-3, "skipped a degree: {seq:?}");
            }
        }
    }

    #[test]
    fn glide_keeps_slide_smooth() {
        for dt in DTS {
            let s = TuneSettings { vibrato: 1.0, ..settings() };
            let inp = ramp(dt, 6000.0);
            let out = run(dt, s, &inp);
            let step = 1200.0 * dt;
            for i in 1..inp.len() {
                if inp[i - 1] >= 4950.0 && inp[i] <= 5900.0 {
                    assert!((out[i] - inp[i]).abs() < 0.02, "{} vs {}", out[i], inp[i]);
                    assert!((out[i] - out[i - 1]).abs() < step * 1.05);
                }
            }
        }
    }

    #[test]
    fn jump_holds_then_switches_after_60ms() {
        for dt in DTS {
            let s = TuneSettings { slides: Slides::Jump, ..settings() };
            let t_step = 0.3;
            let inp = curve(dt, 1.0, |t| if t < t_step { 4800.0 } else { 4900.0 });
            let out = run(dt, s, &inp);
            let at = |o: f32| (o - 4800.0).abs() < 1e-3 || (o - 4900.0).abs() < 1e-3;
            assert!(out.iter().all(|&o| at(o)));
            let first = out.iter().position(|&o| o > 4850.0).unwrap();
            let delay = first as f32 * dt - t_step;
            // Slow pitch needs ~50 ms to reach the 60% point, then 60 ms of hold.
            assert!(delay >= JUMP_HOLD_S, "switched after {delay}");
            assert!(delay < 0.13, "switched after {delay}");
            assert!(out[first..].iter().all(|&o| (o - 4900.0).abs() < 1e-3));
        }
    }

    #[test]
    fn jump_ignores_short_excursion() {
        for dt in DTS {
            let s = TuneSettings { slides: Slides::Jump, ..settings() };
            let inp = curve(dt, 1.0, |t| if (0.3..0.38).contains(&t) { 4900.0 } else { 4800.0 });
            let out = run(dt, s, &inp);
            assert!(out.iter().all(|&o| (o - 4800.0).abs() < 1e-3));
        }
    }

    #[test]
    fn jump_applies_to_slow_drift_too() {
        // Not a slide: 20 c/s drift. The switch still waits for the 60 ms hold.
        let dt = 0.01;
        let s = TuneSettings { slides: Slides::Jump, ..settings() };
        let inp = curve(dt, 5.0, |t| 4800.0 + 20.0 * t);
        let out = run(dt, s, &inp);
        let first = out.iter().position(|&o| o > 4850.0).unwrap();
        // Target flips near 4860; at 20 c/s the 60 ms hold adds ~1.2 cents.
        assert!(inp[first] > 4860.0);
        assert!(out[..first].iter().all(|&o| (o - 4800.0).abs() < 1e-3));
    }

    #[test]
    fn gap_holds_and_reseeds() {
        let s = TuneSettings { snap: 0.0, vibrato: 1.0, speed_ms: 60.0, ..TuneSettings::default() };
        let mut t = Tuner::new(0.01);
        t.set(s);
        let mut last = 0.0;
        for _ in 0..30 {
            last = t.process(4800.0, 1.0);
        }
        for _ in 0..10 {
            assert_eq!(t.process(f32::NAN, 0.0), last);
            assert_eq!(t.process(5000.0, 0.2), last);
            assert_eq!(t.process(f32::INFINITY, 1.0), last);
        }
        // Back at a new pitch: no glide from the stale filter.
        assert_eq!(t.process(5300.0, 1.0), 5300.0);
        assert_eq!(t.process(5300.0, 1.0), 5300.0);
    }

    #[test]
    fn nan_only_input_is_nan() {
        let mut t = Tuner::new(0.005);
        for _ in 0..100 {
            assert!(t.process(f32::NAN, 1.0).is_nan());
            assert!(t.process(f32::NAN, 0.0).is_nan());
        }
    }

    #[test]
    fn reset_clears_state() {
        let mut t = Tuner::new(0.01);
        t.set(settings());
        for _ in 0..20 {
            t.process(4830.0, 1.0);
        }
        t.reset();
        assert!(t.process(f32::NAN, 1.0).is_nan());
        assert!((t.process(5230.0, 1.0) - 5200.0).abs() < 1e-3);
        // Settings survive reset.
        assert_eq!(t.settings().snap, 1.0);
    }

    #[test]
    fn mask_change_repicks_target() {
        let mut t = Tuner::new(0.01);
        t.set(settings());
        for _ in 0..10 {
            t.process(4900.0, 1.0);
        }
        // C# is not in C major; the held target must move to a legal degree.
        t.set(TuneSettings { mask: SCALES[1].1, ..settings() });
        let o = t.process(4900.0, 1.0);
        assert!((o - 4800.0).abs() < 1e-3 || (o - 5000.0).abs() < 1e-3);
    }

    #[test]
    fn extreme_input_does_not_panic() {
        let mut t = Tuner::new(0.005);
        t.set(TuneSettings { slides: Slides::Jump, mask: 0, ..settings() });
        for p in [f32::MAX, f32::MIN, 1e30, -1e30, 0.0, 4800.0] {
            assert!(t.process(p, 1.0).is_finite());
        }
        assert!(t.process(4800.0, f32::NAN).is_finite());
    }

    #[test]
    fn tune_curve_matches_stepwise() {
        let dt = crate::frame::FRAME_SECONDS;
        let n = 600;
        let pitch: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 * dt;
                if (2.0..2.3).contains(&t) {
                    f32::NAN
                } else {
                    4800.0 + 300.0 * t.min(3.0) + 40.0 * (2.0 * std::f32::consts::PI * 5.5 * t).sin()
                }
            })
            .collect();
        let conf: Vec<f32> = (0..n).map(|i| if (4.0..4.2).contains(&(i as f32 * dt)) { 0.1 } else { 0.9 }).collect();
        for slides in [Slides::Glide, Slides::Gliss, Slides::Jump] {
            let s = TuneSettings {
                snap: 0.8,
                speed_ms: 40.0,
                slides,
                vibrato: 1.3,
                root: 3,
                mask: SCALES[2].1,
                ref_cents: 12.0,
            };
            let mut out = vec![0.0; n];
            tune_curve(s, &pitch, &conf, &mut out);
            let mut t = Tuner::new(dt);
            t.set(s);
            for i in 0..n {
                let o = t.process(pitch[i], conf[i]);
                assert_eq!(o.to_bits(), out[i].to_bits(), "frame {i}");
            }
        }
    }

    #[test]
    fn level_out_zero_is_identity_and_one_is_flat() {
        let x: Vec<f32> = (0..300).map(|i| 0.3 + 0.4 * (i % 7) as f32 / 6.0).collect();
        let mut y = vec![0.0; x.len()];
        level_out(&x, 0.0, &mut y);
        assert_eq!(x, y);
        level_out(&x, 1.0, &mut y);
        let spread = |v: &[f32]| v[100..200].iter().cloned().fold(0.0f32, f32::max) - v[100..200].iter().cloned().fold(1.0f32, f32::min);
        assert!(spread(&y) < 0.5 * spread(&x) + 1e-6, "{} {}", spread(&y), spread(&x));
    }

    #[test]
    fn level_out_lifts_quiet_and_tames_loud_and_keeps_silence() {
        let mut x = vec![0.6f32; 400];
        for v in &mut x[100..150] {
            *v = 0.2;
        }
        for v in &mut x[200..250] {
            *v = 1.0;
        }
        for v in &mut x[300..320] {
            *v = 0.0;
        }
        let mut y = vec![0.0; x.len()];
        level_out(&x, 0.5, &mut y);
        assert!(y[120] > x[120] && y[120] < 0.6);
        assert!(y[220] < x[220] && y[220] > 0.6);
        assert!(y[310] == 0.0);
        assert!((y[50] - 0.6).abs() < 1e-6);
        assert!(y.iter().all(|v| (0.0..=1.0).contains(v)));
        let mut z = vec![0.0; x.len()];
        level_out(&x, 7.0, &mut z);
        level_out(&x, 1.0, &mut y);
        assert_eq!(y, z);
    }

    #[test]
    fn level_out_handles_empty_and_nan() {
        let mut y = [0.0f32; 3];
        level_out(&[], 0.5, &mut y);
        level_out(&[f32::NAN, 0.5, 0.5], 0.5, &mut y);
        assert_eq!(y[0], 0.0);
        assert!(y[1] > 0.0);
    }
}
