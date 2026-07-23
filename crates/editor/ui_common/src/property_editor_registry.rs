//! Lookup from a property's concrete type to the editor that renders it.
//!
//! Editors register themselves with `#[pulsar_type(editor = ...)]`, which
//! submits a [`UiPropertyEditorHint`](pulsar_reflection::UiPropertyEditorHint)
//! carrying a type-erased factory pointer.  This registry transmutes those
//! back to [`PropertyEditorFactory`] once, at first access.
//!
//! Nothing here knows what an editor *is* beyond "a fn that builds a
//! [`BoundPropertyEditor`]" — the widgets, subscriptions and event handling all
//! live inside the editors themselves.

use std::any::TypeId;
use std::collections::HashMap;
use std::sync::LazyLock;

pub use pulsar_reflection::{BoundPropertyEditor, PropertyEditorArgs, PropertyEditorFactory};

pub struct PropertyEditorRegistry {
    factories: HashMap<TypeId, PropertyEditorFactory>,
}

impl PropertyEditorRegistry {
    fn new() -> Self {
        let mut factories = HashMap::new();

        for hint in pulsar_reflection::inventory::iter::<pulsar_reflection::UiPropertyEditorHint> {
            // SAFETY: `erase_property_editor_fn_ptr` constrains its input to
            // `PropertyEditorFactory`, so every submitted `fn_ptr` has that type.
            let f: PropertyEditorFactory = unsafe { std::mem::transmute(hint.fn_ptr) };
            factories.insert(hint.type_id, f);
        }

        Self { factories }
    }

    pub fn get(&self, type_id: TypeId) -> Option<PropertyEditorFactory> {
        self.factories.get(&type_id).copied()
    }

    pub fn has(&self, type_id: TypeId) -> bool {
        self.factories.contains_key(&type_id)
    }

    pub fn len(&self) -> usize {
        self.factories.len()
    }

    pub fn is_empty(&self) -> bool {
        self.factories.is_empty()
    }
}

pub static PROPERTY_EDITOR_REGISTRY: LazyLock<PropertyEditorRegistry> =
    LazyLock::new(PropertyEditorRegistry::new);
