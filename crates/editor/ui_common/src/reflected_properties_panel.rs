//! Shared reflected properties panel system
//!
//! This module provides a centralized system for rendering reflected component
//! properties that is used by the level editor, the blueprint prefab editor,
//! the shader/blueprint node inspectors and the asset import configurator.
//!
//! # Division of responsibility
//!
//! This layer knows about exactly three things: a property's
//! [`RuntimeTypeInfo`], its current value as `&dyn Any`, and a write-back
//! callback.  It has **no** knowledge of widgets — not their types, not their
//! events, not their lifecycle.  Each editor is a self-contained GPUI entity
//! that creates its own child entities, registers its own subscriptions and
//! calls `write_back` itself.  Adding a new property type means registering an
//! editor with `#[pulsar_type(editor = ...)]`; nothing in this file changes.

use gpui::{prelude::*, *};
use pulsar_reflection::{BoundPropertyEditor, RuntimeTypeInfo};
use serde_json::Value;
use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

// ============================================================================
// Type Aliases and Traits
// ============================================================================

/// Callback for reading a property value from the data source
pub type PropertyReader = Arc<dyn Fn(&str, &str) -> Value + Send + Sync>;

/// Callback for writing a property value to the data source
pub type PropertyWriter = Arc<dyn Fn(&str, &str, Value) + Send + Sync>;

/// Configuration for the reflected properties panel
pub struct ReflectedPropertiesPanelConfig {
    /// ID prefix for UI elements (e.g., "level", "prefab")
    pub id_prefix: String,
    /// Whether to show the component hierarchy list
    pub show_component_list: bool,
    /// Optional component list renderer
    pub component_list_renderer: Option<Box<dyn Fn(&mut App) -> AnyElement>>,
}

impl Default for ReflectedPropertiesPanelConfig {
    fn default() -> Self {
        Self {
            id_prefix: "panel".to_string(),
            show_component_list: false,
            component_list_renderer: None,
        }
    }
}

// ============================================================================
// Color Utilities
// ============================================================================

pub fn rgba_to_hsla(rgba: [f32; 4]) -> Hsla {
    pulsar_reflection::prims::core::color::rgba_to_hsla(rgba)
}

pub fn hsla_to_rgba(hsla: Hsla) -> [f32; 4] {
    pulsar_reflection::prims::core::color::hsla_to_rgba(hsla)
}

// ============================================================================
// Property State Management
// ============================================================================

/// Cache of live property-editor instances, keyed by `(class_name, prop_name)`.
///
/// Panels own one of these and hand it to [`render_property_row_runtime`].  An
/// editor is constructed the first time its property is rendered and reused
/// afterwards, so it keeps focus, caret position and popover state across
/// re-renders.
///
/// Entries are opaque: [`BoundPropertyEditor`] is a renderable handle plus a
/// type-erased value setter, so this cache never learns what kind of widget it
/// is holding.
pub struct PropertyStateManager {
    editors: HashMap<(String, String), BoundPropertyEditor>,
}

impl PropertyStateManager {
    pub fn new() -> Self {
        Self {
            editors: HashMap::new(),
        }
    }

    /// Drop every cached editor.
    ///
    /// Call this when the panel switches to a different object so stale editor
    /// state cannot leak across selections.
    pub fn clear(&mut self) {
        self.editors.clear();
    }
}

impl Default for PropertyStateManager {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Runtime-Type-Aware Property Rendering
// ============================================================================

/// Render one property row.
///
/// Looks up the editor registered for `type_info`, constructing it on first use
/// and reusing the cached instance afterwards, then pushes the current value
/// into it before rendering.  Returns a placeholder when the property's type
/// has no registered editor.
pub fn render_property_row_runtime<V: 'static>(
    state: &mut PropertyStateManager,
    id_prefix: &str,
    class_name: &str,
    display_name: &str,
    prop_name: &str,
    type_info: &'static RuntimeTypeInfo,
    current_value: &dyn Any,
    write_back: Arc<dyn Fn(Box<dyn Any + Send>, &mut Window, &mut App) + Send + Sync>,
    window: &mut Window,
    cx: &mut Context<V>,
) -> AnyElement {
    use crate::property_editor_registry::{PropertyEditorArgs, PROPERTY_EDITOR_REGISTRY};

    let Some(factory) = PROPERTY_EDITOR_REGISTRY.get(type_info.type_id) else {
        return div().text_sm().child("(nyi)").into_any_element();
    };

    let key = (class_name.to_string(), prop_name.to_string());
    let editor = match state.editors.get(&key) {
        Some(editor) => editor.clone(),
        None => {
            let args = PropertyEditorArgs {
                id_prefix,
                class_name,
                display_name,
                prop_name,
                type_info,
                current_json: &Value::Null,
                current_value,
                write_back,
            };
            let editor = factory(&args, window, cx);
            state.editors.insert(key, editor.clone());
            editor
        }
    };

    // Push the latest value in so edits made elsewhere (undo, a viewport drag,
    // another panel) reach the editor. Editors no-op when it is unchanged.
    (editor.set_value)(current_value, window, cx);

    editor.view.into_any_element()
}
