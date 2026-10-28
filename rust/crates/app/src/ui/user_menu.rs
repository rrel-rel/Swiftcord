//! The menu on someone in a voice channel (right-click their name in the sidebar or their tile), as in
//! Discord: their profile, a DM or a call, how loud they are for us, and a moderator's voice controls.

use eframe::egui::{self, RichText, Ui};
use swiftcord_core::models::Channel;
use swiftcord_core::permissions::perm;
use swiftcord_core::state::State;
use swiftcord_core::Snowflake;

use crate::app::{Action, UiState, VoiceMod};
use crate::theme;

/// `at` is what was right-clicked: the profile card opens beside it.
pub fn voice_user(ui: &mut Ui, state: &State, uis: &UiState, channel: &Channel, user: Snowflake, at: &egui::Response, actions: &mut Vec<Action>) {
    ui.set_min_width(230.0);
    let me = state.me.as_ref().is_some_and(|m| m.id == user);
    if ui.button("Profile").clicked() {
        actions.push(crate::ui::profile::open(user, channel.guild_id, at));
        ui.close();
    }
    if !me {
        if ui.button("Message").clicked() {
            actions.push(Action::OpenDm(user));
            ui.close();
        }
        if ui.button("Call").clicked() {
            actions.push(Action::CallUser(user));
            ui.close();
        }

        // Only for us: how loud they are, or not at all.
        ui.separator();
        ui.label(RichText::new("User Volume").size(12.0).color(theme::MUTED()));
        let mut percent = uis.user_volumes.get(&user).copied().unwrap_or(1.0) * 100.0;
        let r = ui.add(egui::Slider::new(&mut percent, 0.0..=200.0).step_by(1.0).suffix("%"));
        if r.changed() || r.drag_stopped() {
            // Saved once the slider is let go, not on every step of a drag.
            actions.push(Action::SetUserVolume { user, volume: percent / 100.0, save: !r.dragged() });
        }
        let mut muted = uis.muted_users.contains(&user);
        if ui.checkbox(&mut muted, "Mute").changed() {
            actions.push(Action::ToggleLocalMute(user));
        }
    }

    // A moderator's controls, as far as their permissions in this channel go.
    if let Some(guild) = channel.guild_id {
        let perms = state.permissions(channel.id);
        let vs = state.voice_state(Some(guild), user);
        let (can_mute, can_deafen, can_move) = (perms & perm::MUTE_MEMBERS != 0, perms & perm::DEAFEN_MEMBERS != 0, perms & perm::MOVE_MEMBERS != 0);
        if can_mute || can_deafen || can_move {
            ui.separator();
        }
        if can_mute {
            let mut on = vs.is_some_and(|v| v.mute);
            if ui.checkbox(&mut on, "Server Mute").changed() {
                actions.push(Action::ModerateVoice { guild, user, change: VoiceMod::Mute(on) });
            }
        }
        if can_deafen {
            let mut on = vs.is_some_and(|v| v.deaf);
            if ui.checkbox(&mut on, "Server Deafen").changed() {
                actions.push(Action::ModerateVoice { guild, user, change: VoiceMod::Deafen(on) });
            }
        }
        if can_move {
            let others: Vec<&Channel> = state
                .channel_tree(guild)
                .into_iter()
                .flat_map(|(_, cs)| cs)
                .filter(|c| c.is_voice() && c.id != channel.id && state.permissions(c.id) & perm::VIEW_CHANNEL != 0)
                .collect();
            if !others.is_empty() {
                ui.menu_button("Move To", |ui| {
                    for c in others {
                        if ui.button(format!("🔊 {}", c.label())).clicked() {
                            actions.push(Action::ModerateVoice { guild, user, change: VoiceMod::Move(Some(c.id)) });
                            ui.close();
                        }
                    }
                });
            }
            if ui.button(RichText::new("Disconnect").color(theme::RED())).clicked() {
                actions.push(Action::ModerateVoice { guild, user, change: VoiceMod::Move(None) });
                ui.close();
            }
        }
        if me && !can_move && ui.button(RichText::new("Disconnect").color(theme::RED())).clicked() {
            actions.push(Action::LeaveVoice);
            ui.close();
        }
    }

    ui.separator();
    if ui.button("Copy User ID").clicked() {
        actions.push(Action::Copy(user.to_string()));
        ui.close();
    }
}
