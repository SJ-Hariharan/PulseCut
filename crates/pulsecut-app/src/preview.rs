//! In-app preview player: decodes a low-res, low-fps silent proxy of a clip through FFmpeg off
//! the UI thread (bounded memory: ≤ 240 frames at 216x384), with play/pause and scrubbing.
//! "Open in player" hands the real file (with audio) to the OS player.

use crate::app::{PulseCutApp, UiMsg};
use crate::theme::*;
use egui::{RichText, TextureHandle, Ui, vec2};
use uuid::Uuid;

#[derive(Default)]
pub struct Player {
    pub clip: Option<Uuid>,
    frames: Vec<TextureHandle>,
    fps: f32,
    loading: bool,
    playing: bool,
    pos: f32,
    last_t: f64,
}

impl Player {
    pub fn playing(&self) -> bool {
        self.playing
    }

    pub fn loaded(&mut self, ctx: &egui::Context, id: Uuid, frames: Vec<egui::ColorImage>, fps: f32) {
        if self.clip != Some(id) {
            return;
        }
        self.frames = frames
            .into_iter()
            .enumerate()
            .map(|(i, f)| ctx.load_texture(format!("pv-{id}-{i}"), f, egui::TextureOptions::LINEAR))
            .collect();
        self.fps = fps;
        self.loading = false;
        self.pos = 0.0;
    }

    pub fn position_ms(&self) -> i64 {
        if self.fps <= 0.0 {
            0
        } else {
            (self.pos / self.fps * 1000.0) as i64
        }
    }
}

/// Show the player for a clip video (vertical or landscape).
pub fn show(app: &mut PulseCutApp, ui: &mut Ui, id: Uuid, video: &str, duration_ms: i64, max: egui::Vec2) {
    if app.player.clip != Some(id) {
        app.player = Player {
            clip: Some(id),
            loading: true,
            ..Default::default()
        };
        let (tools, path, tx, ctx) = (
            app.engine.tools.clone(),
            std::path::PathBuf::from(video),
            app.tx.clone(),
            ui.ctx().clone(),
        );
        let fps = (240.0 / (duration_ms.max(1000) as f32 / 1000.0)).clamp(2.0, 8.0);
        app.rt.spawn(async move {
            let info = pulsecut_media::probe::probe(&tools, &path).await.ok();
            let (w, h) = match info {
                Some(i) if i.width > i.height => (384, 216),
                _ => (216, 384),
            };
            let frames = pulsecut_media::frames::decode_rgba(&tools, &path, 0, 240, fps, w, h)
                .await
                .unwrap_or_default();
            let imgs = frames
                .into_iter()
                .map(|f| egui::ColorImage::from_rgba_unmultiplied([f.width() as usize, f.height() as usize], &f))
                .collect();
            let _ = tx.send(UiMsg::Frames(id, imgs, fps));
            ctx.request_repaint();
        });
    }
    let p = &mut app.player;
    card()
        .fill(egui::Color32::BLACK)
        .inner_margin(egui::Margin::same(6))
        .show(ui, |ui| {
            if p.loading || p.frames.is_empty() {
                ui.allocate_ui(vec2(max.x.min(220.0), max.y.min(390.0)), |ui| {
                    ui.centered_and_justified(|ui| {
                        ui.label(RichText::new(if p.loading { "Loading preview…" } else { "No preview" }).color(MUTED));
                    });
                });
                return;
            }
            let now = ui.ctx().input(|i| i.time);
            if p.playing {
                let dt = (now - p.last_t) as f32;
                p.pos += dt * p.fps;
                if p.pos >= p.frames.len() as f32 {
                    p.pos = 0.0;
                    p.playing = false;
                }
                ui.ctx().request_repaint();
            }
            p.last_t = now;
            let tex = &p.frames[(p.pos as usize).min(p.frames.len() - 1)];
            let size = crate::widgets::size_of(tex, max);
            ui.add(egui::Image::new((tex.id(), size)));
        });
    ui.horizontal(|ui| {
        if ui.button(if p.playing { "Pause" } else { "Play" }).clicked() {
            p.playing = !p.playing;
        }
        let n = p.frames.len().max(1) as f32;
        let mut pos = p.pos;
        if ui
            .add(egui::Slider::new(&mut pos, 0.0..=n - 1.0).show_value(false))
            .changed()
        {
            p.pos = pos;
            p.playing = false;
        }
        ui.label(
            RichText::new(pulsecut_core::time::fmt_dur_ms(p.position_ms()))
                .color(MUTED)
                .monospace(),
        );
    });
    ui.horizontal(|ui| {
        if ui.button("Open in player (with audio)").clicked() {
            pulsecut_platform::shell::open(video);
        }
        if ui.button("Show in folder").clicked() {
            pulsecut_platform::shell::reveal(std::path::Path::new(video));
        }
    });
}
