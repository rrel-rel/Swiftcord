//! Lays out parsed markdown as wrapped egui widgets.

use std::collections::HashSet;

use chrono::{Local, TimeZone};
use eframe::egui::{self, Color32, RichText, Sense, Ui, Vec2};
use swiftcord_core::markdown::{Block, Span, Style};
use swiftcord_core::state::State;
use swiftcord_core::Snowflake;

use crate::app::Action;
use crate::theme;

pub struct Cx<'a> {
    pub state: &'a State,
    pub guild: Option<Snowflake>,
    pub message: Snowflake,
    pub revealed: &'a mut HashSet<(Snowflake, u16)>,
    pub actions: &'a mut Vec<Action>,
    pub size: f32,
    pub color: Color32,
    pub jumbo: bool,
}

pub fn blocks(ui: &mut Ui, blocks: &[Block], cx: &mut Cx) {
    for b in blocks {
        block(ui, b, cx);
    }
}

fn block(ui: &mut Ui, b: &Block, cx: &mut Cx) {
    match b {
        Block::Paragraph(spans) => spans_ui(ui, spans, cx, cx.size),
        Block::Heading(level, spans) => {
            let size = match level {
                1 => 24.0,
                2 => 20.0,
                _ => 17.0,
            };
            ui.add_space(4.0);
            let mut bold = |s: &Span| match s {
                Span::Text(t, st) => Span::Text(t.clone(), Style { bold: true, ..*st }),
                other => other.clone(),
            };
            let spans: Vec<Span> = spans.iter().map(&mut bold).collect();
            spans_ui(ui, &spans, cx, size);
        }
        Block::Subtext(spans) => {
            let color = std::mem::replace(&mut cx.color, theme::MUTED);
            spans_ui(ui, spans, cx, 12.5);
            cx.color = color;
        }
        Block::ListItem { depth, spans } => {
            ui.horizontal_top(|ui| {
                ui.add_space(4.0 + *depth as f32 * 18.0);
                ui.label(RichText::new(if *depth == 0 { "•" } else { "◦" }).size(cx.size).color(cx.color));
                ui.vertical(|ui| spans_ui(ui, spans, cx, cx.size));
            });
        }
        Block::Quote(inner) => {
            let r = egui::Frame::new().inner_margin(egui::Margin { left: 12, right: 0, top: 0, bottom: 0 }).show(ui, |ui| {
                blocks(ui, inner, cx);
            });
            let rect = r.response.rect;
            ui.painter().rect_filled(egui::Rect::from_min_size(rect.min, Vec2::new(4.0, rect.height())), 2, theme::FAINT);
        }
        Block::Code { code, .. } => {
            egui::Frame::new().fill(theme::CODE_BG).corner_radius(4).inner_margin(8).stroke(egui::Stroke::new(1.0, theme::RAIL)).show(ui, |ui| {
                ui.set_max_width(ui.available_width());
                egui::ScrollArea::horizontal().id_salt(("code", cx.message, code.len())).show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(code).monospace().color(theme::TEXT)).extend());
                });
            });
        }
    }
}

fn spans_ui(ui: &mut Ui, spans: &[Span], cx: &mut Cx, size: f32) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(0.0, 2.0);
        for s in spans {
            span(ui, s, cx, size);
        }
    });
}

fn styled(text: &str, st: &Style, size: f32, color: Color32) -> RichText {
    let font = if st.bold { theme::font_bold(size) } else { theme::font(size) };
    let mut t = RichText::new(text).font(font).color(color);
    if st.italic {
        t = t.italics();
    }
    if st.underline {
        t = t.underline();
    }
    if st.strike {
        t = t.strikethrough();
    }
    t
}

fn hidden(cx: &Cx, st: &Style) -> Option<u16> {
    st.spoiler.filter(|&i| !cx.revealed.contains(&(cx.message, i)))
}

fn span(ui: &mut Ui, s: &Span, cx: &mut Cx, size: f32) {
    match s {
        Span::Text(t, st) => {
            if let Some(i) = hidden(cx, st) {
                // Unrevealed spoilers are blocks of the background colour until clicked.
                let r = ui.add(egui::Label::new(styled(t, st, size, theme::RAIL).background_color(theme::RAIL)).sense(Sense::click()));
                if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    cx.revealed.insert((cx.message, i));
                }
            } else {
                let mut rt = styled(t, st, size, cx.color);
                if st.spoiler.is_some() {
                    rt = rt.background_color(Color32::from_rgba_unmultiplied(255, 255, 255, 18));
                }
                ui.label(rt);
            }
        }
        Span::Code(code) => {
            ui.label(RichText::new(code).monospace().size(size - 1.5).background_color(theme::CODE_BG).color(theme::TEXT));
        }
        Span::Link { text, url, style } => {
            if let Some(i) = hidden(cx, style) {
                if ui.add(egui::Label::new(styled(text, style, size, theme::RAIL).background_color(theme::RAIL)).sense(Sense::click())).clicked() {
                    cx.revealed.insert((cx.message, i));
                }
                return;
            }
            let r = ui.add(egui::Label::new(styled(text, style, size, theme::LINK)).sense(Sense::click()));
            let r = r.on_hover_cursor(egui::CursorIcon::PointingHand);
            let r = if text != url { r.on_hover_text(url) } else { r };
            if r.hovered() {
                ui.painter().hline(r.rect.x_range(), r.rect.bottom() - 2.0, egui::Stroke::new(1.0, theme::LINK));
            }
            if r.clicked() {
                crate::platform::open_url(url);
            }
            r.context_menu(|ui| {
                if ui.button("Copy link").clicked() {
                    ui.ctx().copy_text(url.clone());
                    ui.close();
                }
            });
        }
        Span::User(id) => {
            let name = cx.state.display_name(*id, cx.guild);
            let r = mention(ui, &format!("@{name}"), size);
            if r.clicked() {
                cx.actions.push(Action::OpenDm(*id));
            }
            r.on_hover_text("Message");
        }
        Span::Role(id) => {
            let role = cx.guild.and_then(|g| cx.state.guild(g)).and_then(|g| g.roles.iter().find(|r| r.id == *id));
            let name = role.map_or("unknown-role".to_owned(), |r| r.name.clone());
            let color = role.map(|r| r.color).filter(|&c| c != 0).map(|c| theme::role_color(Some(c)));
            match color {
                Some(c) => {
                    ui.label(RichText::new(format!("@{name}")).size(size).color(c).background_color(c.gamma_multiply(0.15)));
                }
                None => {
                    mention(ui, &format!("@{name}"), size);
                }
            }
        }
        Span::Channel(id) => {
            let name = cx.state.channel(*id).map_or("unknown".to_owned(), |c| c.label());
            if mention(ui, &format!("#{name}"), size).clicked() && cx.state.channel(*id).is_some() {
                cx.actions.push(Action::SelectChannel(*id));
            }
        }
        Span::Everyone(word) => {
            mention(ui, word, size);
        }
        Span::Emoji { id, name, animated } => {
            let px = if cx.jumbo { 48.0 } else { size * 1.375 };
            let url = swiftcord_core::cdn::emoji(*id, *animated, if cx.jumbo { 96 } else { 48 });
            ui.add(egui::Image::from_uri(url).fit_to_exact_size(Vec2::splat(px)).show_loading_spinner(false))
                .on_hover_text(format!(":{name}:"));
        }
        Span::UnicodeEmoji(e) => {
            let px = if cx.jumbo { 48.0 } else { size * 1.375 };
            ui.add(egui::Image::from_uri(swiftcord_core::cdn::twemoji(e)).fit_to_exact_size(Vec2::splat(px)).show_loading_spinner(false))
                .on_hover_text(e.as_str());
        }
        Span::Timestamp(t, style) => {
            let text = format_timestamp(*t, *style);
            let full = format_timestamp(*t, 'F');
            ui.label(RichText::new(text).size(size).background_color(Color32::from_rgba_unmultiplied(255, 255, 255, 14)))
                .on_hover_text(full);
        }
    }
}

fn mention(ui: &mut Ui, text: &str, size: f32) -> egui::Response {
    ui.add(
        egui::Label::new(RichText::new(text).font(theme::font_semibold(size)).color(theme::MENTION_TEXT).background_color(theme::MENTION_BG))
            .sense(Sense::click()),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn format_timestamp(t: i64, style: char) -> String {
    let Some(dt) = Local.timestamp_opt(t, 0).single() else { return t.to_string() };
    match style {
        't' => dt.format("%-I:%M %p").to_string(),
        'T' => dt.format("%-I:%M:%S %p").to_string(),
        'd' => dt.format("%m/%d/%Y").to_string(),
        'D' => dt.format("%B %-d, %Y").to_string(),
        'F' => dt.format("%A, %B %-d, %Y %-I:%M %p").to_string(),
        'R' => relative(dt.with_timezone(&chrono::Utc)),
        _ => dt.format("%B %-d, %Y %-I:%M %p").to_string(),
    }
}

fn relative(t: chrono::DateTime<chrono::Utc>) -> String {
    let secs = (chrono::Utc::now() - t).num_seconds();
    let (n, unit) = match secs.abs() {
        s if s < 60 => (s, "second"),
        s if s < 3600 => (s / 60, "minute"),
        s if s < 86400 => (s / 3600, "hour"),
        s if s < 86400 * 30 => (s / 86400, "day"),
        s if s < 86400 * 365 => (s / (86400 * 30), "month"),
        s => (s / (86400 * 365), "year"),
    };
    let plural = if n == 1 { "" } else { "s" };
    if secs >= 0 { format!("{n} {unit}{plural} ago") } else { format!("in {n} {unit}{plural}") }
}
