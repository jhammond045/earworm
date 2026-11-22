//! Note tracking shared by both detectors: per-frame pitches become notes.
//! A pitch starts a note after `min_on` frames in a row, ends it after
//! `min_off` frames missing, and restarts it (a re-pluck) when a broadband
//! onset fires and, within the next `retrig_window` frames, its own level
//! (its partials in the raw spectrum) rises by `retrig` over where it was
//! just before the onset. At most
//! `max_notes` notes sound at once; stronger pitches start first.

use crate::hs::Det;

pub struct TrackParams {
    pub min_on: u32,
    pub min_off: u32,
    /// Level ratio that counts as a re-pluck; 0 turns retriggers off.
    pub retrig: f32,
    /// Frames after a broadband onset in which a note may retrigger.
    pub retrig_window: u32,
    /// Strum mode: an onset restarts every sounding note whose level rose at
    /// all (by `strum_rise`), not only the ones that jumped by `retrig`.
    pub strum_rise: f32,
    /// Strength a pitch needs to start a note; any detection keeps one going.
    pub open_strength: f32,
    /// When non-zero, a note may only start within this many frames of a
    /// broadband onset: a string sounds because it was struck.
    pub open_gate: u32,
    pub max_notes: usize,
}

/// A tracked note. Times are the frame times the caller passed in.
#[derive(Clone, Copy, Debug)]
pub struct Est {
    pub midi: u8,
    /// When the tracker decided the note had started (live onset).
    pub on: f64,
    /// The first frame it was seen in (offline onset).
    pub first: f64,
    /// When the tracker decided it had ended (live offset).
    pub off: f64,
    /// The last frame it was seen in (offline offset).
    pub last: f64,
    /// Highest level over its first frames.
    pub level: f32,
    pub cents: f32,
}

#[derive(Clone, Copy, Default)]
struct Slot {
    on: bool,
    idx: usize,
    run: u32,
    miss: u32,
    age: u32,
    first: f64,
    last: f64,
    hist: [f32; 3],
    /// Lowest recent level when the last onset fired; 0 once used.
    pre: f32,
}

pub struct Tracker {
    p: TrackParams,
    slots: [Slot; 128],
    pub notes: Vec<Est>,
    active: usize,
    since_onset: u32,
}

impl Tracker {
    pub fn new(p: TrackParams) -> Self {
        Self { p, slots: [Slot::default(); 128], notes: vec![], active: 0, since_onset: u32::MAX }
    }

    fn open(&mut self, midi: u8, t: f64, first: f64, d: &Det) {
        let s = &mut self.slots[midi as usize];
        s.on = true;
        s.idx = self.notes.len();
        s.miss = 0;
        s.age = 0;
        s.last = t;
        s.hist = [d.level; 3];
        s.pre = 0.0;
        self.notes.push(Est { midi, on: t, first, off: f64::NAN, last: t, level: d.level, cents: d.cents });
        self.active += 1;
    }

    fn close(&mut self, midi: u8, t: f64) {
        let s = &mut self.slots[midi as usize];
        s.on = false;
        s.run = 0;
        let n = &mut self.notes[s.idx];
        n.off = t;
        n.last = s.last;
        self.active -= 1;
    }

    /// One frame. `dets` must be sorted strongest first; `onset` is the
    /// broadband onset detector's verdict for this frame.
    pub fn step(&mut self, t: f64, dets: &[Det], onset: bool) {
        if onset {
            self.since_onset = 0;
            for s in self.slots.iter_mut().filter(|s| s.on) {
                s.pre = s.hist.iter().cloned().fold(f32::INFINITY, f32::min);
            }
        } else {
            self.since_onset = self.since_onset.saturating_add(1);
        }
        let window = self.since_onset < self.p.retrig_window;
        let mut seen = [None::<usize>; 128];
        for (i, d) in dets.iter().enumerate() {
            seen[d.midi as usize] = Some(i);
        }
        for midi in 0..128u8 {
            let s = self.slots[midi as usize];
            match (s.on, seen[midi as usize]) {
                (true, Some(i)) => {
                    let d = dets[i];
                    let ratio = if self.p.strum_rise > 0.0 { self.p.strum_rise } else { self.p.retrig };
                    if self.p.retrig > 0.0 && window && s.pre > 0.0 && d.level > ratio * s.pre {
                        self.close(midi, t);
                        self.open(midi, t, t, &d);
                        continue;
                    }
                    let s = &mut self.slots[midi as usize];
                    s.miss = 0;
                    s.age += 1;
                    s.last = t;
                    s.hist = [s.hist[1], s.hist[2], d.level];
                    if s.age <= 3 {
                        let n = &mut self.notes[s.idx];
                        n.level = n.level.max(d.level);
                    }
                }
                (true, None) => {
                    let s = &mut self.slots[midi as usize];
                    s.miss += 1;
                    s.age += 1;
                    if s.miss >= self.p.min_off {
                        self.close(midi, t);
                    }
                }
                (false, None) => self.slots[midi as usize].run = 0,
                (false, Some(_)) => {}
            }
        }
        // New pitches, strongest first, while there is room.
        for d in dets {
            let s = &mut self.slots[d.midi as usize];
            if s.on {
                continue;
            }
            let gated = self.p.open_gate > 0 && self.since_onset >= self.p.open_gate;
            if d.strength < self.p.open_strength || gated {
                s.run = 0;
                continue;
            }
            if s.run == 0 {
                s.first = t;
            }
            s.run += 1;
            if s.run >= self.p.min_on && self.active < self.p.max_notes {
                let first = s.first;
                self.open(d.midi, t, first, d);
            }
        }
    }

    pub fn finish(&mut self, t: f64) {
        for midi in 0..128u8 {
            if self.slots[midi as usize].on {
                self.close(midi, t);
            }
        }
    }
}
