//! Carries out what the server dialogs ask for, and passes the results back to them.

use serde_json::Value;
use swiftcord_core::rest::{AuditLog, Ban};
use swiftcord_core::Snowflake;

use crate::app::{App, AppEvent};
use crate::ui::server_menu::{self, Kind, Which};

#[derive(Debug, Clone)]
pub enum ServerAction {
    Open(Snowflake, Which),
    CreateInvite { channel: Snowflake, max_age: u32, max_uses: u32, temporary: bool },
    SaveGuild(Snowflake, Value),
    SaveNotifications(Snowflake, Value),
    SavePrivacy(Value),
    Kick { guild: Snowflake, user: Snowflake, name: String, reason: String },
    Ban { guild: Snowflake, user: Snowflake, name: String, delete_seconds: u32, reason: String },
    /// `seconds: None` ends a timeout.
    Timeout { guild: Snowflake, user: Snowflake, name: String, seconds: Option<i64>, reason: String },
    Unban { guild: Snowflake, user: Snowflake, name: String },
    LoadBans(Snowflake),
    LoadAudit(Snowflake),
}

#[derive(Debug)]
pub enum ServerEvent {
    Invite(Result<String, String>),
    GuildInfo(Result<Value, String>),
    /// The privacy lists, and whether they were read back after saving.
    Privacy { result: Result<Value, String>, saved: bool },
    Bans(Result<Vec<Ban>, String>),
    Audit(Result<AuditLog, String>),
    /// Something finished: what to say about it.
    Done(Result<String, String>),
    /// As `Done`, taking them off the list of bans if it worked.
    Unbanned(Snowflake, Result<String, String>),
}

fn outcome(result: Result<(), swiftcord_core::rest::ApiError>, done: String) -> Result<String, String> {
    result.map(|()| done).map_err(|e| e.to_string())
}

fn reply(event: ServerEvent) -> AppEvent {
    AppEvent::Server(event)
}

pub fn apply(app: &mut App, action: ServerAction) {
    let Some(s) = &app.session else { return };
    let rest = s.rest.clone();
    if let ServerAction::Open(guild, which) = action {
        app.ui.server_dialog = Some(server_menu::open_dialog(&s.state, guild, which, app.ui.channel()));
        match which {
            Which::Settings => app.spawn(async move { reply(ServerEvent::GuildInfo(rest.guild_info(guild).await.map_err(|e| e.to_string()))) }),
            Which::Privacy => app.spawn(async move { reply(ServerEvent::Privacy { result: rest.user_settings().await.map_err(|e| e.to_string()), saved: false }) }),
            Which::Invite | Which::Moderation | Which::Notifications => {}
        }
        return;
    }
    if let Some(d) = &mut app.ui.server_dialog {
        d.busy = true;
        d.status = None;
    }
    match action {
        ServerAction::Open(..) => {}
        ServerAction::CreateInvite { channel, max_age, max_uses, temporary } => {
            app.spawn(async move { reply(ServerEvent::Invite(rest.create_invite(channel, max_age, max_uses, temporary).await.map_err(|e| e.to_string()))) });
        }
        ServerAction::SaveGuild(guild, change) => {
            // Read back, so a second save compares with what's now there.
            app.spawn(async move {
                match rest.modify_guild(guild, change).await {
                    Ok(()) => reply(ServerEvent::GuildInfo(rest.guild_info(guild).await.map_err(|e| e.to_string()))),
                    Err(e) => reply(ServerEvent::Done(Err(format!("Couldn't save: {e}")))),
                }
            });
        }
        ServerAction::SaveNotifications(guild, change) => {
            app.spawn(async move { reply(ServerEvent::Done(outcome(rest.modify_guild_settings(guild, change).await, "Saved.".into()))) });
        }
        ServerAction::SavePrivacy(change) => {
            app.spawn(async move {
                match rest.modify_user_settings(change).await {
                    Ok(()) => reply(ServerEvent::Privacy { result: rest.user_settings().await.map_err(|e| e.to_string()), saved: true }),
                    Err(e) => reply(ServerEvent::Done(Err(format!("Couldn't save: {e}")))),
                }
            });
        }
        ServerAction::Kick { guild, user, name, reason } => {
            app.spawn(async move { reply(ServerEvent::Done(outcome(rest.kick(guild, user, &reason).await, format!("Kicked {name}.")))) });
        }
        ServerAction::Ban { guild, user, name, delete_seconds, reason } => {
            app.spawn(async move { reply(ServerEvent::Done(outcome(rest.ban(guild, user, delete_seconds, &reason).await, format!("Banned {name}.")))) });
        }
        ServerAction::Timeout { guild, user, name, seconds, reason } => {
            let until = seconds.map(|s| chrono::Utc::now() + chrono::Duration::seconds(s));
            let done = if seconds.is_some() { format!("Timed out {name}.") } else { format!("Ended {name}'s timeout.") };
            app.spawn(async move { reply(ServerEvent::Done(outcome(rest.timeout(guild, user, until, &reason).await, done))) });
        }
        ServerAction::Unban { guild, user, name } => {
            app.spawn(async move { reply(ServerEvent::Unbanned(user, outcome(rest.unban(guild, user).await, format!("Unbanned {name}.")))) });
        }
        ServerAction::LoadBans(guild) => {
            app.spawn(async move { reply(ServerEvent::Bans(rest.bans(guild).await.map_err(|e| e.to_string()))) });
        }
        ServerAction::LoadAudit(guild) => {
            app.spawn(async move { reply(ServerEvent::Audit(rest.audit_log(guild).await.map_err(|e| e.to_string()))) });
        }
    }
}

fn status(r: Result<String, String>) -> Option<(bool, String)> {
    Some(match r {
        Ok(text) => (true, text),
        Err(e) => (false, e),
    })
}

pub fn on_event(app: &mut App, event: ServerEvent) {
    let Some(d) = &mut app.ui.server_dialog else { return };
    d.busy = false;
    let guild = d.guild;
    match event {
        ServerEvent::Invite(r) => match (r, &mut d.kind) {
            (Ok(code), Kind::Invite { link, .. }) => *link = Some(format!("https://discord.gg/{code}")),
            (Err(e), _) => d.status = status(Err(format!("Couldn't create the invite: {e}"))),
            _ => {}
        },
        ServerEvent::GuildInfo(r) => match (r, &mut d.kind) {
            (Ok(info), Kind::Settings { name, description, verification, notifications, original }) => {
                // Having had it before means this is it read back after saving.
                if original.is_some() {
                    d.status = status(Ok("Saved.".to_owned()));
                }
                *name = info["name"].as_str().unwrap_or_default().to_owned();
                *description = info["description"].as_str().unwrap_or_default().to_owned();
                *verification = info["verification_level"].as_u64().unwrap_or(0);
                *notifications = info["default_message_notifications"].as_u64().unwrap_or(0);
                *original = Some(info);
            }
            (Err(e), _) => d.status = status(Err(format!("Couldn't load the settings: {e}"))),
            _ => {}
        },
        ServerEvent::Privacy { result, saved } => match (result, &mut d.kind) {
            (Ok(settings), Kind::Privacy { settings: kept, dms, activity, joining }) => {
                if saved {
                    d.status = status(Ok("Saved.".to_owned()));
                }
                *dms = server_menu::privacy_allowed(&settings, "restricted_guilds", guild);
                *activity = server_menu::privacy_allowed(&settings, "activity_restricted_guild_ids", guild);
                *joining = server_menu::privacy_allowed(&settings, "activity_joining_restricted_guild_ids", guild);
                *kept = Some(settings);
            }
            (Err(e), _) => d.status = status(Err(format!("Couldn't load your privacy settings: {e}"))),
            _ => {}
        },
        ServerEvent::Bans(r) => {
            if let Kind::Moderation(m) = &mut d.kind {
                match r {
                    Ok(bans) => m.bans = Some(bans),
                    Err(e) => {
                        m.bans = Some(Vec::new());
                        d.status = status(Err(format!("Couldn't load the bans: {e}")));
                    }
                }
            }
        }
        ServerEvent::Audit(r) => {
            if let Kind::Moderation(m) = &mut d.kind {
                match r {
                    Ok(log) => m.audit = Some(log),
                    Err(e) => {
                        m.audit = Some(AuditLog::default());
                        d.status = status(Err(format!("Couldn't load the audit log: {e}")));
                    }
                }
            }
        }
        ServerEvent::Done(r) => d.status = status(r),
        ServerEvent::Unbanned(user, r) => {
            if let (true, Kind::Moderation(m)) = (r.is_ok(), &mut d.kind) {
                if let Some(bans) = &mut m.bans {
                    bans.retain(|b| b.user.id != user);
                }
            }
            d.status = status(r);
        }
    }
}
