//! The window and frame loop: winit for the window and input (through egui-winit, which also handles IME,
//! the clipboard and the cursor), and Direct3D 11 for drawing. It stands in for eframe, whose renderers
//! (wgpu, OpenGL) cost several times the memory of the whole client.

use std::sync::Arc;
use std::time::Instant;

use egui::{ViewportId, ViewportInfo};
use egui_winit::ActionRequested;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::platform::windows::IconExtWindows;
use winit::raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, WindowHandle};
use winit::window::{Window, WindowId};

use crate::platform::clipboard;
use crate::platform::d3d11::Painter;

pub trait App {
    /// Runs every frame, including while the window is minimized (when `ui` doesn't).
    fn logic(&mut self, ctx: &egui::Context, frame: &mut Frame);
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut Frame);
}

/// The app's window, for things that attach native windows to it.
pub struct Frame {
    window: Arc<Window>,
}

impl HasWindowHandle for Frame {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        self.window.window_handle()
    }
}

/// Opens the window (hidden until the first frame is drawn, then centered) and runs until it's closed.
pub fn run(
    viewport: egui::ViewportBuilder,
    create_app: impl FnOnce(&egui::Context) -> Box<dyn App>,
) -> Result<(), winit::error::EventLoopError> {
    let event_loop = EventLoop::<Instant>::with_user_event().build()?;
    let ctx = egui::Context::default();
    let proxy: EventLoopProxy<Instant> = event_loop.create_proxy();
    // egui asks for repaints from any thread (e.g. when an image finishes loading); wake the loop for them.
    ctx.set_request_repaint_callback(move |info| {
        if let Some(when) = Instant::now().checked_add(info.delay) {
            let _ = proxy.send_event(when);
        }
    });
    let app = create_app(&ctx);
    let mut shell = Shell { ctx, viewport, app, window: None, next_repaint: None };
    event_loop.run_app(&mut shell)
}

struct Shell {
    ctx: egui::Context,
    viewport: egui::ViewportBuilder,
    app: Box<dyn App>,
    window: Option<AppWindow>,
    next_repaint: Option<Instant>,
}

struct AppWindow {
    window: Arc<Window>,
    egui_winit: egui_winit::State,
    painter: Painter,
    info: ViewportInfo,
    actions: Vec<ActionRequested>,
    shown: bool,
    modifiers: ModifiersState,
}

impl ApplicationHandler<Instant> for Shell {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        match AppWindow::new(&self.ctx, event_loop, self.viewport.clone()) {
            // The first frame is drawn right away: a hidden window gets no redraw events, and it's shown once drawn.
            Ok(mut w) => match w.frame(&self.ctx, self.app.as_mut()) {
                Ok(()) => self.window = Some(w),
                Err(e) => {
                    tracing::error!("couldn't draw the window: {e}");
                    event_loop.exit();
                }
            },
            Err(e) => {
                tracing::error!("couldn't open the window: {e}");
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, when: Instant) {
        if when <= Instant::now() {
            if let Some(w) = &self.window {
                w.window.request_redraw();
            }
        } else if self.next_repaint.is_none_or(|t| when < t) {
            self.next_repaint = Some(when);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(w) = &mut self.window else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => {
                if let Err(e) = w.frame(&self.ctx, self.app.as_mut()) {
                    // Most likely the GPU was reset or removed; there's no recovering the textures egui sent.
                    tracing::error!("drawing failed: {e}");
                    event_loop.exit();
                }
            }
            WindowEvent::KeyboardInput { event: key, .. } if is_paste(&key, w.modifiers) => {
                // egui-winit's own clipboard is off (it would bring in arboard), so pasting is done here.
                w.paste();
                w.window.request_redraw();
            }
            event => {
                if let WindowEvent::ModifiersChanged(m) = &event {
                    w.modifiers = m.state();
                }
                if w.egui_winit.on_window_event(&w.window, &event).repaint {
                    w.window.request_redraw();
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        match self.next_repaint {
            Some(t) if t <= Instant::now() => {
                self.next_repaint = None;
                if let Some(w) = &self.window {
                    w.window.request_redraw();
                }
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            Some(t) => event_loop.set_control_flow(ControlFlow::WaitUntil(t)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}

impl AppWindow {
    fn new(ctx: &egui::Context, event_loop: &ActiveEventLoop, viewport: egui::ViewportBuilder) -> anyhow::Result<Self> {
        let window = Arc::new(egui_winit::create_window(ctx, event_loop, &viewport.with_visible(false))?);
        // The exe's own icon (resource 1, embedded by build.rs).
        window.set_window_icon(winit::window::Icon::from_resource(1, None).ok());
        center(&window);
        let RawWindowHandle::Win32(handle) = window.window_handle()?.as_raw() else {
            anyhow::bail!("not a Win32 window");
        };
        let hwnd = windows::Win32::Foundation::HWND(handle.hwnd.get() as *mut _);
        let size = window.inner_size();
        let painter = Painter::new(hwnd, [size.width, size.height])?;
        let egui_winit = egui_winit::State::new(
            ctx.clone(),
            ViewportId::ROOT,
            event_loop,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(painter.max_texture_side()),
        );
        let mut info = ViewportInfo::default();
        egui_winit::update_viewport_info(&mut info, ctx, &window, true);
        Ok(Self { window, egui_winit, painter, info, actions: Vec::new(), shown: false, modifiers: ModifiersState::empty() })
    }

    /// One frame: input in, the app's logic and UI, then drawing. Minimized, only the logic runs.
    fn frame(&mut self, ctx: &egui::Context, app: &mut dyn App) -> anyhow::Result<()> {
        let mut frame = Frame { window: self.window.clone() };
        egui_winit::update_viewport_info(&mut self.info, ctx, &self.window, false);
        let mut input = self.egui_winit.take_egui_input(&self.window);
        input.viewports = std::iter::once((ViewportId::ROOT, self.info.clone())).collect();
        self.info.events.clear();

        let size = self.window.inner_size();
        if self.shown && (self.window.is_minimized() == Some(true) || size.width == 0 || size.height == 0) {
            let output = ctx.run_logic(&input, |ctx| app.logic(ctx, &mut frame));
            self.handle_platform_output(output.platform_output);
            for (id, commands) in output.viewport_commands {
                if id == ViewportId::ROOT {
                    egui_winit::process_viewport_commands(ctx, &mut self.info, commands, &self.window, &mut self.actions);
                }
            }
            return Ok(());
        }

        let output = ctx.run_ui(input, |ui| {
            app.logic(ui.ctx(), &mut frame);
            app.ui(ui, &mut frame);
        });
        self.handle_platform_output(output.platform_output);
        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
        let clear = ctx.global_style().visuals.panel_fill.to_normalized_gamma_f32();
        self.painter.resize([size.width, size.height])?;
        self.painter.paint(output.pixels_per_point, clear, &primitives, &output.textures_delta)?;
        if !self.shown {
            // Shown only once there's something in it, so it doesn't flash white.
            self.shown = true;
            self.window.set_visible(true);
        }

        if let Some(root) = output.viewport_output.get(&ViewportId::ROOT) {
            egui_winit::process_viewport_commands(ctx, &mut self.info, root.commands.clone(), &self.window, &mut self.actions);
        }
        for action in std::mem::take(&mut self.actions) {
            match action {
                ActionRequested::Cut => self.egui_winit.egui_input_mut().events.push(egui::Event::Cut),
                ActionRequested::Copy => self.egui_winit.egui_input_mut().events.push(egui::Event::Copy),
                ActionRequested::Paste => self.paste(),
                _ => continue,
            }
            self.window.request_redraw();
        }
        Ok(())
    }

    /// Copying goes to the Windows clipboard; the rest (cursor, IME, links) is egui-winit's.
    fn handle_platform_output(&mut self, mut output: egui::PlatformOutput) {
        output.commands.retain(|command| match command {
            egui::OutputCommand::CopyText(text) => {
                clipboard::set_text(text);
                false
            }
            egui::OutputCommand::CopyImage(image) => {
                clipboard::set_image(image);
                false
            }
            _ => true,
        });
        self.egui_winit.handle_platform_output(&self.window, output);
    }

    /// Text if the clipboard has any, otherwise an image (a screenshot, say).
    fn paste(&mut self) {
        let event = match clipboard::get_text().filter(|t| !t.is_empty()) {
            Some(text) => egui::Event::Paste(text.replace("\r\n", "\n")),
            None => match clipboard::get_image() {
                Some(image) => egui::Event::PasteImage(image),
                None => return,
            },
        };
        self.egui_winit.egui_input_mut().events.push(event);
    }
}

/// Ctrl+V, Shift+Insert, or a keyboard's Paste key.
fn is_paste(key: &KeyEvent, modifiers: ModifiersState) -> bool {
    if key.state != ElementState::Pressed {
        return false;
    }
    let ctrl_only = modifiers.control_key() && !modifiers.alt_key() && !modifiers.shift_key() && !modifiers.super_key();
    let v = match &key.logical_key {
        Key::Character(c) => c.eq_ignore_ascii_case("v"),
        _ => key.physical_key == PhysicalKey::Code(KeyCode::KeyV),
    };
    let insert = key.logical_key == Key::Named(NamedKey::Insert);
    key.logical_key == Key::Named(NamedKey::Paste) || (ctrl_only && v) || (modifiers == ModifiersState::SHIFT && insert)
}

/// Centers the window on its monitor.
fn center(window: &Window) {
    let Some(monitor) = window.current_monitor().or_else(|| window.primary_monitor()) else { return };
    let (area, origin, size) = (monitor.size(), monitor.position(), window.outer_size());
    let x = origin.x + (area.width as i32 - size.width as i32) / 2;
    let y = origin.y + (area.height as i32 - size.height as i32) / 2;
    window.set_outer_position(winit::dpi::PhysicalPosition::new(x, y.max(origin.y)));
}
