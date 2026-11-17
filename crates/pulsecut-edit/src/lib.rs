//! Effect decision engine. Produces an `EditPlan` from what we know about a clip; it never
//! touches media (rendering is `pulsecut-media::render`). Every effect carries a confidence
//! and a human-readable reason, and recipes are deterministic for a given input.
//!
//! Principles (from creator-editing research, see RESEARCH.md §Editing):
//! * Most clips get *no* meme effects; effects need a matching moment type and enough
//!   confidence, scaled by the user's intensity setting.
//! * Effects land on the impact point (loudest audio onset near the detected moment), not on
//!   the chat reaction.
//! * Captions never sit over the gameplay band for the chosen layout.

use pulsecut_core::settings::EditSettings;
use pulsecut_core::{EditPlan, EffectCue, EffectKind, HighlightEvent, LayoutMode, MomentKind, TriggerSource};

/// Everything the planner may use. All times are ms relative to the *source file* start.
#[derive(Debug, Clone)]
pub struct ClipContext {
    pub event: HighlightEvent,
    pub source_len_ms: i64,
    /// Requested window inside the source file.
    pub window_start_ms: i64,
    pub window_end_ms: i64,
    /// Detected moment inside the source file.
    pub moment_ms: i64,
    /// Loudest audio onset near the moment, if any (source-file ms).
    pub audio_onset_ms: Option<i64>,
    /// Motion centre (0..1) for smart crop, with focus (0..1).
    pub motion_center: Option<(f32, f32)>,
    pub source_w: u32,
    pub source_h: u32,
    pub has_speech: bool,
    pub hook_text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoundChoice {
    pub category: &'static str,
    pub preferred: &'static str,
}

/// Sound picks per role. The engine resolves ids against the asset library at render time;
/// missing assets are simply skipped.
fn sound(role: &str) -> &'static str {
    match role {
        "scratch" => "scratch",
        "fail" => "slide_down",
        "buzzer" => "fail_buzzer",
        "boing" => "boing",
        "pop" => "pop",
        "impact" => "impact_boom",
        "horn" => "hype_horn",
        "riser" => "riser",
        "dramatic" => "dramatic_hit",
        "whoosh" => "whoosh",
        "heartbeat" => "heartbeat",
        "ding" => "ding",
        _ => "pop",
    }
}

fn stable_pick<'a>(id: &uuid::Uuid, options: &'a [&'a str]) -> &'a str {
    let b = id.as_bytes();
    options[(b[0] as usize + b[7] as usize) % options.len()]
}

fn reaction_text(ev: &HighlightEvent) -> String {
    // Prefer what chat actually said, if it's short and punchy.
    for p in &ev.top_phrases {
        let t = p.trim();
        let words = t.split_whitespace().count();
        if (1..=3).contains(&words) && t.len() <= 14 && t.chars().any(|c| c.is_alphabetic()) {
            let up = t.to_uppercase();
            if !matches!(up.as_str(), "LOL" | "W" | "L" | "F" | "GG" | "XD") || ev.kind == MomentKind::Fail && up == "F"
            {
                return up;
            }
        }
    }
    let opts: &[&str] = match ev.kind {
        MomentKind::Fail => &["BRUH", "NOT LIKE THIS", "IT'S OVER"],
        MomentKind::Funny => &["I'M DEAD", "BRO WHAT", "NO SHOT"],
        MomentKind::Shock => &["WAIT WHAT", "NO WAY", "HOLD UP"],
        MomentKind::Hype => &["LET'S GO", "INSANE", "HUGE"],
        MomentKind::Clutch => &["CLUTCH", "NO WAY", "HOW?!"],
        MomentKind::Wholesome => &["WHOLESOME", "AWW"],
        MomentKind::Generic => &["WAIT FOR IT"],
    };
    stable_pick(&ev.id, opts).to_string()
}

fn cue(kind: EffectKind, start: i64, end: i64, intensity: f32, confidence: f32, reason: &str) -> EffectCue {
    EffectCue {
        kind,
        start_ms: start,
        end_ms: end.max(start + 40),
        intensity: intensity.clamp(0.0, 1.0),
        sound_asset: None,
        text: None,
        image_asset: None,
        confidence,
        reason: reason.to_string(),
    }
}

fn with_sound(mut c: EffectCue, id: &str) -> EffectCue {
    c.sound_asset = Some(id.to_string());
    c
}

/// Caption vertical position that avoids the gameplay band for a layout.
pub fn caption_position(layout: LayoutMode, vertical: bool, src_aspect: f32, out_aspect: f32) -> f32 {
    if !vertical {
        return 0.84;
    }
    match layout {
        LayoutMode::BlurFill => {
            // The source band occupies the middle; sit in the lower blurred area.
            let band_h = out_aspect / src_aspect; // fraction of height
            (0.5 + band_h / 2.0 + 0.09).min(0.82)
        }
        LayoutMode::Facecam => 0.38, // on the seam between facecam and gameplay
        _ => 0.72,
    }
}

pub struct Planner<'a> {
    pub settings: &'a EditSettings,
}

impl Planner<'_> {
    pub fn plan(&self, ctx: &ClipContext) -> EditPlan {
        let s = self.settings;
        let (w, h) = s.format.dims(s.long_edge);
        let vertical = h > w;
        let mut reasons = vec![];

        // Trim to the requested window, clamped to what exists, max 60 s.
        let mut start = ctx.window_start_ms.clamp(0, ctx.source_len_ms);
        let mut end = ctx.window_end_ms.clamp(start, ctx.source_len_ms);
        if end - start > 60_000 {
            start = (ctx.moment_ms - 40_000).max(start);
            end = (start + 60_000).min(end);
            reasons.push("trimmed to 60 s around the moment".into());
        }

        // Layout.
        let mut layout = s.layout;
        if layout == LayoutMode::Facecam && s.facecam.is_none() {
            layout = LayoutMode::BlurFill;
            reasons.push("facecam layout selected but no facecam region configured; using blurred fill".into());
        }
        let mut crop_center_x = 0.5;
        if layout == LayoutMode::SmartCrop {
            match ctx.motion_center {
                Some((x, focus)) if focus >= 0.25 => {
                    crop_center_x = x;
                    reasons.push(format!("smart crop follows motion at x={x:.2} (focus {focus:.2})"));
                }
                _ => {
                    crop_center_x = 0.5;
                    reasons.push("motion too diffuse for smart crop; centred".into());
                }
            }
        }

        let src_aspect = ctx.source_w.max(1) as f32 / ctx.source_h.max(1) as f32;
        let mut style = s.caption_style.clone();
        style.position_y = caption_position(layout, vertical, src_aspect, w as f32 / h as f32);

        // Effects relative to the trimmed clip.
        let impact_abs = ctx
            .audio_onset_ms
            .filter(|o| (o - ctx.moment_ms).abs() <= 2_500)
            .unwrap_or(ctx.moment_ms);
        let impact = (impact_abs - start).clamp(0, (end - start - 500).max(0));
        let clip_len = end - start;
        let effects = if s.effects_enabled && s.effects_intensity > 0.0 {
            self.recipe(ctx, impact, clip_len, &mut reasons)
        } else {
            reasons.push("effects disabled in settings".into());
            (String::from("clean"), vec![])
        };
        let (recipe, mut effects) = effects;
        if !s.sound_effects {
            for e in &mut effects {
                e.sound_asset = None;
            }
            effects.retain(|e| e.kind != EffectKind::SoundEffect);
        }

        EditPlan {
            recipe,
            format: s.format,
            width: w,
            height: h,
            layout,
            crop_center_x,
            facecam: s.facecam,
            trim_start_ms: start,
            trim_end_ms: end,
            effects,
            captions: s.captions && ctx.has_speech,
            caption_style: style,
            normalize_audio: s.normalize_audio,
            watermark_text: Some(s.watermark_text.clone()).filter(|t| !t.is_empty()),
            intro_text: ctx
                .hook_text
                .clone()
                .filter(|t| !t.is_empty())
                .or_else(|| Some(s.intro_text.clone()).filter(|t| !t.is_empty())),
            outro_text: Some(s.outro_text.clone()).filter(|t| !t.is_empty()),
            fps: s.fps,
            reasons,
        }
    }

    /// Choose a recipe. Returns (name, cues).
    fn recipe(&self, ctx: &ClipContext, t: i64, len: i64, reasons: &mut Vec<String>) -> (String, Vec<EffectCue>) {
        let ev = &ctx.event;
        let k = self.settings.effects_intensity;
        // Gate: confidence must clear a bar that rises as intensity falls.
        let bar = 1.0 - 0.6 * k;
        let strength = match ev.trigger {
            TriggerSource::Hotkey | TriggerSource::Voice | TriggerSource::Manual | TriggerSource::LocalApi => {
                // Manual clips: the streamer chose the moment, but we don't know its type.
                if ev.kind == MomentKind::Generic {
                    0.0
                } else {
                    ev.confidence
                }
            }
            _ => ev.confidence,
        };
        if strength < bar {
            reasons.push(format!(
                "no meme effects: confidence {strength:.2} below {bar:.2} (intensity {k:.1})"
            ));
            return ("clean".into(), vec![]);
        }
        if t < 1_500 || len - t < 2_000 {
            reasons.push("impact too close to clip edge for effects".into());
            return ("clean".into(), vec![]);
        }
        let c = strength;
        let mut v = vec![];
        let name = match ev.kind {
            MomentKind::Fail => {
                v.push(with_sound(
                    cue(EffectKind::FreezeFrame, t, t + 900, k, c, "freeze on the failure"),
                    sound("scratch"),
                ));
                v.push(cue(
                    EffectKind::PunchIn,
                    t - 100,
                    t + 1_400,
                    0.5 + 0.5 * k,
                    c,
                    "punch in on the failure",
                ));
                let mut txt = cue(EffectKind::ReactionText, t, t + 400, 1.0, c, "comedic caption");
                txt.text = Some(reaction_text(ev));
                v.push(txt);
                v.push(with_sound(
                    cue(
                        EffectKind::SoundEffect,
                        t + 50,
                        t + 900,
                        0.8,
                        c,
                        "fail sting under freeze",
                    ),
                    sound("fail"),
                ));
                "fail_freeze"
            }
            MomentKind::Funny => {
                v.push(with_sound(
                    cue(
                        EffectKind::FreezeFrame,
                        t + 300,
                        t + 1_000,
                        k,
                        c,
                        "brief freeze on the reaction",
                    ),
                    sound("boing"),
                ));
                let mut txt = cue(EffectKind::ReactionText, t + 300, t + 700, 1.0, c, "reaction subtitle");
                txt.text = Some(reaction_text(ev));
                v.push(txt);
                if k >= 0.5 {
                    v.push(cue(
                        EffectKind::PunchIn,
                        t + 250,
                        t + 1_600,
                        0.4,
                        c,
                        "light punch-in on the laugh",
                    ));
                }
                "funny_freeze"
            }
            MomentKind::Hype | MomentKind::Clutch => {
                let pre = 1_400.min(t);
                if ev.kind == MomentKind::Clutch && k >= 0.4 && t > 2_500 {
                    v.push(with_sound(
                        cue(
                            EffectKind::SoundEffect,
                            t - 2_200,
                            t,
                            0.6,
                            c,
                            "tension riser into the clutch",
                        ),
                        sound("riser"),
                    ));
                }
                v.push(with_sound(
                    cue(EffectKind::Flash, t, t + 150, 0.8, c, "impact flash"),
                    sound("impact"),
                ));
                v.push(cue(
                    EffectKind::PunchIn,
                    t - 80,
                    t + 1_200,
                    0.5 + 0.4 * k,
                    c,
                    "dramatic zoom on the play",
                ));
                if k >= 0.5 && len - t > 3_500 {
                    v.push(cue(
                        EffectKind::Replay,
                        t - pre,
                        t + 1_000,
                        0.7,
                        c,
                        "slow-motion replay of the play",
                    ));
                    let mut txt = cue(EffectKind::ReactionText, t + 1_000, t + 1_050, 1.0, c, "replay label");
                    txt.text = Some(reaction_text(ev));
                    v.push(txt);
                }
                if ev.kind == MomentKind::Hype {
                    "hype_slowmo"
                } else {
                    "clutch_replay"
                }
            }
            MomentKind::Shock => {
                v.push(with_sound(
                    cue(EffectKind::PunchIn, t - 50, t + 1_300, 0.6 + 0.4 * k, c, "sudden zoom"),
                    sound("dramatic"),
                ));
                v.push(cue(
                    EffectKind::ScreenShake,
                    t,
                    t + 450,
                    0.5 + 0.5 * k,
                    c,
                    "shake on the shock",
                ));
                let mut txt = cue(EffectKind::ReactionText, t + 100, t + 1_300, 1.0, c, "shock caption");
                txt.text = Some(reaction_text(ev));
                v.push(txt);
                "shock_zoom"
            }
            MomentKind::Wholesome => {
                v.push(with_sound(
                    cue(EffectKind::SoundEffect, t, t + 500, 0.5, c, "soft accent"),
                    sound("ding"),
                ));
                "wholesome_soft"
            }
            MomentKind::Generic => {
                reasons.push("moment type unknown; only captions".into());
                return ("clean".into(), vec![]);
            }
        };
        reasons.push(format!("recipe {name}: {} (confidence {c:.2})", ev.kind.label()));
        (name.into(), v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(kind: MomentKind, conf: f32, trig: TriggerSource) -> ClipContext {
        let mut ev = HighlightEvent::manual("t", trig, 0);
        ev.kind = kind;
        ev.confidence = conf;
        ClipContext {
            event: ev,
            source_len_ms: 40_000,
            window_start_ms: 5_000,
            window_end_ms: 35_000,
            moment_ms: 25_000,
            audio_onset_ms: Some(25_600),
            motion_center: Some((0.8, 0.6)),
            source_w: 1920,
            source_h: 1080,
            has_speech: true,
            hook_text: Some("wait for it".into()),
        }
    }

    fn settings() -> EditSettings {
        EditSettings::default()
    }

    #[test]
    fn fail_gets_freeze_on_audio_impact() {
        let s = settings();
        let p = Planner { settings: &s }.plan(&ctx(MomentKind::Fail, 0.9, TriggerSource::Chat));
        assert_eq!(p.recipe, "fail_freeze");
        assert_eq!(p.trim_start_ms, 5_000);
        assert_eq!(p.trim_end_ms - p.trim_start_ms, 30_000);
        let fr = p.effects.iter().find(|e| e.kind == EffectKind::FreezeFrame).unwrap();
        assert_eq!(fr.start_ms, 25_600 - 5_000, "freeze aligned to the audio onset");
        assert!(fr.sound_asset.is_some());
        assert!(p.effects.iter().all(|e| !e.reason.is_empty()));
    }

    #[test]
    fn low_confidence_and_generic_stay_clean() {
        let s = settings();
        let pl = Planner { settings: &s };
        assert_eq!(
            pl.plan(&ctx(MomentKind::Hype, 0.3, TriggerSource::Chat)).recipe,
            "clean"
        );
        assert_eq!(
            pl.plan(&ctx(MomentKind::Generic, 1.0, TriggerSource::Hotkey)).recipe,
            "clean"
        );
    }

    #[test]
    fn intensity_zero_disables() {
        let mut s = settings();
        s.effects_intensity = 0.0;
        let p = Planner { settings: &s }.plan(&ctx(MomentKind::Shock, 1.0, TriggerSource::Chat));
        assert!(p.effects.is_empty());
    }

    #[test]
    fn sounds_can_be_disabled() {
        let mut s = settings();
        s.sound_effects = false;
        let p = Planner { settings: &s }.plan(&ctx(MomentKind::Hype, 0.95, TriggerSource::MultiSignal));
        assert!(!p.effects.is_empty());
        assert!(p.effects.iter().all(|e| e.sound_asset.is_none()));
    }

    #[test]
    fn captions_avoid_gameplay_band() {
        // 16:9 source in 9:16 output: band is 0.5 ± 0.158 of height.
        let y = caption_position(LayoutMode::BlurFill, true, 16.0 / 9.0, 9.0 / 16.0);
        assert!(y > 0.5 + 0.158 + 0.05, "{y}");
        let s = settings();
        let mut c = ctx(MomentKind::Hype, 0.95, TriggerSource::Chat);
        c.has_speech = false;
        assert!(
            !Planner { settings: &s }.plan(&c).captions,
            "no captions without speech"
        );
    }

    #[test]
    fn smart_crop_uses_motion() {
        let mut s = settings();
        s.layout = LayoutMode::SmartCrop;
        let p = Planner { settings: &s }.plan(&ctx(MomentKind::Hype, 0.9, TriggerSource::Chat));
        assert!((p.crop_center_x - 0.8).abs() < 1e-6);
    }

    #[test]
    fn facecam_without_region_falls_back() {
        let mut s = settings();
        s.layout = LayoutMode::Facecam;
        let p = Planner { settings: &s }.plan(&ctx(MomentKind::Hype, 0.9, TriggerSource::Chat));
        assert_eq!(p.layout, LayoutMode::BlurFill);
    }
}
