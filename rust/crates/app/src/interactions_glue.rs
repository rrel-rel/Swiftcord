//! Apps' buttons, select menus and forms (modals): sending what was used, following the app's answer over
//! the gateway, and the form window.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use eframe::egui::{self, Align, Color32, Layout, RichText, Vec2};
use serde_json::value::RawValue;
use swiftcord_core::interactions::{self, Field, Modal, Outcome};
use swiftcord_core::models::{component_type as kind, Component, Message};
use swiftcord_core::Snowflake;

use crate::app::{Action, App, AppEvent, UiState};
use crate::theme;

/// How long to wait for an app to acknowledge before saying it didn't respond (Discord gives apps 3 s).
const TIMEOUT: Duration = Duration::from_secs(8);

/// Something we used, waiting for the app's answer.
pub struct Pending {
    pub message: Snowflake,
    pub custom_id: String,
    pub channel: Snowflake,
    pub guild: Option<Snowflake>,
    pub app_name: String,
    sent: Instant,
}

/// An open form.
pub struct OpenModal {
    pub modal: Modal,
    pub fields: Vec<Field>,
    pub channel: Snowflake,
    pub guild: Option<Snowflake>,
    pub error: Option<String>,
    pub busy: bool,
}

#[derive(Default)]
pub struct Interactions {
    /// By nonce.
    pub pending: HashMap<String, Pending>,
    pub modal: Option<OpenModal>,
    /// Choices being made in multi-select menus, by message and menu.
    pub choosing: HashMap<(Snowflake, String), Vec<String>>,
}

impl Interactions {
    /// Whether this message's component is waiting for its app.
    pub fn is_pending(&self, message: Snowflake, custom_id: &str) -> bool {
        self.pending.values().any(|p| p.message == message && p.custom_id == custom_id)
    }
}

/// Uses a button (`values: None`) or a select menu.
pub fn use_component(app: &mut App, msg: &Message, c: &Component, values: Option<Vec<String>>) {
    let Some(s) = &app.session else { return };
    let custom_id = c.custom_id.clone().unwrap_or_default();
    if custom_id.is_empty() || app.ui.interactions.is_pending(msg.id, &custom_id) {
        return;
    }
    let nonce = swiftcord_core::nonce_now();
    let body = interactions::component_body(msg, c, values.as_deref(), &s.state.session_id, nonce);
    let app_name = msg.author.display_name().to_owned();
    app.ui.interactions.pending.insert(
        nonce.to_string(),
        Pending { message: msg.id, custom_id, channel: msg.channel_id, guild: msg.guild_id, app_name, sent: Instant::now() },
    );
    send(app, nonce, body);
}

fn send(app: &mut App, nonce: Snowflake, body: serde_json::Value) {
    let Some(s) = &app.session else { return };
    let rest = s.rest.clone();
    app.spawn(async move {
        let result = rest.interact(body).await.map_err(|e| e.to_string());
        AppEvent::InteractionSent { nonce: nonce.to_string(), result }
    });
}

/// The request itself failed (the app never saw it).
pub fn on_sent(app: &mut App, nonce: String, result: Result<(), String>) {
    let Err(e) = result else { return };
    tracing::info!("interaction {nonce} failed: {e}");
    app.ui.interactions.pending.remove(&nonce);
    if let Some(m) = &mut app.ui.interactions.modal {
        if m.busy {
            m.busy = false;
            m.error = Some(format!("Couldn't send: {e}"));
            return;
        }
    }
    app.banner(format!("Couldn't use that: {e}"));
}

/// INTERACTION_* gateway events. True when it was one.
pub fn on_gateway(app: &mut App, name: &str, data: &RawValue) -> bool {
    match name {
        "INTERACTION_CREATE" => {}
        "INTERACTION_SUCCESS" => {
            let Ok(o) = serde_json::from_str::<Outcome>(data.get()) else { return true };
            if let Some(n) = o.nonce {
                app.ui.interactions.pending.remove(&n);
                // A submitted form closes once the app accepts it.
                if app.ui.interactions.modal.as_ref().is_some_and(|m| m.busy) {
                    app.ui.interactions.modal = None;
                }
            }
        }
        "INTERACTION_FAILURE" => {
            let Ok(o) = serde_json::from_str::<Outcome>(data.get()) else { return true };
            let pending = o.nonce.and_then(|n| app.ui.interactions.pending.remove(&n));
            let who = pending.map_or_else(|| "The app".to_owned(), |p| p.app_name);
            if let Some(m) = &mut app.ui.interactions.modal {
                if m.busy {
                    m.busy = false;
                    m.error = Some(format!("{who} didn't accept that."));
                    return true;
                }
            }
            app.banner(format!("{who} didn't respond."));
        }
        "INTERACTION_MODAL_CREATE" => {
            let Ok(modal) = serde_json::from_str::<Modal>(data.get()) else {
                tracing::warn!("unreadable modal: {}", data.get());
                return true;
            };
            let pending = modal.nonce.as_ref().and_then(|n| app.ui.interactions.pending.remove(n));
            let channel = modal.channel_id.or(pending.as_ref().map(|p| p.channel)).unwrap_or_default();
            let guild = modal.guild_id.or(pending.and_then(|p| p.guild));
            let fields = modal.fields();
            app.ui.interactions.modal = Some(OpenModal { modal, fields, channel, guild, error: None, busy: false });
        }
        _ => return false,
    }
    true
}

/// Gives up on apps that never answered.
pub fn tick(app: &mut App) {
    let late: Vec<String> = app.ui.interactions.pending.iter().filter(|(_, p)| p.sent.elapsed() > TIMEOUT).map(|(n, _)| n.clone()).collect();
    for n in late {
        if let Some(p) = app.ui.interactions.pending.remove(&n) {
            app.banner(format!("{} didn't respond.", p.app_name));
        }
    }
    if !app.ui.interactions.pending.is_empty() {
        app.ctx().request_repaint_after(Duration::from_millis(500));
    }
}

/// Runs the slash command filled in in `channel`'s composer, or says what's missing.
pub fn run_command(app: &mut App, channel: Snowflake) {
    let Some(s) = &app.session else { return };
    let session_id = s.state.session_id.clone();
    let guild = s.state.channel(channel).and_then(|c| c.guild_id);
    let Some(draft) = app.ui.drafts.get_mut(&channel) else { return };
    let Some(cmd) = &mut draft.command else { return };
    let (path, options) = match cmd.values() {
        Ok(v) => v,
        Err(e) => {
            cmd.error = Some(e);
            return;
        }
    };
    let nonce = swiftcord_core::nonce_now();
    let body = swiftcord_core::commands::command_body(&cmd.command, &path, options, guild, channel, &session_id, nonce);
    let app_name = cmd.app.clone();
    draft.command = None;
    app.ui.focus_composer = true;
    app.ui.interactions.pending.insert(
        nonce.to_string(),
        Pending { message: Snowflake::NONE, custom_id: String::new(), channel, guild, app_name, sent: Instant::now() },
    );
    send(app, nonce, body);
}

/// Sends the open form.
pub fn submit_modal(app: &mut App) {
    let Some(s) = &app.session else { return };
    let session_id = s.state.session_id.clone();
    let Some(m) = &mut app.ui.interactions.modal else { return };
    if let Some(problem) = m.fields.iter().find_map(interactions::field_problem) {
        m.error = Some(problem);
        return;
    }
    m.busy = true;
    m.error = None;
    let nonce = swiftcord_core::nonce_now();
    let body = interactions::modal_body(&m.modal, m.channel, m.guild, &m.fields, &session_id, nonce);
    let app_name = m.modal.application.name.clone();
    app.ui.interactions.pending.insert(
        nonce.to_string(),
        Pending { message: Snowflake::NONE, custom_id: String::new(), channel: Snowflake::NONE, guild: None, app_name, sent: Instant::now() },
    );
    send(app, nonce, body);
}

/// The form window.
pub fn modal_window(ui: &egui::Ui, uis: &mut UiState, actions: &mut Vec<Action>) {
    let Some(m) = &mut uis.interactions.modal else { return };
    let mut open = true;
    egui::Window::new(m.modal.title.clone())
        .id(egui::Id::unique("app-modal"))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ui.ctx(), |ui| {
            ui.set_width(440.0);
            if !m.modal.application.name.is_empty() {
                ui.label(RichText::new(&m.modal.application.name).size(12.5).color(theme::MUTED()));
            }
            ui.add_enabled_ui(!m.busy, |ui| {
                for f in &mut m.fields {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(f.label.to_uppercase()).font(theme::font_semibold(12.0)).color(theme::HEADER()));
                        if f.required {
                            ui.label(RichText::new("*").color(theme::RED()));
                        }
                    });
                    if let Some(d) = &f.description {
                        ui.label(RichText::new(d).size(12.0).color(theme::MUTED()));
                    }
                    if f.kind == kind::STRING_SELECT {
                        modal_select(ui, f);
                        continue;
                    }
                    let limit = f.max_length.unwrap_or(4000) as usize;
                    let edit = if f.style == 2 { egui::TextEdit::multiline(&mut f.value).desired_rows(4) } else { egui::TextEdit::singleline(&mut f.value) };
                    ui.add(edit.hint_text(f.placeholder.clone().unwrap_or_default()).char_limit(limit).desired_width(f32::INFINITY));
                    if f.max_length.is_some() {
                        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                            ui.label(RichText::new(format!("{}/{limit}", f.value.chars().count())).size(11.0).color(theme::FAINT()));
                        });
                    }
                }
            });
            if let Some(e) = &m.error {
                ui.add_space(6.0);
                ui.label(RichText::new(e).color(theme::RED()));
            }
            ui.add_space(10.0);
            ui.allocate_ui_with_layout(Vec2::new(ui.available_width(), 30.0), Layout::right_to_left(Align::Center), |ui| {
                let submit = egui::Button::new(RichText::new(if m.busy { "Sending…" } else { "Submit" }).color(Color32::WHITE)).fill(theme::BLURPLE());
                if ui.add_enabled(!m.busy, submit).clicked() {
                    actions.push(Action::SubmitModal);
                }
                if ui.button("Cancel").clicked() {
                    actions.push(Action::CloseModal);
                }
            });
        });
    if !open {
        actions.push(Action::CloseModal);
    }
}

/// A select menu inside a form: checkboxes when several can be chosen, otherwise a list to pick one from.
fn modal_select(ui: &mut egui::Ui, f: &mut Field) {
    let many = f.max_values > 1;
    for o in &f.options {
        let mut on = f.values.contains(&o.value);
        let changed = if many { ui.checkbox(&mut on, &o.label).changed() } else { ui.radio(on, &o.label).clicked() && { on = true; true } };
        if changed {
            if !many {
                f.values.clear();
            }
            f.values.retain(|v| v != &o.value);
            if on && f.values.len() < f.max_values as usize {
                f.values.push(o.value.clone());
            }
        }
        if let Some(d) = &o.description {
            ui.label(RichText::new(d).size(11.5).color(theme::MUTED()));
        }
    }
}
