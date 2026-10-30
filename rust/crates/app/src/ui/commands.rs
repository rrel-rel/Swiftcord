//! Slash commands in the composer: typing `/` lists the apps' commands; picking one turns the composer into a
//! small form for its options (and subcommand), and running it sends an APPLICATION_COMMAND interaction.

use std::collections::HashMap;

use egui::{self, Align, Color32, Layout, RichText, Ui};
use serde_json::{json, Value};
use swiftcord_core::commands::{option_type as ty, option_value, Command, CommandOption, Index};
use swiftcord_core::models::{Channel, ChannelType};
use swiftcord_core::state::State;

use crate::theme;

/// A place's commands, once asked for.
pub enum Loaded {
    Loading,
    Ready(Index),
    Failed,
}

/// One way to run a command: a subcommand (with its group), or the command itself; and its options.
pub struct Variant {
    pub path: Vec<(u8, String)>,
    pub label: String,
    pub description: String,
    pub options: Vec<CommandOption>,
}

fn variants(cmd: &Command) -> Vec<Variant> {
    let mut out = Vec::new();
    for o in &cmd.options {
        match o.kind {
            ty::SUB_COMMAND => out.push(Variant { path: vec![(ty::SUB_COMMAND, o.name.clone())], label: o.name.clone(), description: o.description.clone(), options: o.options.clone() }),
            ty::SUB_COMMAND_GROUP => {
                for s in o.options.iter().filter(|s| s.kind == ty::SUB_COMMAND) {
                    out.push(Variant {
                        path: vec![(ty::SUB_COMMAND_GROUP, o.name.clone()), (ty::SUB_COMMAND, s.name.clone())],
                        label: format!("{} {}", o.name, s.name),
                        description: s.description.clone(),
                        options: s.options.clone(),
                    });
                }
            }
            _ => {}
        }
    }
    if out.is_empty() {
        out.push(Variant { path: Vec::new(), label: String::new(), description: cmd.description.clone(), options: cmd.options.clone() });
    }
    out
}

/// What's been entered for one option.
#[derive(Default, Clone)]
pub struct Field {
    pub text: String,
    pub flag: Option<bool>,
    pub choice: Option<usize>,
    /// A chosen person, role or channel: its ID and how it's shown.
    pub picked: Option<(String, String)>,
    pub search: String,
}

/// A command being filled in, in place of the message box.
pub struct CommandDraft {
    pub command: Command,
    pub app: String,
    pub variants: Vec<Variant>,
    pub variant: usize,
    pub fields: HashMap<String, Field>,
    pub error: Option<String>,
}

impl CommandDraft {
    pub fn new(command: Command, app: String) -> CommandDraft {
        let variants = variants(&command);
        CommandDraft { command, app, variants, variant: 0, fields: HashMap::new(), error: None }
    }

    fn options(&self) -> &[CommandOption] {
        &self.variants[self.variant].options
    }

    /// The subcommand path and option values to send, or what's missing or wrong.
    pub fn values(&self) -> Result<(Vec<(u8, String)>, Vec<Value>), String> {
        let mut out = Vec::new();
        for o in self.options() {
            let f = self.fields.get(&o.name).cloned().unwrap_or_default();
            let text = f.text.trim();
            let value: Option<Value> = match o.kind {
                _ if !o.choices.is_empty() => f.choice.and_then(|i| o.choices.get(i)).map(|c| c.value.clone()),
                ty::STRING if text.is_empty() => None,
                ty::STRING => {
                    let n = text.chars().count() as u32;
                    if o.min_length.is_some_and(|m| n < m) || o.max_length.is_some_and(|m| n > m) {
                        return Err(format!("“{}” must be {}–{} characters.", o.name, o.min_length.unwrap_or(0), o.max_length.unwrap_or(6000)));
                    }
                    Some(json!(text))
                }
                ty::INTEGER | ty::NUMBER if text.is_empty() => None,
                ty::INTEGER | ty::NUMBER => {
                    let n: f64 = text.parse().map_err(|_| format!("“{}” must be a number.", o.name))?;
                    if o.kind == ty::INTEGER && n.fract() != 0.0 {
                        return Err(format!("“{}” must be a whole number.", o.name));
                    }
                    if o.min_value.is_some_and(|m| n < m) || o.max_value.is_some_and(|m| n > m) {
                        let range = format!("{}–{}", o.min_value.map_or("".into(), |v| v.to_string()), o.max_value.map_or("".into(), |v| v.to_string()));
                        return Err(format!("“{}” must be in {range}.", o.name));
                    }
                    Some(if o.kind == ty::INTEGER { json!(n as i64) } else { json!(n) })
                }
                ty::BOOLEAN => f.flag.map(|b| json!(b)),
                ty::USER | ty::CHANNEL | ty::ROLE | ty::MENTIONABLE => f.picked.as_ref().map(|(id, _)| json!(id)),
                ty::ATTACHMENT if o.required => return Err(format!("“{}” needs a file, which Swiftcord can't attach to commands yet.", o.name)),
                _ => None,
            };
            match value {
                Some(v) => out.push(option_value(o.kind, &o.name, v)),
                None if o.required => return Err(format!("“{}” is required.", o.name)),
                None => {}
            }
        }
        Ok((self.variants[self.variant].path.clone(), out))
    }
}

/// People, roles or channels to pick from for an option, matching `search`: (ID, label).
fn picks(kind: u8, search: &str, state: &State, channel: &Channel) -> Vec<(String, String)> {
    let q = search.to_lowercase();
    let users = || {
        state.search_people(channel, search, 10).into_iter().map(|u| (u.id.to_string(), format!("@{}", state.display_name(u.id, channel.guild_id)))).collect::<Vec<_>>()
    };
    let roles = || {
        let Some(g) = channel.guild_id.and_then(|g| state.guild(g)) else { return Vec::new() };
        let mut r: Vec<_> = g.roles.iter().filter(|r| r.id != g.id && r.name.to_lowercase().contains(&q)).collect();
        r.sort_by_key(|r| -r.position);
        r.into_iter().take(10).map(|r| (r.id.to_string(), format!("@{}", r.name))).collect()
    };
    match kind {
        ty::USER => users(),
        ty::ROLE => roles(),
        ty::MENTIONABLE => users().into_iter().chain(roles()).collect(),
        ty::CHANNEL => {
            let Some(g) = channel.guild_id else { return Vec::new() };
            state
                .channel_tree(g)
                .into_iter()
                .flat_map(|(_, cs)| cs)
                .filter(|c| !matches!(c.kind, ChannelType::Category) && c.label().to_lowercase().contains(&q))
                .take(10)
                .map(|c| (c.id.to_string(), format!("#{}", c.label())))
                .collect()
        }
        _ => Vec::new(),
    }
}

/// What the form asks for.
pub enum FormResult {
    Nothing,
    Run,
    Cancel,
}

/// The command form, in place of the message box.
pub fn form(ui: &mut Ui, state: &State, channel: &Channel, draft: &mut CommandDraft) -> FormResult {
    let mut result = FormResult::Nothing;
    egui::Frame::new().inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("/{}", draft.command.name)).font(theme::font_bold(15.0)).color(theme::HEADER()));
            ui.label(RichText::new(&draft.app).size(12.5).color(theme::MUTED()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if crate::ui::widgets::icon_button(ui, theme::icon::CLOSE, "Cancel (Esc)").clicked() {
                    result = FormResult::Cancel;
                }
            });
        });
        if !draft.variants[0].path.is_empty() {
            let current = draft.variants[draft.variant].label.clone();
            let mut chosen = draft.variant;
            egui::ComboBox::from_id_salt(("command-variant", draft.command.id)).width(260.0).selected_text(current).show_ui(ui, |ui| {
                for (i, v) in draft.variants.iter().enumerate() {
                    ui.selectable_value(&mut chosen, i, &v.label).on_hover_text(&v.description);
                }
            });
            if chosen != draft.variant {
                draft.variant = chosen;
                draft.fields.clear();
                draft.error = None;
            }
        }
        let description = draft.variants[draft.variant].description.clone();
        if !description.is_empty() {
            ui.label(RichText::new(description).size(12.5).color(theme::MUTED()));
        }
        let options = draft.options().to_vec();
        if !options.is_empty() {
            ui.add_space(4.0);
            egui::Grid::new(("command-options", draft.command.id, draft.variant)).num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                for o in &options {
                    let label = RichText::new(format!("{}{}", o.name, if o.required { " *" } else { "" })).color(theme::HEADER());
                    ui.label(label).on_hover_text(&o.description);
                    let f = draft.fields.entry(o.name.clone()).or_default();
                    option_field(ui, o, f, state, channel, draft.command.id.0);
                    ui.end_row();
                }
            });
        }
        if let Some(e) = &draft.error {
            ui.label(RichText::new(e).color(theme::RED()));
        }
        ui.horizontal(|ui| {
            let run = egui::Button::new(RichText::new(format!("Run /{}", draft.command.name)).color(Color32::WHITE)).fill(theme::ACCENT());
            if ui.add(run).clicked() {
                result = FormResult::Run;
            }
            ui.label(RichText::new("Enter runs it · Esc cancels").size(11.5).color(theme::FAINT()));
        });
    });
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        result = FormResult::Cancel;
    }
    // Enter runs the command unless a list is open (where it picks).
    if ui.input(|i| i.key_pressed(egui::Key::Enter)) && !egui::Popup::is_any_open(ui.ctx()) {
        result = FormResult::Run;
    }
    result
}

fn option_field(ui: &mut Ui, o: &CommandOption, f: &mut Field, state: &State, channel: &Channel, command: u64) {
    let hint = if o.autocomplete { format!("{} (type a value)", o.description) } else { o.description.clone() };
    if !o.choices.is_empty() {
        let text = f.choice.and_then(|i| o.choices.get(i)).map_or_else(|| "Choose…".to_owned(), |c| c.name.clone());
        egui::ComboBox::from_id_salt(("command-choice", command, &o.name)).width(260.0).selected_text(text).show_ui(ui, |ui| {
            for (i, c) in o.choices.iter().enumerate() {
                if ui.selectable_label(f.choice == Some(i), &c.name).clicked() {
                    f.choice = Some(i);
                }
            }
        });
        return;
    }
    match o.kind {
        ty::BOOLEAN => {
            let text = match f.flag {
                Some(true) => "True",
                Some(false) => "False",
                None => "—",
            };
            egui::ComboBox::from_id_salt(("command-bool", command, &o.name)).width(120.0).selected_text(text).show_ui(ui, |ui| {
                ui.selectable_value(&mut f.flag, None, "—");
                ui.selectable_value(&mut f.flag, Some(true), "True");
                ui.selectable_value(&mut f.flag, Some(false), "False");
            });
        }
        ty::USER | ty::CHANNEL | ty::ROLE | ty::MENTIONABLE => {
            let text = f.picked.as_ref().map_or_else(|| "Choose…".to_owned(), |(_, l)| l.clone());
            egui::ComboBox::from_id_salt(("command-pick", command, &o.name)).width(260.0).selected_text(text).show_ui(ui, |ui| {
                ui.add(egui::TextEdit::singleline(&mut f.search).hint_text("Search").desired_width(f32::INFINITY));
                let list = picks(o.kind, &f.search, state, channel);
                if list.is_empty() {
                    ui.label(RichText::new("No matches.").color(theme::MUTED()));
                }
                for (id, label) in list {
                    if ui.selectable_label(f.picked.as_ref().is_some_and(|(p, _)| *p == id), &label).clicked() {
                        f.picked = Some((id, label));
                    }
                }
            });
        }
        ty::ATTACHMENT => {
            ui.label(RichText::new("Files can't be attached to commands yet.").color(theme::MUTED()));
        }
        _ => {
            ui.add(egui::TextEdit::singleline(&mut f.text).hint_text(hint).desired_width(320.0));
        }
    }
}

/// The commands matching what's typed after `/`, for the suggestion list: (index into the commands, name,
/// description, app name).
pub fn matching(index: &Index, typed: &str, limit: usize) -> Vec<(usize, String, String, String)> {
    index
        .search(typed, limit)
        .into_iter()
        .map(|c| {
            let i = index.commands.iter().position(|x| x.id == c.id).unwrap_or(0);
            let app = index.app(c.application_id).map(|a| a.name.clone()).unwrap_or_default();
            (i, c.name.clone(), c.description.clone(), app)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(options: Value) -> Command {
        let raw = json!({ "id": "1", "application_id": "2", "version": "3", "type": 1, "name": "x", "description": "d", "options": options });
        let mut c: Command = serde_json::from_value(raw.clone()).unwrap();
        c.raw = raw;
        c
    }

    #[test]
    fn values_check_required_numbers_and_choices() {
        let c = command(json!([
            { "type": 4, "name": "count", "description": "", "required": true, "min_value": 1, "max_value": 10 },
            { "type": 3, "name": "mode", "description": "", "choices": [{ "name": "Fast", "value": "f" }] },
            { "type": 5, "name": "loud", "description": "" }
        ]));
        let mut d = CommandDraft::new(c, "App".into());
        assert!(d.values().unwrap_err().contains("required"));
        d.fields.entry("count".into()).or_default().text = "2.5".into();
        assert!(d.values().unwrap_err().contains("whole"));
        d.fields.entry("count".into()).or_default().text = "11".into();
        assert!(d.values().unwrap_err().contains("1–10"));
        d.fields.entry("count".into()).or_default().text = "3".into();
        d.fields.entry("mode".into()).or_default().choice = Some(0);
        d.fields.entry("loud".into()).or_default().flag = Some(true);
        let (path, values) = d.values().unwrap();
        assert!(path.is_empty());
        assert_eq!(values, vec![json!({ "type": 4, "name": "count", "value": 3 }), json!({ "type": 3, "name": "mode", "value": "f" }), json!({ "type": 5, "name": "loud", "value": true })]);
    }

    #[test]
    fn subcommands_and_groups_become_choices() {
        let c = command(json!([
            { "type": 1, "name": "show", "description": "Show it", "options": [] },
            { "type": 2, "name": "role", "description": "", "options": [{ "type": 1, "name": "add", "description": "Add one", "options": [{ "type": 8, "name": "role", "description": "", "required": true }] }] }
        ]));
        let mut d = CommandDraft::new(c, "App".into());
        assert_eq!(d.variants.iter().map(|v| v.label.as_str()).collect::<Vec<_>>(), ["show", "role add"]);
        d.variant = 1;
        assert!(d.values().is_err(), "the role is required");
        d.fields.entry("role".into()).or_default().picked = Some(("77".into(), "@Mods".into()));
        let (path, values) = d.values().unwrap();
        assert_eq!(path, vec![(2, "role".to_owned()), (1, "add".to_owned())]);
        assert_eq!(values, vec![json!({ "type": 8, "name": "role", "value": "77" })]);
    }
}
