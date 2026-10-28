//! Message search: the query from the header's search box is parsed (words and filters like `from:` and
//! `in:`), names are matched to the server's members and channels, and Discord's search is asked a page at
//! a time. A server Discord hasn't indexed yet is asked again after the wait it gives.

use swiftcord_core::models::{ChannelType, Message};
use swiftcord_core::rest::SearchResult;
use swiftcord_core::search::Query;
use swiftcord_core::Snowflake;

use crate::app::{App, AppEvent};

const MAX_INDEX_RETRIES: u32 = 5;

pub struct Search {
    /// Where it was started: the server's channel, or the DM.
    pub channel: Snowflake,
    pub guild: Option<Snowflake>,
    /// The query as typed.
    pub text: String,
    pub results: Vec<Message>,
    pub total: u64,
    pub loading: bool,
    pub error: Option<String>,
    /// Names in the query that matched nobody (their filter is left out).
    pub unknown: Vec<String>,
    params: Vec<(&'static str, String)>,
    generation: u64,
    query: Query,
    /// Asked the server for members matching names in the query, and waiting for them.
    waiting: Option<std::time::Instant>,
}

impl Search {
    /// Whether this search belongs with the channel being shown (same server, or the same DM).
    pub fn applies_to(&self, channel: Snowflake, guild: Option<Snowflake>) -> bool {
        match self.guild {
            Some(g) => guild == Some(g),
            None => channel == self.channel,
        }
    }

    pub fn has_more(&self) -> bool {
        (self.results.len() as u64) < self.total
    }
}

/// The search parameters for a query, with names resolved; also the names nobody matched.
fn params(app: &App, channel: Snowflake, guild: Option<Snowflake>, q: &Query) -> (Vec<(&'static str, String)>, Vec<String>) {
    let mut params = Vec::new();
    let mut unknown = Vec::new();
    let Some(s) = &app.session else { return (params, unknown) };
    let state = &s.state;
    if !q.content.is_empty() {
        params.push(("content", q.content.clone()));
    }
    let person = |name: &str| -> Option<Snowflake> {
        let c = state.channel(channel)?;
        let people = state.search_people(c, name, 10);
        let lower = name.to_lowercase();
        people
            .iter()
            .find(|u| u.username.to_lowercase() == lower || u.display_name().to_lowercase() == lower)
            .or(people.first())
            .map(|u| u.id)
    };
    for (names, key) in [(&q.from, "author_id"), (&q.mentions, "mentions")] {
        for n in names {
            match person(n) {
                Some(id) => params.push((key, id.to_string())),
                None => unknown.push(format!("@{n}")),
            }
        }
    }
    for n in &q.channels {
        let lower = n.to_lowercase();
        let found = guild.and_then(|g| {
            let all: Vec<_> = state.channel_tree(g).into_iter().flat_map(|(_, cs)| cs).filter(|c| c.kind != ChannelType::Category).collect();
            all.iter().find(|c| c.label().to_lowercase() == lower).or_else(|| all.iter().find(|c| c.label().to_lowercase().contains(&lower))).map(|c| c.id)
        });
        match found {
            Some(id) => params.push(("channel_id", id.to_string())),
            None => unknown.push(format!("#{n}")),
        }
    }
    for h in &q.has {
        params.push(("has", h.clone()));
    }
    let (min, max) = q.id_range();
    if let Some(min) = min {
        params.push(("min_id", min.to_string()));
    }
    if let Some(max) = max {
        params.push(("max_id", max.to_string()));
    }
    (params, unknown)
}

pub fn start(app: &mut App, channel: Snowflake, text: String) {
    let q = Query::parse(&text);
    if q.is_empty() {
        return;
    }
    let guild = app.session.as_ref().and_then(|s| s.state.channel(channel)).and_then(|c| c.guild_id);
    let (params, unknown) = params(app, channel, guild, &q);
    let generation = app.ui.search.as_ref().map_or(1, |s| s.generation + 1);
    app.ui.pins = None;
    app.ui.threads_panel = None;
    app.ui.inbox = None;
    // People we don't have cached yet: ask the server for them first, as Discord's client does.
    let mut waiting = None;
    if let (Some(g), Some(s)) = (guild, &app.session) {
        if unknown.iter().any(|n| n.starts_with('@')) {
            for name in q.from.iter().chain(&q.mentions) {
                s.gateway.search_members(g, name, 10);
            }
            waiting = Some(std::time::Instant::now());
        }
    }
    let loading = waiting.is_some();
    app.ui.search = Some(Search { channel, guild, text, results: Vec::new(), total: 0, loading, error: None, unknown, params, generation, query: q, waiting });
    if !loading {
        fetch(app);
    }
}

/// Members arrived from the server (GUILD_MEMBERS_CHUNK): search once everyone named is known.
pub fn members_arrived(app: &mut App) {
    resolve_waiting(app, false);
}

/// Searches anyway if the server hasn't answered about the people named in a few seconds.
pub fn tick(app: &mut App) {
    resolve_waiting(app, true);
}

fn resolve_waiting(app: &mut App, timeout: bool) {
    let Some(search) = &app.ui.search else { return };
    let Some(since) = search.waiting else { return };
    let timed_out = timeout && since.elapsed() > std::time::Duration::from_secs(3);
    let (channel, guild, q) = (search.channel, search.guild, search.query.clone());
    let (params, unknown) = params(app, channel, guild, &q);
    if unknown.iter().any(|n| n.starts_with('@')) && !timed_out {
        return;
    }
    let Some(search) = &mut app.ui.search else { return };
    search.waiting = None;
    search.params = params;
    search.unknown = unknown;
    fetch(app);
}

pub fn more(app: &mut App) {
    if app.ui.search.as_ref().is_some_and(|s| !s.loading && s.has_more()) {
        fetch(app);
    }
}

fn fetch(app: &mut App) {
    let Some(rest) = app.session.as_ref().map(|s| s.rest.clone()) else { return };
    let Some(search) = &mut app.ui.search else { return };
    search.loading = true;
    search.error = None;
    let offset = search.results.len();
    let (guild, channel, generation) = (search.guild, search.channel, search.generation);
    let mut params = search.params.clone();
    params.push(("offset", offset.to_string()));
    app.spawn(async move {
        let mut tries = 0;
        let result = loop {
            match rest.search_messages(guild, channel, &params).await {
                Ok(SearchResult::Indexing { retry_after }) if tries < MAX_INDEX_RETRIES => {
                    tries += 1;
                    tokio::time::sleep(std::time::Duration::from_secs_f64(retry_after.clamp(1.0, 10.0))).await;
                }
                Ok(SearchResult::Indexing { .. }) => break Err("Discord is still getting this server ready for search. Try again in a minute.".to_owned()),
                other => break other.map_err(|e| e.to_string()),
            }
        };
        AppEvent::SearchResults { generation, result }
    });
}

pub fn on_results(app: &mut App, generation: u64, result: Result<SearchResult, String>) {
    if app.ui.search.as_ref().is_none_or(|s| s.generation != generation) {
        return;
    }
    match result {
        Ok(SearchResult::Page { total, messages, threads }) => {
            if let Some(s) = &mut app.session {
                // Results can come from threads we don't know, and from people we haven't seen.
                for t in threads {
                    if s.state.channel(t.id).is_none() {
                        s.state.add_channel(t);
                    }
                }
                for m in &messages {
                    s.state.cache_author(m);
                }
            }
            let Some(search) = &mut app.ui.search else { return };
            search.loading = false;
            search.total = total;
            search.results.extend(messages);
        }
        Ok(SearchResult::Indexing { .. }) => {}
        Err(e) => {
            let Some(search) = &mut app.ui.search else { return };
            search.loading = false;
            search.error = Some(e);
        }
    }
}
