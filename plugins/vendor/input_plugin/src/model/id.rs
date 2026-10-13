//! Identity: what an action or a mapping is called to the rest of the project.
//!
//! An [`AssetId`] is generated once, stored in the file, and never changes.
//! Mappings refer to actions by it, and anything else that refers to a mapping
//! or an action (a component, a script) should do the same, so renaming or
//! moving a file breaks nothing.

use serde::{Deserialize, Serialize};

/// A stable id: a lower-case hyphenated UUID, or empty for an asset that has
/// not been given one yet.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AssetId(String);

impl AssetId {
    /// A fresh random id.
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    /// No id yet.
    pub fn unassigned() -> Self {
        Self(String::new())
    }

    /// The id in `text`, if it is a UUID (any case, braces or not).
    pub fn parse(text: &str) -> Option<Self> {
        uuid::Uuid::parse_str(text.trim().trim_matches(|c| c == '{' || c == '}')).ok().map(|u| Self(u.to_string()))
    }

    pub fn is_assigned(&self) -> bool {
        !self.0.is_empty()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The first group, enough to tell ids apart on screen.
    pub fn short(&self) -> &str {
        self.0.split('-').next().unwrap_or(&self.0)
    }
}

impl std::fmt::Display for AssetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A reference to an action from a mapping (or a chord from a trigger).
///
/// `id` is what counts. `hint` is where the action's file was when the
/// reference was made, kept so a person reading the file, or a tool that has
/// not scanned the project, can find it; it is refreshed whenever the editor
/// sees the action somewhere else.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ActionRef {
    pub id: AssetId,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hint: String,
}

impl ActionRef {
    pub fn new(id: AssetId, hint: impl Into<String>) -> Self {
        Self { id, hint: hint.into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_parse_in_any_spelling_and_normalise() {
        let id = AssetId::new();
        let upper = id.as_str().to_uppercase();
        assert_eq!(AssetId::parse(&upper), Some(id.clone()));
        assert_eq!(AssetId::parse(&format!("{{{id}}}")), Some(id));
        assert_eq!(AssetId::parse("not an id"), None);
        assert!(!AssetId::unassigned().is_assigned());
    }

    #[test]
    fn a_reference_without_a_hint_stays_without_one_in_the_file() {
        let r = ActionRef::new(AssetId::new(), "");
        let text = serde_json::to_string(&r).unwrap();
        assert!(!text.contains("hint"));
        let back: ActionRef = serde_json::from_str(&text).unwrap();
        assert_eq!(back, r);
    }
}
