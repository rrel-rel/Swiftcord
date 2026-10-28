//! Threads and forum posts: browsing a channel's threads, opening, joining and leaving them, and starting new
//! ones. Active threads come from the gateway (THREAD_LIST_SYNC and friends); archived ones and forum posts'
//! opening messages are searched for page by page when a thread list or forum is opened.

use std::collections::HashMap;

use swiftcord_core::models::{Channel, Message};
use swiftcord_core::rest::ThreadPage;
use swiftcord_core::Snowflake;

use crate::app::{Action, App, AppEvent};

/// What's been fetched of one channel's threads.
#[derive(Default)]
pub struct Browser {
    /// Archived threads found so far, newest activity first (active ones are in the state).
    pub archived: Vec<Snowflake>,
    pub has_more: bool,
    pub loading: bool,
    pub loaded: bool,
    /// Forum posts' opening messages, by post.
    pub first_messages: HashMap<Snowflake, Message>,
}

/// The "start a thread" (or "new forum post") window.
pub struct Prompt {
    pub channel: Snowflake,
    /// Starting it from this message.
    pub message: Option<Snowflake>,
    pub name: String,
    /// A forum post's opening message.
    pub content: String,
    pub forum: bool,
    pub busy: bool,
}

pub fn toggle_panel(app: &mut App, channel: Snowflake) {
    if app.ui.threads_panel == Some(channel) {
        app.ui.threads_panel = None;
        return;
    }
    app.ui.pins = None;
    app.ui.threads_panel = Some(channel);
    app.ui.inbox = None;
    load(app, channel, false);
}

/// Loads the first page (active and archived), or with `more` the next page of archived threads.
pub fn load(app: &mut App, channel: Snowflake, more: bool) {
    let b = app.ui.thread_browsers.entry(channel).or_default();
    if b.loading || (b.loaded && !more) || (more && !b.has_more) {
        return;
    }
    b.loading = true;
    let offset = if more { b.archived.len() as u32 } else { 0 };
    let Some(s) = &app.session else { return };
    let rest = s.rest.clone();
    app.spawn(async move {
        let active = if more { None } else { Some(rest.search_threads(channel, false, 0).await.map_err(|e| e.to_string())) };
        let archived = rest.search_threads(channel, true, offset).await.map_err(|e| e.to_string());
        AppEvent::Threads { channel, active, archived }
    });
}

pub fn on_loaded(app: &mut App, channel: Snowflake, active: Option<Result<ThreadPage, String>>, archived: Result<ThreadPage, String>) {
    let mut error = None;
    let mut found_archived = Vec::new();
    let mut has_more = false;
    let mut first_messages = Vec::new();
    for (page, is_archived) in [(active, false), (Some(archived), true)] {
        match page {
            Some(Ok(page)) => {
                if is_archived {
                    found_archived.extend(page.threads.iter().map(|t| t.id));
                    has_more = page.has_more;
                }
                first_messages.extend(page.first_messages);
                add_threads(app, page.threads, &page.members);
            }
            Some(Err(e)) => error = Some(e),
            None => {}
        }
    }
    let b = app.ui.thread_browsers.entry(channel).or_default();
    b.loading = false;
    b.loaded = true;
    b.has_more = has_more;
    for id in found_archived {
        if !b.archived.contains(&id) {
            b.archived.push(id);
        }
    }
    for m in first_messages {
        // A post's opening message is posted in the post itself, so it carries the post's ID as its channel.
        b.first_messages.insert(m.channel_id, m);
    }
    if let Some(s) = &mut app.session {
        for m in b.first_messages.values() {
            s.state.cache_author(m);
        }
    }
    if let Some(e) = error {
        app.banner(format!("Couldn't load threads: {e}"));
    }
}

fn add_threads(app: &mut App, threads: Vec<Channel>, members: &[swiftcord_core::models::ThreadMember]) {
    let Some(s) = &mut app.session else { return };
    for mut t in threads {
        if let Some(m) = members.iter().find(|m| m.id == Some(t.id)) {
            t.member = Some(m.clone());
        }
        s.state.add_channel(t);
    }
}

/// Opens a thread, fetching it first if we don't know it (an archived thread mentioned in chat).
pub fn open(app: &mut App, thread: Snowflake) {
    let Some(s) = &app.session else { return };
    if s.state.channel(thread).is_some() {
        app.apply(Action::SelectChannel(thread));
        return;
    }
    let rest = s.rest.clone();
    app.spawn(async move { AppEvent::ThreadOpened(rest.channel(thread).await.map_err(|e| e.to_string())) });
}

pub fn on_opened(app: &mut App, result: Result<Channel, String>) {
    match result {
        Ok(thread) => {
            let id = thread.id;
            let parent = thread.parent_id;
            if let Some(s) = &mut app.session {
                s.state.add_channel(thread);
            }
            app.ui.thread_prompt = None;
            // Lists of the parent's threads refresh when next shown, to include a new post or thread.
            if let Some(b) = parent.and_then(|p| app.ui.thread_browsers.get_mut(&p)).filter(|b| !b.loading) {
                b.loaded = false;
            }
            app.apply(Action::SelectChannel(id));
        }
        Err(e) => {
            if let Some(p) = &mut app.ui.thread_prompt {
                p.busy = false;
            }
            app.banner(format!("Couldn't open the thread: {e}"));
        }
    }
}

pub fn open_prompt(app: &mut App, channel: Snowflake, message: Option<Snowflake>, name: String) {
    let forum = app.session.as_ref().and_then(|s| s.state.channel(channel)).is_some_and(|c| c.is_forum());
    app.ui.thread_prompt = Some(Prompt { channel, message, name, content: String::new(), forum, busy: false });
}

pub fn create(app: &mut App) {
    let Some(p) = &mut app.ui.thread_prompt else { return };
    let name = p.name.trim().to_owned();
    if name.is_empty() || p.busy || (p.forum && p.content.trim().is_empty()) {
        return;
    }
    p.busy = true;
    let (channel, message, content) = (p.channel, p.message, p.forum.then(|| p.content.trim().to_owned()));
    let Some(s) = &app.session else { return };
    let rest = s.rest.clone();
    app.spawn(async move {
        let result = match message {
            Some(m) => rest.create_thread_from_message(channel, m, &name).await,
            None => rest.create_thread(channel, &name, content.as_deref()).await,
        };
        AppEvent::ThreadOpened(result.map_err(|e| e.to_string()))
    });
}

pub fn set_joined(app: &mut App, thread: Snowflake, joined: bool) {
    let Some(s) = &mut app.session else { return };
    s.state.set_thread_joined(thread, joined);
    let rest = s.rest.clone();
    app.spawn(async move {
        let result = if joined { rest.join_thread(thread).await } else { rest.leave_thread(thread).await };
        match result {
            Ok(()) => AppEvent::Error(String::new()),
            Err(e) => AppEvent::ThreadJoinFailed { thread, joined, error: e.to_string() },
        }
    });
}

pub fn on_join_failed(app: &mut App, thread: Snowflake, joined: bool, error: String) {
    if let Some(s) = &mut app.session {
        s.state.set_thread_joined(thread, !joined);
    }
    let verb = if joined { "join" } else { "leave" };
    app.banner(format!("Couldn't {verb} the thread: {error}"));
}
