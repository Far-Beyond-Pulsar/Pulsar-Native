//! The item side of a reflected list: what a `Vec<T>` property's items say
//! about themselves to the editor that lists them (the editor itself is
//! `ui_common::reflected_list`, registered by the plugin that owns `T`).

use pulsar_reflection::{EngineClass, PropertyMetadata};

/// A value edited as an item of a `Vec` property by the editor's reflected list editor (`ui_common::reflected_list`).
pub trait ReflectedListItem:
    EngineClass + Clone + Default + serde::Serialize + Send + Sync + 'static
{
    /// Class name handed to the field editors.
    const CLASS_NAME: &'static str;
    /// What one item is called on the add button ("element").
    const NOUN: &'static str;
    /// Most items the list holds; the add button hides at this length.
    const MAX_ITEMS: usize = usize::MAX;

    /// The item's card title; `index` is its position.
    fn title(&self, index: usize) -> String;

    /// Whether the item's card shows `prop`. An item whose fields mean
    /// different things by kind shows only those its kind reads; its rows
    /// rebuild when the answer changes.
    fn shows(&self, _prop: &PropertyMetadata) -> bool {
        true
    }

    /// Store an edited item. Returns true when more than the edited field
    /// changed (a new kind bringing its defaults), so every row refreshes.
    fn apply_edit(&mut self, edited: Self) -> bool {
        *self = edited;
        false
    }
}
