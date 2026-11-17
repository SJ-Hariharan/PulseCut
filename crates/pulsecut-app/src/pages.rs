//! Dashboard, Live Monitor, Clip Library and Publishing pages.

use crate::app::{Page, PulseCutApp};
use crate::theme::*;
use crate::widgets::{self, badge, heading, section, stat, status_line};
use egui::{RichText, Ui, vec2};
use pulsecut_core::{ClipStatus, TriggerSource};
use pulsecut_platform::secrets::keys;
use std::sync::Arc;

pub fn dashboard(app: &mut PulseCutApp, ui: &mut Ui) {
    let e = Arc::clone(&app.engine);
    let s = e.settings();
    heading(
        ui,
        "Dashboard",
        "Everything PulseCut is watching, capturing and producing right now.",
    );
    let sys = e.system();
    let clips = e.clips();
    let review = clips.iter().filter(|c| c.status == ClipStatus::Review).count();
    let streams = e.streams();
    let home = streams.iter().find(|v| v.is_home);
    ui.horizontal_wrapped(|ui| {
        stat(
            ui,
            "Chat velocity",
            format!("{:.1}/s", home.map(|h| h.stats.msgs_per_sec).unwrap_or(0.0)),
            INFO,
        );
        stat(
            ui,
            "Unique chatters (60 s)",
            home.map(|h| h.stats.unique_chatters_60s).unwrap_or(0).to_string(),
            ACCENT_2,
        );
        stat(
            ui,
            "Hype score",
            format!("{:.2}", home.map(|h| h.stats.last_score).unwrap_or(0.0)),
            ACCENT,
        );
        stat(ui, "Clips to review", review.to_string(), WARN);
        stat(ui, "Clips total", clips.len().to_string(), TEXT);
    });
    ui.add_space(GAP);
    ui.columns(2, |cols| {
        let ui = &mut cols[0];
        card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            section(ui, "Capture");
            match e.obs_status() {
                Some(o) => {
                    status_line(ui, Some(o.connected), "OBS WebSocket", &format!("{}:{}", s.obs.host, s.obs.port));
                    if o.connected {
                        status_line(ui, Some(o.streaming), "Streaming", if o.streaming { "live" } else { "offline" });
                        status_line(ui, Some(o.recording), "Recording", "");
                        status_line(ui, Some(o.replay_active), "Replay buffer", &o.replay_buffer_secs.map(|b| format!("{b} s")).unwrap_or_default());
                        if o.buffer_too_short {
                            ui.label(RichText::new(format!(
                                "⚠ Replay buffer shorter than the {:.0} s PulseCut needs. Increase it in OBS > Settings > Output > Replay Buffer.",
                                s.required_replay_buffer_secs()
                            )).color(WARN));
                        }
                    }
                    if let Some(err) = o.last_error {
                        ui.label(RichText::new(err).color(BAD).size(12.5));
                    }
                }
                None => match e.segment_status() {
                    Some(seg) => {
                        status_line(ui, Some(seg.running), "Rolling buffer", &format!("{} segments · {:.0} s", seg.segments, seg.buffered_ms as f32 / 1000.0));
                        status_line(ui, Some(seg.anchor_jitter_ms < 1000), "Clock sync", &format!("±{} ms", seg.anchor_jitter_ms));
                        if let Some(err) = seg.last_error {
                            ui.label(RichText::new(err).color(BAD).size(12.5));
                        }
                    }
                    None => {
                        status_line(ui, Some(false), "No capture source", "enable OBS or a stream source in Settings");
                        if ui.button("Open capture settings").clicked() {
                            app.page = Page::Settings;
                            app.settings_tab = 0;
                        }
                    }
                },
            }
            ui.horizontal(|ui| {
                if widgets::primary_button(ui, "Clip now").clicked() {
                    e.clip_now(TriggerSource::Manual);
                    app.toast("Clip requested", true);
                }
                if ui.button("Mark moment").clicked() {
                    e.mark_moment();
                }
            });
            if let Some(err) = &app.hotkey_error {
                ui.label(RichText::new(format!("Hotkeys unavailable: {err}")).color(WARN).size(12.0));
            } else if s.hotkeys.enabled {
                ui.label(RichText::new(format!("Hotkeys: clip {} · mark {}", s.hotkeys.clip, s.hotkeys.mark)).color(MUTED).size(12.0));
            }
        });
        ui.add_space(GAP);
        card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            section(ui, "Detection");
            let mut auto = s.detection.autonomous_enabled;
            if ui.checkbox(&mut auto, "Autonomous clipping").changed() {
                let mut n = e.settings();
                n.detection.autonomous_enabled = auto;
                app.settings_draft.detection.autonomous_enabled = auto;
                if let Err(err) = e.apply_settings(n) {
                    app.toast(err.to_string(), false);
                }
            }
            let mut sens = s.detection.sensitivity;
            ui.horizontal(|ui| {
                ui.label("Sensitivity");
                if ui.add(egui::Slider::new(&mut sens, 0.0..=1.0).show_value(true)).drag_stopped() {
                    let mut n = e.settings();
                    n.detection.sensitivity = sens;
                    app.settings_draft.detection.sensitivity = sens;
                    let _ = e.apply_settings(n);
                }
            });
            for (name, st) in e.chat_states() {
                status_line(ui, Some(matches!(st, pulsecut_chat::SourceState::Connected)), &name, &format!("{st:?}"));
            }
            if e.chat_states().is_empty() {
                ui.label(RichText::new("No chat sources. Add Twitch channels in Settings > Platforms.").color(MUTED));
            }
            ui.label(RichText::new(format!(
                "Audio: mic {} · stream audio {} · voice command {}",
                on(s.audio.mic_enabled), on(s.audio.stream_audio_enabled), on(s.voice.enabled)
            )).color(MUTED).size(12.5));
        });
        let ui = &mut cols[1];
        card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            section(ui, "System");
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("CPU {:.0}%", sys.cpu_percent));
                ui.label(format!("· RAM {} / {} MB (PulseCut {} MB)", sys.mem_used_mb, sys.mem_total_mb, sys.process_mem_mb));
                ui.label(format!("· Disk free {:.1} GB", sys.disk_free_gb));
            });
            ui.label(RichText::new(format!("Encoder: {}  ·  Whisper: {}", e.encoder().ffmpeg_name(),
                if e.tools.whisper_model.is_some() { "ready" } else { "no model" })).color(MUTED).size(12.5));
        });
        ui.add_space(GAP);
        card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            section(ui, "Recent highlights");
            if app.recent.is_empty() {
                ui.label(RichText::new("Nothing yet. Detected moments, manual clips and markers appear here.").color(MUTED));
            }
            for ev in app.recent.iter().take(8) {
                ui.horizontal(|ui| {
                    badge(ui, ev.kind.label(), kind_color(ev.kind));
                    ui.label(RichText::new(pulsecut_core::time::fmt_local(ev.moment_ts)).color(MUTED).size(12.0));
                    ui.label(RichText::new(ev.trigger.label()).strong());
                    ui.label(RichText::new(format!("{:.0}%", ev.confidence * 100.0)).color(MUTED));
                });
                ui.label(RichText::new(ev.reason_summary()).size(12.0).color(MUTED));
            }
        });
        ui.add_space(GAP);
        card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            section(ui, "Activity log");
            for (t, m) in e.logs().iter().rev().take(8) {
                ui.label(RichText::new(format!("{}  {m}", &pulsecut_core::time::fmt_local(*t)[11..])).size(12.0).color(MUTED));
            }
        });
    });
}

fn on(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

pub fn live(app: &mut PulseCutApp, ui: &mut Ui) {
    let e = Arc::clone(&app.engine);
    heading(
        ui,
        "Live Monitor",
        "Signals, the Hype Map and every detection with its explanation.",
    );
    let streams = e.streams();
    if streams.is_empty() {
        widgets::empty_state(
            ui,
            "No live data yet",
            "Add Twitch channels (Settings > Platforms) or enable the microphone.",
        );
        return;
    }
    let sel = app
        .live_stream
        .clone()
        .filter(|s| streams.iter().any(|v| &v.stream_id == s))
        .unwrap_or(streams[0].stream_id.clone());
    ui.horizontal(|ui| {
        ui.label("Stream");
        egui::ComboBox::from_id_salt("live-stream")
            .selected_text(&sel)
            .show_ui(ui, |ui| {
                for v in &streams {
                    ui.selectable_value(&mut app.live_stream, Some(v.stream_id.clone()), &v.stream_id);
                }
            });
    });
    let v = streams.iter().find(|v| v.stream_id == sel).unwrap();
    ui.horizontal_wrapped(|ui| {
        stat(ui, "Messages / s", format!("{:.1}", v.stats.msgs_per_sec), INFO);
        stat(
            ui,
            "Baseline / s",
            format!("{:.2}", v.stats.baseline_msgs_per_sec),
            MUTED,
        );
        stat(
            ui,
            "Unique chatters 60 s",
            v.stats.unique_chatters_60s.to_string(),
            ACCENT_2,
        );
        stat(
            ui,
            "Score / threshold",
            format!("{:.2} / {:.2}", v.stats.last_score, v.stats.threshold),
            ACCENT,
        );
        stat(ui, "Spam filtered", v.stats.suppressed_messages.to_string(), MUTED);
        stat(
            ui,
            "Loudness",
            if v.stats.loudness_db > -89.0 {
                format!("{:.0} dB", v.stats.loudness_db)
            } else {
                "—".into()
            },
            OK,
        );
    });
    ui.add_space(GAP);
    let pts = e.hype_history(&sel, 600);
    let markers: Vec<(i64, egui::Color32)> = app
        .recent
        .iter()
        .filter(|ev| ev.stream_id == sel)
        .map(|ev| (ev.moment_ts / 1000, kind_color(ev.kind)))
        .collect();
    card().show(ui, |ui| {
        section(
            ui,
            "Hype Map — stacked signal contributions, white = score, red = threshold",
        );
        widgets::hype_map(ui, &pts, &markers, 220.0);
        widgets::legend(ui);
    });
    ui.add_space(GAP);
    ui.columns(2, |cols| {
        card().show(&mut cols[0], |ui| {
            ui.set_min_width(ui.available_width());
            section(ui, "Current signal contributions");
            if let Some(last) = pts.last() {
                widgets::signal_bars(ui, &last.comps);
            }
        });
        card().show(&mut cols[1], |ui| {
            ui.set_min_width(ui.available_width());
            section(ui, "Event timeline");
            let evs: Vec<_> = app
                .recent
                .iter()
                .filter(|ev| ev.stream_id == sel)
                .take(12)
                .cloned()
                .collect();
            if evs.is_empty() {
                ui.label(RichText::new("No detections on this stream yet.").color(MUTED));
            }
            for ev in evs {
                ui.horizontal(|ui| {
                    badge(ui, ev.kind.label(), kind_color(ev.kind));
                    ui.label(RichText::new(&pulsecut_core::time::fmt_local(ev.moment_ts)[11..]).monospace());
                    ui.label(format!("{:.0}% · {}", ev.confidence * 100.0, ev.trigger.label()));
                });
                for r in ev.reasons.iter().take(4) {
                    ui.label(RichText::new(format!("   · {}", r.detail)).size(12.0).color(MUTED));
                }
            }
        });
    });
    if streams.len() > 1 {
        ui.add_space(GAP);
        card().show(ui, |ui| {
            section(ui, "Multi-stream leaderboard (hottest streams get capture buffers)");
            egui::Grid::new("leader").striped(true).num_columns(5).show(ui, |ui| {
                for h in ["Stream", "Hype index", "Msgs/s", "Score", "Capturing"] {
                    ui.label(RichText::new(h).color(MUTED));
                }
                ui.end_row();
                for v in &streams {
                    ui.label(&v.stream_id);
                    ui.label(format!("{:.2}", v.hype_index));
                    ui.label(format!("{:.1}", v.stats.msgs_per_sec));
                    ui.label(format!("{:.2}", v.stats.last_score));
                    ui.label(if v.capturing { "yes" } else { "—" });
                    ui.end_row();
                }
            });
        });
    }
}

pub fn library(app: &mut PulseCutApp, ui: &mut Ui) {
    let e = Arc::clone(&app.engine);
    heading(ui, "Clip Library", "Review, edit, approve and publish your clips.");
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut app.search)
                .hint_text("Search titles, transcripts, streams…")
                .desired_width(300.0),
        );
        egui::ComboBox::from_id_salt("status-filter")
            .selected_text(app.status_filter.map(|s| s.label()).unwrap_or("All statuses"))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut app.status_filter, None, "All statuses");
                for s in [
                    ClipStatus::Review,
                    ClipStatus::Approved,
                    ClipStatus::Published,
                    ClipStatus::Rendering,
                    ClipStatus::Failed,
                    ClipStatus::Rejected,
                ] {
                    ui.selectable_value(&mut app.status_filter, Some(s), s.label());
                }
            });
    });
    let q = app.search.to_lowercase();
    let clips: Vec<_> = e
        .clips()
        .into_iter()
        .filter(|c| app.status_filter.is_none_or(|s| s == c.status))
        .filter(|c| {
            q.is_empty()
                || c.title().to_lowercase().contains(&q)
                || c.transcript.to_lowercase().contains(&q)
                || c.stream_id.to_lowercase().contains(&q)
        })
        .collect();
    if clips.is_empty() {
        widgets::empty_state(
            ui,
            "No clips",
            "Clips appear here as soon as a moment is captured and rendered.",
        );
        return;
    }
    let ctx = ui.ctx().clone();
    let detail_w = 400.0;
    ui.horizontal_top(|ui| {
        let left_w = (ui.available_width() - detail_w - 16.0).max(200.0);
        let per_row = ((left_w / 194.0).floor() as usize).max(1);
        ui.vertical(|ui| {
            ui.set_width(left_w);
            for row in clips.chunks(per_row) {
                ui.horizontal(|ui| {
                    for c in row {
                        let sel = app.selected == Some(c.id);
                        let thumb = c.thumb_vertical_path.clone().and_then(|p| app.thumb(&ctx, &p));
                        let resp = card()
                            .stroke(egui::Stroke::new(
                                if sel { 2.0 } else { 1.0 },
                                if sel { ACCENT } else { BORDER },
                            ))
                            .inner_margin(egui::Margin::same(8))
                            .show(ui, |ui| {
                                ui.set_width(170.0);
                                ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                                    match &thumb {
                                        Some(t) => {
                                            ui.add(egui::Image::new((t.id(), widgets::size_of(t, vec2(154.0, 230.0)))));
                                        }
                                        None => {
                                            ui.allocate_ui(vec2(154.0, 230.0), |ui| {
                                                ui.centered_and_justified(|ui| {
                                                    ui.label(RichText::new(c.status.label()).color(MUTED))
                                                });
                                            });
                                        }
                                    }
                                    ui.label(
                                        RichText::new(if c.title().is_empty() { "Untitled" } else { c.title() })
                                            .strong()
                                            .size(13.0),
                                    );
                                    ui.horizontal(|ui| {
                                        badge(ui, c.status.label(), status_color(c.status));
                                        ui.label(
                                            RichText::new(format!("{:.0}s", c.duration_ms as f32 / 1000.0))
                                                .color(MUTED)
                                                .size(12.0),
                                        );
                                    });
                                });
                            })
                            .response
                            .interact(egui::Sense::click());
                        if resp.clicked() {
                            app.selected = Some(c.id);
                        }
                    }
                });
            }
        });
        ui.vertical(|ui| {
            ui.set_width(detail_w);
            let Some(c) = app.selected.and_then(|id| clips.iter().find(|c| c.id == id)).cloned() else {
                card().show(ui, |ui| {
                    ui.label(RichText::new("Select a clip to preview and review it.").color(MUTED));
                });
                return;
            };
            card().show(ui, |ui| {
                if let Some(v) = &c.video_path {
                    crate::preview::show(app, ui, c.id, v, c.duration_ms, vec2(360.0, 420.0));
                } else if let Some(err) = &c.error {
                    ui.label(RichText::new(err).color(BAD));
                }
                ui.horizontal_wrapped(|ui| {
                    badge(ui, c.status.label(), status_color(c.status));
                    badge(ui, c.event.kind.label(), kind_color(c.event.kind));
                    badge(ui, c.event.trigger.label(), MUTED);
                    ui.label(RichText::new(format!("{:.0}% confidence", c.event.confidence * 100.0)).color(MUTED));
                });
                section(ui, "Why it was clipped");
                ui.label(RichText::new(c.event.reason_summary()).size(12.5));
                if let Some(cov) = c.coverage {
                    let ok = cov.full();
                    ui.label(
                        RichText::new(if ok {
                            "✓ full requested window captured".to_string()
                        } else {
                            format!("⚠ {} ms of the requested window missing", cov.missing_ms())
                        })
                        .color(if ok { OK } else { WARN })
                        .size(12.5),
                    );
                }
                section(ui, "Metadata");
                let meta = c.meta.clone().unwrap_or_default();
                if app.edit_meta.as_ref().map(|m| m.0) != Some(c.id) {
                    app.edit_meta = Some((c.id, meta.clone(), meta.hashtags.join(" ")));
                }
                if let Some((_, m, tags)) = app.edit_meta.as_mut() {
                    ui.label("Title");
                    ui.add(egui::TextEdit::singleline(&mut m.title).desired_width(f32::INFINITY));
                    ui.label("Caption");
                    ui.add(
                        egui::TextEdit::multiline(&mut m.caption)
                            .desired_rows(2)
                            .desired_width(f32::INFINITY),
                    );
                    ui.label("Description");
                    ui.add(
                        egui::TextEdit::multiline(&mut m.description)
                            .desired_rows(3)
                            .desired_width(f32::INFINITY),
                    );
                    ui.label("Hashtags");
                    ui.add(egui::TextEdit::singleline(tags).desired_width(f32::INFINITY));
                    ui.label(
                        RichText::new(format!("generated by {}", m.generator))
                            .color(MUTED)
                            .size(11.0),
                    );
                }
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Save metadata").clicked()
                        && let Some((id, mut m, tags)) = app.edit_meta.clone()
                    {
                        m.hashtags = tags
                            .split_whitespace()
                            .map(|t| {
                                if t.starts_with('#') {
                                    t.to_string()
                                } else {
                                    format!("#{t}")
                                }
                            })
                            .collect();
                        let e2 = Arc::clone(&e);
                        app.spawn_action("Metadata", async move {
                            e2.update_meta(id, m).await.map(|_| "saved".into())
                        });
                    }
                    if c.status == ClipStatus::Review && widgets::primary_button(ui, "Approve").clicked() {
                        let (e2, id) = (Arc::clone(&e), c.id);
                        app.spawn_action(
                            "Approve",
                            async move { e2.approve(id).await.map(|_| "approved".into()) },
                        );
                    }
                    if matches!(c.status, ClipStatus::Review | ClipStatus::Approved) && ui.button("Reject").clicked() {
                        let (e2, id) = (Arc::clone(&e), c.id);
                        app.spawn_action("Reject", async move { e2.reject(id).await.map(|_| "rejected".into()) });
                    }
                    if ui.button("Edit").clicked() {
                        app.page = Page::Editor;
                    }
                });
                if matches!(
                    c.status,
                    ClipStatus::Approved | ClipStatus::Published | ClipStatus::Publishing
                ) {
                    section(ui, "Publish");
                    ui.horizontal_wrapped(|ui| {
                        for (p, label) in [
                            ("youtube", "YouTube Shorts"),
                            ("instagram", "Instagram Reels"),
                            ("tiktok", "TikTok (draft)"),
                        ] {
                            if ui.button(format!("Publish to {label}")).clicked() {
                                let (e2, id) = (Arc::clone(&e), c.id);
                                app.spawn_action("Publish", async move { e2.publish(id, p, None).await });
                            }
                        }
                    });
                }
                ui.add_space(6.0);
                if ui.button(RichText::new("Delete clip and files").color(BAD)).clicked() {
                    let (e2, id) = (Arc::clone(&e), c.id);
                    app.selected = None;
                    app.spawn_action(
                        "Delete",
                        async move { e2.delete_clip(id).await.map(|_| "deleted".into()) },
                    );
                }
            });
        });
    });
}

pub fn publishing(app: &mut PulseCutApp, ui: &mut Ui) {
    let e = Arc::clone(&app.engine);
    let s = e.settings();
    heading(
        ui,
        "Publishing",
        "Official APIs only. Nothing is uploaded until you approve it (or enable gated auto-publish).",
    );
    card().show(ui, |ui| {
        section(ui, "Connected accounts");
        let yt = e.account_connected(keys::YOUTUBE_OAUTH);
        ui.horizontal(|ui| {
            status_line(
                ui,
                Some(yt),
                "YouTube",
                if s.publish.youtube_client_id.is_empty() {
                    "set an OAuth client id in Settings > Platforms"
                } else if yt {
                    "connected"
                } else {
                    "not connected"
                },
            );
            if !s.publish.youtube_client_id.is_empty() && ui.button(if yt { "Reconnect" } else { "Connect" }).clicked()
            {
                let e2 = Arc::clone(&e);
                app.spawn_action("YouTube", async move {
                    e2.connect_youtube(|url| {
                        pulsecut_platform::shell::open(url);
                    })
                    .await
                    .map(|_| "connected".into())
                });
            }
        });
        let ig = e.account_connected(keys::INSTAGRAM_TOKEN);
        status_line(
            ui,
            Some(ig && !s.publish.instagram_public_base_url.is_empty()),
            "Instagram",
            if !ig {
                "add a Graph API token in Settings > Platforms"
            } else if s.publish.instagram_public_base_url.is_empty() {
                "needs a public media host"
            } else {
                "ready"
            },
        );
        let tt = e.account_connected(keys::TIKTOK_OAUTH);
        status_line(
            ui,
            Some(tt),
            "TikTok (experimental, inbox upload)",
            if tt { "connected" } else { "not connected" },
        );
        ui.label(RichText::new(format!(
            "Auto-publish: {} (min confidence {:.0}%, max {}/day). Daily caps: YouTube 90, Instagram 45, TikTok 15.",
            if s.publish.auto_publish { "ON" } else { "off" },
            s.publish.auto_publish_min_confidence * 100.0,
            s.publish.auto_publish_max_per_day
        )).color(MUTED).size(12.5));
    });
    ui.add_space(GAP);
    card().show(ui, |ui| {
        section(ui, "Upload queue and history");
        let ups = e.uploads();
        if ups.is_empty() {
            ui.label(
                RichText::new("No uploads yet. Approve a clip in the library, then choose a platform.").color(MUTED),
            );
            return;
        }
        let clips = e.clips();
        let progress = e.upload_progress.lock().unwrap().clone();
        egui::Grid::new("uploads")
            .striped(true)
            .num_columns(6)
            .spacing(vec2(14.0, 8.0))
            .show(ui, |ui| {
                for h in ["Clip", "Platform", "State", "Progress", "Detail", ""] {
                    ui.label(RichText::new(h).color(MUTED));
                }
                ui.end_row();
                for u in ups {
                    let title = clips
                        .iter()
                        .find(|c| c.id == u.clip_id)
                        .map(|c| c.title().to_string())
                        .unwrap_or_else(|| u.title.clone());
                    ui.label(title.chars().take(40).collect::<String>());
                    ui.label(&u.platform);
                    badge(
                        ui,
                        u.state.as_str(),
                        match u.state {
                            pulsecut_store::UploadState::Published => OK,
                            pulsecut_store::UploadState::Failed => BAD,
                            pulsecut_store::UploadState::Uploading => INFO,
                            _ => WARN,
                        },
                    );
                    match progress.get(&u.id) {
                        Some((sent, total)) => {
                            ui.add(
                                egui::ProgressBar::new(*sent as f32 / (*total).max(1) as f32)
                                    .desired_width(140.0)
                                    .show_percentage(),
                            );
                        }
                        None => {
                            ui.label(if u.state == pulsecut_store::UploadState::Published {
                                "100%"
                            } else {
                                "—"
                            });
                        }
                    }
                    ui.label(
                        RichText::new(
                            u.error
                                .clone()
                                .or(u.remote_url.clone())
                                .unwrap_or_default()
                                .chars()
                                .take(70)
                                .collect::<String>(),
                        )
                        .size(12.0)
                        .color(MUTED),
                    );
                    ui.horizontal(|ui| {
                        if u.state == pulsecut_store::UploadState::Failed && ui.button("Retry").clicked() {
                            let (e2, id) = (Arc::clone(&e), u.id);
                            app.spawn_action(
                                "Retry",
                                async move { e2.retry_upload(id).await.map(|_| "re-queued".into()) },
                            );
                        }
                        if let Some(url) = &u.remote_url
                            && url.starts_with("http")
                            && ui.button("Open").clicked()
                        {
                            pulsecut_platform::shell::open(url);
                        }
                    });
                    ui.end_row();
                }
            });
    });
}
