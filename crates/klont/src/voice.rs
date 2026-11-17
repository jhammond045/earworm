//! One Klont voice: the per-voice modules of the TDD "Klont DSP" table and
//! the drum model. The layer resolves parameters (steps 1 to 6) and hands the
//! voice real-unit controls once per control tick; the voice runs its own
//! envelopes and modulators (step 7) and renders audio.
//!
//! Signal path per sample:
//! (partial bank + start/end click) -> clang -> fold -> comb -> filter -> out.
//! Shaped noise is per layer (TDD: equivalent because the path is linear);
//! the voice only reports how loud its noise should be (`noise_level`).
//!
//! Control rate: the engine calls `control` at every global tick boundary
//! (every TICK samples) and `start` at any sample. Between calls `render` is
//! called on consecutive sub-spans whose lengths add up to the `len` given
//! to the last `control`/`start`. Amplitudes ramp linearly across that `len`,
//! so a note can start mid-tick and still ramp to the next boundary.
//! The output must not depend on how a span is split into `render` calls.

/// Samples per control tick.
pub const TICK: usize = 32;
pub const MAX_PARTIALS: usize = 64;
pub const N_BANDS: usize = 16;

/// Per-tick controls in real units, after resolution steps 1 to 6.
#[derive(Clone, Copy, Debug)]
pub struct VoiceControls {
    /// Carrier pitch in cents above C0, before the pitch envelope and LFOs.
    pub cents: f32,
    /// Linear amplitude (layer level is applied by the layer, not here):
    /// velocity and gesture loudness. 1 is nominal.
    pub amp: f32,
    pub harm_mix: f32,
    pub noise_mix: f32,
    pub tilt: f32,
    pub oddeven: f32,
    pub stretch: f32,
    pub jitter: f32,
    pub modal: f32,
    pub damp: f32,
    pub f1_freq: f32,
    pub f1_gain: f32,
    pub f2_freq: f32,
    pub f2_gain: f32,
    /// dB at the 16 band centres (dsp::bands::band_centres(16, ..)).
    pub tone_bands: [f32; N_BANDS],
    pub fold_amt: f32,
    pub fold_bias: f32,
    pub clang_amt: f32,
    pub clang_ratio: f32,
    pub comb_amt: f32,
    pub comb_tune: f32,
    pub comb_decay: f32,
    pub filt_cutoff: f32,
    pub filt_res: f32,
    pub filt_morph: f32,
    pub filt_track: f32,
    pub strike_color: f32,
    pub lfo1_rate: f32,
    pub lfo1_pitch: f32,
    pub lfo1_filter: f32,
    pub lfo1_level: f32,
    pub lfo2_rate: f32,
    pub lfo2_pitch: f32,
    pub lfo2_filter: f32,
    pub chaos_rate: f32,
    pub chaos_amt: f32,
    /// Most partials this voice may sound this tick (the global budget's share).
    pub max_partials: usize,
}

/// Fixed for the life of a note (hit shape already applied by the layer).
#[derive(Clone, Copy, Debug)]
pub struct NoteStart {
    pub velocity: f32,
    pub attack: f32,
    pub hold: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    pub noise_decay: f32,
    /// Semitones at note start, decaying with `pitch_time`.
    pub pitch_amt: f32,
    pub pitch_time: f32,
    /// Start click amount 0..1.
    pub click: f32,
    /// End click amount 0..1, fired on release (or at a choke from end_stop).
    pub end_click: f32,
    /// Hit shape endStop: choke (3 ms) at the end of hold instead of decaying.
    pub end_stop: bool,
}

pub struct Voice {}

impl Voice {
    pub fn new(sample_rate: f32, seed: u32) -> Self {
        let _ = (sample_rate, seed);
        todo!()
    }
    /// Start a note. `len` samples remain until the next tick boundary.
    pub fn start(&mut self, s: &NoteStart, c: &VoiceControls, len: usize) {
        let _ = (s, c, len);
        todo!()
    }
    /// Once per tick while active. `len` is normally TICK.
    pub fn control(&mut self, c: &VoiceControls, len: usize) {
        let _ = (c, len);
        todo!()
    }
    /// Note off: enter release and fire the end click.
    pub fn release(&mut self) {
        todo!()
    }
    /// Fade to silence in 3 ms, then go idle.
    pub fn choke(&mut self) {
        todo!()
    }
    /// Silence at once and clear all state.
    pub fn kill(&mut self) {
        todo!()
    }
    /// Add this voice into `out` (mono).
    pub fn render(&mut self, out: &mut [f32]) {
        let _ = out;
        todo!()
    }
    /// Sounding or ringing (an idle voice costs nothing and may be reused).
    pub fn active(&self) -> bool {
        todo!()
    }
    pub fn released(&self) -> bool {
        todo!()
    }
    /// Envelope times amp, for budget sharing and voice stealing.
    pub fn loudness(&self) -> f32 {
        todo!()
    }
    /// Shaped-noise amplitude this voice asks of its layer for the current
    /// tick: noise_mix * noise envelope * amp * velocity.
    pub fn noise_level(&self) -> f32 {
        todo!()
    }
    /// Audible partials at the last control tick before the budget cut.
    pub fn wanted_partials(&self) -> usize {
        todo!()
    }
    /// Partials actually sounding at the last control tick.
    pub fn active_partials(&self) -> usize {
        todo!()
    }
    /// Current pitch in cents including pitch envelope and modulators.
    pub fn pitch_cents(&self) -> f32 {
        todo!()
    }
}
