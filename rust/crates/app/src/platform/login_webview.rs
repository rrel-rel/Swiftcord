//! Browser sign-in: discord.com/login in a WebView2 window, so captcha, 2FA and passkeys all work. It runs
//! in a child process (`swiftcord --login-webview`) so the main client never loads WebView2, and prints
//! the token on stdout once the page starts using it.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use wry::WebViewBuilder;

pub const ARG: &str = "--login-webview";
const TOKEN_PREFIX: &str = "TOKEN ";

/// Wraps fetch and XMLHttpRequest so the Authorization header of the page's own API calls is reported.
const CAPTURE_SCRIPT: &str = r#"
(() => {
  const report = (t) => { if (t && typeof t === 'string' && t.length > 20) window.ipc.postMessage(t); };
  const setHeader = XMLHttpRequest.prototype.setRequestHeader;
  XMLHttpRequest.prototype.setRequestHeader = function (k, v) {
    if (String(k).toLowerCase() === 'authorization') report(v);
    return setHeader.apply(this, arguments);
  };
  const origFetch = window.fetch;
  window.fetch = function (input, init) {
    try {
      const h = (init && init.headers) || (input && input.headers);
      if (h) {
        const v = h instanceof Headers ? h.get('authorization') : (h.Authorization || h.authorization);
        report(v);
      }
    } catch (_) {}
    return origFetch.apply(this, arguments);
  };
})();
"#;

/// Entry point of the child process.
pub fn run_child() -> ! {
    let token = super::webview_window::run("Sign in to Discord — Swiftcord", (960.0, 720.0), |window, proxy| {
        let data_dir = directories::ProjectDirs::from("", "", "Swiftcord")
            .map(|d| d.data_local_dir().join("WebView2"))
            .unwrap_or_else(|| std::env::temp_dir().join("Swiftcord-WebView2"));
        // Start signed out, otherwise a previous web session would log straight back in after "Log out".
        let _ = std::fs::remove_dir_all(&data_dir);
        let mut context = wry::WebContext::new(Some(data_dir));
        let webview = WebViewBuilder::new_with_web_context(&mut context)
            .with_url("https://discord.com/login")
            .with_initialization_script(CAPTURE_SCRIPT)
            .with_ipc_handler(move |req| {
                let _ = proxy.send_event(req.body().clone());
            })
            .with_devtools(false)
            .build(window)
            .expect("WebView2 is required for browser sign-in");
        (context, webview)
    });
    if let Some(token) = token {
        let mut out = std::io::stdout();
        let _ = writeln!(out, "{TOKEN_PREFIX}{token}");
        let _ = out.flush();
    }
    std::process::exit(0)
}

/// Starts the sign-in window and waits (on a blocking thread) for a token. `None` if the window was closed.
pub fn sign_in_blocking() -> std::io::Result<Option<String>> {
    let exe = std::env::current_exe()?;
    let mut child = Command::new(exe).arg(ARG).stdout(Stdio::piped()).stdin(Stdio::null()).spawn()?;
    let stdout = child.stdout.take().expect("piped stdout");
    let mut token = None;
    for line in BufReader::new(stdout).lines() {
        if let Some(t) = line?.strip_prefix(TOKEN_PREFIX) {
            token = Some(t.trim().to_owned());
            break;
        }
    }
    let _ = child.wait();
    Ok(token)
}
