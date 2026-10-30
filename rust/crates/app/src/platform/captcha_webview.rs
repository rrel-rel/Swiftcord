//! Solves an hCaptcha that Discord asks for (e.g. when finishing QR sign-in). Discord's site key only works
//! on discord.com, so a WebView2 window opens a discord.com URL whose response we supply ourselves: a page
//! with just the hCaptcha widget. Like browser sign-in it runs in a child process
//! (`swiftcord --captcha-webview <sitekey> [rqdata]`) and prints the answer on stdout.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use swiftcord_core::rest::Captcha;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::WindowBuilder;
use webview2_com::Microsoft::Web::WebView2::Win32::{COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT, ICoreWebView2WebResourceResponse};
use webview2_com::WebResourceRequestedEventHandler;
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows::core::HSTRING;
use wry::{WebViewBuilder, WebViewExtWindows};

pub const ARG: &str = "--captcha-webview";
const ANSWER_PREFIX: &str = "CAPTCHA ";
const PAGE_URL: &str = "https://discord.com/swiftcord-captcha";

const PAGE: &str = r#"<!doctype html>
<html>
<head>
<meta charset="utf-8">
<title>Captcha</title>
<style>
  html, body { height: 100%; margin: 0; }
  body { display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 16px;
         background: #313338; color: #dbdee1; font: 15px "Segoe UI", sans-serif; }
</style>
<script>
  const PARAMS = __PARAMS__;
  function ready() {
    const id = hcaptcha.render('captcha', {
      sitekey: PARAMS.sitekey,
      theme: 'dark',
      callback: (answer) => window.ipc.postMessage(answer),
    });
    if (PARAMS.rqdata) hcaptcha.setData(id, { rqdata: PARAMS.rqdata });
  }
</script>
<script src="https://js.hcaptcha.com/1/api.js?render=explicit&onload=ready" async defer></script>
</head>
<body>
  <div>Discord wants to check that you're human before signing you in.</div>
  <div id="captcha"></div>
</body>
</html>"#;

/// Entry point of the child process; the site key and rqdata follow [`ARG`] on the command line.
pub fn run_child() -> ! {
    let mut args = std::env::args().skip_while(|a| a != ARG).skip(1);
    let params = serde_json::json!({ "sitekey": args.next().unwrap_or_default(), "rqdata": args.next() });
    // Keep "</script>" in the data from ending the script early.
    let page = PAGE.replace("__PARAMS__", &params.to_string().replace("</", "<\\/"));

    let event_loop = EventLoopBuilder::<String>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let window = WindowBuilder::new()
        .with_title("Captcha — Swiftcord")
        .with_inner_size(tao::dpi::LogicalSize::new(520.0, 640.0))
        .build(&event_loop)
        .expect("captcha window");

    // Kept between runs: hCaptcha goes easier on a browser it has seen before.
    let mut context = wry::WebContext::new(Some(super::data_dir().join("WebView2-captcha")));
    let webview = WebViewBuilder::new_with_web_context(&mut context)
        .with_ipc_handler(move |req| {
            let _ = proxy.send_event(req.body().clone());
        })
        .with_devtools(false)
        .build(&window)
        .expect("WebView2 is required for the captcha");
    serve_page(&webview, page).expect("captcha page");
    webview.load_url(PAGE_URL).expect("captcha page");

    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(answer) => {
                let mut out = std::io::stdout();
                let _ = writeln!(out, "{ANSWER_PREFIX}{answer}");
                let _ = out.flush();
                *flow = ControlFlow::Exit;
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => *flow = ControlFlow::Exit,
            _ => {}
        }
    })
}

/// Answers the webview's request for [`PAGE_URL`] with `page` instead of going to the network.
fn serve_page(webview: &wry::WebView, page: String) -> windows::core::Result<()> {
    let core = webview.webview();
    let env = webview.environment();
    unsafe {
        core.AddWebResourceRequestedFilter(&HSTRING::from(PAGE_URL), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT)?;
        let handler = WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let stream = SHCreateMemStream(Some(page.as_bytes()));
            let headers = HSTRING::from("Content-Type: text/html; charset=utf-8");
            let response: ICoreWebView2WebResourceResponse =
                env.CreateWebResourceResponse(stream.as_ref(), 200, &HSTRING::from("OK"), &headers)?;
            args.SetResponse(&response)
        }));
        let mut token = 0;
        core.add_WebResourceRequested(&handler, &mut token)?;
    }
    Ok(())
}

/// Shows the captcha and waits (on a blocking thread) for its answer. `None` if the window was closed.
pub fn solve_blocking(captcha: &Captcha) -> std::io::Result<Option<String>> {
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.arg(ARG).arg(&captcha.captcha_sitekey);
    if let Some(rqdata) = &captcha.captcha_rqdata {
        cmd.arg(rqdata);
    }
    let mut child = cmd.stdout(Stdio::piped()).stdin(Stdio::null()).spawn()?;
    let stdout = child.stdout.take().expect("piped stdout");
    let mut answer = None;
    for line in BufReader::new(stdout).lines() {
        if let Some(a) = line?.strip_prefix(ANSWER_PREFIX) {
            answer = Some(a.trim().to_owned());
            break;
        }
    }
    let _ = child.wait();
    Ok(answer)
}
