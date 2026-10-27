//! Apps' slash commands: what can be run where (the application command index), and running one (an
//! APPLICATION_COMMAND interaction; the app answers like any other interaction).

use serde::Deserialize;
use serde_json::{json, Value};

use crate::Snowflake;

pub mod option_type {
    pub const SUB_COMMAND: u8 = 1;
    pub const SUB_COMMAND_GROUP: u8 = 2;
    pub const STRING: u8 = 3;
    pub const INTEGER: u8 = 4;
    pub const BOOLEAN: u8 = 5;
    pub const USER: u8 = 6;
    pub const CHANNEL: u8 = 7;
    pub const ROLE: u8 = 8;
    pub const MENTIONABLE: u8 = 9;
    pub const NUMBER: u8 = 10;
    pub const ATTACHMENT: u8 = 11;
}

/// Chat-input (slash) commands; the others are right-click menu commands.
pub const CHAT_INPUT: u8 = 1;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Command {
    pub id: Snowflake,
    pub application_id: Snowflake,
    pub version: Snowflake,
    #[serde(rename = "type")]
    pub kind: Option<u8>,
    pub name: String,
    pub description: String,
    pub options: Vec<CommandOption>,
    /// The whole command as Discord sent it, sent back when it's run.
    #[serde(skip)]
    pub raw: Value,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CommandOption {
    #[serde(rename = "type")]
    pub kind: u8,
    pub name: String,
    pub description: String,
    pub required: bool,
    pub choices: Vec<Choice>,
    /// A subcommand's or group's own options.
    pub options: Vec<CommandOption>,
    pub autocomplete: bool,
    pub min_value: Option<f64>,
    pub max_value: Option<f64>,
    pub min_length: Option<u32>,
    pub max_length: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Choice {
    pub name: String,
    pub value: Value,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct App {
    pub id: Snowflake,
    pub name: String,
    pub icon: Option<String>,
}

/// What `…/application-command-index` returns.
#[derive(Debug, Clone, Default)]
pub struct Index {
    pub apps: Vec<App>,
    pub commands: Vec<Command>,
}

impl Index {
    /// Reads an index response, keeping each command's raw JSON (to send back when it's run).
    pub fn parse(v: &Value) -> Index {
        let apps = v.get("applications").cloned().and_then(|a| serde_json::from_value(a).ok()).unwrap_or_default();
        let commands = v
            .get("application_commands")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|c| {
                let mut cmd: Command = serde_json::from_value(c.clone()).ok()?;
                cmd.raw = c.clone();
                Some(cmd)
            })
            .collect();
        Index { apps, commands }
    }

    /// Adds another index's apps and commands (e.g. our own installed apps' to a server's), skipping repeats.
    pub fn merge(&mut self, other: Index) {
        for a in other.apps {
            if !self.apps.iter().any(|x| x.id == a.id) {
                self.apps.push(a);
            }
        }
        for c in other.commands {
            if !self.commands.iter().any(|x| x.id == c.id) {
                self.commands.push(c);
            }
        }
    }

    /// Slash commands whose name starts with `prefix` (then ones containing it), at most `limit`.
    pub fn search(&self, prefix: &str, limit: usize) -> Vec<&Command> {
        let q = prefix.to_lowercase();
        let mut hits: Vec<(bool, &Command)> = self
            .commands
            .iter()
            .filter(|c| c.kind.unwrap_or(CHAT_INPUT) == CHAT_INPUT)
            .filter_map(|c| {
                let n = c.name.to_lowercase();
                if n.starts_with(&q) {
                    Some((false, c))
                } else if n.contains(&q) {
                    Some((true, c))
                } else {
                    None
                }
            })
            .collect();
        hits.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.name.cmp(&b.1.name)));
        hits.into_iter().take(limit).map(|(_, c)| c).collect()
    }

    pub fn app(&self, id: Snowflake) -> Option<&App> {
        self.apps.iter().find(|a| a.id == id)
    }
}

/// An option's value as entered: what goes in the interaction's `options`.
pub fn option_value(kind: u8, name: &str, value: Value) -> Value {
    json!({ "type": kind, "name": name, "value": value })
}

/// Running `command`: `path` names the subcommand (and group) chosen, if any; `options` are the leaf
/// options' values (from [`option_value`]).
pub fn command_body(command: &Command, path: &[(u8, String)], options: Vec<Value>, guild: Option<Snowflake>, channel: Snowflake, session_id: &str, nonce: Snowflake) -> Value {
    // Nest the options inside the subcommand, and that inside its group.
    let mut nested = options;
    for (kind, name) in path.iter().rev() {
        nested = vec![json!({ "type": kind, "name": name, "options": nested })];
    }
    json!({
        "type": 2,
        "nonce": nonce.to_string(),
        "guild_id": guild.map(|g| g.to_string()),
        "channel_id": channel.to_string(),
        "application_id": command.application_id.to_string(),
        "session_id": session_id,
        "data": {
            "version": command.version.to_string(),
            "id": command.id.to_string(),
            "name": command.name,
            "type": CHAT_INPUT,
            "options": nested,
            "application_command": command.raw,
            "attachments": [],
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> Index {
        Index::parse(&json!({
            "applications": [{ "id": "50", "name": "Bot", "icon": null }],
            "application_commands": [
                { "id": "1", "application_id": "50", "version": "9", "type": 1, "name": "ping", "description": "Pong",
                  "options": [{ "type": 3, "name": "text", "description": "What", "required": true }] },
                { "id": "2", "application_id": "50", "version": "9", "type": 1, "name": "config", "description": "Settings",
                  "options": [{ "type": 2, "name": "role", "description": "Roles", "options": [
                    { "type": 1, "name": "add", "description": "Add", "options": [{ "type": 8, "name": "role", "description": "Which" }] }] }] },
                { "id": "3", "application_id": "50", "version": "9", "type": 2, "name": "Report", "description": "" },
                { "id": "4", "application_id": "50", "version": "9", "type": 1, "name": "help", "description": "" }
            ]
        }))
    }

    #[test]
    fn searches_slash_commands_only() {
        let i = index();
        assert_eq!(i.search("", 10).iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["config", "help", "ping"]);
        assert_eq!(i.search("p", 10)[0].name, "ping");
        assert_eq!(i.search("el", 10).iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["help"]);
        assert_eq!(i.app(Snowflake(50)).unwrap().name, "Bot");
    }

    #[test]
    fn bodies_nest_subcommands_and_send_the_command_back() {
        let i = index();
        let ping = &i.commands[0];
        let b = command_body(ping, &[], vec![option_value(3, "text", json!("hi"))], Some(Snowflake(30)), Snowflake(20), "s", Snowflake(7));
        assert_eq!(b["type"], 2);
        assert_eq!(b["data"]["id"], "1");
        assert_eq!(b["data"]["version"], "9");
        assert_eq!(b["data"]["options"], json!([{ "type": 3, "name": "text", "value": "hi" }]));
        assert_eq!(b["data"]["application_command"]["name"], "ping");

        let config = &i.commands[1];
        let b = command_body(config, &[(2, "role".into()), (1, "add".into())], vec![option_value(8, "role", json!("77"))], None, Snowflake(20), "s", Snowflake(8));
        assert_eq!(
            b["data"]["options"],
            json!([{ "type": 2, "name": "role", "options": [{ "type": 1, "name": "add", "options": [{ "type": 8, "name": "role", "value": "77" }] }] }])
        );
    }
}
