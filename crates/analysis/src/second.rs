//! The capture second pass (TDD "Second pass" and "Beats"): runs over a whole
//! take after release and adds structure on top of the live frames. Timbre
//! (loudness, bands, openness, breathiness) is never recomputed; only pitch is
//! repaired, and notes, hits, tempo, scale and kit slots are added.
//!
//! Frame records are 25 floats (`STRIDE`): the 24-float feature frame followed
//! by the tuned pitch. Frame k sits at k / 100 s of the take audio.

use crate::beats::{Hit, HitInput, Taught, group, measure_hits};
use crate::notes::{self, Note};
use crate::pyin::pyin;
use crate::signal::{FPS, Levels, detect_onsets, flux};
use crate::tempo::{find_tempo, loop_beats};
use dsp::frame::{CONFIDENCE, LOUDNESS, PITCH, VOICING};

pub const STRIDE: usize = 25;

/// Kind detection thresholds. Fitted on the synthetic corpora in `tests/`
/// (`hum_corpus`, `beat_corpus`); see `detect_kind`.
pub mod thresh {
    /// Onsets per second needed to call a take beats ...
    pub const BEAT_ONSETS_PER_SEC: f32 = 1.0;
    /// ... with a median event no longer than this, seconds ...
    pub const BEAT_MAX_EVENT: f32 = 0.6;
    /// ... and at least this share of percussive events ...
    pub const BEAT_PERCUSSIVE: f32 = 0.5;
    /// ... or this share when the take is voiced and holds its pitch (voiced fraction and
    /// stability at least `PITCHED`), since a hummed note and a pitched "boom" look alike
    /// except for the way they die away.
    pub const BEAT_PERCUSSIVE_PITCHED: f32 = 0.75;
    pub const PITCHED: f32 = 0.65;
    /// An event is percussive when it is shorter than this, seconds ...
    pub const PERC_HOLD: f32 = 0.12;
    /// ... or its level has fallen 10 dB below its peak within this share of its length ...
    pub const PERC_T10: f32 = 0.55;
    /// ... or within this many seconds.
    pub const PERC_T10_S: f32 = 0.16;
    /// Texture: below this voiced fraction ...
    pub const TEXTURE_MAX_VOICED: f32 = 0.4;
    /// ... or voiced but with a pitch that never settles (stability below this).
    pub const TEXTURE_MAX_STABILITY: f32 = 0.4;
    /// A voiced frame is stable when within this many cents of its note's median.
    pub const STABLE_CENTS: f32 = 100.0;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Melody,
    Beat,
    Texture,
}

impl Kind {
    pub fn index(self) -> f32 {
        match self {
            Kind::Melody => 0.0,
            Kind::Beat => 1.0,
            Kind::Texture => 2.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// None = detect.
    pub kind: Option<Kind>,
    /// Number of kit groups for beats, 0 = best silhouette.
    pub kinds: usize,
    pub taught: [Option<Taught>; 4],
}

impl Default for Options {
    fn default() -> Self {
        Self { kind: None, kinds: 0, taught: [None; 4] }
    }
}

/// Floats in the flat options layout: kind, kinds, then four slots of
/// (present, centre[6], spread[6]).
pub const OPTIONS_LEN: usize = 2 + 4 * 13;

impl Options {
    /// Decode the flat layout. Missing floats read as defaults.
    pub fn from_floats(f: &[f32]) -> Self {
        let get = |i: usize| f.get(i).copied().unwrap_or(0.0);
        let kind = match get(0).round() as i32 {
            1 => Some(Kind::Melody),
            2 => Some(Kind::Beat),
            3 => Some(Kind::Texture),
            _ => None,
        };
        let kinds = get(1).max(0.0).round() as usize;
        let mut taught = [None; 4];
        for (s, slot) in taught.iter_mut().enumerate() {
            let o = 2 + s * 13;
            if get(o) > 0.5 {
                *slot = Some(Taught { centre: std::array::from_fn(|d| get(o + 1 + d)), spread: std::array::from_fn(|d| get(o + 7 + d)) });
            }
        }
        Self { kind, kinds, taught }
    }
}

#[derive(Clone, Debug)]
pub struct SecondPassResult {
    pub kind: Kind,
    pub voiced_fraction: f32,
    pub onsets_per_sec: f32,
    /// Repaired pitch per frame, cents, NaN unvoiced.
    pub pitch: Vec<f32>,
    pub repaired: usize,
    pub ref_cents: f32,
    pub root: u8,
    pub scale: usize,
    pub bpm: Option<f32>,
    pub beats: Option<u32>,
    pub loop_sec: f32,
    pub notes: Vec<Note>,
    pub hits: Vec<Hit>,
    pub kinds: usize,
    /// What kind detection looked at: voiced fraction, onsets per second, median event
    /// length (s), pitch stability, percussive share. For tests and tools; not in the flat layout.
    pub kind_features: [f32; 5],
}

/// Header floats of the flat output.
pub const OUT_HEADER: usize = 16;
/// Floats per note in the flat output: t, dur, cents, hard.
pub const NOTE_LEN: usize = 4;
/// Floats per hit: t, dur, slot, strength, attack, burst, cents (NaN = none), bend, hold, vowel,
/// tone, endStop, endClick, features[6], live[3].
pub const HIT_LEN: usize = 22;

impl SecondPassResult {
    /// Floats needed by `to_floats`.
    pub fn flat_len(&self) -> usize {
        OUT_HEADER + self.pitch.len() + self.notes.len() * NOTE_LEN + self.hits.len() * HIT_LEN
    }

    /// Flat layout for the C ABI:
    ///
    /// | Floats | Content |
    /// | --- | --- |
    /// | 0 | kind: 0 melody, 1 beat, 2 texture |
    /// | 1 | voiced fraction |
    /// | 2 | onsets per second |
    /// | 3 | frames repaired |
    /// | 4 | tuning reference, cents |
    /// | 5, 6 | root pitch class, scale index |
    /// | 7, 8 | bpm, beats (NaN when there is no tempo) |
    /// | 9 | loop length, seconds |
    /// | 10 | kinds (kit groups) |
    /// | 11, 12, 13 | frame, note and hit counts |
    /// | 14 | floats per hit (22) |
    /// | 15 | reserved |
    /// | 16.. | pitch (one per frame), notes (4 each), hits (22 each) |
    pub fn to_floats(&self, out: &mut Vec<f32>) {
        out.clear();
        out.reserve(self.flat_len());
        out.extend_from_slice(&[
            self.kind.index(),
            self.voiced_fraction,
            self.onsets_per_sec,
            self.repaired as f32,
            self.ref_cents,
            self.root as f32,
            self.scale as f32,
            self.bpm.unwrap_or(f32::NAN),
            self.beats.map(|b| b as f32).unwrap_or(f32::NAN),
            self.loop_sec,
            self.kinds as f32,
            self.pitch.len() as f32,
            self.notes.len() as f32,
            self.hits.len() as f32,
            HIT_LEN as f32,
            0.0,
        ]);
        out.extend_from_slice(&self.pitch);
        for n in &self.notes {
            out.extend_from_slice(&[n.t, n.dur, n.cents, n.hard]);
        }
        for h in &self.hits {
            let s = &h.shape;
            out.extend_from_slice(&[
                h.t,
                h.dur,
                h.slot as f32,
                h.strength,
                s.attack,
                s.burst,
                s.cents.unwrap_or(f32::NAN),
                s.bend,
                s.hold,
                s.vowel,
                s.tone,
                s.end_stop,
                s.end_click,
            ]);
            out.extend_from_slice(&h.features);
            out.extend_from_slice(&h.live);
        }
    }
}

/// Pitch repair plus filling: where the live tier called a frame unvoiced but
/// the pYIN path holds a pitch for at least `FILL_FRAMES` audible frames, take
/// pYIN's pitch. Returns the curve and the number of frames changed.
pub fn repair_pitch(live: &[f32], second: &[f32], audible: impl Fn(usize) -> bool) -> (Vec<f32>, usize) {
    let (mut out, mut changed) = crate::pyin::repair(live, second);
    let n = live.len().min(second.len());
    let mut i = 0;
    while i < n {
        let fill = |k: usize| !live[k].is_finite() && second[k].is_finite() && audible(k);
        if !fill(i) {
            i += 1;
            continue;
        }
        let start = i;
        while i < n && fill(i) {
            i += 1;
        }
        if i - start >= FILL_FRAMES {
            out[start..i].copy_from_slice(&second[start..i]);
            changed += i - start;
        }
    }
    (out, changed)
}

/// Frames more than this far below the take's loudest are not sounding, dB.
pub const SOUND_RANGE_DB: f32 = 50.0;

/// NaN the pitch wherever the frame's audio level is under `min_db`.
pub fn mask_silent(pitch: &mut [f32], level_db: &[f32], min_db: f32) {
    for (p, &l) in pitch.iter_mut().zip(level_db) {
        if l < min_db {
            *p = f32::NAN;
        }
    }
}

/// Shortest stretch of pYIN-only pitch that is taken over the live tier's "unvoiced".
pub const FILL_FRAMES: usize = 6;

fn median(v: &mut [f32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let mid = v.len() / 2;
    *v.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1
}

/// Kind from the measured features. `ons_per_sec`: onsets per second of sounding
/// time; `med_event`: median event length in seconds; `stability`: share of voiced
/// frames within `STABLE_CENTS` of their note's median; `percussive`: share of events
/// that start loud and die away (or are very short).
///
/// Voicing and pitch stability alone cannot tell a beatboxed "boom" from a hummed
/// note, since both are pitched, so beats are called on the envelope: a run of short
/// events that mostly decay. Texture is a take with few events that is mostly
/// unvoiced or has a pitch that never settles, or one with events that neither
/// decay nor hold a pitch. Everything else is melody.
pub fn detect_kind(voiced: f32, ons_per_sec: f32, med_event: f32, stability: f32, percussive: f32) -> Kind {
    use thresh::*;
    let need = if voiced >= PITCHED && stability >= PITCHED { BEAT_PERCUSSIVE_PITCHED } else { BEAT_PERCUSSIVE };
    if ons_per_sec >= BEAT_ONSETS_PER_SEC && med_event <= BEAT_MAX_EVENT && percussive >= need {
        return Kind::Beat;
    }
    if voiced < TEXTURE_MAX_VOICED || stability < TEXTURE_MAX_STABILITY {
        return Kind::Texture;
    }
    Kind::Melody
}

/// Share of hits that are percussive: shorter than `PERC_HOLD`, or 10 dB down from the
/// event peak within `PERC_T10` of the event's length or `PERC_T10_S` seconds.
pub fn percussive_share(hits: &[Hit], lv: &Levels) -> f32 {
    if hits.is_empty() {
        return 0.0;
    }
    let mut count = 0;
    for h in hits {
        let a = ((h.t * FPS).round() as usize).min(lv.n.saturating_sub(1));
        let b = (((h.t + h.dur) * FPS).round() as usize).clamp(a + 1, lv.n);
        let seg = &lv.frame_db[a..b];
        let (pi, peak) = seg.iter().enumerate().fold((0, f32::NEG_INFINITY), |m, (i, &v)| if v > m.1 { (i, v) } else { m });
        let t10 = seg[pi..].iter().position(|&v| v < peak - 10.0).map(|i| (pi + i) as f32 / FPS);
        let perc = h.dur < thresh::PERC_HOLD || t10.is_some_and(|t| t / h.dur < thresh::PERC_T10 || t < thresh::PERC_T10_S);
        count += perc as usize;
    }
    count as f32 / hits.len() as f32
}

/// Run the second pass over a take. `audio` is the take's high-passed mic
/// signal at `sr`; `frames` its live records at 100 per second, `STRIDE` floats each.
pub fn second_pass(audio: &[f32], sr: f32, frames: &[f32], opts: &Options) -> SecondPassResult {
    let n = frames.len() / STRIDE;
    let mut res = SecondPassResult {
        kind: opts.kind.unwrap_or(Kind::Melody),
        voiced_fraction: 0.0,
        onsets_per_sec: 0.0,
        pitch: vec![f32::NAN; n],
        repaired: 0,
        ref_cents: 0.0,
        root: 0,
        scale: 0,
        bpm: None,
        beats: None,
        loop_sec: n as f32 / FPS,
        notes: Vec::new(),
        hits: Vec::new(),
        kinds: 0,
        kind_features: [0.0; 5],
    };
    if n == 0 || !(sr.is_finite() && sr >= 4000.0) || audio.is_empty() {
        return res;
    }
    let rec = |k: usize| &frames[k * STRIDE..(k + 1) * STRIDE];
    let live: Vec<f32> = (0..n).map(|k| rec(k)[PITCH]).collect();
    let lv = Levels::new(audio, sr, n);
    let second = pyin(audio, sr);
    let floor = lv.floor_db + 10.0;
    let (pitch, repaired) = repair_pitch(&live, &second, |k| lv.frame_db[k] > floor);
    res.pitch = pitch;
    res.repaired = repaired;
    // The live tier holds pitch through its gate release (and pYIN can fill into quiet): drop
    // pitch from frames where the audio itself is silent, so notes end where the sound does.
    mask_silent(&mut res.pitch, &lv.frame_db, floor.max(lv.peak_db - SOUND_RANGE_DB));

    let fl = flux(audio, sr, n);
    let onsets = detect_onsets(&lv, &fl);
    let stored_hard: Vec<f32> = (0..n).map(|k| rec(k)[dsp::frame::HARDNESS]).collect();
    let stored_onset: Vec<f32> = (0..n).map(|k| rec(k)[dsp::frame::ONSET]).collect();
    let all_notes = notes::find_notes(&res.pitch, &lv, &onsets, &stored_hard, &stored_onset);
    let hit_in = HitInput { audio, sr, lv: &lv, flux: &fl, onsets: &onsets, pitch: &res.pitch, frames, stride: STRIDE };
    let mut all_hits = measure_hits(&hit_in);

    // Kind features.
    let active_floor = floor.max(lv.peak_db - 50.0);
    let active: Vec<usize> = (0..n).filter(|&k| lv.frame_db[k] > active_floor).collect();
    let voiced_frames = (0..n).filter(|&k| res.pitch[k].is_finite()).count();
    res.voiced_fraction = if active.is_empty() { 0.0 } else { (voiced_frames as f32 / active.len() as f32).min(1.0) };
    let span = match (active.first(), active.last()) {
        (Some(&a), Some(&b)) => ((b + 1 - a) as f32 / FPS).max(1.0),
        _ => 1.0,
    };
    res.onsets_per_sec = onsets.len() as f32 / span;
    let med_event = if all_hits.is_empty() { 0.0 } else { median(&mut all_hits.iter().map(|h| h.dur).collect::<Vec<_>>()) };
    let segs = notes::find_segments(&res.pitch, &lv, &onsets);
    let stable: usize = segs
        .iter()
        .map(|(s, m)| (s.start..s.end).filter(|&k| res.pitch[k].is_finite() && (res.pitch[k] - m).abs() <= thresh::STABLE_CENTS).count())
        .sum();
    let stability = if voiced_frames == 0 { 0.0 } else { stable as f32 / voiced_frames as f32 };
    let percussive = percussive_share(&all_hits, &lv);
    res.kind_features = [res.voiced_fraction, res.onsets_per_sec, med_event, stability, percussive];
    res.kind = opts.kind.unwrap_or_else(|| detect_kind(res.voiced_fraction, res.onsets_per_sec, med_event, stability, percussive));

    match res.kind {
        Kind::Melody => res.notes = all_notes,
        Kind::Beat => {
            res.kinds = group(&mut all_hits, opts.kinds, &opts.taught);
            res.hits = all_hits;
        }
        Kind::Texture => {}
    }

    // Tuning reference and scale from the notes (or the voiced frames when there are none).
    let items: Vec<(f32, f32)> = if !res.notes.is_empty() {
        res.notes.iter().map(|n| (n.cents, n.dur)).collect()
    } else if res.kind != Kind::Texture {
        segs.iter().map(|(s, m)| (*m, (s.end - s.start) as f32 / FPS)).collect()
    } else {
        Vec::new()
    };
    res.ref_cents = notes::tuning_reference(&items);
    if res.kind == Kind::Melody {
        let (root, scale) = notes::detect_scale(&items, res.ref_cents);
        res.root = root;
        res.scale = scale;
    }

    // Tempo from note starts or hit times.
    let times: Vec<f32> = match res.kind {
        Kind::Melody => res.notes.iter().map(|n| n.t).collect(),
        Kind::Beat => res.hits.iter().map(|h| h.t).collect(),
        Kind::Texture => Vec::new(),
    };
    if let Some(t) = find_tempo(&times) {
        let len = n as f32 / FPS;
        let beats = loop_beats(len, t.bpm);
        res.bpm = Some(t.bpm);
        res.beats = Some(beats);
        res.loop_sec = beats as f32 * 60.0 / t.bpm;
    }
    let _ = (CONFIDENCE, LOUDNESS, VOICING);
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_round_trip_and_defaults() {
        let d = Options::from_floats(&[]);
        assert_eq!(d.kind, None);
        assert_eq!(d.kinds, 0);
        assert!(d.taught.iter().all(|t| t.is_none()));
        let mut f = vec![0.0f32; OPTIONS_LEN];
        f[0] = 2.0;
        f[1] = 3.0;
        f[2 + 13] = 1.0; // slot 1
        for d in 0..6 {
            f[2 + 13 + 1 + d] = d as f32;
            f[2 + 13 + 7 + d] = 0.5;
        }
        let o = Options::from_floats(&f);
        assert_eq!(o.kind, Some(Kind::Beat));
        assert_eq!(o.kinds, 3);
        assert!(o.taught[0].is_none() && o.taught[2].is_none());
        let t = o.taught[1].unwrap();
        assert_eq!(t.centre[5], 5.0);
        assert_eq!(t.spread[0], 0.5);
    }

    #[test]
    fn kind_rules() {
        // A hum: voiced, steady, sustained notes.
        assert_eq!(detect_kind(0.95, 1.5, 0.8, 0.95, 0.1), Kind::Melody);
        // Staccato hum: fast but voiced, steady and sustained within each note.
        assert_eq!(detect_kind(0.9, 4.0, 0.2, 0.9, 0.2), Kind::Melody);
        // Beatbox: fast, short, mostly decaying.
        assert_eq!(detect_kind(0.2, 4.0, 0.15, 0.5, 0.9), Kind::Beat);
        // Voiced, steady booms are still beats because they decay.
        assert_eq!(detect_kind(0.9, 1.2, 0.35, 0.95, 0.9), Kind::Beat);
        // Breath noise: no events, unvoiced.
        assert_eq!(detect_kind(0.05, 0.3, 2.0, 0.0, 0.0), Kind::Texture);
        // Siren: voiced but never settles.
        assert_eq!(detect_kind(0.9, 0.4, 2.0, 0.2, 0.0), Kind::Texture);
    }

    #[test]
    fn flat_layout_matches_counts() {
        let r = SecondPassResult {
            kind: Kind::Beat,
            voiced_fraction: 0.5,
            onsets_per_sec: 3.0,
            pitch: vec![1.0, f32::NAN, 3.0],
            repaired: 2,
            ref_cents: -12.0,
            root: 3,
            scale: 2,
            bpm: Some(100.0),
            beats: Some(8),
            loop_sec: 4.8,
            notes: vec![Note { t: 0.1, dur: 0.2, cents: 5000.0, hard: 0.4 }],
            hits: vec![Hit { t: 0.5, slot: 2, features: [1.0, 2.0, 3.0, 4.0, 5.0, 6.0], live: [0.1, 0.2, 0.3], ..Hit::default() }],
            kinds: 3,
            kind_features: [0.0; 5],
        };
        let mut f = Vec::new();
        r.to_floats(&mut f);
        assert_eq!(f.len(), r.flat_len());
        assert_eq!(f.len(), OUT_HEADER + 3 + 4 + HIT_LEN);
        assert_eq!(f[0], 1.0);
        assert_eq!(f[7], 100.0);
        assert_eq!(f[11], 3.0);
        assert_eq!(f[12], 1.0);
        assert_eq!(f[13], 1.0);
        let h0 = OUT_HEADER + 3 + 4;
        assert_eq!(f[h0], 0.5);
        assert_eq!(f[h0 + 2], 2.0);
        assert!(f[h0 + 6].is_nan());
        assert_eq!(&f[h0 + 13..h0 + 19], &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(&f[h0 + 19..h0 + 22], &[0.1, 0.2, 0.3]);
    }

    #[test]
    fn empty_and_bad_input_are_safe() {
        let r = second_pass(&[], 48_000.0, &[], &Options::default());
        assert!(r.notes.is_empty() && r.hits.is_empty() && r.pitch.is_empty());
        let r = second_pass(&[0.0; 4800], 48_000.0, &vec![0.0; STRIDE * 10], &Options::default());
        assert_eq!(r.pitch.len(), 10);
        let r = second_pass(&[0.0; 4800], f32::NAN, &vec![0.0; STRIDE * 10], &Options::default());
        assert!(r.bpm.is_none());
    }

    #[test]
    fn clean_gap_makes_no_phantom_note_and_notes_end_with_the_sound() {
        // Digital silence between notes while the live pitch lingers 120 ms after each note ends.
        let sr = 48_000.0f32;
        let spans = [(0.3f32, 0.5f32, 220.0f32), (0.95, 0.5, 262.0), (1.6, 0.5, 196.0)];
        let mut audio = vec![0.0f32; (2.4 * sr) as usize];
        let n = 240;
        let mut frames = vec![0.0f32; n * STRIDE];
        for k in 0..n {
            frames[k * STRIDE] = f32::NAN;
            frames[k * STRIDE + 7] = 2.0;
        }
        for &(t, d, hz) in &spans {
            for i in (t * sr) as usize..((t + d) * sr) as usize {
                let x = i as f32 / sr - t;
                let env = (x / 0.01).min(1.0).min((d - x) / 0.02);
                audio[i] = 0.2 * env * (std::f32::consts::TAU * hz * i as f32 / sr).sin();
            }
            let c = dsp::hz_to_cents(hz);
            for k in ((t + 0.03) * 100.0) as usize..(((t + d + 0.12) * 100.0) as usize).min(n) {
                frames[k * STRIDE] = c;
                frames[k * STRIDE + 1] = 0.95;
                frames[k * STRIDE + 7] = 0.0;
            }
        }
        let r = second_pass(&audio, sr, &frames, &Options { kind: Some(Kind::Melody), ..Options::default() });
        assert_eq!(r.notes.len(), 3, "{:?}", r.notes);
        for (note, &(t, d, _)) in r.notes.iter().zip(&spans) {
            assert!((note.t - t).abs() < 0.04, "{note:?}");
            assert!((note.t + note.dur - (t + d)).abs() < 0.04, "{note:?}");
        }
    }

    #[test]
    fn pitch_fill_takes_over_audible_unvoiced_runs() {
        let mut live = vec![6000.0f32; 40];
        let second = vec![6000.0f32; 40];
        for v in &mut live[10..20] {
            *v = f32::NAN; // 10 frames unvoiced live, voiced second
        }
        for v in &mut live[30..33] {
            *v = f32::NAN; // 3 frames: too short to fill
        }
        let (out, n) = repair_pitch(&live, &second, |_| true);
        assert!(out[10..20].iter().all(|v| *v == 6000.0));
        assert!(out[30..33].iter().all(|v| v.is_nan()));
        assert_eq!(n, 10);
        let (out, _) = repair_pitch(&live, &second, |k| k >= 15);
        assert!(out[10..15].iter().all(|v| v.is_nan()));
    }
}
