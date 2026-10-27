//! Discord's HTTP API with client-side per-route rate limiting and 429 retries.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::time::{sleep_until, Instant};

use crate::events::parse_lenient_list;
use crate::models::*;
use crate::Snowflake;

const API_BASE: &str = "https://discord.com/api/v10/";
const MAX_RATE_LIMIT_RETRIES: u32 = 3;
pub const USER_AGENT: &str = concat!("Swiftcord-rs/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("network error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("{message} ({status}, code {code})")]
    Api { status: StatusCode, code: u32, message: String },
    #[error("rate limited")]
    RateLimited,
    #[error("unexpected response: {0}")]
    Json(#[from] serde_json::Error),
}

impl ApiError {
    pub fn status(&self) -> Option<StatusCode> {
        match self {
            ApiError::Api { status, .. } => Some(*status),
            ApiError::Http(e) => e.status(),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, ApiError>;

pub struct FileUpload {
    pub name: String,
    pub bytes: Vec<u8>,
    pub content_type: Option<String>,
}

#[derive(Default, Serialize)]
pub struct NewMessage {
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_reference: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_mentions: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sticker_ids: Vec<Snowflake>,
}

impl NewMessage {
    pub fn reply_to(&mut self, message_id: Snowflake, channel_id: Snowflake, ping: bool) {
        self.message_reference = Some(json!({ "message_id": message_id, "channel_id": channel_id, "fail_if_not_exists": false }));
        self.allowed_mentions = Some(json!({ "parse": ["users", "roles", "everyone"], "replied_user": ping }));
    }
}

#[derive(Default)]
struct Bucket {
    remaining: Option<u32>,
    reset_at: Option<Instant>,
}

#[derive(Default)]
struct RateLimiter {
    buckets: Mutex<HashMap<String, Arc<tokio::sync::Mutex<Bucket>>>>,
    global_reset: Mutex<Option<Instant>>,
}

impl RateLimiter {
    fn bucket(&self, key: &str) -> Arc<tokio::sync::Mutex<Bucket>> {
        self.buckets.lock().unwrap().entry(key.to_owned()).or_default().clone()
    }
}

/// Reduces a path to its rate-limit route: the top-level resource ID (channel, guild, webhook) is kept as a
/// "major parameter"; other IDs and reaction emoji are collapsed.
fn route_key(method: &Method, path: &str) -> String {
    let path = path.split('?').next().unwrap_or("").trim_matches('/');
    let mut out = Vec::new();
    let mut prev: Option<&str> = None;
    for (i, seg) in path.split('/').enumerate() {
        let is_id = seg.len() >= 15 && seg.bytes().all(|b| b.is_ascii_digit());
        let major = i == 1 && matches!(prev, Some("channels" | "guilds" | "webhooks"));
        if prev == Some("reactions") {
            out.push("{emoji}");
        } else if is_id && !major {
            out.push("{id}");
        } else {
            out.push(seg);
        }
        prev = Some(seg);
    }
    format!("{method} {}", out.join("/"))
}

#[derive(Clone)]
pub struct Rest {
    http: reqwest::Client,
    limiter: Arc<RateLimiter>,
}

impl Rest {
    pub fn new(token: &str) -> Rest {
        let mut headers = HeaderMap::new();
        // An empty token makes an anonymous client, e.g. for QR login.
        if let Some(mut v) = Some(token).filter(|t| !t.is_empty()).and_then(|t| HeaderValue::from_str(t).ok()) {
            v.set_sensitive(true);
            headers.insert("Authorization", v);
        }
        headers.insert("X-Discord-Locale", HeaderValue::from_static("en-US"));
        headers.insert("Accept-Language", HeaderValue::from_static("en-US"));
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .default_headers(headers)
            .timeout(Duration::from_secs(120))
            .build()
            .expect("HTTP client");
        Rest { http, limiter: Arc::default() }
    }

    /// A shared client without credentials, for media and unauthenticated endpoints.
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    async fn send(&self, method: Method, path: &str, body: Body<'_>) -> Result<reqwest::Response> {
        let key = route_key(&method, path);
        let bucket = self.limiter.bucket(&key);
        for attempt in 0.. {
            let global = *self.limiter.global_reset.lock().unwrap();
            if let Some(t) = global {
                sleep_until(t).await;
            }
            let mut b = bucket.lock().await;
            if b.remaining == Some(0) {
                if let Some(t) = b.reset_at {
                    sleep_until(t).await;
                }
            }

            let mut req = self.http.request(method.clone(), format!("{API_BASE}{path}"));
            req = match &body {
                Body::None => req,
                Body::Json(v) => req.json(v),
                Body::Multipart(payload, files) => {
                    // Rebuilt per attempt so rate-limit retries don't reuse a consumed body.
                    let mut form = reqwest::multipart::Form::new().part(
                        "payload_json",
                        reqwest::multipart::Part::text(payload.to_string()).mime_str("application/json").unwrap(),
                    );
                    for (i, f) in files.iter().enumerate() {
                        let mut part = reqwest::multipart::Part::bytes(f.bytes.clone()).file_name(f.name.clone());
                        if let Some(ct) = &f.content_type {
                            part = part.mime_str(ct).unwrap_or_else(|_| reqwest::multipart::Part::bytes(f.bytes.clone()).file_name(f.name.clone()));
                        }
                        form = form.part(format!("files[{i}]"), part);
                    }
                    req.multipart(form)
                }
            };
            let resp = req.send().await?;
            let h = resp.headers();
            let header_f64 = |name: &str| h.get(name).and_then(|v| v.to_str().ok()).and_then(|s| s.parse::<f64>().ok());
            b.remaining = header_f64("x-ratelimit-remaining").map(|r| r as u32);
            if let Some(after) = header_f64("x-ratelimit-reset-after") {
                b.reset_at = Some(Instant::now() + Duration::from_secs_f64(after));
            }

            if resp.status() == StatusCode::TOO_MANY_REQUESTS {
                let is_global = h.contains_key("x-ratelimit-global");
                let header_retry = header_f64("retry-after");
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct Limited {
                    retry_after: Option<f64>,
                    global: bool,
                }
                let body: Limited = resp.json().await.unwrap_or_default();
                let wait = Duration::from_secs_f64(body.retry_after.or(header_retry).unwrap_or(1.0));
                let until = Instant::now() + wait;
                if is_global || body.global {
                    *self.limiter.global_reset.lock().unwrap() = Some(until);
                } else {
                    b.remaining = Some(0);
                    b.reset_at = Some(until);
                }
                tracing::warn!("rate limited on {key}; retrying in {wait:?}");
                if attempt >= MAX_RATE_LIMIT_RETRIES {
                    return Err(ApiError::RateLimited);
                }
                continue;
            }
            drop(b);

            if !resp.status().is_success() {
                let status = resp.status();
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct E {
                    code: u32,
                    message: String,
                }
                let text = resp.text().await.unwrap_or_default();
                let e: E = serde_json::from_str(&text).unwrap_or_default();
                let message = if e.message.is_empty() { status.canonical_reason().unwrap_or("error").to_owned() } else { e.message };
                return Err(ApiError::Api { status, code: e.code, message });
            }
            return Ok(resp);
        }
        unreachable!()
    }

    async fn json<T: DeserializeOwned>(&self, method: Method, path: &str, body: Body<'_>) -> Result<T> {
        let text = self.send(method, path, body).await?.text().await?;
        Ok(serde_json::from_str(&text)?)
    }

    async fn empty(&self, method: Method, path: &str, body: Body<'_>) -> Result<()> {
        self.send(method, path, body).await.map(drop)
    }

    // MARK: Users

    pub async fn current_user(&self) -> Result<User> {
        self.json(Method::GET, "users/@me", Body::None).await
    }

    pub async fn create_dm(&self, recipient: Snowflake) -> Result<Channel> {
        self.json(Method::POST, "users/@me/channels", Body::Json(json!({ "recipients": [recipient] }))).await
    }

    /// Invalidates the token server-side, as signing out of the official client does.
    pub async fn logout(&self) -> Result<()> {
        self.empty(Method::POST, "auth/logout", Body::Json(json!({ "provider": null, "voip_provider": null }))).await
    }

    pub async fn settings_proto(&self) -> Result<String> {
        #[derive(Deserialize)]
        struct S {
            settings: String,
        }
        Ok(self.json::<S>(Method::GET, "users/@me/settings-proto/1", Body::None).await?.settings)
    }

    /// Saves our status (online, idle, dnd or invisible) to the account. Discord merges partial settings, so
    /// only the status is sent.
    pub async fn set_status(&self, status: PresenceStatus) -> Result<()> {
        let body = Body::Json(json!({ "settings": crate::proto::status_settings(status.as_str()) }));
        self.json::<serde_json::Value>(Method::PATCH, "users/@me/settings-proto/1", body).await.map(drop)
    }

    /// The frecency settings (favourite GIFs, stickers and emoji), base64-encoded protobuf.
    pub async fn frecency_settings(&self) -> Result<String> {
        #[derive(Deserialize)]
        struct S {
            settings: String,
        }
        Ok(self.json::<S>(Method::GET, "users/@me/settings-proto/2", Body::None).await?.settings)
    }

    /// Replaces the frecency settings; returns what Discord stored.
    pub async fn set_frecency_settings(&self, settings: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct S {
            settings: String,
        }
        let body = Body::Json(json!({ "settings": settings }));
        Ok(self.json::<S>(Method::PATCH, "users/@me/settings-proto/2", body).await?.settings)
    }

    // MARK: Messages

    /// Up to `limit` (1–100) messages, newest first.
    pub async fn messages(&self, channel: Snowflake, before: Option<Snowflake>, limit: u8) -> Result<Vec<Message>> {
        let mut path = format!("channels/{channel}/messages?limit={}", limit.clamp(1, 100));
        if let Some(b) = before {
            path.push_str(&format!("&before={b}"));
        }
        let text = self.send(Method::GET, &path, Body::None).await?.text().await?;
        Ok(parse_lenient_list(&text)?)
    }

    /// Up to `limit` messages newer than `after`.
    pub async fn messages_after(&self, channel: Snowflake, after: Snowflake, limit: u8) -> Result<Vec<Message>> {
        let text = self
            .send(Method::GET, &format!("channels/{channel}/messages?limit={}&after={after}", limit.clamp(1, 100)), Body::None)
            .await?
            .text()
            .await?;
        Ok(parse_lenient_list(&text)?)
    }

    pub async fn messages_around(&self, channel: Snowflake, around: Snowflake) -> Result<Vec<Message>> {
        let text = self
            .send(Method::GET, &format!("channels/{channel}/messages?limit=50&around={around}"), Body::None)
            .await?
            .text()
            .await?;
        Ok(parse_lenient_list(&text)?)
    }

    pub async fn send_message(&self, channel: Snowflake, mut msg: NewMessage, files: Vec<FileUpload>) -> Result<Message> {
        let path = format!("channels/{channel}/messages");
        if files.is_empty() {
            return self.json(Method::POST, &path, Body::Json(serde_json::to_value(&msg)?)).await;
        }
        msg.attachments = Some(files.iter().enumerate().map(|(i, f)| json!({ "id": i, "filename": f.name })).collect());
        let payload = serde_json::to_value(&msg)?;
        self.json(Method::POST, &path, Body::Multipart(payload, &files)).await
    }

    pub async fn edit_message(&self, channel: Snowflake, message: Snowflake, content: &str) -> Result<Message> {
        self.json(Method::PATCH, &format!("channels/{channel}/messages/{message}"), Body::Json(json!({ "content": content })))
            .await
    }

    pub async fn delete_message(&self, channel: Snowflake, message: Snowflake) -> Result<()> {
        self.empty(Method::DELETE, &format!("channels/{channel}/messages/{message}"), Body::None).await
    }

    pub async fn add_reaction(&self, channel: Snowflake, message: Snowflake, emoji: &Emoji) -> Result<()> {
        let key = urlencode(&emoji.reaction_key());
        self.empty(Method::PUT, &format!("channels/{channel}/messages/{message}/reactions/{key}/@me"), Body::None).await
    }

    pub async fn remove_reaction(&self, channel: Snowflake, message: Snowflake, emoji: &Emoji) -> Result<()> {
        let key = urlencode(&emoji.reaction_key());
        self.empty(Method::DELETE, &format!("channels/{channel}/messages/{message}/reactions/{key}/@me"), Body::None).await
    }

    pub async fn pins(&self, channel: Snowflake) -> Result<Vec<Message>> {
        #[derive(Deserialize)]
        struct Item {
            message: Message,
        }
        #[derive(Deserialize)]
        struct Page {
            items: Vec<Item>,
        }
        let page: Page = self.json(Method::GET, &format!("channels/{channel}/messages/pins?limit=50"), Body::None).await?;
        Ok(page.items.into_iter().map(|i| i.message).collect())
    }

    /// Rings everyone in a DM or group DM call.
    // MARK: Threads

    /// A page of a channel's threads (or a forum's posts), most recently active first: active ones, or archived
    /// ones with `archived`. Messages that started them come back too, as forum posts show them.
    pub async fn search_threads(&self, channel: Snowflake, archived: bool, offset: u32) -> Result<ThreadPage> {
        let path = format!("channels/{channel}/threads/search?archived={archived}&sort_by=last_message_time&sort_order=desc&limit=25&offset={offset}");
        self.json(Method::GET, &path, Body::None).await
    }

    /// A channel or thread we haven't been sent (e.g. an archived thread someone linked).
    /// Changes our server profile: `nick` (empty for none) and/or `avatar` (a data URI, or `None` to remove
    /// it; a server avatar needs Nitro). Fields left as `None` at the outer level aren't changed.
    pub async fn update_server_profile(&self, guild: Snowflake, nick: Option<&str>, avatar: Option<Option<&str>>) -> Result<Member> {
        let mut body = serde_json::Map::new();
        if let Some(n) = nick {
            body.insert("nick".into(), json!(n));
        }
        if let Some(a) = avatar {
            body.insert("avatar".into(), json!(a));
        }
        self.json(Method::PATCH, &format!("guilds/{guild}/members/@me"), Body::Json(serde_json::Value::Object(body))).await
    }

    /// Someone's profile for their profile card, with their server profile in `guild`.
    pub async fn user_profile(&self, user: Snowflake, guild: Option<Snowflake>) -> Result<Profile> {
        let mut path = format!("users/{user}/profile?with_mutual_guilds=false&with_mutual_friends=false&with_mutual_friends_count=false");
        if let Some(g) = guild {
            path.push_str(&format!("&guild_id={g}"));
        }
        self.json(Method::GET, &path, Body::None).await
    }

    /// Searches a server's messages (or, without `guild`, one DM's), with Discord's search parameters
    /// (`content`, `author_id`, `channel_id`, `has`, `mentions`, `min_id`, `max_id`, `offset`), newest first.
    /// A server not yet indexed for search answers `Indexing` with how long to wait.
    pub async fn search_messages(&self, guild: Option<Snowflake>, channel: Snowflake, params: &[(&str, String)]) -> Result<SearchResult> {
        let base = match guild {
            Some(g) => format!("guilds/{g}/messages/search"),
            None => format!("channels/{channel}/messages/search"),
        };
        let mut query: Vec<String> = params.iter().map(|(k, v)| format!("{k}={}", percent_encode(v))).collect();
        query.extend(["sort_by=timestamp".into(), "sort_order=desc".into(), "include_nsfw=true".into()]);
        let raw: serde_json::Value = self.json(Method::GET, &format!("{base}?{}", query.join("&")), Body::None).await?;
        if raw.get("messages").is_none() {
            let wait = raw.get("retry_after").and_then(|v| v.as_f64()).unwrap_or(2.0);
            return Ok(SearchResult::Indexing { retry_after: wait });
        }
        #[derive(Deserialize)]
        struct Page {
            #[serde(default)]
            total_results: u64,
            #[serde(default)]
            messages: Vec<Vec<serde_json::Value>>,
            #[serde(default)]
            threads: Vec<Channel>,
        }
        let page: Page = serde_json::from_value(raw)?;
        // Each hit comes as a list holding the message (older APIs added context around it); unreadable ones are
        // skipped rather than failing the page.
        let messages = page.messages.into_iter().filter_map(|hit| hit.into_iter().next()).filter_map(|m| serde_json::from_value(m).ok()).collect();
        Ok(SearchResult::Page { total: page.total_results, messages, threads: page.threads })
    }

    /// The latest preview picture of a Go Live stream, if its streamer has sent one.
    pub async fn stream_preview(&self, stream_key: &str) -> Result<Option<String>> {
        #[derive(Deserialize)]
        struct Preview {
            url: Option<String>,
        }
        match self.json::<Preview>(Method::GET, &format!("streams/{stream_key}/preview"), Body::None).await {
            Ok(p) => Ok(p.url),
            Err(e) if e.status() == Some(StatusCode::NOT_FOUND) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Sends a preview picture of our own stream (a JPEG), for people looking at it in the channel list.
    pub async fn upload_stream_preview(&self, stream_key: &str, jpeg: &[u8]) -> Result<()> {
        use base64::Engine;
        let thumbnail = format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(jpeg));
        self.empty(Method::POST, &format!("streams/{stream_key}/preview"), Body::Json(json!({ "thumbnail": thumbnail }))).await
    }

    /// Discord's standard sticker packs, which anyone can send.
    pub async fn sticker_packs(&self) -> Result<Vec<StickerPack>> {
        #[derive(Deserialize)]
        struct Packs {
            sticker_packs: Vec<StickerPack>,
        }
        let packs: Packs = self.json(Method::GET, "sticker-packs", Body::None).await?;
        Ok(packs.sticker_packs)
    }

    pub async fn channel(&self, id: Snowflake) -> Result<Channel> {
        self.json(Method::GET, &format!("channels/{id}"), Body::None).await
    }

    pub async fn join_thread(&self, thread: Snowflake) -> Result<()> {
        self.empty(Method::PUT, &format!("channels/{thread}/thread-members/@me"), Body::None).await
    }

    pub async fn leave_thread(&self, thread: Snowflake) -> Result<()> {
        self.empty(Method::DELETE, &format!("channels/{thread}/thread-members/@me"), Body::None).await
    }

    /// Starts a thread from a message (the thread takes the message's ID).
    pub async fn create_thread_from_message(&self, channel: Snowflake, message: Snowflake, name: &str) -> Result<Channel> {
        let body = Body::Json(json!({ "name": name, "auto_archive_duration": 1440 }));
        self.json(Method::POST, &format!("channels/{channel}/messages/{message}/threads"), body).await
    }

    /// Starts a public thread in a text channel, or (with `content`) a post in a forum.
    pub async fn create_thread(&self, channel: Snowflake, name: &str, content: Option<&str>) -> Result<Channel> {
        let body = match content {
            Some(text) => json!({ "name": name, "auto_archive_duration": 1440, "message": { "content": text } }),
            None => json!({ "name": name, "auto_archive_duration": 1440, "type": 11 }),
        };
        self.json(Method::POST, &format!("channels/{channel}/threads"), Body::Json(body)).await
    }

    pub async fn ring(&self, channel: Snowflake) -> Result<()> {
        self.empty(Method::POST, &format!("channels/{channel}/call/ring"), Body::Json(json!({ "recipients": null }))).await
    }

    /// Declines a call: stops it ringing for these users (yourself, usually).
    pub async fn stop_ringing(&self, channel: Snowflake, users: &[Snowflake]) -> Result<()> {
        self.empty(Method::POST, &format!("channels/{channel}/call/stop-ringing"), Body::Json(json!({ "recipients": users }))).await
    }

    pub async fn typing(&self, channel: Snowflake) -> Result<()> {
        self.empty(Method::POST, &format!("channels/{channel}/typing"), Body::None).await
    }

    /// Marks a channel as read up to a message (user accounts only).
    pub async fn ack(&self, channel: Snowflake, message: Snowflake) -> Result<()> {
        self.empty(Method::POST, &format!("channels/{channel}/messages/{message}/ack"), Body::Json(json!({ "token": null })))
            .await
    }

    // MARK: GIFs (Tenor, through Discord)

    pub async fn gif_search(&self, query: &str) -> Result<Vec<Gif>> {
        let q = urlencode(query);
        let text = self
            .send(Method::GET, &format!("gifs/search?q={q}&media_format=mp4&provider=tenor&locale=en-US&limit=50"), Body::None)
            .await?
            .text()
            .await?;
        Ok(parse_lenient_list(&text)?)
    }

    pub async fn gif_trending(&self) -> Result<Vec<Gif>> {
        let text = self
            .send(Method::GET, "gifs/trending-gifs?media_format=mp4&provider=tenor&locale=en-US&limit=50", Body::None)
            .await?
            .text()
            .await?;
        Ok(parse_lenient_list(&text)?)
    }

    /// Trending search categories (names are what to search for).
    pub async fn gif_categories(&self) -> Result<Vec<String>> {
        #[derive(Deserialize)]
        struct Category {
            name: String,
        }
        #[derive(Deserialize)]
        struct Trending {
            #[serde(default)]
            categories: Vec<Category>,
        }
        let t: Trending = self.json(Method::GET, "gifs/trending?media_format=mp4&provider=tenor&locale=en-US", Body::None).await?;
        Ok(t.categories.into_iter().map(|c| c.name).collect())
    }

    // MARK: Remote auth (QR login)

    /// Exchanges a remote-auth ticket for the encrypted token. Called on a client without a token.
    pub async fn remote_auth_login(&self, ticket: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct R {
            encrypted_token: String,
        }
        let r: R = self.json(Method::POST, "users/@me/remote-auth/login", Body::Json(json!({ "ticket": ticket }))).await?;
        Ok(r.encrypted_token)
    }
}

/// A page of message search results.
#[derive(Debug, Clone)]
pub enum SearchResult {
    Page { total: u64, messages: Vec<Message>, threads: Vec<Channel> },
    /// Discord is still indexing the server for search; try again after this many seconds.
    Indexing { retry_after: f64 },
}

/// Percent-encodes a query string value.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// A page of thread search results.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ThreadPage {
    #[serde(deserialize_with = "crate::events::lenient_vec")]
    pub threads: Vec<Channel>,
    /// Our memberships among them.
    #[serde(deserialize_with = "crate::events::lenient_vec")]
    pub members: Vec<ThreadMember>,
    /// The messages that started them (forum posts' opening messages).
    #[serde(deserialize_with = "crate::events::lenient_vec")]
    pub first_messages: Vec<Message>,
    pub has_more: bool,
}

/// A GIF from Discord's Tenor search. Sending one means sending its `url`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Gif {
    pub url: String,
    /// The video (MP4), when requested with media_format=mp4.
    pub src: String,
    pub title: String,
    /// The animated GIF.
    pub gif_src: String,
    /// A still frame.
    pub preview: String,
    pub width: u32,
    pub height: u32,
}

enum Body<'a> {
    None,
    Json(serde_json::Value),
    Multipart(serde_json::Value, &'a [FileUpload]),
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~:".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_keys_keep_major_ids_only() {
        assert_eq!(
            route_key(&Method::GET, "channels/123456789012345678/messages/223456789012345678?limit=50"),
            "GET channels/123456789012345678/messages/{id}"
        );
        assert_eq!(
            route_key(&Method::PUT, "channels/123456789012345678/messages/223456789012345678/reactions/%F0%9F%91%8D/@me"),
            "PUT channels/123456789012345678/messages/{id}/reactions/{emoji}/@me"
        );
        assert_eq!(route_key(&Method::GET, "users/123456789012345678/profile"), "GET users/{id}/profile");
    }

    #[test]
    fn percent_encodes_search_text() {
        assert_eq!(percent_encode("hi there & ok"), "hi%20there%20%26%20ok");
        assert_eq!(percent_encode("日本"), "%E6%97%A5%E6%9C%AC");
    }

    #[test]
    fn sends_sticker_ids_only_when_there_are_some() {
        let plain = serde_json::to_value(NewMessage { content: "hi".into(), ..Default::default() }).unwrap();
        assert!(plain.get("sticker_ids").is_none());
        let sticker = NewMessage { sticker_ids: vec![Snowflake(796140620111544330)], ..Default::default() };
        assert_eq!(serde_json::to_value(sticker).unwrap()["sticker_ids"], serde_json::json!(["796140620111544330"]));
        let packs: Vec<StickerPack> = serde_json::from_str(r#"[{"id":"1","name":"Wumpus","stickers":[{"id":"2","name":"Hi","tags":"wave","format_type":3,"pack_id":"1"}]}]"#).unwrap();
        assert!(packs[0].stickers[0].available, "stickers are available unless they say otherwise");
    }

    #[test]
    fn encodes_reaction_keys() {
        assert_eq!(urlencode("👍"), "%F0%9F%91%8D");
        assert_eq!(urlencode("blob:123"), "blob:123");
    }
}
