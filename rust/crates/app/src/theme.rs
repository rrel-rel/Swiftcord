//! Colours, fonts and egui styling. The colours come from the chosen palette (Dark, Darker or Midnight) and
//! accent; they're functions so the theme can change while the app runs.
#![allow(non_snake_case)]

use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId, TextStyle};
use serde::{Deserialize, Serialize};

/// The background shades: how dark the app is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Palette {
    /// Discord's dark theme.
    #[default]
    Dark,
    /// Deeper greys.
    Darker,
    /// Near-black, for OLED screens and dark rooms.
    Midnight,
}

impl Palette {
    pub const ALL: [Palette; 3] = [Palette::Dark, Palette::Darker, Palette::Midnight];

    pub fn name(self) -> &'static str {
        match self {
            Palette::Dark => "Dark",
            Palette::Darker => "Darker",
            Palette::Midnight => "Midnight",
        }
    }
}

/// The accent colours offered (buttons, selection, links to act on), Discord's blurple first.
pub const ACCENTS: [(&str, u32); 6] =
    [("Blurple", 0x5865F2), ("Green", 0x23A55A), ("Pink", 0xEB459E), ("Orange", 0xE67E22), ("Teal", 0x1ABC9C), ("Red", 0xED4245)];

/// The shades of one palette.
struct Shades {
    rail: Color32,
    sidebar: Color32,
    chat: Color32,
    input: Color32,
    hover: Color32,
    selected: Color32,
    message_hover: Color32,
    /// Embeds and code blocks.
    card: Color32,
}

const fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

const DARK: Shades = Shades {
    rail: rgb(0x1e1f22),
    sidebar: rgb(0x2b2d31),
    chat: rgb(0x313338),
    input: rgb(0x383a40),
    hover: rgb(0x35373c),
    selected: rgb(0x404249),
    message_hover: rgb(0x2e3035),
    card: rgb(0x2b2d31),
};

const DARKER: Shades = Shades {
    rail: rgb(0x111214),
    sidebar: rgb(0x1a1b1e),
    chat: rgb(0x1f2023),
    input: rgb(0x292a2e),
    hover: rgb(0x26272b),
    selected: rgb(0x303136),
    message_hover: rgb(0x1b1c1f),
    card: rgb(0x18191c),
};

const MIDNIGHT: Shades = Shades {
    rail: rgb(0x000000),
    sidebar: rgb(0x08080a),
    chat: rgb(0x000000),
    input: rgb(0x16171a),
    hover: rgb(0x121315),
    selected: rgb(0x1d1e22),
    message_hover: rgb(0x0b0b0d),
    card: rgb(0x0d0e10),
};

static PALETTE: AtomicU8 = AtomicU8::new(0);
static ACCENT: AtomicU32 = AtomicU32::new(0x5865F2);

/// Switches the colours; [`apply_visuals`] then restyles egui's own widgets.
pub fn set(palette: Palette, accent: u32) {
    PALETTE.store(palette as u8, Ordering::Relaxed);
    ACCENT.store(accent & 0xFF_FFFF, Ordering::Relaxed);
}

fn shades() -> &'static Shades {
    match PALETTE.load(Ordering::Relaxed) {
        1 => &DARKER,
        2 => &MIDNIGHT,
        _ => &DARK,
    }
}

pub fn RAIL() -> Color32 {
    shades().rail
}
pub fn SIDEBAR() -> Color32 {
    shades().sidebar
}
pub fn CHAT() -> Color32 {
    shades().chat
}
pub fn INPUT() -> Color32 {
    shades().input
}
pub fn HOVER() -> Color32 {
    shades().hover
}
pub fn SELECTED() -> Color32 {
    shades().selected
}
pub fn MESSAGE_HOVER() -> Color32 {
    shades().message_hover
}
pub fn CODE_BG() -> Color32 {
    shades().card
}
pub fn EMBED_BG() -> Color32 {
    shades().card
}
pub fn TEXT() -> Color32 {
    rgb(0xdbdee1)
}
pub fn MUTED() -> Color32 {
    rgb(0x949ba4)
}
pub fn FAINT() -> Color32 {
    rgb(0x6d6f78)
}
pub fn HEADER() -> Color32 {
    rgb(0xf2f3f5)
}
/// The accent colour (Discord's blurple unless another was chosen).
pub fn BLURPLE() -> Color32 {
    rgb(ACCENT.load(Ordering::Relaxed))
}
pub fn GREEN() -> Color32 {
    rgb(0x23a559)
}
pub fn YELLOW() -> Color32 {
    rgb(0xf0b232)
}
pub fn RED() -> Color32 {
    rgb(0xf23f43)
}
/// The offline / invisible status.
pub fn OFFLINE() -> Color32 {
    rgb(0x80848e)
}
pub fn LINK() -> Color32 {
    rgb(0x00a8fc)
}
/// Mentions: a dim wash of the accent behind, a pale tint of it for the text.
pub fn MENTION_BG() -> Color32 {
    BLURPLE().lerp_to_gamma(SIDEBAR(), 0.62)
}
pub fn MENTION_TEXT() -> Color32 {
    BLURPLE().lerp_to_gamma(Color32::WHITE, 0.7)
}
/// Discord's gold tint for messages that mention you.
pub fn MENTIONED_ROW() -> Color32 {
    rgb(0xf0b232).lerp_to_gamma(CHAT(), 0.9)
}
pub fn MENTIONED_ROW_HOVER() -> Color32 {
    rgb(0xf0b232).lerp_to_gamma(CHAT(), 0.87)
}

/// Segoe Fluent Icons code points.
pub mod icon {
    pub const MIC: &str = "\u{E720}";
    pub const MIC_OFF: &str = "\u{EC54}";
    pub const HEADPHONES: &str = "\u{E7F6}";
    pub const DEAFENED: &str = "\u{E74F}";
    pub const SETTINGS: &str = "\u{E713}";
    pub const PIN: &str = "\u{E718}";
    pub const ADD: &str = "\u{E710}";
    pub const EMOJI: &str = "\u{E76E}";
    pub const REPLY: &str = "\u{E97A}";
    pub const EDIT: &str = "\u{E70F}";
    pub const DELETE: &str = "\u{E74D}";
    pub const HANG_UP: &str = "\u{E778}";
    pub const SPEAKER: &str = "\u{E767}";
    pub const PEOPLE: &str = "\u{E716}";
    pub const CHEVRON_DOWN: &str = "\u{E70D}";
    pub const CHEVRON_RIGHT: &str = "\u{E76C}";
    pub const CLOSE: &str = "\u{E711}";
    pub const HOME: &str = "\u{E80F}";
    pub const DOWNLOAD: &str = "\u{E896}";
    pub const PLAY: &str = "\u{E768}";
    pub const PAUSE: &str = "\u{E769}";
    pub const SEARCH: &str = "\u{E721}";
    pub const VOLUME: &str = "\u{E767}";
    pub const MUTED: &str = "\u{E74F}";
    pub const FULL_SCREEN: &str = "\u{E740}";
    pub const OPEN_EXTERNAL: &str = "\u{E8A7}";
    pub const REPLAY: &str = "\u{E72C}";
    pub const FOLDER: &str = "\u{E8B7}";
    pub const FILE: &str = "\u{E8A5}";
    pub const SIGNAL: &str = "\u{E701}";
    pub const PHONE: &str = "\u{E717}";
    pub const CHAT: &str = "\u{E8BD}";
    /// A monitor: screens, and going live.
    pub const SCREEN: &str = "\u{E7F4}";
    pub const CAMERA: &str = "\u{E714}";
    pub const WINDOW: &str = "\u{E7C4}";
    pub const STAR: &str = "\u{E734}";
    pub const STAR_FILLED: &str = "\u{E735}";
}

pub fn icons() -> FontFamily {
    FontFamily::Name("icons".into())
}

pub fn icon_font(size: f32) -> FontId {
    FontId::new(size, icons())
}

pub fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}

pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

pub fn font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn font_bold(size: f32) -> FontId {
    FontId::new(size, bold())
}

pub fn font_semibold(size: f32) -> FontId {
    FontId::new(size, semibold())
}

/// Memory-maps a system font: the file costs nothing until glyphs are read from it, so large CJK fallbacks
/// don't bloat the working set.
fn system_font(file: &str, index: u32) -> Option<FontData> {
    let path = std::path::Path::new(r"C:\Windows\Fonts").join(file);
    let f = std::fs::File::open(path).ok()?;
    let map = unsafe { memmap2::Mmap::map(&f) }.ok()?;
    let bytes: &'static [u8] = &Box::leak(Box::new(map))[..];
    let mut data = FontData::from_static(bytes);
    data.index = index;
    Some(data)
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let mut add = |name: &str, file: &str, index: u32| -> Option<String> {
        system_font(file, index).map(|d| {
            fonts.font_data.insert(name.to_owned(), Arc::new(d));
            name.to_owned()
        })
    };

    let regular = add("segoe", "segoeui.ttf", 0);
    let bold = add("segoe-bold", "segoeuib.ttf", 0);
    let semibold = add("segoe-semibold", "seguisb.ttf", 0);
    let mono = add("consolas", "consola.ttf", 0);
    let icons = add("fluent-icons", "SegoeIcons.ttf", 0).or_else(|| add("mdl2", "segmdl2.ttf", 0));
    // Fallbacks in order: symbols, then Simplified Chinese (covers most CJK), Japanese and Korean.
    let fallbacks: Vec<String> = [
        add("symbols", "seguisym.ttf", 0),
        add("yahei", "msyh.ttc", 0),
        add("yugothic", "YuGothM.ttc", 0),
        add("malgun", "malgun.ttf", 0),
    ]
    .into_iter()
    .flatten()
    .collect();

    // egui's bundled fonts stay at the end of each list as a last resort (and for its own icons).
    let builtin_prop = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let builtin_mono = fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    let family = |primary: Option<String>, builtin: &[String]| -> Vec<String> {
        primary.into_iter().chain(fallbacks.iter().cloned()).chain(builtin.iter().cloned()).collect()
    };
    fonts.families.insert(FontFamily::Proportional, family(regular.clone(), &builtin_prop));
    fonts.families.insert(FontFamily::Monospace, family(mono, &builtin_mono));
    fonts.families.insert(self::bold(), family(bold.or(regular.clone()), &builtin_prop));
    fonts.families.insert(self::semibold(), family(semibold.or(regular), &builtin_prop));
    fonts.families.insert(self::icons(), icons.into_iter().chain(builtin_prop.iter().cloned()).collect());
    ctx.set_fonts(fonts);
    apply_visuals(ctx);
}

/// Styles egui's own widgets in the current palette and accent (again after either changes).
pub fn apply_visuals(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        style.text_styles = [
            (TextStyle::Small, font(12.0)),
            (TextStyle::Body, font(15.0)),
            (TextStyle::Button, font(14.0)),
            (TextStyle::Heading, font_bold(20.0)),
            (TextStyle::Monospace, FontId::monospace(13.5)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.scroll.bar_width = 8.0;
        style.spacing.scroll.floating = true;
        style.interaction.selectable_labels = true;

        let v = &mut style.visuals;
        *v = egui::Visuals::dark();
        v.override_text_color = Some(TEXT());
        v.panel_fill = CHAT();
        v.window_fill = SIDEBAR();
        v.extreme_bg_color = INPUT();
        v.faint_bg_color = SIDEBAR();
        v.code_bg_color = CODE_BG();
        v.hyperlink_color = LINK();
        v.selection.bg_fill = BLURPLE().gamma_multiply(0.6);
        v.window_corner_radius = 8.into();
        v.menu_corner_radius = 6.into();
        v.window_stroke = egui::Stroke::new(1.0, Color32::from_gray(30));
        for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.noninteractive] {
            w.corner_radius = 4.into();
        }
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(0x4e, 0x50, 0x58);
        v.widgets.inactive.bg_fill = Color32::from_rgb(0x4e, 0x50, 0x58);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x5d, 0x60, 0x6b);
        v.widgets.active.weak_bg_fill = Color32::from_rgb(0x6d, 0x6f, 0x78);
        v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(0x3f, 0x41, 0x47));
    });
}

pub fn role_color(c: Option<u32>) -> Color32 {
    match c {
        Some(c) if c != 0 => Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8),
        _ => HEADER(),
    }
}
