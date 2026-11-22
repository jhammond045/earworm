//! Scoring tracked notes against the notes played.
//!
//! A played note is found when a tracked note of the same semitone starts
//! near its onset: live, from 20 ms before to 100 ms after (the detection
//! time, so the latency counts); offline, within 50 ms either way (the usual
//! transcription tolerance). Each tracked note can be used once.

use crate::guitar::Note;
use crate::spectrum::midi_hz;
use crate::track::Est;

#[derive(Default)]
pub struct Metrics {
    pub n_ref: usize,
    pub n_est: usize,
    pub tp: usize,
    /// False notes an octave or two from a note that sounds.
    pub fp_octave: usize,
    /// ... a twelfth, fifth, fourth or higher partial from one.
    pub fp_harmonic: usize,
    /// ... a semitone from one.
    pub fp_semitone: usize,
    /// ... on a note that sounds, outside its onset window: a dropout and
    /// return, or a retrigger with no re-pluck.
    pub fp_repeat: usize,
    pub fp_other: usize,
    /// Missed notes with a note sounding one or two octaves below them.
    pub miss_shadowed: usize,
    pub miss_other: usize,
    pub latency: Vec<f64>,
    /// Per 10 ms: notes sounding and tracked, tracked only, sounding only.
    pub frame: (usize, usize, usize),
    /// (played velocity dB, tracked level dB) of found notes.
    pub vel: Vec<(f64, f64)>,
    /// Pitch error of found notes, cents.
    pub cents: Vec<f64>,
    /// Per section: name, notes played, notes found.
    pub sections: Vec<(&'static str, usize, usize)>,
}

impl Metrics {
    pub fn add(&mut self, o: &Metrics) {
        self.n_ref += o.n_ref;
        self.n_est += o.n_est;
        self.tp += o.tp;
        self.fp_octave += o.fp_octave;
        self.fp_harmonic += o.fp_harmonic;
        self.fp_semitone += o.fp_semitone;
        self.fp_repeat += o.fp_repeat;
        self.fp_other += o.fp_other;
        self.miss_shadowed += o.miss_shadowed;
        self.miss_other += o.miss_other;
        self.latency.extend(&o.latency);
        self.frame.0 += o.frame.0;
        self.frame.1 += o.frame.1;
        self.frame.2 += o.frame.2;
        self.vel.extend(&o.vel);
        self.cents.extend(&o.cents);
        for &(name, n, tp) in &o.sections {
            match self.sections.iter_mut().find(|s| s.0 == name) {
                Some(s) => {
                    s.1 += n;
                    s.2 += tp;
                }
                None => self.sections.push((name, n, tp)),
            }
        }
    }
    pub fn precision(&self) -> f64 {
        self.tp as f64 / self.n_est.max(1) as f64
    }
    pub fn recall(&self) -> f64 {
        self.tp as f64 / self.n_ref.max(1) as f64
    }
    pub fn f1(&self) -> f64 {
        let (p, r) = (self.precision(), self.recall());
        if p + r > 0.0 { 2.0 * p * r / (p + r) } else { 0.0 }
    }
    pub fn frame_f1(&self) -> f64 {
        let (tp, fp, fn_) = self.frame;
        2.0 * tp as f64 / (2 * tp + fp + fn_).max(1) as f64
    }
}

pub fn percentile(v: &[f64], q: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    s[((s.len() - 1) as f64 * q).round() as usize]
}

pub fn pearson(v: &[(f64, f64)]) -> f64 {
    let n = v.len() as f64;
    let (mx, my) = (v.iter().map(|p| p.0).sum::<f64>() / n, v.iter().map(|p| p.1).sum::<f64>() / n);
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for &(x, y) in v {
        sxy += (x - mx) * (y - my);
        sxx += (x - mx) * (x - mx);
        syy += (y - my) * (y - my);
    }
    sxy / (sxx * syy).sqrt().max(1e-12)
}

fn mask(notes: impl Iterator<Item = u8>) -> u128 {
    notes.fold(0u128, |m, n| m | 1u128 << n)
}

/// `window`: how early and how late a tracked onset may be, seconds.
pub fn score(truth: &[Note], est: &[Est], live: bool, window: (f64, f64), sections: &[(f64, &'static str)], len: f64) -> Metrics {
    let on = |e: &Est| if live { e.on } else { e.first };
    let off = |e: &Est| if live { e.off } else { e.last };
    let (pre, post) = window;
    let mut m = Metrics { n_ref: truth.len(), n_est: est.len(), ..Default::default() };
    m.sections = sections.iter().map(|s| (s.1, 0, 0)).collect();
    let section_of = |t: f64| sections.iter().rposition(|s| s.0 <= t + 1e-9).unwrap_or(0);

    let mut used = vec![false; est.len()];
    for t in truth {
        let mut best: Option<usize> = None;
        for (i, e) in est.iter().enumerate() {
            let d = on(e) - t.on;
            if used[i] || e.midi != t.midi || d < -pre || d > post {
                continue;
            }
            if best.is_none_or(|b| d.abs() < (on(&est[b]) - t.on).abs()) {
                best = Some(i);
            }
        }
        let sec = section_of(t.on);
        m.sections[sec].1 += 1;
        match best {
            Some(i) => {
                used[i] = true;
                m.tp += 1;
                m.sections[sec].2 += 1;
                let e = &est[i];
                m.latency.push(on(e) - t.on);
                m.vel.push((t.vel_db, 20.0 * (e.level.max(1e-9) as f64).log10()));
                let true_cents = 1200.0 * (t.hz / midi_hz(t.midi as f32) as f64).log2();
                m.cents.push((e.cents as f64 - true_cents).abs());
            }
            None => {
                let shadowed = truth.iter().any(|u| {
                    u.on <= t.on + 0.05 && u.off > t.on && (t.midi as i32 - u.midi as i32 == 12 || t.midi as i32 - u.midi as i32 == 24)
                });
                if shadowed {
                    m.miss_shadowed += 1;
                } else {
                    m.miss_other += 1;
                }
            }
        }
    }

    for (i, e) in est.iter().enumerate() {
        if used[i] {
            continue;
        }
        let te = on(e);
        let diffs: Vec<i32> =
            truth.iter().filter(|u| u.on - 0.05 <= te && te <= u.off + 0.05).map(|u| e.midi as i32 - u.midi as i32).collect();
        let any = |set: &[i32]| diffs.iter().any(|d| set.contains(&d.abs()));
        if any(&[0]) {
            m.fp_repeat += 1;
        } else if any(&[12, 24, 36]) {
            m.fp_octave += 1;
        } else if any(&[5, 7, 19, 28, 31]) {
            m.fp_harmonic += 1;
        } else if any(&[1]) {
            m.fp_semitone += 1;
        } else {
            m.fp_other += 1;
        }
    }

    let steps = (len / 0.01) as usize;
    for s in 0..steps {
        let t = s as f64 * 0.01;
        let r = mask(truth.iter().filter(|u| u.on <= t && t < u.off).map(|u| u.midi));
        let e = mask(est.iter().filter(|e| on(e) <= t && t < off(e)).map(|e| e.midi));
        m.frame.0 += (r & e).count_ones() as usize;
        m.frame.1 += (e & !r).count_ones() as usize;
        m.frame.2 += (r & !e).count_ones() as usize;
    }
    m
}
