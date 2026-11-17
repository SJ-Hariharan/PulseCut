//! PulseCut core: shared domain model, settings, application paths, external tool discovery
//! and a safe (shell-free) process runner. Every other crate depends on this one; it contains
//! no platform-specific code (see `pulsecut-platform` for OS abstractions).

pub mod domain;
pub mod error;
pub mod paths;
pub mod process;
pub mod settings;
pub mod time;

pub use domain::*;
pub use error::{Error, Result};
pub use settings::Settings;
pub use time::{TsMs, now_ms};
