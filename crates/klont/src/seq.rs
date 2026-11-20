//! Sequencer: the compiled track table and the transport (TDD "Sequencer,
//! transport and tuning"). The engine owns time; the interface only edits
//! data. Nothing here allocates after `new`.
//!
//! # Track blob (`ew_load` kind LOAD_TRACK)
//!
//! 32-bit little-endian words. `u32` words are integers, the rest are f32.
//! Two versions load: v1 (magic "EWT1") and v2 (magic "EWT2", M4), which adds
//! the morph markers below. The layout shown is v2; v1 differs in that header
//! word 6 is 0, clips are 8 words (the first eight below) and there is no
//! morph array.
//!
//! ```text
//! header, 8 words:
//!   u32 magic TRACK_MAGIC or TRACK_MAGIC2, u32 n_sections, u32 n_clips,
//!   u32 n_events, u32 n_warp, u32 flags (bit 0: loop the track),
//!   u32 n_morph (v2; 0 in v1), u32 0
//! sections, n_sections x 8 words:
//!   u32 feel (FEEL_GRID or FEEL_FREE),
//!   f32 length (ticks for grid, 960 per beat; seconds for free),
//!   f32 bpm (grid; ignored for free), u32 repeats (>= 1),
//!   f32 ramp_ms (tempo ramp from the previous section's bpm),
//!   u32 uid (stable id of the section, 0 = none), 2 x u32 0
//! clips, n_clips x 8 words:
//!   u32 section, u32 layer, f32 loop_len (section units, > 0),
//!   u32 first_event, u32 n_events (a run in the events array, sorted by t),
//!   i32 curve (curve slot, -1 for none), u32 first_warp, u32 n_warp,
//!   (v2, 4 more words:) u32 first_morph, u32 n_morph (a run in the morph
//!   array, sorted by t), 2 x u32 0
//! events, n_events x 16 words (one record for notes and hits):
//!   0 t, 1 dur (section units), 2 cents (NaN: kit slot pitch), 3 hard (0..1),
//!   4 u32 slot (kit slot for hits), 5 strength (0..1, velocity),
//!   6 attack, 7 burst, 8 bend, 9 hold, 10 vowel, 11 tone, 12 end_stop,
//!   13 end_click (hit shape, TDD HitShape; NaN or 0 when unused), 14 0, 15 0
//! warp anchors, n_warp x 2 words:
//!   f32 take_sec, f32 clip_t (section units), sorted by clip_t
//! morph markers (v2), n_morph x 4 words:
//!   f32 t (section units), u32 state index (< MAX_STATES), f32 len_ms (>= 0),
//!   u32 shape (MORPH_LINE, _EASE, _LATE, _STEP, _WOBBLE)
//! ```
//!
//! A clip plays in its section on its layer. At most one clip per (section,
//! layer); a later duplicate is ignored. Events with t >= loop_len never play.
//!
//! # Morph lane (resolution step 1)
//!
//! A layer with states loaded (`LOAD_STATES`) blends two of them per control
//! tick, chosen by the morph markers of its clip in the current section; see
//! `Track::morph_blend`. At the clip position p (the section position modulo
//! the clip's loop_len), marker k is the newest with t <= p, or the last
//! marker when none is yet (the lane wraps with the loop). The blend goes from
//! the state of the marker before k to the state of k with progress
//! `x = clamp(ms since t_k / len_ms, 0, 1)`, then shaped: line x, ease
//! smoothstep, late x^3, step 1 at once, wobble `x + 0.12 sin(6 pi x) (1 - x)`
//! clamped to 0..1. Milliseconds come from the transport's rate at that tick
//! (a tempo ramp inside one morph is not integrated). One marker means that
//! state throughout. Markers with t >= loop_len never apply.
//!
//! # Transport rules
//!
//! - Position is f64 in section units. Grid: each sample advances it by
//!   bpm * 960 / (60 * sample_rate). Free: by 1 / sample_rate.
//! - Entering a section whose bpm differs from the previous section's, bpm
//!   ramps linearly from the old value to the new one over ramp_ms (0: jump),
//!   integrated per sample (or exactly per span).
//! - At section end: take a queued jump if any; else repeat until `repeats`
//!   passes are done; else go to the next section; after the last section,
//!   go to section 0 if looping, otherwise stop (`ended()` becomes true).
//!   Jumps land only on a section end.
//! - Each layer reads its clip at the section position modulo its clip's
//!   loop_len (uneven loops). An event fires when that position crosses its
//!   t, including at a wrap and at a section start (t = 0 fires on entry).
//! - Sample accuracy: every event carries its sample offset inside the span
//!   passed to `advance`, so the engine can split the block there.
//! - Notes end by duration: `dur_samples` is the event's dur converted at the
//!   rate when it fires. Notes are not cut at loop or section boundaries.

use crate::protocol::{MAX_CURVES, MAX_LAYERS, MAX_STATES};

/// Ticks per beat.
pub const PPQ: f64 = 960.0;
pub const TRACK_MAGIC: u32 = 0x3154_5745; // "EWT1"
pub const TRACK_MAGIC2: u32 = 0x3254_5745; // "EWT2"
pub const MORPH_LINE: u32 = 0;
pub const MORPH_EASE: u32 = 1;
pub const MORPH_LATE: u32 = 2;
pub const MORPH_STEP: u32 = 3;
pub const MORPH_WOBBLE: u32 = 4;
pub const FEEL_GRID: u32 = 0;
pub const FEEL_FREE: u32 = 1;

pub const MAX_SECTIONS: usize = 64;
pub const MAX_CLIPS: usize = 512;
pub const MAX_EVENTS: usize = 16384;
pub const MAX_WARP: usize = 16384;
pub const MAX_MORPH: usize = 4096;
/// Most events `advance` reports for one span; later ones are dropped and
/// counted in `EventList::dropped`.
pub const MAX_SPAN_EVENTS: usize = 256;

pub const EVENT_WORDS: usize = 16;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Section {
    pub feel: u32,
    pub length: f64,
    pub bpm: f64,
    pub repeats: u32,
    pub ramp_ms: f64,
    /// Stable id across track reloads, so the transport stays on the same
    /// section when sections before it are added, removed or moved. 0 = none.
    pub uid: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Clip {
    pub section: u32,
    pub layer: u32,
    pub loop_len: f64,
    pub first_event: u32,
    pub n_events: u32,
    pub curve: i32,
    pub first_warp: u32,
    pub n_warp: u32,
    /// Run in the morph array (always empty for a v1 blob).
    pub first_morph: u32,
    pub n_morph: u32,
}

/// One morph marker, as in the blob.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Morph {
    pub t: f64,
    pub state: u32,
    pub len_ms: f64,
    pub shape: u32,
}

/// Which two states a layer blends and how far: `x` is already shaped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MorphBlend {
    pub from: u32,
    pub to: u32,
    pub x: f32,
}

/// One note or hit, as in the blob.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Event {
    pub t: f64,
    pub dur: f64,
    pub cents: f32,
    pub hard: f32,
    pub slot: u32,
    pub strength: f32,
    pub attack: f32,
    pub burst: f32,
    pub bend: f32,
    pub hold: f32,
    pub vowel: f32,
    pub tone: f32,
    pub end_stop: f32,
    pub end_click: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WarpAnchor {
    pub take_sec: f64,
    pub clip_t: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadError {
    BadMagic,
    TooBig,
    Malformed,
}

const NO_CLIP: u32 = u32::MAX;
const HEADER_WORDS: usize = 8;
const SECTION_WORDS: usize = 8;
const CLIP_WORDS: usize = 8;
const CLIP_WORDS2: usize = 12;
const WARP_WORDS: usize = 2;
const MORPH_WORDS: usize = 4;

/// Tolerance, in section units, when comparing a position with an event time
/// or a section end. Positions are computed in closed form from an integer
/// sample count, so the only error is f64 rounding (about 1e-12 at the sizes
/// in play); 1e-6 of a tick (or of a second) is far above that and far below
/// anything audible.
const EPS: f64 = 1e-6;

fn word(b: &[u8], i: usize) -> Result<u32, LoadError> {
    let s = i.checked_mul(4).ok_or(LoadError::Malformed)?;
    let c = b.get(s..s + 4).ok_or(LoadError::Malformed)?;
    Ok(u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
}

fn fword(b: &[u8], i: usize) -> Result<f32, LoadError> {
    word(b, i).map(f32::from_bits)
}

fn read_section(b: &[u8], i: usize) -> Result<Section, LoadError> {
    let o = HEADER_WORDS + i * SECTION_WORDS;
    let feel = word(b, o)?;
    let length = fword(b, o + 1)? as f64;
    let bpm = fword(b, o + 2)? as f64;
    let repeats = word(b, o + 3)?;
    let ramp_ms = fword(b, o + 4)? as f64;
    let uid = word(b, o + 5)?;
    if feel != FEEL_GRID && feel != FEEL_FREE {
        return Err(LoadError::Malformed);
    }
    if !(length.is_finite() && length > 0.0) || repeats < 1 {
        return Err(LoadError::Malformed);
    }
    if !(ramp_ms.is_finite() && ramp_ms >= 0.0) {
        return Err(LoadError::Malformed);
    }
    if feel == FEEL_GRID && !(bpm.is_finite() && bpm > 0.0) {
        return Err(LoadError::Malformed);
    }
    let bpm = if feel == FEEL_GRID { bpm } else { 0.0 };
    Ok(Section {
        feel,
        length,
        bpm,
        repeats,
        ramp_ms,
        uid,
    })
}

/// Sizes of the pools a blob declares, and its version.
#[derive(Clone, Copy)]
struct Dims {
    sections: usize,
    clips: usize,
    events: usize,
    warp: usize,
    morph: usize,
    v2: bool,
}

impl Dims {
    fn clip_words(&self) -> usize {
        if self.v2 { CLIP_WORDS2 } else { CLIP_WORDS }
    }
    fn ev_base(&self) -> usize {
        HEADER_WORDS + self.sections * SECTION_WORDS + self.clips * self.clip_words()
    }
    fn warp_base(&self) -> usize {
        self.ev_base() + self.events * EVENT_WORDS
    }
    fn morph_base(&self) -> usize {
        self.warp_base() + self.warp * WARP_WORDS
    }
}

fn read_clip(b: &[u8], d: &Dims, i: usize) -> Result<Clip, LoadError> {
    let (n_sections, n_events, n_warp) = (d.sections, d.events, d.warp);
    let o = HEADER_WORDS + n_sections * SECTION_WORDS + i * d.clip_words();
    let (first_morph, n_morph) = if d.v2 {
        (word(b, o + 8)?, word(b, o + 9)?)
    } else {
        (0, 0)
    };
    let clip = Clip {
        first_morph,
        n_morph,
        section: word(b, o)?,
        layer: word(b, o + 1)?,
        loop_len: fword(b, o + 2)? as f64,
        first_event: word(b, o + 3)?,
        n_events: word(b, o + 4)?,
        curve: word(b, o + 5)? as i32,
        first_warp: word(b, o + 6)?,
        n_warp: word(b, o + 7)?,
    };
    let in_run = |first: u32, n: u32, total: usize| {
        (first as usize)
            .checked_add(n as usize)
            .is_some_and(|e| e <= total)
    };
    if clip.section as usize >= n_sections
        || clip.layer as usize >= MAX_LAYERS
        || !(clip.loop_len.is_finite() && clip.loop_len > 0.0)
        || clip.curve < -1
        || clip.curve >= MAX_CURVES as i32
        || !in_run(clip.first_event, clip.n_events, n_events)
        || !in_run(clip.first_warp, clip.n_warp, n_warp)
        || !in_run(clip.first_morph, clip.n_morph, d.morph)
    {
        return Err(LoadError::Malformed);
    }
    Ok(clip)
}

fn read_event(b: &[u8], base: usize, i: usize) -> Result<Event, LoadError> {
    let o = base + i * EVENT_WORDS;
    let f = |k: usize| fword(b, o + k);
    let e = Event {
        t: f(0)? as f64,
        dur: f(1)? as f64,
        cents: f(2)?,
        hard: f(3)?,
        slot: word(b, o + 4)?,
        strength: f(5)?,
        attack: f(6)?,
        burst: f(7)?,
        bend: f(8)?,
        hold: f(9)?,
        vowel: f(10)?,
        tone: f(11)?,
        end_stop: f(12)?,
        end_click: f(13)?,
    };
    if !(e.t.is_finite() && e.t >= 0.0 && e.dur.is_finite() && e.dur >= 0.0) {
        return Err(LoadError::Malformed);
    }
    Ok(e)
}

fn read_warp(b: &[u8], base: usize, i: usize) -> Result<WarpAnchor, LoadError> {
    let o = base + i * WARP_WORDS;
    let a = WarpAnchor {
        take_sec: fword(b, o)? as f64,
        clip_t: fword(b, o + 1)? as f64,
    };
    if !(a.take_sec.is_finite() && a.clip_t.is_finite()) {
        return Err(LoadError::Malformed);
    }
    Ok(a)
}

fn read_morph(b: &[u8], base: usize, i: usize) -> Result<Morph, LoadError> {
    let o = base + i * MORPH_WORDS;
    let m = Morph {
        t: fword(b, o)? as f64,
        state: word(b, o + 1)?,
        len_ms: fword(b, o + 2)? as f64,
        shape: word(b, o + 3)?,
    };
    if !(m.t.is_finite() && m.t >= 0.0 && m.len_ms.is_finite() && m.len_ms >= 0.0)
        || m.state as usize >= MAX_STATES
        || m.shape > MORPH_WOBBLE
    {
        return Err(LoadError::Malformed);
    }
    Ok(m)
}

/// The compiled arrangement: fixed-capacity pools filled by `load`.
pub struct Track {
    sections: Vec<Section>,
    clips: Vec<Clip>,
    events: Vec<Event>,
    warp: Vec<WarpAnchor>,
    morph: Vec<Morph>,
    looping: bool,
    /// Clip index per (section, layer), NO_CLIP when none.
    lut: Vec<u32>,
}

impl Default for Track {
    fn default() -> Self {
        Self::new()
    }
}

impl Track {
    /// Allocates every pool at its maximum size. An empty track has no sections.
    pub fn new() -> Self {
        Self {
            sections: Vec::with_capacity(MAX_SECTIONS),
            clips: Vec::with_capacity(MAX_CLIPS),
            events: Vec::with_capacity(MAX_EVENTS),
            warp: Vec::with_capacity(MAX_WARP),
            morph: Vec::with_capacity(MAX_MORPH),
            looping: false,
            lut: vec![NO_CLIP; MAX_SECTIONS * MAX_LAYERS],
        }
    }

    /// Parse a blob (v1 or v2) into the pools. On error the previous contents
    /// are kept. Validates counts against the MAX_* limits and every index
    /// (section, layer < protocol::MAX_LAYERS, event, warp and morph runs in
    /// range, lengths and loop_len finite and > 0, repeats >= 1, morph state
    /// < MAX_STATES, shape known). Events, warp anchors and morph markers inside
    /// a clip must be sorted; the blob must be exactly the size its header
    /// implies.
    pub fn load(&mut self, bytes: &[u8]) -> Result<(), LoadError> {
        if bytes.len() < HEADER_WORDS * 4 {
            return Err(LoadError::Malformed);
        }
        let magic = word(bytes, 0)?;
        if magic != TRACK_MAGIC && magic != TRACK_MAGIC2 {
            return Err(LoadError::BadMagic);
        }
        let v2 = magic == TRACK_MAGIC2;
        let d = Dims {
            sections: word(bytes, 1)? as usize,
            clips: word(bytes, 2)? as usize,
            events: word(bytes, 3)? as usize,
            warp: word(bytes, 4)? as usize,
            morph: if v2 { word(bytes, 6)? as usize } else { 0 },
            v2,
        };
        let flags = word(bytes, 5)?;
        if d.sections > MAX_SECTIONS
            || d.clips > MAX_CLIPS
            || d.events > MAX_EVENTS
            || d.warp > MAX_WARP
            || d.morph > MAX_MORPH
        {
            return Err(LoadError::TooBig);
        }
        let words = d.morph_base() + d.morph * MORPH_WORDS;
        if bytes.len() != words * 4 {
            return Err(LoadError::Malformed);
        }
        let (ev_base, warp_base, morph_base) = (d.ev_base(), d.warp_base(), d.morph_base());

        // Check pass: nothing is modified.
        for i in 0..d.sections {
            read_section(bytes, i)?;
        }
        for i in 0..d.events {
            read_event(bytes, ev_base, i)?;
        }
        for i in 0..d.warp {
            read_warp(bytes, warp_base, i)?;
        }
        for i in 0..d.morph {
            read_morph(bytes, morph_base, i)?;
        }
        for i in 0..d.clips {
            let c = read_clip(bytes, &d, i)?;
            let mut last = 0.0;
            for k in 0..c.n_events as usize {
                let e = read_event(bytes, ev_base, c.first_event as usize + k)?;
                if e.t < last {
                    return Err(LoadError::Malformed);
                }
                last = e.t;
            }
            let mut last = f64::NEG_INFINITY;
            for k in 0..c.n_warp as usize {
                let a = read_warp(bytes, warp_base, c.first_warp as usize + k)?;
                if a.clip_t < last {
                    return Err(LoadError::Malformed);
                }
                last = a.clip_t;
            }
            let mut last = 0.0;
            for k in 0..c.n_morph as usize {
                let m = read_morph(bytes, morph_base, c.first_morph as usize + k)?;
                if m.t < last {
                    return Err(LoadError::Malformed);
                }
                last = m.t;
            }
        }

        // Copy pass: cannot fail now, and never grows past capacity.
        self.sections.clear();
        self.clips.clear();
        self.events.clear();
        self.warp.clear();
        self.morph.clear();
        self.lut.fill(NO_CLIP);
        self.looping = flags & 1 != 0;
        for i in 0..d.sections {
            if let Ok(s) = read_section(bytes, i) {
                self.sections.push(s);
            }
        }
        for i in 0..d.clips {
            if let Ok(c) = read_clip(bytes, &d, i) {
                let slot = c.section as usize * MAX_LAYERS + c.layer as usize;
                if self.lut[slot] == NO_CLIP {
                    self.lut[slot] = i as u32;
                }
                self.clips.push(c);
            }
        }
        for i in 0..d.events {
            if let Ok(e) = read_event(bytes, ev_base, i) {
                self.events.push(e);
            }
        }
        for i in 0..d.warp {
            if let Ok(a) = read_warp(bytes, warp_base, i) {
                self.warp.push(a);
            }
        }
        for i in 0..d.morph {
            if let Ok(m) = read_morph(bytes, morph_base, i) {
                self.morph.push(m);
            }
        }
        Ok(())
    }

    /// The morph markers of a clip, sorted by t.
    pub fn clip_morph(&self, clip: &Clip) -> &[Morph] {
        let a = clip.first_morph as usize;
        &self.morph[a..a + clip.n_morph as usize]
    }

    /// Step 1 of resolution for the layer playing clip `clip_i`, at section
    /// position `pos`. `ms_per_unit` converts section units to milliseconds at
    /// the current rate (0 when the transport has no rate: every morph then
    /// completes at once). None when the clip has no marker that applies.
    pub fn morph_blend(&self, clip_i: usize, pos: f64, ms_per_unit: f64) -> Option<MorphBlend> {
        let clip = self.clips.get(clip_i)?;
        let all = self.clip_morph(clip);
        let len = clip.loop_len;
        // Markers at or past the loop end never apply (sorted: a prefix).
        let m = &all[..all.partition_point(|m| m.t < len)];
        let n = m.len();
        if n == 0 {
            return None;
        }
        let w = ((pos + EPS) / len).floor().max(0.0);
        let p = (pos - w * len).max(0.0);
        let (k, since) = match m.iter().rposition(|mk| mk.t <= p + EPS) {
            Some(k) => (k, (p - m[k].t).max(0.0)),
            None => (n - 1, p + len - m[n - 1].t),
        };
        let prev = (k + n - 1) % n;
        let mk = m[k];
        let ms = since * ms_per_unit;
        let raw = if mk.shape == MORPH_STEP || mk.len_ms <= 0.0 || !(ms_per_unit > 0.0) {
            1.0
        } else {
            (ms / mk.len_ms).clamp(0.0, 1.0)
        };
        let x = match mk.shape {
            MORPH_EASE => raw * raw * (3.0 - 2.0 * raw),
            MORPH_LATE => raw * raw * raw,
            MORPH_WOBBLE => (raw + 0.12 * (6.0 * std::f64::consts::PI * raw).sin() * (1.0 - raw))
                .clamp(0.0, 1.0),
            _ => raw,
        };
        Some(MorphBlend {
            from: m[prev].state,
            to: mk.state,
            x: x as f32,
        })
    }
    pub fn n_sections(&self) -> usize {
        self.sections.len()
    }
    pub fn n_clips(&self) -> usize {
        self.clips.len()
    }
    pub fn n_events(&self) -> usize {
        self.events.len()
    }
    pub fn section(&self, i: usize) -> &Section {
        &self.sections[i]
    }
    pub fn looping(&self) -> bool {
        self.looping
    }
    /// The clip playing on `layer` in `section`, if any.
    pub fn clip_for(&self, section: usize, layer: usize) -> Option<&Clip> {
        self.clip_index_for(section, layer).map(|i| &self.clips[i])
    }
    /// Index of the clip playing on `layer` in `section`, if any.
    pub fn clip_index_for(&self, section: usize, layer: usize) -> Option<usize> {
        if section >= self.sections.len() || layer >= MAX_LAYERS {
            return None;
        }
        match self.lut[section * MAX_LAYERS + layer] {
            NO_CLIP => None,
            i => Some(i as usize),
        }
    }
    pub fn clip(&self, i: usize) -> &Clip {
        &self.clips[i]
    }
    pub fn event(&self, i: usize) -> &Event {
        &self.events[i]
    }
    pub fn warp(&self, clip: &Clip) -> &[WarpAnchor] {
        let a = clip.first_warp as usize;
        &self.warp[a..a + clip.n_warp as usize]
    }
    /// The events of a clip, sorted by t.
    pub fn clip_events(&self, clip: &Clip) -> &[Event] {
        let a = clip.first_event as usize;
        &self.events[a..a + clip.n_events as usize]
    }
}

/// One event the transport fired during a span.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SeqEvent {
    /// Sample offset inside the span passed to `advance`.
    pub offset: u32,
    pub layer: u32,
    /// Index into the track's clips.
    pub clip: u32,
    /// Index into the track's events (global, not relative to the clip).
    pub event: u32,
    /// Event duration converted to samples at the rate when it fired (>= 1).
    pub dur_samples: u32,
}

/// Fixed-capacity list of events, sorted by offset (ties: layer order, then
/// event order).
pub struct EventList {
    pub items: [SeqEvent; MAX_SPAN_EVENTS],
    pub len: usize,
    pub dropped: u32,
}

impl EventList {
    pub fn new() -> Self {
        Self {
            items: [SeqEvent::default(); MAX_SPAN_EVENTS],
            len: 0,
            dropped: 0,
        }
    }
    pub fn clear(&mut self) {
        self.len = 0;
    }
    pub fn as_slice(&self) -> &[SeqEvent] {
        &self.items[..self.len]
    }
}

impl Default for EventList {
    fn default() -> Self {
        Self::new()
    }
}

/// The transport.
///
/// # Exact positions
///
/// Time inside a section is a count of samples `j` since the start of the
/// current segment (a pass of a section, or the rest of a pass after a seek),
/// plus the position `p0` the segment started at. The position of sample `j`
/// is a closed form of `j`, never a running sum, so it does not drift:
///
/// - grid, constant tempo: `p0 + j * bpm * 960 / (60 * sample_rate)`
/// - free: `p0 + j / sample_rate`
/// - grid, inside a ramp of `R` samples from `a` to `b` bpm:
///   `p0 + (a*j + (b-a)*j*j/(2R)) * 960 / (60 * sample_rate)`, the exact
///   integral of the linear tempo; after the ramp the tempo is `b`.
///
/// "The position of sample j" is the position before that sample advances.
/// An event at time t in a layer fires at the first sample whose position p
/// satisfies `p >= t - 1e-6` (the layer position, i.e. modulo the clip's
/// loop length). So at 120 bpm and 48 kHz (0.04 ticks per sample), an event at
/// t = 960 fires at sample 24000 counted from the start of play at 0, and an
/// event at t = 0 fires at sample 0. How the samples are split into `advance`
/// calls makes no difference. A section ends at the first sample whose
/// position is `>= length - 1e-6`; that sample is the first of the next pass,
/// whose start position is the small overshoot past the length (so the
/// fraction of a sample is carried across repeats).
pub struct Transport {
    sr: f64,
    playing: bool,
    ended: bool,
    /// Cursors match the current section and position.
    entered: bool,
    section: usize,
    repeat: u32,
    jump: Option<usize>,
    // Segment.
    p0: f64,
    j: u64,
    free: bool,
    /// Tempo at the segment start, tempo after the ramp, ramp length in
    /// samples (0: no ramp).
    from: f64,
    to: f64,
    ramp: f64,
    // Per-layer clip and read cursor.
    clip: [u32; MAX_LAYERS],
    idx: [u32; MAX_LAYERS],
    wraps: [f64; MAX_LAYERS],
    /// Beat crossings in the last `advance` (offsets from the start of that call).
    pub beat_off: [u32; 8],
    pub beat_index: [i32; 8],
    pub n_beats: usize,
    /// Beat index of the previous sample, so a crossing clicks once. -1 arms
    /// a click on the next sample that sits on a beat (the start of play).
    prev_beat: i64,
}

impl Transport {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sr: sample_rate as f64,
            playing: false,
            ended: false,
            entered: false,
            section: 0,
            repeat: 0,
            jump: None,
            p0: 0.0,
            j: 0,
            free: false,
            from: 0.0,
            to: 0.0,
            ramp: 0.0,
            clip: [NO_CLIP; MAX_LAYERS],
            idx: [0; MAX_LAYERS],
            wraps: [0.0; MAX_LAYERS],
            beat_off: [0; 8],
            beat_index: [0; 8],
            n_beats: 0,
            prev_beat: -1,
        }
    }

    /// Start from the current position. No effect on an empty track (the
    /// transport stops itself at the first `advance`). After the track ended,
    /// starts again from section 0.
    pub fn play(&mut self) {
        if self.ended {
            self.ended = false;
            self.section = 0;
            self.repeat = 0;
            self.jump = None;
            self.p0 = 0.0;
            self.j = 0;
            self.ramp = 0.0;
            self.entered = false;
        }
        self.playing = true;
    }
    pub fn stop(&mut self) {
        self.playing = false;
    }
    pub fn playing(&self) -> bool {
        self.playing
    }

    /// Set the segment to the start of `section` data at position `p0`, flat tempo.
    fn set_segment(&mut self, track: &Track, p0: f64) {
        let s = track.section(self.section);
        self.free = s.feel == FEEL_FREE;
        self.p0 = p0;
        self.j = 0;
        self.to = s.bpm;
        self.from = s.bpm;
        self.ramp = 0.0;
    }

    /// Fill the per-layer clips and cursors for the current section. At a
    /// pass start (`from_start`) every event up to the position fires; after a
    /// seek, events before the position are skipped.
    fn set_cursors(&mut self, track: &Track, from_start: bool) {
        let p = self.p0;
        for l in 0..MAX_LAYERS {
            self.idx[l] = 0;
            self.wraps[l] = 0.0;
            match track.clip_index_for(self.section, l) {
                None => self.clip[l] = NO_CLIP,
                Some(c) => {
                    self.clip[l] = c as u32;
                    if !from_start {
                        let clip = track.clip(c);
                        let w = ((p + EPS) / clip.loop_len).floor().max(0.0);
                        let lp = p - w * clip.loop_len;
                        self.wraps[l] = w;
                        let ev = track.clip_events(clip);
                        self.idx[l] =
                            ev.iter().position(|e| e.t >= lp - EPS).unwrap_or(ev.len()) as u32;
                    }
                }
            }
        }
        self.entered = true;
    }

    /// Move to `pos` (section units) in `section`, first pass, clearing any
    /// queued jump. Events exactly at `pos` fire on the next advance.
    pub fn seek(&mut self, track: &Track, section: usize, pos: f64) {
        if track.n_sections() == 0 {
            return;
        }
        self.section = section.min(track.n_sections() - 1);
        self.repeat = 0;
        self.jump = None;
        self.ended = false;
        let len = track.section(self.section).length;
        let p = if pos.is_finite() {
            pos.clamp(0.0, len)
        } else {
            0.0
        };
        self.set_segment(track, p);
        self.set_cursors(track, false);
        // Landing on a beat (including the start) clicks on the next sample.
        self.arm_beat(true);
    }
    pub fn queued_jump(&self) -> Option<usize> {
        self.jump
    }
    pub fn queue_jump(&mut self, section: usize) {
        self.jump = Some(section);
    }
    /// Before `track_changed`: point the current section and the queued jump
    /// at the sections with the same uids in the new track, so adding,
    /// removing or moving sections before the playhead keeps the same music
    /// playing. A uid of 0, or one the new track lacks, leaves the index as is.
    pub fn follow_uids(&mut self, track: &Track, cur_uid: u32, jump_uid: u32) {
        let find = |uid: u32| {
            (uid != 0)
                .then(|| (0..track.n_sections()).find(|&i| track.section(i).uid == uid))
                .flatten()
        };
        if let Some(i) = find(cur_uid) {
            self.section = i;
        }
        if self.jump.is_some() {
            if let Some(i) = find(jump_uid) {
                self.jump = Some(i);
            }
        }
    }

    /// A new track was loaded: keep playing, clamping the section, repeat
    /// and position into it. The tempo takes the new section's value (no
    /// ramp), and events at or after the position still fire.
    pub fn track_changed(&mut self, track: &Track) {
        let n = track.n_sections();
        if n == 0 {
            self.entered = false;
            return;
        }
        let was = if self.entered { self.pos() } else { self.p0 };
        self.section = self.section.min(n - 1);
        let s = *track.section(self.section);
        self.repeat = self.repeat.min(s.repeats - 1);
        if self.jump.is_some_and(|j| j >= n) {
            self.jump = None;
        }
        self.set_segment(track, was.clamp(0.0, s.length));
        self.set_cursors(track, false);
        // While playing, hold the current beat so a reload does not click
        // again. While stopped, a beat under the playhead (including the
        // start) clicks when play begins.
        self.arm_beat(!self.playing);
    }

    fn bpm_at(&self, j: f64) -> f64 {
        if self.free {
            0.0
        } else if self.ramp > 0.0 && j < self.ramp {
            self.from + (self.to - self.from) * j / self.ramp
        } else {
            self.to
        }
    }

    fn pos_at(&self, j: u64) -> f64 {
        let j = j as f64;
        if self.free {
            return self.p0 + j / self.sr;
        }
        let units = if self.ramp > 0.0 {
            let d = self.to - self.from;
            if j < self.ramp {
                self.from * j + d * j * j / (2.0 * self.ramp)
            } else {
                self.from * self.ramp + d * self.ramp / 2.0 + self.to * (j - self.ramp)
            }
        } else {
            self.to * j
        };
        self.p0 + units * PPQ / (60.0 * self.sr)
    }

    fn rate_at(&self, j: u64) -> f64 {
        if self.free {
            1.0 / self.sr
        } else {
            self.bpm_at(j as f64) * PPQ / (60.0 * self.sr)
        }
    }

    /// Handle the end of a pass at position `pos` (>= length). Returns false
    /// when the track ended.
    fn boundary(&mut self, track: &Track, pos: f64) -> bool {
        let n = track.n_sections();
        let s = *track.section(self.section);
        let over = (pos - s.length).max(0.0).min(s.length * 0.5);
        let cur_bpm = self.bpm_at(self.j as f64);

        let mut target = None;
        if let Some(j) = self.jump.take() {
            if j < n {
                target = Some(j);
            }
        }
        if target.is_none() {
            if self.repeat + 1 < s.repeats {
                // Next pass of the same section: a running ramp continues.
                self.repeat += 1;
                let j = self.j as f64;
                if self.ramp > 0.0 && j < self.ramp {
                    self.from = cur_bpm;
                    self.ramp -= j;
                } else {
                    self.ramp = 0.0;
                    self.from = self.to;
                }
                self.p0 = over;
                self.j = 0;
                self.set_cursors(track, true);
                return true;
            }
            let next = self.section + 1;
            if next < n {
                target = Some(next);
            } else if track.looping() {
                target = Some(0);
            } else {
                self.ended = true;
                self.playing = false;
                self.p0 = s.length;
                self.j = 0;
                self.ramp = 0.0;
                return false;
            }
        }
        let target = target.unwrap_or(0);
        self.section = target;
        self.repeat = 0;
        self.set_segment(track, over);
        let t = track.section(target);
        if !self.free && cur_bpm > 0.0 && t.ramp_ms > 0.0 && (cur_bpm - t.bpm).abs() > 1e-12 {
            self.from = cur_bpm;
            self.ramp = t.ramp_ms * self.sr / 1000.0;
        }
        self.set_cursors(track, true);
        true
    }

    /// Grid position is on a beat boundary when `edge` is set, so the next
    /// sample clicks; otherwise the current beat is already accounted for.
    fn arm_beat(&mut self, edge: bool) {
        if self.free {
            self.prev_beat = -1;
            return;
        }
        let q = (self.pos_at(self.j) + EPS) / PPQ;
        let beat = q.floor() as i64;
        let on = (q - q.floor()) * PPQ < 1e-3;
        self.prev_beat = if edge && on { beat - 1 } else { beat };
    }

    pub fn is_free(&self) -> bool {
        self.free
    }

    /// Advance `n` samples while playing, appending fired events to `out`
    /// (which the caller clears). Does nothing when stopped. Events beyond
    /// MAX_SPAN_EVENTS are dropped and counted in `out.dropped`.
    /// Beat crossings in this call are left in `beat_off` / `beat_index`.
    pub fn advance(&mut self, track: &Track, n: usize, out: &mut EventList) {
        self.n_beats = 0;
        if !self.playing {
            return;
        }
        if track.n_sections() == 0 {
            self.playing = false;
            return;
        }
        if !self.entered {
            // First start, or the track changed while stopped.
            self.section = self.section.min(track.n_sections() - 1);
            let p = self.p0.clamp(0.0, track.section(self.section).length);
            self.set_segment(track, p);
            self.set_cursors(track, false);
        }
        for i in 0..n {
            let mut pos = self.pos_at(self.j);
            let mut guard = 0;
            while pos >= track.section(self.section).length - EPS {
                if !self.boundary(track, pos) {
                    return;
                }
                pos = self.pos_at(self.j);
                guard += 1;
                if guard > 4 {
                    // Degenerate section shorter than a sample: restart it.
                    self.p0 = 0.0;
                    self.j = 0;
                    pos = 0.0;
                    break;
                }
            }
            if !self.free {
                let beat = ((pos + EPS) / PPQ).floor() as i64;
                if beat != self.prev_beat {
                    if self.n_beats < 8 {
                        self.beat_off[self.n_beats] = i as u32;
                        self.beat_index[self.n_beats] = beat as i32;
                        self.n_beats += 1;
                    }
                    self.prev_beat = beat;
                }
            }
            for l in 0..MAX_LAYERS {
                let c = self.clip[l];
                if c == NO_CLIP {
                    continue;
                }
                let clip = track.clip(c as usize);
                let len = clip.loop_len;
                let w = ((pos + EPS) / len).floor();
                if w > self.wraps[l] {
                    self.wraps[l] = w;
                    self.idx[l] = 0;
                }
                let lp = pos - self.wraps[l] * len;
                let run = clip.n_events;
                while self.idx[l] < run {
                    let gi = clip.first_event + self.idx[l];
                    let e = track.event(gi as usize);
                    if e.t >= len || e.t > lp + EPS {
                        break;
                    }
                    self.idx[l] += 1;
                    if out.len < MAX_SPAN_EVENTS {
                        let ups = self.rate_at(self.j);
                        let dur = (e.dur / ups).round().max(1.0);
                        out.items[out.len] = SeqEvent {
                            offset: i as u32,
                            layer: l as u32,
                            clip: c,
                            event: gi,
                            dur_samples: dur as u32,
                        };
                        out.len += 1;
                    } else {
                        out.dropped += 1;
                    }
                }
            }
            self.j += 1;
        }
    }
    pub fn section(&self) -> usize {
        self.section
    }
    /// Position in section units, before the next sample advances.
    pub fn pos(&self) -> f64 {
        self.pos_at(self.j)
    }
    /// Pass number inside the current section, 0-based.
    pub fn repeat(&self) -> u32 {
        self.repeat
    }
    /// Current bpm (after any ramp); 0 in a free section.
    pub fn bpm(&self) -> f64 {
        self.bpm_at(self.j as f64)
    }
    /// Section units per sample right now (ticks or seconds).
    pub fn units_per_sample(&self) -> f64 {
        self.rate_at(self.j)
    }
    /// True when a non-looping track ran past its last section.
    pub fn ended(&self) -> bool {
        self.ended
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48000.0;

    struct S {
        feel: u32,
        length: f32,
        bpm: f32,
        repeats: u32,
        ramp_ms: f32,
    }
    fn grid(beats: f32, bpm: f32, repeats: u32, ramp_ms: f32) -> S {
        S {
            feel: FEEL_GRID,
            length: beats * 960.0,
            bpm,
            repeats,
            ramp_ms,
        }
    }
    fn free(secs: f32) -> S {
        S {
            feel: FEEL_FREE,
            length: secs,
            bpm: 0.0,
            repeats: 1,
            ramp_ms: 0.0,
        }
    }
    struct C {
        section: u32,
        layer: u32,
        loop_len: f32,
        curve: i32,
        events: Vec<(f32, f32)>, // (t, dur)
        warp: Vec<(f32, f32)>,
    }
    fn clip(section: u32, layer: u32, loop_len: f32, events: &[f32]) -> C {
        C {
            section,
            layer,
            loop_len,
            curve: -1,
            events: events.iter().map(|&t| (t, 10.0)).collect(),
            warp: vec![],
        }
    }

    fn blob(secs: &[S], clips: &[C], looping: bool) -> Vec<u8> {
        let mut w: Vec<u32> = vec![];
        let ne: usize = clips.iter().map(|c| c.events.len()).sum();
        let nw: usize = clips.iter().map(|c| c.warp.len()).sum();
        w.extend([
            TRACK_MAGIC,
            secs.len() as u32,
            clips.len() as u32,
            ne as u32,
            nw as u32,
            looping as u32,
            0,
            0,
        ]);
        for s in secs {
            w.extend([
                s.feel,
                s.length.to_bits(),
                s.bpm.to_bits(),
                s.repeats,
                s.ramp_ms.to_bits(),
                0,
                0,
                0,
            ]);
        }
        let (mut fe, mut fw) = (0u32, 0u32);
        for c in clips {
            w.extend([
                c.section,
                c.layer,
                c.loop_len.to_bits(),
                fe,
                c.events.len() as u32,
                c.curve as u32,
                fw,
                c.warp.len() as u32,
            ]);
            fe += c.events.len() as u32;
            fw += c.warp.len() as u32;
        }
        for c in clips {
            for &(t, d) in &c.events {
                let mut r = [0u32; 16];
                r[0] = t.to_bits();
                r[1] = d.to_bits();
                r[2] = f32::NAN.to_bits();
                r[5] = 1f32.to_bits();
                w.extend(r);
            }
        }
        for c in clips {
            for &(a, b) in &c.warp {
                w.extend([a.to_bits(), b.to_bits()]);
            }
        }
        w.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    fn setup(secs: &[S], clips: &[C], looping: bool) -> (Track, Transport) {
        let mut t = Track::new();
        t.load(&blob(secs, clips, looping)).unwrap();
        let mut tr = Transport::new(SR);
        tr.play();
        (t, tr)
    }

    /// Run `total` samples in spans of `span`; returns (absolute sample, layer, event).
    fn run(tr: &mut Transport, track: &Track, total: usize, span: usize) -> Vec<(usize, u32, u32)> {
        let mut out = EventList::new();
        let mut got = vec![];
        let mut done = 0;
        while done < total {
            let n = span.min(total - done);
            out.clear();
            tr.advance(track, n, &mut out);
            for e in out.as_slice() {
                got.push((done + e.offset as usize, e.layer, e.event));
            }
            done += n;
        }
        got
    }

    #[test]
    fn exact_offsets_any_split() {
        let secs = [grid(4.0, 120.0, 1, 0.0)];
        let clips = [
            clip(0, 0, 3840.0, &[0.0, 960.0, 1920.0]),
            clip(0, 2, 3840.0, &[960.0]),
        ];
        let (t, mut tr) = setup(&secs, &clips, false);
        let reference = run(&mut tr, &t, 60000, 60000);
        assert_eq!(
            reference,
            vec![(0, 0, 0), (24000, 0, 1), (24000, 2, 3), (48000, 0, 2)]
        );
        for span in [1, 7, 128, 4096] {
            let mut tr = Transport::new(SR);
            tr.play();
            assert_eq!(run(&mut tr, &t, 60000, span), reference, "span {span}");
        }
    }

    #[test]
    fn odd_tempo_is_split_independent() {
        let secs = [grid(4.0, 117.3, 3, 0.0)];
        let clips = [clip(0, 0, 3840.0, &[0.0, 5.0, 479.5, 1000.0, 3839.0])];
        let (t, mut tr) = setup(&secs, &clips, false);
        let reference = run(&mut tr, &t, 400000, 400000);
        assert!(reference.len() > 10);
        for span in [1, 7, 128, 4096] {
            let mut tr = Transport::new(SR);
            tr.play();
            assert_eq!(run(&mut tr, &t, 400000, span), reference, "span {span}");
        }
    }

    #[test]
    fn repeats_then_next_section_then_end() {
        let secs = [grid(1.0, 120.0, 2, 0.0), grid(1.0, 120.0, 1, 0.0)];
        let clips = [clip(0, 0, 960.0, &[0.0]), clip(1, 1, 960.0, &[0.0])];
        let (t, mut tr) = setup(&secs, &clips, false);
        let got = run(&mut tr, &t, 80000, 333);
        assert_eq!(got, vec![(0, 0, 0), (24000, 0, 0), (48000, 1, 1)]);
        assert!(tr.ended());
        assert!(!tr.playing());
    }

    #[test]
    fn loop_flag_wraps_to_first_section() {
        let secs = [grid(1.0, 120.0, 1, 0.0), grid(1.0, 120.0, 1, 0.0)];
        let clips = [clip(0, 0, 960.0, &[0.0]), clip(1, 0, 960.0, &[0.0])];
        let (t, mut tr) = setup(&secs, &clips, true);
        let got = run(&mut tr, &t, 100000, 1000);
        let at: Vec<_> = got.iter().map(|g| (g.0, g.2)).collect();
        assert_eq!(
            at,
            vec![(0, 0), (24000, 1), (48000, 0), (72000, 1), (96000, 0)]
        );
        assert!(!tr.ended());
        assert_eq!(tr.section(), 0);
    }

    #[test]
    fn free_section_in_seconds() {
        let secs = [free(2.5)];
        let clips = [clip(0, 3, 2.5, &[0.0, 0.5, 2.0])];
        let (t, mut tr) = setup(&secs, &clips, false);
        let got = run(&mut tr, &t, 130000, 4096);
        assert_eq!(got, vec![(0, 3, 0), (24000, 3, 1), (96000, 3, 2)]);
        assert!(tr.ended());
        assert_eq!(tr.bpm(), 0.0);
    }

    #[test]
    fn uneven_loop_refires() {
        // 3-beat loop in a 4-beat section: t=0 fires at beat 0 and beat 3.
        let secs = [grid(4.0, 120.0, 1, 0.0)];
        let clips = [clip(0, 0, 2880.0, &[0.0, 960.0])];
        let (t, mut tr) = setup(&secs, &clips, false);
        let got = run(&mut tr, &t, 96000, 128);
        let at: Vec<_> = got.iter().map(|g| (g.0, g.2)).collect();
        assert_eq!(at, vec![(0, 0), (24000, 1), (72000, 0)]);
    }

    #[test]
    fn events_past_loop_len_never_play() {
        let secs = [grid(4.0, 120.0, 1, 0.0)];
        let clips = [clip(0, 0, 960.0, &[0.0, 960.0, 2000.0])];
        let (t, mut tr) = setup(&secs, &clips, false);
        let got = run(&mut tr, &t, 96000, 500);
        let at: Vec<_> = got.iter().map(|g| g.0).collect();
        assert_eq!(at, vec![0, 24000, 48000, 72000]);
        assert!(got.iter().all(|g| g.2 == 0));
    }

    #[test]
    fn tempo_ramp_integrates() {
        let secs = [grid(1.0, 120.0, 1, 0.0), grid(4.0, 60.0, 1, 1000.0)];
        let clips = [clip(1, 0, 3840.0, &[100.0, 400.0, 1000.0, 2000.0])];
        let (t, mut tr) = setup(&secs, &clips, false);
        let mut out = EventList::new();
        tr.advance(&t, 24000 + 24000, &mut out);
        assert_eq!(tr.section(), 1);
        assert!((tr.bpm() - 90.0).abs() < 1e-9, "{}", tr.bpm());
        tr.advance(&t, 24000, &mut out);
        assert!((tr.bpm() - 60.0).abs() < 1e-9);
        // Falls monotonically.
        let mut tr = Transport::new(SR);
        tr.play();
        tr.advance(&t, 24000, &mut out);
        let mut last = 121.0;
        for _ in 0..48 {
            tr.advance(&t, 1000, &mut out);
            assert!(tr.bpm() < last);
            last = tr.bpm();
        }
        // Event samples: ticks(j) = (120 j - 60 j^2 / (2 * 48000)) / 3000 in
        // the ramp, then 0.02 per sample after 1440 ticks (j = 48000).
        let mut tr = Transport::new(SR);
        tr.play();
        let got = run(&mut tr, &t, 24000 + 200000, 977);
        let expect = |tick: f64| -> usize {
            let j = if tick <= 1440.0 {
                // 60/96000 j^2 - 120 j + 3000 tick = 0
                let (a, b, c) = (60.0 / 96000.0, -120.0, 3000.0 * tick);
                (-b - (b * b - 4.0 * a * c).sqrt()) / (2.0 * a)
            } else {
                48000.0 + (tick - 1440.0) / 0.02
            };
            24000 + j.ceil() as usize
        };
        let want: Vec<usize> = [100.0, 400.0, 1000.0, 2000.0]
            .iter()
            .map(|&t| expect(t))
            .collect();
        let at: Vec<usize> = got.iter().map(|g| g.0).collect();
        assert_eq!(at, want);
    }

    #[test]
    fn ramp_continues_across_repeats() {
        let secs = [grid(1.0, 120.0, 1, 0.0), grid(1.0, 60.0, 3, 4000.0)];
        let clips: [C; 0] = [];
        let (t, mut tr) = setup(&secs, &clips, false);
        let mut out = EventList::new();
        // 24000 samples of section 0, then 1 s of section 1 at about 120..114: pass 0 ends,
        // pass 1 starts mid-ramp. bpm keeps falling instead of jumping.
        let mut last = 120.0;
        for _ in 0..200 {
            tr.advance(&t, 1000, &mut out);
            assert!(tr.bpm() <= last + 1e-9);
            last = tr.bpm();
        }
        assert!(tr.repeat() >= 1);
    }

    #[test]
    fn queued_jump_lands_at_section_end() {
        let secs = [
            grid(2.0, 120.0, 4, 0.0),
            grid(1.0, 120.0, 1, 0.0),
            grid(1.0, 120.0, 1, 0.0),
        ];
        let clips = [clip(0, 0, 1920.0, &[0.0]), clip(2, 0, 960.0, &[0.0])];
        let (t, mut tr) = setup(&secs, &clips, false);
        let mut out = EventList::new();
        tr.advance(&t, 1000, &mut out);
        tr.queue_jump(2);
        tr.advance(&t, 47000, &mut out); // up to 48000 - 1: still in pass 0
        assert_eq!((tr.section(), tr.repeat()), (0, 0));
        out.clear();
        tr.advance(&t, 1, &mut out);
        assert_eq!((tr.section(), tr.repeat()), (2, 0));
        assert!(tr.pos() < 1.0);
        // Section 2's t=0 event (global index 1) fired on the sample that entered it.
        assert_eq!(
            out.as_slice()
                .iter()
                .map(|e| (e.offset, e.layer, e.event))
                .collect::<Vec<_>>(),
            vec![(0, 0, 1)]
        );
        assert!(run(&mut tr, &t, 30000, 100).is_empty());
        assert!(tr.ended());
    }

    #[test]
    fn seek_and_stop() {
        let secs = [grid(4.0, 120.0, 1, 0.0), grid(4.0, 120.0, 1, 0.0)];
        let clips = [
            clip(0, 0, 3840.0, &[0.0, 960.0, 1920.0]),
            clip(1, 0, 3840.0, &[0.0, 960.0]),
        ];
        let (t, mut tr) = setup(&secs, &clips, false);
        tr.seek(&t, 1, 960.0);
        assert_eq!((tr.section(), tr.pos()), (1, 960.0));
        let got = run(&mut tr, &t, 100, 100);
        assert_eq!(got, vec![(0, 0, 4)]); // global event 4: section 1's t=960
        tr.stop();
        let p = tr.pos();
        assert!(run(&mut tr, &t, 1000, 100).is_empty());
        assert_eq!(tr.pos(), p);
        tr.play();
        run(&mut tr, &t, 1000, 100);
        assert!(tr.pos() > p);
        // seek past the middle skips earlier events
        tr.seek(&t, 0, 1000.0);
        let got = run(&mut tr, &t, 40000, 1000);
        assert_eq!(got.iter().map(|g| g.2).collect::<Vec<_>>(), vec![2]);
    }

    #[test]
    fn track_changed_clamps_and_continues() {
        let long = [grid(8.0, 120.0, 1, 0.0), grid(4.0, 120.0, 1, 0.0)];
        let (t, mut tr) = setup(&long, &[clip(0, 0, 7680.0, &[0.0, 6000.0])], false);
        tr.seek(&t, 1, 3000.0);
        let short = [grid(2.0, 90.0, 1, 0.0)];
        let mut t2 = Track::new();
        t2.load(&blob(&short, &[clip(0, 0, 1920.0, &[0.0, 960.0])], false))
            .unwrap();
        tr.track_changed(&t2);
        assert_eq!(tr.section(), 0);
        assert_eq!(tr.pos(), 1920.0);
        assert!(tr.playing());
        assert_eq!(tr.bpm(), 90.0);
        // Position inside range is kept.
        tr.seek(&t2, 0, 500.0);
        tr.track_changed(&t2);
        assert_eq!(tr.pos(), 500.0);
        let got = run(&mut tr, &t2, 20000, 128);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].2, 1);
    }

    #[test]
    fn dur_samples_at_fire_time() {
        let secs = [grid(4.0, 120.0, 1, 0.0)];
        let mut c = clip(0, 0, 3840.0, &[0.0, 960.0, 1000.0]);
        c.events[0].1 = 480.0; // half a beat = 12000 samples
        c.events[1].1 = 0.0; // clamps to 1
        c.events[2].1 = 0.01;
        let (t, mut tr) = setup(&secs, &[c], false);
        let mut out = EventList::new();
        tr.advance(&t, 30000, &mut out);
        let d: Vec<u32> = out.as_slice().iter().map(|e| e.dur_samples).collect();
        assert_eq!(d, vec![12000, 1, 1]);
    }

    #[test]
    fn same_sample_order_is_layer_then_event() {
        let secs = [grid(1.0, 120.0, 1, 0.0)];
        let clips = [clip(0, 5, 960.0, &[0.0, 0.0]), clip(0, 1, 960.0, &[0.0])];
        let (t, mut tr) = setup(&secs, &clips, false);
        let mut out = EventList::new();
        tr.advance(&t, 10, &mut out);
        let k: Vec<(u32, u32)> = out.as_slice().iter().map(|e| (e.layer, e.event)).collect();
        assert_eq!(k, vec![(1, 2), (5, 0), (5, 1)]);
    }

    #[test]
    fn span_overflow_is_counted() {
        let secs = [grid(1.0, 120.0, 1, 0.0)];
        let clips = [clip(0, 0, 960.0, &vec![0.0; MAX_SPAN_EVENTS + 5])];
        let (t, mut tr) = setup(&secs, &clips, false);
        let mut out = EventList::new();
        tr.advance(&t, 2, &mut out);
        assert_eq!((out.len, out.dropped), (MAX_SPAN_EVENTS, 5));
    }

    #[test]
    fn empty_track_does_nothing() {
        let t = Track::new();
        let mut tr = Transport::new(SR);
        tr.play();
        let mut out = EventList::new();
        tr.advance(&t, 128, &mut out);
        assert_eq!(out.len, 0);
        assert!(!tr.playing());
    }

    #[test]
    fn load_reads_every_field_and_ignores_duplicate_clip() {
        let mut c = clip(0, 1, 960.0, &[1.0, 2.0]);
        c.curve = 5;
        c.warp = vec![(0.0, 0.0), (1.0, 960.0)];
        let dup = clip(0, 1, 480.0, &[3.0]);
        let secs = [grid(1.0, 100.0, 2, 250.0), free(2.5)];
        let mut t = Track::new();
        t.load(&blob(&secs, &[c, dup], true)).unwrap();
        assert!(t.looping());
        assert_eq!(t.n_sections(), 2);
        assert_eq!(
            *t.section(0),
            Section {
                feel: 0,
                length: 960.0,
                bpm: 100.0,
                repeats: 2,
                ramp_ms: 250.0,
                uid: 0
            }
        );
        assert_eq!(t.section(1).feel, FEEL_FREE);
        assert_eq!(t.section(1).length, 2.5);
        assert_eq!(t.clip_index_for(0, 1), Some(0));
        let cl = t.clip_for(0, 1).unwrap();
        assert_eq!((cl.curve, cl.loop_len, cl.n_events), (5, 960.0, 2));
        assert!(
            t.clip_for(0, 0).is_none() && t.clip_for(1, 1).is_none() && t.clip_for(9, 1).is_none()
        );
        assert_eq!(
            t.warp(cl),
            &[
                WarpAnchor {
                    take_sec: 0.0,
                    clip_t: 0.0
                },
                WarpAnchor {
                    take_sec: 1.0,
                    clip_t: 960.0
                }
            ]
        );
        assert_eq!(t.event(1).t, 2.0);
        assert!(t.event(0).cents.is_nan());
    }

    #[test]
    fn malformed_blobs_keep_previous_contents() {
        let good = blob(
            &[grid(1.0, 120.0, 1, 0.0)],
            &[clip(0, 0, 960.0, &[0.0])],
            false,
        );
        let mut t = Track::new();
        t.load(&good).unwrap();
        let patch = |f: &dyn Fn(&mut Vec<u8>)| {
            let mut b = blob(
                &[grid(2.0, 90.0, 1, 0.0), grid(2.0, 90.0, 1, 0.0)],
                &[clip(1, 0, 960.0, &[0.0, 1.0])],
                false,
            );
            f(&mut b);
            b
        };
        let w = |b: &mut Vec<u8>, i: usize, v: u32| {
            b[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes())
        };
        let sec0 = 8;
        let clip0 = 8 + 16;
        let ev0 = clip0 + 8;
        let cases: Vec<(Vec<u8>, LoadError)> = vec![
            (vec![], LoadError::Malformed),
            (good[..good.len() - 1].to_vec(), LoadError::Malformed),
            (patch(&|b| w(b, 0, 7)), LoadError::BadMagic),
            (patch(&|b| w(b, 1, 65)), LoadError::TooBig),
            (patch(&|b| w(b, 3, 20000)), LoadError::TooBig),
            (patch(&|b| b.push(0)), LoadError::Malformed),
            (patch(&|b| w(b, sec0, 3)), LoadError::Malformed),
            (patch(&|b| w(b, sec0 + 1, 0)), LoadError::Malformed),
            (
                patch(&|b| w(b, sec0 + 1, f32::NAN.to_bits())),
                LoadError::Malformed,
            ),
            (patch(&|b| w(b, sec0 + 3, 0)), LoadError::Malformed),
            (patch(&|b| w(b, clip0, 2)), LoadError::Malformed),
            (patch(&|b| w(b, clip0 + 1, 8)), LoadError::Malformed),
            (patch(&|b| w(b, clip0 + 2, 0)), LoadError::Malformed),
            (patch(&|b| w(b, clip0 + 4, 3)), LoadError::Malformed),
            (patch(&|b| w(b, clip0 + 3, u32::MAX)), LoadError::Malformed),
            (patch(&|b| w(b, clip0 + 5, 64)), LoadError::Malformed),
            (patch(&|b| w(b, clip0 + 7, 1)), LoadError::Malformed),
            (
                patch(&|b| w(b, ev0, 2000f32.to_bits())),
                LoadError::Malformed,
            ), // unsorted
            (
                patch(&|b| w(b, ev0, f32::INFINITY.to_bits())),
                LoadError::Malformed,
            ),
        ];
        for (i, (b, e)) in cases.iter().enumerate() {
            assert_eq!(t.load(b), Err(*e), "case {i}");
            assert_eq!(t.n_sections(), 1, "case {i}");
            assert_eq!(t.section(0).length, 960.0);
            assert_eq!(t.n_events(), 1);
        }
    }
    fn with_uids(mut b: Vec<u8>, uids: &[u32]) -> Vec<u8> {
        for (i, u) in uids.iter().enumerate() {
            let o = (8 + i * 8 + 5) * 4;
            b[o..o + 4].copy_from_slice(&u.to_le_bytes());
        }
        b
    }

    #[test]
    fn reload_follows_section_uids() {
        // Playing the third section (uid 30) with a jump queued to uid 10; then the
        // first section is removed and the rest reordered. The transport must stay
        // on uid 30 at the same position, and the jump must still go to uid 10.
        let mut t = Track::new();
        t.load(&with_uids(
            blob(
                &[
                    grid(4.0, 120.0, 1, 0.0),
                    grid(4.0, 120.0, 1, 0.0),
                    grid(8.0, 120.0, 1, 0.0),
                    free(2.0),
                ],
                &[],
                true,
            ),
            &[20, 10, 30, 40],
        ))
        .unwrap();
        let mut tr = Transport::new(SR);
        tr.seek(&t, 2, 1000.0);
        tr.play();
        tr.queue_jump(1);
        let (cur, jump) = (t.section(2).uid, t.section(1).uid);
        t.load(&with_uids(
            blob(
                &[
                    grid(8.0, 120.0, 1, 0.0),
                    grid(4.0, 120.0, 1, 0.0),
                    free(2.0),
                ],
                &[],
                true,
            ),
            &[30, 10, 40],
        ))
        .unwrap();
        tr.follow_uids(&t, cur, jump);
        tr.track_changed(&t);
        assert_eq!(tr.section(), 0);
        assert!((tr.pos() - 1000.0).abs() < 1e-6);
        assert_eq!(tr.queued_jump(), Some(1));
        // A uid the new track lacks leaves the index as it was (then clamped).
        tr.follow_uids(&t, 99, 0);
        assert_eq!(tr.section(), 0);
    }

    // ---- v2: morph markers

    /// (t, state, len_ms, shape)
    type Mk = (f32, u32, f32, u32);

    /// A v2 blob: one grid section of `len` ticks and one clip per entry of
    /// `clips` (layer = index, loop_len = `len`), each with the given markers.
    fn blob2(len: f32, clips: &[Vec<Mk>]) -> Vec<u32> {
        let nm: usize = clips.iter().map(|c| c.len()).sum();
        let mut w: Vec<u32> = vec![TRACK_MAGIC2, 1, clips.len() as u32, 0, 0, 0, nm as u32, 0];
        w.extend([FEEL_GRID, len.to_bits(), 120f32.to_bits(), 1, 0, 0, 0, 0]);
        let mut first = 0u32;
        for (i, c) in clips.iter().enumerate() {
            w.extend([
                0,
                i as u32,
                len.to_bits(),
                0,
                0,
                (-1i32) as u32,
                0,
                0,
                first,
                c.len() as u32,
                0,
                0,
            ]);
            first += c.len() as u32;
        }
        for c in clips {
            for &(t, st, ms, sh) in c {
                w.extend([t.to_bits(), st, ms.to_bits(), sh]);
            }
        }
        w
    }

    fn bytes(w: &[u32]) -> Vec<u8> {
        w.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    #[test]
    fn v2_loads_markers_and_v1_still_loads() {
        let mut t = Track::new();
        let w = blob2(
            3840.0,
            &[
                vec![(0.0, 0, 0.0, MORPH_LINE), (1920.0, 1, 500.0, MORPH_EASE)],
                vec![],
            ],
        );
        t.load(&bytes(&w)).unwrap();
        assert_eq!(t.n_clips(), 2);
        assert_eq!(t.clip(0).n_morph, 2);
        assert_eq!(
            t.clip_morph(t.clip(0))[1],
            Morph {
                t: 1920.0,
                state: 1,
                len_ms: 500.0,
                shape: MORPH_EASE
            }
        );
        assert_eq!(t.clip(1).n_morph, 0);
        assert!(t.morph_blend(1, 10.0, 1.0).is_none());
        // A v1 blob replaces it and has no markers.
        let secs = [grid(4.0, 120.0, 1, 0.0)];
        t.load(&blob(&secs, &[clip(0, 0, 3840.0, &[0.0])], false))
            .unwrap();
        assert_eq!((t.n_clips(), t.clip(0).n_morph), (1, 0));
        assert!(t.morph_blend(0, 0.0, 1.0).is_none());
    }

    #[test]
    fn malformed_v2_is_rejected_and_keeps_the_previous_track() {
        let good = blob2(3840.0, &[vec![(0.0, 0, 0.0, 0), (960.0, 1, 100.0, 1)]]);
        let mut t = Track::new();
        t.load(&bytes(&good)).unwrap();
        let first_marker = 8 + 8 + 12;
        let bad: Vec<(&str, Vec<u32>, LoadError)> = vec![
            (
                "state out of range",
                {
                    let mut w = good.clone();
                    w[first_marker + 1] = MAX_STATES as u32;
                    w
                },
                LoadError::Malformed,
            ),
            (
                "unknown shape",
                {
                    let mut w = good.clone();
                    w[first_marker + 3] = 9;
                    w
                },
                LoadError::Malformed,
            ),
            (
                "negative length",
                {
                    let mut w = good.clone();
                    w[first_marker + 2] = (-5f32).to_bits();
                    w
                },
                LoadError::Malformed,
            ),
            (
                "NaN time",
                {
                    let mut w = good.clone();
                    w[first_marker] = f32::NAN.to_bits();
                    w
                },
                LoadError::Malformed,
            ),
            (
                "unsorted",
                {
                    let mut w = good.clone();
                    w[first_marker + 4] = 0f32.to_bits();
                    w[first_marker] = 5f32.to_bits();
                    w
                },
                LoadError::Malformed,
            ),
            (
                "run past the array",
                {
                    let mut w = good.clone();
                    w[8 + 8 + 9] = 3;
                    w
                },
                LoadError::Malformed,
            ),
            (
                "short",
                good[..good.len() - 1].to_vec(),
                LoadError::Malformed,
            ),
            (
                "long",
                {
                    let mut w = good.clone();
                    w.push(0);
                    w
                },
                LoadError::Malformed,
            ),
            (
                "too many markers",
                {
                    let mut w = good.clone();
                    w[6] = MAX_MORPH as u32 + 1;
                    w
                },
                LoadError::TooBig,
            ),
        ];
        for (what, w, err) in bad {
            assert_eq!(t.load(&bytes(&w)), Err(err), "{what}");
            assert_eq!(t.n_clips(), 1, "{what}");
            assert_eq!(t.clip_morph(t.clip(0)).len(), 2, "{what}");
        }
        let mut w = good.clone();
        w[0] = 0x3354_5745;
        assert_eq!(t.load(&bytes(&w)), Err(LoadError::BadMagic));
    }

    #[test]
    fn morph_blend_follows_the_lane() {
        // 120 bpm: 1 ms is 1.92 ticks.
        let ms = 1.0 / 1.92;
        let w = blob2(
            3840.0,
            &[vec![
                (0.0, 0, 0.0, MORPH_LINE),
                (1920.0, 1, 500.0, MORPH_LINE),
                (2880.0, 2, 200.0, MORPH_STEP),
            ]],
        );
        let mut t = Track::new();
        t.load(&bytes(&w)).unwrap();
        let at = |p: f64| t.morph_blend(0, p, ms).unwrap();
        assert_eq!(
            at(100.0),
            MorphBlend {
                from: 2,
                to: 0,
                x: 1.0
            }
        ); // len 0 on the first marker
        let b = at(1920.0 + 480.0); // 250 ms into the 500 ms line
        assert_eq!((b.from, b.to), (0, 1));
        assert!((b.x - 0.5).abs() < 1e-3, "{}", b.x);
        assert_eq!(at(1920.0 + 2000.0).x, 1.0);
        assert_eq!(
            at(2880.0 + 1.0),
            MorphBlend {
                from: 1,
                to: 2,
                x: 1.0
            }
        ); // step: at once
        // The position wraps with the loop.
        assert_eq!(at(3840.0 + 1920.0 + 480.0).x, b.x);
        // No rate: everything completes.
        assert_eq!(t.morph_blend(0, 1920.0 + 10.0, 0.0).unwrap().x, 1.0);
    }

    #[test]
    fn morph_shapes() {
        let mk = |shape| {
            let w = blob2(
                3840.0,
                &[vec![(0.0, 0, 0.0, 0), (1000.0, 1, 1000.0, shape)]],
            );
            let mut t = Track::new();
            t.load(&bytes(&w)).unwrap();
            // 1000 ms per 1920 ticks: half way is 500 ms in.
            t.morph_blend(0, 1000.0 + 960.0, 1000.0 / 1920.0).unwrap().x
        };
        assert!((mk(MORPH_LINE) - 0.5).abs() < 1e-6);
        assert!((mk(MORPH_EASE) - 0.5).abs() < 1e-6);
        assert!((mk(MORPH_LATE) - 0.125).abs() < 1e-6);
        assert_eq!(mk(MORPH_STEP), 1.0);
        // wobble: 0.5 + 0.12 sin(3 pi) * 0.5 = 0.5 (sin(3pi) = 0)
        assert!((mk(MORPH_WOBBLE) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn one_marker_means_that_state_throughout() {
        let w = blob2(3840.0, &[vec![(960.0, 3, 500.0, MORPH_LINE)]]);
        let mut t = Track::new();
        t.load(&bytes(&w)).unwrap();
        for p in [0.0, 500.0, 960.0, 1000.0, 3000.0] {
            let b = t.morph_blend(0, p, 0.5).unwrap();
            assert_eq!((b.from, b.to), (3, 3), "at {p}");
        }
    }
}
