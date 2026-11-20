//! Sample-accurate metronome and count-in.
//!
//! Clicks are generated from the transport's beat crossings (one sample when
//! the grid position lands on a beat) or from this clock during a count-in.
//! The engine mixes them into the live output only. `ew_render` never calls
//! this, and the click is not written into the mic buffer, so a take and an
//! export stay free of the click.

/// One click inside the span just advanced, at `offset` samples.
#[derive(Clone, Copy, Debug)]
pub struct Click {
    pub offset: u32,
    pub accent: bool,
}

pub struct Metro {
    pub on: bool,
    pub level: f32,
    pub kit: bool,
    pub kit_layer: usize,
    pub kit_slot: usize,
    pub bar: u32,
    sr: f32,
    env: f32,
    phase: f32,
    freq: f32,
    amp: f32,
    noise: u32,
    counting: bool,
    count_bpm: f64,
    /// Samples elapsed since the count-in started. Position is a closed form
    /// of this count, not a running sum, so a beat does not drift.
    count_samples: u64,
    count_prev: i64,
    count_clicks: u32,
    /// Finished count-ins. The interface starts a take when this changes.
    pub count_seq: u32,
    pub clicks: [Click; 8],
    pub n_clicks: usize,
}

impl Metro {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            on: false,
            level: 0.7,
            kit: false,
            kit_layer: 0,
            kit_slot: 0,
            bar: 4,
            sr: sample_rate.max(1.0),
            env: 0.0,
            phase: 0.0,
            freq: 1200.0,
            amp: 0.0,
            noise: 0xA341_316C,
            counting: false,
            count_bpm: 100.0,
            count_samples: 0,
            count_prev: -1,
            count_clicks: 0,
            count_seq: 0,
            clicks: [Click {
                offset: 0,
                accent: false,
            }; 8],
            n_clicks: 0,
        }
    }

    pub fn counting(&self) -> bool {
        self.counting
    }

    /// Beats of the count-in still to hear, including the one that has not
    /// fired yet. 0 when idle.
    pub fn remaining(&self) -> u32 {
        if !self.counting {
            0
        } else {
            self.bar.saturating_sub(self.count_clicks)
        }
    }

    /// Beat within the bar while counting (0 = the accent just heard, or about to be).
    pub fn shown_beat(&self) -> u32 {
        if self.count_clicks == 0 {
            0
        } else {
            (self.count_clicks - 1) % self.bar.max(1)
        }
    }

    pub fn set(&mut self, param: u16, value: f32) {
        match param {
            0 => self.on = value > 0.5, // METRO_ON
            1 => {
                if value.is_finite() {
                    self.level = value.clamp(0.0, 1.0);
                }
            }
            2 => self.kit = value > 0.5, // METRO_VOICE
            3 => {
                if value.is_finite() {
                    self.kit_layer = (value.max(0.0) as usize).min(7);
                }
            }
            4 => {
                if value.is_finite() {
                    self.kit_slot = (value.max(0.0) as usize).min(3);
                }
            }
            5 => {
                if value.is_finite() {
                    self.bar = (value.round() as u32).clamp(1, 32);
                }
            }
            _ => {}
        }
    }

    /// Drop a count-in without starting the transport (the user hit play or stop).
    pub fn cancel(&mut self) {
        self.counting = false;
        self.n_clicks = 0;
    }

    /// One bar of clicks, then the caller starts the transport on the next sample.
    pub fn start_count(&mut self, bpm: f64) {
        if self.counting {
            return;
        }
        let bpm = if bpm.is_finite() { bpm } else { 100.0 };
        self.counting = true;
        self.count_bpm = bpm.clamp(40.0, 300.0);
        self.count_samples = 0;
        self.count_prev = -1;
        self.count_clicks = 0;
        self.n_clicks = 0;
    }

    /// Advance the count-in by up to `n` samples.
    /// Returns `(consumed, start_transport)`.
    /// `start_transport` means sample `consumed` (which may be 0) is the
    /// downbeat and was not consumed: the transport should play from there.
    /// Clicks in this slice are left in `clicks`.
    pub fn advance_count(&mut self, n: usize) -> (usize, bool) {
        self.n_clicks = 0;
        if !self.counting || n == 0 {
            return (0, false);
        }
        let mut i = 0usize;
        while i < n && self.counting {
            let j = self.count_samples + i as u64;
            let pos = self.count_bpm * j as f64 * 960.0 / (60.0 * self.sr as f64);
            let beat = ((pos + 1e-6) / 960.0).floor() as i64;
            if beat != self.count_prev {
                if self.count_clicks >= self.bar {
                    self.counting = false;
                    self.count_seq = self.count_seq.wrapping_add(1);
                    self.count_samples += i as u64;
                    return (i, true);
                }
                let accent = self.count_clicks % self.bar.max(1) == 0;
                self.push_click(i as u32, accent);
                self.count_clicks += 1;
                self.count_prev = beat;
            }
            i += 1;
        }
        self.count_samples += i as u64;
        (i, false)
    }

    fn push_click(&mut self, offset: u32, accent: bool) {
        if self.n_clicks < self.clicks.len() {
            self.clicks[self.n_clicks] = Click { offset, accent };
            self.n_clicks += 1;
        }
    }

    /// Start a tick. `count_in` keeps a quiet level audible so the bar can be
    /// heard with the metronome level at zero.
    pub fn trigger(&mut self, accent: bool, count_in: bool) {
        let level = if count_in {
            self.level.max(0.45)
        } else {
            self.level
        };
        if level <= 0.0001 {
            return;
        }
        self.env = 1.0;
        self.freq = if accent { 1760.0 } else { 1174.0 };
        self.amp = level * if accent { 0.85 } else { 0.48 };
        self.phase = 0.0;
    }

    /// Mix the tick into `l` and `r` (the same mono click on both sides).
    /// No work when the envelope is idle, so a metronome that is off does not
    /// touch the buffer.
    pub fn mix(&mut self, l: &mut [f32], r: &mut [f32]) {
        if self.env < 1e-5 {
            return;
        }
        let decay = (-1.0 / (self.sr * 0.016)).exp();
        let inc = self.freq / self.sr;
        for i in 0..l.len() {
            self.phase += inc;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            }
            let sine = (self.phase * std::f32::consts::TAU).sin();
            self.noise = self.noise.wrapping_mul(1664525).wrapping_add(1013904223);
            let nz = ((self.noise >> 16) as i16 as f32) / 32768.0;
            let burst = if self.env > 0.55 { nz } else { 0.0 };
            let s = self.amp * self.env * (0.72 * sine + 0.28 * burst);
            l[i] += s;
            r[i] += s;
            self.env *= decay;
            if self.env < 1e-5 {
                self.env = 0.0;
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_in_is_one_bar_then_the_downbeat_sample() {
        let mut m = Metro::new(48_000.0);
        m.bar = 4;
        m.start_count(120.0);
        // 120 bpm, 48 kHz: 24000 samples per beat. Four clicks, then sample 96000 starts the song.
        let mut at = 0usize;
        let mut clicks = Vec::new();
        let mut started = None;
        while started.is_none() && at < 200_000 {
            let (n, start) = m.advance_count(128);
            for c in &m.clicks[..m.n_clicks] {
                clicks.push(at + c.offset as usize);
            }
            at += n;
            if start {
                started = Some(at);
            }
        }
        assert_eq!(clicks, vec![0, 24_000, 48_000, 72_000]);
        assert_eq!(started, Some(96_000));
        assert!(!m.counting());
        assert_eq!(m.count_seq, 1);
    }

    #[test]
    fn count_in_does_not_depend_on_block_size() {
        let run = |block: usize| {
            let mut m = Metro::new(48_000.0);
            m.bar = 4;
            m.start_count(100.0);
            let mut at = 0usize;
            let mut clicks = Vec::new();
            let mut started = 0usize;
            while started == 0 && at < 400_000 {
                let (n, start) = m.advance_count(block);
                for c in &m.clicks[..m.n_clicks] {
                    clicks.push(at + c.offset as usize);
                }
                at += n;
                if start {
                    started = at;
                }
            }
            (clicks, started)
        };
        let a = run(128);
        assert_eq!(run(1), a);
        assert_eq!(run(100), a);
        assert_eq!(run(32), a);
        // 100 bpm: 28800 samples per beat. Four clicks, start on the fifth beat.
        assert_eq!(a.0, vec![0, 28_800, 57_600, 86_400]);
        assert_eq!(a.1, 115_200);
    }
}
