//! The mentions inbox: recent messages that mention us, across servers and DMs, newest first and a page at
//! a time, as Discord keeps them. Mentions arriving while it's open are added at the top.

use swiftcord_core::events::Event;
use swiftcord_core::models::Message;
use swiftcord_core::rest::MENTIONS_PAGE;
use swiftcord_core::state::State;
use swiftcord_core::Snowflake;

use crate::app::{App, AppEvent};

pub struct Inbox {
    pub messages: Vec<Message>,
    pub loading: bool,
    pub error: Option<String>,
    /// The last page came back short: there are no older mentions.
    pub reached_end: bool,
    /// Only this server's mentions.
    pub guild: Option<Snowflake>,
    /// Include @everyone/@here, and role mentions.
    pub everyone: bool,
    pub roles: bool,
    generation: u64,
}

impl Inbox {
    /// Whether a mention passes the filters.
    fn wants(&self, m: &Message, guild: Option<Snowflake>, direct: bool) -> bool {
        self.guild.is_none_or(|g| guild == Some(g)) && (direct || (self.everyone && m.mention_everyone) || (self.roles && !m.mention_roles.is_empty()))
    }
}

pub fn toggle(app: &mut App) {
    if app.ui.inbox.take().is_some() {
        return;
    }
    app.ui.search = None;
    app.ui.pins = None;
    app.ui.threads_panel = None;
    app.ui.inbox = Some(Inbox { messages: Vec::new(), loading: false, error: None, reached_end: false, guild: None, everyone: true, roles: true, generation: 0 });
    load(app, None);
}

pub fn set_filters(app: &mut App, guild: Option<Snowflake>, everyone: bool, roles: bool) {
    let Some(inbox) = &mut app.ui.inbox else { return };
    (inbox.guild, inbox.everyone, inbox.roles) = (guild, everyone, roles);
    inbox.messages.clear();
    inbox.reached_end = false;
    load(app, None);
}

pub fn more(app: &mut App) {
    let Some(inbox) = &app.ui.inbox else { return };
    if inbox.loading || inbox.reached_end {
        return;
    }
    let before = inbox.messages.last().map(|m| m.id);
    load(app, before);
}

fn load(app: &mut App, before: Option<Snowflake>) {
    let (Some(s), Some(inbox)) = (&app.session, &mut app.ui.inbox) else { return };
    inbox.generation += 1;
    inbox.loading = true;
    inbox.error = None;
    let (generation, guild, everyone, roles) = (inbox.generation, inbox.guild, inbox.everyone, inbox.roles);
    let rest = s.rest.clone();
    app.spawn(async move {
        let result = rest.mentions(before, guild, everyone, roles).await.map_err(|e| e.to_string());
        AppEvent::Mentions { generation, result }
    });
}

pub fn on_loaded(app: &mut App, generation: u64, result: Result<Vec<Message>, String>) {
    if app.ui.inbox.as_ref().is_none_or(|i| i.generation != generation) {
        return;
    }
    if let (Ok(page), Some(s)) = (&result, &mut app.session) {
        for m in page {
            s.state.cache_author(m);
        }
    }
    let Some(inbox) = &mut app.ui.inbox else { return };
    inbox.loading = false;
    match result {
        Ok(page) => {
            inbox.reached_end = page.len() < MENTIONS_PAGE;
            // A mention that arrived live while the page loaded is already listed.
            let page: Vec<Message> = page.into_iter().filter(|m| !inbox.messages.iter().any(|have| have.id == m.id)).collect();
            inbox.messages.extend(page);
        }
        Err(e) => inbox.error = Some(e),
    }
}

/// Takes a message off the list, and out of Discord's recent mentions.
pub fn dismiss(app: &mut App, message: Snowflake) {
    let (Some(s), Some(inbox)) = (&app.session, &mut app.ui.inbox) else { return };
    inbox.messages.retain(|m| m.id != message);
    let rest = s.rest.clone();
    app.spawn(async move {
        match rest.dismiss_mention(message).await {
            Ok(()) => AppEvent::Error(String::new()),
            Err(e) => AppEvent::Error(format!("Couldn't remove the mention: {e}")),
        }
    });
}

/// Keeps an open inbox current: new mentions go at the top, and edits and deletions show.
pub fn on_event(inbox: &mut Option<Inbox>, state: &State, event: &Event) {
    let Some(inbox) = inbox else { return };
    match event {
        Event::MessageCreate(m) => {
            let me = state.me.as_ref().map(|u| u.id);
            if me == Some(m.author.id) || !state.mentions_me(m) {
                return;
            }
            let guild = m.guild_id.or_else(|| state.channel(m.channel_id).and_then(|c| c.guild_id));
            let direct = m.mentions.iter().any(|u| Some(u.id) == me);
            if inbox.wants(m, guild, direct) && !inbox.messages.iter().any(|have| have.id == m.id) {
                inbox.messages.insert(0, (**m).clone());
            }
        }
        Event::MessageUpdate(u) => {
            let Some(m) = inbox.messages.iter_mut().find(|m| m.id == u.id) else { return };
            if let Some(c) = &u.content {
                m.content.clone_from(c);
            }
            if u.edited_timestamp.is_some() {
                m.edited_timestamp = u.edited_timestamp;
            }
            if let Some(v) = &u.embeds {
                m.embeds.clone_from(v);
            }
            if let Some(v) = &u.attachments {
                m.attachments.clone_from(v);
            }
        }
        Event::MessageDelete(d) => inbox.messages.retain(|m| m.id != d.id),
        Event::MessageDeleteBulk(d) => inbox.messages.retain(|m| !d.ids.contains(&m.id)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inbox(guild: Option<Snowflake>, everyone: bool, roles: bool) -> Inbox {
        Inbox { messages: Vec::new(), loading: false, error: None, reached_end: false, guild, everyone, roles, generation: 0 }
    }

    #[test]
    fn filters_follow_discords_options() {
        let everyone = Message { mention_everyone: true, ..Default::default() };
        let role = Message { mention_roles: vec![Snowflake(7)], ..Default::default() };
        let direct = Message::default();
        let all = inbox(None, true, true);
        assert!(all.wants(&everyone, Some(Snowflake(1)), false));
        assert!(all.wants(&role, None, false));
        let quiet = inbox(None, false, false);
        assert!(!quiet.wants(&everyone, None, false));
        assert!(!quiet.wants(&role, None, false));
        assert!(quiet.wants(&direct, None, true));
        let one_server = inbox(Some(Snowflake(1)), true, true);
        assert!(one_server.wants(&direct, Some(Snowflake(1)), true));
        assert!(!one_server.wants(&direct, Some(Snowflake(2)), true));
        assert!(!one_server.wants(&direct, None, true));
    }
}
