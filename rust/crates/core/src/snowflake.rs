use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, TimeZone, Utc};
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Milliseconds between the Unix epoch and the Discord epoch (2015-01-01).
const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;

/// A Discord ID. Serialized as a string, but numbers are accepted too.
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Snowflake(pub u64);

impl Snowflake {
    pub const NONE: Snowflake = Snowflake(0);

    pub fn created_at(self) -> DateTime<Utc> {
        let ms = (self.0 >> 22) + DISCORD_EPOCH_MS;
        Utc.timestamp_millis_opt(ms as i64).single().unwrap_or_default()
    }

    pub fn is_none(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for Snowflake {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl fmt::Debug for Snowflake {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

impl FromStr for Snowflake {
    type Err = std::num::ParseIntError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Snowflake)
    }
}

impl From<u64> for Snowflake {
    fn from(v: u64) -> Self {
        Snowflake(v)
    }
}

impl Serialize for Snowflake {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Snowflake {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = Snowflake;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a snowflake string or integer")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Snowflake, E> {
                v.parse().map_err(E::custom)
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Snowflake, E> {
                Ok(Snowflake(v))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Snowflake, E> {
                u64::try_from(v).map(Snowflake).map_err(E::custom)
            }
        }
        d.deserialize_any(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_strings_and_numbers() {
        let a: Snowflake = serde_json::from_str("\"175928847299117063\"").unwrap();
        let b: Snowflake = serde_json::from_str("175928847299117063").unwrap();
        assert_eq!(a, b);
        assert_eq!(serde_json::to_string(&a).unwrap(), "\"175928847299117063\"");
    }

    #[test]
    fn computes_creation_time() {
        // From Discord's documentation.
        let id = Snowflake(175928847299117063);
        assert_eq!(id.created_at().timestamp_millis(), 1462015105796);
    }
}
