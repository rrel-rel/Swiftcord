// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod clip;
mod demo;
mod emoji_sheet;
mod image_decode;
mod inbox_glue;
mod interactions_glue;
mod lottie;
mod stickers;
mod media;
mod notifications;
mod session_cache;
mod settings;
mod window;
mod sounds;
mod search_glue;
mod share_glue;
mod threads_glue;
mod stream_glue;
mod platform;
mod player;
mod theme;
mod ui;
mod voice_glue;
mod audio;
mod perf;
mod image_cache;

use std::fs::{self, File};
use std::sync::Mutex;


fn init_logging() {
    let dir = platform::data_dir().join("logs");
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("swiftcord.log");
    // Keep the previous run's log for bug reports.
    let _ = fs::rename(&path, dir.join("swiftcord.old.log"));
    // SWIFTCORD_LOG takes "debug", or per-module levels like "info,swiftcord=debug".
    let filter: tracing_subscriber::filter::Targets = std::env::var("SWIFTCORD_LOG")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| tracing_subscriber::filter::Targets::new().with_default(tracing::Level::INFO));
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let layer = tracing_subscriber::fmt::layer().with_ansi(false).with_target(false);
    match File::create(&path) {
        Ok(f) => tracing_subscriber::registry().with(layer.with_writer(Mutex::new(f))).with(filter).init(),
        Err(_) => tracing_subscriber::registry().with(layer).with(filter).init(),
    }
    std::panic::set_hook(Box::new(|info| {
        tracing::error!("panic: {info}\n{}", std::backtrace::Backtrace::force_capture());
    }));
}

fn main() -> anyhow::Result<()> {
    if std::env::args().any(|a| a == platform::login_webview::ARG) {
        platform::login_webview::run_child();
    }
    if std::env::args().any(|a| a == platform::captcha_webview::ARG) {
        platform::captcha_webview::run_child();
    }
    if std::env::args().any(|a| a == platform::video_window::ARG) {
        platform::video_window::run_child();
    }

    init_logging();
    tracing::info!("Swiftcord {} starting", env!("CARGO_PKG_VERSION"));
    // Before anything connects: Swiftcord identifies as the desktop client installed here.
    platform::discord_install::apply();
    platform::register_app_id();

    // Two workers are plenty for a chat client's network I/O.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("swiftcord-io")
        .enable_all()
        .build()
        .expect("Tokio runtime");

    let viewport = egui::ViewportBuilder::default()
        .with_title("Swiftcord")
        .with_inner_size([1100.0, 680.0])
        .with_min_inner_size([760.0, 460.0]);
    let handle = rt.handle().clone();
    let _ = clip::RUNTIME.set(handle.clone());
    let result = window::run(viewport, move |ctx| Box::new(app::App::new(ctx, handle)));
    tracing::info!("exiting");
    rt.shutdown_timeout(std::time::Duration::from_millis(500));
    Ok(result?)
}
