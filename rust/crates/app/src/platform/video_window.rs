//! Videos that are web players rather than files (YouTube and the like) play in a small WebView2 window.
//! Like sign-in, it runs in a child process (`swiftcord --video-window <url> <title>`), so the client never
//! loads WebView2 itself and the window costs nothing until it's opened, and nothing once it's closed.

use std::process::Command;

use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::WindowBuilder;
use wry::{WebViewBuilder, WebViewBuilderExtWindows};

pub const ARG: &str = "--video-window";

/// The player page for a video embed: YouTube's embed player, set to start playing.
pub fn player_url(embed: &str) -> String {
    let sep = if embed.contains('?') { '&' } else { '?' };
    if embed.contains("youtube.com/embed/") || embed.contains("youtube-nocookie.com/embed/") {
        format!("{embed}{sep}autoplay=1")
    } else {
        embed.to_owned()
    }
}

/// A page that's just the player, filling the window.
pub(crate) fn player_page(url: &str) -> String {
    let src = url.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;");
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><meta name="referrer" content="strict-origin-when-cross-origin">
<style>html,body{{margin:0;height:100%;background:#000;overflow:hidden}}iframe{{border:0;width:100%;height:100%}}</style></head>
<body><iframe src="{src}" allow="autoplay; encrypted-media; fullscreen; picture-in-picture" allowfullscreen></iframe></body></html>"#
    )
}

/// The site a URL is on, without `www.`.
pub(crate) fn site(url: &str) -> Option<&str> {
    let host = url.split("://").nth(1)?.split(['/', '?', '#']).next()?;
    Some(host.strip_prefix("www.").unwrap_or(host))
}

/// Opens the video window for a player URL. Only web links are opened.
pub fn open(url: &str, title: &str) {
    if !url.starts_with("https://") {
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    if let Err(e) = Command::new(exe).arg(ARG).arg(player_url(url)).arg(title).spawn() {
        tracing::warn!("couldn't open the video window: {e}");
        crate::platform::open_url(url);
    }
}

/// Entry point of the child process.
pub fn run_child() -> ! {
    let mut args = std::env::args().skip_while(|a| a != ARG).skip(1);
    let url = args.next().filter(|u| u.starts_with("https://")).unwrap_or_default();
    let title = args.next().filter(|t| !t.is_empty()).unwrap_or_else(|| "Video".into());
    let home = site(&url).unwrap_or_default().to_owned();

    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title(format!("{title} — Swiftcord"))
        .with_inner_size(tao::dpi::LogicalSize::new(854.0, 480.0))
        .build(&event_loop)
        .expect("video window");
    let data_dir = crate::platform::data_dir().join("WebView2-video");
    let mut context = wry::WebContext::new(Some(data_dir));
    // Players refuse to load without a page embedding them (YouTube's "Error 153"), so the player goes in an
    // iframe on a page of our own, served from a private scheme (https://swiftcord.localhost/ in WebView2).
    let page = player_page(&url);
    let _webview = WebViewBuilder::new_with_web_context(&mut context)
        .with_custom_protocol("swiftcord".into(), move |_, _| {
            wry::http::Response::builder().header("Content-Type", "text/html; charset=utf-8").body(std::borrow::Cow::Owned(page.clone().into_bytes())).expect("response")
        })
        .with_https_scheme(true)
        .with_url("https://swiftcord.localhost/")
        // Stay on our page and the video's site; anything else opens in the browser.
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
        .build(&window)
        .expect("WebView2 is required for the video window");

    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        if let Event::WindowEvent { event: WindowEvent::CloseRequested, .. } = event {
            *flow = ControlFlow::Exit;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_embeds_autoplay() {
        assert_eq!(player_url("https://www.youtube.com/embed/dQw4w9WgXcQ"), "https://www.youtube.com/embed/dQw4w9WgXcQ?autoplay=1");
        assert_eq!(player_url("https://www.youtube.com/embed/x?start=5"), "https://www.youtube.com/embed/x?start=5&autoplay=1");
        assert_eq!(player_url("https://player.vimeo.com/video/1"), "https://player.vimeo.com/video/1");
        assert_eq!(site("https://www.youtube.com/embed/x"), Some("youtube.com"));
        assert!(player_page("https://x.com/e?a=1&b=\"2\"").contains(r#"src="https://x.com/e?a=1&amp;b=&quot;2&quot;""#));
    }
}
