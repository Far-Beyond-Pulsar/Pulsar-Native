//! Values the inspector edits as one line of text.
//!
//! A data type that reads and shows as a line (a list of names and ids, a
//! set of tags) says how through a [`TextEditedType`] registration; the
//! editor's property registry then edits it with its line editor. The
//! type's crate names no editor and links no GPUI.

use std::any::{Any, TypeId};

/// How values of one type read from and show as a line of text.
pub struct TextEditedType {
    /// The type registered.
    pub type_id: TypeId,
    /// The line a value shows as; `None` if the value is not of the type.
    pub to_text: fn(&dyn Any) -> Option<String>,
    /// The value a line reads as, or why it does not read.
    pub from_text: fn(&str) -> Result<Box<dyn Any + Send>, String>,
}

inventory::collect!(TextEditedType);

/// The text editing registered for `type_id`, if any.
pub fn text_edited(type_id: TypeId) -> Option<&'static TextEditedType> {
    inventory::iter::<TextEditedType>
        .into_iter()
        .find(|registration| registration.type_id == type_id)
}
