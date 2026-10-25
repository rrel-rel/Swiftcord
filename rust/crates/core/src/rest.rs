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
    pub async fn ring(&self, channel: Snowflake) -> Result<()> {
        self.empty(Method::POST, &format!("channels/{channel}/call/ring"), Body::Json(json!({ "recipients": null }))).await
    }

    pub async fn typing(&self, channel: Snowflake) -> Result<()> {
        self.empty(Method::POST, &format!("channels/{channel}/typing"), Body::None).await
    }

    /// Marks a channel as read up to a message (user accounts only).
    pub async fn ack(&self, channel: Snowflake, message: Snowflake) -> Result<()> {
        self.empty(Method::POST, &format!("channels/{channel}/messages/{message}/ack"), Body::Json(json!({ "token": null })))
            .await
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
    fn encodes_reaction_keys() {
        assert_eq!(urlencode("👍"), "%F0%9F%91%8D");
        assert_eq!(urlencode("blob:123"), "blob:123");
    }
}
