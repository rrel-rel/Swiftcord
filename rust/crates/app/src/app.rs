//! Application state and the glue between the UI, background tasks and the Discord session.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use eframe::egui;
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
    Stream { generation: u64, event: swiftcord_voice::Event },
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
    Notice(String),
    SearchMembers { guild: Snowflake, query: String },
    OpenDm(Snowflake),
    ShowPins(Snowflake),
    Ack(Snowflake),
    JoinVoice { guild: Option<Snowflake>, channel: Snowflake },
    /// Answer a DM call: join it and show the conversation.
    AcceptCall(Snowflake),
    DeclineCall(Snowflake),
    WatchStream { guild: Option<Snowflake>, channel: Snowflake, user: Snowflake },
    StopWatching,
    /// Show the stream in the main area (true) or go back to the chat (false).
    ShowStream(bool),
    ToggleFullscreen,
    LeaveVoice,
    ToggleMute,
    ToggleDeafen,
    Copy(String),
    /// A clicked link: Discord channel/message links open in the app, others in the browser.
    OpenLink(String),
    JumpTo { channel: Snowflake, message: Option<Snowflake> },
    LoadNewer(Snowflake),
    JumpToPresent(Snowflake),
    LogOut,
    StartQrLogin,
    SaveSettings(crate::settings::Settings),
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
    pub open_folders: HashSet<i64>,
    pub drafts: HashMap<Snowflake, Draft>,
    pub editing: Option<(Snowflake, String)>,
    pub chat: HashMap<Snowflake, ui::chat::ChatView>,
    pub loading: HashSet<Snowflake>,
    pub revealed: HashSet<(Snowflake, u16)>,
    pub emoji_picker: Option<ui::emoji::PickerTarget>,
    pub emoji_search: String,
    pub pins: Option<(Snowflake, Option<Vec<Message>>)>,
    pub banner: Option<(String, Instant)>,
    pub login: ui::login::LoginUi,
    pub self_mute: bool,
    pub self_deaf: bool,
    pub focus_composer: bool,
    /// Users heard in the current call just now (including us).
    pub speaking: HashSet<Snowflake>,
    pub voice: Option<crate::voice_glue::VoiceView>,
    pub stream: Option<crate::stream_glue::StreamView>,
    pub settings_draft: Option<ui::settings::Draft>,
    /// A pending member search (guild, query, when it changed, sent yet).
    pub member_query: Option<(Snowflake, String, Instant, bool)>,
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
    pub settings: crate::settings::Settings,
    history_failed: HashMap<Snowflake, Instant>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, rt: tokio::runtime::Handle) -> App {
        crate::theme::install(&cc.egui_ctx);
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let cache = platform::data_dir().join("cache").join("media");
        cc.egui_ctx.add_bytes_loader(std::sync::Arc::new(crate::media::MediaLoader::new(rt.clone(), cache)));

        let (tx, rx) = unbounded_channel();
        let mut app = App {
            rt,
            tx,
            rx,
            ctx: cc.egui_ctx.clone(),
            session: None,
            ui: UiState::default(),
            outgoing: HashMap::new(),
            emoji: ui::emoji::Catalog::load(),
            voice: None,
            watch: None,
            settings: crate::settings::Settings::load(),
            history_failed: HashMap::new(),
        };
        match credentials::load_token() {
            Some(token) => app.start_session(token),
            None => app.start_qr_login(),
        }
        app
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
    fn spawn<F>(&self, fut: F)
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
            Self::send_event(&tx, &ctx, AppEvent::Gateway(out));
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
        self.watch = None;
        self.voice = None;
        let login = std::mem::take(&mut self.ui.login);
        self.ui = UiState { login, ..Default::default() };
        self.outgoing.clear();
    }

    fn start_qr_login(&mut self) {
        self.ui.login.reset_qr();
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        self.rt.spawn(async move {
            let (etx, mut erx) = unbounded_channel();
            tokio::spawn(swiftcord_core::remote_auth::run(etx));
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
                        s.state.add_history(channel, page, HISTORY_PAGE as usize)
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
                    Ok(page) => s.state.add_newer(channel, page, HISTORY_PAGE as usize),
                    Err(e) => self.banner(format!("Couldn't load messages: {e}")),
                }
            }
            AppEvent::Around { channel, message, result } => {
                self.ui.loading.remove(&channel);
                let Some(s) = &mut self.session else { return };
                match result {
                    Ok(page) if page.iter().any(|m| m.id == message) => {
                        s.state.set_around(channel, page);
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
                Ok(p) => self.ui.pins = Some((channel, Some(p))),
                Err(e) => {
                    self.ui.pins = None;
                    self.banner(format!("Couldn't load pins: {e}"));
                }
            },
            AppEvent::Voice { generation, event } => crate::voice_glue::on_event(self, generation, event),
            AppEvent::Stream { generation, event } => crate::stream_glue::on_event(self, generation, event),
            AppEvent::Folders(folders) => {
                if let (Some(f), Some(s)) = (folders, &mut self.session) {
                    s.state.set_folders(f);
                }
            }
            AppEvent::DmOpened(channel) => {
                let id = channel.id;
                if let Some(s) = &mut self.session {
                    s.state.add_channel(channel);
                }
                self.ui.guild = None;
                self.select_channel(id);
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
                let is_ready = matches!(event, swiftcord_core::events::Event::Ready(_));
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
            }
        }
    }

    fn on_ready(&mut self) {
        let Some(s) = &mut self.session else { return };
        if crate::demo::enabled() {
            crate::demo::install(&mut s.state);
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
        // READY with legacy capabilities may not include the protobuf settings that hold server folders.
        let rest = s.rest.clone();
        self.spawn(async move {
            let folders = rest.settings_proto().await.ok().and_then(|p| swiftcord_core::proto::read_guild_folders(&p));
            AppEvent::Folders(folders)
        });
    }

    fn handle_notice(&mut self, notice: Notice) {
        let focused = self.ctx.input(|i| i.viewport().focused.unwrap_or(true));
        let current = self.ui.channel();
        let Some(s) = &mut self.session else { return };
        match notice {
            Notice::MessageCreated { message, notify } => {
                let _ = s;
                if current == Some(message.channel_id) && focused {
                    self.queue_ack(message.channel_id, message.id);
                } else if notify && self.settings.notifications {
                    self.toast(&message);
                }
            }
            Notice::Ring(channel) => {
                let caller = s.state.channel(channel).map(|c| c.label()).unwrap_or_default();
                platform::notify(format!("{caller} is calling you"), "Open Swiftcord to answer.".into());
            }
            Notice::RingStopped(_) => {}
            Notice::OwnVoiceState | Notice::VoiceServer(_) | Notice::Other(..) => {
                crate::voice_glue::on_notice(self, notice);
            }
        }
    }

    fn toast(&self, m: &Message) {
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
        platform::notify(title, body);
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
        if let Some(s) = &self.session {
            if s.state.channel(channel).is_some_and(|c| c.is_private()) {
                // Learn whether a call is going on in this DM.
                s.gateway.request_call(channel);
            }
        }
        self.ui.focus_composer = true;
        self.ui.emoji_picker = None;
        self.ui.pins = None;
        self.ensure_history(channel);
        self.mark_viewed(channel);
    }

    fn mark_viewed(&mut self, channel: Snowflake) {
        let Some(s) = &self.session else { return };
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
        let Some(me) = s.state.me.clone() else { return };
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
        draft.mentions.clear();
        draft.dismissed = None;
        let files = std::mem::take(&mut draft.files);
        let reply = draft.reply.take();
        draft.text.clear();

        let nonce = nonce_now();
        let mut msg = NewMessage { content: content.clone(), nonce: Some(nonce.to_string()), ..Default::default() };
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
        let keep_msg = NewMessage { content: msg.content.clone(), nonce: msg.nonce.clone(), message_reference: msg.message_reference.clone(), allowed_mentions: msg.allowed_mentions.clone(), attachments: None };
        self.outgoing.insert(nonce, Outgoing { msg: keep_msg, files: keep_files });
        self.spawn(async move {
            let result = rest.send_message(channel, msg, files).await.map_err(|e| e.to_string());
            AppEvent::Sent { channel, nonce, result }
        });
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::SelectGuild(g) => {
                self.ui.guild = g;
                self.ui.emoji_picker = None;
                self.ui.pins = None;
                if let (Some(g), Some(s)) = (g, &mut self.session) {
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
                    if let (Some(g), Some(s)) = (g, &mut self.session) {
                        if s.subscribed.insert(g) {
                            s.gateway.subscribe_guild(g);
                        }
                    }
                }
                self.select_channel(c);
            }
            Action::AddFiles { channel, files } => self.add_files(channel, files),
            Action::Notice(text) => self.banner(text),
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
            Action::Send(c) => self.send(c),
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
            Action::ShowPins(channel) => {
                if matches!(self.ui.pins, Some((c, _)) if c == channel) {
                    self.ui.pins = None;
                    return;
                }
                let Some(s) = &self.session else { return };
                self.ui.pins = Some((channel, None));
                let rest = s.rest.clone();
                self.spawn(async move {
                    let result = rest.pins(channel).await.map_err(|e| e.to_string());
                    AppEvent::Pins { channel, result }
                });
            }
            Action::Ack(c) => self.mark_viewed(c),
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
            Action::StopWatching => crate::stream_glue::stop(self),
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
                self.ui.settings_draft = Some(ui::settings::Draft { settings: self.settings.clone(), inputs, outputs, mic_test: None });
            }
            Action::SaveSettings(s) => {
                let vad = s.vad_threshold_db;
                self.settings = s;
                self.settings.save();
                if let Some(v) = self.voice.as_ref().and_then(|c| c.voice()) {
                    v.set_vad_threshold(vad);
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

impl eframe::App for App {
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
        crate::voice_glue::tick(self);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let mut actions = Vec::new();
        match &self.session {
            None => ui::login::show(ui, &mut self.ui.login, &mut actions),
            Some(s) => ui::shell::show(ui, s, &mut self.ui, &mut self.emoji, &mut actions),
        }
        for a in actions {
            self.apply(a);
        }
        // The visible channel's history may need fetching after a reconnect reset the cache.
        if let Some(c) = self.ui.channel() {
            self.ensure_history(c);
        }
    }
}
