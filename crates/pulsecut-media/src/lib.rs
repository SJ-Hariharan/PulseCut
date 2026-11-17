//! Media pipeline. Rust decides *what* to do; FFmpeg (and whisper.cpp) do the codec work.
//!
//! * `probe`     – ffprobe metadata
//! * `encoder`   – hardware/software H.264 encoder selection with a real test encode
//! * `textimg`   – font loading and outlined text rasterization (no libass/drawtext needed)
//! * `captions`  – word timings → animated caption image sequence (ffconcat)
//! * `transcribe`– whisper.cpp word-level transcription
//! * `timeline`  – source→output time mapping for timeline-altering effects
//! * `render`    – EditPlan → FFmpeg filter graph → finished clip
//! * `analysis`  – motion-centre (smart crop), audio peaks of a file
//! * `thumbnail` – frame scoring and thumbnail composition
//! * `sfx`       – original procedurally synthesized sound effects (CC0)
//! * `frames`    – decoded preview frames for the desktop UI

pub mod analysis;
pub mod assets;
pub mod captions;
pub mod encoder;
pub mod frames;
pub mod probe;
pub mod render;
pub mod sfx;
pub mod textimg;
pub mod thumbnail;
pub mod timeline;
pub mod transcribe;

pub use probe::MediaInfo;
