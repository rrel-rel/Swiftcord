//! Message search queries as Discord's search box takes them: words to find, plus filters such as
//! `from:alice`, `in:general`, `has:image`, `mentions:bob`, `before:2024-05-01`, `after:` and `during:`.
//! Names are resolved to IDs by the caller, which knows the server's members and channels.

use chrono::{NaiveDate, TimeZone, Utc};

use crate::Snowflake;

/// What `has:` accepts.
pub const HAS: &[&str] = &["link", "embed", "file", "image", "video", "sound", "sticker", "poll", "forward"];

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Query {
    /// The words to find (filters removed).
    pub content: String,
    pub from: Vec<String>,
    pub channels: Vec<String>,
    pub has: Vec<String>,
    pub mentions: Vec<String>,
    pub before: Option<NaiveDate>,
    pub after: Option<NaiveDate>,
    pub during: Option<NaiveDate>,
}

impl Query {
    pub fn parse(text: &str) -> Query {
        let mut q = Query::default();
        let mut words = Vec::new();
        for token in text.split_whitespace() {
            let Some((key, value)) = token.split_once(':').filter(|(_, v)| !v.is_empty()) else {
                words.push(token);
                continue;
            };
            let date = || NaiveDate::parse_from_str(value, "%Y-%m-%d").ok();
            match key.to_ascii_lowercase().as_str() {
                "from" => q.from.push(value.trim_start_matches('@').to_owned()),
                "in" => q.channels.push(value.trim_start_matches('#').to_owned()),
                "has" if HAS.contains(&value.to_ascii_lowercase().as_str()) => q.has.push(value.to_ascii_lowercase()),
                "mentions" => q.mentions.push(value.trim_start_matches('@').to_owned()),
                "before" if date().is_some() => q.before = date(),
                "after" if date().is_some() => q.after = date(),
                "during" | "on" if date().is_some() => q.during = date(),
                _ => words.push(token),
            }
        }
        q.content = words.join(" ");
        q
    }

    /// The `min_id` and `max_id` limits for the dates given.
    pub fn id_range(&self) -> (Option<Snowflake>, Option<Snowflake>) {
        let start = |d: NaiveDate| snowflake_at(d.and_hms_opt(0, 0, 0).expect("midnight"));
        let mut min = self.after.map(|d| start(d.succ_opt().unwrap_or(d)));
        let mut max = self.before.map(start);
        if let Some(d) = self.during {
            min = Some(start(d));
            max = Some(start(d.succ_opt().unwrap_or(d)));
        }
        (min, max)
    }

    pub fn is_empty(&self) -> bool {
        *self == Query::default()
    }
}

/// The smallest message ID that could have been sent at `t` (UTC).
pub fn snowflake_at(t: chrono::NaiveDateTime) -> Snowflake {
    const DISCORD_EPOCH_MS: i64 = 1_420_070_400_000;
    let ms = Utc.from_utc_datetime(&t).timestamp_millis();
    Snowflake(((ms - DISCORD_EPOCH_MS).max(0) as u64) << 22)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_filters_from_words() {
        let q = Query::parse("minecraft skins from:@Rivvi in:#nihongo-lounge has:image has:nonsense before:2024-08-01");
        assert_eq!(q.content, "minecraft skins has:nonsense");
        assert_eq!(q.from, ["Rivvi"]);
        assert_eq!(q.channels, ["nihongo-lounge"]);
        assert_eq!(q.has, ["image"]);
        assert_eq!(q.before, NaiveDate::from_ymd_opt(2024, 8, 1));
        // A colon in ordinary text stays text.
        assert_eq!(Query::parse("at 10:30 ok").content, "at 10:30 ok");
    }

    #[test]
    fn dates_become_id_limits() {
        let q = Query::parse("during:2024-07-10");
        let (min, max) = q.id_range();
        let day = |d: u32| snowflake_at(NaiveDate::from_ymd_opt(2024, 7, d).unwrap().and_hms_opt(0, 0, 0).unwrap());
        assert_eq!((min, max), (Some(day(10)), Some(day(11))));
        // A message sent at noon on the 10th falls inside.
        let noon = snowflake_at(NaiveDate::from_ymd_opt(2024, 7, 10).unwrap().and_hms_opt(12, 0, 0).unwrap());
        assert!(min.unwrap() < noon && noon < max.unwrap());
        assert_eq!(Query::parse("after:2024-07-10").id_range().0, Some(day(11)));
    }
}
