//! PulseCut desktop application.

mod app;
mod assets_page;
mod editor;
mod pages;
mod preview;
mod settings_page;
mod theme;
mod widgets;

use pulsecut_core::paths::AppPaths;
use pulsecut_core::settings::Settings;
use pulsecut_engine::{Engine, StartOptions};
use pulsecut_platform::secrets::Keychain;
use std::sync::Arc;

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,obws=warn,sqlx=warn,eframe=warn".into()),
        )
        .with_target(false)
        .init();
    let minimized = std::env::args().any(|a| a == "--minimized");
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .thread_name("pulsecut-rt")
        .build()
        .expect("tokio runtime");
    let paths = AppPaths::discover().expect("app directories");
    let settings = Settings::load(&paths.settings_file()).unwrap_or_default();
    let engine = rt
        .block_on(Engine::start(
            paths,
            settings,
            StartOptions {
                secrets: Arc::new(Keychain),
                obs_password: None,
                start_chat: true,
                start_api: true,
                start_uploads: true,
            },
        ))
        .expect("engine start");
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("PulseCut")
            .with_inner_size([1360.0, 860.0])
            .with_min_inner_size([980.0, 640.0])
            .with_visible(!minimized),
        ..Default::default()
    };
    let e2 = Arc::clone(&engine);
    let r = eframe::run_native(
        "PulseCut",
        opts,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(app::PulseCutApp::new(cc, e2, rt)))
        }),
    );
    engine.shutdown();
    r
}
