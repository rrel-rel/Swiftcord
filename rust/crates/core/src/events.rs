//! Gateway dispatch events, parsed off the UI thread.

use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;

use crate::models::*;
use crate::Snowflake;

/// Reads an array element by element, skipping (and logging) entries that don't parse, so one unexpected
/// shape can't make a whole payload unreadable.
pub(crate) fn lenient_vec<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let raw: Option<Vec<Box<RawValue>>> = Deserialize::deserialize(d)?;
    Ok(raw
        .unwrap_or_default()
        .into_iter()
        .filter_map(|r| match serde_json::from_str::<T>(r.get()) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!("skipping unreadable {}: {e}", std::any::type_name::<T>());
                None
            }
        })
        .collect())
}

pub fn parse_lenient_list<T: DeserializeOwned>(json: &str) -> Result<Vec<T>, serde_json::Error> {
    #[derive(Deserialize)]
    #[serde(transparent)]
    struct W<T: DeserializeOwned>(#[serde(deserialize_with = "lenient_vec")] Vec<T>);
    serde_json::from_str::<W<T>>(json).map(|w| w.0)
}

/// Read states and guild settings arrive as a bare array, or as `{ version, partial, entries }` with newer
/// capabilities.
fn entries<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    // Untagged enums can't buffer RawValue, so branch on a Value instead; these lists are small.
    let list = match Option::<serde_json::Value>::deserialize(d)? {
        Some(serde_json::Value::Array(l)) => l,
        Some(serde_json::Value::Object(mut o)) => match o.remove("entries") {
            Some(serde_json::Value::Array(l)) => l,
            _ => return Ok(Vec::new()),
        },
        _ => return Ok(Vec::new()),
    };
    Ok(list.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect())
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Ready {
    pub session_id: String,
    pub resume_gateway_url: Option<String>,
    pub user: User,
    #[serde(deserialize_with = "lenient_vec")]
    pub guilds: Vec<Guild>,
    #[serde(deserialize_with = "lenient_vec")]
    pub private_channels: Vec<Channel>,
    #[serde(deserialize_with = "lenient_vec")]
    pub users: Vec<User>,
    pub merged_members: Vec<Vec<Member>>,
    #[serde(deserialize_with = "entries")]
    pub read_state: Vec<ReadStateEntry>,
    #[serde(deserialize_with = "entries")]
    pub user_guild_settings: Vec<UserGuildSettings>,
    pub user_settings_proto: Option<String>,
    pub user_settings: Option<LegacyUserSettings>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct LegacyUserSettings {
    pub guild_folders: Option<Vec<GuildFolder>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SupplementalGuild {
    pub id: Snowflake,
    #[serde(deserialize_with = "lenient_vec")]
    pub voice_states: Vec<VoiceState>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Presence {
    pub user_id: Option<Snowflake>,
    pub user: Option<PresenceUser>,
    pub status: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PresenceUser {
    pub id: Snowflake,
}

impl Presence {
    pub fn user_id(&self) -> Snowflake {
        self.user_id.or(self.user.as_ref().map(|u| u.id)).unwrap_or_default()
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct MergedPresences {
    pub friends: Vec<Presence>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ReadySupplemental {
    pub merged_presences: MergedPresences,
    #[serde(deserialize_with = "lenient_vec")]
    pub guilds: Vec<SupplementalGuild>,
}

/// A partial message from MESSAGE_UPDATE; present fields replace the cached ones.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct MessageUpdate {
    pub id: Snowflake,
    pub channel_id: Snowflake,
    pub guild_id: Option<Snowflake>,
    pub author: Option<User>,
    pub content: Option<String>,
    pub edited_timestamp: Option<DateTime<Utc>>,
    pub mentions: Option<Vec<User>>,
    pub mention_roles: Option<Vec<Snowflake>>,
    pub mention_everyone: Option<bool>,
    pub attachments: Option<Vec<Attachment>>,
    pub embeds: Option<Vec<Embed>>,
    pub flags: Option<u64>,
    pub pinned: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct MessageDelete {
    pub id: Snowflake,
    pub channel_id: Snowflake,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct MessageDeleteBulk {
    pub ids: Vec<Snowflake>,
    pub channel_id: Snowflake,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct MessageAck {
    pub channel_id: Snowflake,
    pub message_id: Snowflake,
    pub mention_count: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ReactionEvent {
    pub user_id: Snowflake,
    pub channel_id: Snowflake,
    pub message_id: Snowflake,
    pub emoji: Emoji,
    /// 1 for super (burst) reactions.
    #[serde(rename = "type")]
    pub kind: u8,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ReactionRemoveAll {
    pub channel_id: Snowflake,
    pub message_id: Snowflake,
    pub emoji: Option<Emoji>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct TypingStart {
    pub channel_id: Snowflake,
    pub guild_id: Option<Snowflake>,
    pub user_id: Snowflake,
    pub member: Option<Member>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct GuildDelete {
    pub id: Snowflake,
    pub unavailable: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct GuildRoleEvent {
    pub guild_id: Snowflake,
    pub role: Option<Role>,
    pub role_id: Option<Snowflake>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct GuildMemberUpdate {
    pub guild_id: Snowflake,
    pub user: User,
    pub nick: Option<String>,
    pub avatar: Option<String>,
    pub roles: Vec<Snowflake>,
    pub joined_at: Option<DateTime<Utc>>,
    pub communication_disabled_until: Option<DateTime<Utc>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct GuildEmojisUpdate {
    pub guild_id: Snowflake,
    pub emojis: Vec<Emoji>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct GuildUpdate {
    pub id: Snowflake,
    pub name: Option<String>,
    pub icon: Option<String>,
    pub owner_id: Option<Snowflake>,
    pub roles: Option<Vec<Role>>,
    pub emojis: Option<Vec<Emoji>>,
    pub default_message_notifications: Option<u8>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PresenceUpdate {
    pub user: PresenceUser,
    pub guild_id: Option<Snowflake>,
    pub status: String,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct VoiceServerUpdate {
    pub token: String,
    pub guild_id: Option<Snowflake>,
    pub channel_id: Option<Snowflake>,
    pub endpoint: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SettingsProtoUpdate {
    pub settings: SettingsProto,
    pub partial: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SettingsProto {
    #[serde(rename = "type")]
    pub kind: u8,
    pub proto: String,
}

#[derive(Debug)]
pub enum Event {
    Ready(Box<Ready>),
    ReadySupplemental(Box<ReadySupplemental>),
    Resumed,
    GuildCreate(Box<Guild>),
    GuildUpdate(Box<GuildUpdate>),
    GuildDelete(GuildDelete),
    GuildRoleUpsert(GuildRoleEvent),
    GuildRoleDelete(GuildRoleEvent),
    GuildMemberUpdate(Box<GuildMemberUpdate>),
    GuildEmojisUpdate(GuildEmojisUpdate),
    ChannelCreate(Box<Channel>),
    ChannelUpdate(Box<Channel>),
    ChannelDelete(Box<Channel>),
    MessageCreate(Box<Message>),
    MessageUpdate(Box<MessageUpdate>),
    MessageDelete(MessageDelete),
    MessageDeleteBulk(MessageDeleteBulk),
    MessageAck(MessageAck),
    ReactionAdd(ReactionEvent),
    ReactionRemove(ReactionEvent),
    ReactionRemoveAll(ReactionRemoveAll),
    TypingStart(Box<TypingStart>),
    PresenceUpdate(PresenceUpdate),
    UserUpdate(Box<User>),
    UserSettingsProtoUpdate(SettingsProtoUpdate),
    UserGuildSettingsUpdate(Box<UserGuildSettings>),
    VoiceStateUpdate(Box<VoiceState>),
    VoiceServerUpdate(VoiceServerUpdate),
    /// Anything else, kept raw for subsystems (e.g. Go Live streams) that parse it themselves.
    Other(String, Box<RawValue>),
}

impl Event {
    pub fn parse(name: &str, data: Box<RawValue>) -> Result<Event, serde_json::Error> {
        fn p<T: DeserializeOwned>(d: &RawValue) -> Result<T, serde_json::Error> {
            serde_json::from_str(d.get())
        }
        let d = &*data;
        Ok(match name {
            "READY" => Event::Ready(p(d)?),
            "READY_SUPPLEMENTAL" => Event::ReadySupplemental(p(d)?),
            "RESUMED" => Event::Resumed,
            "GUILD_CREATE" => Event::GuildCreate(p(d)?),
            "GUILD_UPDATE" => Event::GuildUpdate(p(d)?),
            "GUILD_DELETE" => Event::GuildDelete(p(d)?),
            "GUILD_ROLE_CREATE" | "GUILD_ROLE_UPDATE" => Event::GuildRoleUpsert(p(d)?),
            "GUILD_ROLE_DELETE" => Event::GuildRoleDelete(p(d)?),
            "GUILD_MEMBER_UPDATE" => Event::GuildMemberUpdate(p(d)?),
            "GUILD_EMOJIS_UPDATE" => Event::GuildEmojisUpdate(p(d)?),
            "CHANNEL_CREATE" | "THREAD_CREATE" => Event::ChannelCreate(p(d)?),
            "CHANNEL_UPDATE" | "THREAD_UPDATE" => Event::ChannelUpdate(p(d)?),
            "CHANNEL_DELETE" | "THREAD_DELETE" => Event::ChannelDelete(p(d)?),
            "MESSAGE_CREATE" => Event::MessageCreate(p(d)?),
            "MESSAGE_UPDATE" => Event::MessageUpdate(p(d)?),
            "MESSAGE_DELETE" => Event::MessageDelete(p(d)?),
            "MESSAGE_DELETE_BULK" => Event::MessageDeleteBulk(p(d)?),
            "MESSAGE_ACK" => Event::MessageAck(p(d)?),
            "MESSAGE_REACTION_ADD" => Event::ReactionAdd(p(d)?),
            "MESSAGE_REACTION_REMOVE" => Event::ReactionRemove(p(d)?),
            "MESSAGE_REACTION_REMOVE_ALL" | "MESSAGE_REACTION_REMOVE_EMOJI" => Event::ReactionRemoveAll(p(d)?),
            "TYPING_START" => Event::TypingStart(p(d)?),
            "PRESENCE_UPDATE" => Event::PresenceUpdate(p(d)?),
            "USER_UPDATE" => Event::UserUpdate(p(d)?),
            "USER_SETTINGS_PROTO_UPDATE" => Event::UserSettingsProtoUpdate(p(d)?),
            "USER_GUILD_SETTINGS_UPDATE" => Event::UserGuildSettingsUpdate(p(d)?),
            "VOICE_STATE_UPDATE" => Event::VoiceStateUpdate(p(d)?),
            "VOICE_SERVER_UPDATE" => Event::VoiceServerUpdate(p(d)?),
            _ => Event::Other(name.to_owned(), data),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_skips_unreadable_guilds_and_reads_wrapped_entries() {
        let json = r#"{"session_id":"s","user":{"id":"1","username":"me"},
            "guilds":[{"id":"10","name":"ok"},{"id":"not a number"}],
            "read_state":{"version":1,"partial":false,"entries":[{"id":"5","last_message_id":"6","mention_count":2}]},
            "user_guild_settings":[{"guild_id":"10","muted":true}]}"#;
        let raw = RawValue::from_string(json.to_owned()).unwrap();
        let Event::Ready(r) = Event::parse("READY", raw).unwrap() else { panic!() };
        assert_eq!(r.guilds.len(), 1);
        assert_eq!(r.read_state[0].mention_count, 2);
        assert!(r.user_guild_settings[0].muted);
    }

    #[test]
    fn unknown_events_stay_raw() {
        let raw = RawValue::from_string(r#"{"stream_key":"x"}"#.to_owned()).unwrap();
        assert!(matches!(Event::parse("STREAM_CREATE", raw).unwrap(), Event::Other(n, _) if n == "STREAM_CREATE"));
    }
}
