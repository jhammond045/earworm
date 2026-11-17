//! M0 parameter list: a hand-written subset of the TDD parameter table.
//!
//! Values are in real units (not normalized 0..1 as in the product), since
//! M0 has no morph, macros or Influence blend to interpolate. The order here
//! is the wire order: `web/src/host/protocol.ts` mirrors it and a test in
//! `web/src/host/protocol.test.ts` checks the two lists match.

#[derive(Clone, Copy, Debug)]
pub struct ParamDesc {
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    /// One-pole smoothing time applied by the synth.
    pub smooth_ms: f32,
}

const fn p(name: &'static str, min: f32, max: f32, default: f32, smooth_ms: f32) -> ParamDesc {
    ParamDesc { name, min, max, default, smooth_ms }
}

// Sources
pub const HARM_MIX: usize = 0;
pub const NOISE_MIX: usize = 1;
pub const TILT: usize = 2; // dB per octave of partial number
pub const ODDEVEN: usize = 3; // -1 = only even boosted .. +1 = odd boosted, dB-scaled below
pub const STRETCH: usize = 4; // inharmonicity B in f_k = k f0 sqrt(1 + B k^2)
pub const MODAL: usize = 5; // 0 harmonic, 1 drumhead, 2 bar
pub const DAMP: usize = 6; // 0 = all partials share env.decay; >0 high partials die first
pub const F1_FREQ: usize = 7;
pub const F1_GAIN: usize = 8; // dB
pub const F2_FREQ: usize = 9;
pub const F2_GAIN: usize = 10; // dB
// Envelopes (seconds unless noted)
pub const ENV_ATTACK: usize = 11;
pub const ENV_HOLD: usize = 12;
pub const ENV_DECAY: usize = 13;
pub const ENV_SUSTAIN: usize = 14; // 0..1
pub const ENV_RELEASE: usize = 15;
pub const ENV_NOISE_DECAY: usize = 16;
pub const ENV_PITCH_AMT: usize = 17; // semitones at note start
pub const ENV_PITCH_TIME: usize = 18; // seconds
// Voice gesture depths, 0..1 (Mouth: 0 keeps own tone, 1 imposes the vowel)
pub const DEPTH_LOUD: usize = 19;
pub const DEPTH_OPEN: usize = 20;
pub const DEPTH_HARD: usize = 21;
pub const DEPTH_BREATH: usize = 22;
pub const DEPTH_MOUTH: usize = 23;
// Tuning
pub const TUNE_SNAP: usize = 24; // 0..1
pub const TUNE_SPEED: usize = 25; // ms of lag, 0..250
pub const TUNE_SLIDES: usize = 26; // 0 glide, 1 gliss, 2 jump
pub const TUNE_VIBRATO: usize = 27; // 0 flatten, 1 keep, 2 exaggerate
pub const TUNE_ROOT: usize = 28; // pitch class 0..11, 0 = C
pub const TUNE_SCALE: usize = 29; // index into tune::SCALES
pub const TUNE_REF_CENTS: usize = 30; // singer's offset from concert pitch, -50..50
pub const TUNE_OCTAVE: usize = 31; // output transpose in octaves, -2..2
// Output
pub const LEVEL: usize = 32; // dB
pub const PARTIALS: usize = 33; // number of partials, 1..64

pub const COUNT: usize = 34;

pub static PARAMS: [ParamDesc; COUNT] = [
    p("harm_mix", 0.0, 1.0, 1.0, 15.0),
    p("noise_mix", 0.0, 1.0, 0.15, 15.0),
    p("tilt", -24.0, 6.0, -6.0, 15.0),
    p("oddeven", -1.0, 1.0, 0.0, 15.0),
    p("stretch", 0.0, 0.01, 0.0, 15.0),
    p("modal", 0.0, 2.0, 0.0, 15.0),
    p("damp", 0.0, 1.0, 0.0, 15.0),
    p("f1_freq", 200.0, 1200.0, 600.0, 15.0),
    p("f1_gain", 0.0, 24.0, 0.0, 15.0),
    p("f2_freq", 700.0, 3500.0, 1500.0, 15.0),
    p("f2_gain", 0.0, 24.0, 0.0, 15.0),
    p("env.attack", 0.001, 2.0, 0.01, 0.0),
    p("env.hold", 0.0, 2.0, 0.0, 0.0),
    p("env.decay", 0.01, 4.0, 0.3, 0.0),
    p("env.sustain", 0.0, 1.0, 0.8, 0.0),
    p("env.release", 0.005, 4.0, 0.15, 0.0),
    p("env.noise_decay", 0.005, 4.0, 0.2, 0.0),
    p("env.pitch_amt", -24.0, 48.0, 0.0, 0.0),
    p("env.pitch_time", 0.001, 0.5, 0.05, 0.0),
    p("depth.loud", 0.0, 1.0, 1.0, 15.0),
    p("depth.open", 0.0, 1.0, 0.5, 15.0),
    p("depth.hard", 0.0, 1.0, 0.5, 15.0),
    p("depth.breath", 0.0, 1.0, 0.5, 15.0),
    p("depth.mouth", 0.0, 1.0, 1.0, 15.0),
    p("tune.snap", 0.0, 1.0, 0.0, 0.0),
    p("tune.speed", 0.0, 250.0, 60.0, 0.0),
    p("tune.slides", 0.0, 2.0, 0.0, 0.0),
    p("tune.vibrato", 0.0, 2.0, 1.0, 0.0),
    p("tune.root", 0.0, 11.0, 0.0, 0.0),
    p("tune.scale", 0.0, 5.0, 0.0, 0.0),
    p("tune.ref_cents", -50.0, 50.0, 0.0, 0.0),
    p("tune.octave", -2.0, 2.0, 0.0, 0.0),
    p("level", -60.0, 6.0, -12.0, 15.0),
    p("partials", 1.0, 64.0, 48.0, 0.0),
];

/// Clamp a value into a parameter's range.
pub fn clamp(id: usize, v: f32) -> f32 {
    let d = &PARAMS[id];
    v.clamp(d.min, d.max)
}

/// A full parameter set: scalars plus the two band shapes (dB per band).
#[derive(Clone, Debug)]
pub struct ParamSet {
    pub scalars: [f32; COUNT],
    pub tone_bands: [f32; crate::frame::MAX_BANDS],
    pub noise_bands: [f32; crate::frame::MAX_BANDS],
}

impl Default for ParamSet {
    fn default() -> Self {
        let mut scalars = [0.0; COUNT];
        for (i, d) in PARAMS.iter().enumerate() {
            scalars[i] = d.default;
        }
        Self { scalars, tone_bands: [0.0; crate::frame::MAX_BANDS], noise_bands: [0.0; crate::frame::MAX_BANDS] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_in_range_and_names_unique() {
        for (i, d) in PARAMS.iter().enumerate() {
            assert!(d.min <= d.default && d.default <= d.max, "{}", d.name);
            for e in &PARAMS[i + 1..] {
                assert_ne!(d.name, e.name);
            }
        }
    }
}
