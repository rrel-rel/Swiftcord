//! The look: Source engine menus of the late 2000s (VGUI). Flat olive-grey panels, near-white Tahoma
//! text, controls outlined with a light 1 px border, square corners throughout, and Source's orange as
//! the one accent.
#![allow(non_snake_case)]

use std::sync::Arc;

use egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId, TextStyle};

const fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// The darkest shade: the server rail, the user panel, dark dividers.
pub fn RAIL() -> Color32 {
    rgb(0x3e3f3b)
}
pub fn SIDEBAR() -> Color32 {
    rgb(0x4a4b47)
}
/// The main panel, like a VGUI frame.
pub fn CHAT() -> Color32 {
    rgb(0x585954)
}
/// Text fields and drop-downs, a shade darker than the panel they sit in.
pub fn INPUT() -> Color32 {
    rgb(0x424340)
}
pub fn HOVER() -> Color32 {
    rgb(0x60615c)
}
pub fn SELECTED() -> Color32 {
    rgb(0x6e6f69)
}
pub fn MESSAGE_HOVER() -> Color32 {
    rgb(0x51524d)
}
pub fn CODE_BG() -> Color32 {
    rgb(0x464743)
}
pub fn EMBED_BG() -> Color32 {
    rgb(0x4a4b47)
}
/// The light edge VGUI draws around buttons and fields.
pub fn BORDER() -> Color32 {
    rgb(0x8c8d87)
}
pub fn TEXT() -> Color32 {
    rgb(0xdcdcd6)
}
pub fn MUTED() -> Color32 {
    rgb(0xa2a39d)
}
pub fn FAINT() -> Color32 {
    rgb(0x7f807a)
}
pub fn HEADER() -> Color32 {
    rgb(0xffffff)
}
/// Source's orange: selection, the home button, primary buttons.
pub fn ACCENT() -> Color32 {
    rgb(0xc47f1e)
}
/// Steam's in-game green.
pub fn GREEN() -> Color32 {
    rgb(0x8db24a)
}
pub fn YELLOW() -> Color32 {
    rgb(0xd8b23c)
}
pub fn RED() -> Color32 {
    rgb(0xc8483a)
}
/// The offline / invisible status.
pub fn OFFLINE() -> Color32 {
    rgb(0x7a7b76)
}
/// Links are white and underlined, as in VGUI.
pub fn LINK() -> Color32 {
    rgb(0xffffff)
}
/// Mentions: a dim wash of the accent behind, a pale tint of it for the text.
pub fn MENTION_BG() -> Color32 {
    ACCENT().lerp_to_gamma(SIDEBAR(), 0.6)
}
pub fn MENTION_TEXT() -> Color32 {
    ACCENT().lerp_to_gamma(Color32::WHITE, 0.7)
}
/// A gold tint for messages that mention you.
pub fn MENTIONED_ROW() -> Color32 {
    YELLOW().lerp_to_gamma(CHAT(), 0.88)
}
pub fn MENTIONED_ROW_HOVER() -> Color32 {
    YELLOW().lerp_to_gamma(CHAT(), 0.84)
}
/// The outline for text fields and boxes.
pub fn field_stroke() -> egui::Stroke {
    egui::Stroke::new(1.0, BORDER())
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
    pub const INBOX: &str = "\u{E715}";
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

    let regular = add("tahoma", "tahoma.ttf", 0);
    let bold = add("tahoma-bold", "tahomabd.ttf", 0);
    let mono = add("consolas", "consola.ttf", 0);
    let icons = add("fluent-icons", "SegoeIcons.ttf", 0).or_else(|| add("mdl2", "segmdl2.ttf", 0));
    // Fallbacks in order: Segoe UI (Tahoma lacks some punctuation and symbols), symbols, emoji (drawn in one colour, like any text), then Simplified Chinese
    // (covers most CJK), Japanese and Korean.
    let fallbacks: Vec<String> = [
        add("segoe", "segoeui.ttf", 0),
        add("symbols", "seguisym.ttf", 0),
        add("emoji", "seguiemj.ttf", 0),
        add("yahei", "msyh.ttc", 0),
        add("yugothic", "YuGothM.ttc", 0),
        add("malgun", "malgun.ttf", 0),
    ]
    .into_iter()
    .flatten()
    .collect();

    let family = |primary: Option<String>| -> Vec<String> { primary.into_iter().chain(fallbacks.iter().cloned()).collect() };
    fonts.families.insert(FontFamily::Proportional, family(regular.clone()));
    fonts.families.insert(FontFamily::Monospace, family(mono));
    fonts.families.insert(self::bold(), family(bold.clone().or(regular.clone())));
    // Tahoma has no semibold; VGUI used its bold.
    fonts.families.insert(self::semibold(), family(bold.clone().or(regular.clone())));
    fonts.families.insert(self::icons(), family(icons));
    ctx.set_fonts(fonts);
    apply_visuals(ctx);
}

/// Styles egui's own widgets as VGUI controls.
fn apply_visuals(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        style.text_styles = [
            (TextStyle::Small, font(11.5)),
            (TextStyle::Body, font(14.0)),
            (TextStyle::Button, font(13.5)),
            (TextStyle::Heading, font_bold(18.0)),
            (TextStyle::Monospace, FontId::monospace(13.5)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(10.0, 4.0);
        // Solid bars in their own gutter, not overlays.
        style.spacing.scroll = egui::style::ScrollStyle::solid();
        style.spacing.scroll.bar_width = 10.0;
        // Interface text keeps the arrow cursor; messages and profiles turn selection back on.
        style.interaction.selectable_labels = false;

        let v = &mut style.visuals;
        *v = egui::Visuals::dark();
        v.override_text_color = Some(TEXT());
        v.panel_fill = CHAT();
        v.window_fill = SIDEBAR();
        v.extreme_bg_color = INPUT();
        v.faint_bg_color = SIDEBAR();
        v.code_bg_color = CODE_BG();
        v.hyperlink_color = LINK();
        v.selection.bg_fill = ACCENT().gamma_multiply(0.7);
        v.selection.stroke = egui::Stroke::new(1.0, HEADER());
        v.window_corner_radius = 0.into();
        v.menu_corner_radius = 0.into();
        v.window_stroke = field_stroke();
        v.window_shadow = egui::Shadow::NONE;
        v.popup_shadow = egui::Shadow::NONE;
        // Buttons: a shade darker than the panel, outlined; brighter when pointed at, sunk when pressed.
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = SIDEBAR();
        w.noninteractive.bg_stroke = egui::Stroke::new(1.0, rgb(0x6e6f69));
        w.noninteractive.fg_stroke = egui::Stroke::new(1.0, TEXT());
        w.inactive.bg_fill = rgb(0x4e4f4a);
        w.inactive.weak_bg_fill = rgb(0x4e4f4a);
        w.inactive.bg_stroke = field_stroke();
        w.inactive.fg_stroke = egui::Stroke::new(1.0, TEXT());
        w.hovered.bg_fill = rgb(0x5c5d58);
        w.hovered.weak_bg_fill = rgb(0x5c5d58);
        w.hovered.bg_stroke = egui::Stroke::new(1.0, rgb(0xc4c5be));
        w.hovered.fg_stroke = egui::Stroke::new(1.0, HEADER());
        w.active.bg_fill = RAIL();
        w.active.weak_bg_fill = RAIL();
        w.active.bg_stroke = egui::Stroke::new(1.0, rgb(0x2a2a28));
        w.active.fg_stroke = egui::Stroke::new(1.0, HEADER());
        w.open = w.active;
        for s in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            s.corner_radius = 0.into();
            s.expansion = 0.0;
        }
    });
}

/// VGUI has no rounded corners: squares off every rectangle (panels, buttons, highlights, avatars) before
/// it's drawn, wherever it came from.
pub fn square_corners(shapes: &mut [egui::epaint::ClippedShape]) {
    fn square(shape: &mut egui::Shape) {
        match shape {
            egui::Shape::Rect(r) => r.corner_radius = egui::CornerRadius::ZERO,
            egui::Shape::Vec(v) => v.iter_mut().for_each(square),
            _ => {}
        }
    }
    for s in shapes {
        square(&mut s.shape);
    }
}

pub fn role_color(c: Option<u32>) -> Color32 {
    match c {
        Some(c) if c != 0 => Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8),
        _ => HEADER(),
    }
}
