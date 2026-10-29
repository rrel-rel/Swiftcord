//! A diagnostic for finding what keeps the window redrawing. With `SWIFTCORD_PERF` set, the log gets, every
//! five seconds, the frame rate, the average time spent building a frame, and what asked for the redraws.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use eframe::egui;

const EVERY: Duration = Duration::from_secs(5);

pub struct Perf {
    on: bool,
    since: Instant,
    frames: u32,
    busy: Duration,
    causes: HashMap<String, u32>,
    events: HashMap<String, u32>,
}

impl Default for Perf {
    fn default() -> Self {
        Self { on: std::env::var_os("SWIFTCORD_PERF").is_some(), since: Instant::now(), frames: 0, busy: Duration::ZERO, causes: HashMap::new(), events: HashMap::new() }
    }
}

impl Perf {
    /// Counts an event from the gateway, by kind.
    pub fn event(&mut self, event: &impl std::fmt::Debug) {
        if self.on {
            let name = format!("{event:?}");
            let kind = name.split(['(', ' ', '{']).next().unwrap_or_default().to_owned();
            *self.events.entry(kind).or_default() += 1;
        }
    }

    /// Counts a frame that took `took` to build.
    pub fn frame(&mut self, ctx: &egui::Context, took: Duration) {
        if !self.on {
            return;
        }
        self.frames += 1;
        self.busy += took;
        for c in ctx.repaint_causes() {
            *self.causes.entry(c.to_string()).or_default() += 1;
        }
        let elapsed = self.since.elapsed();
        if elapsed < EVERY {
            return;
        }
        let mut causes: Vec<_> = self.causes.drain().collect();
        causes.sort_by(|a, b| b.1.cmp(&a.1));
        let top: Vec<String> = causes.iter().take(6).map(|(c, n)| format!("{n}× {c}")).collect();
        let mut events: Vec<_> = self.events.drain().collect();
        events.sort_by(|a, b| b.1.cmp(&a.1));
        let events: Vec<String> = events.iter().take(8).map(|(e, n)| format!("{n}× {e}")).collect();
        tracing::info!(
            "perf: {:.1} frames/s, {:.2} ms per frame; redraws asked for by {}; gateway events: {}",
            self.frames as f64 / elapsed.as_secs_f64(),
            self.busy.as_secs_f64() * 1000.0 / self.frames.max(1) as f64,
            if top.is_empty() { "input".to_owned() } else { top.join(", ") },
            events.join(", "),
        );
        self.frames = 0;
        self.busy = Duration::ZERO;
        self.since = Instant::now();
    }
}
