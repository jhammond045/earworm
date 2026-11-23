//! Earworm M1 engine core ("the Klont"): layers of voices, the drum model,
//! space, the sequencer and transport, behind the C ABI in the TDD.
//! See docs/M1.md.
//!
//! The same module runs live in the AudioWorklet (`ew_process`, one render
//! quantum) and offline in a worker (`ew_render`, any length). All memory is
//! allocated in `ew_init`; process and render calls never allocate.

pub mod build;
pub mod classic;
pub mod curve;
pub mod engine;
pub mod grain;
pub mod layer;
pub mod live;
mod metro;
pub mod notecurve;
pub mod modules;
pub mod patch;
pub mod protocol;
pub mod samples;
pub mod seq;
pub mod stretch;
pub mod voice;

pub use engine::Engine;
