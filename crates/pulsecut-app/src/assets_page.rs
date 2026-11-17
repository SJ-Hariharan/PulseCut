//! Asset Library: sounds, meme images, fonts and creator packs with licensing metadata.

use crate::app::PulseCutApp;
use crate::theme::*;
use crate::widgets::{badge, heading, section};
use egui::{RichText, Ui};
use pulsecut_media::assets::{AssetKind, import_user_asset};
use std::sync::Arc;

#[derive(Default)]
pub struct ImportForm {
    pub path: String,
    pub name: String,
    pub category: String,
    pub license: String,
    pub attribution: String,
    pub playhead_ms: i64,
}

pub fn assets(app: &mut PulseCutApp, ui: &mut Ui) {
    let e = Arc::clone(&app.engine);
    heading(
        ui,
        "Asset Library",
        "Sounds, meme images, fonts and creator packs. Every asset carries its license.",
    );
    let lib = e.assets();
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut app.asset_query)
                .hint_text("Search name, category, tag…")
                .desired_width(260.0),
        );
        egui::ComboBox::from_id_salt("acat")
            .selected_text(if app.asset_category.is_empty() {
                "All categories"
            } else {
                &app.asset_category
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut app.asset_category, String::new(), "All categories");
                for c in [
                    "comedy",
                    "dramatic",
                    "impact",
                    "suspense",
                    "reaction",
                    "transition",
                    "meme",
                ] {
                    ui.selectable_value(&mut app.asset_category, c.to_string(), c);
                }
            });
        if ui.button("Open assets folder").clicked() {
            pulsecut_platform::shell::open(&e.paths.assets_dir().display().to_string());
        }
        if ui.button("Reload packs").clicked() {
            e.reload_assets();
            app.toast("Asset packs reloaded", true);
        }
    });
    ui.add_space(GAP);
    ui.columns(2, |cols| {
        card().show(&mut cols[0], |ui| {
            ui.set_min_width(ui.available_width());
            section(ui, "Assets");
            let ffplay = e.tools.ffmpeg.as_ref().map(|p| p.with_file_name(if cfg!(windows) { "ffplay.exe" } else { "ffplay" })).filter(|p| p.is_file());
            let mut any = false;
            for a in lib.search(&app.asset_query, None) {
                if !app.asset_category.is_empty() && a.entry.category != app.asset_category {
                    continue;
                }
                any = true;
                ui.horizontal(|ui| {
                    badge(ui, match a.entry.kind { AssetKind::Sound => "sound", AssetKind::Image => "image", AssetKind::Font => "font" }, match a.entry.kind {
                        AssetKind::Sound => OK,
                        AssetKind::Image => WARN,
                        AssetKind::Font => INFO,
                    });
                    ui.label(RichText::new(&a.entry.name).strong());
                    ui.label(RichText::new(&a.entry.category).color(MUTED));
                    if a.entry.kind == AssetKind::Sound && ui.small_button("Preview").clicked() {
                        let vol = e.settings().edit.sfx_volume * a.entry.volume;
                        if !pulsecut_platform::shell::play_sound(ffplay.as_deref(), &a.path, vol) {
                            app.toast("Could not start an audio player", false);
                        }
                    }
                });
                ui.label(RichText::new(format!(
                    "   {} · license {}{} · {}",
                    a.pack.manifest.name,
                    a.license(),
                    a.entry.attribution.as_ref().map(|x| format!(" · by {x}")).unwrap_or_default(),
                    a.qid()
                )).size(11.5).color(MUTED));
            }
            if !any {
                ui.label(RichText::new("No assets match.").color(MUTED));
            }
        });
        let ui = &mut cols[1];
        card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            section(ui, "Packs");
            for p in &lib.packs {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&p.manifest.name).strong());
                    badge(ui, &p.manifest.license, OK);
                    ui.label(RichText::new(format!("{} assets · v{}", p.manifest.assets.len(), p.manifest.version)).color(MUTED));
                });
                ui.label(RichText::new(&p.manifest.description).size(12.0).color(MUTED));
            }
            ui.label(RichText::new("Install a creator pack by copying its folder (with pack.json) into the assets folder, then Reload.").size(12.0).color(MUTED));
        });
        ui.add_space(GAP);
        card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            section(ui, "Import your own asset");
            let f = &mut app.import;
            ui.label("File path (WAV/MP3/OGG/FLAC, PNG/JPG/GIF, TTF/OTF; ≤ 25 MB)");
            ui.text_edit_singleline(&mut f.path);
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut f.name);
            });
            ui.horizontal(|ui| {
                ui.label("Category");
                ui.text_edit_singleline(&mut f.category);
            });
            ui.horizontal(|ui| {
                ui.label("License / usage rights");
                ui.text_edit_singleline(&mut f.license);
            });
            ui.horizontal(|ui| {
                ui.label("Attribution");
                ui.text_edit_singleline(&mut f.attribution);
            });
            ui.label(RichText::new("Only import media you own or have the rights to use. A license statement is required.").color(WARN).size(12.0));
            if ui.button("Import").clicked() {
                let r = import_user_asset(
                    &e.paths.assets_dir(),
                    std::path::Path::new(f.path.trim()),
                    if f.name.is_empty() { "imported" } else { &f.name },
                    if f.category.is_empty() { "meme" } else { &f.category },
                    &f.license,
                    Some(f.attribution.clone()).filter(|a| !a.is_empty()),
                );
                match r {
                    Ok(id) => {
                        e.reload_assets();
                        *f = ImportForm::default();
                        app.toast(format!("Imported {id}"), true);
                    }
                    Err(err) => app.toast(format!("Import failed: {err}"), false),
                }
            }
        });
        ui.add_space(GAP);
        card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            section(ui, "Fonts");
            match e.font() {
                Some(f) => ui.label(format!("Caption & title font: {}", f.path.display())),
                None => ui.label(RichText::new("No bold font found; set one in Settings > Editing.").color(BAD)),
            };
            ui.label(RichText::new("Fonts are used from your system and never bundled.").size(12.0).color(MUTED));
        });
    });
}
