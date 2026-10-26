//! Discord API models. Every field defaults so that a missing or newly-optional field never makes a
//! whole payload unreadable; unknown fields are ignored.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};

use crate::Snowflake;

fn null_as_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// Permission bitsets arrive as decimal strings.
fn permission_string<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum S {
        Str(String),
        Num(u64),
    }
    Ok(match Option::<S>::deserialize(d)? {
        Some(S::Str(s)) => s.parse().unwrap_or(0),
        Some(S::Num(n)) => n,
        None => 0,
    })
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct User {
    pub id: Snowflake,
    pub username: String,
    pub global_name: Option<String>,
    pub discriminator: Option<String>,
    pub avatar: Option<String>,
    #[serde(deserialize_with = "null_as_default")]
    pub bot: bool,
    pub public_flags: Option<u64>,
    /// Nitro (only sent for yourself): 0 none, 1 Classic, 2 Nitro, 3 Basic.
    pub premium_type: Option<u8>,
}

impl User {
    pub fn display_name(&self) -> &str {
        match &self.global_name {
            Some(n) if !n.is_empty() => n,
            _ => &self.username,
        }
    }

    /// Username with the legacy discriminator, if the account still has one.
    pub fn tag(&self) -> String {
        match self.discriminator.as_deref() {
            Some(d) if d != "0" && !d.is_empty() => format!("{}#{}", self.username, d),
            _ => self.username.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Member {
    pub user: Option<User>,
    /// Present instead of `user` in READY's merged member lists.
    pub user_id: Option<Snowflake>,
    pub nick: Option<String>,
    pub avatar: Option<String>,
    #[serde(deserialize_with = "null_as_default")]
    pub roles: Vec<Snowflake>,
    pub joined_at: Option<DateTime<Utc>>,
    pub communication_disabled_until: Option<DateTime<Utc>>,
}

impl Member {
    pub fn user_id(&self) -> Option<Snowflake> {
        self.user.as_ref().map(|u| u.id).or(self.user_id)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Role {
    pub id: Snowflake,
    pub name: String,
    pub color: u32,
    pub position: i32,
    #[serde(deserialize_with = "permission_string")]
    pub permissions: u64,
    pub hoist: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Emoji {
    pub id: Option<Snowflake>,
    pub name: Option<String>,
    #[serde(deserialize_with = "null_as_default")]
    pub animated: bool,
}

impl Emoji {
    pub fn same_as(&self, other: &Emoji) -> bool {
        match (self.id, other.id) {
            (Some(a), Some(b)) => a == b,
            (None, None) => self.name == other.name,
            _ => false,
        }
    }

    /// The path segment used by the reactions endpoints: `name:id` for custom emoji, the character otherwise.
    pub fn reaction_key(&self) -> String {
        match self.id {
            Some(id) => format!("{}:{}", self.name.as_deref().unwrap_or("_"), id),
            None => self.name.clone().unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct GuildProperties {
    pub name: Option<String>,
    pub icon: Option<String>,
    pub owner_id: Option<Snowflake>,
    pub default_message_notifications: Option<u8>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Guild {
    pub id: Snowflake,
    pub name: String,
    pub icon: Option<String>,
    pub owner_id: Snowflake,
    #[serde(deserialize_with = "null_as_default")]
    pub roles: Vec<Role>,
    #[serde(deserialize_with = "null_as_default")]
    pub emojis: Vec<Emoji>,
    #[serde(deserialize_with = "null_as_default")]
    pub stickers: Vec<Sticker>,
    #[serde(deserialize_with = "null_as_default")]
    pub channels: Vec<Channel>,
    #[serde(deserialize_with = "null_as_default")]
    pub threads: Vec<Channel>,
    #[serde(deserialize_with = "null_as_default")]
    pub members: Vec<Member>,
    #[serde(deserialize_with = "null_as_default")]
    pub voice_states: Vec<VoiceState>,
    pub joined_at: Option<DateTime<Utc>>,
    #[serde(deserialize_with = "null_as_default")]
    pub unavailable: bool,
    pub default_message_notifications: u8,
    /// Newer gateway capabilities nest the guild's own fields here.
    pub properties: Option<GuildProperties>,
}

impl Guild {
    /// Lifts fields out of `properties` when the gateway used the nested format.
    pub fn normalize(&mut self) {
        if let Some(p) = self.properties.take() {
            if let Some(n) = p.name {
                self.name = n;
            }
            if p.icon.is_some() {
                self.icon = p.icon;
            }
            if let Some(o) = p.owner_id {
                self.owner_id = o;
            }
            if let Some(d) = p.default_message_notifications {
                self.default_message_notifications = d;
            }
        }
        for c in self.channels.iter_mut().chain(self.threads.iter_mut()) {
            c.guild_id = Some(self.id);
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "u8", into = "u8")]
pub enum ChannelType {
    #[default]
    Text,
    Dm,
    Voice,
    GroupDm,
    Category,
    Announcement,
    AnnouncementThread,
    PublicThread,
    PrivateThread,
    Stage,
    Directory,
    Forum,
    Media,
    Unknown(u8),
}

impl From<u8> for ChannelType {
    fn from(v: u8) -> Self {
        use ChannelType::*;
        match v {
            0 => Text,
            1 => Dm,
            2 => Voice,
            3 => GroupDm,
            4 => Category,
            5 => Announcement,
            10 => AnnouncementThread,
            11 => PublicThread,
            12 => PrivateThread,
            13 => Stage,
            14 => Directory,
            15 => Forum,
            16 => Media,
            n => Unknown(n),
        }
    }
}

impl From<ChannelType> for u8 {
    fn from(t: ChannelType) -> u8 {
        use ChannelType::*;
        match t {
            Text => 0,
            Dm => 1,
            Voice => 2,
            GroupDm => 3,
            Category => 4,
            Announcement => 5,
            AnnouncementThread => 10,
            PublicThread => 11,
            PrivateThread => 12,
            Stage => 13,
            Directory => 14,
            Forum => 15,
            Media => 16,
            Unknown(n) => n,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Overwrite {
    pub id: Snowflake,
    /// 0 = role, 1 = member.
    #[serde(rename = "type")]
    pub kind: u8,
    #[serde(deserialize_with = "permission_string")]
    pub allow: u64,
    #[serde(deserialize_with = "permission_string")]
    pub deny: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Channel {
    pub id: Snowflake,
    #[serde(rename = "type")]
    pub kind: ChannelType,
    pub guild_id: Option<Snowflake>,
    pub name: Option<String>,
    pub topic: Option<String>,
    pub position: i32,
    pub parent_id: Option<Snowflake>,
    pub last_message_id: Option<Snowflake>,
    #[serde(deserialize_with = "null_as_default")]
    pub permission_overwrites: Vec<Overwrite>,
    #[serde(deserialize_with = "null_as_default")]
    pub recipients: Vec<User>,
    /// Present instead of `recipients` in deduplicated READY payloads.
    pub recipient_ids: Option<Vec<Snowflake>>,
    pub icon: Option<String>,
    #[serde(deserialize_with = "null_as_default")]
    pub nsfw: bool,
    pub bitrate: Option<u32>,
    pub user_limit: Option<u32>,
    pub rate_limit_per_user: Option<u32>,
    /// Threads: archived and locked state.
    pub thread_metadata: Option<ThreadMetadata>,
    /// Threads: our membership, present once we've joined.
    pub member: Option<ThreadMember>,
    /// Threads: who started it.
    pub owner_id: Option<Snowflake>,
    /// Threads: messages sent in it (not counting the starter message).
    pub message_count: Option<u32>,
    pub member_count: Option<u32>,
    /// Forum channels: the tags posts can carry.
    #[serde(deserialize_with = "null_as_default")]
    pub available_tags: Vec<ForumTag>,
    /// Forum posts: the tags it carries.
    #[serde(deserialize_with = "null_as_default")]
    pub applied_tags: Vec<Snowflake>,
    /// Forum posts: 2 = pinned to the top of the forum.
    #[serde(deserialize_with = "null_as_default")]
    pub flags: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ThreadMetadata {
    #[serde(deserialize_with = "null_as_default")]
    pub archived: bool,
    #[serde(deserialize_with = "null_as_default")]
    pub locked: bool,
    pub archive_timestamp: Option<DateTime<Utc>>,
    pub auto_archive_duration: Option<u32>,
    pub create_timestamp: Option<DateTime<Utc>>,
}

/// Someone's membership of a thread; `id` is the thread and `user_id` the member (both omitted in some payloads).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ThreadMember {
    pub id: Option<Snowflake>,
    pub user_id: Option<Snowflake>,
    pub join_timestamp: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ForumTag {
    pub id: Snowflake,
    pub name: String,
    pub emoji_id: Option<Snowflake>,
    pub emoji_name: Option<String>,
}

impl Channel {
    pub fn is_archived(&self) -> bool {
        self.thread_metadata.as_ref().is_some_and(|m| m.archived)
    }

    pub fn is_forum(&self) -> bool {
        matches!(self.kind, ChannelType::Forum | ChannelType::Media)
    }

    /// Channels that can have threads started in them.
    pub fn has_threads(&self) -> bool {
        matches!(self.kind, ChannelType::Text | ChannelType::Announcement) || self.is_forum()
    }

    pub fn is_private(&self) -> bool {
        matches!(self.kind, ChannelType::Dm | ChannelType::GroupDm)
    }

    pub fn is_thread(&self) -> bool {
        matches!(
            self.kind,
            ChannelType::AnnouncementThread | ChannelType::PublicThread | ChannelType::PrivateThread
        )
    }

    pub fn is_voice(&self) -> bool {
        matches!(self.kind, ChannelType::Voice | ChannelType::Stage)
    }

    /// Channels whose messages can be read in the chat view.
    pub fn is_text_based(&self) -> bool {
        matches!(
            self.kind,
            ChannelType::Text
                | ChannelType::Dm
                | ChannelType::GroupDm
                | ChannelType::Announcement
                | ChannelType::AnnouncementThread
                | ChannelType::PublicThread
                | ChannelType::PrivateThread
                | ChannelType::Voice
        )
    }

    /// The name shown in the UI; DMs are named after their recipients.
    pub fn label(&self) -> String {
        if let Some(n) = self.name.as_deref().filter(|n| !n.is_empty()) {
            return n.to_owned();
        }
        if self.is_private() {
            let names: Vec<&str> = self.recipients.iter().map(|u| u.display_name()).collect();
            if !names.is_empty() {
                return names.join(", ");
            }
        }
        "unnamed".to_owned()
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct VoiceState {
    pub guild_id: Option<Snowflake>,
    pub channel_id: Option<Snowflake>,
    pub user_id: Snowflake,
    pub member: Option<Member>,
    pub session_id: String,
    pub deaf: bool,
    pub mute: bool,
    pub self_deaf: bool,
    pub self_mute: bool,
    #[serde(deserialize_with = "null_as_default")]
    pub self_stream: bool,
    pub self_video: bool,
    pub suppress: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Attachment {
    pub id: Snowflake,
    pub filename: String,
    pub size: u64,
    pub url: String,
    pub proxy_url: String,
    pub content_type: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub description: Option<String>,
}

impl Attachment {
    pub fn is_image(&self) -> bool {
        self.width.is_some() && self.content_type.as_deref().is_none_or(|t| t.starts_with("image/"))
    }

    pub fn is_video(&self) -> bool {
        self.content_type.as_deref().is_some_and(|t| t.starts_with("video/"))
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct EmbedMedia {
    pub url: String,
    pub proxy_url: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct EmbedAuthor {
    pub name: String,
    pub url: Option<String>,
    pub icon_url: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct EmbedFooter {
    pub text: String,
    pub icon_url: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct EmbedField {
    pub name: String,
    pub value: String,
    #[serde(deserialize_with = "null_as_default")]
    pub inline: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Embed {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub color: Option<u32>,
    pub timestamp: Option<DateTime<Utc>>,
    pub author: Option<EmbedAuthor>,
    pub footer: Option<EmbedFooter>,
    pub image: Option<EmbedMedia>,
    pub thumbnail: Option<EmbedMedia>,
    pub video: Option<EmbedMedia>,
    pub provider: Option<EmbedAuthor>,
    #[serde(deserialize_with = "null_as_default")]
    pub fields: Vec<EmbedField>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Reaction {
    pub emoji: Emoji,
    pub count: u32,
    pub me: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MessageReference {
    /// 0 = reply, 1 = forward.
    #[serde(rename = "type")]
    pub kind: u8,
    pub message_id: Option<Snowflake>,
    pub channel_id: Option<Snowflake>,
    pub guild_id: Option<Snowflake>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct StickerItem {
    pub id: Snowflake,
    pub name: String,
    /// 1 = PNG, 2 = APNG, 3 = Lottie, 4 = GIF.
    pub format_type: u8,
}

/// A sticker that can be sent: a server's own, or one of Discord's standard packs.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Sticker {
    pub id: Snowflake,
    pub name: String,
    pub description: Option<String>,
    /// Search words (an emoji, for server stickers).
    #[serde(deserialize_with = "null_as_default")]
    pub tags: String,
    pub format_type: u8,
    pub guild_id: Option<Snowflake>,
    pub pack_id: Option<Snowflake>,
    /// False when a server lost the boosts it needs for it.
    #[serde(default = "yes")]
    pub available: bool,
    pub sort_value: Option<i64>,
}

fn yes() -> bool {
    true
}

impl Sticker {
    pub fn item(&self) -> StickerItem {
        StickerItem { id: self.id, name: self.name.clone(), format_type: self.format_type }
    }
}

/// One of Discord's standard sticker packs.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct StickerPack {
    pub id: Snowflake,
    pub name: String,
    #[serde(deserialize_with = "null_as_default")]
    pub stickers: Vec<Sticker>,
    pub cover_sticker_id: Option<Snowflake>,
}

/// The copied content of a forwarded message.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SnapshotMessage {
    pub content: String,
    #[serde(deserialize_with = "null_as_default")]
    pub attachments: Vec<Attachment>,
    #[serde(deserialize_with = "null_as_default")]
    pub embeds: Vec<Embed>,
    pub timestamp: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MessageSnapshot {
    pub message: SnapshotMessage,
}

pub mod message_type {
    pub const DEFAULT: u8 = 0;
    pub const RECIPIENT_ADD: u8 = 1;
    pub const RECIPIENT_REMOVE: u8 = 2;
    pub const CALL: u8 = 3;
    pub const CHANNEL_NAME_CHANGE: u8 = 4;
    pub const CHANNEL_PINNED_MESSAGE: u8 = 6;
    pub const USER_JOIN: u8 = 7;
    pub const GUILD_BOOST: u8 = 8;
    pub const THREAD_CREATED: u8 = 18;
    pub const REPLY: u8 = 19;
    pub const CHAT_INPUT_COMMAND: u8 = 20;
    /// First message of a thread started from a message: shows that message (`referenced_message`).
    pub const THREAD_STARTER_MESSAGE: u8 = 21;
    pub const CONTEXT_MENU_COMMAND: u8 = 23;
}

pub mod message_flags {
    pub const SUPPRESS_EMBEDS: u64 = 1 << 2;
    pub const SUPPRESS_NOTIFICATIONS: u64 = 1 << 12;
    /// The message is laid out entirely by `components`; `content` and `embeds` are unused.
    pub const IS_COMPONENTS_V2: u64 = 1 << 15;
}

pub mod component_type {
    pub const ACTION_ROW: u8 = 1;
    pub const BUTTON: u8 = 2;
    pub const SECTION: u8 = 9;
    pub const TEXT_DISPLAY: u8 = 10;
    pub const THUMBNAIL: u8 = 11;
    pub const MEDIA_GALLERY: u8 = 12;
    pub const FILE: u8 = 13;
    pub const SEPARATOR: u8 = 14;
    pub const CONTAINER: u8 = 17;

    /// String, user, role, mentionable and channel select menus.
    pub fn is_select(kind: u8) -> bool {
        matches!(kind, 3 | 5..=8)
    }
}

/// A message component: buttons and menus in action rows, and (Components V2) the layout of a whole message:
/// containers, sections, text, thumbnails, galleries, files and separators. One struct holds the fields of
/// every kind; unknown kinds are skipped when drawn.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Component {
    #[serde(rename = "type")]
    pub kind: u8,
    /// Children of containers, sections and action rows.
    #[serde(deserialize_with = "null_as_default")]
    pub components: Vec<Component>,
    /// A section's thumbnail or button.
    pub accessory: Option<Box<Component>>,
    /// Text display markdown.
    pub content: Option<String>,
    /// A thumbnail's picture.
    pub media: Option<UnfurledMedia>,
    /// A media gallery's pictures and videos.
    #[serde(deserialize_with = "null_as_default")]
    pub items: Vec<GalleryItem>,
    /// A file component's attachment (`attachment://name`, or the resolved CDN URL).
    pub file: Option<UnfurledMedia>,
    pub accent_color: Option<u32>,
    #[serde(deserialize_with = "null_as_default")]
    pub spoiler: bool,
    /// Separators: whether a line is drawn (default true) and the gap, 1 small or 2 large.
    pub divider: Option<bool>,
    pub spacing: Option<u8>,
    /// Buttons: 1 primary, 2 secondary, 3 success, 4 danger, 5 link, 6 premium.
    pub style: Option<u8>,
    pub label: Option<String>,
    pub url: Option<String>,
    pub emoji: Option<Emoji>,
    #[serde(deserialize_with = "null_as_default")]
    pub disabled: bool,
    /// Select menus' prompt.
    pub placeholder: Option<String>,
    /// Alt text of thumbnails.
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct UnfurledMedia {
    pub url: String,
    pub proxy_url: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub content_type: Option<String>,
}

impl UnfurledMedia {
    pub fn is_video(&self) -> bool {
        self.content_type.as_deref().is_some_and(|t| t.starts_with("video/"))
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct GalleryItem {
    pub media: UnfurledMedia,
    pub description: Option<String>,
    #[serde(deserialize_with = "null_as_default")]
    pub spoiler: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Message {
    pub id: Snowflake,
    pub channel_id: Snowflake,
    pub guild_id: Option<Snowflake>,
    pub author: User,
    pub member: Option<Member>,
    #[serde(deserialize_with = "null_as_default")]
    pub content: String,
    pub timestamp: Option<DateTime<Utc>>,
    pub edited_timestamp: Option<DateTime<Utc>>,
    #[serde(rename = "type")]
    pub kind: u8,
    #[serde(deserialize_with = "null_as_default")]
    pub mentions: Vec<User>,
    #[serde(deserialize_with = "null_as_default")]
    pub mention_roles: Vec<Snowflake>,
    #[serde(deserialize_with = "null_as_default")]
    pub mention_everyone: bool,
    #[serde(deserialize_with = "null_as_default")]
    pub attachments: Vec<Attachment>,
    #[serde(deserialize_with = "null_as_default")]
    pub embeds: Vec<Embed>,
    #[serde(deserialize_with = "null_as_default")]
    pub reactions: Vec<Reaction>,
    #[serde(deserialize_with = "null_as_default")]
    pub sticker_items: Vec<StickerItem>,
    #[serde(deserialize_with = "null_as_default")]
    pub components: Vec<Component>,
    /// The thread started from this message.
    pub thread: Option<Box<Channel>>,
    pub message_reference: Option<MessageReference>,
    pub referenced_message: Option<Box<Message>>,
    #[serde(deserialize_with = "null_as_default")]
    pub message_snapshots: Vec<MessageSnapshot>,
    pub webhook_id: Option<Snowflake>,
    #[serde(deserialize_with = "null_as_default")]
    pub flags: u64,
    #[serde(deserialize_with = "null_as_default")]
    pub pinned: bool,
    /// Client-side only: set on a message that's being sent and hasn't been confirmed yet.
    #[serde(skip)]
    pub pending: Option<PendingState>,
    /// Nonce of a message we sent, to match the gateway echo against the optimistic copy.
    pub nonce: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingState {
    Sending,
    Failed,
}

impl Message {
    pub fn timestamp(&self) -> DateTime<Utc> {
        self.timestamp.unwrap_or_else(|| self.id.created_at())
    }

    pub fn is_reply(&self) -> bool {
        self.kind == message_type::REPLY
    }

    pub fn is_forward(&self) -> bool {
        self.message_reference.as_ref().is_some_and(|r| r.kind == 1) && !self.message_snapshots.is_empty()
    }

    /// Ordinary user messages, which get an author header; everything else is a system line.
    pub fn is_regular(&self) -> bool {
        matches!(
            self.kind,
            message_type::DEFAULT
                | message_type::REPLY
                | message_type::CHAT_INPUT_COMMAND
                | message_type::CONTEXT_MENU_COMMAND
        )
    }

    pub fn nonce_str(&self) -> Option<String> {
        match &self.nonce {
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            Some(serde_json::Value::Number(n)) => Some(n.to_string()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ReadStateEntry {
    pub id: Snowflake,
    pub last_message_id: Option<Snowflake>,
    pub mention_count: u32,
    /// Non-channel read states (e.g. guild events) use a non-zero type.
    pub read_state_type: u8,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MuteConfig {
    pub end_time: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ChannelOverride {
    pub channel_id: Snowflake,
    pub muted: bool,
    pub mute_config: Option<MuteConfig>,
    /// 0 all, 1 mentions, 2 nothing, 3 inherit.
    pub message_notifications: u8,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct UserGuildSettings {
    pub guild_id: Option<Snowflake>,
    pub muted: bool,
    pub mute_config: Option<MuteConfig>,
    pub message_notifications: u8,
    pub suppress_everyone: bool,
    pub suppress_roles: bool,
    #[serde(deserialize_with = "null_as_default")]
    pub channel_overrides: Vec<ChannelOverride>,
}

fn muted_now(muted: bool, config: &Option<MuteConfig>) -> bool {
    muted && config.as_ref().and_then(|c| c.end_time).is_none_or(|end| end > Utc::now())
}

impl UserGuildSettings {
    pub fn is_muted(&self) -> bool {
        muted_now(self.muted, &self.mute_config)
    }
}

impl ChannelOverride {
    pub fn is_muted(&self) -> bool {
        muted_now(self.muted, &self.mute_config)
    }
}

/// A server folder from the user's settings. Servers outside a real folder are folders of one with no ID.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct GuildFolder {
    pub id: Option<i64>,
    pub name: Option<String>,
    pub color: Option<u32>,
    pub guild_ids: Vec<Snowflake>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PresenceStatus {
    Online,
    Idle,
    Dnd,
    #[default]
    Offline,
}

impl PresenceStatus {
    pub fn parse(s: &str) -> Self {
        match s {
            "online" => Self::Online,
            "idle" => Self::Idle,
            "dnd" => Self::Dnd,
            _ => Self::Offline,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Idle => "idle",
            Self::Dnd => "dnd",
            Self::Offline => "invisible",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_components_v2_messages() {
        // Trimmed from a real embed bot post: a container holding a section with a thumbnail, a gallery and text.
        let json = r#"{"id":"1","channel_id":"2","author":{"id":"3","username":"Embedded","bot":true},"content":"",
            "flags":36868,"embeds":[],
            "components":[{"type":17,"id":2,"accent_color":null,"spoiler":false,"components":[
              {"type":9,"id":3,"components":[{"type":10,"id":4,"content":"**ENCE**"},{"type":10,"id":1,"content":"End of an era"}],
               "accessory":{"type":11,"id":5,"media":{"url":"https://pbs.twimg.com/a.png","width":200,"height":200,"content_type":"image/png","flags":0},
                            "description":null,"spoiler":false}},
              {"type":12,"id":7,"items":[{"media":{"url":"https://pbs.twimg.com/b.jpg","width":2000,"height":2500},"description":null,"spoiler":false}]},
              {"type":14,"divider":false,"spacing":2},
              {"type":1,"components":[{"type":2,"style":5,"label":"Open","url":"https://x.com"},{"type":3,"custom_id":"m","options":[],"placeholder":"Pick"}]},
              {"type":10,"id":6,"content":"-# <t:1793023201:R> • [Link](https://x.com/ence)"}]}]}"#;
        let m: Message = serde_json::from_str(json).unwrap();
        assert!(m.flags & message_flags::IS_COMPONENTS_V2 != 0);
        let container = &m.components[0];
        assert_eq!(container.kind, component_type::CONTAINER);
        let section = &container.components[0];
        assert_eq!(section.components[1].content.as_deref(), Some("End of an era"));
        assert_eq!(section.accessory.as_ref().unwrap().media.as_ref().unwrap().width, Some(200));
        assert_eq!(container.components[1].items[0].media.height, Some(2500));
        assert_eq!(container.components[2].divider, Some(false));
        let row = &container.components[3];
        assert_eq!(row.components[0].url.as_deref(), Some("https://x.com"));
        assert!(component_type::is_select(row.components[1].kind));
    }

    #[test]
    fn tolerates_nulls_and_missing_fields() {
        let m: Message = serde_json::from_str(
            r#"{"id":"1","channel_id":"2","author":{"id":"3","username":"a"},"content":null,
                "mentions":null,"attachments":[{"id":"4","filename":"x.png","width":10,"height":5,"url":"u"}],
                "timestamp":"2024-01-01T00:00:00.000000+00:00","type":19}"#,
        )
        .unwrap();
        assert!(m.content.is_empty());
        assert!(m.is_reply());
        assert!(m.attachments[0].is_image());
    }

    #[test]
    fn reads_permission_strings() {
        let r: Role = serde_json::from_str(r#"{"id":"1","permissions":"2048"}"#).unwrap();
        assert_eq!(r.permissions, 2048);
    }

    #[test]
    fn unknown_channel_types_survive() {
        let c: Channel = serde_json::from_str(r#"{"id":"1","type":99}"#).unwrap();
        assert_eq!(c.kind, ChannelType::Unknown(99));
    }

    #[test]
    fn lifts_nested_guild_properties() {
        let mut g: Guild = serde_json::from_str(
            r#"{"id":"1","properties":{"name":"Test","owner_id":"5"},"channels":[{"id":"9","type":0}]}"#,
        )
        .unwrap();
        g.normalize();
        assert_eq!(g.name, "Test");
        assert_eq!(g.owner_id, Snowflake(5));
        assert_eq!(g.channels[0].guild_id, Some(Snowflake(1)));
    }
}
