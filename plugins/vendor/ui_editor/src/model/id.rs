//! Identity: what a widget is called to the rest of the project, and what a
//! node is called inside its widget.
//!
//! A widget's [`WidgetId`] is generated once, stored in the file, and never
//! changes. Everything that refers to a widget (another widget's instance of
//! it, a level, a script) refers to that id, never to its name or path, so
//! renaming or moving the file breaks nothing.

use serde::{Deserialize, Serialize};

/// A widget's stable id: a lower-case hyphenated UUID.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WidgetId(String);

impl WidgetId {
    /// A fresh random id.
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    /// The id in `text`, if it is a UUID (any case, braces or not).
    pub fn parse(text: &str) -> Option<Self> {
        uuid::Uuid::parse_str(text.trim().trim_matches(|c| c == '{' || c == '}')).ok().map(|u| Self(u.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The first group, enough to tell ids apart on screen.
    pub fn short(&self) -> &str {
        self.0.split('-').next().unwrap_or(&self.0)
    }
}

impl Default for WidgetId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for WidgetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A node's id within its widget. Unique in the widget, never reused, and
/// unrelated to the node's name, so renaming never breaks a parameter that
/// points at it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub u32);

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// A parameter's id within its widget. Instances of the widget set parameters
/// by this id, so renaming a parameter breaks none of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ParamId(pub u32);

/// A reference to an image (or other file asset) in the project.
///
/// The engine has no id service for file assets yet, so these are
/// project-relative paths with `/` separators. Widgets are the exception:
/// they always use [`WidgetId`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct AssetPath(pub String);

impl AssetPath {
    pub fn is_empty(&self) -> bool {
        self.0.trim().is_empty()
    }

    /// The file name without its directories.
    pub fn file_name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_parse_in_any_spelling_and_print_canonically() {
        let id = WidgetId::new();
        assert_eq!(WidgetId::parse(id.as_str()), Some(id.clone()));
        let upper = id.as_str().to_uppercase();
        assert_eq!(WidgetId::parse(&upper), Some(id.clone()));
        assert_eq!(WidgetId::parse(&format!("{{{}}}", id.as_str())), Some(id.clone()));
        assert_eq!(WidgetId::parse("not an id"), None);
        assert_eq!(WidgetId::parse(""), None);
        assert_eq!(id.short().len(), 8);
        assert_ne!(WidgetId::new(), WidgetId::new());
    }
}
