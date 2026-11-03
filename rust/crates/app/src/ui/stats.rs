//! The connection stats window: how the call and any stream are doing, measured once a second while it's open.

use std::time::{Duration, Instant};

use egui::{self, RichText, Ui};
use swiftcord_voice::Counters;

use crate::app::{Action, App, UiState};
use crate::theme;

type Rows = Vec<(String, String)>;

/// The last reading, and what it was worked out into for showing.
#[derive(Default)]
pub struct Stats {
    last: Option<Reading>,
    pub sections: Vec<(String, Rows)>,
}

struct Reading {
    at: Instant,
    call: Counters,
    share: Counters,
    watch: Counters,
    cpu_ticks: u64,
}

fn kbps(now: u64, before: u64, secs: f64) -> String {
    format!("{:.0} kbps", now.saturating_sub(before) as f64 * 8.0 / 1000.0 / secs)
}

fn rate(now: u64, before: u64, secs: f64) -> f64 {
    now.saturating_sub(before) as f64 / secs
}

fn row(name: &str, value: impl Into<String>) -> (String, String) {
    (name.to_owned(), value.into())
}

/// Rows for video coming in on a connection.
fn incoming_video(rows: &mut Rows, now: &Counters, before: &Counters, secs: f64) {
    if now.video_received_frames == 0 {
        return;
    }
    if now.video_width > 0 {
        rows.push(row("Video size", format!("{}×{}", now.video_width, now.video_height)));
    }
    rows.push(row("Video received", format!("{}, {:.0} fps", kbps(now.video_received_bytes, before.video_received_bytes, secs), rate(now.video_received_frames, before.video_received_frames, secs))));
    let decoded = now.video_decoded_frames.saturating_sub(before.video_decoded_frames);
    if decoded > 0 {
        let micros = now.video_decode_micros.saturating_sub(before.video_decode_micros);
        rows.push(row("Decode time", format!("{:.1} ms per frame", micros as f64 / decoded as f64 / 1000.0)));
    }
    rows.push(row("Video frames lost", now.video_lost_frames.to_string()));
}

/// Takes a new reading if a second has passed, while the window is open.
pub fn tick(app: &mut App) {
    if !app.ui.stats_open {
        app.ui.stats = Stats::default();
        return;
    }
    app.ctx().request_repaint_after(Duration::from_millis(250));
    if app.ui.stats.last.as_ref().is_some_and(|r| r.at.elapsed() < Duration::from_secs(1)) {
        return;
    }
    let call_voice = app.voice.as_ref().and_then(|c| c.voice());
    let share_voice = app.sharing.as_ref().and_then(|s| s.voice());
    let watch_voice = app.watch.as_ref().and_then(|w| w.voice());
    let (cpu_ticks, memory) = crate::platform::process_usage();
    let reading = Reading {
        at: Instant::now(),
        call: call_voice.map(|v| v.counters()).unwrap_or_default(),
        share: share_voice.map(|v| v.counters()).unwrap_or_default(),
        watch: watch_voice.map(|v| v.counters()).unwrap_or_default(),
        cpu_ticks,
    };
    if let Some(last) = &app.ui.stats.last {
        let secs = reading.at.duration_since(last.at).as_secs_f64().max(0.001);
        let mut sections = Vec::new();
        if let Some(v) = call_voice {
            let (now, before) = (&reading.call, &last.call);
            let total = now.audio_received_packets + now.audio_lost_packets;
            let lost = if total > 0 { now.audio_lost_packets as f64 * 100.0 / total as f64 } else { 0.0 };
            let mut rows = vec![
                row("Ping", format!("{} ms", v.ping_ms())),
                row("End-to-end encrypted", if v.privacy_code().is_some() { "Yes" } else { "No" }),
                row("Noise suppression", if app.settings.noise_suppression { "On (RNNoise)" } else { "Off" }),
                row("Audio sent", format!("{}, {:.0} packets/s", kbps(now.audio_sent_bytes, before.audio_sent_bytes, secs), rate(now.audio_sent_packets, before.audio_sent_packets, secs))),
                row("Audio received", format!("{}, {:.0} packets/s", kbps(now.audio_received_bytes, before.audio_received_bytes, secs), rate(now.audio_received_packets, before.audio_received_packets, secs))),
                row("Audio lost", format!("{} packets ({lost:.1}%)", now.audio_lost_packets)),
                row("Audio undecryptable", now.audio_undecryptable.to_string()),
            ];
            if now.video_sent_frames > 0 {
                rows.push(row("Camera sent", format!("{}, {:.0} fps", kbps(now.video_sent_bytes, before.video_sent_bytes, secs), rate(now.video_sent_frames, before.video_sent_frames, secs))));
            }
            incoming_video(&mut rows, now, before, secs);
            sections.push(("Voice".to_owned(), rows));
        }
        if let Some(v) = share_voice {
            let (now, before) = (&reading.share, &last.share);
            let mut rows = vec![row("Encoder", swiftcord_voice::video::ENCODER), row("Ping", format!("{} ms", v.ping_ms()))];
            if let Some(s) = v.share_stats() {
                let budget = 1000.0 / s.target_fps.max(1) as f32;
                rows.push(row("Capture", s.method));
                rows.push(row("Size", format!("{}×{}", s.width, s.height)));
                rows.push(row("Frame rate", format!("{:.1} of {} fps", s.fps, s.target_fps)));
                rows.push(row("Video sent", format!("{} (target {} kbps)", kbps(now.video_sent_bytes, before.video_sent_bytes, secs), s.target_bitrate / 1000)));
                rows.push(row("Time per frame", format!("capture {:.1} ms, convert {:.1} ms, encode {:.1} ms", s.capture_ms, s.convert_ms, s.encode_ms)));
                rows.push(row("Frame budget used", format!("{:.0}% of {budget:.1} ms", (s.capture_ms + s.convert_ms + s.encode_ms) * 100.0 / budget)));
            }
            rows.push(row("Stream audio sent", kbps(now.audio_sent_bytes, before.audio_sent_bytes, secs)));
            sections.push(("Your Stream".to_owned(), rows));
        }
        if let Some(v) = watch_voice {
            let (now, before) = (&reading.watch, &last.watch);
            let mut rows = vec![row("Ping", format!("{} ms", v.ping_ms())), row("Stream audio received", kbps(now.audio_received_bytes, before.audio_received_bytes, secs))];
            incoming_video(&mut rows, now, before, secs);
            sections.push(("Watching a Stream".to_owned(), rows));
        }
        // Process times are in 100 ns units, added up across the machine's processors.
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
        let cpu = reading.cpu_ticks.saturating_sub(last.cpu_ticks) as f64 / 10_000_000.0 / secs;
        sections.push(("This App".to_owned(), vec![row("CPU", format!("{:.1}% of {cores:.0} processors", cpu * 100.0 / cores)), row("Memory", format!("{:.0} MB", memory as f64 / 1_048_576.0))]));
        app.ui.stats.sections = sections;
    }
    app.ui.stats.last = Some(reading);
}

pub fn show(ui: &Ui, uis: &mut UiState, actions: &mut Vec<Action>) {
    if !uis.stats_open {
        return;
    }
    let mut open = true;
    egui::Window::new("Connection Stats").open(&mut open).collapsible(false).resizable(false).default_width(420.0).show(ui.ctx(), |ui| {
        if uis.stats.sections.is_empty() {
            ui.add(egui::Spinner::new());
        }
        for (title, rows) in &uis.stats.sections {
            ui.label(RichText::new(title.to_uppercase()).font(theme::font_semibold(12.0)).color(theme::MUTED()));
            egui::Grid::new(("stats", title)).num_columns(2).spacing([16.0, 2.0]).show(ui, |ui| {
                for (name, value) in rows {
                    ui.label(RichText::new(name).color(theme::MUTED()));
                    ui.label(RichText::new(value).color(theme::TEXT()));
                    ui.end_row();
                }
            });
            ui.add_space(8.0);
        }
        if ui.button("Copy").clicked() {
            let mut text = String::new();
            for (title, rows) in &uis.stats.sections {
                text.push_str(&format!("{title}\n"));
                for (name, value) in rows {
                    text.push_str(&format!("  {name}: {value}\n"));
                }
            }
            actions.push(Action::Copy(text));
        }
    });
    uis.stats_open = open;
}
