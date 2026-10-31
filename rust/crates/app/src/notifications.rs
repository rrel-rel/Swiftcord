//! When message notifications pop up and sound.
//!
//! As in Discord: the message sound plays at most once a second, and a channel's notifications are taken
//! away once it's read (opened, read elsewhere, or the window is focused on it). Unlike Discord, which shows a
//! toast per message, each channel has one toast: a burst of messages updates it in place (the latest message
//! and how many there are), and it only pops up again once the last one has had time to leave the screen.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use swiftcord_core::Snowflake;

/// About how long Windows keeps a toast on screen (5 s by default), and a little more.
const ON_SCREEN: Duration = Duration::from_secs(7);
/// Discord throttles its message sound to one a second.
const SOUND_GAP: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toast {
    /// Pop up a toast for the channel (replacing its last one), with `count` unread messages.
    Pop { count: u32 },
    /// Change the toast that's still up, in place.
    Update { count: u32 },
}

struct Unread {
    /// When its toast last popped up.
    popped: Instant,
    /// Messages notified about since it was last read, and the newest.
    count: u32,
    latest: Snowflake,
}

#[derive(Default)]
pub struct Notifications {
    channels: HashMap<Snowflake, Unread>,
    last_sound: Option<Instant>,
}

impl Notifications {
    /// A message to notify about: whether its channel's toast pops up or is updated.
    pub fn message(&mut self, channel: Snowflake, message: Snowflake, now: Instant) -> Toast {
        match self.channels.get_mut(&channel) {
            Some(u) => {
                u.count += 1;
                u.latest = u.latest.max(message);
                if now.duration_since(u.popped) < ON_SCREEN {
                    Toast::Update { count: u.count }
                } else {
                    u.popped = now;
                    Toast::Pop { count: u.count }
                }
            }
            None => {
                self.channels.insert(channel, Unread { popped: now, count: 1, latest: message });
                Toast::Pop { count: 1 }
            }
        }
    }

    /// Whether the message sound plays now (not if it played less than a second ago).
    pub fn sound(&mut self, now: Instant) -> bool {
        if self.last_sound.is_some_and(|t| now.duration_since(t) < SOUND_GAP) {
            return false;
        }
        self.last_sound = Some(now);
        true
    }

    /// Forgets the channels that `read(channel, newest notified message)` says have been read, and returns
    /// them (their toasts are to go).
    pub fn take_read(&mut self, read: impl Fn(Snowflake, Snowflake) -> bool) -> Vec<Snowflake> {
        let mut gone = Vec::new();
        self.channels.retain(|&c, u| {
            let keep = !read(c, u.latest);
            if !keep {
                gone.push(c);
            }
            keep
        });
        gone
    }
}

/// The toast's tag for a channel.
pub fn tag(channel: Snowflake) -> String {
    format!("channel-{}", channel.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_pops_up_once_and_updates_the_toast() {
        let mut n = Notifications::default();
        let (c, t) = (Snowflake(1), Instant::now());
        let toasts: Vec<Toast> = (0..5).map(|i| n.message(c, Snowflake(10 + i), t + Duration::from_millis(300 * i))).collect();
        assert_eq!(toasts[0], Toast::Pop { count: 1 });
        assert_eq!(toasts[4], Toast::Update { count: 5 });
        // Once it's had time to leave the screen, the next one pops up again, still counting.
        assert_eq!(n.message(c, Snowflake(20), t + ON_SCREEN + Duration::from_secs(1)), Toast::Pop { count: 6 });
        // Another channel has its own.
        assert_eq!(n.message(Snowflake(2), Snowflake(21), t), Toast::Pop { count: 1 });
    }

    #[test]
    fn reading_a_channel_starts_it_over() {
        let mut n = Notifications::default();
        let t = Instant::now();
        n.message(Snowflake(1), Snowflake(10), t);
        n.message(Snowflake(2), Snowflake(11), t);
        // Read up to an older message: still unread.
        assert!(n.take_read(|_, latest| latest <= Snowflake(9)).is_empty());
        assert_eq!(n.take_read(|c, _| c == Snowflake(1)), vec![Snowflake(1)]);
        assert_eq!(n.message(Snowflake(1), Snowflake(12), t), Toast::Pop { count: 1 });
    }

    #[test]
    fn the_sound_plays_at_most_once_a_second() {
        let mut n = Notifications::default();
        let t = Instant::now();
        let played: Vec<bool> = [0, 200, 900, 1000, 1500, 2100].iter().map(|ms| n.sound(t + Duration::from_millis(*ms))).collect();
        assert_eq!(played, [true, false, false, true, false, true]);
    }
}
