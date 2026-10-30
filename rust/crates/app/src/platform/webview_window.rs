//! A plain window holding a WebView2, for the child processes (browser sign-in, captcha, web video player).

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::platform::windows::IconExtWindows;
use winit::window::{Icon, Window, WindowAttributes, WindowId};

/// Opens a window and lets `build` put a webview in it (returning whatever has to stay alive with it). Runs
/// until the window is closed (`None`) or a message is sent through the proxy (`Some(message)`).
pub fn run<T>(title: &str, size: (f64, f64), build: impl FnOnce(&Window, EventLoopProxy<String>) -> T) -> Option<String> {
    let event_loop = EventLoop::<String>::with_user_event().build().expect("event loop");
    let proxy = event_loop.create_proxy();
    let attributes = Window::default_attributes()
        .with_title(title)
        .with_inner_size(LogicalSize::new(size.0, size.1))
        .with_window_icon(Icon::from_resource(1, None).ok());
    let mut host = Host { attributes, proxy, build: Some(build), webview: None, window: None, message: None };
    let _ = event_loop.run_app(&mut host);
    host.message
}

struct Host<F, T> {
    attributes: WindowAttributes,
    proxy: EventLoopProxy<String>,
    build: Option<F>,
    // Declared before the window so it's dropped first.
    webview: Option<T>,
    window: Option<Window>,
    message: Option<String>,
}

impl<F: FnOnce(&Window, EventLoopProxy<String>) -> T, T> ApplicationHandler<String> for Host<F, T> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let Some(build) = self.build.take() else { return };
        let window = event_loop.create_window(self.attributes.clone()).expect("window");
        self.webview = Some(build(&window, self.proxy.clone()));
        self.window = Some(window);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, message: String) {
        self.message = Some(message);
        event_loop.exit();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let WindowEvent::CloseRequested = event {
            event_loop.exit();
        }
    }
}
