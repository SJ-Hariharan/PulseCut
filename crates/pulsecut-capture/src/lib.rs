//! Capture: turning a `CaptureRequest` (wall-clock window around a moment) into a local file
//! that provably contains that window — or an explicit coverage report saying what's missing.
//!
//! Backends:
//! * **OBS replay buffer, deferred save** (`obs`): OBS's `SaveReplayBuffer` only flushes the
//!   footage *already* in the ring, so PulseCut waits until `clip_end + margin` before saving.
//!   The buffer must therefore be ≥ pre-roll + post-roll + detection latency + margin; the
//!   controller reads `RecRBTime` from the active OBS profile and warns when it's too short.
//! * **FFmpeg rolling segment buffer** (`segment`): PulseCut runs its own `-f segment` ring from
//!   any FFmpeg input (RTMP/SRT/HLS URL, capture device, file, test source). Used for
//!   Autonomous mode on remote streams and for development without OBS.
//!
//! Both feed the `coordinator`, which merges overlapping requests and schedules saves.

pub mod coordinator;
pub mod mock_obs;
pub mod obs;
pub mod segment;
pub mod sync;

use pulsecut_core::{Coverage, TsMs};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A local media file whose time 0 corresponds to wall-clock `file_start_ts`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapturedFile {
    pub path: PathBuf,
    pub file_start_ts: TsMs,
    pub duration_ms: i64,
    pub coverage: Coverage,
    /// Bound on the wall-clock anchoring error (ms).
    pub sync_uncertainty_ms: i64,
    pub backend: String,
}

impl CapturedFile {
    /// Position of a wall-clock instant inside this file.
    pub fn offset_of(&self, ts: TsMs) -> i64 {
        ts - self.file_start_ts
    }
}

pub fn coverage_for(file_start: TsMs, duration_ms: i64, want_start: TsMs, want_end: TsMs, moment: TsMs) -> Coverage {
    Coverage {
        requested_start: want_start,
        requested_end: want_end,
        actual_start: file_start,
        actual_end: file_start + duration_ms,
        moment_ts: moment,
    }
}
