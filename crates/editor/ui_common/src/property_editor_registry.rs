//! Lookup from a property's concrete type to the editor that renders it.
//!
//! Editors register themselves with `#[pulsar_type(editor = ...)]`, which
//! submits a [`UiPropertyEditorHint`](pulsar_reflection::UiPropertyEditorHint)
//! carrying a type-erased factory pointer; a reflected enum without one is
//! edited with the enum dropdown.  This registry transmutes those
//! back to [`PropertyEditorFactory`] at first access, and again when an attached
//! plugin library adds hints.
//!
//! Nothing here knows what an editor *is* beyond "a fn that builds a
//! [`BoundPropertyEditor`]" — the widgets, subscriptions and event handling all
//! live inside the editors themselves.

use std::any::TypeId;
use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

pub use pulsar_reflection::{BoundPropertyEditor, PropertyEditorArgs, PropertyEditorFactory};

/// Built from every registered hint: the editor's and those of every plugin
/// library attached to the world runtimes (Pulsar-Native#1083), rebuilt when
/// a plugin's hints arrive after first access.
pub struct PropertyEditorRegistry {
    factories: RwLock<(usize, HashMap<TypeId, PropertyEditorFactory>)>,
}

impl PropertyEditorRegistry {
    fn new() -> Self {
        Self {
            factories: RwLock::new((0, HashMap::new())),
        }
    }

    fn with<R>(&self, f: impl FnOnce(&HashMap<TypeId, PropertyEditorFactory>) -> R) -> R {
        let hints = pulsar_reflection::runtime::editor_hints();
        {
            let factories = self.factories.read().unwrap_or_else(|e| e.into_inner());
            if factories.0 == hints.len() {
                return f(&factories.1);
            }
        }
        let mut factories = self.factories.write().unwrap_or_else(|e| e.into_inner());
        if factories.0 != hints.len() {
            let mut map = HashMap::new();
            for hint in hints {
                // SAFETY: `erase_property_editor_fn_ptr` constrains its input to
                // `PropertyEditorFactory`, so every submitted `fn_ptr` has that type.
                let factory: PropertyEditorFactory = unsafe { std::mem::transmute(hint.fn_ptr) };
                map.insert(hint.type_id, factory);
            }
            *factories = (hints.len(), map);
        }
        f(&factories.1)
    }

    /// The editor registered for `type_id`; else the editor its reflection
    /// data asks for: the enum dropdown for an enum whose type info lists its
    /// variants, the line editor for a type registered as text-edited. Data
    /// types register no editor of their own, so their crates link no GPUI.
    pub fn get(&self, type_id: TypeId) -> Option<PropertyEditorFactory> {
        self.with(|factories| factories.get(&type_id).copied())
            .or_else(|| fallback(type_id))
    }

    pub fn has(&self, type_id: TypeId) -> bool {
        self.get(type_id).is_some()
    }

    pub fn len(&self) -> usize {
        self.with(HashMap::len)
    }

    pub fn is_empty(&self) -> bool {
        self.with(HashMap::is_empty)
    }
}

/// The editor a type's reflection data asks for, when none is registered.
fn fallback(type_id: TypeId) -> Option<PropertyEditorFactory> {
    let is_enum = pulsar_reflection::RUNTIME_TYPE_REGISTRY
        .get_by_id(type_id)
        .is_some_and(|info| info.is_enum());
    if is_enum {
        return Some(pulsar_reflection::enum_dropdown_editor as PropertyEditorFactory);
    }
    ui_types_common::text_edited(type_id)
        .map(|_| crate::text_line_editor::text_line_editor as PropertyEditorFactory)
}

pub static PROPERTY_EDITOR_REGISTRY: LazyLock<PropertyEditorRegistry> =
    LazyLock::new(PropertyEditorRegistry::new);
