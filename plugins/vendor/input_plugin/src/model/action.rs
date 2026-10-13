//! The input action asset: something the player can do, independent of which
//! key does it.
//!
//! An action says what it reports ([`ValueType`]), how that value is shaped
//! (modifiers), and when it fires (triggers). Which keys drive it is the
//! mapping's business, so one action can be bound differently per platform,
//! per game mode, or by the player.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::behavior::{Modifier, Tolerant, Trigger, ValueType};
use super::id::AssetId;
use super::{parse_object, Asset};

/// File extension of action assets.
pub const ACTION_EXTENSION: &str = "inputaction";

/// Default distance from rest at which a value counts as pressed.
pub const DEFAULT_THRESHOLD: f32 = 0.5;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ActionAsset {
    #[serde(default = "AssetId::unassigned")]
    pub id: AssetId,
    /// The name scripts and events know the action by (`Jump` for
    /// `input::Jump.triggered`). Empty: the file's name. Kept apart from the
    /// file name so renaming the file never silently renames the event.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub identifier: String,
    pub description: String,
    pub value_type: ValueType,
    /// A key that fires this action is hidden from lower-priority mappings.
    pub consume_input: bool,
    /// Keep firing while the game is paused (menus).
    pub trigger_when_paused: bool,
    /// How far from rest a value must be to count as pressed.
    pub actuation_threshold: f32,
    pub triggers: Vec<Tolerant<Trigger>>,
    pub modifiers: Vec<Tolerant<Modifier>>,
    /// Fields this version doesn't know, written back as they were read.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for ActionAsset {
    fn default() -> Self {
        Self {
            id: AssetId::new(),
            identifier: String::new(),
            description: String::new(),
            value_type: ValueType::Bool,
            consume_input: true,
            trigger_when_paused: false,
            actuation_threshold: DEFAULT_THRESHOLD,
            triggers: Vec::new(),
            modifiers: Vec::new(),
            extra: Map::new(),
        }
    }
}

impl ActionAsset {
    pub fn new() -> Self {
        Self::default()
    }

    /// The name events carry: the identifier, else `file_stem` made safe.
    pub fn event_name(&self, file_stem: &str) -> String {
        if self.identifier.trim().is_empty() {
            to_identifier(file_stem)
        } else {
            self.identifier.trim().to_string()
        }
    }

    /// Whether `name` can be an event name: letters, digits and underscores,
    /// not starting with a digit.
    pub fn is_valid_identifier(name: &str) -> bool {
        let mut chars = name.chars();
        chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    }

    pub fn known_triggers(&self) -> impl Iterator<Item = &Trigger> {
        self.triggers.iter().filter_map(Tolerant::known)
    }

    pub fn known_modifiers(&self) -> impl Iterator<Item = &Modifier> {
        self.modifiers.iter().filter_map(Tolerant::known)
    }
}

/// `text` as an identifier: other characters become `_`, words after a
/// space or dash are capitalised (`"open menu"` is `openMenu`-style
/// `OpenMenu`), and a leading digit gets a `_` in front.
pub fn to_identifier(text: &str) -> String {
    let mut out = String::new();
    let mut upper_next = true;
    for c in text.trim().chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(if upper_next { c.to_ascii_uppercase() } else { c });
            upper_next = false;
        } else {
            upper_next = true;
        }
    }
    if out.is_empty() || out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

impl Asset for ActionAsset {
    const KIND: &'static str = "input action";

    fn new_default() -> Self {
        Self::new()
    }

    fn parse(text: &str) -> Result<(Self, Vec<String>), String> {
        let value = parse_object(text)?;
        let mut notes = Vec::new();
        let mut action: ActionAsset = serde_json::from_value(value).map_err(|e| e.to_string())?;
        if !action.id.is_assigned() {
            // `stamp` gives it one; a file that is only being read has none.
            notes.push("This action had no id, so it was given one.".to_string());
        }
        let unknown = action.triggers.iter().filter(|t| t.known().is_none()).count()
            + action.modifiers.iter().filter(|m| m.known().is_none()).count();
        if unknown > 0 {
            notes.push(format!("{unknown} trigger or modifier made by a newer version is kept but can't be edited here."));
        }
        if !(0.0..=1.0).contains(&action.actuation_threshold) {
            action.actuation_threshold = action.actuation_threshold.clamp(0.0, 1.0);
            notes.push("The actuation threshold was outside 0 to 1 and was brought back in.".to_string());
        }
        Ok((action, notes))
    }

    fn to_text(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).unwrap_or_default();
        text.push('\n');
        text
    }

    fn stamp(&mut self) -> bool {
        if self.id.is_assigned() {
            return false;
        }
        self.id = AssetId::new();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_is_a_new_action_that_gets_an_id_on_stamp() {
        let (mut action, notes) = ActionAsset::parse("").unwrap();
        assert!(!action.id.is_assigned());
        assert!(!notes.is_empty());
        assert!(action.stamp());
        assert!(action.id.is_assigned());
        assert!(!action.stamp(), "stamping twice changes nothing");
        assert_eq!(action.value_type, ValueType::Bool);
        assert!(action.consume_input);
    }

    #[test]
    fn unknown_fields_and_triggers_survive_a_round_trip() {
        let id = AssetId::new();
        let text = format!(
            r#"{{"id":"{id}","value_type":"axis2d","future":{{"a":1}},"triggers":[{{"type":"pressed"}},{{"type":"mystery"}}]}}"#
        );
        let (action, notes) = ActionAsset::parse(&text).unwrap();
        assert_eq!(action.value_type, ValueType::Axis2D);
        assert_eq!(notes.len(), 1);
        let again = ActionAsset::parse(&action.to_text()).unwrap().0;
        assert_eq!(again, action);
        let json: Value = serde_json::from_str(&action.to_text()).unwrap();
        assert_eq!(json["future"], serde_json::json!({"a": 1}));
        assert_eq!(json["triggers"][1], serde_json::json!({"type": "mystery"}));
    }

    #[test]
    fn a_file_that_isnt_an_object_is_refused() {
        assert!(ActionAsset::parse("[1, 2]").is_err());
        assert!(ActionAsset::parse("{ nope").is_err());
        assert!(ActionAsset::parse("null").is_ok());
    }

    #[test]
    fn event_names_come_from_the_identifier_or_the_file_name() {
        let mut a = ActionAsset::new();
        assert_eq!(a.event_name("open menu"), "OpenMenu");
        assert_eq!(a.event_name("2nd_jump"), "_2nd_jump");
        a.identifier = "Jump".into();
        assert_eq!(a.event_name("anything"), "Jump");
        assert!(ActionAsset::is_valid_identifier("Jump_2"));
        assert!(!ActionAsset::is_valid_identifier("2Jump"));
        assert!(!ActionAsset::is_valid_identifier("a b"));
        assert!(!ActionAsset::is_valid_identifier(""));
    }

    #[test]
    fn the_threshold_is_kept_in_range() {
        let (a, notes) = ActionAsset::parse(r#"{"actuation_threshold": 4.0}"#).unwrap();
        assert_eq!(a.actuation_threshold, 1.0);
        assert!(notes.iter().any(|n| n.contains("threshold")));
    }
}
