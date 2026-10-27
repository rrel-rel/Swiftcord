//! Using apps' buttons, select menus and modals (POST `/interactions`). The app's answer arrives over the
//! gateway: INTERACTION_SUCCESS or INTERACTION_FAILURE by our nonce, INTERACTION_MODAL_CREATE when it opens a
//! form, and its reply or edit as an ordinary message.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::models::{component_type as kind, Component, Message};
use crate::Snowflake;

const MESSAGE_COMPONENT: u8 = 3;
const MODAL_SUBMIT: u8 = 5;

/// The app behind a message: the one that sent it, or for a plain bot message the bot itself.
pub fn application_of(msg: &Message) -> Snowflake {
    msg.application_id.unwrap_or(msg.author.id)
}

/// Clicking a button (`values: None`) or choosing in a select menu.
pub fn component_body(msg: &Message, c: &Component, values: Option<&[String]>, session_id: &str, nonce: Snowflake) -> Value {
    let mut data = json!({ "component_type": c.kind, "custom_id": c.custom_id.as_deref().unwrap_or_default() });
    if let Some(v) = values {
        data["type"] = json!(c.kind);
        data["values"] = json!(v);
    }
    json!({
        "type": MESSAGE_COMPONENT,
        "nonce": nonce.to_string(),
        "guild_id": msg.guild_id.map(|g| g.to_string()),
        "channel_id": msg.channel_id.to_string(),
        "message_flags": msg.flags,
        "message_id": msg.id.to_string(),
        "application_id": application_of(msg).to_string(),
        "session_id": session_id,
        "data": data,
    })
}

/// A form an app opened (INTERACTION_MODAL_CREATE).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Modal {
    /// The interaction that opened it.
    pub id: Snowflake,
    pub nonce: Option<String>,
    pub custom_id: String,
    pub title: String,
    pub components: Vec<Component>,
    pub application: ModalApp,
    pub channel_id: Option<Snowflake>,
    pub guild_id: Option<Snowflake>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ModalApp {
    pub id: Snowflake,
    pub name: String,
}

/// One field of a modal, whatever wraps it (an action row in older modals, a label in newer ones).
#[derive(Debug, Clone)]
pub struct Field {
    pub custom_id: String,
    pub kind: u8,
    pub label: String,
    pub description: Option<String>,
    /// 1 short, 2 paragraph (text inputs).
    pub style: u8,
    pub placeholder: Option<String>,
    pub required: bool,
    pub min_length: Option<u32>,
    pub max_length: Option<u32>,
    /// A text input's text, or a select's chosen values.
    pub value: String,
    pub values: Vec<String>,
    /// A string select's choices.
    pub options: Vec<crate::models::SelectOption>,
    pub max_values: u32,
    /// Wrapped in a label (newer modals) rather than an action row.
    pub labelled: bool,
}

impl Modal {
    /// The fields to fill in, in order, with any prefilled values.
    pub fn fields(&self) -> Vec<Field> {
        let mut out = Vec::new();
        for top in &self.components {
            let (inner, labelled, label, description) = match top.kind {
                kind::LABEL => match top.component.as_deref() {
                    Some(c) => (vec![c], true, top.label.clone(), top.description.clone()),
                    None => continue,
                },
                kind::ACTION_ROW => (top.components.iter().collect(), false, None, None),
                _ => continue,
            };
            for c in inner {
                if c.kind != kind::TEXT_INPUT && c.kind != kind::STRING_SELECT {
                    continue;
                }
                let defaults: Vec<String> = c.options.iter().filter(|o| o.default).map(|o| o.value.clone()).collect();
                out.push(Field {
                    custom_id: c.custom_id.clone().unwrap_or_default(),
                    kind: c.kind,
                    label: label.clone().or_else(|| c.label.clone()).unwrap_or_default(),
                    description: description.clone(),
                    style: c.style.unwrap_or(1),
                    placeholder: c.placeholder.clone(),
                    required: c.required.unwrap_or(true),
                    min_length: c.min_length,
                    max_length: c.max_length,
                    value: c.value.clone().unwrap_or_default(),
                    values: defaults,
                    options: c.options.clone(),
                    max_values: c.max_values.unwrap_or(1),
                    labelled,
                });
            }
        }
        out
    }
}

/// What's wrong with a field's input, if anything.
pub fn field_problem(f: &Field) -> Option<String> {
    if f.kind == kind::STRING_SELECT {
        return (f.required && f.values.is_empty()).then(|| format!("Choose something for “{}”.", f.label));
    }
    let n = f.value.chars().count() as u32;
    if f.required && n == 0 {
        return Some(format!("“{}” is required.", f.label));
    }
    if n > 0 && f.min_length.is_some_and(|m| n < m) {
        return Some(format!("“{}” needs at least {} characters.", f.label, f.min_length.unwrap_or(0)));
    }
    None
}

/// Submitting a modal: the fields' values, wrapped the way the modal wrapped them.
pub fn modal_body(modal: &Modal, channel: Snowflake, guild: Option<Snowflake>, fields: &[Field], session_id: &str, nonce: Snowflake) -> Value {
    let components: Vec<Value> = fields
        .iter()
        .map(|f| {
            let inner = if f.kind == kind::STRING_SELECT {
                json!({ "type": f.kind, "custom_id": f.custom_id, "values": f.values })
            } else {
                json!({ "type": f.kind, "custom_id": f.custom_id, "value": f.value })
            };
            if f.labelled { json!({ "type": kind::LABEL, "component": inner }) } else { json!({ "type": kind::ACTION_ROW, "components": [inner] }) }
        })
        .collect();
    json!({
        "type": MODAL_SUBMIT,
        "nonce": nonce.to_string(),
        "guild_id": guild.map(|g| g.to_string()),
        "channel_id": channel.to_string(),
        "application_id": modal.application.id.to_string(),
        "session_id": session_id,
        "data": { "id": modal.id.to_string(), "custom_id": modal.custom_id, "components": components },
    })
}

/// INTERACTION_SUCCESS / INTERACTION_FAILURE: which of ours it was (by nonce).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Outcome {
    pub id: Snowflake,
    pub nonce: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bot_message() -> Message {
        serde_json::from_str(
            r#"{"id":"10","channel_id":"20","guild_id":"30","author":{"id":"40","username":"bot","bot":true},"application_id":"50","flags":0,
                "components":[{"type":1,"components":[{"type":2,"style":1,"label":"Go","custom_id":"go"},
                  {"type":3,"custom_id":"pick","max_values":2,"options":[{"label":"A","value":"a"},{"label":"B","value":"b","default":true}]}]}]}"#,
        )
        .unwrap()
    }

    #[test]
    fn button_and_select_bodies() {
        let m = bot_message();
        let row = &m.components[0].components;
        let b = component_body(&m, &row[0], None, "sess", Snowflake(99));
        assert_eq!(b["type"], 3);
        assert_eq!(b["application_id"], "50");
        assert_eq!(b["message_id"], "10");
        assert_eq!(b["nonce"], "99");
        assert_eq!(b["data"], json!({ "component_type": 2, "custom_id": "go" }));
        assert_eq!(row[1].options.len(), 2);
        let s = component_body(&m, &row[1], Some(&["a".into(), "b".into()]), "sess", Snowflake(100));
        assert_eq!(s["data"], json!({ "component_type": 3, "custom_id": "pick", "type": 3, "values": ["a", "b"] }));
    }

    #[test]
    fn plain_bot_messages_use_the_bot_as_the_app() {
        let mut m = bot_message();
        m.application_id = None;
        assert_eq!(application_of(&m), Snowflake(40));
    }

    #[test]
    fn modals_read_both_layouts_and_submit_in_kind() {
        let modal: Modal = serde_json::from_str(
            r#"{"id":"7","nonce":"99","custom_id":"form","title":"Report","application":{"id":"50","name":"Bot"},
                "components":[
                  {"type":1,"components":[{"type":4,"custom_id":"why","label":"Why?","style":2,"required":true,"min_length":5}]},
                  {"type":18,"label":"Name","description":"Yours","component":{"type":4,"custom_id":"name","style":1,"value":"pre","required":false}},
                  {"type":18,"label":"Pick","component":{"type":3,"custom_id":"sel","options":[{"label":"X","value":"x"}]}}]}"#,
        )
        .unwrap();
        let mut fields = modal.fields();
        assert_eq!(fields.len(), 3);
        assert_eq!((fields[0].label.as_str(), fields[0].style, fields[0].labelled), ("Why?", 2, false));
        assert_eq!((fields[1].label.as_str(), fields[1].value.as_str(), fields[1].required), ("Name", "pre", false));
        assert!(field_problem(&fields[0]).is_some(), "required and empty");
        fields[0].value = "abc".into();
        assert!(field_problem(&fields[0]).unwrap().contains("at least 5"));
        fields[0].value = "because".into();
        assert!(field_problem(&fields[0]).is_none());
        assert!(field_problem(&fields[2]).is_some());
        fields[2].values = vec!["x".into()];
        let body = modal_body(&modal, Snowflake(20), Some(Snowflake(30)), &fields, "sess", Snowflake(101));
        assert_eq!(body["type"], 5);
        assert_eq!(body["data"]["id"], "7");
        assert_eq!(body["data"]["components"][0], json!({ "type": 1, "components": [{ "type": 4, "custom_id": "why", "value": "because" }] }));
        assert_eq!(body["data"]["components"][1], json!({ "type": 18, "component": { "type": 4, "custom_id": "name", "value": "pre" } }));
        assert_eq!(body["data"]["components"][2], json!({ "type": 18, "component": { "type": 3, "custom_id": "sel", "values": ["x"] } }));
    }
}
