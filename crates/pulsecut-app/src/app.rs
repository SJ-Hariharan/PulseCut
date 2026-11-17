//! App shell: navigation, shared state, background-task plumbing, hotkeys, toasts.

use crate::preview::Player;
use crate::theme::*;
use crate::widgets;
use egui::{RichText, TextureHandle, Ui};
use pulsecut_core::{EditPlan, HighlightEvent, TriggerSource};
use pulsecut_engine::{Engine, EngineEvent};
use pulsecut_platform::hotkeys::{HotkeyAction, Hotkeys};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Dashboard,
    Live,
    Library,
    Editor,
    Assets,
    Publishing,
    Settings,
}

impl Page {
    pub const ALL: [(Page, &'static str, &'static str); 7] = [
        (Page::Dashboard, "", "Dashboard"),
        (Page::Live, "", "Live Monitor"),
        (Page::Library, "", "Clip Library"),
        (Page::Editor, "", "Editor"),
        (Page::Assets, "", "Assets"),
        (Page::Publishing, "", "Publishing"),
        (Page::Settings, "", "Settings"),
    ];
}

/// Results of background work delivered back to the UI thread.
pub enum UiMsg {
    Toast(String, bool),
    Thumb(PathBuf, Option<egui::ColorImage>),
    Frames(Uuid, Vec<egui::ColorImage>, f32),
}

pub struct PulseCutApp {
    pub engine: Arc<Engine>,
    pub rt: tokio::runtime::Runtime,
    pub page: Page,
    pub tx: Sender<UiMsg>,
    rx: Receiver<UiMsg>,
    events_rx: tokio::sync::broadcast::Receiver<EngineEvent>,
    pub toasts: VecDeque<(f64, String, bool)>,
    pub recent: VecDeque<HighlightEvent>,
    hotkeys: Option<Hotkeys>,
    pub hotkey_error: Option<String>,
    pub thumbs: HashMap<PathBuf, Option<TextureHandle>>,
    pub selected: Option<Uuid>,
    pub player: Player,
    // page state
    pub search: String,
    pub status_filter: Option<pulsecut_core::ClipStatus>,
    pub live_stream: Option<String>,
    /// Text field for adding a streamer to the watchlist.
    pub new_streamer: String,
    pub editor_plan: Option<(Uuid, EditPlan)>,
    pub edit_meta: Option<(Uuid, pulsecut_core::ClipMetadata, String)>,
    pub settings_draft: pulsecut_core::Settings,
    pub settings_dirty: bool,
    pub settings_tab: usize,
    pub secret_inputs: HashMap<&'static str, String>,
    pub asset_query: String,
    pub asset_category: String,
    pub import: crate::assets_page::ImportForm,
    pub new_effect: pulsecut_core::EffectKind,
}

impl PulseCutApp {
    pub fn new(cc: &eframe::CreationContext<'_>, engine: Arc<Engine>, rt: tokio::runtime::Runtime) -> Self {
        let (tx, rx) = channel();
        let s = engine.settings();
        let (hotkeys, hotkey_error) = if s.hotkeys.enabled {
            match Hotkeys::register(&s.hotkeys.clip, &s.hotkeys.mark) {
                Ok(h) => (Some(h), None),
                Err(e) => (None, Some(e.to_string())),
            }
        } else {
            (None, None)
        };
        // Wake the UI when the engine has news.
        let ctx = cc.egui_ctx.clone();
        let mut sub = engine.subscribe();
        rt.spawn(async move {
            while sub.recv().await.is_ok() {
                ctx.request_repaint();
            }
        });
        let events_rx = engine.subscribe();
        let recent: VecDeque<HighlightEvent> = rt.block_on(engine.recent_events(30)).into();
        PulseCutApp {
            settings_draft: s,
            engine,
            rt,
            page: Page::Dashboard,
            tx,
            rx,
            events_rx,
            toasts: VecDeque::new(),
            recent,
            hotkeys,
            hotkey_error,
            thumbs: HashMap::new(),
            selected: None,
            player: Player::default(),
            search: String::new(),
            status_filter: None,
            live_stream: None,
            new_streamer: String::new(),
            editor_plan: None,
            edit_meta: None,
            settings_dirty: false,
            settings_tab: 0,
            secret_inputs: HashMap::new(),
            asset_query: String::new(),
            asset_category: String::new(),
            import: Default::default(),
            new_effect: pulsecut_core::EffectKind::PunchIn,
        }
    }

    pub fn toast(&mut self, msg: impl Into<String>, ok: bool) {
        self.toasts.push_back((0.0, msg.into(), ok));
    }

    /// Run an engine future in the background and toast its outcome.
    pub fn spawn_action<F>(&self, label: &'static str, fut: F)
    where
        F: std::future::Future<Output = pulsecut_core::Result<String>> + Send + 'static,
    {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let r = fut.await;
            let _ = tx.send(match r {
                Ok(m) if m.is_empty() => UiMsg::Toast(format!("{label}: done"), true),
                Ok(m) => UiMsg::Toast(format!("{label}: {m}"), true),
                Err(e) => UiMsg::Toast(format!("{label} failed: {e}"), false),
            });
        });
    }

    /// Lazily load a thumbnail texture (decoded off the UI thread).
    pub fn thumb(&mut self, ctx: &egui::Context, path: &str) -> Option<TextureHandle> {
        let p = PathBuf::from(path);
        if let Some(t) = self.thumbs.get(&p) {
            return t.clone();
        }
        self.thumbs.insert(p.clone(), None);
        let tx = self.tx.clone();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let img = image::open(&p).ok().map(|i| {
                let i = i.thumbnail(480, 480).to_rgba8();
                egui::ColorImage::from_rgba_unmultiplied([i.width() as usize, i.height() as usize], &i)
            });
            let _ = tx.send(UiMsg::Thumb(p, img));
            c.request_repaint();
        });
        None
    }

    fn pump(&mut self, ctx: &egui::Context) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                UiMsg::Toast(t, ok) => self.toast(t, ok),
                UiMsg::Thumb(p, img) => {
                    let tex = img.map(|i| ctx.load_texture(p.display().to_string(), i, egui::TextureOptions::LINEAR));
                    self.thumbs.insert(p, tex);
                }
                UiMsg::Frames(id, frames, fps) => self.player.loaded(ctx, id, frames, fps),
            }
        }
        while let Ok(ev) = self.events_rx.try_recv() {
            match ev {
                EngineEvent::Highlight(h) => {
                    if self.recent.len() >= 60 {
                        self.recent.pop_back();
                    }
                    self.recent.push_front(h);
                }
                EngineEvent::CaptureFailed { error, .. } => self.toast(format!("Capture failed: {error}"), false),
                EngineEvent::Error(e) => self.toast(e, false),
                _ => {}
            }
        }
        if let Some(h) = &self.hotkeys {
            for a in h.poll() {
                match a {
                    HotkeyAction::Clip => {
                        self.engine.clip_now(TriggerSource::Hotkey);
                        self.toast("Clip requested (hotkey)", true);
                    }
                    HotkeyAction::Mark => {
                        self.engine.mark_moment();
                        self.toast("Marker added", true);
                    }
                }
            }
        }
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            widgets::dot(ui, ACCENT);
            ui.label(RichText::new("PulseCut").size(20.0).strong());
        });
        ui.add_space(14.0);
        for (p, _, name) in Page::ALL {
            let sel = self.page == p;
            let text = RichText::new(format!("{}{name}", if sel { "| " } else { "  " }))
                .size(14.5)
                .color(if sel { TEXT } else { MUTED });
            let b = egui::Button::new(text)
                .fill(if sel { SURFACE_2 } else { egui::Color32::TRANSPARENT })
                .stroke(egui::Stroke::NONE)
                .min_size(egui::vec2(ui.available_width(), 34.0));
            if ui.add(b).clicked() {
                self.page = p;
            }
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            ui.add_space(10.0);
            let obs = self.engine.obs_status();
            let seg = self.engine.segment_status();
            let (ok, label) = match (&obs, &seg) {
                (Some(o), _) if o.connected => (true, if o.streaming { "OBS · live" } else { "OBS · connected" }),
                (Some(_), _) => (false, "OBS · offline"),
                (None, Some(s)) if s.running => (true, "Buffer · recording"),
                (None, Some(_)) => (false, "Buffer · stopped"),
                (None, None) => (false, "No capture source"),
            };
            widgets::status_line(ui, Some(ok), label, "");
            if self.engine.capture_paused() {
                widgets::status_line(ui, Some(false), "Disk low", "capture paused");
            }
            if widgets::primary_button(ui, "Clip now").clicked() {
                self.engine.clip_now(TriggerSource::Manual);
                self.toast("Clip requested", true);
            }
        });
    }

    fn toasts(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        for t in self.toasts.iter_mut() {
            if t.0 == 0.0 {
                t.0 = now;
            }
        }
        self.toasts.retain(|t| now - t.0 < 5.0);
        if self.toasts.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::unique("pulsecut-toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -16.0))
            .show(ctx, |ui| {
                for (_, msg, ok) in self.toasts.iter() {
                    card()
                        .fill(if *ok { SURFACE_2 } else { BAD.linear_multiply(0.25) })
                        .show(ui, |ui| {
                            ui.set_max_width(380.0);
                            ui.label(RichText::new(msg).color(if *ok { TEXT } else { egui::Color32::WHITE }));
                        });
                }
            });
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
}

impl eframe::App for PulseCutApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.pump(&ctx);
        // Live pages refresh once per second without busy-looping.
        ctx.request_repaint_after(std::time::Duration::from_millis(if self.player.playing() {
            33
        } else {
            1000
        }));
        egui::Panel::left("nav")
            .exact_size(210.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(SURFACE)
                    .inner_margin(egui::Margin::symmetric(12, 8)),
            )
            .show(ui, |ui| self.sidebar(ui));
        egui::CentralPanel::default_margins()
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(22, 14)),
            )
            .show(ui, |ui| {
                let width = ui.available_width();
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.set_max_width(width);
                    match self.page {
                        Page::Dashboard => crate::pages::dashboard(self, ui),
                        Page::Live => crate::pages::live(self, ui),
                        Page::Library => crate::pages::library(self, ui),
                        Page::Editor => crate::editor::editor(self, ui),
                        Page::Assets => crate::assets_page::assets(self, ui),
                        Page::Publishing => crate::pages::publishing(self, ui),
                        Page::Settings => crate::settings_page::settings(self, ui),
                    }
                });
            });
        self.toasts(&ctx);
    }
}
