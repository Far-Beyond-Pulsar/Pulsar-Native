use gpui::{AnyElement, App};
use std::any::TypeId;
use std::collections::HashMap;
use std::sync::LazyLock;

pub use pulsar_reflection::PropertyEditorArgs;

pub type PropertyEditorRenderFn = fn(&PropertyEditorArgs<'_>, &App) -> AnyElement;

pub struct PropertyEditorRegistry {
    editors: HashMap<TypeId, PropertyEditorRenderFn>,
}

impl PropertyEditorRegistry {
    fn new() -> Self {
        let mut editors = HashMap::new();

        for hint in pulsar_reflection::inventory::iter::<pulsar_reflection::UiPropertyEditorHint> {
            // SAFETY: every submitted UiPropertyEditorHint carries a function whose
            // actual signature is PropertyEditorRenderFn.  The erase/transmute pair
            // is sound as long as that invariant is upheld by all submitters.
            let f: PropertyEditorRenderFn = unsafe { std::mem::transmute(hint.fn_ptr) };
            editors.insert(hint.type_id, f);
        }

        tracing::info!(
            "PropertyEditorRegistry: {} registered editors",
            editors.len()
        );

        Self { editors }
    }

    pub fn get(&self, type_id: TypeId) -> Option<PropertyEditorRenderFn> {
        self.editors.get(&type_id).copied()
    }

    pub fn has(&self, type_id: TypeId) -> bool {
        self.editors.contains_key(&type_id)
    }

    pub fn len(&self) -> usize {
        self.editors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.editors.is_empty()
    }
}

pub static PROPERTY_EDITOR_REGISTRY: LazyLock<PropertyEditorRegistry> =
    LazyLock::new(PropertyEditorRegistry::new);
