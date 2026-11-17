//! Settings. Edits a draft; "Save & apply" persists settings.toml and applies changes live.
//! Secrets are written straight to the OS keychain and never shown again.

use crate::app::PulseCutApp;
use crate::theme::*;
use crate::widgets::{self, heading, section};
use egui::{RichText, Ui};
use pulsecut_core::settings::{CaptureBackend, StreamSource};
use pulsecut_core::{AspectFormat, CaptionStyle, LayoutMode};
use pulsecut_platform::secrets::keys;
use std::sync::Arc;

const TABS: [&str; 9] = [
    "Capture & OBS",
    "Platforms",
    "Audio & Voice",
    "Hotkeys",
    "Detection",
    "Editing",
    "Storage",
    "Startup & Privacy",
    "Resources",
];

fn secret_row(app: &mut PulseCutApp, ui: &mut Ui, label: &str, key: &'static str) {
    let e = Arc::clone(&app.engine);
    let set = e.account_connected(key);
    ui.horizontal(|ui| {
        ui.label(label);
        let v = app.secret_inputs.entry(key).or_default();
        ui.add(
            egui::TextEdit::singleline(v)
                .password(true)
                .hint_text(if set {
                    "•••• stored in keychain"
                } else {
                    "not set"
                })
                .desired_width(240.0),
        );
        if ui.button("Save to keychain").clicked() {
            let val = std::mem::take(v);
            match if val.is_empty() {
                e.secrets.delete(key)
            } else {
                e.secrets.set(key, &val)
            } {
                Ok(()) => app.toast(
                    format!("{label} {}", if val.is_empty() { "removed" } else { "saved" }),
                    true,
                ),
                Err(err) => app.toast(err.to_string(), false),
            }
        }
    });
}

fn list_edit(ui: &mut Ui, label: &str, v: &mut Vec<String>, hint: &str) -> bool {
    let mut s = v.join(", ");
    ui.label(label);
    let ch = ui
        .add(
            egui::TextEdit::singleline(&mut s)
                .hint_text(hint)
                .desired_width(f32::INFINITY),
        )
        .changed();
    if ch {
        *v = s
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect();
    }
    ch
}

pub fn settings(app: &mut PulseCutApp, ui: &mut Ui) {
    heading(
        ui,
        "Settings",
        "Changes apply live when saved. Secrets go to the OS keychain.",
    );
    ui.horizontal_wrapped(|ui| {
        for (i, t) in TABS.iter().enumerate() {
            if ui.selectable_label(app.settings_tab == i, *t).clicked() {
                app.settings_tab = i;
            }
        }
    });
    ui.add_space(GAP);
    let before = app.settings_draft.clone();
    card().show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        match app.settings_tab {
            0 => capture_tab(app, ui),
            1 => platforms_tab(app, ui),
            2 => audio_tab(app, ui),
            3 => hotkeys_tab(app, ui),
            4 => detection_tab(app, ui),
            5 => editing_tab(app, ui),
            6 => storage_tab(app, ui),
            7 => startup_tab(app, ui),
            _ => resources_tab(app, ui),
        }
    });
    if app.settings_draft != before {
        app.settings_dirty = true;
    }
    ui.add_space(GAP);
    ui.horizontal(|ui| {
        let label = if app.settings_dirty { "Save & apply" } else { "Saved" };
        if widgets::primary_button(ui, label).clicked() && app.settings_dirty {
            let s = app.settings_draft.clone();
            match app.engine.apply_settings(s) {
                Ok(()) => {
                    app.settings_dirty = false;
                    app.toast("Settings saved", true);
                }
                Err(e) => app.toast(e.to_string(), false),
            }
        }
        if app.settings_dirty && ui.button("Discard changes").clicked() {
            app.settings_draft = app.engine.settings();
            app.settings_dirty = false;
        }
        ui.label(
            RichText::new(app.engine.paths.settings_file().display().to_string())
                .color(MUTED)
                .size(11.5),
        );
    });
}

fn capture_tab(app: &mut PulseCutApp, ui: &mut Ui) {
    let s = &mut app.settings_draft;
    section(ui, "Capture backend");
    ui.radio_value(
        &mut s.capture.backend,
        CaptureBackend::ObsReplayBuffer,
        "OBS replay buffer (Streamer mode)",
    );
    ui.radio_value(
        &mut s.capture.backend,
        CaptureBackend::SegmentBuffer,
        "PulseCut rolling buffer from a URL/device (Autonomous mode)",
    );
    ui.add(egui::Slider::new(&mut s.capture.pre_roll_secs, 3.0..=120.0).text("seconds before the moment"));
    ui.add(egui::Slider::new(&mut s.capture.post_roll_secs, 2.0..=60.0).text("seconds after the moment"));
    ui.add(egui::Slider::new(&mut s.capture.max_clip_secs, 10.0..=180.0).text("max clip length (s)"));
    ui.add(egui::Slider::new(&mut s.capture.save_margin_secs, 0.5..=10.0).text("save margin (s)"));
    ui.label(
        RichText::new(format!(
            "OBS replay buffer must be at least {:.0} s (pre + post + chat reaction delay + margin).",
            s.required_replay_buffer_secs()
        ))
        .color(MUTED)
        .size(12.5),
    );
    section(ui, "OBS WebSocket");
    ui.checkbox(&mut s.obs.enabled, "Connect to OBS");
    ui.horizontal(|ui| {
        ui.label("Host");
        ui.text_edit_singleline(&mut s.obs.host);
        ui.label("Port");
        ui.add(egui::DragValue::new(&mut s.obs.port));
    });
    ui.checkbox(
        &mut s.obs.use_password,
        "Use password (OBS > Tools > WebSocket Server Settings)",
    );
    ui.checkbox(
        &mut s.obs.auto_start_replay_buffer,
        "Start the replay buffer automatically",
    );
    secret_row(app, ui, "OBS password", keys::OBS_PASSWORD);
    let s = &mut app.settings_draft;
    section(ui, "Rolling buffer source");
    ui.label("FFmpeg input: rtmp://, srt://, https://….m3u8, device:<name>, file:<path> or lavfi:test");
    ui.text_edit_singleline(&mut s.capture.segment_source);
    ui.checkbox(
        &mut s.capture.segment_copy_codec,
        "Copy the incoming codec (no re-encode while buffering)",
    );
    ui.add(egui::Slider::new(&mut s.capture.segment_ring_secs, 60..=1800).text("buffer length (s)"));
    section(ui, "Multi-stream capture (Autonomous)");
    ui.add(egui::Slider::new(&mut s.capture.max_parallel_buffers, 0..=4).text("max simultaneous stream buffers"));
    let mut remove = None;
    for (i, src) in s.capture.extra_sources.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut src.stream_id)
                    .hint_text("twitch:channel")
                    .desired_width(160.0),
            );
            ui.add(
                egui::TextEdit::singleline(&mut src.input)
                    .hint_text("HLS/RTMP URL")
                    .desired_width(320.0),
            );
            if ui.small_button("remove").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        s.capture.extra_sources.remove(i);
    }
    if ui.button("+ Add stream source").clicked() {
        s.capture.extra_sources.push(StreamSource::default());
    }
    ui.label(RichText::new("Only buffer streams you have the right to clip. Chat for every listed channel is analysed; the hottest get buffers.").color(MUTED).size(12.0));
}

fn platforms_tab(app: &mut PulseCutApp, ui: &mut Ui) {
    let s = &mut app.settings_draft;
    section(ui, "Chat sources");
    list_edit(
        ui,
        "Twitch channels (first = your channel; read-only, no login needed)",
        &mut s.chat.twitch_channels,
        "mychannel, friend1, friend2",
    );
    list_edit(ui, "YouTube live video IDs", &mut s.chat.youtube_targets, "dQw4w9WgXcQ");
    list_edit(
        ui,
        "Extra reaction words",
        &mut s.chat.custom_reactions,
        "myemote1, catJAM",
    );
    secret_row(app, ui, "YouTube Data API key", keys::YOUTUBE_API_KEY);
    secret_row(app, ui, "Kick public key (PEM)", "kick-public-key");
    let s = &mut app.settings_draft;
    section(ui, "YouTube publishing");
    ui.checkbox(&mut s.publish.youtube_enabled, "Enable YouTube Shorts");
    ui.horizontal(|ui| {
        ui.label("OAuth client id (Desktop app)");
        ui.text_edit_singleline(&mut s.publish.youtube_client_id);
    });
    egui::ComboBox::from_label("Privacy")
        .selected_text(&s.publish.youtube_privacy)
        .show_ui(ui, |ui| {
            for p in ["private", "unlisted", "public"] {
                ui.selectable_value(&mut s.publish.youtube_privacy, p.to_string(), p);
            }
        });
    secret_row(app, ui, "OAuth client secret", keys::YOUTUBE_CLIENT_SECRET);
    let s = &mut app.settings_draft;
    section(ui, "Instagram Reels");
    ui.checkbox(&mut s.publish.instagram_enabled, "Enable Instagram");
    ui.horizontal(|ui| {
        ui.label("IG user id");
        ui.text_edit_singleline(&mut s.publish.instagram_user_id);
    });
    ui.horizontal(|ui| {
        ui.label("Public media folder");
        ui.text_edit_singleline(&mut s.publish.media_host_dir);
    });
    ui.horizontal(|ui| {
        ui.label("Served at (https://…)");
        ui.text_edit_singleline(&mut s.publish.instagram_public_base_url);
    });
    ui.label(
        RichText::new("Instagram downloads the video from a public URL; hosting may cost money and is optional.")
            .color(MUTED)
            .size(12.0),
    );
    secret_row(app, ui, "Instagram Graph token", keys::INSTAGRAM_TOKEN);
    let s = &mut app.settings_draft;
    section(ui, "TikTok (experimental)");
    ui.checkbox(&mut s.publish.tiktok_enabled, "Enable TikTok inbox upload");
    ui.horizontal(|ui| {
        ui.label("Client key");
        ui.text_edit_singleline(&mut s.publish.tiktok_client_key);
    });
    secret_row(app, ui, "TikTok client secret", keys::TIKTOK_CLIENT_SECRET);
    let s = &mut app.settings_draft;
    section(ui, "Auto-publish (off by default)");
    ui.checkbox(
        &mut s.publish.auto_publish,
        "Publish high-confidence autonomous clips without review",
    );
    ui.add(egui::Slider::new(&mut s.publish.auto_publish_min_confidence, 0.5..=1.0).text("minimum confidence"));
    ui.add(
        egui::Slider::new(&mut s.publish.auto_publish_max_per_day, 1..=90)
            .text("max per day per platform (YouTube stops at 90, Instagram at 45)"),
    );
}

fn audio_tab(app: &mut PulseCutApp, ui: &mut Ui) {
    let s = &mut app.settings_draft;
    section(ui, "Microphone");
    ui.checkbox(
        &mut s.audio.mic_enabled,
        "Analyse my microphone for excitement and laughter",
    );
    let devices = pulsecut_platform::mic::list_inputs();
    egui::ComboBox::from_label("Input device")
        .selected_text(if s.audio.mic_device.is_empty() {
            "System default"
        } else {
            &s.audio.mic_device
        })
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut s.audio.mic_device, String::new(), "System default");
            for d in devices {
                ui.selectable_value(&mut s.audio.mic_device, d.clone(), d);
            }
        });
    ui.checkbox(
        &mut s.audio.stream_audio_enabled,
        "Analyse the captured stream's audio (rolling buffer)",
    );
    ui.label(
        RichText::new("Audio is analysed locally and never recorded or uploaded by the microphone feature.")
            .color(MUTED)
            .size(12.0),
    );
    section(ui, "Voice command");
    ui.checkbox(
        &mut s.voice.enabled,
        "Clip when I say a trigger phrase (uses the microphone)",
    );
    list_edit(ui, "Trigger phrases", &mut s.voice.phrases, "clip it, clip that");
    ui.add(egui::Slider::new(&mut s.voice.max_chunk_secs, 1.0..=6.0).text("max utterance length (s)"));
    ui.label(RichText::new("Short utterances are transcribed with whisper.cpp only when speech is detected (ggml-tiny.en is used if present).").color(MUTED).size(12.0));
}

fn hotkeys_tab(app: &mut PulseCutApp, ui: &mut Ui) {
    let s = &mut app.settings_draft;
    ui.checkbox(
        &mut s.hotkeys.enabled,
        "Global hotkeys (work while other apps are focused)",
    );
    for (label, v) in [("Clip", &mut s.hotkeys.clip), ("Mark moment", &mut s.hotkeys.mark)] {
        ui.horizontal(|ui| {
            ui.label(label);
            ui.text_edit_singleline(v);
            match pulsecut_platform::hotkeys::parse(v) {
                Ok(_) => ui.label(RichText::new("✓").color(OK)),
                Err(e) => ui.label(RichText::new(e.to_string()).color(BAD).size(12.0)),
            };
        });
    }
    ui.label(
        RichText::new("Format: CmdOrCtrl+Shift+K, Alt+F9 … Restart PulseCut after changing hotkeys.")
            .color(MUTED)
            .size(12.0),
    );
    ui.label(
        RichText::new(format!(
            "Local API for Stream Deck / bots: POST http://127.0.0.1:{}/api/clip",
            s.resources.local_api_port
        ))
        .color(MUTED)
        .size(12.0),
    );
}

fn detection_tab(app: &mut PulseCutApp, ui: &mut Ui) {
    let s = &mut app.settings_draft;
    ui.checkbox(&mut s.detection.autonomous_enabled, "Autonomous clipping");
    ui.add(egui::Slider::new(&mut s.detection.sensitivity, 0.0..=1.0).text("sensitivity"));
    ui.add(egui::Slider::new(&mut s.detection.cooldown_secs, 0.0..=300.0).text("min seconds between separate clips"));
    ui.add(egui::Slider::new(&mut s.detection.chat_reaction_delay_secs, 0.0..=20.0).text("chat reaction delay (s)"));
    ui.label(
        RichText::new("Stream latency + human reaction time. Low-latency Twitch ≈ 4–7 s; normal latency ≈ 8–12 s.")
            .color(MUTED)
            .size(12.0),
    );
    ui.checkbox(&mut s.detection.use_chat, "Use chat signals");
    ui.checkbox(&mut s.detection.use_audio, "Use audio signals");
    ui.add(egui::Slider::new(&mut s.detection.max_clips_per_hour, 1..=120).text("max clips per hour"));
    ui.horizontal(|ui| {
        ui.label("Tuned weights file (from `pulsecut-cli eval --tune --out`)");
        ui.text_edit_singleline(&mut s.detection.weights_file);
    });
}

fn editing_tab(app: &mut PulseCutApp, ui: &mut Ui) {
    let s = &mut app.settings_draft;
    section(ui, "Output");
    egui::ComboBox::from_label("Format")
        .selected_text(s.edit.format.label())
        .show_ui(ui, |ui| {
            for f in [
                AspectFormat::Vertical9x16,
                AspectFormat::Landscape16x9,
                AspectFormat::Square1x1,
            ] {
                ui.selectable_value(&mut s.edit.format, f, f.label());
            }
        });
    egui::ComboBox::from_label("Resolution (long edge)")
        .selected_text(s.edit.long_edge.to_string())
        .show_ui(ui, |ui| {
            for r in [1280u32, 1920, 2560] {
                ui.selectable_value(&mut s.edit.long_edge, r, r.to_string());
            }
        });
    egui::ComboBox::from_label("Layout")
        .selected_text(s.edit.layout.label())
        .show_ui(ui, |ui| {
            for l in LayoutMode::ALL {
                ui.selectable_value(&mut s.edit.layout, l, l.label());
            }
        });
    if s.edit.layout == LayoutMode::Facecam {
        let r = s.edit.facecam.get_or_insert(pulsecut_core::NormRect {
            x: 0.72,
            y: 0.66,
            w: 0.26,
            h: 0.3,
        });
        ui.add(egui::Slider::new(&mut r.x, 0.0..=1.0).text("facecam x"));
        ui.add(egui::Slider::new(&mut r.y, 0.0..=1.0).text("facecam y"));
        ui.add(egui::Slider::new(&mut r.w, 0.05..=1.0).text("facecam width"));
        ui.add(egui::Slider::new(&mut r.h, 0.05..=1.0).text("facecam height"));
    }
    ui.add(egui::Slider::new(&mut s.edit.fps, 24..=60).text("fps"));
    ui.checkbox(
        &mut s.edit.hardware_encode,
        "Use the hardware encoder when its test encode passes",
    );
    ui.checkbox(&mut s.edit.also_landscape, "Also export 16:9");
    ui.checkbox(&mut s.edit.normalize_audio, "Normalize loudness");
    section(ui, "Captions");
    ui.checkbox(&mut s.edit.captions, "Animated captions (local whisper.cpp)");
    egui::ComboBox::from_label("Caption style")
        .selected_text(&s.edit.caption_style.preset)
        .show_ui(ui, |ui| {
            for p in CaptionStyle::PRESETS {
                if ui.selectable_label(s.edit.caption_style.preset == p, p).clicked() {
                    s.edit.caption_style = CaptionStyle::preset(p);
                }
            }
        });
    ui.horizontal(|ui| {
        ui.label("Font file (empty = system bold font)");
        ui.text_edit_singleline(&mut s.edit.font_path);
    });
    section(ui, "Meme effects & sound");
    ui.checkbox(
        &mut s.edit.effects_enabled,
        "Context-aware effects (freeze, zoom, slow-mo, replay…)",
    );
    ui.add(egui::Slider::new(&mut s.edit.effects_intensity, 0.0..=1.0).text("effect intensity / frequency"));
    ui.checkbox(&mut s.edit.sound_effects, "Sound effects");
    ui.add(egui::Slider::new(&mut s.edit.sfx_volume, 0.0..=1.5).text("sound effect volume"));
    section(ui, "Branding");
    for (label, v) in [
        ("Watermark", &mut s.edit.watermark_text),
        ("Default hook", &mut s.edit.intro_text),
        ("Outro", &mut s.edit.outro_text),
    ] {
        ui.horizontal(|ui| {
            ui.label(label);
            ui.text_edit_singleline(v);
        });
    }
    ui.horizontal(|ui| {
        ui.label("Creator name (titles, thumbnails)");
        ui.text_edit_singleline(&mut s.meta.creator_name);
    });
    list_edit(ui, "Default hashtags", &mut s.meta.default_hashtags, "#shorts, #gaming");
    ui.checkbox(&mut s.meta.thumbnail_text, "Title text on thumbnails");
    ui.horizontal(|ui| {
        ui.label("Optional local LLM (OpenAI-compatible, loopback only)");
        ui.text_edit_singleline(&mut s.meta.local_llm_url);
    });
}

fn storage_tab(app: &mut PulseCutApp, ui: &mut Ui) {
    let s = &mut app.settings_draft;
    ui.horizontal(|ui| {
        ui.label("Library folder (empty = default)");
        ui.text_edit_singleline(&mut s.storage.library_dir);
    });
    ui.add(
        egui::Slider::new(&mut s.storage.retention_days, 0..=365)
            .text("delete published/rejected clips after (days, 0 = never)"),
    );
    ui.add(egui::Slider::new(&mut s.storage.min_free_gb, 1.0..=100.0).text("pause capture below free GB"));
    ui.checkbox(
        &mut s.storage.delete_raw_after_render,
        "Delete raw footage after rendering (disables re-render)",
    );
    if ui.button("Open data folder").clicked() {
        pulsecut_platform::shell::open(&app.engine.paths.data_dir.display().to_string());
    }
}

fn startup_tab(app: &mut PulseCutApp, ui: &mut Ui) {
    let s = &mut app.settings_draft;
    section(ui, "Startup");
    let mut login = pulsecut_platform::autostart::is_enabled();
    if ui.checkbox(&mut login, "Launch PulseCut at login").changed() {
        match pulsecut_platform::autostart::set_enabled(login, s.startup.start_minimized) {
            Ok(()) => s.startup.launch_at_login = login,
            Err(e) => app.toasts.push_back((0.0, e.to_string(), false)),
        }
    }
    let s = &mut app.settings_draft;
    ui.checkbox(&mut s.startup.start_minimized, "Start hidden");
    section(ui, "Privacy");
    ui.label("PulseCut processes everything on this computer. It has no telemetry or analytics backend.");
    ui.checkbox(&mut s.privacy.local_crash_logs, "Keep local crash logs");
    ui.label(
        RichText::new("Network use: chat sources you configure, uploads you approve, and OAuth sign-in. Nothing else.")
            .color(MUTED)
            .size(12.0),
    );
}

fn resources_tab(app: &mut PulseCutApp, ui: &mut Ui) {
    let s = &mut app.settings_draft;
    ui.add(egui::Slider::new(&mut s.resources.max_render_jobs, 1..=4).text("concurrent renders (restart to apply)"));
    ui.add(egui::Slider::new(&mut s.resources.ffmpeg_threads, 0..=16).text("FFmpeg threads (0 = auto)"));
    ui.add(egui::Slider::new(&mut s.resources.render_queue, 1..=64).text("render queue size"));
    ui.add(
        egui::Slider::new(&mut s.resources.min_free_mem_mb_for_whisper, 200..=4000)
            .text("skip transcription below free RAM (MB)"),
    );
    ui.horizontal(|ui| {
        ui.label("Local API port (0 = off)");
        ui.add(egui::DragValue::new(&mut s.resources.local_api_port));
    });
    section(ui, "Encoders (tested on this machine)");
    for p in app.engine.encoder_probes() {
        ui.label(format!(
            "{} — {} ({} ms)",
            p.encoder.ffmpeg_name(),
            if p.ok { "works" } else { "failed" },
            p.ms
        ));
    }
}
