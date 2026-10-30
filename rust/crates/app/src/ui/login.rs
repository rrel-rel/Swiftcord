//! Sign-in: QR code (scan with the Discord phone app), the real login page in a browser window, or a token.

use eframe::egui::{self, Align, Color32, ColorImage, Layout, RichText, TextureHandle, TextureOptions, Vec2};
use swiftcord_core::remote_auth::RemoteAuthEvent;

use crate::app::Action;
use crate::theme;

#[derive(Default)]
pub struct LoginUi {
    qr: Option<TextureHandle>,
    scanned: Option<String>,
    /// Discord asked for a captcha after the phone approved; it's open in its own window.
    captcha: bool,
    qr_failed: Option<String>,
    token: String,
    show_token: bool,
    pub busy: bool,
    pub error: Option<String>,
}

impl LoginUi {
    pub fn reset_qr(&mut self) {
        self.qr = None;
        self.scanned = None;
        self.captcha = false;
        self.qr_failed = None;
    }

    pub fn on_remote_auth(&mut self, ctx: &egui::Context, e: RemoteAuthEvent) {
        match e {
            RemoteAuthEvent::QrCode(url) => {
                self.scanned = None;
                self.qr = render_qr(&url).map(|img| ctx.load_texture("login-qr", img, TextureOptions::NEAREST));
            }
            RemoteAuthEvent::Scanned { username } => self.scanned = Some(username),
            RemoteAuthEvent::Captcha => self.captcha = true,
            RemoteAuthEvent::Failed(e) => {
                tracing::warn!("QR login failed: {e}");
                self.qr = None;
                // Otherwise "Check your phone!" stays up and hides the failure.
                self.scanned = None;
                self.captcha = false;
                self.qr_failed = Some(e);
            }
            RemoteAuthEvent::Token(_) | RemoteAuthEvent::Cancelled => {}
        }
    }
}

fn render_qr(data: &str) -> Option<ColorImage> {
    let code = qrcode::QrCode::new(data.as_bytes()).ok()?;
    let w = code.width();
    let colors = code.to_colors();
    const QUIET: usize = 2;
    const SCALE: usize = 5;
    let side = (w + QUIET * 2) * SCALE;
    let mut px = vec![Color32::WHITE; side * side];
    for y in 0..w {
        for x in 0..w {
            if colors[y * w + x] == qrcode::Color::Dark {
                for dy in 0..SCALE {
                    for dx in 0..SCALE {
                        px[((y + QUIET) * SCALE + dy) * side + (x + QUIET) * SCALE + dx] = Color32::BLACK;
                    }
                }
            }
        }
    }
    Some(ColorImage::new([side, side], px))
}

pub fn show(ui: &mut egui::Ui, login: &mut LoginUi, actions: &mut Vec<Action>) {
    egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::RAIL())).show(ui, |ui| {
        ui.with_layout(Layout::top_down(Align::Center), |ui| {
            let card_w = 720.0_f32.min(ui.available_width() - 32.0);
            ui.add_space(((ui.available_height() - 420.0) / 2.0).max(16.0));
            egui::Frame::new().fill(theme::SIDEBAR()).corner_radius(8).inner_margin(32).show(ui, |ui| {
                ui.set_width(card_w - 64.0);
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width((card_w - 64.0) * 0.58);
                        ui.label(RichText::new("Welcome back!").font(theme::font_bold(24.0)).color(theme::HEADER()));
                        ui.label(RichText::new("Sign in to Discord to continue.").color(theme::MUTED()));
                        ui.add_space(20.0);

                        let enabled = !login.busy;
                        let b = egui::Button::new(RichText::new("Sign in with your browser").color(Color32::WHITE))
                            .fill(theme::BLURPLE())
                            .min_size(Vec2::new(ui.available_width(), 40.0));
                        if ui.add_enabled(enabled, b).clicked() {
                            login.error = None;
                            actions.push(Action::BrowserLogin);
                        }
                        ui.label(RichText::new("Opens discord.com so captcha, 2FA and passkeys work.").small().color(theme::FAINT()));
                        ui.add_space(16.0);

                        if ui.link(if login.show_token { "Hide token sign-in" } else { "Sign in with a token instead" }).clicked() {
                            login.show_token = !login.show_token;
                        }
                        if login.show_token {
                            ui.add_space(6.0);
                            let edit = egui::TextEdit::singleline(&mut login.token)
                                .password(true)
                                .hint_text("Token")
                                .desired_width(ui.available_width());
                            let r = ui.add_enabled(enabled, edit);
                            let submit = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            if ui.add_enabled(enabled && !login.token.trim().is_empty(), egui::Button::new("Sign in")).clicked() || submit {
                                login.error = None;
                                actions.push(Action::TokenLogin(std::mem::take(&mut login.token)));
                            }
                        }
                        if login.busy {
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Signing in…");
                            });
                        }
                        if let Some(e) = &login.error {
                            ui.add_space(8.0);
                            ui.colored_label(theme::RED(), e);
                        }
                    });

                    ui.add_space(24.0);
                    ui.vertical_centered(|ui| {
                        let size = 176.0;
                        match (&login.qr, &login.scanned, &login.qr_failed) {
                            (_, Some(_), _) if login.captcha => {
                                ui.add_space(40.0);
                                ui.label(RichText::new("One more step").font(theme::font_bold(18.0)).color(theme::HEADER()));
                                ui.label("Solve the captcha in the window that opened.");
                            }
                            (_, Some(user), _) => {
                                ui.add_space(40.0);
                                ui.label(RichText::new("Check your phone!").font(theme::font_bold(18.0)).color(theme::HEADER()));
                                ui.label(format!("Approve the sign-in as {user}."));
                            }
                            (Some(tex), None, _) => {
                                egui::Frame::new().fill(Color32::WHITE).corner_radius(6).inner_margin(4).show(ui, |ui| {
                                    ui.add(egui::Image::new(tex).fit_to_exact_size(Vec2::splat(size)));
                                });
                                ui.add_space(8.0);
                                ui.label(RichText::new("Sign in with QR code").font(theme::font_semibold(16.0)).color(theme::HEADER()));
                                ui.label(RichText::new("Scan this with the Discord mobile app.").color(theme::MUTED()));
                            }
                            (None, None, Some(e)) => {
                                ui.add_space(50.0);
                                ui.label(RichText::new("QR sign-in didn't work.").color(theme::MUTED()));
                                ui.label(RichText::new(e).small().color(theme::MUTED()));
                                if ui.button("Try again").clicked() {
                                    actions.push(Action::StartQrLogin);
                                }
                            }
                            (None, None, None) => {
                                ui.add_space(70.0);
                                ui.spinner();
                            }
                        }
                    });
                });
            });
            ui.add_space(12.0);
            ui.label(
                RichText::new("Swiftcord is an unofficial client. Using it may break Discord's Terms of Service.")
                    .small()
                    .color(theme::FAINT()),
            );
        });
    });
}
