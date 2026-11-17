//! YIN pitch estimation (de Cheveigné & Kawahara 2002).
//!
//! Live tier settings (TDD): ~16 kHz input, 512-sample window (32 ms), 80-sample
//! hop (5 ms), 70 to 1,000 Hz, parabolic refinement. Window and hop are given in
//! seconds here so other rates keep the same timing.

/// Result of one YIN analysis.
#[derive(Clone, Copy, Debug, Default)]
pub struct YinEstimate {
    /// Frequency in Hz, 0 when no candidate under the threshold or in range.
    pub hz: f32,
    /// 1 - cmnd at the chosen lag, clamped to 0..1.
    pub confidence: f32,
}

/// Largest lag searched: one period of `fmin`, rounded up (229 for 70 Hz at 16 kHz).
fn max_lag_for(rate: f32, fmin: f32) -> usize {
    ((rate / fmin).ceil() as usize).max(4)
}

/// Cumulative mean normalized difference d'(tau) for tau in 0..max_lag.
/// The difference function sums over window = frame.len() - max_lag samples.
/// out[0] = 1. Writes out[..max_lag].
pub fn cmnd(frame: &[f32], max_lag: usize, out: &mut [f32]) {
    let window = frame.len() - max_lag;
    out[0] = 1.0;
    let mut running = 0.0f32;
    for tau in 1..max_lag {
        let a = &frame[..window];
        let b = &frame[tau..tau + window];
        let mut d = 0.0f32;
        for (x, y) in a.iter().zip(b) {
            let e = x - y;
            d += e * e;
        }
        running += d;
        // Silent input gives 0/0; call it "no periodicity".
        out[tau] = if running > 1e-12 { d * tau as f32 / running } else { 1.0 };
    }
}

/// Parabolic interpolation of the minimum around `tau` in `d`. Returns fractional lag.
pub fn parabolic(d: &[f32], tau: usize) -> f32 {
    if tau == 0 || tau + 1 >= d.len() {
        return tau as f32;
    }
    let (a, b, c) = (d[tau - 1], d[tau], d[tau + 1]);
    let den = a - 2.0 * b + c;
    if den <= 1e-12 {
        return tau as f32;
    }
    tau as f32 + (0.5 * (a - c) / den).clamp(-1.0, 1.0)
}

/// One stateless YIN pass over `frame` (len = window + max_lag samples).
/// Picks the first local minimum of cmnd below `threshold` within
/// [min_lag, max_lag); if none, the global minimum in range with its
/// (low) confidence. `scratch.len() >= max_lag`.
pub fn yin(frame: &[f32], rate: f32, fmin: f32, fmax: f32, threshold: f32, scratch: &mut [f32]) -> YinEstimate {
    let max_lag = max_lag_for(rate, fmin);
    let min_lag = ((rate / fmax).floor() as usize).max(2);
    if frame.len() <= max_lag || scratch.len() < max_lag || min_lag + 2 >= max_lag {
        return YinEstimate::default();
    }
    cmnd(frame, max_lag, scratch);
    let d = &scratch[..max_lag];

    // Stop one short of the end so parabolic interpolation has a right neighbour.
    let hi = max_lag - 1;
    let mut pick = None;
    let mut tau = min_lag;
    while tau < hi {
        if d[tau] < threshold {
            while tau + 1 < hi && d[tau + 1] < d[tau] {
                tau += 1;
            }
            pick = Some(tau);
            break;
        }
        tau += 1;
    }
    let tau = pick.unwrap_or_else(|| {
        let mut best = min_lag;
        for t in min_lag..hi {
            if d[t] < d[best] {
                best = t;
            }
        }
        best
    });

    let confidence = (1.0 - d[tau]).clamp(0.0, 1.0);
    let lag = parabolic(d, tau);
    let hz = if confidence > 0.0 && lag > 0.0 { rate / lag } else { 0.0 };
    YinEstimate { hz, confidence }
}

/// Streaming YIN: push samples, get an estimate every hop once primed.
pub struct Yin {
    rate: f32,
    fmin: f32,
    fmax: f32,
    threshold: f32,
    hop: usize,
    ring: Vec<f32>,
    pos: usize,
    filled: usize,
    since: usize,
    frame: Vec<f32>,
    scratch: Vec<f32>,
}

impl Yin {
    /// `window_s` 0.032 and `hop_s` 0.005 in the live tier; threshold 0.15.
    pub fn new(rate: f32, fmin: f32, fmax: f32, window_s: f32, hop_s: f32, threshold: f32) -> Self {
        let window = ((window_s * rate).round() as usize).max(8);
        let max_lag = max_lag_for(rate, fmin);
        let hop = ((hop_s * rate).round() as usize).max(1);
        let len = window + max_lag;
        Self {
            rate,
            fmin,
            fmax,
            threshold,
            hop,
            ring: vec![0.0; len],
            pos: 0,
            filled: 0,
            since: 0,
            frame: vec![0.0; len],
            scratch: vec![0.0; max_lag],
        }
    }
    /// Push one sample. Returns Some every hop samples once the buffer is full.
    #[inline]
    pub fn push(&mut self, x: f32) -> Option<YinEstimate> {
        self.ring[self.pos] = if x.is_finite() { x } else { 0.0 };
        self.pos += 1;
        if self.pos == self.ring.len() {
            self.pos = 0;
        }
        if self.filled < self.ring.len() {
            self.filled += 1;
            if self.filled < self.ring.len() {
                return None;
            }
            // First full buffer: analyse right away, then every hop.
            self.since = 0;
        } else {
            self.since += 1;
            if self.since < self.hop {
                return None;
            }
            self.since = 0;
        }
        // Linearise oldest-first; pos is the oldest sample.
        let (head, tail) = self.ring.split_at(self.pos);
        self.frame[..tail.len()].copy_from_slice(tail);
        self.frame[tail.len()..].copy_from_slice(head);
        Some(yin(&self.frame, self.rate, self.fmin, self.fmax, self.threshold, &mut self.scratch))
    }
    /// Delay from the newest sample to the analysed window's centre, in seconds.
    ///
    /// The frame is window + max_lag samples; the comparisons span all of it,
    /// so the centre of what is analysed is (window + max_lag) / 2 before the
    /// newest sample, i.e. window/2 + max_lag/2.
    pub fn latency_s(&self) -> f32 {
        0.5 * self.ring.len() as f32 / self.rate
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::noise::Noise;

    const RATE: f32 = 16_000.0;

    fn cents(a: f32, b: f32) -> f32 {
        1200.0 * (a / b).log2().abs()
    }

    fn frame_of(mut f: impl FnMut(usize) -> f32) -> Vec<f32> {
        (0..512 + 229).map(f).collect()
    }

    #[test]
    fn sine_220() {
        let fr = frame_of(|i| (2.0 * std::f32::consts::PI * 220.0 * i as f32 / RATE).sin());
        let mut s = vec![0.0; 229];
        let e = yin(&fr, RATE, 70.0, 1000.0, 0.15, &mut s);
        assert!(cents(e.hz, 220.0) < 5.0, "{}", e.hz);
        assert!(e.confidence > 0.9, "{}", e.confidence);
    }

    #[test]
    fn bandlimited_saw() {
        for &f0 in &[110.0f32, 220.0, 440.0] {
            let fr = frame_of(|i| {
                let mut v = 0.0;
                let mut k = 1;
                while (k as f32) * f0 < RATE / 2.0 {
                    v += (2.0 * std::f32::consts::PI * f0 * k as f32 * i as f32 / RATE).sin() / k as f32;
                    k += 1;
                }
                v
            });
            let mut s = vec![0.0; 229];
            let e = yin(&fr, RATE, 70.0, 1000.0, 0.15, &mut s);
            assert!(cents(e.hz, f0) < 5.0, "f0 {f0} got {}", e.hz);
            assert!(e.confidence > 0.9, "f0 {f0} conf {}", e.confidence);
        }
    }

    #[test]
    fn white_noise_low_confidence() {
        let mut n = Noise::new(7);
        let fr = frame_of(|_| n.next());
        let mut s = vec![0.0; 229];
        let e = yin(&fr, RATE, 70.0, 1000.0, 0.15, &mut s);
        assert!(e.confidence < 0.6, "{}", e.confidence);
    }

    #[test]
    fn silence_is_not_confident() {
        let fr = frame_of(|_| 0.0);
        let mut s = vec![0.0; 229];
        let e = yin(&fr, RATE, 70.0, 1000.0, 0.15, &mut s);
        assert_eq!(e.confidence, 0.0);
        assert_eq!(e.hz, 0.0);
    }

    #[test]
    fn cmnd_and_parabolic() {
        let fr = frame_of(|i| (2.0 * std::f32::consts::PI * 400.0 * i as f32 / RATE).sin());
        let mut d = vec![0.0; 229];
        cmnd(&fr, 229, &mut d);
        assert_eq!(d[0], 1.0);
        assert!(d[40] < 0.01); // period is exactly 40 samples
        assert!(d[20] > 0.9);
        assert!((parabolic(&d, 40) - 40.0).abs() < 0.05);
        assert_eq!(parabolic(&d, 0), 0.0);
    }

    #[test]
    fn streaming_gives_estimate_every_hop() {
        let mut y = Yin::new(RATE, 70.0, 1000.0, 0.032, 0.005, 0.15);
        let total = 4000;
        let mut hits = Vec::new();
        let mut last = None;
        for i in 0..total {
            let x = (2.0 * std::f32::consts::PI * 220.0 * i as f32 / RATE).sin();
            if let Some(e) = y.push(x) {
                hits.push(i);
                last = Some(e);
            }
        }
        // Buffer = 512 + 229 = 741 samples; first estimate when full, then every 80.
        assert_eq!(hits[0], 740);
        assert!(hits.windows(2).all(|w| w[1] - w[0] == 80));
        assert_eq!(hits.len(), (total - 741) / 80 + 1);
        let e = last.unwrap();
        assert!(cents(e.hz, 220.0) < 5.0 && e.confidence > 0.9);
        assert!((y.latency_s() - 741.0 / 2.0 / RATE).abs() < 1e-6);
    }
}
