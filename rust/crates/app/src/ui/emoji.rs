//! Emoji picker for reactions and the composer: recents, server emoji, then Unicode by category.

use std::collections::HashMap;

use eframe::egui::{self, Pos2, RichText, Ui, Vec2};
use serde::Deserialize;
use swiftcord_core::models::Emoji;
use swiftcord_core::state::State;
use swiftcord_core::Snowflake;

use crate::app::{Action, UiState};
use crate::theme;

pub(crate) const EMOJI_JSON: &str = include_str!("../../assets/emoji.json");
const RECENT_MAX: usize = 24;
const CELL: f32 = 38.0;
const COLUMNS: usize = 9;

#[derive(Debug, Clone, Copy)]
pub enum PickerTarget {
    Reaction { channel: Snowflake, message: Snowflake, at: Pos2 },
    Composer { channel: Snowflake, at: Pos2 },
}

impl PickerTarget {
    fn at(&self) -> Pos2 {
        match self {
            Self::Reaction { at, .. } | Self::Composer { at, .. } => *at,
        }
    }
}

struct Unicode {
    ch: String,
    name: String,
    /// Other names that work as :shortcodes:.
    aliases: Vec<String>,
    keywords: String,
    group: usize,
    /// Light to dark.
    skins: Option<[String; 5]>,
}

impl Unicode {
    fn names(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.name.as_str()).chain(self.aliases.iter().map(String::as_str))
    }
}

pub struct Catalog {
    unicode: Vec<Unicode>,
    groups: Vec<String>,
    recent: Vec<Emoji>,
    /// Every name and alias, and every emoji (with skin tones), to its entry.
    by_name: HashMap<String, usize>,
    by_char: HashMap<String, usize>,
    /// 0 for the default yellow, 1–5 light to dark.
    skin_tone: usize,
    /// The reactions used most, synced from Discord (Unicode emoji names and custom emoji IDs), most first.
    frequent: Vec<String>,
    /// Bumped when the recent or frequent lists change, so derived lists know to refresh.
    pub version: u64,
}

#[derive(Deserialize)]
struct File {
    groups: Vec<String>,
    /// [character, name, other names, keywords, group, (skin tones)]; the last is optional, so entries are
    /// read loosely.
    emoji: Vec<Vec<serde_json::Value>>,
}

fn recent_path() -> std::path::PathBuf {
    crate::platform::data_dir().join("recent-emoji.json")
}

fn skin_tone_path() -> std::path::PathBuf {
    crate::platform::data_dir().join("emoji-skin-tone")
}

impl Catalog {
    pub fn load() -> Catalog {
        let file: File = serde_json::from_str(EMOJI_JSON).unwrap_or(File { groups: vec![], emoji: vec![] });
        let unicode: Vec<Unicode> = file
            .emoji
            .into_iter()
            .filter_map(|e| {
                let s = |i: usize| e.get(i).and_then(|v| v.as_str()).map(str::to_owned);
                Some(Unicode {
                    ch: s(0)?,
                    name: s(1)?,
                    aliases: s(2).unwrap_or_default().split_whitespace().map(str::to_owned).collect(),
                    keywords: s(3).unwrap_or_default(),
                    group: e.get(4)?.as_u64()? as usize,
                    skins: e.get(5).and_then(|v| serde_json::from_value(v.clone()).ok()),
                })
            })
            .collect();
        let mut by_name = HashMap::new();
        let mut by_char = HashMap::new();
        for (i, u) in unicode.iter().enumerate() {
            for n in u.names() {
                by_name.entry(n.to_owned()).or_insert(i);
            }
            for c in std::iter::once(&u.ch).chain(u.skins.iter().flatten()) {
                by_char.insert(c.clone(), i);
            }
        }
        let recent = std::fs::read_to_string(recent_path())
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<(Option<Snowflake>, Option<String>, bool)>>(&s).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|(id, name, animated)| Emoji { id, name, animated })
            .collect();
        let skin_tone = std::fs::read_to_string(skin_tone_path()).ok().and_then(|s| s.trim().parse().ok()).filter(|t| *t <= 5).unwrap_or(0);
        Catalog { unicode, groups: file.groups, recent, by_name, by_char, skin_tone, frequent: Vec::new(), version: 0 }
    }

    /// The reactions Discord says are used most (from the synced frecency settings).
    pub fn set_frequent(&mut self, keys: Vec<String>) {
        self.frequent = keys;
        self.version += 1;
    }

    /// The `n` reactions to offer with one click, as the official client does: ones reacted with here most
    /// recently, then the ones used most anywhere (synced from Discord). Custom emoji are only offered where
    /// they can be used: from this server, or from any server with Nitro.
    pub fn quick_reactions(&self, state: &State, guild: Option<Snowflake>, n: usize) -> Vec<Emoji> {
        let nitro = state.me.as_ref().is_some_and(|m| m.premium_type.is_some_and(|t| t > 0));
        let here = guild.and_then(|g| state.guild(g)).map(|g| g.emojis.as_slice()).unwrap_or_default();
        let custom = |id: Snowflake| -> Option<Emoji> {
            let found = here.iter().find(|e| e.id == Some(id));
            let found = match found {
                Some(e) => Some(e),
                None if nitro => state.guilds().find_map(|g| g.emojis.iter().find(|e| e.id == Some(id))),
                None => None,
            };
            found.map(|e| Emoji { id: e.id, name: e.name.clone(), animated: e.animated })
        };
        let from_key = |key: &str| -> Option<Emoji> {
            match key.parse::<u64>() {
                Ok(id) if key.len() > 5 => custom(Snowflake(id)),
                _ => {
                    let u = &self.unicode[*self.by_name.get(key)?];
                    Some(Emoji { id: None, name: Some(self.toned(u, self.skin_tone)), animated: false })
                }
            }
        };
        let recent = self.recent.iter().filter_map(|e| match e.id {
            Some(id) => custom(id),
            None => Some(e.clone()),
        });
        let mut out: Vec<Emoji> = Vec::new();
        for e in recent.chain(self.frequent.iter().filter_map(|k| from_key(k))) {
            if !out.iter().any(|o| o.same_as(&e)) {
                out.push(e);
            }
            if out.len() == n {
                break;
            }
        }
        out
    }

    fn toned(&self, u: &Unicode, tone: usize) -> String {
        match (&u.skins, tone) {
            (Some(skins), 1..=5) => skins[tone - 1].clone(),
            _ => u.ch.clone(),
        }
    }

    pub fn set_skin_tone(&mut self, tone: usize) {
        self.skin_tone = tone.min(5);
        let _ = std::fs::write(skin_tone_path(), self.skin_tone.to_string());
    }

    /// The emoji a :shortcode: stands for (`thumbsup`, `+1`, `thumbsup_tone2`, `thumbsup::skin-tone-2`), in
    /// the chosen skin tone unless the code gives one.
    pub fn by_shortcode(&self, code: &str) -> Option<String> {
        let valid = |t: &str| t.parse::<usize>().ok().filter(|t| (1..=5).contains(t));
        let (mut name, mut tone) = match code.split_once("::skin-tone-") {
            Some((n, t)) => (n, valid(t)?),
            None => (code, self.skin_tone),
        };
        if let Some((n, t)) = name.rsplit_once("_tone").and_then(|(n, t)| Some((n, valid(t)?))) {
            if self.by_name.contains_key(n) {
                (name, tone) = (n, t);
            }
        }
        Some(self.toned(&self.unicode[*self.by_name.get(name)?], tone))
    }

    /// An emoji's :shortcode: name, for showing on hover.
    pub fn name_of(&self, ch: &str) -> Option<&str> {
        self.by_char.get(ch).map(|&i| self.unicode[i].name.as_str())
    }

    /// Unicode emoji named `query` exactly, then ones with a name starting with it, then ones with a word of
    /// their names or keywords starting with it, then names containing it, as (character, name) in the chosen
    /// skin tone.
    pub fn search(&self, query: &str, limit: usize) -> Vec<(String, String)> {
        let q = query.to_lowercase();
        let rank = |u: &Unicode| {
            if u.names().any(|n| n == q) {
                Some(0)
            } else if u.names().any(|n| n.starts_with(&q)) {
                Some(1)
            } else if u.names().flat_map(|n| n.split('_')).chain(u.keywords.split_whitespace()).any(|w| w.starts_with(&q)) {
                Some(2)
            } else if u.names().any(|n| n.contains(&q)) {
                Some(3)
            } else {
                None
            }
        };
        let mut hits: Vec<(u8, usize, &Unicode)> = self.unicode.iter().enumerate().filter_map(|(i, u)| rank(u).map(|r| (r, i, u))).collect();
        hits.sort_by_key(|&(r, i, _)| (r, i));
        hits.into_iter().take(limit).map(|(_, _, u)| (self.toned(u, self.skin_tone), u.name.clone())).collect()
    }

    pub fn note_used(&mut self, e: &Emoji) {
        self.recent.retain(|r| !r.same_as(e));
        self.recent.insert(0, e.clone());
        self.recent.truncate(RECENT_MAX);
        self.version += 1;
        let raw: Vec<_> = self.recent.iter().map(|e| (e.id, e.name.clone(), e.animated)).collect();
        let _ = std::fs::write(recent_path(), serde_json::to_string(&raw).unwrap_or_default());
    }
}

/// Replaces :shortcodes: in a message with their emoji, as Discord's client does when sending: Unicode ones
/// from the catalog (`:thumbsup:`, `:thumbsup::skin-tone-2:`), and custom ones through `custom`, which gives
/// their `<:name:id>` form. Code spans and blocks, and unknown names, are left alone.
pub fn expand_shortcodes(text: &str, catalog: &Catalog, custom: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(ch) = rest.chars().next() {
        if ch == '`' {
            let fence = if rest.starts_with("```") { "```" } else { "`" };
            let n = rest[fence.len()..].find(fence).map_or(rest.len(), |end| end + fence.len() * 2);
            out.push_str(&rest[..n]);
            rest = &rest[n..];
            continue;
        }
        // Mentions and custom emoji (<:blob:1>), and links, go through whole.
        let token = match ch {
            '<' => rest.find('>').filter(|&end| !rest[..end].contains(char::is_whitespace)).map(|end| end + 1),
            'h' if rest.starts_with("https://") || rest.starts_with("http://") => Some(rest.find(char::is_whitespace).unwrap_or(rest.len())),
            _ => None,
        };
        if let Some(n) = token {
            out.push_str(&rest[..n]);
            rest = &rest[n..];
            continue;
        }
        if ch == ':' {
            let body = &rest[1..];
            let end = body.find(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '-' | '+'))).unwrap_or(body.len());
            if end > 0 && body[end..].starts_with(':') {
                let name = &body[..end];
                let mut used = end + 2;
                let mut code = name.to_owned();
                // A skin tone written after it, as Discord shows toned emoji: :thumbsup::skin-tone-2:
                let after = &body[end + 1..];
                if let Some(t) = after.strip_prefix(":skin-tone-").and_then(|t| t.get(..2)).filter(|t| t.as_bytes()[0].is_ascii_digit() && t.ends_with(':')) {
                    code = format!("{name}::skin-tone-{}", &t[..1]);
                    used += ":skin-tone-".len() + 2;
                }
                let found = catalog.by_shortcode(&code).or_else(|| catalog.by_shortcode(name).is_none().then(|| custom(name)).flatten());
                if let Some(e) = found {
                    out.push_str(&e);
                    rest = &rest[used..];
                    continue;
                }
            }
        }
        out.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    out
}

enum Item {
    Header(String),
    Emoji(Emoji),
}

/// Lays the picker out as rows (headers take a whole row) so only visible rows are drawn.
fn rows(catalog: &Catalog, state: &State, guild: Option<Snowflake>, query: &str) -> Vec<Vec<Item>> {
    let q = query.trim().to_lowercase();
    let mut sections: Vec<(String, Vec<Emoji>)> = Vec::new();
    if q.is_empty() && !catalog.recent.is_empty() {
        sections.push(("Frequently Used".into(), catalog.recent.clone()));
    }
    // Custom emoji: this server's first, then the rest.
    let mut guild_ids: Vec<Snowflake> = guild.into_iter().collect();
    guild_ids.extend(state.rail().into_iter().flat_map(|r| match r {
        swiftcord_core::state::RailItem::Guild(g) => vec![g],
        swiftcord_core::state::RailItem::Folder { guilds, .. } => guilds,
    }).filter(|g| Some(*g) != guild));
    for g in guild_ids {
        let Some(gd) = state.guild(g) else { continue };
        let list: Vec<Emoji> = gd
            .emojis
            .iter()
            .filter(|e| q.is_empty() || e.name.as_deref().is_some_and(|n| n.to_lowercase().contains(&q)))
            .cloned()
            .collect();
        if !list.is_empty() {
            sections.push((gd.name.clone(), list));
        }
    }
    for (gi, group) in catalog.groups.iter().enumerate() {
        let list: Vec<Emoji> = catalog
            .unicode
            .iter()
            .filter(|u| u.group == gi && (q.is_empty() || u.names().any(|n| n.contains(&q)) || u.keywords.contains(&q)))
            .map(|u| Emoji { id: None, name: Some(catalog.toned(u, catalog.skin_tone)), animated: false })
            .collect();
        if !list.is_empty() {
            sections.push((group.clone(), list));
        }
    }
    let mut out = Vec::new();
    for (title, list) in sections {
        out.push(vec![Item::Header(title)]);
        let mut it = list.into_iter().peekable();
        while it.peek().is_some() {
            out.push(it.by_ref().take(COLUMNS).map(Item::Emoji).collect());
        }
    }
    out
}

/// The skin tone button beside the search box (a raised hand in the chosen tone) and its menu of tones.
/// Returns whether the menu was open this frame.
fn skin_tone_button(ui: &mut Ui, catalog: &mut Catalog) -> bool {
    let id = egui::Id::unique("emoji-skin-tone-menu");
    let was_open = ui.memory(|m| m.data.get_temp::<bool>(id).unwrap_or(false));
    let hands: Vec<String> =
        (0..=5).map(|tone| catalog.by_name.get("raised_hand").map_or_else(|| "✋".to_owned(), |&i| catalog.toned(&catalog.unicode[i], tone))).collect();
    let ctx = ui.ctx().clone();
    let image = |ch: &str| crate::emoji_sheet::image(&ctx, ch, Vec2::splat(22.0));
    let b = ui.add(egui::Button::image(image(&hands[catalog.skin_tone])).frame_when_inactive(false).min_size(Vec2::splat(32.0))).on_hover_text("Skin tone");
    let mut open = was_open;
    if b.clicked() {
        open = !open;
    }
    if open {
        let area = egui::Area::new(id.with("area")).order(egui::Order::Tooltip).fixed_pos(b.rect.left_bottom() + Vec2::new(0.0, 4.0)).show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).fill(theme::SIDEBAR()).inner_margin(4).show(ui, |ui| {
                for tone in 0..=5 {
                    let r = ui.add(egui::Button::image(image(&hands[tone])).frame_when_inactive(tone == catalog.skin_tone).min_size(Vec2::splat(32.0)));
                    if r.clicked() {
                        catalog.set_skin_tone(tone);
                        open = false;
                    }
                }
            });
        });
        // A click anywhere else closes the menu (but not the picker).
        if was_open && ui.input(|i| i.pointer.any_pressed()) && !area.response.contains_pointer() && !b.contains_pointer() {
            open = false;
        }
    }
    ui.memory_mut(|m| m.data.insert_temp(id, open));
    was_open || open
}

pub fn show(ui: &mut Ui, state: &State, uis: &mut UiState, catalog: &mut Catalog, actions: &mut Vec<Action>) {
    let Some(target) = uis.emoji_picker else { return };
    let ctx = ui.ctx().clone();
    let size = Vec2::new(COLUMNS as f32 * CELL + 24.0, 420.0);
    let screen = ctx.content_rect();
    let mut pos = target.at() - Vec2::new(size.x, size.y + 8.0);
    pos.x = pos.x.clamp(screen.left() + 8.0, screen.right() - size.x - 8.0);
    pos.y = pos.y.clamp(screen.top() + 8.0, screen.bottom() - size.y - 8.0);
    let guild = uis.channel().and_then(|c| state.channel(c)).and_then(|c| c.guild_id);

    let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    // The skin tone menu is its own layer, so clicks in it aren't clicks outside the picker.
    let mut tone_open = false;
    let area = egui::Area::new(egui::Id::unique("emoji-picker")).order(egui::Order::Foreground).fixed_pos(pos).show(&ctx, |ui| {
        egui::Frame::popup(ui.style()).fill(theme::SIDEBAR()).inner_margin(10).show(ui, |ui| {
            ui.set_width(size.x - 20.0);
            ui.set_height(size.y - 20.0);
            ui.horizontal(|ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut uis.emoji_search).hint_text("Find the perfect emoji").desired_width(ui.available_width() - 40.0));
                if !r.has_focus() && uis.emoji_search.is_empty() {
                    r.request_focus();
                }
                tone_open = skin_tone_button(ui, catalog);
            });
            ui.add_space(4.0);
            let rows = rows(catalog, state, guild, &uis.emoji_search);
            let mut hovered: Option<String> = None;
            egui::ScrollArea::vertical().auto_shrink(false).max_height(size.y - 90.0).show_rows(ui, CELL, rows.len(), |ui, range| {
                for row in &rows[range] {
                    ui.horizontal(|ui| {
                        ui.set_height(CELL);
                        ui.spacing_mut().item_spacing.x = 0.0;
                        for item in row {
                            match item {
                                Item::Header(t) => {
                                    ui.label(RichText::new(t.to_uppercase()).font(theme::font_semibold(12.0)).color(theme::MUTED()));
                                }
                                Item::Emoji(e) => {
                                    let b = egui::Button::image(crate::emoji_sheet::any(ui.ctx(), e, Vec2::splat(28.0)))
                                        .frame_when_inactive(false)
                                        .min_size(Vec2::splat(CELL));
                                    let r = ui.add(b);
                                    if r.hovered() {
                                        hovered = e.name.clone();
                                    }
                                    if r.clicked() {
                                        match target {
                                            // The app records the reaction as recently used.
                                            PickerTarget::Reaction { channel, message, .. } => {
                                                actions.push(Action::React { channel, message, emoji: e.clone(), add: true });
                                            }
                                            PickerTarget::Composer { channel, .. } => {
                                                catalog.note_used(e);
                                                let text = match e.id {
                                                    Some(id) => format!("<{}:{}:{id}>", if e.animated { "a" } else { "" }, e.name.as_deref().unwrap_or("_")),
                                                    None => e.name.clone().unwrap_or_default(),
                                                };
                                                uis.drafts.entry(channel).or_default().text.push_str(&text);
                                                uis.focus_composer = true;
                                            }
                                        }
                                        // Shift-click picks several, as in Discord.
                                        if !ui.input(|i| i.modifiers.shift) {
                                            close = true;
                                        }
                                    }
                                }
                            }
                        }
                    });
                }
            });
            ui.separator();
            // Unicode emoji are named by their character; show their :shortcode: instead.
            let footer = hovered.map(|n| format!(":{}:", catalog.name_of(&n).unwrap_or(&n))).unwrap_or_default();
            ui.label(RichText::new(footer).color(theme::MUTED()));
        });
    });
    // Clicking anywhere else closes it.
    if ctx.input(|i| i.pointer.any_pressed()) && !tone_open && !area.response.contains_pointer() && !ctx.is_being_dragged(area.response.id) {
        let just_opened = ctx.memory(|m| m.data.get_temp::<bool>(egui::Id::unique("emoji-picker-open")).is_none());
        if !just_opened {
            close = true;
        }
    }
    ctx.memory_mut(|m| m.data.insert_temp(egui::Id::unique("emoji-picker-open"), true));
    if close {
        uis.emoji_picker = None;
        uis.emoji_search.clear();
        ctx.memory_mut(|m| m.data.remove::<bool>(egui::Id::unique("emoji-picker-open")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_the_bundled_table_and_searches_it() {
        let c = Catalog::load();
        assert!(c.unicode.len() > 1500, "{} emoji", c.unicode.len());
        assert!(c.groups.len() >= 8);
        let hits = c.search("thumbs", 5);
        assert!(hits.iter().any(|(ch, _)| ch.starts_with('👍')), "{hits:?}");
    }

    fn catalog() -> Catalog {
        let mut c = Catalog::load();
        c.skin_tone = 0;
        c
    }

    #[test]
    fn uses_discord_names() {
        let c = catalog();
        assert_eq!(c.unicode.len(), 1932, "Discord's count of default emoji");
        for (code, emoji) in [("thumbsup", "👍"), ("+1", "👍"), ("slight_smile", "🙂"), ("flag_us", "🇺🇸"), ("heart", "❤️"), ("regional_indicator_a", "🇦"), ("face_with_bags_under_eyes", "🫩")] {
            assert_eq!(c.by_shortcode(code).as_deref(), Some(emoji), "{code}");
        }
        assert_eq!(c.by_shortcode("thumbsup::skin-tone-2").as_deref(), Some("👍🏼"));
        assert_eq!(c.by_shortcode("thumbsup_tone5").as_deref(), Some("👍🏿"));
        assert_eq!(c.name_of("👍🏽"), Some("thumbsup"));
        assert_eq!(c.search("slight_sm", 1)[0].0, "🙂");
    }

    #[test]
    fn skin_tone_applies_to_search_and_shortcodes() {
        let mut c = catalog();
        c.skin_tone = 3;
        assert_eq!(c.by_shortcode("wave").as_deref(), Some("👋🏽"));
        assert_eq!(c.search("thumbsup", 1)[0].0, "👍🏽");
        // Emoji without tones are unaffected.
        assert_eq!(c.by_shortcode("joy").as_deref(), Some("😂"));
    }

    #[test]
    fn expands_shortcodes_outside_code() {
        let c = catalog();
        let custom = |n: &str| (n == "blob").then(|| "<:blob:1>".to_owned());
        assert_eq!(expand_shortcodes("hi :wave: :thumbsup::skin-tone-1: :blob: :nope:", &c, custom), "hi 👋 👍🏻 <:blob:1> :nope:");
        assert_eq!(expand_shortcodes("`:wave:` ```\n:wave:\n``` :wave:", &c, custom), "`:wave:` ```\n:wave:\n``` 👋");
        assert_eq!(expand_shortcodes("10:30:45 and a:b", &c, custom), "10:30:45 and a:b");
        assert_eq!(expand_shortcodes("<:wave:12> <@34> https://x.com/:b:/ :b:", &c, custom), "<:wave:12> <@34> https://x.com/:b:/ 🅱️");
    }

    /// Lists emoji (and skin tones) without a Twemoji picture or not recognised in messages, given a list of
    /// Twemoji asset names (`SWIFTCORD_TWEMOJI_LIST`, one per line, e.g. from jsDelivr's file listing).
    #[test]
    fn audit_coverage() {
        let Ok(list) = std::env::var("SWIFTCORD_TWEMOJI_LIST") else { return };
        let names: std::collections::HashSet<String> = std::fs::read_to_string(list).unwrap().lines().map(|l| l.trim().trim_start_matches('\u{feff}').to_owned()).collect();
        let c = Catalog::load();
        let (mut no_image, mut not_detected) = (Vec::new(), Vec::new());
        for u in &c.unicode {
            for ch in std::iter::once(&u.ch).chain(u.skins.iter().flatten()) {
                if !names.contains(&swiftcord_core::markdown::twemoji_name(ch)) {
                    no_image.push(format!("{ch} {}", u.name));
                }
                if !swiftcord_core::markdown::is_emoji_grapheme(ch) {
                    not_detected.push(format!("{ch} {}", u.name));
                }
            }
        }
        println!("no image: {} {:?}", no_image.len(), &no_image[..no_image.len().min(60)]);
        println!("not detected: {} {:?}", not_detected.len(), &not_detected[..not_detected.len().min(60)]);
    }
}
