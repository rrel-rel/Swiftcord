//! Editing your profile: the main one everyone sees (display name, avatar, banner, colours, pronouns, bio) and
//! your profile in one server (nickname, avatar, banner, theme, pronouns, bio). Banners, theme colours, server
//! avatars and server bios are Nitro perks; a nickname needs the server's Change Nickname permission.

use egui::{self, Align, Color32, ColorImage, Layout, RichText, TextureHandle, Ui, Vec2};
use serde_json::{json, Map, Value};
use swiftcord_core::cdn;
use swiftcord_core::permissions::perm;
use swiftcord_core::Snowflake;

use crate::app::{Action, Session, UiState};
use crate::theme;

/// The largest image Discord takes for an avatar or banner.
const MAX_BYTES: usize = 10 * 1024 * 1024;
const BIO_LIMIT: usize = 190;
const PRONOUNS_LIMIT: usize = 40;

/// A picture being changed.
pub enum ImageChange {
    Keep,
    Remove,
    /// A new picture: the data URI to send, and a preview.
    Set { data_uri: String, preview: TextureHandle },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Avatar,
    Banner,
}

/// The text and colour fields of one profile, as loaded and as edited.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Fields {
    /// The display name (main profile) or nickname (server profile).
    pub name: String,
    pub pronouns: String,
    pub bio: String,
    pub accent: Option<u32>,
    pub theme: Option<[u32; 2]>,
}

pub struct Side {
    pub fields: Fields,
    original: Fields,
    pub avatar: ImageChange,
    pub banner: ImageChange,
    /// The current banner hash, for showing it.
    banner_hash: Option<String>,
    /// Filled in from the fetched profile yet.
    pub loaded: bool,
}

impl Side {
    fn new(name: String) -> Side {
        let fields = Fields { name, ..Default::default() };
        Side { original: fields.clone(), fields, avatar: ImageChange::Keep, banner: ImageChange::Keep, banner_hash: None, loaded: false }
    }

    fn image(&mut self, kind: ImageKind) -> &mut ImageChange {
        match kind {
            ImageKind::Avatar => &mut self.avatar,
            ImageKind::Banner => &mut self.banner,
        }
    }

    fn changed(&self) -> bool {
        self.fields != self.original || !matches!(self.avatar, ImageChange::Keep) || !matches!(self.banner, ImageChange::Keep)
    }
}

fn image_value(c: &ImageChange) -> Option<Value> {
    match c {
        ImageChange::Keep => None,
        ImageChange::Remove => Some(Value::Null),
        ImageChange::Set { data_uri, .. } => Some(json!(data_uri)),
    }
}

/// What to send, as `(path, body)` requests, in order; only what changed.
pub type Requests = Vec<(String, Map<String, Value>)>;

pub struct Editor {
    /// The server whose profile the Server tab edits, if one was open.
    pub guild: Option<Snowflake>,
    /// Showing the server tab.
    pub server_tab: bool,
    pub main: Side,
    pub server: Side,
    pub busy: bool,
    pub error: Option<String>,
}

impl Editor {
    pub fn new(s: &Session, guild: Option<Snowflake>, server_tab: bool) -> Editor {
        let me = s.state.me.clone().unwrap_or_default();
        let nick = guild.and_then(|g| s.state.member(g, me.id)).and_then(|m| m.nick.clone()).unwrap_or_default();
        Editor {
            guild,
            server_tab: server_tab && guild.is_some(),
            main: Side::new(me.global_name.clone().unwrap_or_default()),
            server: Side::new(nick),
            busy: false,
            error: None,
        }
    }

    pub fn side(&mut self) -> &mut Side {
        if self.server_tab { &mut self.server } else { &mut self.main }
    }

    /// Fills in what the fetched profiles say (bio, pronouns, colours, banner) once they arrive.
    fn fill(&mut self, uis_profiles: &crate::ui::profile::Profiles, me: Snowflake) {
        if !self.main.loaded {
            if let Some(Some(Ok(p))) = uis_profiles.get(&(me, self.guild)).or_else(|| uis_profiles.get(&(me, None))) {
                let d = p.user_profile.clone().unwrap_or_default();
                let f = &mut self.main.fields;
                f.pronouns = d.pronouns.unwrap_or_default();
                f.bio = d.bio.or_else(|| p.user.bio.clone()).unwrap_or_default();
                f.accent = d.accent_color.or(p.user.accent_color);
                f.theme = d.theme_colors.filter(|c| c.len() >= 2).map(|c| [c[0], c[1]]);
                self.main.banner_hash = d.banner.or_else(|| p.user.banner.clone()).filter(|b| !b.is_empty());
                self.main.original = self.main.fields.clone();
                self.main.loaded = true;
            }
        }
        if !self.server.loaded {
            if let Some(Some(Ok(p))) = self.guild.and_then(|g| uis_profiles.get(&(me, Some(g)))) {
                let d = p.guild_member_profile.clone().unwrap_or_default();
                let f = &mut self.server.fields;
                f.pronouns = d.pronouns.unwrap_or_default();
                f.bio = d.bio.unwrap_or_default();
                f.accent = d.accent_color;
                f.theme = d.theme_colors.filter(|c| c.len() >= 2).map(|c| [c[0], c[1]]);
                self.server.banner_hash = d.banner.filter(|b| !b.is_empty());
                self.server.original = self.server.fields.clone();
                self.server.loaded = true;
            }
        }
    }

    /// The requests that save both tabs' changes.
    pub fn requests(&self) -> Requests {
        let mut out = Vec::new();
        // Main profile: the account (name, avatar), then the profile (the rest).
        let (m, o) = (&self.main.fields, &self.main.original);
        let mut account = Map::new();
        if m.name.trim() != o.name.trim() {
            account.insert("global_name".into(), if m.name.trim().is_empty() { Value::Null } else { json!(m.name.trim()) });
        }
        if let Some(v) = image_value(&self.main.avatar) {
            account.insert("avatar".into(), v);
        }
        if !account.is_empty() {
            out.push(("users/@me".to_owned(), account));
        }
        let profile = profile_body(m, o, &self.main.banner, true);
        if !profile.is_empty() {
            out.push(("users/@me/profile".to_owned(), profile));
        }
        // Server profile: the member (nickname, avatar), then the profile (the rest).
        if let Some(g) = self.guild {
            let (m, o) = (&self.server.fields, &self.server.original);
            let mut member = Map::new();
            if m.name.trim() != o.name.trim() {
                member.insert("nick".into(), json!(m.name.trim()));
            }
            if let Some(v) = image_value(&self.server.avatar) {
                member.insert("avatar".into(), v);
            }
            if !member.is_empty() {
                out.push((format!("guilds/{g}/members/@me"), member));
            }
            let profile = profile_body(m, o, &self.server.banner, false);
            if !profile.is_empty() {
                out.push((format!("guilds/{g}/profile/@me"), profile));
            }
        }
        out
    }
}

/// The profile fields that changed (`accent_color` only exists on the main profile here).
fn profile_body(m: &Fields, o: &Fields, banner: &ImageChange, main: bool) -> Map<String, Value> {
    let mut body = Map::new();
    if m.pronouns.trim() != o.pronouns.trim() {
        body.insert("pronouns".into(), json!(m.pronouns.trim()));
    }
    if m.bio.trim() != o.bio.trim() {
        body.insert("bio".into(), json!(m.bio.trim()));
    }
    if main && m.accent != o.accent {
        body.insert("accent_color".into(), json!(m.accent));
    }
    if m.theme != o.theme {
        body.insert("theme_colors".into(), json!(m.theme));
    }
    if let Some(v) = image_value(banner) {
        body.insert("banner".into(), v);
    }
    body
}

/// Turns a picked image into an avatar (a centred square of at most 512 px) or a banner (a centred 5:2
/// crop of at most 1500×600). Animated GIFs go as they are (Discord crops them). Returns the data URI and a
/// small preview.
pub fn prepare_image(bytes: &[u8], kind: ImageKind) -> Result<(String, ColorImage), String> {
    use base64::Engine;
    if bytes.len() > MAX_BYTES {
        return Err("That image is over 10 MB.".into());
    }
    use crate::platform::wic;
    let img = wic::decode(bytes).map_err(|_| "That file isn't an image Discord can use.".to_owned())?;
    let (w, h) = (img.width, img.height);
    let (cw, ch, max_w) = match kind {
        ImageKind::Avatar => {
            let side = w.min(h);
            (side, side, 512)
        }
        ImageKind::Banner => {
            // 5:2, as wide as fits.
            let cw = w.min(h * 5 / 2);
            (cw, cw * 2 / 5, 1500)
        }
    };
    let cropped = img.crop((w - cw) / 2, (h - ch) / 2, cw, ch);
    let preview = wic::fit(&cropped, 256, 256)?.to_color_image();
    let b64 = base64::engine::general_purpose::STANDARD;
    if bytes.starts_with(b"GIF8") {
        return Ok((format!("data:image/gif;base64,{}", b64.encode(bytes)), preview));
    }
    let resized = if cw > max_w { wic::resize(&cropped, max_w, max_w * ch / cw.max(1))? } else { cropped };
    let png = wic::encode_png(&resized)?;
    Ok((format!("data:image/png;base64,{}", b64.encode(png)), preview))
}

/// Applies a picked image to the tab it was picked for.
pub fn set_image(ed: &mut Editor, server: bool, kind: ImageKind, data_uri: String, preview: TextureHandle) {
    let side = if server { &mut ed.server } else { &mut ed.main };
    *side.image(kind) = ImageChange::Set { data_uri, preview };
}

fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

fn to_u32(c: Color32) -> u32 {
    ((c.r() as u32) << 16) | ((c.g() as u32) << 8) | c.b() as u32
}

/// An optional colour: a checkbox, and a picker while it's on.
fn color_field(ui: &mut Ui, label: &str, value: &mut Option<u32>, default: u32, enabled: bool) {
    ui.add_enabled_ui(enabled, |ui| {
        ui.horizontal(|ui| {
            let mut on = value.is_some();
            if ui.checkbox(&mut on, label).changed() {
                *value = on.then_some(value.unwrap_or(default));
            }
            if let Some(v) = value {
                let mut c = rgb(*v);
                if egui::color_picker::color_edit_button_srgba(ui, &mut c, egui::color_picker::Alpha::Opaque).changed() {
                    *v = to_u32(c);
                }
            }
        });
    });
}

fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new(text.to_uppercase()).font(theme::font_semibold(12.0)).color(theme::MUTED()));
}

fn nitro_note(ui: &mut Ui, nitro: bool) {
    if !nitro {
        ui.label(RichText::new("Needs Nitro.").size(12.0).color(theme::FAINT()));
    }
}

pub fn show(ui: &egui::Ui, s: &Session, uis: &mut UiState, actions: &mut Vec<Action>) {
    let Some(ed) = &mut uis.profile_editor else { return };
    let state = &s.state;
    let Some(me) = state.me.clone() else { return };
    ed.fill(&uis.profiles, me.id);
    let nitro = me.premium_type.is_some_and(|t| t > 0);
    let guild = ed.guild.and_then(|g| state.guild(g));
    let can_nick = ed.guild.is_some_and(|g| state.guild_permissions(g) & (perm::CHANGE_NICKNAME | perm::ADMINISTRATOR) != 0) || guild.is_some_and(|g| g.owner_id == me.id);
    let mut open = true;
    egui::Window::new("Edit Profile")
        .id(egui::Id::unique("profile-editor"))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ui.ctx(), |ui| {
            ui.set_width(420.0);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut ed.server_tab, false, "Main Profile");
                if let Some(g) = guild {
                    ui.selectable_value(&mut ed.server_tab, true, format!("Server Profile — {}", g.name));
                }
            });
            ui.label(
                RichText::new(if ed.server_tab { "How you appear in this server only. Empty fields use your main profile." } else { "How you appear everywhere." })
                    .size(12.5)
                    .color(theme::MUTED()),
            );
            ui.separator();
            let server = ed.server_tab;
            let busy = ed.busy;
            let edit_guild = ed.guild.filter(|_| server);
            let side = ed.side();
            if !side.loaded {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(14.0));
                    ui.label(RichText::new("Loading your profile…").color(theme::MUTED()));
                });
            }
            ui.add_enabled_ui(!busy, |ui| {
                // Name.
                heading(ui, if server { "Nickname" } else { "Display Name" });
                let hint = if server { me.display_name().to_owned() } else { me.username.clone() };
                ui.add_enabled(!server || can_nick, egui::TextEdit::singleline(&mut side.fields.name).hint_text(hint).char_limit(32).desired_width(f32::INFINITY));
                if server && !can_nick {
                    ui.label(RichText::new("This server doesn't let you change your nickname.").size(12.0).color(theme::FAINT()));
                }

                // Avatar and banner.
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        heading(ui, "Avatar");
                        let size = Vec2::splat(72.0);
                        match &side.avatar {
                            ImageChange::Set { preview, .. } => {
                                ui.add(egui::Image::new(preview).fit_to_exact_size(size).corner_radius(36));
                            }
                            ImageChange::Remove => {
                                let url = if server { cdn::avatar(&me, 128) } else { cdn::default_avatar(&me) };
                                ui.add(egui::Image::from_uri(url).fit_to_exact_size(size).corner_radius(36));
                            }
                            ImageChange::Keep => {
                                ui.add(egui::Image::from_uri(state.avatar_url(&me, if server { edit_guild } else { None }, 128)).fit_to_exact_size(size).corner_radius(36));
                            }
                        }
                        let avatar_ok = !server || nitro;
                        ui.add_enabled_ui(avatar_ok, |ui| {
                            if ui.button("Change…").clicked() {
                                actions.push(Action::PickProfileImage { server, kind: ImageKind::Avatar });
                            }
                            let has = matches!(side.avatar, ImageChange::Set { .. }) || (server && state.member(edit_guild.unwrap_or_default(), me.id).and_then(|m| m.avatar.as_deref()).is_some_and(|a| !a.is_empty())) || (!server && me.avatar.is_some());
                            if has && ui.button(if server { "Use Main Avatar" } else { "Remove" }).clicked() {
                                side.avatar = ImageChange::Remove;
                            }
                        });
                        if server {
                            nitro_note(ui, nitro);
                        }
                    });
                    ui.add_space(16.0);
                    ui.vertical(|ui| {
                        heading(ui, "Banner");
                        let size = Vec2::new(200.0, 80.0);
                        match (&side.banner, &side.banner_hash) {
                            (ImageChange::Set { preview, .. }, _) => {
                                ui.add(egui::Image::new(preview).fit_to_exact_size(size).corner_radius(6));
                            }
                            (ImageChange::Keep, Some(hash)) => {
                                let url = cdn::banner(me.id, if server { edit_guild } else { None }, hash, false);
                                ui.add(egui::Image::from_uri(url).fit_to_exact_size(size).corner_radius(6));
                            }
                            _ => {
                                let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                                let fill = side.fields.accent.map(rgb).unwrap_or(theme::SELECTED());
                                ui.painter().rect_filled(rect, 6, fill);
                            }
                        }
                        ui.add_enabled_ui(nitro, |ui| {
                            ui.horizontal(|ui| {
                                if ui.button("Change…").clicked() {
                                    actions.push(Action::PickProfileImage { server, kind: ImageKind::Banner });
                                }
                                let has = matches!(side.banner, ImageChange::Set { .. }) || (matches!(side.banner, ImageChange::Keep) && side.banner_hash.is_some());
                                if has && ui.button("Remove").clicked() {
                                    side.banner = ImageChange::Remove;
                                }
                            });
                        });
                        nitro_note(ui, nitro);
                    });
                });

                // Colours.
                heading(ui, "Colours");
                if !server {
                    color_field(ui, "Banner colour (shown without a banner)", &mut side.fields.accent, 0x5865F2, true);
                }
                let mut theme_on = side.fields.theme.is_some();
                ui.add_enabled_ui(nitro, |ui| {
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut theme_on, "Profile theme").changed() {
                            side.fields.theme = theme_on.then_some(side.fields.theme.unwrap_or([0x5865F2, 0x1E1F22]));
                        }
                        if let Some(t) = &mut side.fields.theme {
                            for c in t.iter_mut() {
                                let mut col = rgb(*c);
                                if egui::color_picker::color_edit_button_srgba(ui, &mut col, egui::color_picker::Alpha::Opaque).changed() {
                                    *c = to_u32(col);
                                }
                            }
                            ui.label(RichText::new("primary, accent").size(12.0).color(theme::FAINT()));
                        }
                    });
                });
                nitro_note(ui, nitro);

                // Pronouns and bio.
                heading(ui, "Pronouns");
                ui.add(egui::TextEdit::singleline(&mut side.fields.pronouns).hint_text("e.g. they/them").char_limit(PRONOUNS_LIMIT).desired_width(f32::INFINITY));
                heading(ui, "About Me");
                let bio_ok = !server || nitro;
                ui.add_enabled(bio_ok, egui::TextEdit::multiline(&mut side.fields.bio).char_limit(BIO_LIMIT).desired_rows(4).desired_width(f32::INFINITY));
                ui.horizontal(|ui| {
                    if server {
                        nitro_note(ui, nitro);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(format!("{}/{BIO_LIMIT}", side.fields.bio.chars().count())).size(11.5).color(theme::FAINT()));
                    });
                });
            });

            if let Some(e) = &ed.error {
                ui.add_space(6.0);
                ui.label(RichText::new(e).color(theme::RED()));
            }
            ui.add_space(10.0);
            ui.allocate_ui_with_layout(Vec2::new(ui.available_width(), 30.0), Layout::right_to_left(Align::Center), |ui| {
                let changed = ed.main.changed() || ed.server.changed();
                let save = egui::Button::new(RichText::new(if ed.busy { "Saving…" } else { "Save" }).color(Color32::WHITE)).fill(theme::ACCENT());
                if ui.add_enabled(changed && !ed.busy, save).clicked() {
                    actions.push(Action::SaveProfile);
                }
                if ui.button("Cancel").clicked() {
                    actions.push(Action::CloseProfileEditor);
                }
            });
        });
    if !open {
        actions.push(Action::CloseProfileEditor);
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn editor() -> Editor {
        Editor { guild: Some(Snowflake(9)), server_tab: false, main: Side::new("Me".into()), server: Side::new(String::new()), busy: false, error: None }
    }

    #[test]
    fn sends_only_what_changed_to_the_right_place() {
        let mut ed = editor();
        assert!(ed.requests().is_empty());
        ed.main.fields.bio = "hello".into();
        ed.main.fields.accent = Some(0xff0000);
        ed.server.fields.name = "Nick".into();
        ed.server.banner = ImageChange::Remove;
        let r = ed.requests();
        let paths: Vec<&str> = r.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths, ["users/@me/profile", "guilds/9/members/@me", "guilds/9/profile/@me"]);
        assert_eq!(Value::Object(r[0].1.clone()), json!({ "bio": "hello", "accent_color": 0xff0000 }));
        assert_eq!(Value::Object(r[1].1.clone()), json!({ "nick": "Nick" }));
        assert_eq!(Value::Object(r[2].1.clone()), json!({ "banner": null }));
    }

    #[test]
    fn clearing_the_display_name_sends_null() {
        let mut ed = editor();
        ed.main.fields.name.clear();
        assert_eq!(Value::Object(ed.requests()[0].1.clone()), json!({ "global_name": null }));
    }

    #[test]
    fn banners_are_cropped_to_five_by_two() {
        let img = crate::platform::wic::Rgba { width: 1000, height: 1000, pixels: [10, 20, 30, 255].repeat(1000 * 1000) };
        let png = crate::platform::wic::encode_png(&img).unwrap();
        let (uri, preview) = prepare_image(&png, ImageKind::Banner).unwrap();
        assert!(uri.starts_with("data:image/png;base64,"));
        let ratio = preview.size[0] as f32 / preview.size[1] as f32;
        assert!((ratio - 2.5).abs() < 0.05, "{:?}", preview.size);
    }
}
