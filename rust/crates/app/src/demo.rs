//! A local "Swiftcord demo" conversation with sample embeds and links, for checking rendering without
//! touching real conversations. Enabled with `SWIFTCORD_DEMO=1`; nothing is sent to Discord.

use swiftcord_core::events::parse_lenient_list;
use swiftcord_core::models::{Channel, ChannelType, Message, User};
use swiftcord_core::state::State;
use swiftcord_core::Snowflake;

pub const CHANNEL: Snowflake = Snowflake(1_000_000_000_000_000_001);

pub fn enabled() -> bool {
    std::env::var("SWIFTCORD_DEMO").is_ok_and(|v| v == "1")
}

pub fn install(state: &mut State) {
    let me = state.me.clone().unwrap_or_default();
    let bot = User { id: Snowflake(2), username: "Demo Bot".into(), bot: true, ..Default::default() };
    state.add_channel(Channel {
        id: CHANNEL,
        kind: ChannelType::Dm,
        name: Some("Swiftcord demo".into()),
        recipients: vec![bot],
        // Sorts first in the DM list.
        last_message_id: Some(swiftcord_core::nonce_now()),
        ..Default::default()
    });
    let json = SAMPLES.replace("{CH}", &CHANNEL.to_string()).replace("{ME}", &me.id.to_string());
    let mut msgs: Vec<Message> = parse_lenient_list(&json).unwrap_or_default();
    msgs.reverse(); // History pages are newest first.
    state.add_history(CHANNEL, msgs, 50);
    // A pretend incoming call from the demo bot, to show the call screen and popup.
    let call = format!(
        r#"{{"channel_id":"{CHANNEL}","ringing":["{}"],"voice_states":[{{"user_id":"2","channel_id":"{CHANNEL}","session_id":"demo"}}]}}"#,
        me.id
    );
    if let Ok(raw) = serde_json::value::RawValue::from_string(call) {
        if let Ok(e) = swiftcord_core::events::Event::parse("CALL_CREATE", raw) {
            state.apply(e);
        }
    }
}

const SAMPLES: &str = r##"[
{"id":"1400000000000000001","channel_id":"{CH}","author":{"id":"2","username":"Demo Bot","bot":true},
 "timestamp":"2026-10-25T10:00:00+00:00","content":"A rich embed:",
 "embeds":[{"type":"rich","title":"Server status","url":"https://example.com/status","color":5793266,
   "description":"All systems **operational**. See [the dashboard](https://example.com) for details.\n> Last checked just now",
   "author":{"name":"Status Bot","icon_url":"https://cdn.discordapp.com/embed/avatars/1.png"},
   "thumbnail":{"url":"https://cdn.discordapp.com/embed/avatars/2.png","proxy_url":"https://cdn.discordapp.com/embed/avatars/2.png","width":128,"height":128},
   "fields":[{"name":"API","value":"🟢 Up","inline":true},{"name":"Gateway","value":"🟢 Up","inline":true},{"name":"Voice","value":"🟡 Degraded","inline":true},
             {"name":"Notes","value":"Voice in us-east is being investigated. This field is not inline, so it spans the embed.","inline":false}],
   "footer":{"text":"Updated","icon_url":"https://cdn.discordapp.com/embed/avatars/3.png"},"timestamp":"2026-10-25T09:59:00+00:00"}]},
{"id":"1400000000000000002","channel_id":"{CH}","author":{"id":"{ME}","username":"me"},
 "timestamp":"2026-10-25T10:01:00+00:00","content":"https://www.youtube.com/watch?v=dQw4w9WgXcQ",
 "embeds":[{"type":"video","url":"https://www.youtube.com/watch?v=dQw4w9WgXcQ","title":"Rick Astley - Never Gonna Give You Up (Official Video)",
   "description":"The official video for “Never Gonna Give You Up” by Rick Astley.","color":16711680,
   "provider":{"name":"YouTube","url":"https://www.youtube.com"},"author":{"name":"Rick Astley","url":"https://www.youtube.com/@RickAstleyYT"},
   "thumbnail":{"url":"https://i.ytimg.com/vi/dQw4w9WgXcQ/maxresdefault.jpg","proxy_url":"https://images-ext-1.discordapp.net/external/x/https/i.ytimg.com/vi/dQw4w9WgXcQ/maxresdefault.jpg","width":1280,"height":720},
   "video":{"url":"https://www.youtube.com/embed/dQw4w9WgXcQ","width":1280,"height":720}}]},
{"id":"1400000000000000003","channel_id":"{CH}","author":{"id":"2","username":"Demo Bot","bot":true},
 "timestamp":"2026-10-25T10:02:00+00:00","content":"A link preview: https://github.com/emilk/egui",
 "embeds":[{"type":"article","url":"https://github.com/emilk/egui","title":"GitHub - emilk/egui: an easy-to-use immediate mode GUI in Rust",
   "description":"egui: an easy-to-use immediate mode GUI in Rust that runs on both web and native.","color":1975079,
   "provider":{"name":"GitHub"},
   "thumbnail":{"url":"https://opengraph.githubassets.com/1/emilk/egui","proxy_url":"https://opengraph.githubassets.com/1/emilk/egui","width":1200,"height":600}}]},
{"id":"1400000000000000004","channel_id":"{CH}","author":{"id":"{ME}","username":"me"},
 "timestamp":"2026-10-25T10:03:00+00:00","content":"https://klipy.com/gifs/lol",
 "embeds":[{"type":"gifv","url":"https://klipy.com/gifs/lol","provider":{"name":"Klipy"},
   "thumbnail":{"url":"https://cdn.discordapp.com/embed/avatars/4.png","proxy_url":"https://cdn.discordapp.com/embed/avatars/4.png","width":498,"height":280},
   "video":{"url":"https://static.klipy.com/ii/c3a19a0b747a76e98651f2b9a3cca5ff/8a/b1/A0uhZl0BSvMFS.mp4","width":498,"height":280}}]},
{"id":"1400000000000000005","channel_id":"{CH}","author":{"id":"2","username":"Demo Bot","bot":true},
 "timestamp":"2026-10-25T10:04:00+00:00","content":"Link to an earlier message: https://discord.com/channels/@me/{CH}/1400000000000000001 and a masked one: [jump](https://discord.com/channels/@me/{CH}/1400000000000000003)"},
{"id":"1400000000000000006","channel_id":"{CH}","author":{"id":"2","username":"Demo Bot","bot":true},
 "timestamp":"2026-10-25T10:05:00+00:00","content":"",
 "embeds":[{"type":"rich","description":"An embed with only a description and a big image.","color":15548997,
   "image":{"url":"https://cdn.discordapp.com/embed/avatars/0.png","proxy_url":"https://cdn.discordapp.com/embed/avatars/0.png","width":256,"height":256}}]},
{"id":"1400000000000000007","channel_id":"{CH}","author":{"id":"2","username":"Demo Bot","bot":true},
 "timestamp":"2026-10-25T10:06:00+00:00","content":"An embed bot's post with four pictures (extra embeds sharing the URL):",
 "embeds":[{"type":"rich","url":"https://fxtwitter.com/demo/status/1","color":1942002,"description":"Four pictures from one post",
   "author":{"name":"Demo (@demo)","url":"https://x.com/demo","icon_url":"https://cdn.discordapp.com/embed/avatars/5.png"},
   "footer":{"text":"FxTwitter"},
   "image":{"url":"https://cdn.discordapp.com/embed/avatars/0.png","proxy_url":"https://cdn.discordapp.com/embed/avatars/0.png","width":256,"height":256}},
  {"type":"rich","url":"https://fxtwitter.com/demo/status/1","image":{"url":"https://cdn.discordapp.com/embed/avatars/1.png","proxy_url":"https://cdn.discordapp.com/embed/avatars/1.png","width":256,"height":256}},
  {"type":"rich","url":"https://fxtwitter.com/demo/status/1","image":{"url":"https://cdn.discordapp.com/embed/avatars/2.png","proxy_url":"https://cdn.discordapp.com/embed/avatars/2.png","width":256,"height":256}},
  {"type":"rich","url":"https://fxtwitter.com/demo/status/1","image":{"url":"https://cdn.discordapp.com/embed/avatars/3.png","proxy_url":"https://cdn.discordapp.com/embed/avatars/3.png","width":256,"height":256}}]},
{"id":"1400000000000000008","channel_id":"{CH}","author":{"id":"2","username":"Demo Bot","bot":true},
 "timestamp":"2026-10-25T10:07:00+00:00","content":"",
 "embeds":[{"type":"video","url":"https://fxtwitter.com/demo/status/2","color":1942002,"description":"A post with a video and no thumbnail",
   "author":{"name":"Demo (@demo)","url":"https://x.com/demo"},
   "video":{"url":"https://static.klipy.com/ii/c3a19a0b747a76e98651f2b9a3cca5ff/8a/b1/A0uhZl0BSvMFS.mp4","width":498,"height":280}}]},
{"id":"1400000000000000009","channel_id":"{CH}","author":{"id":"2","username":"Demo Bot","bot":true},
 "timestamp":"2026-10-25T10:08:00+00:00","content":"","flags":36868,"embeds":[],
 "components":[{"type":17,"accent_color":null,"components":[
   {"type":9,"components":[{"type":10,"content":"**ENCE ([@ENCE](https://x.com/ence))**"},{"type":10,"content":"End of an era 💔"}],
    "accessory":{"type":11,"media":{"url":"https://pbs.twimg.com/profile_images/2039244530504089600/UQhd8MUj_200x200.png",
      "proxy_url":"https://images-ext-1.discordapp.net/external/n-CEjCguGStmPhEFtLrEj_BU-rC0kdLItnU136BjeAs/https/pbs.twimg.com/profile_images/2039244530504089600/UQhd8MUj_200x200.png",
      "width":200,"height":200,"content_type":"image/png"}}},
   {"type":12,"items":[{"media":{"url":"https://pbs.twimg.com/media/HVj6mU8WsAEzNmB.jpg?name=orig",
      "proxy_url":"https://images-ext-1.discordapp.net/external/hDG2OBiqL61eThZSZq2ftymaSiYiyE7RYx12HBYk5_I/%3Fname%3Dorig/https/pbs.twimg.com/media/HVj6mU8WsAEzNmB.jpg",
      "width":2000,"height":2500,"content_type":"image/jpeg"}}]},
   {"type":10,"content":"-# 💬 218  🔁 219  ❤️ 3.61K  📊 77.08K"},
   {"type":10,"content":"-# <t:1793023201:R> • [Link](https://x.com/ence/status/2114718698422284713)"},
   {"type":14,"divider":true,"spacing":1},
   {"type":1,"components":[{"type":2,"style":5,"label":"Open post","url":"https://x.com/ence/status/2114718698422284713"},
     {"type":2,"style":1,"label":"Translate","custom_id":"t"},{"type":3,"custom_id":"m","options":[],"placeholder":"More options"}]}]}]}
]"##;
