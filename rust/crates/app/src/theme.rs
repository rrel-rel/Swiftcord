//! The look: Source engine menus of the late 2000s (VGUI, as in the Orange Box games). Mid-grey frames
//! with darker fields and lists, light grey Tahoma text with bold titles, controls and
//! dividers drawn with light 1 px lines, square corners, VGUI scrollbars (see `ui::widgets::scrollbar`),
//! and Source's orange only to mark what's selected.
#![allow(non_snake_case)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId, TextStyle};

const fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// One colour scheme. VGUI's bevels aren't listed: they're worked out from `button` (see [`vgui`]).
struct Palette {
    name: &'static str,
    rail: u32,
    sidebar: u32,
    chat: u32,
    input: u32,
    hover: u32,
    message_hover: u32,
    code_bg: u32,
    border: u32,
    text: u32,
    muted: u32,
    faint: u32,
    header: u32,
    accent: u32,
    green: u32,
    yellow: u32,
    red: u32,
    link: u32,
    mention_bg: u32,
    divider: u32,
    button: u32,
    button_hover: u32,
}

/// The first is the default, and is drawn with Source's exact colours.
const PALETTES: &[Palette] = &[
    Palette { name: "Steam", rail: 0x575757, sidebar: 0x636363, chat: 0x6e6e6e, input: 0x4c4c4c, hover: 0x7a7a7a, message_hover: 0x686868, code_bg: 0x5a5a5a, border: 0xa8a8a8, text: 0xdedede, muted: 0xb2b2b2, faint: 0x949494, header: 0xffffff, accent: 0xff9c00, green: 0x6fbf4a, yellow: 0xe0c040, red: 0xd04a3c, link: 0xffffff, mention_bg: 0x8a8a8a, divider: 0x9c9c9c, button: 0x656565, button_hover: 0x787878 },
    Palette { name: "Steam Dark", rail: 0x434343, sidebar: 0x4f4f4f, chat: 0x5a5a5a, input: 0x3a3a3a, hover: 0x666666, message_hover: 0x545454, code_bg: 0x464646, border: 0x949494, text: 0xdedede, muted: 0xb2b2b2, faint: 0x969696, header: 0xffffff, accent: 0xff9c00, green: 0x6fbf4a, yellow: 0xe0c040, red: 0xd04a3c, link: 0xffffff, mention_bg: 0x767676, divider: 0x888888, button: 0x515151, button_hover: 0x646464 },
    Palette { name: "Dracula", rail: 0x191a21, sidebar: 0x21222c, chat: 0x282a36, input: 0x191a21, hover: 0x44475a, message_hover: 0x303241, code_bg: 0x21222c, border: 0x6272a4, text: 0xf8f8f2, muted: 0xc8c9d6, faint: 0x8f93b5, header: 0xffffff, accent: 0xbd93f9, green: 0x50fa7b, yellow: 0xf1fa8c, red: 0xff5555, link: 0x8be9fd, mention_bg: 0x44475a, divider: 0x44475a, button: 0x343746, button_hover: 0x44475a },
    Palette { name: "Nord", rail: 0x242933, sidebar: 0x292e39, chat: 0x2e3440, input: 0x272c36, hover: 0x434c5e, message_hover: 0x353b49, code_bg: 0x272c36, border: 0x4c566a, text: 0xd8dee9, muted: 0xb0b8c6, faint: 0x8791a3, header: 0xeceff4, accent: 0x88c0d0, green: 0xa3be8c, yellow: 0xebcb8b, red: 0xbf616a, link: 0x88c0d0, mention_bg: 0x4c566a, divider: 0x434c5e, button: 0x3b4252, button_hover: 0x434c5e },
    Palette { name: "Gruvbox Dark", rail: 0x1d2021, sidebar: 0x242322, chat: 0x282828, input: 0x1d2021, hover: 0x3c3836, message_hover: 0x32302f, code_bg: 0x1d2021, border: 0x665c54, text: 0xebdbb2, muted: 0xd5c4a1, faint: 0xa89984, header: 0xfbf1c7, accent: 0xfe8019, green: 0xb8bb26, yellow: 0xfabd2f, red: 0xfb4934, link: 0x83a598, mention_bg: 0x504945, divider: 0x504945, button: 0x3c3836, button_hover: 0x504945 },
    Palette { name: "Monokai", rail: 0x1e1f1c, sidebar: 0x23241f, chat: 0x272822, input: 0x1e1f1c, hover: 0x3e3d32, message_hover: 0x31322b, code_bg: 0x1e1f1c, border: 0x75715e, text: 0xf8f8f2, muted: 0xcfcfc2, faint: 0x9e9b8a, header: 0xffffff, accent: 0xfd971f, green: 0xa6e22e, yellow: 0xe6db74, red: 0xf92672, link: 0x66d9ef, mention_bg: 0x49483e, divider: 0x49483e, button: 0x3e3d32, button_hover: 0x49483e },
    Palette { name: "One Dark", rail: 0x1e2127, sidebar: 0x21252b, chat: 0x282c34, input: 0x1e2127, hover: 0x3e4451, message_hover: 0x2f333d, code_bg: 0x21252b, border: 0x5c6370, text: 0xabb2bf, muted: 0x9da5b4, faint: 0x7f8694, header: 0xe6e6e6, accent: 0x61afef, green: 0x98c379, yellow: 0xe5c07b, red: 0xe06c75, link: 0x61afef, mention_bg: 0x3e4451, divider: 0x3e4451, button: 0x3a3f4b, button_hover: 0x4b5263 },
    Palette { name: "Solarized Dark", rail: 0x00212b, sidebar: 0x00252f, chat: 0x002b36, input: 0x00212b, hover: 0x0a404e, message_hover: 0x03313d, code_bg: 0x073642, border: 0x586e75, text: 0x93a1a1, muted: 0x839496, faint: 0x6f8186, header: 0xeee8d5, accent: 0x268bd2, green: 0x859900, yellow: 0xb58900, red: 0xdc322f, link: 0x2aa198, mention_bg: 0x0b4a5a, divider: 0x0b3f4c, button: 0x073642, button_hover: 0x0a4a59 },
    Palette { name: "Tokyo Night", rail: 0x13141c, sidebar: 0x16161e, chat: 0x1a1b26, input: 0x13141c, hover: 0x292e42, message_hover: 0x1f2030, code_bg: 0x16161e, border: 0x414868, text: 0xc0caf5, muted: 0xa9b1d6, faint: 0x7982a9, header: 0xe4e8ff, accent: 0x7aa2f7, green: 0x9ece6a, yellow: 0xe0af68, red: 0xf7768e, link: 0x7dcfff, mention_bg: 0x33467c, divider: 0x292e42, button: 0x24283b, button_hover: 0x2f3549 },
    Palette { name: "Catppuccin Mocha", rail: 0x11111b, sidebar: 0x181825, chat: 0x1e1e2e, input: 0x11111b, hover: 0x313244, message_hover: 0x262637, code_bg: 0x181825, border: 0x585b70, text: 0xcdd6f4, muted: 0xbac2de, faint: 0x9399b2, header: 0xe6ecff, accent: 0xcba6f7, green: 0xa6e3a1, yellow: 0xf9e2af, red: 0xf38ba8, link: 0x89b4fa, mention_bg: 0x45475a, divider: 0x313244, button: 0x313244, button_hover: 0x45475a },
];

/// Which palette is showing, an index into [`PALETTES`].
static CURRENT: AtomicUsize = AtomicUsize::new(0);

fn palette() -> &'static Palette {
    &PALETTES[CURRENT.load(Ordering::Relaxed)]
}

/// The palettes' names, for the settings window.
pub fn names() -> impl Iterator<Item = &'static str> {
    PALETTES.iter().map(|p| p.name)
}

/// Shows the named palette (an unknown name is the default) from the next frame.
pub fn select(name: &str, ctx: &egui::Context) {
    CURRENT.store(PALETTES.iter().position(|p| p.name == name).unwrap_or(0), Ordering::Relaxed);
    apply_visuals(ctx);
    ctx.request_repaint();
}

/// One of Source's greys (from its bevels and scrollbars), as it comes out in this palette: mid grey is the
/// button face, darker greys go towards a deep shadow, lighter ones towards the brightest text.
pub fn vgui(c: u32) -> Color32 {
    const FACE: f32 = 0x7d as f32;
    let p = palette();
    if std::ptr::eq(p, &PALETTES[0]) {
        return rgb(c);
    }
    let grey = ((c >> 8) & 0xff) as f32;
    let (face, shadow) = (rgb(p.button), rgb(p.rail).lerp_to_gamma(Color32::BLACK, 0.6));
    if grey < FACE { shadow.lerp_to_gamma(face, grey / FACE) } else { face.lerp_to_gamma(rgb(p.header), (grey - FACE) / (255.0 - FACE)) }
}

/// The darkest shade: the server rail, the user panel, dark dividers.
pub fn RAIL() -> Color32 {
    rgb(palette().rail)
}
pub fn SIDEBAR() -> Color32 {
    rgb(palette().sidebar)
}
/// The main panel, like a VGUI frame.
pub fn CHAT() -> Color32 {
    rgb(palette().chat)
}
/// Text fields and drop-downs, a shade darker than the panel they sit in.
pub fn INPUT() -> Color32 {
    rgb(palette().input)
}
pub fn HOVER() -> Color32 {
    rgb(palette().hover)
}
pub fn SELECTED() -> Color32 {
    rgb(palette().hover)
}
pub fn MESSAGE_HOVER() -> Color32 {
    rgb(palette().message_hover)
}
pub fn CODE_BG() -> Color32 {
    rgb(palette().code_bg)
}
pub fn EMBED_BG() -> Color32 {
    rgb(palette().sidebar)
}
/// The light edge VGUI draws around buttons and fields.
pub fn BORDER() -> Color32 {
    rgb(palette().border)
}
pub fn TEXT() -> Color32 {
    rgb(palette().text)
}
pub fn MUTED() -> Color32 {
    rgb(palette().muted)
}
pub fn FAINT() -> Color32 {
    rgb(palette().faint)
}
pub fn HEADER() -> Color32 {
    rgb(palette().header)
}
/// Source's orange, or the palette's accent: what's selected (the open channel's name, selected text), progress, focus.
pub fn ACCENT() -> Color32 {
    rgb(palette().accent)
}
/// Online.
pub fn GREEN() -> Color32 {
    rgb(palette().green)
}
pub fn YELLOW() -> Color32 {
    rgb(palette().yellow)
}
pub fn RED() -> Color32 {
    rgb(palette().red)
}
/// The offline / invisible status.
pub fn OFFLINE() -> Color32 {
    rgb(palette().faint)
}
/// Links are white and underlined, as in VGUI (other palettes have their own colour).
pub fn LINK() -> Color32 {
    rgb(palette().link)
}
/// Mentions: a lighter chip with bright text.
pub fn MENTION_BG() -> Color32 {
    rgb(palette().mention_bg)
}
pub fn MENTION_TEXT() -> Color32 {
    HEADER()
}
/// The light lines VGUI separates sections with.
pub fn DIVIDER() -> egui::Stroke {
    egui::Stroke::new(1.0, rgb(palette().divider))
}

/// How wide scrollbars are; scroll areas keep this much room on their right for one.
pub const SCROLLBAR: f32 = 17.0;
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

/// Memory-maps a system font, drawn at `scale` times the size asked for: the file costs nothing until glyphs
/// are read from it, so large CJK fallbacks don't bloat the working set.
fn system_font(file: &str, index: u32, scale: f32) -> Option<FontData> {
    let path = std::path::Path::new(r"C:\Windows\Fonts").join(file);
    let f = std::fs::File::open(path).ok()?;
    let map = unsafe { memmap2::Mmap::map(&f) }.ok()?;
    let bytes: &'static [u8] = &Box::leak(Box::new(map))[..];
    let mut data = FontData::from_static(bytes);
    data.index = index;
    data.tweak.scale = scale;
    Some(data)
}

/// Tahoma's size relative to what's asked for.
const TEXT_SCALE: f32 = 0.9;

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let mut add_scaled = |name: &str, file: &str, index: u32, scale: f32| -> Option<String> {
        system_font(file, index, scale).map(|d| {
            fonts.font_data.insert(name.to_owned(), Arc::new(d));
            name.to_owned()
        })
    };

    // Tahoma, a size smaller than the rest of the layout's text sizes suggest, as VGUI's is.
    let regular = add_scaled("tahoma", "tahoma.ttf", 0, TEXT_SCALE);
    let bold = add_scaled("tahoma-bold", "tahomabd.ttf", 0, TEXT_SCALE);
    let mut add = |name: &str, file: &str, index: u32| add_scaled(name, file, index, 1.0);
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
    ctx.add_widget_theme::<egui::widget_style::ButtonStyle>(VguiButtons);
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
        // egui's scrollbars take no room and aren't drawn; `ui::widgets::scrollbar` draws VGUI's in the room
        // kept for them on the right.
        style.spacing.scroll = egui::style::ScrollStyle::solid();
        style.spacing.scroll.bar_width = 0.0;
        style.spacing.scroll.bar_inner_margin = 0.0;
        style.spacing.scroll.bar_outer_margin = 0.0;
        style.spacing.scroll.content_margin = egui::Margin { left: 0, right: SCROLLBAR as i8, top: 0, bottom: 0 };
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
        v.selection.bg_fill = ACCENT();
        v.selection.stroke = egui::Stroke::new(1.0, BORDER());
        // A light caret, not egui's blue.
        v.text_cursor.stroke = egui::Stroke::new(1.0, TEXT());
        v.window_corner_radius = 0.into();
        v.menu_corner_radius = 0.into();
        v.window_stroke = field_stroke();
        v.window_shadow = egui::Shadow::NONE;
        // Sliders' thumbs are upright rectangles, not knobs.
        v.handle_shape = egui::style::HandleShape::Rect { aspect_ratio: 0.5 };
        v.popup_shadow = egui::Shadow::NONE;
        // Buttons: a shade darker than the panel, outlined; brighter when pointed at, sunk when pressed.
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = SIDEBAR();
        w.noninteractive.bg_stroke = DIVIDER();
        w.noninteractive.fg_stroke = egui::Stroke::new(1.0, TEXT());
        w.inactive.bg_fill = rgb(palette().button);
        w.inactive.weak_bg_fill = rgb(palette().button);
        w.inactive.bg_stroke = field_stroke();
        w.inactive.fg_stroke = egui::Stroke::new(1.0, TEXT());
        w.hovered.bg_fill = rgb(palette().button_hover);
        w.hovered.weak_bg_fill = rgb(palette().button_hover);
        w.hovered.bg_stroke = egui::Stroke::new(1.0, vgui(0xd4d4d4));
        w.hovered.fg_stroke = egui::Stroke::new(1.0, HEADER());
        w.active.bg_fill = RAIL();
        w.active.weak_bg_fill = RAIL();
        w.active.bg_stroke = egui::Stroke::new(1.0, vgui(0x2c2c2c));
        w.active.fg_stroke = egui::Stroke::new(1.0, HEADER());
        w.open = w.active;
        for s in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            s.corner_radius = 0.into();
            s.expansion = 0.0;
        }
    });
}

// MARK: Buttons

/// How a VGUI button is drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ButtonLook {
    Default,
    /// Keyboard focus: a dark outline around the raised bevel.
    Focused,
    Pressed,
    /// Raised, with dark text.
    Disabled,
}

/// Fills egui's buttons are given so their frames can be found and redrawn as VGUI bevels before drawing
/// (egui can only fill and outline a frame evenly). Not colours anything shows.
const MARK_UP: Color32 = Color32::from_rgb(0x7c, 0x7e, 0x81);
const MARK_DOWN: Color32 = Color32::from_rgb(0x7c, 0x7e, 0x83);

/// egui's button style with the marker fills (framed buttons outside menus; menus keep a flat highlight).
struct VguiButtons;

impl egui::theme::StyleProvider<egui::widget_style::ButtonStyle> for VguiButtons {
    fn style(&mut self, args: &egui::widget_style::StyleArgs<'_>) -> egui::widget_style::ButtonStyle {
        let mut style: egui::widget_style::ButtonStyle = egui::theme::DefaultStyle.style(args);
        let frame = &mut style.atom_layout.frame;
        if frame.fill != Color32::TRANSPARENT && !args.stack.contained_in(egui::UiKind::Menu) {
            frame.fill = if args.state == egui::widget_style::WidgetState::Active { MARK_DOWN } else { MARK_UP };
            frame.stroke = egui::Stroke::NONE;
            style.atom_layout.text_style.color = HEADER();
        }
        style
    }
}

/// Which look a marked frame stands for, if `fill` is a marker (disabled buttons are drawn faded, so theirs
/// arrives scaled down).
fn marked(fill: Color32, pointer_down_on: bool) -> Option<ButtonLook> {
    let a = fill.a();
    if a == 0 {
        return None;
    }
    let unfaded = |c: u8| (c as u32 * 255 + a as u32 / 2) / a as u32;
    let rgb = [unfaded(fill.r()), unfaded(fill.g()), unfaded(fill.b())];
    let is = |m: Color32| rgb.iter().zip([m.r(), m.g(), m.b()]).all(|(&c, m)| c.abs_diff(m as u32) <= 1);
    let look = if is(MARK_UP) {
        ButtonLook::Default
    } else if is(MARK_DOWN) {
        // egui shows a focused button as active too; it is pressed only with the pointer down on it.
        if pointer_down_on { ButtonLook::Pressed } else { ButtonLook::Focused }
    } else {
        return None;
    };
    Some(if a < 255 { ButtonLook::Disabled } else { look })
}

/// A VGUI button frame over `rect`, pixel for pixel as Source draws it, at whole multiples of its size like
/// the scrollbar.
pub fn button_frame(rect: egui::Rect, look: ButtonLook, ppp: f32) -> egui::Shape {
    let s = ppp.floor().max(1.0);
    let (x0, x1) = ((rect.left() * ppp).round(), (rect.right() * ppp).round());
    let (y0, y1) = ((rect.top() * ppp).round(), (rect.bottom() * ppp).round());
    let mut spans: Vec<(f32, f32, f32, f32, u32)> = Vec::new();
    // Design pixels counted from each side: l(i)/r(i) are the i-th columns from the left/right, t/b rows.
    let l = |i: f32| x0 + i * s;
    let r = |i: f32| x1 - (i + 1.0) * s;
    let t = |j: f32| y0 + j * s;
    let b = |j: f32| y1 - (j + 1.0) * s;
    let mut fill = |a: f32, b: f32, c: f32, d: f32, color: u32| spans.push((a, b, c, d, color));
    let mut corners: Vec<(f32, f32, u32)> = Vec::new();
    match look {
        ButtonLook::Default | ButtonLook::Disabled => {
            fill(x0, y0, x1, y1, 0x7d7e7f);
            fill(l(0.0), t(0.0), r(0.0), t(1.0), 0xbbbbbb);
            fill(l(0.0), t(1.0), l(1.0), b(0.0), 0xbbbbbb);
            fill(r(0.0), t(0.0), x1, b(0.0), 0x464646);
            fill(l(0.0), b(0.0), r(0.0), y1, 0x464646);
            corners.extend([(l(0.0), t(0.0), 0xc6c6c6), (r(0.0), b(0.0), 0x2d2d2e)]);
        }
        ButtonLook::Pressed => {
            fill(x0, y0, x1, y1, 0x808080);
            fill(l(0.0), t(0.0), x1, t(1.0), 0x474747);
            fill(l(0.0), t(1.0), l(1.0), b(0.0), 0x474747);
            fill(l(0.0), b(0.0), x1, y1, 0xbbbbbb);
            fill(r(0.0), t(1.0), x1, b(0.0), 0xbbbbbb);
            corners.extend([(l(0.0), t(0.0), 0x2e2e2e), (r(0.0), b(0.0), 0xc6c6c6)]);
        }
        ButtonLook::Focused => {
            fill(x0, y0, x1, y1, 0x7d7e7f);
            // The dark outline.
            fill(l(0.0), t(0.0), x1, t(1.0), 0x3e3e3e);
            fill(l(0.0), b(0.0), x1, y1, 0x3e3e3e);
            fill(l(0.0), t(1.0), l(1.0), b(0.0), 0x3e3e3e);
            fill(r(0.0), t(1.0), x1, b(0.0), 0x3e3e3e);
            // The raised bevel inside it.
            fill(l(1.0), t(1.0), r(1.0), t(2.0), 0xbbbbbb);
            fill(l(1.0), t(2.0), l(2.0), b(1.0), 0xbbbbbb);
            fill(r(1.0), t(2.0), r(0.0), b(1.0), 0x464646);
            fill(l(2.0), b(1.0), r(1.0), b(0.0), 0x464647);
            // The softened corners.
            corners.extend([
                (l(0.0), t(0.0), 0x1b1b1b),
                (r(0.0), t(0.0), 0x1b1c1c),
                (l(0.0), b(0.0), 0x1b1c1c),
                (r(0.0), b(0.0), 0x1b1b1c),
                (l(1.0), t(0.0), 0x5f5f5f),
                (r(0.0), t(1.0), 0x5f5f5f),
                (l(1.0), t(1.0), 0xc6c6c6),
                (r(1.0), t(1.0), 0x646464),
                (l(1.0), b(1.0), 0x646465),
                (l(0.0), b(1.0), 0x2b2b2b),
                (r(0.0), b(1.0), 0x2b2b2b),
                (r(1.0), b(1.0), 0x2d2e2e),
                (l(1.0), b(0.0), 0x3f3f3f),
                (r(1.0), b(0.0), 0x1f2020),
            ]);
        }
    }
    let mut mesh = egui::Mesh::default();
    let mut add = |a: f32, b: f32, c: f32, d: f32, color: u32| {
        mesh.add_colored_rect(egui::Rect::from_min_max(egui::pos2(a / ppp, b / ppp), egui::pos2(c / ppp, d / ppp)), vgui(color));
    };
    for (a, b, c, d, color) in spans {
        add(a, b, c, d, color);
    }
    for (x, y, color) in corners {
        add(x, y, x + s, y + s, color);
    }
    egui::Shape::mesh(mesh)
}

/// The VGUI buttons drawn by hand this frame (egui's are found by their markers), for their sounds.
#[derive(Clone, Default)]
struct HandDrawnButtons(Vec<egui::Rect>);

/// Notes a hand-drawn VGUI button, so it clicks like the rest (see `sounds::button_feedback`).
pub fn register_button(ctx: &egui::Context, rect: egui::Rect) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<HandDrawnButtons>(egui::Id::NULL).0.push(rect));
}

/// The list items (DMs, channels, servers) drawn this frame, for their sounds.
#[derive(Clone, Default)]
struct ListItems(Vec<egui::Rect>);

/// Notes a list item (the visible part of it), so it sounds as the pointer rolls over and selects it.
pub fn register_list_item(ui: &egui::Ui, rect: egui::Rect) {
    let rect = rect.intersect(ui.clip_rect());
    if rect.is_positive() {
        ui.ctx().data_mut(|d| d.get_temp_mut_or_default::<ListItems>(egui::Id::NULL).0.push(rect));
    }
}

/// Last touches before drawing: VGUI has no rounded corners, so every rectangle is squared off (panels,
/// highlights, avatars), and egui buttons get VGUI bevelled frames, their labels nudged when pressed and
/// darkened when disabled. True if the pointer is over one of those buttons (which keep the arrow cursor).
pub fn post_process(ctx: &egui::Context, shapes: &mut [egui::epaint::ClippedShape]) -> bool {
    fn square(shape: &mut egui::Shape) {
        match shape {
            egui::Shape::Rect(r) => r.corner_radius = egui::CornerRadius::ZERO,
            egui::Shape::Vec(v) => v.iter_mut().for_each(square),
            _ => {}
        }
    }
    let ppp = ctx.pixels_per_point();
    let (pointer, down) = ctx.input(|i| (i.pointer.interact_pos(), i.pointer.primary_down()));
    let hover = ctx.input(|i| i.pointer.hover_pos());
    let mut over_button = false;
    // Every enabled VGUI button this frame, for their sounds.
    let mut buttons = ctx.data_mut(|d| std::mem::take(&mut d.get_temp_mut_or_default::<HandDrawnButtons>(egui::Id::NULL).0));
    let mut i = 0;
    while i < shapes.len() {
        let button = match &shapes[i].shape {
            egui::Shape::Rect(r) => {
                let down_on = down && pointer.is_some_and(|p| r.rect.contains(p));
                marked(r.fill, down_on).map(|look| (r.rect, look))
            }
            _ => None,
        };
        let Some((rect, look)) = button else {
            square(&mut shapes[i].shape);
            i += 1;
            continue;
        };
        over_button |= hover.is_some_and(|p| shapes[i].clip_rect.contains(p) && rect.contains(p));
        if look != ButtonLook::Disabled {
            buttons.push(rect.intersect(shapes[i].clip_rect));
        }
        shapes[i].shape = button_frame(rect, look, ppp);
        // The label is drawn next, inside the frame.
        let mut j = i + 1;
        while let Some(egui::Shape::Text(text)) = shapes.get_mut(j).map(|s| &mut s.shape) {
            if !rect.expand(1.0).contains_rect(text.visual_bounding_rect()) {
                break;
            }
            match look {
                ButtonLook::Pressed => text.pos += egui::vec2(2.0 * ppp.floor().max(1.0) / ppp, 0.0),
                ButtonLook::Disabled => {
                    // Source's near-black on its mid grey; dark palettes' faces need the opposite.
                    text.override_text_color = Some(if std::ptr::eq(palette(), &PALETTES[0]) { rgb(0x181818) } else { FAINT() });
                    text.opacity_factor = 1.0;
                }
                _ => {}
            }
            j += 1;
        }
        i = j;
    }
    let items = ctx.data_mut(|d| std::mem::take(&mut d.get_temp_mut_or_default::<ListItems>(egui::Id::NULL).0));
    crate::sounds::button_feedback(ctx, &buttons, &items);
    over_button
}

pub fn role_color(c: Option<u32>) -> Color32 {
    match c {
        Some(c) if c != 0 => Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8),
        _ => HEADER(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_markers_map_to_looks() {
        assert_eq!(marked(MARK_UP, false), Some(ButtonLook::Default));
        assert_eq!(marked(MARK_DOWN, true), Some(ButtonLook::Pressed));
        assert_eq!(marked(MARK_DOWN, false), Some(ButtonLook::Focused));
        // Disabled widgets are drawn at reduced opacity.
        assert_eq!(marked(MARK_UP.gamma_multiply(0.5), false), Some(ButtonLook::Disabled));
        assert_eq!(marked(CHAT(), false), None);
        assert_eq!(marked(Color32::TRANSPARENT, false), None);
    }

    /// Relative luminance's contrast ratio, as WCAG defines it.
    fn contrast(a: u32, b: u32) -> f32 {
        let lum = |c: u32| {
            let f = |v: u32| {
                let s = (v & 0xff) as f32 / 255.0;
                if s <= 0.03928 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * f(c >> 16) + 0.7152 * f(c >> 8) + 0.0722 * f(c)
        };
        let (a, b) = (lum(a), lum(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn palettes_are_readable_and_never_look_like_a_button_marker() {
        for p in &PALETTES[2..] {
            assert!(contrast(p.text, p.chat) >= 4.5, "{}: text on chat", p.name);
            assert!(contrast(p.header, p.sidebar) >= 4.5, "{}: header on sidebar", p.name);
        }
        for p in PALETTES {
            for c in [p.rail, p.sidebar, p.chat, p.input, p.hover, p.message_hover, p.code_bg, p.mention_bg, p.button, p.button_hover, p.accent, p.green, p.yellow, p.red] {
                assert_eq!(marked(rgb(c), false), None, "{}: {c:06x}", p.name);
            }
        }
    }

    #[test]
    fn steam_dark_is_a_little_darker_than_steam() {
        let (steam, dark) = (&PALETTES[0], &PALETTES[1]);
        for (a, b) in [(steam.rail, dark.rail), (steam.sidebar, dark.sidebar), (steam.chat, dark.chat), (steam.input, dark.input), (steam.button, dark.button)] {
            let (a, b) = (a & 0xff, b & 0xff);
            assert!(b < a && a - b <= 0x14, "{a:x} to {b:x}");
        }
    }
}
