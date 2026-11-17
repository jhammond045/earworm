//! Sequencer: the compiled track table and the transport (TDD "Sequencer,
//! transport and tuning"). The engine owns time; the interface only edits
//! data. Nothing here allocates after `new`.
//!
//! # Track blob (`ew_load` kind LOAD_TRACK)
//!
//! 32-bit little-endian words. `u32` words are integers, the rest are f32.
//!
//! ```text
//! header, 8 words:
//!   u32 magic TRACK_MAGIC, u32 n_sections, u32 n_clips, u32 n_events,
//!   u32 n_warp, u32 flags (bit 0: loop the track), u32 0, u32 0
//! sections, n_sections x 8 words:
//!   u32 feel (FEEL_GRID or FEEL_FREE),
//!   f32 length (ticks for grid, 960 per beat; seconds for free),
//!   f32 bpm (grid; ignored for free), u32 repeats (>= 1),
//!   f32 ramp_ms (tempo ramp from the previous section's bpm), 3 x u32 0
//! clips, n_clips x 8 words:
//!   u32 section, u32 layer, f32 loop_len (section units, > 0),
//!   u32 first_event, u32 n_events (a run in the events array, sorted by t),
//!   i32 curve (curve slot, -1 for none), u32 first_warp, u32 n_warp
//! events, n_events x 16 words (one record for notes and hits):
//!   0 t, 1 dur (section units), 2 cents (NaN: kit slot pitch), 3 hard (0..1),
//!   4 u32 slot (kit slot for hits), 5 strength (0..1, velocity),
//!   6 attack, 7 burst, 8 bend, 9 hold, 10 vowel, 11 tone, 12 end_stop,
//!   13 end_click (hit shape, TDD HitShape; NaN or 0 when unused), 14 0, 15 0
//! warp anchors, n_warp x 2 words:
//!   f32 take_sec, f32 clip_t (section units), sorted by clip_t
//! ```
//!
//! A clip plays in its section on its layer. At most one clip per (section,
//! layer); a later duplicate is ignored. Events with t >= loop_len never play.
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

/// Ticks per beat.
pub const PPQ: f64 = 960.0;
pub const TRACK_MAGIC: u32 = 0x3154_5745; // "EWT1"
pub const FEEL_GRID: u32 = 0;
pub const FEEL_FREE: u32 = 1;

pub const MAX_SECTIONS: usize = 64;
pub const MAX_CLIPS: usize = 512;
pub const MAX_EVENTS: usize = 16384;
pub const MAX_WARP: usize = 16384;
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

/// The compiled arrangement: fixed-capacity pools filled by `load`.
pub struct Track {}

impl Track {
    /// Allocates every pool at its maximum size. An empty track has no sections.
    pub fn new() -> Self {
        todo!()
    }
    /// Parse a blob into the pools. On error the previous contents are kept.
    /// Validates counts against the MAX_* limits and every index (section,
    /// layer < protocol::MAX_LAYERS, event and warp runs in range, lengths and
    /// loop_len finite and > 0, repeats >= 1).
    pub fn load(&mut self, bytes: &[u8]) -> Result<(), LoadError> {
        let _ = bytes;
        todo!()
    }
    pub fn n_sections(&self) -> usize {
        todo!()
    }
    pub fn section(&self, i: usize) -> &Section {
        let _ = i;
        todo!()
    }
    pub fn looping(&self) -> bool {
        todo!()
    }
    /// The clip playing on `layer` in `section`, if any.
    pub fn clip_for(&self, section: usize, layer: usize) -> Option<&Clip> {
        let _ = (section, layer);
        todo!()
    }
    pub fn clip(&self, i: usize) -> &Clip {
        let _ = i;
        todo!()
    }
    pub fn event(&self, i: usize) -> &Event {
        let _ = i;
        todo!()
    }
    pub fn warp(&self, clip: &Clip) -> &[WarpAnchor] {
        let _ = clip;
        todo!()
    }
}

/// One event the transport fired during a span.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SeqEvent {
    /// Sample offset inside the span passed to `advance`.
    pub offset: u32,
    pub layer: u32,
    /// Index into the track's clips and events.
    pub clip: u32,
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
        Self { items: [SeqEvent::default(); MAX_SPAN_EVENTS], len: 0, dropped: 0 }
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

pub struct Transport {}

impl Transport {
    pub fn new(sample_rate: f32) -> Self {
        let _ = sample_rate;
        todo!()
    }
    /// Start from the current position. No effect on an empty track.
    pub fn play(&mut self) {
        todo!()
    }
    pub fn stop(&mut self) {
        todo!()
    }
    pub fn playing(&self) -> bool {
        todo!()
    }
    /// Move to `pos` (section units) in `section`, first pass, clearing any
    /// queued jump. Events exactly at `pos` fire on the next advance.
    pub fn seek(&mut self, track: &Track, section: usize, pos: f64) {
        let _ = (track, section, pos);
        todo!()
    }
    pub fn queue_jump(&mut self, section: usize) {
        let _ = section;
        todo!()
    }
    /// A new track was loaded: keep playing, clamping the position into it.
    pub fn track_changed(&mut self, track: &Track) {
        let _ = track;
        todo!()
    }
    /// Advance `n` samples while playing, appending fired events to `out`
    /// (which the caller clears). Does nothing when stopped.
    pub fn advance(&mut self, track: &Track, n: usize, out: &mut EventList) {
        let _ = (track, n, out);
        todo!()
    }
    pub fn section(&self) -> usize {
        todo!()
    }
    pub fn pos(&self) -> f64 {
        todo!()
    }
    /// Pass number inside the current section, 0-based.
    pub fn repeat(&self) -> u32 {
        todo!()
    }
    /// Current bpm (after any ramp); 0 in a free section.
    pub fn bpm(&self) -> f64 {
        todo!()
    }
    /// Section units per sample right now (ticks or seconds).
    pub fn units_per_sample(&self) -> f64 {
        todo!()
    }
    /// True when a non-looping track ran past its last section.
    pub fn ended(&self) -> bool {
        todo!()
    }
}
