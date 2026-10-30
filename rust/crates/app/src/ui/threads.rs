//! Threads: the thread list beside a channel, a forum's posts, the "start a thread" window, and the chip under
//! a message that started a thread.

use chrono::Utc;
use egui::{self, Align, Color32, Layout, Rect, RichText, Sense, Ui, Vec2};
use swiftcord_core::models::Channel;
use swiftcord_core::permissions::perm;
use swiftcord_core::state::State;
use swiftcord_core::Snowflake;

use crate::app::{Action, Session, UiState};
use crate::theme::{self, icon};
use crate::ui::widgets;

/// Forum post flag: pinned to the top.
const PINNED_POST: u64 = 1 << 1;

/// "5m ago", from the last activity (or creation) of a thread.
fn ago(id: Snowflake) -> String {
    let secs = (Utc::now() - id.created_at()).num_seconds().max(0);
    match secs {
        0..=59 => "just now".to_owned(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        86_400..=2_591_999 => format!("{}d ago", secs / 86_400),
        2_592_000..=31_535_999 => format!("{}mo ago", secs / 2_592_000),
        _ => format!("{}y ago", secs / 31_536_000),
    }
}

fn message_count(t: &Channel) -> String {
    match t.message_count.unwrap_or(0) {
        1 => "1 message".to_owned(),
        n => format!("{n} messages"),
    }
}

fn can_start_threads(state: &State, channel: &Channel) -> bool {
    let perms = state.permissions(channel.id);
    let needed = if channel.is_forum() { perm::SEND_MESSAGES } else { perm::CREATE_PUBLIC_THREADS };
    perms & needed != 0
}

/// The thread list beside a text channel: active threads, then older (archived) ones page by page.
pub fn panel(ui: &mut Ui, s: &Session, uis: &UiState, c: &Channel, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon::CHAT).font(theme::icon_font(16.0)).color(theme::MUTED()));
        ui.label(RichText::new("Threads").font(theme::font_semibold(16.0)).color(theme::HEADER()));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::icon_button(ui, icon::CLOSE, "Close").clicked() {
                actions.push(Action::ShowThreads(c.id));
            }
            if can_start_threads(&s.state, c) && ui.button("Create").clicked() {
                actions.push(Action::StartThread { channel: c.id, message: None, name: String::new() });
            }
        });
    });
    ui.separator();
    let browser = uis.thread_browsers.get(&c.id);
    let active = s.state.threads(c.id);
    let scroll = egui::ScrollArea::vertical().id_salt(("threads-list", c.id)).auto_shrink(false).show(ui, |ui| {
        section_label(ui, &format!("{} ACTIVE THREADS", active.len()));
        if active.is_empty() {
            ui.label(RichText::new("No active threads.").color(theme::MUTED()));
        }
        for t in &active {
            thread_row(ui, &s.state, t, actions);
        }
        let archived: Vec<&Channel> = browser.map_or(Vec::new(), |b| b.archived.iter().filter_map(|id| s.state.channel(*id)).filter(|t| t.is_archived()).collect());
        if !archived.is_empty() {
            ui.add_space(10.0);
            section_label(ui, "OLDER THREADS");
            for t in archived {
                thread_row(ui, &s.state, t, actions);
            }
        }
        more_or_spinner(ui, c.id, browser, actions);
    });
    crate::ui::widgets::scrollbar(ui, &scroll);
}

fn section_label(ui: &mut Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text).font(theme::font_semibold(12.0)).color(theme::MUTED()));
    ui.add_space(2.0);
}

fn more_or_spinner(ui: &mut Ui, channel: Snowflake, browser: Option<&crate::threads_glue::Browser>, actions: &mut Vec<Action>) {
    match browser {
        Some(b) if b.loading => {
            ui.add_space(8.0);
            ui.vertical_centered(|ui| ui.spinner());
        }
        Some(b) if b.has_more => {
            ui.add_space(8.0);
            if ui.button("Load older").clicked() {
                actions.push(Action::LoadThreads { channel, more: true });
            }
        }
        _ => {}
    }
}

fn thread_row(ui: &mut Ui, state: &State, t: &Channel, actions: &mut Vec<Action>) {
    let (rect, r) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 52.0), Sense::click());
    if r.hovered() {
        ui.painter().rect_filled(rect, 6, theme::HOVER());
    }
    let p = ui.painter();
    let x = rect.left() + 10.0;
    let mut title = t.label();
    if t.thread_metadata.as_ref().is_some_and(|m| m.locked) {
        title = format!("🔒 {title}");
    }
    let galley = p.layout_no_wrap(title, theme::font_semibold(15.0), theme::HEADER());
    let clip = Rect::from_min_max(rect.min, egui::pos2(rect.right() - 8.0, rect.bottom()));
    p.with_clip_rect(clip).galley(egui::pos2(x, rect.top() + 8.0), galley, theme::HEADER());
    let owner = t.owner_id.map(|o| state.display_name(o, t.guild_id)).unwrap_or_default();
    let mut detail = vec![message_count(t), ago(t.last_message_id.unwrap_or(t.id))];
    if !owner.is_empty() {
        detail.insert(0, owner);
    }
    p.with_clip_rect(clip).text(egui::pos2(x, rect.top() + 34.0), egui::Align2::LEFT_CENTER, detail.join(" · "), theme::font(12.5), theme::MUTED());
    if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        actions.push(Action::OpenThread(t.id));
    }
}

/// A forum: its posts as cards, pinned first, then by latest activity.
pub fn forum(ui: &mut Ui, s: &Session, uis: &UiState, c: &Channel, actions: &mut Vec<Action>) {
    // Loads once; cheap to ask every frame (and picks up a refresh after a new post).
    actions.push(Action::LoadThreads { channel: c.id, more: false });
    let browser = uis.thread_browsers.get(&c.id);
    let mut posts = s.state.threads(c.id);
    if let Some(b) = browser {
        posts.extend(b.archived.iter().filter_map(|id| s.state.channel(*id)).filter(|t| t.is_archived()));
    }
    posts.sort_by_key(|t| (t.flags & PINNED_POST == 0, std::cmp::Reverse(t.last_message_id.unwrap_or(t.id))));

    let scroll = egui::ScrollArea::vertical().id_salt(("forum", c.id)).auto_shrink(false).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(24, 16)).show(ui, |ui| {
            ui.set_max_width(760.0_f32.min(ui.available_width()));
            ui.horizontal(|ui| {
                if let Some(t) = c.topic.as_deref().filter(|t| !t.is_empty()) {
                    ui.add(egui::Label::new(RichText::new(t).color(theme::MUTED())).wrap());
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if can_start_threads(&s.state, c) {
                        let b = egui::Button::new(RichText::new("New Post").font(theme::font_semibold(14.0)).color(Color32::WHITE));
                        if ui.add(b).clicked() {
                            actions.push(Action::StartThread { channel: c.id, message: None, name: String::new() });
                        }
                    }
                });
            });
            ui.add_space(12.0);
            if posts.is_empty() && browser.is_some_and(|b| b.loaded && !b.loading) {
                ui.label(RichText::new("There are no posts yet.").color(theme::MUTED()));
            }
            for t in posts {
                post_card(ui, s, browser, c, t, actions);
                ui.add_space(8.0);
            }
            more_or_spinner(ui, c.id, browser, actions);
        });
    });
    crate::ui::widgets::scrollbar(ui, &scroll);
}

fn post_card(ui: &mut Ui, s: &Session, browser: Option<&crate::threads_glue::Browser>, forum: &Channel, t: &Channel, actions: &mut Vec<Action>) {
    let first = browser.and_then(|b| b.first_messages.get(&t.id));
    let r = egui::Frame::new().fill(theme::SIDEBAR()).corner_radius(8).inner_margin(14).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            if t.flags & PINNED_POST != 0 {
                ui.label(RichText::new(icon::PIN).font(theme::icon_font(13.0)).color(theme::MUTED()));
            }
            ui.add(egui::Label::new(RichText::new(t.label()).font(theme::font_semibold(16.0)).color(theme::HEADER())).truncate().selectable(false));
        });
        // Tags, as Discord shows them under the title.
        let tags: Vec<String> = t
            .applied_tags
            .iter()
            .filter_map(|id| forum.available_tags.iter().find(|tag| tag.id == *id))
            .map(|tag| format!("{}{}", tag.emoji_name.as_deref().map(|e| format!("{e} ")).unwrap_or_default(), tag.name))
            .collect();
        if !tags.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for tag in tags {
                    ui.label(RichText::new(format!(" {tag} ")).size(12.0).color(theme::TEXT()).background_color(theme::SELECTED()));
                }
            });
        }
        if let Some(m) = first {
            let author = s.state.display_name(m.author.id, forum.guild_id);
            let text: String = m.content.lines().next().unwrap_or_default().chars().take(160).collect();
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{author}:")).font(theme::font_semibold(13.5)).color(theme::MUTED()));
                ui.add(egui::Label::new(RichText::new(text).size(13.5).color(theme::MUTED())).truncate().selectable(false));
            });
        }
        ui.label(RichText::new(format!("{} · {}", message_count(t), ago(t.last_message_id.unwrap_or(t.id)))).size(12.5).color(theme::FAINT()));
    });
    let r = ui.interact(r.response.rect, egui::Id::unique(("post", t.id)), Sense::click());
    if r.hovered() {
        ui.painter().rect_stroke(r.rect, 8, egui::Stroke::new(1.0, theme::SELECTED()), egui::StrokeKind::Inside);
    }
    if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        actions.push(Action::OpenThread(t.id));
    }
}

/// The window for naming a new thread (or writing a new forum post).
pub fn prompt(ui: &Ui, uis: &mut UiState, actions: &mut Vec<Action>) {
    let Some(p) = &mut uis.thread_prompt else { return };
    let mut open = true;
    let title = if p.forum { "New Post" } else { "Start a Thread" };
    egui::Window::new(title)
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(420.0)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ui.ctx(), |ui| {
            ui.label(RichText::new(if p.forum { "POST TITLE" } else { "THREAD NAME" }).font(theme::font_semibold(12.0)).color(theme::MUTED()));
            let name = ui.add(egui::TextEdit::singleline(&mut p.name).char_limit(100).desired_width(f32::INFINITY).hint_text("New Thread"));
            if !name.has_focus() && p.name.is_empty() && p.content.is_empty() {
                name.request_focus();
            }
            let mut submit = name.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !p.forum;
            if p.forum {
                ui.add_space(6.0);
                ui.label(RichText::new("MESSAGE").font(theme::font_semibold(12.0)).color(theme::MUTED()));
                ui.add(egui::TextEdit::multiline(&mut p.content).desired_rows(5).desired_width(f32::INFINITY));
            }
            ui.add_space(8.0);
            // A fixed-height row: a bare right-to-left layout would stretch to fill the window.
            ui.allocate_ui_with_layout(Vec2::new(ui.available_width(), 32.0), Layout::right_to_left(Align::Center), |ui| {
                let ready = !p.name.trim().is_empty() && (!p.forum || !p.content.trim().is_empty()) && !p.busy;
                let label = if p.forum { "Post" } else { "Create" };
                let go = egui::Button::new(RichText::new(label).color(Color32::WHITE));
                submit |= ui.add_enabled(ready, go).clicked();
                if ui.button("Cancel").clicked() {
                    actions.push(Action::CloseThreadPrompt);
                }
                if p.busy {
                    ui.spinner();
                }
            });
            if submit {
                actions.push(Action::CreateThread);
            }
        });
    if !open {
        actions.push(Action::CloseThreadPrompt);
    }
}

/// Under a message that started a thread: its name, size and latest activity; opens it.
pub fn chip(ui: &mut Ui, t: &Channel, actions: &mut Vec<Action>) {
    let r = egui::Frame::new().fill(theme::EMBED_BG()).corner_radius(6).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.set_max_width(420.0_f32.min(ui.available_width()));
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::CHAT).font(theme::icon_font(14.0)).color(theme::MUTED()));
            ui.add(egui::Label::new(RichText::new(t.label()).font(theme::font_semibold(14.0)).color(theme::HEADER())).truncate().selectable(false));
            ui.label(RichText::new(format!("{} ›", message_count(t))).size(13.0).color(theme::LINK()));
        });
    });
    let r = ui.interact(r.response.rect, egui::Id::unique(("thread-chip", t.id)), Sense::click());
    if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        actions.push(Action::OpenThread(t.id));
    }
}
