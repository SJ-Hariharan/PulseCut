//! Automatic Editor: inspect and adjust the generated EditPlan, then re-render from raw footage.

use crate::app::PulseCutApp;
use crate::theme::*;
use crate::widgets::{self, badge, heading, section};
use egui::{RichText, Ui, vec2};
use pulsecut_core::{AspectFormat, CaptionStyle, EffectCue, EffectKind, LayoutMode, NormRect};
use pulsecut_media::assets::AssetKind;
use std::sync::Arc;

const ADDABLE: [EffectKind; 10] = [
    EffectKind::PunchIn,
    EffectKind::FreezeFrame,
    EffectKind::SlowMotion,
    EffectKind::Replay,
    EffectKind::SpeedRamp,
    EffectKind::ScreenShake,
    EffectKind::Flash,
    EffectKind::ReactionText,
    EffectKind::MemeOverlay,
    EffectKind::SoundEffect,
];

fn effect_color(k: EffectKind) -> egui::Color32 {
    match k {
        EffectKind::FreezeFrame | EffectKind::Replay => ACCENT_2,
        EffectKind::SlowMotion | EffectKind::SpeedRamp => INFO,
        EffectKind::PunchIn | EffectKind::ScreenShake | EffectKind::Flash => ACCENT,
        EffectKind::ReactionText | EffectKind::MemeOverlay => WARN,
        EffectKind::SoundEffect => OK,
    }
}

pub fn editor(app: &mut PulseCutApp, ui: &mut Ui) {
    let e = Arc::clone(&app.engine);
    heading(
        ui,
        "Automatic Editor",
        "PulseCut picked these edits automatically. Adjust anything and re-render.",
    );
    let clips: Vec<_> = e
        .clips()
        .into_iter()
        .filter(|c| c.plan.is_some() && c.raw_path.is_some())
        .collect();
    if clips.is_empty() {
        widgets::empty_state(ui, "Nothing to edit", "Rendered clips with raw footage appear here.");
        return;
    }
    let cur = app
        .selected
        .filter(|id| clips.iter().any(|c| c.id == *id))
        .unwrap_or(clips[0].id);
    app.selected = Some(cur);
    ui.horizontal(|ui| {
        ui.label("Clip");
        let title = clips
            .iter()
            .find(|c| c.id == cur)
            .map(|c| c.title().to_string())
            .unwrap_or_default();
        egui::ComboBox::from_id_salt("edit-clip")
            .width(420.0)
            .selected_text(title)
            .show_ui(ui, |ui| {
                for c in &clips {
                    ui.selectable_value(
                        &mut app.selected,
                        Some(c.id),
                        format!("{} · {}", c.event.kind.label(), c.title()),
                    );
                }
            });
    });
    let clip = clips.iter().find(|c| Some(c.id) == app.selected).unwrap().clone();
    if app.editor_plan.as_ref().map(|p| p.0) != Some(clip.id) {
        app.editor_plan = Some((clip.id, clip.plan.clone().unwrap()));
    }
    let assets = e.assets();
    let sounds: Vec<(String, String)> = assets
        .all()
        .filter(|a| a.entry.kind == AssetKind::Sound)
        .map(|a| (a.qid(), format!("{} ({})", a.entry.name, a.entry.category)))
        .collect();
    let images: Vec<(String, String)> = assets
        .all()
        .filter(|a| a.entry.kind == AssetKind::Image)
        .map(|a| (a.qid(), a.entry.name.clone()))
        .collect();
    let src_len = clip
        .coverage
        .map(|c| c.actual_end - c.actual_start)
        .unwrap_or(clip.duration_ms)
        .max(1);
    let moment = clip.coverage.map(|c| c.moment_ts - c.actual_start);

    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(380.0);
            card().show(ui, |ui| {
                if let Some(v) = &clip.video_path {
                    crate::preview::show(app, ui, clip.id, v, clip.duration_ms, vec2(340.0, 460.0));
                }
                section(ui, "Decision trail");
                if let Some((_, plan)) = &app.editor_plan {
                    badge(ui, &format!("recipe: {}", plan.recipe), ACCENT_2);
                    for r in &plan.reasons {
                        ui.label(RichText::new(format!("· {r}")).size(12.0).color(MUTED));
                    }
                    for fx in &plan.effects {
                        ui.label(
                            RichText::new(format!(
                                "· {}: {} ({:.0}%)",
                                fx.kind.label(),
                                fx.reason,
                                fx.confidence * 100.0
                            ))
                            .size(12.0)
                            .color(MUTED),
                        );
                    }
                }
            });
        });
        ui.vertical(|ui| {
            let Some((pid, mut plan_owned)) = app.editor_plan.take() else {
                return;
            };
            let plan = &mut plan_owned;
            let mut reset = false;
            card().show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                section(
                    ui,
                    "Timeline (source footage) — click to place the playhead for new effects",
                );
                let blocks: Vec<_> = plan
                    .effects
                    .iter()
                    .map(|f| {
                        (
                            plan.trim_start_ms + f.start_ms,
                            plan.trim_start_ms + f.end_ms,
                            effect_color(f.kind),
                            f.kind.label().to_string(),
                        )
                    })
                    .collect();
                let ph = plan.trim_start_ms + app.player.position_ms();
                if let Some(t) = widgets::timeline_strip(
                    ui,
                    src_len,
                    (plan.trim_start_ms, plan.trim_end_ms),
                    moment,
                    &blocks,
                    Some(ph),
                ) {
                    app.import.playhead_ms = t;
                }
                ui.horizontal(|ui| {
                    ui.label("Trim");
                    ui.add(
                        egui::DragValue::new(&mut plan.trim_start_ms)
                            .range(0..=plan.trim_end_ms - 1000)
                            .speed(50)
                            .suffix(" ms"),
                    );
                    ui.label("to");
                    ui.add(
                        egui::DragValue::new(&mut plan.trim_end_ms)
                            .range(plan.trim_start_ms + 1000..=src_len)
                            .speed(50)
                            .suffix(" ms"),
                    );
                    ui.label(
                        RichText::new(format!(
                            "{:.1} s",
                            (plan.trim_end_ms - plan.trim_start_ms) as f32 / 1000.0
                        ))
                        .color(MUTED),
                    );
                });
            });
            ui.add_space(GAP);
            ui.columns(2, |cols| {
                let ui = &mut cols[0];
                card().show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    section(ui, "Format & framing");
                    egui::ComboBox::from_label("Format")
                        .selected_text(plan.format.label())
                        .show_ui(ui, |ui| {
                            for f in [
                                AspectFormat::Vertical9x16,
                                AspectFormat::Landscape16x9,
                                AspectFormat::Square1x1,
                            ] {
                                if ui.selectable_value(&mut plan.format, f, f.label()).changed() {
                                    let long = plan.width.max(plan.height);
                                    (plan.width, plan.height) = f.dims(long);
                                }
                            }
                        });
                    egui::ComboBox::from_label("Layout")
                        .selected_text(plan.layout.label())
                        .show_ui(ui, |ui| {
                            for l in LayoutMode::ALL {
                                ui.selectable_value(&mut plan.layout, l, l.label());
                            }
                        });
                    if matches!(
                        plan.layout,
                        LayoutMode::SmartCrop | LayoutMode::CenterCrop | LayoutMode::Facecam
                    ) {
                        ui.add(egui::Slider::new(&mut plan.crop_center_x, 0.0..=1.0).text("crop centre"));
                    }
                    if plan.layout == LayoutMode::Facecam {
                        let r = plan.facecam.get_or_insert(NormRect {
                            x: 0.72,
                            y: 0.66,
                            w: 0.26,
                            h: 0.3,
                        });
                        ui.label(
                            RichText::new("Facecam region (fraction of the source frame)")
                                .color(MUTED)
                                .size(12.0),
                        );
                        ui.add(egui::Slider::new(&mut r.x, 0.0..=1.0).text("x"));
                        ui.add(egui::Slider::new(&mut r.y, 0.0..=1.0).text("y"));
                        ui.add(egui::Slider::new(&mut r.w, 0.05..=1.0).text("width"));
                        ui.add(egui::Slider::new(&mut r.h, 0.05..=1.0).text("height"));
                        *r = r.clamped();
                    }
                    ui.checkbox(&mut plan.normalize_audio, "Normalize loudness (−14 LUFS)");
                    section(ui, "Text");
                    let mut hook = plan.intro_text.clone().unwrap_or_default();
                    ui.horizontal(|ui| {
                        ui.label("Hook");
                        if ui.text_edit_singleline(&mut hook).changed() {
                            plan.intro_text = Some(hook.clone()).filter(|h| !h.is_empty());
                        }
                    });
                    let mut wm = plan.watermark_text.clone().unwrap_or_default();
                    ui.horizontal(|ui| {
                        ui.label("Watermark");
                        if ui.text_edit_singleline(&mut wm).changed() {
                            plan.watermark_text = Some(wm.clone()).filter(|h| !h.is_empty());
                        }
                    });
                    let mut outro = plan.outro_text.clone().unwrap_or_default();
                    ui.horizontal(|ui| {
                        ui.label("Outro");
                        if ui.text_edit_singleline(&mut outro).changed() {
                            plan.outro_text = Some(outro.clone()).filter(|h| !h.is_empty());
                        }
                    });
                });
                ui.add_space(GAP);
                card().show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    section(ui, "Captions");
                    ui.checkbox(&mut plan.captions, "Animated word captions");
                    egui::ComboBox::from_label("Style")
                        .selected_text(&plan.caption_style.preset)
                        .show_ui(ui, |ui| {
                            for p in CaptionStyle::PRESETS {
                                if ui.selectable_label(plan.caption_style.preset == p, p).clicked() {
                                    let y = plan.caption_style.position_y;
                                    plan.caption_style = CaptionStyle::preset(p);
                                    plan.caption_style.position_y = y;
                                }
                            }
                        });
                    ui.add(
                        egui::Slider::new(&mut plan.caption_style.position_y, 0.08..=0.92).text("vertical position"),
                    );
                    ui.add(egui::Slider::new(&mut plan.caption_style.font_size, 30.0..=120.0).text("size"));
                    ui.add(egui::Slider::new(&mut plan.caption_style.max_words_per_line, 1..=8).text("words / line"));
                    ui.checkbox(&mut plan.caption_style.uppercase, "UPPERCASE");
                    ui.checkbox(&mut plan.caption_style.animate, "Pop active word");
                });
                let ui = &mut cols[1];
                card().show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    section(ui, "Effects & sound");
                    let mut remove = None;
                    for (i, fx) in plan.effects.iter_mut().enumerate() {
                        egui::Frame::new()
                            .fill(SURFACE_2)
                            .corner_radius(8)
                            .inner_margin(8)
                            .show(ui, |ui| {
                                ui.set_min_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    badge(ui, fx.kind.label(), effect_color(fx.kind));
                                    if ui.small_button("remove").clicked() {
                                        remove = Some(i);
                                    }
                                });
                                ui.horizontal(|ui| {
                                    ui.add(egui::DragValue::new(&mut fx.start_ms).speed(20).suffix(" ms"));
                                    ui.label("to");
                                    ui.add(egui::DragValue::new(&mut fx.end_ms).speed(20).suffix(" ms"));
                                    ui.add(egui::Slider::new(&mut fx.intensity, 0.0..=1.0).text("int."));
                                });
                                if matches!(fx.kind, EffectKind::ReactionText) {
                                    let mut t = fx.text.clone().unwrap_or_default();
                                    if ui.text_edit_singleline(&mut t).changed() {
                                        fx.text = Some(t);
                                    }
                                }
                                if fx.kind == EffectKind::MemeOverlay {
                                    egui::ComboBox::from_id_salt(("img", i))
                                        .selected_text(fx.image_asset.clone().unwrap_or("choose image…".into()))
                                        .show_ui(ui, |ui| {
                                            for (q, n) in &images {
                                                ui.selectable_value(&mut fx.image_asset, Some(q.clone()), n);
                                            }
                                            if images.is_empty() {
                                                ui.label("Import images in Assets first");
                                            }
                                        });
                                }
                                egui::ComboBox::from_id_salt(("snd", i))
                                    .selected_text(fx.sound_asset.clone().unwrap_or("no sound".into()))
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(&mut fx.sound_asset, None, "no sound");
                                        for (q, n) in &sounds {
                                            ui.selectable_value(&mut fx.sound_asset, Some(q.clone()), n);
                                        }
                                    });
                            });
                    }
                    if let Some(i) = remove {
                        plan.effects.remove(i);
                    }
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("add-fx")
                            .selected_text(app.new_effect.label())
                            .show_ui(ui, |ui| {
                                for k in ADDABLE {
                                    ui.selectable_value(&mut app.new_effect, k, k.label());
                                }
                            });
                        if ui.button("+ Add at playhead").clicked() {
                            let at = (app.import.playhead_ms - plan.trim_start_ms)
                                .clamp(0, plan.trim_end_ms - plan.trim_start_ms - 500);
                            let len = match app.new_effect {
                                EffectKind::Flash => 150,
                                EffectKind::ScreenShake => 450,
                                EffectKind::FreezeFrame => 900,
                                _ => 1200,
                            };
                            plan.effects.push(EffectCue {
                                kind: app.new_effect,
                                start_ms: at,
                                end_ms: at + len,
                                intensity: 0.6,
                                sound_asset: None,
                                text: (app.new_effect == EffectKind::ReactionText).then(|| "NO WAY".into()),
                                image_asset: None,
                                confidence: 1.0,
                                reason: "added in editor".into(),
                            });
                        }
                    });
                    ui.label(
                        RichText::new("Times are relative to the trimmed clip start.")
                            .color(MUTED)
                            .size(11.5),
                    );
                });
            });
            ui.add_space(GAP);
            ui.horizontal(|ui| {
                if widgets::primary_button(ui, "Re-render clip").clicked() {
                    let (e2, id, p) = (Arc::clone(&e), clip.id, plan.clone());
                    app.player = Default::default();
                    app.spawn_action("Re-render", async move {
                        e2.rerender(id, Some(p)).await.map(|_| "queued".into())
                    });
                }
                if ui.button("Reset to automatic plan").clicked() {
                    reset = true;
                }
            });
            if !reset {
                app.editor_plan = Some((pid, plan_owned));
            }
        });
    });
}
