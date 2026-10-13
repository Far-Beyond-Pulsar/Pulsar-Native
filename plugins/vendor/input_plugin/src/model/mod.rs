//! The input formats as typed data: ids, triggers and modifiers, the action
//! and mapping files, editing operations and validation.
//!
//! Nothing here knows about GPUI, so all of it is unit-tested, and the
//! runtime (`crate::runtime`) loads the same types the editors edit.

pub mod action;
pub mod behavior;
pub mod id;
pub mod mapping;
pub mod presets;
pub mod validate;

pub use action::ActionAsset;
pub use behavior::{Curve, Modifier, Swizzle, Tolerant, Trigger, ValueType};
pub use id::{ActionRef, AssetId};
pub use mapping::{Category, Entry, MappingAsset};

use serde::{Deserialize, Serialize};

/// How serious a validation finding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Severity {
    /// Worth knowing, nothing is wrong.
    Info,
    /// Legal, but probably not what was meant.
    Warning,
    /// It will not work as built.
    Error,
}

/// One validation finding.
#[derive(Clone, Debug, PartialEq)]
pub struct Issue<F> {
    pub severity: Severity,
    /// The category it concerns, when there is one.
    pub category: Option<u32>,
    /// The entry it concerns, when there is one.
    pub entry: Option<u32>,
    /// What it concerns, for grouping and display.
    pub subject: String,
    pub message: String,
    /// A one-click repair, when there is an obvious one.
    pub fix: Option<F>,
}

impl<F> Issue<F> {
    pub fn new(severity: Severity, subject: impl Into<String>, message: impl Into<String>) -> Self {
        Self { severity, category: None, entry: None, subject: subject.into(), message: message.into(), fix: None }
    }

    pub fn on_entry(mut self, category: u32, entry: u32) -> Self {
        self.category = Some(category);
        self.entry = Some(entry);
        self
    }

    pub fn on_category(mut self, category: u32) -> Self {
        self.category = Some(category);
        self
    }

    pub fn with_fix(mut self, fix: F) -> Self {
        self.fix = Some(fix);
        self
    }
}

/// An asset the document layer can load, save and diff.
pub trait Asset: Clone + PartialEq + Send + 'static {
    /// What the file type is called in messages.
    const KIND: &'static str;

    /// A fresh asset, as the "new file" template creates one.
    fn new_default() -> Self;

    /// Parse the file's text, with notes about anything repaired on the way.
    /// `Err` only when the text isn't usable at all.
    fn parse(text: &str) -> Result<(Self, Vec<String>), String>;

    /// The file's text.
    fn to_text(&self) -> String;

    /// Called after loading. Returns `true` when loading made a change that
    /// belongs on disk straight away (an id assigned to a file that had
    /// none), so the document writes the file without an undoable edit.
    fn stamp(&mut self) -> bool {
        false
    }
}

/// Parse `text` as a JSON object; an empty file is an empty object.
pub(crate) fn parse_object(text: &str) -> Result<serde_json::Value, String> {
    if text.trim().is_empty() {
        return Ok(serde_json::Value::Object(Default::default()));
    }
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(v @ serde_json::Value::Object(_)) => Ok(v),
        Ok(serde_json::Value::Null) => Ok(serde_json::Value::Object(Default::default())),
        Ok(_) => Err("the file isn't an object".to_string()),
        Err(error) => Err(error.to_string()),
    }
}

/// The `id` field of a file's text, if it has a valid one. Cheap enough to
/// run over every file of a project.
pub fn peek_id(text: &str) -> Option<AssetId> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    AssetId::parse(value.get("id")?.as_str()?)
}
