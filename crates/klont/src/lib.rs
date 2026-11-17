//! Earworm M1 engine core ("the Klont"): layers of voices, the drum model,
//! space, the sequencer and transport, behind the C ABI in the TDD.
//! See docs/M1.md.
//!
//! The same module runs live in the AudioWorklet (`ew_process`, one render
//! quantum) and offline in a worker (`ew_render`, any length). All memory is
//! allocated in `ew_init`; process and render calls never allocate.

pub mod curve;
pub mod engine;
pub mod layer;
pub mod protocol;
pub mod seq;
pub mod voice;

pub use engine::Engine;
