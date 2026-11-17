//! Streaming highlight detection.
//!
//! Chat messages and per-second audio features are pushed in as they arrive; `Detector::tick`
//! advances a 1-second evaluation clock and emits `HighlightEvent`s with explainable reasons.
//!
//! Design notes (see ARCHITECTURE.md §Detection):
//! * Every signal is converted to a 0..1 "excitement" relative to an adaptive per-channel
//!   baseline. Count signals use `min(log-ratio term, Poisson-z term)` so that quiet channels
//!   need a real z-score and large channels need a real ratio (idea from sharkyclips, MIT,
//!   independently reimplemented for streaming input).
//! * Chat is shifted back by the configured reaction delay so chat and audio line up on the
//!   *moment* timeline.
//! * Weights are not hand-picked: `eval::tune` grid-searches them on labelled synthetic
//!   scenarios; the shipped defaults are the tuned result recorded in PERFORMANCE.md.

pub mod audio;
pub mod baseline;
pub mod chat;
pub mod eval;
pub mod merge;
pub mod rng;
pub mod scorer;
pub mod synth;
pub mod text;

pub use audio::{AudioAnalyzer, AudioSecond};
pub use chat::ChatMessage;
pub use scorer::{Detector, DetectorConfig, HypePoint, Signal, Weights};
