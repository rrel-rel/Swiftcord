//! The server menu (from the sidebar's header or a right-click on the server's icon) and its dialogs: invites,
//! server settings, moderation, and our own notification and privacy settings for that server.

use chrono::Utc;
use egui::{self, Color32, RichText, Ui, Vec2};
use serde_json::{json, Value};
use swiftcord_core::models::ChannelType;
use swiftcord_core::permissions::perm;
use swiftcord_core::rest::{AuditLog, Ban};
use swiftcord_core::state::State;
use swiftcord_core::Snowflake;

use crate::app::{Action, Session, UiState};
use crate::server_glue::ServerAction;
use crate::theme;

const INVITE_AGES: [(u32, &str); 7] = [(1800, "30 minutes"), (3600, "1 hour"), (21_600, "6 hours"), (43_200, "12 hours"), (86_400, "1 day"), (604_800, "7 days"), (0, "Never")];
const INVITE_USES: [(u32, &str); 6] = [(0, "No limit"), (1, "1 use"), (5, "5 uses"), (10, "10 uses"), (25, "25 uses"), (100, "100 uses")];
const TIMEOUTS: [(i64, &str); 6] = [(60, "60 seconds"), (300, "5 minutes"), (600, "10 minutes"), (3600, "1 hour"), (86_400, "1 day"), (604_800, "1 week")];
const BAN_DELETES: [(u32, &str); 4] = [(0, "Don't delete any"), (3600, "Previous hour"), (86_400, "Previous day"), (604_800, "Previous 7 days")];
const VERIFICATION: [(u64, &str); 5] = [(0, "None"), (1, "Low: verified email"), (2, "Medium: on Discord for 5 minutes"), (3, "High: in this server for 10 minutes"), (4, "Highest: verified phone")];
const NOTIFY: [(u8, &str); 3] = [(0, "All Messages"), (1, "Only @mentions"), (2, "Nothing")];

/// A server dialog that's open.
pub struct Dialog {
    pub guild: Snowflake,
    pub kind: Kind,
    /// The outcome of what was last asked for: whether it worked, and what to say.
    pub status: Option<(bool, String)>,
    pub busy: bool,
}

pub enum Kind {
    Invite { channel: Option<Snowflake>, max_age: u32, max_uses: u32, temporary: bool, link: Option<String> },
    Settings { name: String, description: String, verification: u64, notifications: u64, original: Option<Value> },
    Moderation(Moderation),
    Notifications { level: u8, suppress_everyone: bool, suppress_roles: bool, muted: bool, mute_events: bool, highlights: bool, mobile_push: bool },
    Privacy { settings: Option<Value>, dms: bool, activity: bool, joining: bool },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ModTab {
    Members,
    Bans,
    AuditLog,
}

pub struct Moderation {
    pub tab: ModTab,
    pub query: String,
    pub reason: String,
    pub ban_delete: u32,
    pub confirm: Option<ModAct>,
    pub bans: Option<Vec<Ban>>,
    pub audit: Option<AuditLog>,
}

/// A moderator's action on someone, waiting to be confirmed.
#[derive(Clone)]
pub enum ModAct {
    Kick(Snowflake, String),
    Ban(Snowflake, String),
    Timeout(Snowflake, String, i64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    Invite,
    Settings,
    Moderation,
    Notifications,
    Privacy,
}

/// Opens a dialog on `guild`, with what's already known filled in.
pub fn open_dialog(state: &State, guild: Snowflake, which: Which, channel: Option<Snowflake>) -> Dialog {
    let kind = match which {
        Which::Invite => {
            // The open channel if an invite can be made to it, else the first that can.
            let usable = |id: Snowflake| state.channel(id).is_some_and(|c| c.guild_id == Some(guild) && !c.is_thread() && c.kind != ChannelType::Category && state.permissions(id) & perm::CREATE_INSTANT_INVITE != 0);
            let first = state.channel_tree(guild).into_iter().flat_map(|(_, cs)| cs).map(|c| c.id).find(|id| usable(*id));
            Kind::Invite { channel: channel.filter(|c| usable(*c)).or(first), max_age: 604_800, max_uses: 0, temporary: false, link: None }
        }
        Which::Settings => Kind::Settings { name: state.guild(guild).map(|g| g.name.clone()).unwrap_or_default(), description: String::new(), verification: 0, notifications: 0, original: None },
        Which::Moderation => {
            let perms = state.guild_permissions(guild);
            let tab = if perms & (perm::KICK_MEMBERS | perm::BAN_MEMBERS | perm::MODERATE_MEMBERS) != 0 { ModTab::Members } else { ModTab::AuditLog };
            Kind::Moderation(Moderation { tab, query: String::new(), reason: String::new(), ban_delete: 0, confirm: None, bans: None, audit: None })
        }
        Which::Notifications => {
            let s = state.guild_settings(guild).cloned().unwrap_or_default();
            // 3 means the server's default.
            let level = if s.message_notifications > 2 { state.guild(guild).map_or(0, |g| g.default_message_notifications) } else { s.message_notifications };
            Kind::Notifications { level, suppress_everyone: s.suppress_everyone, suppress_roles: s.suppress_roles, muted: s.is_muted(), mute_events: s.mute_scheduled_events, highlights: s.notify_highlights != 1, mobile_push: s.mobile_push }
        }
        Which::Privacy => Kind::Privacy { settings: None, dms: true, activity: true, joining: true },
    };
    Dialog { guild, kind, status: None, busy: false }
}

/// The menu's entries, leaving out what we don't have permission for.
pub fn items(ui: &mut Ui, state: &State, guild: Snowflake, actions: &mut Vec<Action>) {
    let perms = state.guild_permissions(guild);
    let mut item = |ui: &mut Ui, label: &str, which: Which| {
        if ui.button(label).clicked() {
            actions.push(Action::Server(ServerAction::Open(guild, which)));
            ui.close();
        }
    };
    ui.set_min_width(190.0);
    // Channels can allow invites that the server's roles don't, so this looks at each one.
    let can_invite = state.channel_tree(guild).into_iter().flat_map(|(_, cs)| cs).any(|c| state.permissions(c.id) & perm::CREATE_INSTANT_INVITE != 0);
    if can_invite {
        item(ui, "Invite People", Which::Invite);
    }
    if perms & perm::MANAGE_GUILD != 0 {
        item(ui, "Server Settings", Which::Settings);
    }
    if perms & (perm::KICK_MEMBERS | perm::BAN_MEMBERS | perm::MODERATE_MEMBERS | perm::VIEW_AUDIT_LOG) != 0 {
        item(ui, "Moderation", Which::Moderation);
    }
    item(ui, "Notification Settings", Which::Notifications);
    item(ui, "Privacy Settings", Which::Privacy);
}

/// Picks one of `options` in a dropdown.
fn choice<T: Copy + PartialEq>(ui: &mut Ui, id: &str, value: &mut T, options: &[(T, &str)]) {
    let selected = options.iter().find(|(v, _)| v == value).map_or("", |(_, l)| *l);
    egui::ComboBox::from_id_salt(id).selected_text(selected).width(ui.available_width().min(300.0)).show_ui(ui, |ui| {
        for (v, label) in options {
            ui.selectable_value(value, *v, *label);
        }
    });
}

fn title(ui: &mut Ui, text: &str, guild: &str) {
    ui.label(RichText::new(text).font(theme::font_bold(20.0)).color(theme::HEADER()));
    ui.add(egui::Label::new(RichText::new(guild).color(theme::MUTED())).truncate());
    ui.add_space(8.0);
}

fn field(ui: &mut Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(text.to_uppercase()).font(theme::font_semibold(12.0)).color(theme::MUTED()));
}

fn note(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(theme::FAINT()));
}

/// The usual bottom row: a primary button, and one that closes the dialog. Returns (primary clicked, close clicked).
fn buttons(ui: &mut Ui, primary: &str, enabled: bool) -> (bool, bool) {
    let mut out = (false, false);
    ui.add_space(10.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let b = egui::Button::new(RichText::new(primary).color(Color32::WHITE)).min_size(Vec2::new(90.0, 32.0));
        out.0 = ui.add_enabled(enabled, b).clicked();
        out.1 = ui.add(egui::Button::new("Close").min_size(Vec2::new(80.0, 32.0))).clicked();
    });
    out
}

pub fn show(ui: &Ui, s: &Session, uis: &mut UiState, actions: &mut Vec<Action>) {
    let Some(d) = &mut uis.server_dialog else { return };
    let state = &s.state;
    let guild = d.guild;
    let Some(name) = state.guild(guild).map(|g| g.name.clone()) else {
        uis.server_dialog = None;
        return;
    };
    let (status, busy) = (d.status.clone(), d.busy);
    let mut close = false;
    let modal = egui::Modal::new(egui::Id::unique("server-dialog")).show(ui.ctx(), |ui| {
        ui.set_width(if matches!(d.kind, Kind::Moderation(_)) { 620.0 } else { 440.0 });
        match &mut d.kind {
            Kind::Invite { channel, max_age, max_uses, temporary, link } => {
                title(ui, "Invite People", &name);
                let channels: Vec<(Snowflake, String)> = state
                    .channel_tree(guild)
                    .into_iter()
                    .flat_map(|(_, cs)| cs)
                    .filter(|c| state.permissions(c.id) & perm::CREATE_INSTANT_INVITE != 0)
                    .map(|c| (c.id, c.label().to_string()))
                    .collect();
                field(ui, "Invite to channel");
                let options: Vec<(Option<Snowflake>, &str)> = channels.iter().map(|(id, n)| (Some(*id), n.as_str())).collect();
                choice(ui, "invite-channel", channel, &options);
                field(ui, "Expire after");
                choice(ui, "invite-age", max_age, &INVITE_AGES);
                field(ui, "Maximum uses");
                choice(ui, "invite-uses", max_uses, &INVITE_USES);
                ui.add_space(6.0);
                ui.checkbox(temporary, "Temporary membership");
                note(ui, "People who join with it are removed when they disconnect, unless given a role.");
                if let Some(l) = link.as_ref() {
                    field(ui, "Invite link");
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(l.as_str()).color(theme::LINK()));
                        if ui.button("Copy").clicked() {
                            actions.push(Action::Copy(l.clone()));
                        }
                    });
                }
                let (go, closed) = buttons(ui, "Create Link", channel.is_some() && !busy);
                close = closed;
                if let (true, Some(c)) = (go, *channel) {
                    actions.push(Action::Server(ServerAction::CreateInvite { channel: c, max_age: *max_age, max_uses: *max_uses, temporary: *temporary }));
                }
            }
            Kind::Settings { name: new_name, description, verification, notifications, original } => {
                title(ui, "Server Settings", &name);
                field(ui, "Server name");
                ui.add(egui::TextEdit::singleline(new_name).char_limit(100).desired_width(f32::INFINITY));
                field(ui, "Description");
                ui.add(egui::TextEdit::multiline(description).char_limit(300).desired_rows(3).desired_width(f32::INFINITY));
                note(ui, "Discord only uses a description for Community servers.");
                field(ui, "Verification level");
                choice(ui, "server-verification", verification, &VERIFICATION);
                field(ui, "Default notifications");
                choice(ui, "server-notifications", notifications, &[(0, "All Messages"), (1, "Only @mentions")]);
                let change = original.as_ref().map(|o| settings_change(o, new_name, description, *verification, *notifications)).unwrap_or_default();
                let valid = (2..=100).contains(&new_name.trim().chars().count());
                let (go, closed) = buttons(ui, "Save Changes", valid && !change.is_empty() && !busy);
                close = closed;
                if go {
                    actions.push(Action::Server(ServerAction::SaveGuild(guild, Value::Object(change))));
                }
            }
            Kind::Notifications { level, suppress_everyone, suppress_roles, muted, mute_events, highlights, mobile_push } => {
                title(ui, "Notification Settings", &name);
                ui.checkbox(muted, "Mute this server");
                field(ui, "Notify me about");
                for (v, label) in NOTIFY {
                    ui.radio_value(level, v, label);
                }
                ui.add_space(8.0);
                ui.checkbox(suppress_everyone, "Suppress @everyone and @here");
                ui.checkbox(suppress_roles, "Suppress all role @mentions");
                ui.checkbox(highlights, "Show highlights");
                ui.checkbox(mute_events, "Mute new events");
                ui.checkbox(mobile_push, "Mobile push notifications");
                let (go, closed) = buttons(ui, "Save Changes", !busy);
                close = closed;
                if go {
                    let change = json!({
                        "muted": *muted,
                        // Muting here lasts until undone, and unmuting ends a mute set for a while.
                        "mute_config": null,
                        "message_notifications": *level,
                        "suppress_everyone": *suppress_everyone,
                        "suppress_roles": *suppress_roles,
                        "notify_highlights": if *highlights { 2 } else { 1 },
                        "mute_scheduled_events": *mute_events,
                        "mobile_push": *mobile_push,
                    });
                    actions.push(Action::Server(ServerAction::SaveNotifications(guild, change)));
                }
            }
            Kind::Privacy { settings, dms, activity, joining } => {
                title(ui, "Privacy Settings", &name);
                if settings.is_none() {
                    ui.add(egui::Spinner::new());
                }
                ui.add_enabled_ui(settings.is_some(), |ui| {
                    ui.checkbox(dms, "Direct messages from server members");
                    note(ui, "Lets people in this server message you without being your friend.");
                    ui.add_space(6.0);
                    ui.checkbox(activity, "Share my activity status here");
                    note(ui, "Shows people in this server what you're playing or doing.");
                    ui.add_space(6.0);
                    ui.checkbox(joining, "Let people here join my activity");
                });
                let change = settings.as_ref().map(|o| privacy_change(o, guild, *dms, *activity, *joining)).unwrap_or_default();
                let (go, closed) = buttons(ui, "Save Changes", !change.is_empty() && !busy);
                close = closed;
                if go {
                    actions.push(Action::Server(ServerAction::SavePrivacy(Value::Object(change))));
                }
            }
            Kind::Moderation(m) => {
                title(ui, "Moderation", &name);
                moderation(ui, state, guild, m, busy, actions);
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close = ui.add(egui::Button::new("Close").min_size(Vec2::new(80.0, 32.0))).clicked();
                });
            }
        }
        if let Some((ok, text)) = &status {
            ui.add_space(6.0);
            ui.label(RichText::new(text).size(13.0).color(if *ok { theme::GREEN() } else { theme::RED() }));
        }
    });
    if close || modal.should_close() {
        uis.server_dialog = None;
    }
}

/// The fields that differ from what the server has.
fn settings_change(original: &Value, name: &str, description: &str, verification: u64, notifications: u64) -> serde_json::Map<String, Value> {
    let mut out = serde_json::Map::new();
    if original["name"].as_str() != Some(name.trim()) {
        out.insert("name".into(), json!(name.trim()));
    }
    if original["description"].as_str().unwrap_or("") != description.trim() {
        out.insert("description".into(), json!(description.trim()));
    }
    if original["verification_level"].as_u64() != Some(verification) {
        out.insert("verification_level".into(), json!(verification));
    }
    if original["default_message_notifications"].as_u64() != Some(notifications) {
        out.insert("default_message_notifications".into(), json!(notifications));
    }
    out
}

/// The privacy lists that need changing: each names the servers where the thing is turned off.
fn privacy_change(settings: &Value, guild: Snowflake, dms: bool, activity: bool, joining: bool) -> serde_json::Map<String, Value> {
    let id = guild.to_string();
    let mut out = serde_json::Map::new();
    for (key, allowed) in [("restricted_guilds", dms), ("activity_restricted_guild_ids", activity), ("activity_joining_restricted_guild_ids", joining)] {
        let mut list: Vec<String> = settings[key].as_array().map(|l| l.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default();
        if list.contains(&id) == allowed {
            list.retain(|g| *g != id);
            if !allowed {
                list.push(id.clone());
            }
            out.insert(key.into(), json!(list));
        }
    }
    out
}

/// Whether a privacy list leaves this server's option on.
pub fn privacy_allowed(settings: &Value, key: &str, guild: Snowflake) -> bool {
    let id = guild.to_string();
    !settings[key].as_array().is_some_and(|l| l.iter().any(|v| v.as_str() == Some(id.as_str())))
}

fn moderation(ui: &mut Ui, state: &State, guild: Snowflake, m: &mut Moderation, busy: bool, actions: &mut Vec<Action>) {
    let perms = state.guild_permissions(guild);
    if let Some(act) = m.confirm.clone() {
        let (verb, who, detail) = match &act {
            ModAct::Kick(_, n) => ("Kick", n.clone(), "They can rejoin with a new invite.".to_owned()),
            ModAct::Ban(_, n) => ("Ban", n.clone(), "They can't rejoin until unbanned.".to_owned()),
            ModAct::Timeout(_, n, secs) => ("Time out", n.clone(), format!("They can't talk here for {}.", TIMEOUTS.iter().find(|t| t.0 == *secs).map_or("a while", |t| t.1))),
        };
        ui.label(RichText::new(format!("{verb} {who}?")).font(theme::font_bold(16.0)).color(theme::HEADER()));
        ui.label(RichText::new(detail).color(theme::MUTED()));
        field(ui, "Reason (shown in the audit log)");
        ui.add(egui::TextEdit::singleline(&mut m.reason).char_limit(400).desired_width(f32::INFINITY));
        if matches!(act, ModAct::Ban(..)) {
            field(ui, "Delete their messages from");
            choice(ui, "ban-delete", &mut m.ban_delete, &BAN_DELETES);
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy, egui::Button::new(RichText::new(verb).color(Color32::WHITE)).fill(theme::RED()).min_size(Vec2::new(90.0, 32.0))).clicked() {
                let reason = m.reason.trim().to_owned();
                let action = match &act {
                    ModAct::Kick(u, n) => ServerAction::Kick { guild, user: *u, name: n.clone(), reason },
                    ModAct::Ban(u, n) => ServerAction::Ban { guild, user: *u, name: n.clone(), delete_seconds: m.ban_delete, reason },
                    ModAct::Timeout(u, n, secs) => ServerAction::Timeout { guild, user: *u, name: n.clone(), seconds: Some(*secs), reason },
                };
                actions.push(Action::Server(action));
                m.confirm = None;
                m.reason.clear();
            }
            if ui.add(egui::Button::new("Cancel").min_size(Vec2::new(80.0, 32.0))).clicked() {
                m.confirm = None;
            }
        });
        return;
    }
    ui.horizontal(|ui| {
        if perms & (perm::KICK_MEMBERS | perm::BAN_MEMBERS | perm::MODERATE_MEMBERS) != 0 {
            ui.selectable_value(&mut m.tab, ModTab::Members, "Members");
        }
        if perms & perm::BAN_MEMBERS != 0 && ui.selectable_value(&mut m.tab, ModTab::Bans, "Bans").clicked() && m.bans.is_none() {
            actions.push(Action::Server(ServerAction::LoadBans(guild)));
        }
        if perms & perm::VIEW_AUDIT_LOG != 0 && ui.selectable_value(&mut m.tab, ModTab::AuditLog, "Audit Log").clicked() && m.audit.is_none() {
            actions.push(Action::Server(ServerAction::LoadAudit(guild)));
        }
    });
    ui.separator();
    let scroll = egui::ScrollArea::vertical().max_height(360.0).auto_shrink([false, true]);
    match m.tab {
        ModTab::Members => {
            let r = ui.add(egui::TextEdit::singleline(&mut m.query).hint_text("Search members").desired_width(f32::INFINITY));
            if r.changed() && !m.query.trim().is_empty() {
                actions.push(Action::SearchMembers { guild, query: m.query.trim().to_owned() });
            }
            ui.add_space(4.0);
            let owner = state.guild(guild).map(|g| g.owner_id);
            let me = state.me.as_ref().map(|u| u.id);
            let found: Vec<Snowflake> = state.search_guild_members(guild, &m.query, 50).into_iter().filter(|id| Some(*id) != me && Some(*id) != owner).collect();
            let shown = scroll.show(ui, |ui| {
                for &id in &found {
                    member_row(ui, state, guild, id, perms, busy, m, actions);
                }
                if found.is_empty() {
                    ui.label(RichText::new("Nobody found. Type a name to search the whole server.").color(theme::MUTED()));
                }
            });
            crate::ui::widgets::scrollbar(ui, &shown);
        }
        ModTab::Bans => match &m.bans {
            None => {
                ui.add(egui::Spinner::new());
            }
            Some(bans) => {
                let shown = scroll.show(ui, |ui| {
                    for b in bans {
                        ui.horizontal(|ui| {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.add_enabled(!busy, egui::Button::new("Unban")).clicked() {
                                    actions.push(Action::Server(ServerAction::Unban { guild, user: b.user.id, name: b.user.display_name().to_owned() }));
                                }
                                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                    ui.label(RichText::new(b.user.display_name()).color(theme::TEXT()));
                                    ui.add(egui::Label::new(RichText::new(b.reason.as_deref().unwrap_or("No reason given")).size(12.0).color(theme::FAINT())).truncate());
                                });
                            });
                        });
                    }
                    if bans.is_empty() {
                        ui.label(RichText::new("Nobody is banned.").color(theme::MUTED()));
                    }
                });
                crate::ui::widgets::scrollbar(ui, &shown);
            }
        },
        ModTab::AuditLog => match &m.audit {
            None => {
                ui.add(egui::Spinner::new());
                // Opened straight on this tab, for someone who can only read the log.
                if !busy {
                    actions.push(Action::Server(ServerAction::LoadAudit(guild)));
                }
            }
            Some(log) => {
                let shown = scroll.show(ui, |ui| {
                    for line in audit_lines(state, guild, log) {
                        ui.add(egui::Label::new(RichText::new(line).size(13.0).color(theme::TEXT())).wrap());
                    }
                    if log.audit_log_entries.is_empty() {
                        ui.label(RichText::new("Nothing has been logged.").color(theme::MUTED()));
                    }
                });
                crate::ui::widgets::scrollbar(ui, &shown);
            }
        },
    }
}

/// A member, with what we're allowed to do to them. Discord refuses anyone ranked at or above us, which comes
/// back as the dialog's status.
#[allow(clippy::too_many_arguments)]
fn member_row(ui: &mut Ui, state: &State, guild: Snowflake, id: Snowflake, perms: u64, busy: bool, m: &mut Moderation, actions: &mut Vec<Action>) {
    let name = state.display_name(id, Some(guild));
    let timed_out = state.member(guild, id).and_then(|m| m.communication_disabled_until).is_some_and(|t| t > Utc::now());
    ui.horizontal(|ui| {
        ui.add_enabled_ui(!busy, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if perms & perm::BAN_MEMBERS != 0 && ui.button(RichText::new("Ban").color(theme::RED())).clicked() {
                    m.confirm = Some(ModAct::Ban(id, name.clone()));
                }
                if perms & perm::KICK_MEMBERS != 0 && ui.button(RichText::new("Kick").color(theme::RED())).clicked() {
                    m.confirm = Some(ModAct::Kick(id, name.clone()));
                }
                if perms & perm::MODERATE_MEMBERS != 0 {
                    if timed_out {
                        if ui.button("End Timeout").clicked() {
                            actions.push(Action::Server(ServerAction::Timeout { guild, user: id, name: name.clone(), seconds: None, reason: String::new() }));
                        }
                    } else {
                        ui.menu_button("Timeout", |ui| {
                            for (secs, label) in TIMEOUTS {
                                if ui.button(label).clicked() {
                                    m.confirm = Some(ModAct::Timeout(id, name.clone(), secs));
                                    ui.close();
                                }
                            }
                        });
                    }
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let tag = state.user(id).map(|u| u.tag()).unwrap_or_default();
                    ui.label(RichText::new(&name).color(theme::TEXT()));
                    ui.add(egui::Label::new(RichText::new(tag).size(12.0).color(theme::FAINT())).truncate());
                });
            });
        });
    });
}

fn audit_action(kind: u32) -> &'static str {
    match kind {
        1 => "updated the server",
        10 => "created a channel",
        11 => "updated a channel",
        12 => "deleted a channel",
        13..=15 => "changed channel permissions",
        20 => "kicked",
        21 => "pruned members",
        22 => "banned",
        23 => "unbanned",
        24 => "updated member",
        25 => "changed the roles of",
        26 => "moved members in voice",
        27 => "disconnected members from voice",
        28 => "added a bot:",
        30 => "created a role",
        31 => "updated a role",
        32 => "deleted a role",
        40 => "created an invite",
        41 => "updated an invite",
        42 => "deleted an invite",
        50..=52 => "changed a webhook",
        60..=62 => "changed an emoji",
        72 | 73 => "deleted messages",
        74 => "pinned a message",
        75 => "unpinned a message",
        90..=92 => "changed a sticker",
        110..=112 => "changed a thread",
        130..=132 => "changed a soundboard sound",
        140..=142 => "changed an AutoMod rule",
        143..=145 => "AutoMod acted on",
        _ => "made a change",
    }
}

fn audit_lines(state: &State, guild: Snowflake, log: &AuditLog) -> Vec<String> {
    let name = |id: Snowflake| log.users.iter().find(|u| u.id == id).map(|u| u.display_name().to_owned()).unwrap_or_else(|| state.display_name(id, Some(guild)));
    log.audit_log_entries
        .iter()
        .map(|e| {
            let when = e.id.created_at().with_timezone(&chrono::Local).format("%b %-d, %-I:%M %p");
            let who = e.user_id.map_or_else(|| "Someone".to_owned(), name);
            // Only people are worth naming as a target; other kinds of ID mean nothing to read.
            let target = e.target_id.as_deref().and_then(|t| t.parse::<u64>().ok()).map(Snowflake).filter(|id| log.users.iter().any(|u| u.id == *id)).map(name);
            let mut line = format!("{when}  {who} {}", audit_action(e.action_type));
            if let Some(t) = target {
                line.push_str(&format!(" {t}"));
            }
            if let Some(r) = e.reason.as_deref().filter(|r| !r.is_empty()) {
                line.push_str(&format!(" ({r})"));
            }
            line
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privacy_lists_only_change_when_they_must() {
        let g = Snowflake(5);
        let s = json!({ "restricted_guilds": ["5", "6"], "activity_restricted_guild_ids": ["6"] });
        assert!(!privacy_allowed(&s, "restricted_guilds", g));
        assert!(privacy_allowed(&s, "activity_restricted_guild_ids", g));
        // As they are: nothing to send.
        assert!(privacy_change(&s, g, false, true, true).is_empty());
        let c = privacy_change(&s, g, true, false, true);
        assert_eq!(c["restricted_guilds"], json!(["6"]));
        assert_eq!(c["activity_restricted_guild_ids"], json!(["6", "5"]));
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn settings_only_sends_what_changed() {
        let o = json!({ "name": "a b", "description": null, "verification_level": 1, "default_message_notifications": 0 });
        assert!(settings_change(&o, "a b", "", 1, 0).is_empty());
        let c = settings_change(&o, " new ", "", 2, 0);
        assert_eq!(c["name"], json!("new"));
        assert_eq!(c["verification_level"], json!(2));
        assert_eq!(c.len(), 2);
    }
}
