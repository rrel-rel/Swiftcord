//! Editing your server profile: the nickname and avatar you have in one server (a server avatar is a Nitro
//! perk; a nickname needs the server's Change Nickname permission).

use eframe::egui::{self, Align, ColorImage, Layout, RichText, TextureHandle, Vec2};
use swiftcord_core::permissions::perm;
use swiftcord_core::Snowflake;

use crate::app::{Action, Session, UiState};
use crate::theme;

/// The largest image Discord takes for an avatar.
const MAX_BYTES: usize = 10 * 1024 * 1024;

pub enum AvatarChange {
    Keep,
    Remove,
    /// A new picture: the data URI to send, and a preview.
    Set { data_uri: String, preview: TextureHandle },
}

pub struct Editor {
    pub guild: Snowflake,
    pub nick: String,
    /// The nickname when the editor opened, to send only what changed.
    original_nick: String,
    pub avatar: AvatarChange,
    pub busy: bool,
    pub error: Option<String>,
}

impl Editor {
    pub fn new(s: &Session, guild: Snowflake) -> Editor {
        let nick = s.state.me.as_ref().and_then(|me| s.state.member(guild, me.id)).and_then(|m| m.nick.clone()).unwrap_or_default();
        Editor { guild, original_nick: nick.clone(), nick, avatar: AvatarChange::Keep, busy: false, error: None }
    }

    /// What to send: the nickname if it changed, and the avatar if it changed.
    pub fn changes(&self) -> (Option<String>, Option<Option<String>>) {
        let nick = (self.nick.trim() != self.original_nick.trim()).then(|| self.nick.trim().to_owned());
        let avatar = match &self.avatar {
            AvatarChange::Keep => None,
            AvatarChange::Remove => Some(None),
            AvatarChange::Set { data_uri, .. } => Some(Some(data_uri.clone())),
        };
        (nick, avatar)
    }
}

/// Turns a picked image into an avatar: animated GIFs go as they are (Discord crops them); anything else is
/// cropped to a centred square of at most 512 px. Returns the data URI and a small preview.
pub fn prepare_avatar(bytes: &[u8]) -> Result<(String, ColorImage), String> {
    use base64::Engine;
    if bytes.len() > MAX_BYTES {
        return Err("That image is over 10 MB.".into());
    }
    let img = image::load_from_memory(bytes).map_err(|_| "That file isn't an image Discord can use.".to_owned())?;
    let side = img.width().min(img.height());
    let square = img.crop_imm((img.width() - side) / 2, (img.height() - side) / 2, side, side);
    let preview = square.thumbnail(128, 128).to_rgba8();
    let preview = ColorImage::from_rgba_unmultiplied([preview.width() as usize, preview.height() as usize], preview.as_raw());
    let b64 = base64::engine::general_purpose::STANDARD;
    if bytes.starts_with(b"GIF8") {
        return Ok((format!("data:image/gif;base64,{}", b64.encode(bytes)), preview));
    }
    let resized = if side > 512 { square.resize_exact(512, 512, image::imageops::FilterType::Triangle) } else { square };
    let mut png = std::io::Cursor::new(Vec::new());
    resized.write_to(&mut png, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok((format!("data:image/png;base64,{}", b64.encode(png.into_inner())), preview))
}

pub fn show(ui: &egui::Ui, s: &Session, uis: &mut UiState, actions: &mut Vec<Action>) {
    let Some(ed) = &mut uis.server_profile else { return };
    let state = &s.state;
    let Some(guild) = state.guild(ed.guild) else {
        uis.server_profile = None;
        return;
    };
    let Some(me) = state.me.clone() else { return };
    let nitro = me.premium_type.is_some_and(|t| t > 0);
    let can_nick = state.guild_permissions(ed.guild) & (perm::CHANGE_NICKNAME | perm::ADMINISTRATOR) != 0 || guild.owner_id == me.id;
    let has_server_avatar = state.member(ed.guild, me.id).and_then(|m| m.avatar.as_deref()).is_some_and(|a| !a.is_empty());
    let mut open = true;
    egui::Window::new(format!("Server Profile — {}", guild.name))
        .id(egui::Id::unique("server-profile"))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ui.ctx(), |ui| {
            ui.set_width(380.0);
            ui.label(RichText::new("How you appear in this server only.").color(theme::MUTED));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                // The avatar as it will be.
                let size = Vec2::splat(80.0);
                match &ed.avatar {
                    AvatarChange::Set { preview, .. } => {
                        ui.add(egui::Image::new(preview).fit_to_exact_size(size).corner_radius(40));
                    }
                    AvatarChange::Remove => {
                        ui.add(egui::Image::from_uri(swiftcord_core::cdn::avatar(&me, 128)).fit_to_exact_size(size).corner_radius(40));
                    }
                    AvatarChange::Keep => {
                        ui.add(egui::Image::from_uri(state.avatar_url(&me, Some(ed.guild), 128)).fit_to_exact_size(size).corner_radius(40));
                    }
                }
                ui.vertical(|ui| {
                    ui.label(RichText::new("Server Avatar").font(theme::font_semibold(13.0)).color(theme::HEADER));
                    ui.add_enabled_ui(nitro && !ed.busy, |ui| {
                        if ui.button("Change Avatar…").clicked() {
                            actions.push(Action::PickServerAvatar);
                        }
                        let removable = has_server_avatar && !matches!(ed.avatar, AvatarChange::Remove) || matches!(ed.avatar, AvatarChange::Set { .. });
                        if removable && ui.button("Use My Main Avatar").clicked() {
                            ed.avatar = if has_server_avatar { AvatarChange::Remove } else { AvatarChange::Keep };
                        }
                    });
                    if !nitro {
                        ui.label(RichText::new("Server avatars need Nitro.").size(12.0).color(theme::FAINT));
                    }
                });
            });
            ui.add_space(10.0);
            ui.label(RichText::new("Nickname").font(theme::font_semibold(13.0)).color(theme::HEADER));
            ui.add_enabled(
                can_nick && !ed.busy,
                egui::TextEdit::singleline(&mut ed.nick).hint_text(me.display_name()).char_limit(32).desired_width(f32::INFINITY),
            );
            if !can_nick {
                ui.label(RichText::new("This server doesn't let you change your nickname.").size(12.0).color(theme::FAINT));
            }
            if let Some(e) = &ed.error {
                ui.add_space(6.0);
                ui.label(RichText::new(e).color(theme::RED));
            }
            ui.add_space(10.0);
            ui.allocate_ui_with_layout(Vec2::new(ui.available_width(), 30.0), Layout::right_to_left(Align::Center), |ui| {
                let (nick, avatar) = ed.changes();
                let changed = nick.is_some() || avatar.is_some();
                let save = egui::Button::new(RichText::new(if ed.busy { "Saving…" } else { "Save" }).color(egui::Color32::WHITE)).fill(theme::BLURPLE);
                if ui.add_enabled(changed && !ed.busy, save).clicked() {
                    actions.push(Action::SaveServerProfile);
                }
                if ui.button("Cancel").clicked() {
                    actions.push(Action::CloseServerProfile);
                }
            });
        });
    if !open {
        actions.push(Action::CloseServerProfile);
    }
}
