//! Shared domain model. These types cross crate boundaries (detector → capture → editor →
//! library → publisher) and are persisted as JSON in SQLite, so changes must stay
//! backwards-compatible (`#[serde(default)]` on new fields).

use crate::time::TsMs;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// What caused a capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerSource {
    Hotkey,
    Voice,
    Manual,
    LocalApi,
    Chat,
    Audio,
    MultiSignal,
    Gameplay,
}

impl TriggerSource {
    pub fn label(self) -> &'static str {
        match self {
            TriggerSource::Hotkey => "Hotkey",
            TriggerSource::Voice => "Voice command",
            TriggerSource::Manual => "Manual",
            TriggerSource::LocalApi => "Local API",
            TriggerSource::Chat => "Chat burst",
            TriggerSource::Audio => "Audio spike",
            TriggerSource::MultiSignal => "Multi-signal",
            TriggerSource::Gameplay => "Gameplay",
        }
    }
    pub fn is_manual(self) -> bool {
        matches!(
            self,
            TriggerSource::Hotkey | TriggerSource::Voice | TriggerSource::Manual | TriggerSource::LocalApi
        )
    }
}

/// The detector's best guess at what kind of moment this is. Drives editing recipes and titles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MomentKind {
    Hype,
    Funny,
    Fail,
    Clutch,
    Shock,
    Wholesome,
    #[default]
    Generic,
}

impl MomentKind {
    pub const ALL: [MomentKind; 7] = [
        MomentKind::Hype,
        MomentKind::Funny,
        MomentKind::Fail,
        MomentKind::Clutch,
        MomentKind::Shock,
        MomentKind::Wholesome,
        MomentKind::Generic,
    ];
    pub fn label(self) -> &'static str {
        match self {
            MomentKind::Hype => "Hype",
            MomentKind::Funny => "Funny",
            MomentKind::Fail => "Fail",
            MomentKind::Clutch => "Clutch",
            MomentKind::Shock => "Shock",
            MomentKind::Wholesome => "Wholesome",
            MomentKind::Generic => "Moment",
        }
    }
}

/// One human-readable explanation of why something scored highly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reason {
    /// Signal name, e.g. `chat.velocity`, `audio.loudness`, `chat.laugh_vocab`.
    pub signal: String,
    /// Human explanation, e.g. "42 msg/s, 6.1σ above channel baseline".
    pub detail: String,
    /// Share of the final score attributable to this signal (0..1).
    pub contribution: f32,
}

/// A detected (or manually requested) highlight.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HighlightEvent {
    pub id: Uuid,
    /// Source stream key, e.g. `twitch:somechannel`, `local:obs`.
    pub stream_id: String,
    pub trigger: TriggerSource,
    /// Estimated wall-clock time of the on-stream moment itself (after latency correction).
    pub moment_ts: TsMs,
    /// When the detector fired.
    pub detected_at: TsMs,
    /// Span of elevated activity that produced the detection.
    pub activity_start: TsMs,
    pub activity_end: TsMs,
    /// Raw combined score (sum of weighted normalized signals).
    pub score: f32,
    /// 0..1 calibrated confidence.
    pub confidence: f32,
    pub kind: MomentKind,
    pub reasons: Vec<Reason>,
    #[serde(default)]
    pub top_phrases: Vec<String>,
    #[serde(default)]
    pub top_emotes: Vec<String>,
    #[serde(default)]
    pub unique_chatters: u32,
    #[serde(default)]
    pub messages: u32,
}

impl HighlightEvent {
    pub fn manual(stream_id: impl Into<String>, trigger: TriggerSource, at: TsMs) -> Self {
        HighlightEvent {
            id: Uuid::new_v4(),
            stream_id: stream_id.into(),
            trigger,
            moment_ts: at,
            detected_at: at,
            activity_start: at,
            activity_end: at,
            score: 0.0,
            confidence: 1.0,
            kind: MomentKind::Generic,
            reasons: vec![Reason {
                signal: "manual".into(),
                detail: format!("{} trigger", trigger.label()),
                contribution: 1.0,
            }],
            top_phrases: vec![],
            top_emotes: vec![],
            unique_chatters: 0,
            messages: 0,
        }
    }

    pub fn reason_summary(&self) -> String {
        self.reasons
            .iter()
            .map(|r| r.detail.as_str())
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// A request to preserve footage `[clip_start, clip_end]` (wall clock) from a source.
/// Every trigger (hotkey, voice, chat, audio) produces one of these.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureRequest {
    pub id: Uuid,
    pub event: HighlightEvent,
    pub clip_start: TsMs,
    pub clip_end: TsMs,
}

impl CaptureRequest {
    pub fn around(event: HighlightEvent, pre_roll_ms: i64, post_roll_ms: i64) -> Self {
        CaptureRequest {
            id: Uuid::new_v4(),
            clip_start: event.moment_ts - pre_roll_ms,
            clip_end: event.moment_ts + post_roll_ms,
            event,
        }
    }
    pub fn duration_ms(&self) -> i64 {
        self.clip_end - self.clip_start
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipStatus {
    Capturing,
    Rendering,
    Review,
    Approved,
    Publishing,
    Published,
    Rejected,
    Failed,
}

impl ClipStatus {
    pub fn label(self) -> &'static str {
        match self {
            ClipStatus::Capturing => "Capturing",
            ClipStatus::Rendering => "Rendering",
            ClipStatus::Review => "Needs review",
            ClipStatus::Approved => "Approved",
            ClipStatus::Publishing => "Publishing",
            ClipStatus::Published => "Published",
            ClipStatus::Rejected => "Rejected",
            ClipStatus::Failed => "Failed",
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            ClipStatus::Capturing => "capturing",
            ClipStatus::Rendering => "rendering",
            ClipStatus::Review => "review",
            ClipStatus::Approved => "approved",
            ClipStatus::Publishing => "publishing",
            ClipStatus::Published => "published",
            ClipStatus::Rejected => "rejected",
            ClipStatus::Failed => "failed",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "capturing" => ClipStatus::Capturing,
            "rendering" => ClipStatus::Rendering,
            "review" => ClipStatus::Review,
            "approved" => ClipStatus::Approved,
            "publishing" => ClipStatus::Publishing,
            "published" => ClipStatus::Published,
            "rejected" => ClipStatus::Rejected,
            "failed" => ClipStatus::Failed,
            _ => return None,
        })
    }
}

/// Titles, captions and thumbnails attached to a clip.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClipMetadata {
    pub title: String,
    pub caption: String,
    pub description: String,
    pub hashtags: Vec<String>,
    /// Which generator produced the text: `template` or `local-llm:<model>`.
    #[serde(default)]
    pub generator: String,
}

/// Coverage report for captured footage vs. what was requested. Clips whose event moment is
/// not covered are never silently produced: they are marked with this report.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Coverage {
    pub requested_start: TsMs,
    pub requested_end: TsMs,
    pub actual_start: TsMs,
    pub actual_end: TsMs,
    pub moment_ts: TsMs,
}

impl Coverage {
    pub fn moment_covered(&self) -> bool {
        self.actual_start <= self.moment_ts && self.moment_ts <= self.actual_end
    }
    pub fn full(&self) -> bool {
        // 250 ms tolerance for container/keyframe granularity.
        self.actual_start <= self.requested_start + 250 && self.actual_end >= self.requested_end - 250
    }
    pub fn missing_ms(&self) -> i64 {
        (self.actual_start - self.requested_start).max(0) + (self.requested_end - self.actual_end).max(0)
    }
}

/// Normalized rectangle (0..1 in both axes) relative to the source frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NormRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl NormRect {
    pub fn clamped(self) -> Self {
        let w = self.w.clamp(0.02, 1.0);
        let h = self.h.clamp(0.02, 1.0);
        NormRect {
            x: self.x.clamp(0.0, 1.0 - w),
            y: self.y.clamp(0.0, 1.0 - h),
            w,
            h,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AspectFormat {
    #[default]
    Vertical9x16,
    Landscape16x9,
    Square1x1,
}

impl AspectFormat {
    pub fn label(self) -> &'static str {
        match self {
            AspectFormat::Vertical9x16 => "9:16 vertical",
            AspectFormat::Landscape16x9 => "16:9 landscape",
            AspectFormat::Square1x1 => "1:1 square",
        }
    }
    /// Output size for a given long edge.
    pub fn dims(self, long_edge: u32) -> (u32, u32) {
        let even = |v: u32| v & !1;
        match self {
            AspectFormat::Vertical9x16 => (even(long_edge * 9 / 16), even(long_edge)),
            AspectFormat::Landscape16x9 => (even(long_edge), even(long_edge * 9 / 16)),
            AspectFormat::Square1x1 => (even(long_edge), even(long_edge)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LayoutMode {
    CenterCrop,
    /// Crop centered on the region with the most motion.
    SmartCrop,
    /// Whole frame letterboxed over a blurred, zoomed copy of itself.
    #[default]
    BlurFill,
    /// Facecam stacked above a gameplay crop.
    Facecam,
}

impl LayoutMode {
    pub const ALL: [LayoutMode; 4] = [
        LayoutMode::BlurFill,
        LayoutMode::CenterCrop,
        LayoutMode::SmartCrop,
        LayoutMode::Facecam,
    ];
    pub fn label(self) -> &'static str {
        match self {
            LayoutMode::CenterCrop => "Center crop",
            LayoutMode::SmartCrop => "Smart crop",
            LayoutMode::BlurFill => "Blurred fill",
            LayoutMode::Facecam => "Facecam split",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    FreezeFrame,
    PunchIn,
    SlowMotion,
    SpeedRamp,
    ScreenShake,
    ReactionText,
    MemeOverlay,
    SoundEffect,
    Replay,
    Flash,
}

impl EffectKind {
    pub fn label(self) -> &'static str {
        match self {
            EffectKind::FreezeFrame => "Freeze frame",
            EffectKind::PunchIn => "Punch-in zoom",
            EffectKind::SlowMotion => "Slow motion",
            EffectKind::SpeedRamp => "Speed ramp",
            EffectKind::ScreenShake => "Screen shake",
            EffectKind::ReactionText => "Reaction text",
            EffectKind::MemeOverlay => "Meme overlay",
            EffectKind::SoundEffect => "Sound effect",
            EffectKind::Replay => "Instant replay",
            EffectKind::Flash => "Flash",
        }
    }
    /// Effects that change the timeline (inserted time) vs. pure overlays.
    pub fn alters_timeline(self) -> bool {
        matches!(
            self,
            EffectKind::FreezeFrame | EffectKind::SlowMotion | EffectKind::SpeedRamp | EffectKind::Replay
        )
    }
}

/// One effect placed on the clip timeline. Times are relative to the *source* clip start
/// (before any timeline-altering effect is applied).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectCue {
    pub kind: EffectKind,
    pub start_ms: i64,
    pub end_ms: i64,
    /// 0..1, interpreted per effect (zoom factor, shake amplitude, slow-mo factor...).
    pub intensity: f32,
    #[serde(default)]
    pub sound_asset: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub image_asset: Option<String>,
    pub confidence: f32,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptionStyle {
    pub preset: String,
    pub font_size: f32,
    pub uppercase: bool,
    pub max_words_per_line: usize,
    pub max_lines: usize,
    /// Vertical centre of the caption block as a fraction of output height.
    pub position_y: f32,
    pub text_rgba: [u8; 4],
    pub highlight_rgba: [u8; 4],
    pub outline_rgba: [u8; 4],
    pub outline_px: f32,
    pub shadow: bool,
    /// Pop-in scale animation for the active word.
    pub animate: bool,
}

impl Default for CaptionStyle {
    fn default() -> Self {
        CaptionStyle::preset("bold_pop")
    }
}

impl CaptionStyle {
    pub const PRESETS: [&'static str; 4] = ["bold_pop", "clean", "karaoke", "minimal"];

    pub fn preset(name: &str) -> Self {
        let base = CaptionStyle {
            preset: name.to_string(),
            font_size: 74.0,
            uppercase: true,
            max_words_per_line: 3,
            max_lines: 2,
            position_y: 0.70,
            text_rgba: [255, 255, 255, 255],
            highlight_rgba: [255, 214, 0, 255],
            outline_rgba: [0, 0, 0, 255],
            outline_px: 6.0,
            shadow: true,
            animate: true,
        };
        match name {
            "clean" => CaptionStyle {
                uppercase: false,
                font_size: 62.0,
                highlight_rgba: [255, 255, 255, 255],
                animate: false,
                max_words_per_line: 4,
                ..base
            },
            "karaoke" => CaptionStyle {
                highlight_rgba: [64, 224, 255, 255],
                max_words_per_line: 4,
                animate: false,
                ..base
            },
            "minimal" => CaptionStyle {
                uppercase: false,
                font_size: 52.0,
                outline_px: 3.0,
                shadow: false,
                animate: false,
                highlight_rgba: [255, 255, 255, 255],
                max_words_per_line: 6,
                position_y: 0.80,
                ..base
            },
            _ => base,
        }
    }
}

/// The complete, renderer-independent description of how to turn source footage into a
/// finished clip. Produced by `pulsecut-edit`, consumed by `pulsecut-media::render`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditPlan {
    pub recipe: String,
    pub format: AspectFormat,
    pub width: u32,
    pub height: u32,
    pub layout: LayoutMode,
    /// Horizontal crop centre (0..1) used by CenterCrop/SmartCrop.
    pub crop_center_x: f32,
    pub facecam: Option<NormRect>,
    /// Trim within the source file (ms from file start).
    pub trim_start_ms: i64,
    pub trim_end_ms: i64,
    pub effects: Vec<EffectCue>,
    pub captions: bool,
    pub caption_style: CaptionStyle,
    pub normalize_audio: bool,
    pub watermark_text: Option<String>,
    pub intro_text: Option<String>,
    pub outro_text: Option<String>,
    pub fps: u32,
    pub reasons: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_detects_gaps() {
        let c = Coverage {
            requested_start: 0,
            requested_end: 30_000,
            actual_start: 5_000,
            actual_end: 30_000,
            moment_ts: 20_000,
        };
        assert!(c.moment_covered());
        assert!(!c.full());
        assert_eq!(c.missing_ms(), 5_000);
    }

    #[test]
    fn aspect_dims_even() {
        assert_eq!(AspectFormat::Vertical9x16.dims(1920), (1080, 1920));
        assert_eq!(AspectFormat::Landscape16x9.dims(1280), (1280, 720));
        let (w, h) = AspectFormat::Vertical9x16.dims(1279);
        assert!(w % 2 == 0 && h % 2 == 0);
    }

    #[test]
    fn status_roundtrip() {
        for s in [
            ClipStatus::Capturing,
            ClipStatus::Review,
            ClipStatus::Published,
            ClipStatus::Failed,
        ] {
            assert_eq!(ClipStatus::parse(s.as_str()), Some(s));
        }
    }
}
