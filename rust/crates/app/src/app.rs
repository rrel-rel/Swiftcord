//! Application state and the glue between the UI, background tasks and the Discord session.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use swiftcord_core::gateway::{ConnectionState, Gateway, GatewayConfig, GatewayOutput};
use swiftcord_core::models::{Emoji, Message, PendingState};
use swiftcord_core::remote_auth::RemoteAuthEvent;
use swiftcord_core::rest::{FileUpload, NewMessage, Rest};
use swiftcord_core::state::{Notice, State};
use swiftcord_core::{nonce_now, Snowflake};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::platform::{self, credentials};
use crate::ui;

const HISTORY_PAGE: u8 = 50;
const TYPING_INTERVAL: Duration = Duration::from_secs(8);
/// Windows' own: the text cursor blinks for this long after the last input.
const CARET_TIMEOUT: Duration = Duration::from_secs(5);
/// Gateway events that change nothing on screen right away (presences, member lists) are shown within this
/// long, so a busy server's stream of them doesn't redraw the window for each one.
const BACKGROUND_REDRAW: Duration = Duration::from_millis(1000);
/// One-click reactions offered (the hover bar shows three of them; the right-click menu all).
pub const QUICK_REACTIONS: usize = 4;

/// Discord's upload limit without Nitro.
pub const MAX_UPLOAD_BYTES: usize = 10 * 1024 * 1024;

pub enum AppEvent {
    Gateway(GatewayOutput),
    History { channel: Snowflake, result: Result<Vec<Message>, String> },
    Newer { channel: Snowflake, result: Result<Vec<Message>, String> },
    Around { channel: Snowflake, message: Snowflake, result: Result<Vec<Message>, String> },
    Sent { channel: Snowflake, nonce: Snowflake, result: Result<Message, String> },
    Error(String),
    Token(String),
    TokenRejected(String),
    RemoteAuth(RemoteAuthEvent),
    FilesPicked { channel: Snowflake, files: Vec<FileUpload> },
    Pins { channel: Snowflake, result: Result<Vec<Message>, String> },
    DmOpened(swiftcord_core::models::Channel),
    Folders(Option<Vec<swiftcord_core::models::GuildFolder>>),
    Voice { generation: u64, event: swiftcord_voice::Event },
    /// GIF results for a search (an empty query is trending).
    Gifs { query: String, result: Result<Vec<swiftcord_core::rest::Gif>, String> },
    GifCategories(Result<Vec<String>, String>),
    StickerPacks(Result<Vec<swiftcord_core::models::StickerPack>, String>),
    StreamPreview { key: String, result: Result<Option<String>, String> },
    SearchResults { generation: u64, result: Result<swiftcord_core::rest::SearchResult, String> },
    Mentions { generation: u64, result: Result<Vec<Message>, String> },
    /// A picture chosen for the profile editor: its data URI and a preview (`None` if the dialog was cancelled).
    ProfileImagePicked { server: bool, kind: ui::profile_edit::ImageKind, result: Result<Option<(String, egui::ColorImage)>, String> },
    /// A server's (or DM's) slash commands, by the server or DM channel ID.
    Commands { key: Snowflake, result: Result<swiftcord_core::commands::Index, String> },
    /// An interaction request went out (or didn't).
    InteractionSent { nonce: String, result: Result<(), String> },
    /// Profile changes saved: each request's path and response, then the error that stopped the rest, if any.
    ProfileSaved { done: Vec<(String, serde_json::Value)>, error: Option<String> },
    Profile { user: Snowflake, guild: Option<Snowflake>, result: Result<swiftcord_core::models::Profile, String> },
    /// The frecency settings (favourite GIFs) as read or as saved.
    Frecency(Result<String, String>),
    /// Saving favourites failed; restore these settings.
    FrecencyRejected { previous: Option<String>, error: String },
    Stream { generation: u64, event: swiftcord_voice::Event },
    Share { generation: u64, event: swiftcord_voice::Event },
    /// A page of a channel's threads: active ones (first page only) and archived ones.
    Threads {
        channel: Snowflake,
        active: Option<Result<swiftcord_core::rest::ThreadPage, String>>,
        archived: Result<swiftcord_core::rest::ThreadPage, String>,
    },
    /// A thread was fetched or created; show it.
    ThreadOpened(Result<swiftcord_core::models::Channel, String>),
    ThreadJoinFailed { thread: Snowflake, joined: bool, error: String },
}

/// A moderator's change to someone's voice: server mute or deafen, a move, or (`Move(None)`) a disconnect.
#[derive(Debug, Clone, Copy)]
pub enum VoiceMod {
    Mute(bool),
    Deafen(bool),
    Move(Option<Snowflake>),
}

/// Things the UI asks for; applied after the frame is drawn so drawing only borrows state.
pub enum Action {
    SelectGuild(Option<Snowflake>),
    SelectChannel(Snowflake),
    LoadOlder(Snowflake),
    Send(Snowflake),
    SaveEdit { channel: Snowflake, message: Snowflake, content: String },
    Delete { channel: Snowflake, message: Snowflake },
    React { channel: Snowflake, message: Snowflake, emoji: Emoji, add: bool },
    Retry { channel: Snowflake, nonce: Snowflake },
    Discard { channel: Snowflake, nonce: Snowflake },
    Typing(Snowflake),
    PickFiles(Snowflake),
    AddFiles { channel: Snowflake, files: Vec<FileUpload> },
    PasteImage { channel: Snowflake, image: std::sync::Arc<egui::ColorImage> },
    Notice(String),
    SearchMembers { guild: Snowflake, query: String },
    OpenGifPicker { channel: Snowflake, at: egui::Pos2 },
    OpenStickerPicker { channel: Snowflake, at: egui::Pos2 },
    LoadStickerPacks,
    SendSticker { channel: Snowflake, sticker: swiftcord_core::models::StickerItem },
    /// Search GIFs; an empty query loads trending ones.
    GifSearch(String),
    GifCategories,
    SendGif { channel: Snowflake, url: String },
    CloseGifPicker,
    ToggleFavoriteGif(swiftcord_core::proto::FavoriteGif),
    OpenDm(Snowflake),
    /// Show someone's profile card (as they appear in `guild`) at `at`.
    OpenProfile { user: Snowflake, guild: Option<Snowflake>, at: egui::Pos2 },
    CloseProfile,
    ShowPins(Snowflake),
    /// Open or close the thread list beside a channel.
    ShowThreads(Snowflake),
    /// Load a channel's threads (forums do this when shown), or with `more` the next page of archived ones.
    LoadThreads { channel: Snowflake, more: bool },
    OpenThread(Snowflake),
    /// Ask for a name for a new thread (from `message`, if given) or forum post in `channel`.
    StartThread { channel: Snowflake, message: Option<Snowflake>, name: String },
    CloseThreadPrompt,
    CreateThread,
    SetThreadJoined(Snowflake, bool),
    ToggleMemberList,
    /// Online, idle, do not disturb or invisible (`Offline`).
    SetStatus(swiftcord_core::models::PresenceStatus),
    /// Discord's "Share my activity".
    SetShareActivity(bool),
    /// Opens a voice channel's screen with its text chat showing.
    OpenVoiceChat(Snowflake),
    /// Someone's volume in calls, for us; `save` once the slider is let go.
    SetUserVolume { user: Snowflake, volume: f32, save: bool },
    /// Mutes or unmutes someone in calls, for us alone.
    ToggleLocalMute(Snowflake),
    /// Opens a DM with someone and starts a call there.
    CallUser(Snowflake),
    /// Plays a web player embed (YouTube) inline in its message; `None` stops it.
    PlayWeb(Option<(String, Snowflake)>),
    /// Forwards a message to up to five DMs or channels, each followed by `note` if there is one.
    Forward { channel: Snowflake, message: Snowflake, to: Vec<Snowflake>, note: String },
    /// Marks a channel unread from this message on.
    MarkUnread { channel: Snowflake, message: Snowflake },
    /// A moderator's change to someone's voice state in a server.
    ModerateVoice { guild: Snowflake, user: Snowflake, change: VoiceMod },
    /// The member list beside `channel` shows rows `first..=last`; subscribes to the blocks covering them.
    MemberListRange { guild: Snowflake, channel: Snowflake, first: usize, last: usize },
    Ack(Snowflake),
    JoinVoice { guild: Option<Snowflake>, channel: Snowflake },
    /// Answer a DM call: join it and show the conversation.
    AcceptCall(Snowflake),
    DeclineCall(Snowflake),
    WatchStream { guild: Option<Snowflake>, channel: Snowflake, user: Snowflake },
    /// Fetch a Go Live stream's preview picture (by stream key).
    LoadStreamPreview(String),
    /// Open the profile editor: on the main profile, or (`server`) on our profile in `guild`.
    EditProfile { guild: Option<Snowflake>, server: bool },
    /// Choose a new avatar or banner for the main (`server: false`) or server profile.
    PickProfileImage { server: bool, kind: ui::profile_edit::ImageKind },
    SaveProfile,
    CloseProfileEditor,
    /// Use an app's button (`values: None`) or select menu on a message.
    UseComponent { msg: Box<Message>, component: Box<swiftcord_core::models::Component>, values: Option<Vec<String>> },
    SubmitModal,
    CloseModal,
    /// Fetch the slash commands usable in this server (or DM).
    LoadCommands { guild: Option<Snowflake>, channel: Snowflake },
    /// Run the slash command being filled in, in this channel's composer.
    RunCommand(Snowflake),
    /// Search messages (the whole server, or the DM) from this channel.
    Search { channel: Snowflake, text: String },
    SearchMore,
    CloseSearch,
    /// Opens or closes the mentions inbox.
    ShowInbox,
    InboxMore,
    InboxFilters { guild: Option<Snowflake>, everyone: bool, roles: bool },
    DismissMention(Snowflake),
    StopWatching,
    /// The watched stream's volume (0–2), and whether to keep hearing it while deafened.
    SetStreamAudio { volume: f32, when_deafened: bool },
    OpenSharePicker,
    CloseSharePicker,
    /// Go live with what the share picker has selected.
    StartShare,
    StopShare,
    /// Show the stream in the main area (true) or go back to the chat (false).
    ShowStream(bool),
    ToggleFullscreen,
    LeaveVoice,
    ToggleMute,
    ToggleDeafen,
    ToggleCamera,
    Copy(String),
    /// A clicked link: Discord channel/message links open in the app, others in the browser.
    OpenLink(String),
    JumpTo { channel: Snowflake, message: Option<Snowflake> },
    LoadNewer(Snowflake),
    JumpToPresent(Snowflake),
    LogOut,
    StartQrLogin,
    SaveSettings(crate::settings::Settings),
    /// Start (or stop starting) when the user signs in to Windows.
    SetLaunchAtStartup(bool),
    /// Show the app in this palette and accent (while choosing in Settings; Cancel restores the saved one).
    OpenSettings,
    BrowserLogin,
    TokenLogin(String),
}

pub struct Session {
    pub rest: Rest,
    pub gateway: Gateway,
    pub state: State,
    pub conn: ConnectionState,
    pub ready: bool,
    subscribed: HashSet<Snowflake>,
    last_typing: HashMap<Snowflake, Instant>,
    last_ack: HashMap<Snowflake, Snowflake>,
}

/// A message being written in one channel.
#[derive(Default)]
pub struct Draft {
    pub text: String,
    /// Replying to this message, pinging its author if the flag is set.
    pub reply: Option<(Snowflake, bool)>,
    pub files: Vec<FileUpload>,
    /// Caret position (in characters) as of the last frame, for autocomplete.
    pub caret: usize,
    /// Mentions picked from suggestions: readable text in the box, and what it becomes when sent.
    pub mentions: Vec<(String, String)>,
    /// The token start the user dismissed suggestions for (Escape).
    pub dismissed: Option<usize>,
    pub selected: usize,
    /// A slash command being filled in, shown in place of the text box.
    pub command: Option<ui::commands::CommandDraft>,
}

/// Retained copies of failed sends so they can be retried.
struct Outgoing {
    msg: NewMessage,
    files: Vec<FileUpload>,
}

#[derive(Default)]
pub struct UiState {
    /// `None` is Home (direct messages).
    pub guild: Option<Snowflake>,
    /// Last channel per guild (`Snowflake::NONE` for Home).
    pub channel_for: HashMap<Snowflake, Snowflake>,
    pub collapsed: HashSet<Snowflake>,
    pub sidebar: ui::sidebar::Lists,
    pub open_folders: HashSet<i64>,
    pub drafts: HashMap<Snowflake, Draft>,
    pub editing: Option<(Snowflake, String)>,
    /// A message waiting for "Delete Message" to be confirmed: (channel, message).
    pub confirm_delete: Option<(Snowflake, Snowflake)>,
    /// The picture open in the image viewer.
    pub viewer: Option<ui::viewer::Viewed>,
    /// The voice channel screen shows the channel's text chat beside it.
    pub voice_room_chat: bool,
    /// The "Forward To" picker, while open.
    pub forward: Option<ui::forward::Forward>,
    /// The web player (YouTube) playing inline: its embed URL and channel.
    pub web_player: Option<(String, Snowflake)>,
    /// Where that player's embed was drawn this frame, and the visible part of the list around it.
    pub web_player_slot: Option<(egui::Rect, egui::Rect)>,
    /// Where we'd read up to when the open channel was opened with unread messages: the NEW line goes after
    /// it, until the channel is marked read.
    pub unread_marker: HashMap<Snowflake, Snowflake>,
    /// A channel marked unread while open: it isn't marked read again as messages come in, until it's left
    /// or marked read.
    pub kept_unread: Option<Snowflake>,
    pub chat: HashMap<Snowflake, ui::chat::ChatView>,
    pub loading: HashSet<Snowflake>,
    pub revealed: HashSet<(Snowflake, u16)>,
    pub emoji_picker: Option<ui::emoji::PickerTarget>,
    pub emoji_search: String,
    pub pins: Option<(Snowflake, Option<Vec<Message>>)>,
    /// The channel whose thread list is open beside the chat.
    pub threads_panel: Option<Snowflake>,
    pub thread_browsers: HashMap<Snowflake, crate::threads_glue::Browser>,
    pub thread_prompt: Option<crate::threads_glue::Prompt>,
    pub banner: Option<(String, Instant)>,
    pub login: ui::login::LoginUi,
    pub self_mute: bool,
    pub self_deaf: bool,
    /// Our camera is on in the current call.
    pub self_video: bool,
    /// The latest camera picture of each person in our call with theirs on (ours is a mirror preview).
    pub cameras: HashMap<Snowflake, egui::TextureHandle>,
    pub focus_composer: bool,
    /// Users heard in the current call just now (including us).
    pub speaking: HashSet<Snowflake>,
    pub voice: Option<crate::voice_glue::VoiceView>,
    pub stream: Option<crate::stream_glue::StreamView>,
    pub share: Option<crate::share_glue::ShareView>,
    pub share_picker: Option<crate::share_glue::Picker>,
    /// The quality last chosen for going live.
    pub share_quality: crate::share_glue::Quality,
    pub settings_draft: Option<ui::settings::Draft>,
    /// A pending member search (guild, query, when it changed, sent yet).
    pub member_query: Option<(Snowflake, String, Instant, bool)>,
    pub hide_members: bool,
    /// The member list we're subscribed to: channel and row ranges.
    pub member_sub: Option<(Snowflake, Vec<[u32; 2]>)>,
    pub gif_picker: Option<ui::gif::GifPicker>,
    pub sticker_picker: Option<ui::sticker_picker::StickerPicker>,
    /// Discord's standard sticker packs: `Some(None)` while they load.
    pub sticker_packs: Option<Option<Vec<swiftcord_core::models::StickerPack>>>,
    /// Animated stickers being shown.
    pub stickers: crate::stickers::Stickers,
    /// Go Live preview pictures by stream key: when fetched, and the picture (`None` while loading, then
    /// `Some(None)` if there isn't one).
    pub stream_previews: HashMap<String, (Instant, Option<Option<String>>)>,
    /// The open message search, and what's typed in the header's search box.
    pub search: Option<crate::search_glue::Search>,
    pub search_box: String,
    /// The open mentions inbox.
    pub inbox: Option<crate::inbox_glue::Inbox>,
    /// The open profile editor.
    pub profile_editor: Option<ui::profile_edit::Editor>,
    /// Apps' buttons and menus waiting for an answer, and an app's open form.
    pub interactions: crate::interactions_glue::Interactions,
    /// Slash commands, by server (or DM channel) ID.
    pub commands: HashMap<Snowflake, ui::commands::Loaded>,
    /// One-click reactions for the server being viewed, and what they were worked out for (server, emoji
    /// catalog version, state revision).
    pub quick_reactions: (Option<(Option<Snowflake>, u64, u64)>, Vec<Emoji>),
    pub profile_card: Option<ui::profile::Card>,
    pub profiles: ui::profile::Profiles,
    /// The watched stream's sound: volume (0–2) and whether it plays while deafened (from settings).
    pub stream_audio: (f32, bool),
    /// Copies of the saved per-person call volumes and mutes, for their voice menus.
    pub user_volumes: HashMap<Snowflake, f32>,
    pub muted_users: HashSet<Snowflake>,
    /// Favourite GIFs, most recent first (from the synced frecency settings).
    pub favorite_gifs: Vec<swiftcord_core::proto::FavoriteGif>,
    /// Players for GIF-style video clips on screen.
    pub clips: crate::clip::Clips,
    /// Videos played in chat.
    pub videos: crate::player::Videos,
    /// Voice messages and audio files played in chat.
    pub audio: crate::audio::AudioClips,
}

impl UiState {
    pub fn channel(&self) -> Option<Snowflake> {
        self.channel_for.get(&self.guild.unwrap_or_default()).copied()
    }
}

pub struct App {
    pub rt: tokio::runtime::Handle,
    tx: UnboundedSender<AppEvent>,
    rx: UnboundedReceiver<AppEvent>,
    ctx: egui::Context,
    pub session: Option<Session>,
    pub ui: UiState,
    outgoing: HashMap<Snowflake, Outgoing>,
    pub emoji: ui::emoji::Catalog,
    pub voice: Option<crate::voice_glue::Call>,
    pub watch: Option<crate::stream_glue::Watch>,
    pub sharing: Option<crate::share_glue::Sharing>,
    pub settings: crate::settings::Settings,
    history_failed: HashMap<Snowflake, Instant>,
    /// Someone we asked to call before their DM existed: the call starts once it's open.
    pending_call: Option<Snowflake>,
    /// The inline web player, while one is playing.
    web_player: Option<crate::platform::inline_player::InlinePlayer>,
    /// Members already asked for (by server), so each is asked for once per connection.
    members_requested: HashSet<(Snowflake, Snowflake)>,
    /// The frecency settings as last read or written, so favourites can be edited without touching the rest.
    frecency: Option<String>,
    pub sounds: crate::sounds::Sounds,
    /// Which channels have message toasts up, and when the message sound last played.
    notifications: crate::notifications::Notifications,
    perf: crate::perf::Perf,
    /// Images shown lately, so those that haven't been can be forgotten.
    images: std::sync::Arc<crate::image_cache::ImageCache>,
    images_checked: Instant,
    /// When the user last typed, clicked or moved the mouse.
    last_input: Instant,
    /// The mentions and window visibility last shown on the taskbar and tray badges.
    badge: Option<(u32, bool)>,
    /// Since when (ms since the epoch) the user has been away from the computer, as told to Discord.
    away_since: Option<u64>,
    away_checked: Instant,
}

impl App {
    pub fn new(ctx: &egui::Context, rt: tokio::runtime::Handle) -> App {
        let settings = crate::settings::Settings::load();
        crate::theme::install(&ctx);
        ctx.add_image_loader(std::sync::Arc::new(crate::image_decode::WicLoader::new(rt.clone())));
        let images = crate::image_cache::install(&ctx);
        let cache = platform::data_dir().join("cache").join("media");
        ctx.add_bytes_loader(std::sync::Arc::new(crate::media::MediaLoader::new(rt.clone(), cache)));

        let (tx, rx) = unbounded_channel();
        let mut app = App {
            rt,
            tx,
            rx,
            ctx: ctx.clone(),
            session: None,
            ui: UiState::default(),
            outgoing: HashMap::new(),
            emoji: ui::emoji::Catalog::load(),
            voice: None,
            watch: None,
            sharing: None,
            settings,
            history_failed: HashMap::new(),
            pending_call: None,
            web_player: None,
            members_requested: HashSet::new(),
            frecency: None,
            sounds: Default::default(),
            notifications: Default::default(),
            perf: Default::default(),
            images,
            images_checked: Instant::now(),
            last_input: Instant::now(),
            badge: None,
            away_since: None,
            away_checked: Instant::now(),
        };
        app.ui.hide_members = app.settings.hide_members;
        app.ui.stream_audio = (app.settings.stream_volume, app.settings.stream_audio_when_deafened);
        app.ui.user_volumes = app.settings.user_volumes.iter().map(|(&u, &v)| (Snowflake(u), v)).collect();
        app.ui.muted_users = app.settings.muted_users.iter().map(|&u| Snowflake(u)).collect();
        match credentials::load_token() {
            Some(token) => {
                app.start_session(token);
                app.restore_session();
            }
            None => app.start_qr_login(),
        }
        app
    }

    /// Shows what was open when the app was last closed, until the gateway's READY replaces it.
    fn restore_session(&mut self) {
        let Some(s) = &mut self.session else { return };
        let Some(saved) = crate::session_cache::load() else { return };
        s.state = State::from_snapshot(saved.state);
        self.ui.guild = saved.guild.filter(|g| s.state.guild(*g).is_some());
        self.ui.channel_for = saved.channel_for.into_iter().collect();
        self.ui.collapsed = saved.collapsed.into_iter().collect();
    }

    fn send_event(tx: &UnboundedSender<AppEvent>, ctx: &egui::Context, e: AppEvent) {
        let _ = tx.send(e);
        ctx.request_repaint();
    }

    /// Delivers events from other threads (e.g. the voice engine) to the UI thread.
    pub fn event_sink(&self) -> impl Fn(AppEvent) + Send + Sync + 'static {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        move |e| Self::send_event(&tx, &ctx, e)
    }

    pub fn ctx(&self) -> &egui::Context {
        &self.ctx
    }

    /// Runs a future on the runtime and delivers its result as an event.
    pub(crate) fn spawn<F>(&self, fut: F)
    where
        F: std::future::Future<Output = AppEvent> + Send + 'static,
    {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        self.rt.spawn(async move { Self::send_event(&tx, &ctx, fut.await) });
    }

    pub fn banner(&mut self, text: impl Into<String>) {
        self.ui.banner = Some((text.into(), Instant::now()));
    }

    // MARK: Session

    fn start_session(&mut self, token: String) {
        let _guard = self.rt.enter();
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        let gateway = Gateway::spawn(GatewayConfig::new(token.clone()), move |out| {
            use swiftcord_core::events::Event;
            let background = matches!(&out, GatewayOutput::Event(Event::PresenceUpdate(_) | Event::MemberListUpdate(_)));
            let _ = tx.send(AppEvent::Gateway(out));
            if background {
                ctx.request_repaint_after(BACKGROUND_REDRAW);
            } else {
                ctx.request_repaint();
            }
        });
        self.session = Some(Session {
            rest: Rest::new(&token),
            gateway,
            state: State::default(),
            conn: ConnectionState::Connecting,
            ready: false,
            subscribed: HashSet::new(),
            last_typing: HashMap::new(),
            last_ack: HashMap::new(),
        });
    }

    fn end_session(&mut self, revoke: bool) {
        if let Some(s) = self.session.take() {
            s.gateway.stop();
            if revoke {
                let rest = s.rest.clone();
                self.rt.spawn(async move {
                    let _ = rest.logout().await;
                });
            }
        }
        credentials::delete_token();
        crate::session_cache::delete();
        self.watch = None;
        self.sharing = None;
        self.voice = None;
        let login = std::mem::take(&mut self.ui.login);
        self.ui = UiState { login, hide_members: self.settings.hide_members, ..Default::default() };
        self.outgoing.clear();
    }

    fn start_qr_login(&mut self) {
        self.ui.login.reset_qr();
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        self.rt.spawn(async move {
            let (etx, mut erx) = unbounded_channel();
            tokio::spawn(swiftcord_core::remote_auth::run(etx, |captcha| async move {
                match tokio::task::spawn_blocking(move || platform::captcha_webview::solve_blocking(&captcha)).await {
                    Ok(Ok(answer)) => answer,
                    Ok(Err(e)) => {
                        tracing::warn!("couldn't open the captcha window: {e}");
                        None
                    }
                    Err(_) => None,
                }
            }));
            while let Some(e) = erx.recv().await {
                Self::send_event(&tx, &ctx, AppEvent::RemoteAuth(e));
            }
        });
    }

    /// Checks a token before keeping it, so a typo doesn't get saved.
    fn try_token(&mut self, token: String) {
        let token = token.trim().trim_matches('"').to_owned();
        if token.is_empty() {
            return;
        }
        self.ui.login.busy = true;
        self.spawn(async move {
            match Rest::new(&token).current_user().await {
                Ok(_) => AppEvent::Token(token),
                Err(e) => AppEvent::TokenRejected(e.to_string()),
            }
        });
    }

    // MARK: Events

    fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Gateway(out) => self.handle_gateway(out),
            AppEvent::History { channel, result } => {
                self.ui.loading.remove(&channel);
                let Some(s) = &mut self.session else { return };
                match result {
                    Ok(page) => {
                        tracing::info!("loaded {} messages in {channel}", page.len());
                        s.state.add_history(channel, page, HISTORY_PAGE as usize);
                        self.request_authors(channel);
                    }
                    Err(e) => {
                        // Don't retry every frame; wait a while before trying this channel again.
                        tracing::warn!("history for {channel} failed: {e}");
                        self.history_failed.insert(channel, Instant::now());
                        self.banner(format!("Couldn't load messages: {e}"));
                    }
                }
            }
            AppEvent::Newer { channel, result } => {
                self.ui.loading.remove(&channel);
                let Some(s) = &mut self.session else { return };
                match result {
                    Ok(page) => {
                        s.state.add_newer(channel, page, HISTORY_PAGE as usize);
                        self.request_authors(channel);
                    }
                    Err(e) => self.banner(format!("Couldn't load messages: {e}")),
                }
            }
            AppEvent::Around { channel, message, result } => {
                self.ui.loading.remove(&channel);
                let Some(s) = &mut self.session else { return };
                match result {
                    Ok(page) if page.iter().any(|m| m.id == message) => {
                        s.state.set_around(channel, page);
                        self.request_authors(channel);
                        self.ui.chat.entry(channel).or_default().jump_to(message);
                    }
                    Ok(_) => self.banner("That message was deleted or isn't available."),
                    Err(e) => self.banner(format!("Couldn't open that message: {e}")),
                }
            }
            AppEvent::Sent { channel, nonce, result } => {
                let Some(s) = &mut self.session else { return };
                match result {
                    Ok(msg) => {
                        self.outgoing.remove(&nonce);
                        s.state.remove_message(channel, nonce);
                        // Inserted like a pending message so it doesn't mark unloaded history as loaded.
                        s.state.add_pending(msg);
                    }
                    Err(e) => {
                        if let Some(m) = s.state.message_mut(channel, nonce) {
                            m.pending = Some(PendingState::Failed);
                        }
                        tracing::warn!("send to {channel} failed: {e}");
                        self.banner(format!("Message not sent: {e}"));
                    }
                }
            }
            AppEvent::Error(e) => self.banner(e),
            AppEvent::Token(token) => {
                self.ui.login = Default::default();
                if let Err(e) = credentials::save_token(&token) {
                    tracing::warn!("couldn't save token: {e}");
                }
                self.start_session(token);
            }
            AppEvent::TokenRejected(e) => {
                tracing::warn!("token rejected: {e}");
                self.ui.login.busy = false;
                self.ui.login.error = Some(format!("That token didn't work: {e}"));
            }
            AppEvent::RemoteAuth(e) => match e {
                RemoteAuthEvent::Token(t) => self.try_token(t),
                RemoteAuthEvent::Cancelled if self.session.is_none() => self.start_qr_login(),
                other => self.ui.login.on_remote_auth(&self.ctx, other),
            },
            AppEvent::FilesPicked { channel, files } => self.add_files(channel, files),
            AppEvent::Pins { channel, result } => match result {
                Ok(p) => {
                    // Their authors may never have spoken since we connected; without this they're "Unknown User".
                    if let Some(s) = &mut self.session {
                        for m in &p {
                            s.state.cache_author(m);
                        }
                    }
                    self.ui.pins = Some((channel, Some(p)));
                }
                Err(e) => {
                    self.ui.pins = None;
                    self.banner(format!("Couldn't load pins: {e}"));
                }
            },
            AppEvent::Voice { generation, event } => crate::voice_glue::on_event(self, generation, event),
            AppEvent::Stream { generation, event } => crate::stream_glue::on_event(self, generation, event),
            AppEvent::Share { generation, event } => crate::share_glue::on_event(self, generation, event),
            AppEvent::Threads { channel, active, archived } => crate::threads_glue::on_loaded(self, channel, active, archived),
            AppEvent::ThreadOpened(result) => crate::threads_glue::on_opened(self, result),
            AppEvent::ThreadJoinFailed { thread, joined, error } => crate::threads_glue::on_join_failed(self, thread, joined, error),
            AppEvent::Gifs { query, result } => {
                if let Some(p) = self.ui.gif_picker.as_mut().filter(|p| p.requested() == Some(query.as_str())) {
                    match result {
                        Ok(gifs) => p.results = Some(gifs),
                        Err(e) => {
                            p.results = Some(Vec::new());
                            self.banner(format!("Couldn't load GIFs: {e}"));
                        }
                    }
                }
            }
            AppEvent::Frecency(result) => match result {
                Ok(b64) => self.set_frecency(b64),
                Err(e) => tracing::warn!("couldn't load favourite GIFs: {e}"),
            },
            AppEvent::FrecencyRejected { previous, error } => {
                if let Some(p) = previous {
                    self.set_frecency(p);
                }
                self.banner(format!("Couldn't update favourites: {error}"));
            }
            AppEvent::SearchResults { generation, result } => crate::search_glue::on_results(self, generation, result),
            AppEvent::Mentions { generation, result } => crate::inbox_glue::on_loaded(self, generation, result),
            AppEvent::ProfileImagePicked { server, kind, result } => {
                let ctx = self.ctx().clone();
                let Some(ed) = &mut self.ui.profile_editor else { return };
                match result {
                    Ok(Some((data_uri, image))) => {
                        let preview = ctx.load_texture("profile-image-preview", image, egui::TextureOptions::LINEAR);
                        ui::profile_edit::set_image(ed, server, kind, data_uri, preview);
                        ed.error = None;
                    }
                    Ok(None) => {}
                    Err(e) => ed.error = Some(e),
                }
            }
            AppEvent::ProfileSaved { done, error } => self.on_profile_saved(done, error),
            AppEvent::InteractionSent { nonce, result } => crate::interactions_glue::on_sent(self, nonce, result),
            AppEvent::Commands { key, result } => {
                let loaded = match result {
                    Ok(index) => ui::commands::Loaded::Ready(index),
                    Err(e) => {
                        tracing::info!("slash commands for {key}: {e}");
                        ui::commands::Loaded::Failed
                    }
                };
                self.ui.commands.insert(key, loaded);
            }
            AppEvent::Profile { user, guild, result } => {
                if let Err(e) = &result {
                    tracing::info!("profile of {user}: {e}");
                }
                self.ui.profiles.insert((user, guild), Some(result));
            }
            AppEvent::StreamPreview { key, result } => {
                let url = result.unwrap_or_else(|e| {
                    tracing::debug!("stream preview for {key}: {e}");
                    None
                });
                self.ui.stream_previews.insert(key, (Instant::now(), Some(url)));
            }
            AppEvent::StickerPacks(result) => {
                let packs = result.unwrap_or_else(|e| {
                    tracing::warn!("sticker packs: {e}");
                    Vec::new()
                });
                self.ui.sticker_packs = Some(Some(packs));
            }
            AppEvent::GifCategories(result) => {
                if let Some(p) = &mut self.ui.gif_picker {
                    p.categories = Some(result.unwrap_or_default());
                }
            }
            AppEvent::Folders(folders) => {
                if let (Some(f), Some(s)) = (folders, &mut self.session) {
                    s.state.set_folders(f);
                }
            }
            AppEvent::DmOpened(channel) => {
                let id = channel.id;
                let call = self.pending_call.take().filter(|u| channel.recipients.iter().any(|r| r.id == *u));
                if let Some(s) = &mut self.session {
                    s.state.add_channel(channel);
                }
                self.ui.guild = None;
                self.select_channel(id);
                if call.is_some() {
                    self.apply(Action::JoinVoice { guild: None, channel: id });
                }
            }
        }
    }

    fn handle_gateway(&mut self, out: GatewayOutput) {
        let Some(s) = &mut self.session else { return };
        match out {
            GatewayOutput::State(c) => {
                tracing::info!("gateway: {c:?}");
                s.conn = c;
            }
            GatewayOutput::Fatal { code, reason } => {
                let msg = if code == 4004 { "Your session has expired. Please sign in again.".to_owned() } else { format!("Disconnected: {reason} ({code})") };
                self.end_session(false);
                self.ui.login.error = Some(msg);
                self.start_qr_login();
            }
            GatewayOutput::Event(event) => {
                self.perf.event(&event);
                let is_ready = matches!(event, swiftcord_core::events::Event::Ready(_));
                let members_chunk = matches!(event, swiftcord_core::events::Event::GuildMembersChunk(_));
                // A message pinned or unpinned in the channel whose pins are open: load them again.
                let pins_changed = match (&event, &self.ui.pins) {
                    (swiftcord_core::events::Event::MessageUpdate(u), Some((c, Some(list)))) if u.channel_id == *c => {
                        u.pinned.is_some_and(|p| p != list.iter().any(|m| m.id == u.id))
                    }
                    _ => false,
                };
                crate::inbox_glue::on_event(&mut self.ui.inbox, &s.state, &event);
                let notices = s.state.apply(event);
                if is_ready {
                    tracing::info!("READY: {} channels, me = {:?}", s.state.private_channels().len(), s.state.me.as_ref().map(|m| m.id));
                    s.ready = true;
                    s.subscribed.clear();
                    self.on_ready();
                }
                for n in notices {
                    self.handle_notice(n);
                }
                if members_chunk {
                    crate::search_glue::members_arrived(self);
                }
                if let (true, Some(c)) = (pins_changed, self.ui.pins.as_ref().map(|p| p.0)) {
                    self.ui.pins = None;
                    self.apply(Action::ShowPins(c));
                }
            }
        }
    }

    /// Asks for the members behind a server channel's loaded messages (authors and people mentioned) that we
    /// don't have yet: history from the API comes without them, and they hold the server profile (nickname
    /// and server avatar) and roles.
    fn request_authors(&mut self, channel: Snowflake) {
        let Some(s) = &self.session else { return };
        let Some(guild) = s.state.channel(channel).and_then(|c| c.guild_id) else { return };
        let Some(messages) = s.state.messages(channel) else { return };
        let mut wanted = Vec::new();
        for m in messages.list.values() {
            // The message, and the one it replies to (shown above it).
            let people = std::iter::once(m).chain(m.referenced_message.as_deref()).flat_map(|m| {
                let author = (!m.author.bot).then_some(m.author.id);
                author.into_iter().chain(m.mentions.iter().map(|u| u.id))
            });
            for id in people {
                if s.state.member(guild, id).is_none() && self.members_requested.insert((guild, id)) {
                    wanted.push(id);
                }
            }
        }
        if !wanted.is_empty() {
            tracing::debug!("asking for {} members in {guild}", wanted.len());
            s.gateway.request_members(guild, &wanted);
        }
    }

    fn on_ready(&mut self) {
        self.members_requested.clear();
        // A new connection starts out present; still away, say so again.
        if self.away_since.is_some() {
            self.send_presence();
        }
        let Some(s) = &mut self.session else { return };
        if crate::demo::enabled() {
            crate::demo::install(&mut s.state);
            // The demo bot's camera, for checking call layouts without a webcam.
            self.ui.cameras.insert(Snowflake(2), self.ctx.load_texture("demo-camera", crate::demo::camera_picture(), egui::TextureOptions::LINEAR));
        }
        // The selection may point at something that no longer exists after a fresh READY.
        if let Some(g) = self.ui.guild {
            if s.state.guild(g).is_none() {
                self.ui.guild = None;
            }
        }
        if let Some(g) = self.ui.guild {
            s.gateway.subscribe_guild(g);
            s.subscribed.insert(g);
        }
        // A fresh session has no member list subscriptions; the list asks again when next drawn.
        self.ui.member_sub = None;
        // READY with legacy capabilities may not include the protobuf settings that hold server folders.
        let rest = s.rest.clone();
        self.spawn(async move {
            let folders = rest.settings_proto().await.ok().and_then(|p| swiftcord_core::proto::read_guild_folders(&p));
            AppEvent::Folders(folders)
        });
        self.load_frecency();
    }

    fn handle_notice(&mut self, notice: Notice) {
        let focused = self.ctx.input(|i| i.viewport().focused.unwrap_or(true));
        let current = self.ui.channel();
        let Some(s) = &mut self.session else { return };
        // Do not disturb silences desktop notifications, as in Discord.
        let dnd = s.state.own_status() == swiftcord_core::models::PresenceStatus::Dnd;
        match notice {
            Notice::MessageCreated { message, notify } => {
                let _ = s;
                if current == Some(message.channel_id) && focused && self.ui.kept_unread != current {
                    self.queue_ack(message.channel_id, message.id);
                } else if notify && !dnd {
                    let now = Instant::now();
                    if self.settings.notifications {
                        self.toast(&message, now);
                    }
                    // A burst of messages plays the sound once (see `notifications`).
                    if self.settings.message_sounds && !self.ui.self_deaf && self.notifications.sound(now) {
                        self.sounds.play(crate::sounds::Sound::Message, &self.settings.output_device);
                    }
                }
            }
            Notice::Ring(channel) => {
                let caller = s.state.channel(channel).map(|c| c.label()).unwrap_or_default();
                platform::notify(&format!("call-{}", channel.0), format!("{caller} is calling you"), "Open '08cord to answer.".into(), channel.0);
            }
            Notice::RingStopped(_) => {}
            Notice::FrecencyChanged => self.load_frecency(),
            Notice::OwnStatusChanged => {
                // Chosen on another device: this session follows.
                self.send_presence();
            }
            Notice::OwnVoiceState | Notice::VoiceServer(_) | Notice::Other(..) => {
                crate::voice_glue::on_notice(self, notice);
            }
        }
    }

    /// After saving profile changes: our account and server member come back updated, so they're applied
    /// right away, and our cached profiles are dropped so cards show the new ones.
    fn on_profile_saved(&mut self, done: Vec<(String, serde_json::Value)>, error: Option<String>) {
        if let Some(s) = &mut self.session {
            for (path, v) in &done {
                if path == "users/@me" {
                    if let Ok(user) = serde_json::from_value::<swiftcord_core::models::User>(v.clone()) {
                        let premium = s.state.me.as_ref().and_then(|m| m.premium_type);
                        s.state.me = Some(swiftcord_core::models::User { premium_type: user.premium_type.or(premium), ..user });
                    }
                } else if let Some(g) = path.strip_prefix("guilds/").and_then(|p| p.strip_suffix("/members/@me")).and_then(|g| g.parse().ok()) {
                    if let Ok(member) = serde_json::from_value(v.clone()) {
                        s.state.set_my_member(Snowflake(g), member);
                    }
                }
            }
            if let Some(me) = s.state.me.as_ref().map(|m| m.id) {
                self.ui.profiles.retain(|(u, _), _| *u != me);
            }
        }
        match error {
            None => self.ui.profile_editor = None,
            Some(e) => {
                if let Some(ed) = &mut self.ui.profile_editor {
                    ed.busy = false;
                    let saved = if done.is_empty() { String::new() } else { " (some changes were saved)".to_owned() };
                    ed.error = Some(format!("Couldn't save: {e}{saved}"));
                }
            }
        }
    }

    /// Fetches someone's profile (as in `guild`) unless it's loaded or loading. Kept for the session; failures
    /// are retried next time. Bots have profiles too; webhook authors aren't users, which the request reports.
    fn fetch_profile(&mut self, user: Snowflake, guild: Option<Snowflake>) {
        let Some(s) = &self.session else { return };
        if matches!(self.ui.profiles.get(&(user, guild)), Some(Some(Ok(_))) | Some(None)) {
            return;
        }
        self.ui.profiles.insert((user, guild), None);
        let rest = s.rest.clone();
        self.spawn(async move {
            let result = rest.user_profile(user, guild).await.map_err(|e| e.to_string());
            AppEvent::Profile { user, guild, result }
        });
    }

    /// Rings while a call is coming in, and plays the ringback while a DM call we're in rings the others.
    /// Not while deafened or on Do Not Disturb, as in Discord.
    fn ring_sounds(&mut self) {
        use crate::sounds::Sound;
        let (mut ring, mut ringback) = (false, false);
        if let Some(s) = &self.session {
            let me = s.state.me.as_ref().map(|m| m.id);
            let in_call = self.voice.as_ref().map(|c| c.channel);
            let quiet = !self.settings.call_sounds || self.ui.self_deaf || s.state.own_status() == swiftcord_core::models::PresenceStatus::Dnd;
            ring = !quiet && s.state.ringing_me().into_iter().any(|c| Some(c) != in_call);
            let connected = matches!(self.voice.as_ref().map(|c| c.status), Some(crate::voice_glue::Status::Connected { .. }));
            ringback = !quiet && connected && in_call.and_then(|c| s.state.call(c)).is_some_and(|c| c.ringing.iter().any(|u| Some(*u) != me));
        }
        let output = self.settings.output_device.clone();
        self.sounds.set_looping(Sound::Ring, ring, &output);
        self.sounds.set_looping(Sound::Ringback, ringback, &output);
        self.sounds.tick();
    }

    /// The message's channel's toast: popped up, or updated in place during a burst.
    fn toast(&mut self, m: &Message, now: Instant) {
        let Some(s) = &self.session else { return };
        let author = s.state.display_name(m.author.id, m.guild_id);
        let title = match s.state.channel(m.channel_id) {
            Some(c) if c.is_private() => author,
            Some(c) => {
                let guild = c.guild_id.and_then(|g| s.state.guild(g)).map(|g| g.name.as_str()).unwrap_or("");
                format!("{author} (#{}, {guild})", c.label())
            }
            None => author,
        };
        let mut body = m.content.chars().take(200).collect::<String>();
        if body.is_empty() && !m.attachments.is_empty() {
            body = format!("Sent {} attachment(s)", m.attachments.len());
        }
        let tag = crate::notifications::tag(m.channel_id);
        let count = |n: u32| if n > 1 { format!("{title} · {n} new messages") } else { title.clone() };
        match self.notifications.message(m.channel_id, m.id, now) {
            crate::notifications::Toast::Pop { count: n } => platform::toast::show(tag, count(n), body, true, m.channel_id.0),
            crate::notifications::Toast::Update { count: n } => platform::toast::update(tag, count(n), body, n, m.channel_id.0),
        }
    }

    /// Opens the unread channel after (`step` 1) or before (-1) the open one, in the order they're listed,
    /// going round past the end. Nothing happens if there are none.
    fn jump_to_unread(&mut self, step: isize) {
        let Some(s) = &self.session else { return };
        let order = s.state.listed_channels();
        let n = order.len() as isize;
        let current = self.ui.channel();
        // From the open channel's place; with none, from just before the start (or after the end).
        let start = current.and_then(|c| order.iter().position(|(id, _)| *id == c)).map_or(if step > 0 { -1 } else { n }, |i| i as isize);
        let next = (1..=n).map(|k| order[(start + step * k).rem_euclid(n.max(1)) as usize]).find(|&(id, unread)| unread && Some(id) != current);
        if let Some((channel, _)) = next {
            self.apply(Action::SelectChannel(channel));
        }
    }

    /// The session's presence: the chosen status, and whether the user is away (see `check_away`).
    fn send_presence(&self) {
        if let Some(s) = self.session.as_ref().filter(|s| s.ready) {
            s.gateway.update_presence(s.state.chosen_status(), s.state.own_custom_status(), self.away_since);
        }
    }

    /// Tells Discord when the user has been away from the computer for a while, and when they're back, as
    /// the desktop client does: away, Online shows as Idle, and Discord sends notifications to the phone.
    fn check_away(&mut self, ctx: &egui::Context) {
        const AWAY_AFTER: Duration = Duration::from_secs(10 * 60);
        const CHECK_EVERY: Duration = Duration::from_secs(15);
        // Checked even while nothing else happens (hidden in the tray, say).
        ctx.request_repaint_after(CHECK_EVERY);
        if self.away_checked.elapsed() < CHECK_EVERY {
            return;
        }
        self.away_checked = Instant::now();
        let idle = platform::idle_time();
        let away = idle >= AWAY_AFTER;
        if away == self.away_since.is_some() {
            return;
        }
        self.away_since = away.then(|| {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
            now.saturating_sub(idle).as_millis() as u64
        });
        tracing::info!("{}", if away { "away from the computer" } else { "back at the computer" });
        self.send_presence();
    }

    /// Unread mentions on the taskbar button and the tray icon, as Discord shows them.
    fn update_badges(&mut self, frame: &crate::window::Frame) {
        let mentions = self.session.as_ref().map_or(0, |s| s.state.total_mentions());
        // The taskbar button is new each time the window comes back from the tray, without its overlay.
        let state = (mentions, frame.is_visible());
        if self.badge == Some(state) {
            return;
        }
        self.badge = Some(state);
        if let Some(hwnd) = frame.hwnd() {
            platform::badge::set_taskbar(hwnd, mentions);
        }
        platform::tray::set_mentions(mentions);
    }

    /// Takes away the toasts of channels that have been read since (here or elsewhere), or that the window is
    /// focused on, as Discord does.
    fn clear_read_toasts(&mut self, ctx: &egui::Context) {
        let Some(s) = &self.session else { return };
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        let current = self.ui.channel();
        let read = |c: Snowflake, latest: Snowflake| (focused && current == Some(c)) || s.state.last_read(c).is_some_and(|r| r >= latest);
        for channel in self.notifications.take_read(read) {
            platform::toast::remove(crate::notifications::tag(channel));
        }
    }

    fn queue_ack(&mut self, channel: Snowflake, message: Snowflake) {
        let Some(s) = &mut self.session else { return };
        s.state.mark_read(channel, message);
        if s.last_ack.get(&channel).is_some_and(|&l| l >= message) {
            return;
        }
        s.last_ack.insert(channel, message);
        let rest = s.rest.clone();
        self.rt.spawn(async move {
            let _ = rest.ack(channel, message).await;
        });
    }

    fn set_frecency(&mut self, b64: String) {
        let favs = swiftcord_core::proto::read_favorite_gifs(&b64);
        // Only allow editing if a no-op edit of these exact settings round-trips cleanly; otherwise saving
        // could damage settings we don't understand.
        let safe = favs.is_some() && swiftcord_core::proto::edit_round_trips(&b64);
        if !safe {
            tracing::warn!("favourite GIFs are read-only: the settings didn't round-trip");
        }
        if let Some(favs) = favs {
            self.ui.favorite_gifs = favs;
        }
        // The same settings say which reactions are used most, for the one-click reactions.
        if let Some(frequent) = swiftcord_core::proto::read_reaction_frecency(&b64) {
            self.emoji.set_frequent(frequent);
        }
        self.frecency = safe.then_some(b64);
    }

    fn load_frecency(&self) {
        let Some(s) = &self.session else { return };
        let rest = s.rest.clone();
        self.spawn(async move { AppEvent::Frecency(rest.frecency_settings().await.map_err(|e| e.to_string())) });
    }

    // MARK: Actions

    fn add_files(&mut self, channel: Snowflake, files: Vec<FileUpload>) {
        let draft = self.ui.drafts.entry(channel).or_default();
        for f in files {
            if f.bytes.len() > MAX_UPLOAD_BYTES {
                self.ui.banner = Some((format!("{} is larger than 10 MB", f.name), Instant::now()));
            } else if draft.files.len() < 10 {
                draft.files.push(f);
            }
        }
        self.ui.focus_composer = true;
    }

    fn select_channel(&mut self, channel: Snowflake) {
        self.ui.channel_for.insert(self.ui.guild.unwrap_or_default(), channel);
        if let Some(s) = self.session.as_ref().filter(|s| s.ready) {
            if s.state.channel(channel).is_some_and(|c| c.is_private()) {
                // Learn whether a call is going on in this DM.
                s.gateway.request_call(channel);
            }
        }
        self.ui.focus_composer = true;
        self.ui.emoji_picker = None;
        // Jumping to a pinned message keeps the pins open.
        if !matches!(self.ui.pins, Some((c, _)) if c == channel) {
            self.ui.pins = None;
        }
        self.ui.threads_panel = None;
        // Forums have posts (threads), not messages.
        let ready = self.session.as_ref().is_some_and(|s| s.ready);
        if self.session.as_ref().and_then(|s| s.state.channel(channel)).is_some_and(|c| c.is_forum()) {
            if ready {
                crate::threads_glue::load(self, channel, false);
            }
            return;
        }
        self.ui.kept_unread = None;
        self.ensure_history(channel);
        // Remember where we'd read up to before acknowledging, for the NEW line. Leaving a channel forgets it.
        self.ui.unread_marker.retain(|c, _| *c == channel);
        if let Some(read) = self.session.as_ref().filter(|s| s.state.is_unread(channel)).and_then(|s| s.state.last_read(channel)) {
            self.ui.unread_marker.insert(channel, read);
            self.ui.chat.entry(channel).or_default().show_unread();
        }
        self.mark_viewed(channel);
    }

    fn mark_viewed(&mut self, channel: Snowflake) {
        // Not before READY: the saved read states may be stale, and acknowledging needs the connection.
        let Some(s) = self.session.as_ref().filter(|s| s.ready) else { return };
        if let Some(last) = s.state.channel(channel).and_then(|c| c.last_message_id) {
            if s.state.is_unread(channel) || s.state.mention_count(channel) > 0 {
                self.queue_ack(channel, last);
            }
        }
    }

    pub fn ensure_history(&mut self, channel: Snowflake) {
        let Some(s) = &self.session else { return };
        if !s.ready || self.ui.loading.contains(&channel) || s.state.messages(channel).is_some_and(|m| m.loaded) {
            return;
        }
        if self.history_failed.get(&channel).is_some_and(|t| t.elapsed() < Duration::from_secs(15)) {
            return;
        }
        self.fetch_history(channel, None);
    }

    fn fetch_history(&mut self, channel: Snowflake, before: Option<Snowflake>) {
        let Some(s) = &self.session else { return };
        tracing::info!("fetching history for {channel} before {before:?}");
        self.ui.loading.insert(channel);
        let rest = s.rest.clone();
        self.spawn(async move {
            let result = rest.messages(channel, before, HISTORY_PAGE).await.map_err(|e| e.to_string());
            AppEvent::History { channel, result }
        });
    }

    fn send(&mut self, channel: Snowflake) {
        let Some(s) = &mut self.session else { return };
        let draft = self.ui.drafts.entry(channel).or_default();
        if draft.text.trim().is_empty() && draft.files.is_empty() {
            return;
        }
        // "@alice" picked from suggestions (or typed in full) becomes a real mention.
        let content = match s.state.channel(channel) {
            Some(ch) => swiftcord_core::mentions::finalize(draft.text.trim(), &draft.mentions, |name| {
                s.state
                    .search_people(ch, name, 10)
                    .into_iter()
                    .find(|u| u.username.eq_ignore_ascii_case(name))
                    .map(|u| format!("<@{}>", u.id))
            }),
            None => draft.text.trim().to_owned(),
        };
        // ":wave:" typed out in full becomes the emoji, as in Discord; custom ones from this server.
        let guild = s.state.channel(channel).and_then(|c| c.guild_id).and_then(|g| s.state.guild(g));
        let content = crate::ui::emoji::expand_shortcodes(&content, &self.emoji, |name| {
            let e = guild?.emojis.iter().find(|e| e.name.as_deref() == Some(name))?;
            Some(format!("<{}:{name}:{}>", if e.animated { "a" } else { "" }, e.id?))
        });
        draft.mentions.clear();
        draft.dismissed = None;
        let files = std::mem::take(&mut draft.files);
        let reply = draft.reply.take();
        draft.text.clear();
        self.post(channel, content, files, reply);
    }

    /// Sends a message, showing it straight away as pending.
    fn post(&mut self, channel: Snowflake, content: String, files: Vec<FileUpload>, reply: Option<(Snowflake, bool)>) {
        self.post_with(channel, content, files, reply, None);
    }

    fn post_with(&mut self, channel: Snowflake, content: String, files: Vec<FileUpload>, reply: Option<(Snowflake, bool)>, sticker: Option<swiftcord_core::models::StickerItem>) {
        let Some(s) = &mut self.session else { return };
        let Some(me) = s.state.me.clone() else { return };
        let nonce = nonce_now();
        let mut msg = NewMessage { content: content.clone(), nonce: Some(nonce.to_string()), ..Default::default() };
        msg.sticker_ids = sticker.iter().map(|st| st.id).collect();
        if let Some((id, ping)) = reply {
            msg.reply_to(id, channel, ping);
        }
        let guild_id = s.state.channel(channel).and_then(|c| c.guild_id);
        let referenced = reply.and_then(|(id, _)| s.state.message(channel, id).cloned()).map(Box::new);
        s.state.add_pending(Message {
            id: nonce,
            channel_id: channel,
            guild_id,
            member: guild_id.and_then(|g| s.state.member(g, me.id).cloned()),
            author: me,
            content,
            timestamp: Some(chrono::Utc::now()),
            kind: if reply.is_some() { 19 } else { 0 },
            referenced_message: referenced,
            attachments: files
                .iter()
                .map(|f| swiftcord_core::models::Attachment { filename: f.name.clone(), size: f.bytes.len() as u64, ..Default::default() })
                .collect(),
            sticker_items: sticker.into_iter().collect(),
            pending: Some(PendingState::Sending),
            ..Default::default()
        });
        s.last_typing.remove(&channel);
        if let Some(v) = self.ui.chat.get_mut(&channel) {
            v.scroll_to_bottom();
        }
        self.dispatch_send(channel, nonce, msg, files);
    }

    fn dispatch_send(&mut self, channel: Snowflake, nonce: Snowflake, msg: NewMessage, files: Vec<FileUpload>) {
        let Some(s) = &self.session else { return };
        let rest = s.rest.clone();
        // Keep a copy for retrying; file bytes are shared cheaply enough at these sizes.
        let keep_files = files.iter().map(|f| FileUpload { name: f.name.clone(), bytes: f.bytes.clone(), content_type: f.content_type.clone() }).collect();
        let keep_msg = NewMessage { content: msg.content.clone(), nonce: msg.nonce.clone(), message_reference: msg.message_reference.clone(), allowed_mentions: msg.allowed_mentions.clone(), attachments: None, sticker_ids: msg.sticker_ids.clone() };
        self.outgoing.insert(nonce, Outgoing { msg: keep_msg, files: keep_files });
        self.spawn(async move {
            let result = rest.send_message(channel, msg, files).await.map_err(|e| e.to_string());
            AppEvent::Sent { channel, nonce, result }
        });
    }

    pub(crate) fn apply(&mut self, action: Action) {
        // Showing the saved session, not yet connected: what needs Discord does nothing.
        if self.session.as_ref().is_some_and(|s| !s.ready) && !works_offline(&action) {
            return;
        }
        match action {
            Action::SelectGuild(g) => {
                self.ui.guild = g;
                self.ui.emoji_picker = None;
                self.ui.pins = None;
                if let (Some(g), Some(s)) = (g, self.session.as_mut().filter(|s| s.ready)) {
                    if s.subscribed.insert(g) {
                        s.gateway.subscribe_guild(g);
                    }
                }
                // Restore the last channel, or pick the first readable one.
                let channel = self.ui.channel().or_else(|| {
                    let s = self.session.as_ref()?;
                    match g {
                        Some(g) => s.state.channel_tree(g).iter().flat_map(|(_, c)| c.iter()).find(|c| c.is_text_based() && !c.is_voice()).map(|c| c.id),
                        None => s.state.private_channels().first().map(|c| c.id),
                    }
                });
                if let Some(c) = channel {
                    self.select_channel(c);
                }
            }
            Action::SelectChannel(c) => {
                // A channel mention can point at another server.
                let guild = self.session.as_ref().and_then(|s| s.state.channel(c)).map(|ch| ch.guild_id);
                if let Some(g) = guild.filter(|g| *g != self.ui.guild) {
                    self.ui.guild = g;
                    if let (Some(g), Some(s)) = (g, self.session.as_mut().filter(|s| s.ready)) {
                        if s.subscribed.insert(g) {
                            s.gateway.subscribe_guild(g);
                        }
                    }
                }
                self.select_channel(c);
            }
            Action::AddFiles { channel, files } => self.add_files(channel, files),
            Action::PasteImage { channel, image } => {
                self.spawn(async move {
                    match tokio::task::spawn_blocking(move || pasted_png(&image)).await.map_err(|e| e.to_string()).and_then(|r| r) {
                        Ok(bytes) => AppEvent::FilesPicked { channel, files: vec![FileUpload { name: "image.png".into(), bytes, content_type: Some("image/png".into()) }] },
                        Err(e) => AppEvent::Error(format!("Couldn't paste the image: {e}")),
                    }
                });
            }
            Action::Notice(text) => self.banner(text),
            Action::OpenGifPicker { channel, at } => {
                self.ui.emoji_picker = None;
                self.ui.sticker_picker = None;
                self.ui.gif_picker = Some(ui::gif::GifPicker::new(channel, at));
            }
            Action::GifSearch(query) => {
                let Some(s) = &self.session else { return };
                let rest = s.rest.clone();
                self.spawn(async move {
                    let result = if query.is_empty() { rest.gif_trending().await } else { rest.gif_search(&query).await };
                    AppEvent::Gifs { query, result: result.map_err(|e| e.to_string()) }
                });
            }
            Action::GifCategories => {
                let Some(s) = &self.session else { return };
                let rest = s.rest.clone();
                self.spawn(async move { AppEvent::GifCategories(rest.gif_categories().await.map_err(|e| e.to_string())) });
            }
            Action::SendGif { channel, url } => {
                self.apply(Action::CloseGifPicker);
                let reply = self.ui.drafts.entry(channel).or_default().reply.take();
                self.post(channel, url, Vec::new(), reply);
            }
            Action::OpenStickerPicker { channel, at } => {
                self.ui.emoji_picker = None;
                self.apply(Action::CloseGifPicker);
                self.ui.sticker_picker = Some(ui::sticker_picker::StickerPicker::new(channel, at));
            }
            Action::LoadStickerPacks => {
                let Some(s) = &self.session else { return };
                let rest = s.rest.clone();
                self.spawn(async move { AppEvent::StickerPacks(rest.sticker_packs().await.map_err(|e| e.to_string())) });
            }
            Action::SendSticker { channel, sticker } => {
                // Sent on its own, keeping whatever's being typed, but as the reply if one is set up.
                let reply = self.ui.drafts.entry(channel).or_default().reply.take();
                self.post_with(channel, String::new(), Vec::new(), reply, Some(sticker));
            }
            Action::ToggleFavoriteGif(gif) => {
                // Without the current settings we'd overwrite everything else in them, so don't try.
                let Some(current) = self.frecency.clone() else {
                    self.banner("Favourites haven't loaded yet.");
                    return;
                };
                let exists = self.ui.favorite_gifs.iter().any(|g| g.url == gif.url);
                let order = self.ui.favorite_gifs.iter().map(|g| g.order).max().unwrap_or(0) + 1;
                let added = swiftcord_core::proto::FavoriteGif { order, ..gif.clone() };
                let Some(updated) = swiftcord_core::proto::edit_favorite_gifs(&current, &gif.url, (!exists).then_some(&added)) else {
                    self.banner("Couldn't read your favourites.");
                    return;
                };
                // Show the change now; put it back if Discord refuses it.
                self.set_frecency(updated.clone());
                let Some(s) = &self.session else { return };
                let rest = s.rest.clone();
                self.spawn(async move {
                    match rest.set_frecency_settings(&updated).await {
                        Ok(saved) => AppEvent::Frecency(Ok(saved)),
                        Err(e) => AppEvent::FrecencyRejected { previous: Some(current), error: e.to_string() },
                    }
                });
            }
            Action::CloseGifPicker => {
                // Animated GIFs are heavy once decoded; let them go.
                if let Some(p) = self.ui.gif_picker.take() {
                    for uri in p.animated {
                        self.ctx.forget_image(&uri);
                    }
                }
            }
            Action::SearchMembers { guild, query } => {
                if let Some(s) = &self.session {
                    s.gateway.search_members(guild, &query, 10);
                }
            }
            Action::LoadOlder(c) => {
                let Some(s) = &self.session else { return };
                let Some(list) = s.state.messages(c) else { return };
                if self.ui.loading.contains(&c) || list.reached_start {
                    return;
                }
                let oldest = list.list.keys().find(|id| list.list[id].pending.is_none()).copied();
                self.fetch_history(c, oldest);
            }
            Action::Send(c) => {
                // Talking in a channel means you've caught up with it.
                self.ui.unread_marker.remove(&c);
                self.ui.kept_unread = self.ui.kept_unread.filter(|k| *k != c);
                self.send(c);
            }
            Action::SaveEdit { channel, message, content } => {
                self.ui.editing = None;
                let Some(s) = &mut self.session else { return };
                if let Some(m) = s.state.message_mut(channel, message) {
                    if m.content == content {
                        return;
                    }
                    m.content = content.clone();
                }
                let rest = s.rest.clone();
                self.spawn(async move {
                    match rest.edit_message(channel, message, &content).await {
                        Ok(_) => AppEvent::Error(String::new()),
                        Err(e) => AppEvent::Error(format!("Couldn't edit: {e}")),
                    }
                });
            }
            Action::Delete { channel, message } => {
                let Some(s) = &self.session else { return };
                let rest = s.rest.clone();
                self.spawn(async move {
                    match rest.delete_message(channel, message).await {
                        Ok(_) => AppEvent::Error(String::new()),
                        Err(e) => AppEvent::Error(format!("Couldn't delete: {e}")),
                    }
                });
            }
            Action::React { channel, message, emoji, add } => {
                let Some(s) = &mut self.session else { return };
                s.state.optimistic_reaction(channel, message, &emoji, add);
                if add {
                    self.emoji.note_used(&emoji);
                }
                let rest = s.rest.clone();
                self.spawn(async move {
                    let r = if add { rest.add_reaction(channel, message, &emoji).await } else { rest.remove_reaction(channel, message, &emoji).await };
                    AppEvent::Error(r.err().map(|e| format!("Couldn't react: {e}")).unwrap_or_default())
                });
            }
            Action::Retry { channel, nonce } => {
                if let Some(o) = self.outgoing.remove(&nonce) {
                    if let Some(m) = self.session.as_mut().and_then(|s| s.state.message_mut(channel, nonce)) {
                        m.pending = Some(PendingState::Sending);
                    }
                    self.dispatch_send(channel, nonce, o.msg, o.files);
                }
            }
            Action::Discard { channel, nonce } => {
                self.outgoing.remove(&nonce);
                if let Some(s) = &mut self.session {
                    s.state.remove_message(channel, nonce);
                }
            }
            Action::Typing(c) => {
                let Some(s) = &mut self.session else { return };
                if s.last_typing.get(&c).is_some_and(|t| t.elapsed() < TYPING_INTERVAL) {
                    return;
                }
                s.last_typing.insert(c, Instant::now());
                let rest = s.rest.clone();
                self.rt.spawn(async move {
                    let _ = rest.typing(c).await;
                });
            }
            Action::EditProfile { guild, server } => {
                let Some(s) = &self.session else { return };
                let Some(me) = s.state.me.as_ref().map(|m| m.id) else { return };
                self.ui.profile_editor = Some(ui::profile_edit::Editor::new(s, guild, server));
                // The current bio, pronouns, colours and banners come from our own profile.
                self.fetch_profile(me, None);
                if let Some(g) = guild {
                    self.fetch_profile(me, Some(g));
                }
            }
            Action::PickProfileImage { server, kind } => {
                let title = match kind {
                    ui::profile_edit::ImageKind::Avatar => "Choose an avatar",
                    ui::profile_edit::ImageKind::Banner => "Choose a banner",
                };
                self.spawn(async move {
                    let picked = rfd::AsyncFileDialog::new().set_title(title).add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp"]).pick_file().await;
                    let Some(file) = picked else { return AppEvent::ProfileImagePicked { server, kind, result: Ok(None) } };
                    let bytes = file.read().await;
                    let result = tokio::task::spawn_blocking(move || ui::profile_edit::prepare_image(&bytes, kind)).await.map_err(|e| e.to_string()).and_then(|r| r);
                    AppEvent::ProfileImagePicked { server, kind, result: result.map(Some) }
                });
            }
            Action::SaveProfile => {
                let Some(s) = &self.session else { return };
                let Some(ed) = &mut self.ui.profile_editor else { return };
                ed.busy = true;
                ed.error = None;
                let requests = ed.requests();
                let rest = s.rest.clone();
                self.spawn(async move {
                    // One after another, stopping at the first failure (what's saved so far stays saved).
                    let mut done = Vec::new();
                    for (path, body) in requests {
                        match rest.patch_me(&path, body).await {
                            Ok(v) => done.push((path, v)),
                            Err(e) => return AppEvent::ProfileSaved { done, error: Some(e.to_string()) },
                        }
                    }
                    AppEvent::ProfileSaved { done, error: None }
                });
            }
            Action::CloseProfileEditor => self.ui.profile_editor = None,
            Action::UseComponent { msg, component, values } => crate::interactions_glue::use_component(self, &msg, &component, values),
            Action::SubmitModal => crate::interactions_glue::submit_modal(self),
            Action::CloseModal => self.ui.interactions.modal = None,
            Action::LoadCommands { guild, channel } => {
                let Some(s) = &self.session else { return };
                let key = guild.unwrap_or(channel);
                if self.ui.commands.contains_key(&key) {
                    return;
                }
                self.ui.commands.insert(key, ui::commands::Loaded::Loading);
                let rest = s.rest.clone();
                self.spawn(async move {
                    let result = rest.command_index(guild, channel).await.map_err(|e| e.to_string());
                    AppEvent::Commands { key, result }
                });
            }
            Action::RunCommand(channel) => crate::interactions_glue::run_command(self, channel),
            Action::PickFiles(channel) => {
                self.spawn(async move {
                    let picked = rfd::AsyncFileDialog::new().set_title("Upload files").pick_files().await.unwrap_or_default();
                    let mut files = Vec::new();
                    for f in picked {
                        files.push(FileUpload { name: f.file_name(), bytes: f.read().await, content_type: None });
                    }
                    AppEvent::FilesPicked { channel, files }
                });
            }
            Action::OpenDm(user) => {
                let Some(s) = &self.session else { return };
                if let Some(c) = s.state.private_channels().iter().find(|c| c.kind == swiftcord_core::models::ChannelType::Dm && c.recipients.iter().any(|r| r.id == user)) {
                    let id = c.id;
                    self.ui.guild = None;
                    self.select_channel(id);
                    return;
                }
                let rest = s.rest.clone();
                self.spawn(async move {
                    match rest.create_dm(user).await {
                        Ok(c) => AppEvent::DmOpened(c),
                        Err(e) => AppEvent::Error(format!("Couldn't open DM: {e}")),
                    }
                });
            }
            Action::OpenProfile { user, guild, at } => {
                if self.session.is_none() {
                    return;
                }
                self.ui.profile_card = Some(ui::profile::Card { user, guild, at });
                self.fetch_profile(user, guild);
            }
            Action::CloseProfile => self.ui.profile_card = None,
            Action::SetStatus(status) => {
                let Some(s) = &mut self.session else { return };
                // The gateway update shows it right away; the settings write keeps it across sessions and devices.
                s.gateway.update_presence(status, s.state.own_custom_status(), self.away_since);
                s.state.set_own_status(status);
                let rest = s.rest.clone();
                self.spawn(async move {
                    match rest.set_status(status).await {
                        Ok(()) => AppEvent::Error(String::new()),
                        Err(e) => AppEvent::Error(format!("Couldn't save your status: {e}")),
                    }
                });
            }
            Action::SetUserVolume { user, volume, save } => {
                let volume = volume.clamp(0.0, 2.0);
                self.ui.user_volumes.insert(user, volume);
                self.settings.user_volumes.insert(user.0, volume);
                if let Some(v) = self.voice.as_ref().and_then(|c| c.voice()) {
                    v.set_user_volume(user.0, self.settings.user_gain(user.0));
                }
                if save {
                    self.settings.save();
                }
            }
            Action::ToggleLocalMute(user) => {
                if !self.ui.muted_users.remove(&user) {
                    self.ui.muted_users.insert(user);
                }
                if !self.settings.muted_users.remove(&user.0) {
                    self.settings.muted_users.insert(user.0);
                }
                if let Some(v) = self.voice.as_ref().and_then(|c| c.voice()) {
                    v.set_user_volume(user.0, self.settings.user_gain(user.0));
                }
                self.settings.save();
            }
            Action::CallUser(user) => {
                let Some(s) = &self.session else { return };
                let dm = s.state.private_channels().iter().find(|c| c.kind == swiftcord_core::models::ChannelType::Dm && c.recipients.iter().any(|r| r.id == user)).map(|c| c.id);
                match dm {
                    Some(channel) => {
                        self.ui.guild = None;
                        self.select_channel(channel);
                        self.apply(Action::JoinVoice { guild: None, channel });
                    }
                    None => {
                        // The call starts when the new DM opens.
                        self.pending_call = Some(user);
                        self.apply(Action::OpenDm(user));
                    }
                }
            }
            Action::PlayWeb(player) => self.ui.web_player = player,
            Action::Forward { channel, message, to, note } => {
                let Some(s) = &self.session else { return };
                let guild = s.state.channel(channel).and_then(|c| c.guild_id);
                let rest = s.rest.clone();
                self.spawn(async move {
                    let mut failed = Vec::new();
                    for dest in to {
                        // The forward itself carries nothing but the reference; a note follows it as its own message.
                        let reference = serde_json::json!({ "type": 1, "message_id": message, "channel_id": channel, "guild_id": guild });
                        let fwd = NewMessage { nonce: Some(nonce_now().to_string()), message_reference: Some(reference), ..Default::default() };
                        match rest.send_message(dest, fwd, Vec::new()).await {
                            Ok(_) if !note.is_empty() => {
                                let msg = NewMessage { content: note.clone(), nonce: Some(nonce_now().to_string()), ..Default::default() };
                                if let Err(e) = rest.send_message(dest, msg, Vec::new()).await {
                                    failed.push(e.to_string());
                                }
                            }
                            Ok(_) => {}
                            Err(e) => failed.push(e.to_string()),
                        }
                    }
                    match failed.first() {
                        None => AppEvent::Error(String::new()),
                        Some(e) => AppEvent::Error(format!("Couldn't forward the message: {e}")),
                    }
                });
            }
            Action::ModerateVoice { guild, user, change } => {
                let Some(s) = &self.session else { return };
                let body = match change {
                    VoiceMod::Mute(on) => serde_json::json!({ "mute": on }),
                    VoiceMod::Deafen(on) => serde_json::json!({ "deaf": on }),
                    VoiceMod::Move(channel) => serde_json::json!({ "channel_id": channel }),
                };
                let rest = s.rest.clone();
                self.spawn(async move {
                    match rest.moderate_voice(guild, user, body).await {
                        Ok(()) => AppEvent::Error(String::new()),
                        Err(e) => AppEvent::Error(format!("Couldn't change their voice state: {e}")),
                    }
                });
            }
            Action::OpenVoiceChat(channel) => {
                self.ui.voice_room_chat = true;
                self.apply(Action::SelectChannel(channel));
            }
            Action::SetShareActivity(show) => {
                let Some(s) = &mut self.session else { return };
                s.state.set_shares_activity(show);
                let rest = s.rest.clone();
                self.spawn(async move {
                    match rest.set_share_activity(show).await {
                        Ok(()) => AppEvent::Error(String::new()),
                        Err(e) => AppEvent::Error(format!("Couldn't save your activity setting: {e}")),
                    }
                });
            }
            Action::SetStreamAudio { volume, when_deafened } => {
                self.settings.stream_volume = volume.clamp(0.0, 2.0);
                self.settings.stream_audio_when_deafened = when_deafened;
                self.settings.save();
                self.ui.stream_audio = (self.settings.stream_volume, when_deafened);
                crate::stream_glue::update_self_state(self);
            }
            Action::ToggleMemberList => {
                self.ui.hide_members = !self.ui.hide_members;
                self.settings.hide_members = self.ui.hide_members;
                self.settings.save();
            }
            Action::MemberListRange { guild, channel, first, last } => {
                // The first block always, as Discord's client does, plus up to two covering what's in view.
                const BLOCK: usize = 100;
                let mut ranges = vec![[0, BLOCK as u32 - 1]];
                let from = (first / BLOCK).max(1);
                for b in (from..from + 2).filter(|b| b * BLOCK <= last) {
                    ranges.push([(b * BLOCK) as u32, (b * BLOCK + BLOCK - 1) as u32]);
                }
                if self.ui.member_sub.as_ref() == Some(&(channel, ranges.clone())) {
                    return;
                }
                let Some(s) = &mut self.session else { return };
                s.gateway.subscribe_member_list(guild, channel, &ranges);
                s.subscribed.insert(guild);
                self.ui.member_sub = Some((channel, ranges));
            }
            Action::ShowThreads(channel) => crate::threads_glue::toggle_panel(self, channel),
            Action::LoadThreads { channel, more } => crate::threads_glue::load(self, channel, more),
            Action::OpenThread(thread) => crate::threads_glue::open(self, thread),
            Action::StartThread { channel, message, name } => crate::threads_glue::open_prompt(self, channel, message, name),
            Action::CloseThreadPrompt => self.ui.thread_prompt = None,
            Action::CreateThread => crate::threads_glue::create(self),
            Action::SetThreadJoined(thread, joined) => crate::threads_glue::set_joined(self, thread, joined),
            Action::ShowPins(channel) => {
                self.ui.search = None;
                if matches!(self.ui.pins, Some((c, _)) if c == channel) {
                    self.ui.pins = None;
                    return;
                }
                let Some(s) = &self.session else { return };
                self.ui.pins = Some((channel, None));
                self.ui.inbox = None;
                self.ui.threads_panel = None;
                let rest = s.rest.clone();
                self.spawn(async move {
                    let result = rest.pins(channel).await.map_err(|e| e.to_string());
                    AppEvent::Pins { channel, result }
                });
            }
            Action::Ack(c) => {
                if self.ui.kept_unread == Some(c) {
                    self.ui.kept_unread = None;
                }
                self.mark_viewed(c);
            }
            Action::MarkUnread { channel, message } => {
                let Some(s) = &mut self.session else { return };
                let (before, mentions) = s.state.mark_unread(channel, message);
                s.last_ack.insert(channel, before);
                self.ui.unread_marker.insert(channel, before);
                if self.ui.channel() == Some(channel) {
                    self.ui.kept_unread = Some(channel);
                }
                let rest = s.rest.clone();
                self.rt.spawn(async move {
                    if let Err(e) = rest.ack_manual(channel, before, mentions).await {
                        tracing::warn!("mark unread: {e}");
                    }
                });
            }
            Action::JoinVoice { guild, channel } => crate::voice_glue::join(self, guild, channel),
            Action::AcceptCall(channel) => {
                self.apply(Action::SelectChannel(channel));
                crate::voice_glue::join(self, None, channel);
            }
            Action::DeclineCall(channel) => {
                let Some(s) = &self.session else { return };
                let Some(me) = s.state.me.as_ref().map(|m| m.id) else { return };
                let rest = s.rest.clone();
                self.spawn(async move {
                    let r = rest.stop_ringing(channel, &[me]).await;
                    AppEvent::Error(r.err().map(|e| format!("Couldn't decline: {e}")).unwrap_or_default())
                });
            }
            Action::LeaveVoice => crate::voice_glue::leave(self),
            Action::WatchStream { guild, channel, user } => crate::stream_glue::watch(self, guild, channel, user),
            Action::Search { channel, text } => crate::search_glue::start(self, channel, text),
            Action::SearchMore => crate::search_glue::more(self),
            Action::CloseSearch => self.ui.search = None,
            Action::ShowInbox => crate::inbox_glue::toggle(self),
            Action::InboxMore => crate::inbox_glue::more(self),
            Action::InboxFilters { guild, everyone, roles } => crate::inbox_glue::set_filters(self, guild, everyone, roles),
            Action::DismissMention(message) => crate::inbox_glue::dismiss(self, message),
            Action::LoadStreamPreview(key) => {
                let Some(s) = &self.session else { return };
                self.ui.stream_previews.insert(key.clone(), (Instant::now(), None));
                let rest = s.rest.clone();
                self.spawn(async move {
                    let result = rest.stream_preview(&key).await.map_err(|e| e.to_string());
                    AppEvent::StreamPreview { key, result }
                });
            }
            Action::StopWatching => crate::stream_glue::stop(self),
            Action::OpenSharePicker => crate::share_glue::open_picker(self),
            Action::CloseSharePicker => self.ui.share_picker = None,
            Action::StartShare => crate::share_glue::start(self),
            Action::StopShare => crate::share_glue::stop(self),
            Action::ShowStream(show) => {
                if let Some(v) = &mut self.ui.stream {
                    v.visible = show;
                    if !show && v.fullscreen {
                        v.fullscreen = false;
                        self.ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                    }
                }
            }
            Action::ToggleFullscreen => {
                if let Some(v) = &mut self.ui.stream {
                    v.fullscreen = !v.fullscreen;
                    self.ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(v.fullscreen));
                }
            }
            Action::ToggleMute => {
                self.ui.self_mute = !self.ui.self_mute;
                if !self.ui.self_mute {
                    self.ui.self_deaf = false;
                }
                crate::voice_glue::update_self_state(self);
            }
            Action::ToggleDeafen => {
                self.ui.self_deaf = !self.ui.self_deaf;
                self.ui.self_mute = self.ui.self_deaf || self.ui.self_mute;
                crate::voice_glue::update_self_state(self);
            }
            Action::ToggleCamera => crate::voice_glue::toggle_camera(self),
            Action::Copy(text) => self.ctx.copy_text(text),
            Action::OpenLink(url) => match swiftcord_core::links::parse(&url) {
                Some(link) => self.apply(Action::JumpTo { channel: link.channel, message: link.message }),
                None => platform::open_url(&url),
            },
            Action::JumpTo { channel, message } => {
                let known = self.session.as_ref().is_some_and(|s| s.state.channel(channel).is_some());
                if !known {
                    self.banner("You don't have access to that channel.");
                    return;
                }
                self.apply(Action::SelectChannel(channel));
                let Some(message) = message else { return };
                let Some(s) = &self.session else { return };
                if s.state.message(channel, message).is_some() {
                    self.ui.chat.entry(channel).or_default().jump_to(message);
                    return;
                }
                // Not loaded: fetch the history around it instead.
                self.ui.loading.insert(channel);
                let rest = s.rest.clone();
                self.spawn(async move {
                    let result = rest.messages_around(channel, message).await.map_err(|e| e.to_string());
                    AppEvent::Around { channel, message, result }
                });
            }
            Action::LoadNewer(channel) => {
                let Some(s) = &self.session else { return };
                let Some(newest) = s.state.messages(channel).filter(|m| m.has_newer).and_then(|m| m.list.keys().next_back().copied()) else { return };
                if !self.ui.loading.insert(channel) {
                    return;
                }
                let rest = s.rest.clone();
                self.spawn(async move {
                    let result = rest.messages_after(channel, newest, HISTORY_PAGE).await.map_err(|e| e.to_string());
                    AppEvent::Newer { channel, result }
                });
            }
            Action::JumpToPresent(channel) => {
                if let Some(s) = &mut self.session {
                    if s.state.messages(channel).is_some_and(|m| m.has_newer) {
                        s.state.reset_history(channel);
                        self.ensure_history(channel);
                    }
                }
                self.ui.chat.entry(channel).or_default().scroll_to_bottom();
            }
            Action::LogOut => {
                crate::voice_glue::leave(self);
                self.end_session(true);
                self.start_qr_login();
            }
            Action::StartQrLogin => self.start_qr_login(),
            Action::OpenSettings => {
                let (inputs, outputs) = swiftcord_voice::list_devices();
                let cameras = swiftcord_voice::camera::list();
                let share_activity = self.session.as_ref().is_none_or(|s| s.state.shares_activity());
                let launch_at_startup = platform::startup::enabled();
                self.ui.settings_draft = Some(ui::settings::Draft {
                    settings: self.settings.clone(),
                    inputs,
                    outputs,
                    cameras,
                    mic_test: None,
                    share_activity,
                    saved_share_activity: share_activity,
                    launch_at_startup,
                    saved_launch_at_startup: launch_at_startup,
                });
            }
            Action::SaveSettings(s) => {
                let (vad, noise_suppression) = (s.vad_threshold_db, s.noise_suppression);
                self.settings = s;
                self.settings.save();
                if let Some(v) = self.voice.as_ref().and_then(|c| c.voice()) {
                    v.set_vad_threshold(vad);
                    v.set_noise_suppression(noise_suppression);
                }
            }
            Action::SetLaunchAtStartup(on) => {
                if let Err(e) = platform::startup::set(on) {
                    self.banner(format!("Couldn't change launching at startup: {e}"));
                }
            }
            Action::BrowserLogin => {
                self.ui.login.busy = true;
                self.spawn(async move {
                    match tokio::task::spawn_blocking(platform::login_webview::sign_in_blocking).await {
                        Ok(Ok(Some(token))) => AppEvent::RemoteAuth(RemoteAuthEvent::Token(token)),
                        Ok(Ok(None)) => AppEvent::TokenRejected("the sign-in window was closed".into()),
                        Ok(Err(e)) => AppEvent::TokenRejected(format!("couldn't open the sign-in window: {e}")),
                        Err(e) => AppEvent::TokenRejected(e.to_string()),
                    }
                });
            }
            Action::TokenLogin(t) => self.try_token(t),
        }
    }
}

/// Whether an action works without a connection to Discord (only views, local settings, and signing in or
/// out). The rest do nothing while the saved session is shown before the gateway is ready.
fn works_offline(action: &Action) -> bool {
    matches!(
        action,
        Action::SelectGuild(_)
            | Action::SelectChannel(_)
            | Action::Discard { .. }
            | Action::PickFiles(_)
            | Action::AddFiles { .. }
            | Action::PasteImage { .. }
            | Action::Notice(_)
            | Action::CloseGifPicker
            | Action::CloseProfile
            | Action::CloseThreadPrompt
            | Action::ToggleMemberList
            | Action::SetUserVolume { .. }
            | Action::ToggleLocalMute(_)
            | Action::PlayWeb(_)
            | Action::CloseProfileEditor
            | Action::CloseModal
            | Action::CloseSearch
            | Action::SetStreamAudio { .. }
            | Action::CloseSharePicker
            | Action::ShowStream(_)
            | Action::ToggleFullscreen
            | Action::ToggleMute
            | Action::ToggleDeafen
            | Action::Copy(_)
            | Action::OpenLink(_)
            | Action::LogOut
            | Action::StartQrLogin
            | Action::SaveSettings(_)
            | Action::SetLaunchAtStartup(_)
            | Action::OpenSettings
            | Action::BrowserLogin
            | Action::TokenLogin(_)
    )
}

impl crate::window::App for App {
    fn on_exit(&mut self) {
        if let Some(s) = self.session.as_ref().filter(|s| s.ready) {
            crate::session_cache::save(&s.state, &self.ui);
        }
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut crate::window::Frame) {
        // Bounded so a flood of events can't starve a frame.
        for _ in 0..512 {
            match self.rx.try_recv() {
                Ok(e) => self.handle_event(e),
                Err(_) => break,
            }
        }
        if self.ui.banner.as_ref().is_some_and(|(t, at)| t.is_empty() || at.elapsed() > Duration::from_secs(6)) {
            self.ui.banner = None;
        }
        self.update_badges(frame);
        self.check_away(ctx);
        // A clicked toast opens its channel (the tray has brought the window back).
        for channel in platform::toast::take_clicked() {
            self.apply(Action::SelectChannel(Snowflake(channel)));
        }
        self.clear_read_toasts(ctx);
        for sound in crate::sounds::take_pending() {
            self.sounds.play(sound, &self.settings.output_device);
        }
        crate::voice_glue::tick(self);
        self.ring_sounds();
        crate::interactions_glue::tick(self);
        crate::share_glue::tick(self);
        self.ui.clips.tick();
        self.ui.videos.tick();
        let (output, volume) = (self.ui.videos.output.clone(), self.ui.videos.volume.unwrap_or(1.0));
        self.ui.audio.tick(output, volume);
        self.ui.stickers.tick();
        crate::search_glue::tick(self);
        crate::image_cache::tick(ctx, &self.images, &mut self.images_checked);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut crate::window::Frame) {
        let started = Instant::now();
        self.caret_timeout(ui.ctx());
        // Alt+Shift+Down/Up: the next or previous unread channel, as in Discord. Taken before the text box
        // could use the arrows.
        let jump = ui.input_mut(|i| {
            let mods = egui::Modifiers::ALT | egui::Modifiers::SHIFT;
            if i.consume_key(mods, egui::Key::ArrowDown) {
                Some(1)
            } else if i.consume_key(mods, egui::Key::ArrowUp) {
                Some(-1)
            } else {
                None
            }
        });
        if let Some(step) = jump {
            self.jump_to_unread(step);
        }
        self.ui.web_player_slot = None;
        let mut actions = Vec::new();
        if let Some(s) = &self.session {
            let key = (self.ui.guild, self.emoji.version, s.state.revision);
            if self.ui.quick_reactions.0 != Some(key) {
                self.ui.quick_reactions = (Some(key), self.emoji.quick_reactions(&s.state, self.ui.guild, QUICK_REACTIONS));
            }
        }
        match &self.session {
            None => ui::login::show(ui, &mut self.ui.login, &mut actions),
            Some(s) => ui::shell::show(ui, s, &mut self.ui, &mut self.emoji, &mut actions),
        }
        if self.session.is_some() {
            self.ui.videos.output.clone_from(&self.settings.output_device);
            self.ui.videos.viewer(ui.ctx());
        }
        for a in actions {
            self.apply(a);
        }
        self.place_web_player(ui.ctx(), frame);
        ui::widgets::pointer_cursor(ui.ctx());
        // The visible channel's history may need fetching after a reconnect reset the cache.
        if let Some(c) = self.ui.channel() {
            self.ensure_history(c);
        }
        self.perf.frame(ui.ctx(), started.elapsed());
    }
}

impl App {
    /// As in Windows, the text cursor stops blinking a few seconds after the last input and stays lit: a
    /// blinking cursor redraws the window twice a second, which with the message box focused is all the time.
    fn caret_timeout(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| !i.events.is_empty() || i.pointer.is_moving()) {
            self.last_input = Instant::now();
        }
        let blink = self.last_input.elapsed() < CARET_TIMEOUT;
        if ctx.style_of(egui::Theme::Dark).visuals.text_cursor.blink != blink {
            ctx.style_mut_of(egui::Theme::Dark, |s| s.visuals.text_cursor.blink = blink);
            // The style takes effect from the next frame.
            ctx.request_repaint();
        }
    }

    /// Starts, moves, hides or stops the inline web player to match the embed it's playing in.
    fn place_web_player(&mut self, ctx: &egui::Context, frame: &crate::window::Frame) {
        // Leaving its channel stops it, as does being out of sight a while; another video replaces it.
        if self.ui.web_player.as_ref().is_some_and(|(_, c)| self.ui.channel() != Some(*c)) || self.web_player.as_ref().is_some_and(|p| p.stale()) {
            self.ui.web_player = None;
        }
        let wanted = self.ui.web_player.clone();
        if self.web_player.as_ref().is_some_and(|p| wanted.as_ref().is_none_or(|(u, _)| *u != p.url)) {
            self.web_player = None;
        }
        if self.web_player.is_none() {
            let Some((url, _)) = wanted else { return };
            match crate::platform::inline_player::InlinePlayer::new(frame, &url) {
                Ok(p) => self.web_player = Some(p),
                Err(e) => {
                    // No WebView2 here: the video still plays in its own window.
                    tracing::warn!("inline player unavailable ({e}); using the video window");
                    crate::platform::video_window::open(&url, "Video");
                    self.ui.web_player = None;
                    return;
                }
            }
        }
        let Some(player) = &mut self.web_player else { return };
        // Shown only where it's drawn whole, with nothing (a menu, a dialog) on top: a native window can't be
        // clipped to the list or drawn under egui.
        let rect = self.ui.web_player_slot.filter(|(rect, visible)| visible.contains_rect(*rect)).map(|(rect, _)| rect).filter(|rect| {
            [rect.center(), rect.left_top(), rect.right_top(), rect.left_bottom(), rect.right_bottom()]
                .iter()
                .all(|p| ctx.layer_id_at(*p).is_none_or(|l| l.order == egui::Order::Background))
        });
        let ppp = ctx.pixels_per_point();
        player.place(rect.map(|r| [(r.min.x * ppp).round() as i32, (r.min.y * ppp).round() as i32, (r.width() * ppp).round() as i32, (r.height() * ppp).round() as i32]));
        // Keep checking while it's out of sight, so it stops in time.
        if rect.is_none() {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }
}

/// A pasted clipboard image as PNG bytes, to upload.
fn pasted_png(image: &egui::ColorImage) -> Result<Vec<u8>, String> {
    platform::wic::encode_png(&platform::wic::Rgba::from_color_image(image))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pasted_image_round_trips_through_png() {
        let pixels = vec![egui::Color32::from_rgb(200, 30, 90); 6];
        let png = pasted_png(&egui::ColorImage::new([3, 2], pixels)).unwrap();
        let back = platform::wic::decode(&png).unwrap();
        assert_eq!((back.width, back.height), (3, 2));
        assert_eq!(&back.pixels[20..24], &[200, 30, 90, 255]);
    }
}
