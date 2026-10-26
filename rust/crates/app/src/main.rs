// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod clip;
mod demo;
mod emoji_sheet;
mod lottie;
mod stickers;
mod media;
mod settings;
mod share_glue;
mod threads_glue;
mod stream_glue;
mod platform;
mod player;
mod theme;
mod ui;
mod voice_glue;

use std::fs::{self, File};
use std::sync::Mutex;

use eframe::egui;

fn init_logging() {
    let dir = platform::data_dir().join("logs");
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("swiftcord.log");
    // Keep the previous run's log for bug reports.
    let _ = fs::rename(&path, dir.join("swiftcord.old.log"));
    let filter = tracing_subscriber::EnvFilter::try_from_env("SWIFTCORD_LOG")
        .unwrap_or_else(|_| "info,wgpu_core=warn,wgpu_hal=warn,naga=warn".into());
    let builder = tracing_subscriber::fmt().with_env_filter(filter).with_ansi(false).with_target(false);
    match File::create(&path) {
        Ok(f) => builder.with_writer(Mutex::new(f)).init(),
        Err(_) => builder.init(),
    }
    std::panic::set_hook(Box::new(|info| {
        tracing::error!("panic: {info}\n{}", std::backtrace::Backtrace::force_capture());
    }));
}

fn main() -> eframe::Result {
    if std::env::args().any(|a| a == platform::login_webview::ARG) {
        platform::login_webview::run_child();
    }
    if std::env::args().any(|a| a == platform::video_window::ARG) {
        platform::video_window::run_child();
    }

    init_logging();
    tracing::info!("Swiftcord {} starting", env!("CARGO_PKG_VERSION"));
    platform::register_app_id();

    // Two workers are plenty for a chat client's network I/O.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("swiftcord-io")
        .enable_all()
        .build()
        .expect("Tokio runtime");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Swiftcord")
            .with_app_id("Swiftcord")
            .with_inner_size([1100.0, 680.0])
            .with_min_inner_size([760.0, 460.0]),
        renderer: eframe::Renderer::Wgpu,
        centered: true,
        ..Default::default()
    };
    let handle = rt.handle().clone();
    let _ = clip::RUNTIME.set(handle.clone());
    let result = eframe::run_native("Swiftcord", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, handle)))));
    tracing::info!("exiting");
    rt.shutdown_timeout(std::time::Duration::from_millis(500));
    result
}
