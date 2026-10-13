//! The widget format as typed data: ids, the property schema, the node tree,
//! the asset file, editing operations and validation.
//!
//! Nothing here knows about GPUI, so all of it is unit-tested. A game runtime
//! can load widgets with this module and draw them with [`crate::layout`] and
//! [`crate::display`] without the editor.

pub mod geom;
pub mod id;
pub mod node;
pub mod ops;
pub mod schema;
pub mod validate;
pub mod widget;

pub use geom::{Affine, Color, Insets, Rect, Vec2};
pub use id::{AssetPath, NodeId, ParamId, WidgetId};
pub use node::{Arg, Instance, Node, Props};
pub use schema::{Kind, SlotKind, ValueClass};
pub use widget::{Parameter, Target, WidgetAsset};

use serde::{Deserialize, Serialize};

/// How serious a validation finding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Severity {
    /// Worth knowing, nothing is wrong.
    Info,
    /// Legal, but probably not what was meant.
    Warning,
    /// The widget will not work as built.
    Error,
}

/// One validation finding.
#[derive(Clone, Debug, PartialEq)]
pub struct Issue<F> {
    pub severity: Severity,
    /// The node it concerns, when there is one.
    pub node: Option<NodeId>,
    /// What it concerns, for grouping and display.
    pub subject: String,
    pub message: String,
    /// A one-click repair, when there is an obvious one.
    pub fix: Option<F>,
}

impl<F> Issue<F> {
    pub fn new(severity: Severity, subject: impl Into<String>, message: impl Into<String>) -> Self {
        Self { severity, node: None, subject: subject.into(), message: message.into(), fix: None }
    }

    pub fn on(mut self, node: NodeId) -> Self {
        self.node = Some(node);
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
