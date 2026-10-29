//! A web player (YouTube and the like) playing inside its message, as Discord shows it: a WebView2 child of
//! our window, kept over the embed's video area as the message list moves. A native window would cover
//! anything egui draws over it, so it's hidden while the area is out of view or a menu or dialog is open on
//! top; it closes when its channel is left or it's been out of sight a while.

use std::time::{Duration, Instant};

use wry::raw_window_handle::HasWindowHandle;
use wry::{dpi, Rect, WebViewBuilder, WebViewBuilderExtWindows};

use crate::platform::video_window::{player_page, player_url, site};

/// Out of sight this long (scrolled away, or covered), it stops.
const KEEP_HIDDEN: Duration = Duration::from_secs(30);

pub struct InlinePlayer {
    /// The embed's player URL, which identifies it in the message list.
    pub url: String,
    webview: wry::WebView,
    _context: wry::WebContext,
    shown: Option<[i32; 4]>,
    hidden_since: Option<Instant>,
}

impl InlinePlayer {
    pub fn new(parent: &impl HasWindowHandle, url: &str) -> Result<InlinePlayer, String> {
        let mut context = wry::WebContext::new(Some(crate::platform::data_dir().join("WebView2-video")));
        // As in the video window: players refuse to load unless a page embeds them, so the player goes in an
        // iframe on a page of our own (https://swiftcord.localhost/).
        let page = player_page(&player_url(url));
        let home = site(url).unwrap_or_default().to_owned();
        let webview = WebViewBuilder::new_with_web_context(&mut context)
            .with_custom_protocol("swiftcord".into(), move |_, _| {
                wry::http::Response::builder().header("Content-Type", "text/html; charset=utf-8").body(std::borrow::Cow::Owned(page.clone().into_bytes())).expect("response")
            })
            .with_https_scheme(true)
            .with_url("https://swiftcord.localhost/")
            // Stay on our page and the video's site; anything else (the YouTube logo, channel links) opens in
            // the browser.
            .with_navigation_handler(move |to| {
                let ok = site(&to).is_some_and(|s| s == "swiftcord.localhost" || s == home || s.ends_with(&format!(".{home}")));
                if !ok {
                    crate::platform::open_url(&to);
                }
                ok
            })
            .with_new_window_req_handler(|to, _| {
                crate::platform::open_url(&to);
                wry::NewWindowResponse::Deny
            })
            .with_devtools(false)
            .with_visible(false)
            .with_bounds(Rect::default())
            .build_as_child(parent)
            .map_err(|e| e.to_string())?;
        Ok(InlinePlayer { url: url.to_owned(), webview, _context: context, shown: None, hidden_since: Some(Instant::now()) })
    }

    /// Shows it over `rect` (physical pixels in the window's client area), or hides it.
    pub fn place(&mut self, rect: Option<[i32; 4]>) {
        if rect == self.shown {
            return;
        }
        match rect {
            Some([x, y, w, h]) => {
                let bounds = Rect { position: dpi::PhysicalPosition::new(x, y).into(), size: dpi::PhysicalSize::new(w.max(1) as u32, h.max(1) as u32).into() };
                let _ = self.webview.set_bounds(bounds);
                if self.shown.is_none() {
                    let _ = self.webview.set_visible(true);
                }
                self.hidden_since = None;
            }
            None => {
                let _ = self.webview.set_visible(false);
                self.hidden_since.get_or_insert_with(Instant::now);
            }
        }
        self.shown = rect;
    }

    /// Out of sight long enough to stop.
    pub fn stale(&self) -> bool {
        self.hidden_since.is_some_and(|t| t.elapsed() > KEEP_HIDDEN)
    }
}
