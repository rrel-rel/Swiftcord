//! Colours, fonts and egui styling.

use std::sync::Arc;

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId, TextStyle};

pub const RAIL: Color32 = Color32::from_rgb(0x1e, 0x1f, 0x22);
pub const SIDEBAR: Color32 = Color32::from_rgb(0x2b, 0x2d, 0x31);
pub const CHAT: Color32 = Color32::from_rgb(0x31, 0x33, 0x38);
pub const INPUT: Color32 = Color32::from_rgb(0x38, 0x3a, 0x40);
pub const HOVER: Color32 = Color32::from_rgb(0x35, 0x37, 0x3c);
pub const SELECTED: Color32 = Color32::from_rgb(0x40, 0x42, 0x49);
pub const MESSAGE_HOVER: Color32 = Color32::from_rgb(0x2e, 0x30, 0x35);
pub const TEXT: Color32 = Color32::from_rgb(0xdb, 0xde, 0xe1);
pub const MUTED: Color32 = Color32::from_rgb(0x94, 0x9b, 0xa4);
pub const FAINT: Color32 = Color32::from_rgb(0x6d, 0x6f, 0x78);
pub const HEADER: Color32 = Color32::from_rgb(0xf2, 0xf3, 0xf5);
pub const BLURPLE: Color32 = Color32::from_rgb(0x58, 0x65, 0xf2);
pub const GREEN: Color32 = Color32::from_rgb(0x23, 0xa5, 0x59);
pub const YELLOW: Color32 = Color32::from_rgb(0xf0, 0xb2, 0x32);
pub const RED: Color32 = Color32::from_rgb(0xf2, 0x3f, 0x43);
pub const LINK: Color32 = Color32::from_rgb(0x00, 0xa8, 0xfc);
pub const MENTION_BG: Color32 = Color32::from_rgba_premultiplied(0x3c, 0x42, 0x70, 0xff);
pub const MENTION_TEXT: Color32 = Color32::from_rgb(0xc9, 0xcd, 0xfb);
/// Discord's gold tint for messages that mention you.
pub const MENTIONED_ROW: Color32 = Color32::from_rgb(0x44, 0x3d, 0x2d);
pub const MENTIONED_ROW_HOVER: Color32 = Color32::from_rgb(0x4a, 0x42, 0x2f);
pub const CODE_BG: Color32 = Color32::from_rgb(0x2b, 0x2d, 0x31);
pub const EMBED_BG: Color32 = Color32::from_rgb(0x2b, 0x2d, 0x31);

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
    pub const FOLDER: &str = "\u{E8B7}";
    pub const FILE: &str = "\u{E8A5}";
    pub const SIGNAL: &str = "\u{E701}";
    pub const PHONE: &str = "\u{E717}";
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
        v.override_text_color = Some(TEXT);
        v.panel_fill = CHAT;
        v.window_fill = SIDEBAR;
        v.extreme_bg_color = INPUT;
        v.faint_bg_color = SIDEBAR;
        v.code_bg_color = CODE_BG;
        v.hyperlink_color = LINK;
        v.selection.bg_fill = BLURPLE.gamma_multiply(0.6);
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
        _ => HEADER,
    }
}
