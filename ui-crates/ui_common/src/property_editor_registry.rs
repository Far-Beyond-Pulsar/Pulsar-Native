//! Per-type GPUI property editor registry.
//!
//! Types register a custom editor via the `editor = fn` argument on
//! [`pulsar_reflection::pulsar_type`]:
//!
//! ```rust,ignore
//! // Define the render function anywhere GPUI types are available.
//! fn render_my_type(args: &PropertyEditorArgs<'_>, cx: &gpui::App) -> gpui::AnyElement {
//!     gpui::div().child(args.display_name.to_string()).into_any_element()
//! }
//!
//! // Pass it to the macro — the macro generates the inventory::submit! for you.
//! #[pulsar_reflection::pulsar_type(
//!     primitive,
//!     serialize_json_with   = my_serialize,
//!     deserialize_json_with = my_deserialize,
//!     editor = render_my_type,          // ← new
//! )]
//! type RegisteredMyType = MyType;
//! ```
//!
//! If the calling crate has no GPUI dependency (e.g. `pulsar_reflection/prims`),
//! submit the hint manually from the nearest GPUI-aware companion file instead:
//!
//! ```rust,ignore
//! // In ui_common/src/prim_editors/color.rs (or similar):
//! fn render_color(args: &PropertyEditorArgs<'_>, cx: &gpui::App) -> gpui::AnyElement { … }
//!
//! pulsar_reflection::inventory::submit! {
//!     pulsar_reflection::UiPropertyEditorHint {
//!         type_id: std::any::TypeId::of::<[f32; 4]>(),
//!         fn_ptr:  render_color as usize,
//!     }
//! }
//! ```
//!
//! Either way, the registry is populated automatically at startup with zero
//! manual wiring in the central dispatcher.

use gpui::{AnyElement, App, Entity, Window};
use pulsar_reflection::RuntimeTypeInfo;
use serde_json::Value;
use std::any::TypeId;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use ui::color_picker::ColorPickerState;
use ui::input::InputState;

use crate::MeshAssetPicker;

// ── Args passed to every registered editor ────────────────────────────────────

/// Everything a property editor needs to render one row in the inspector.
pub struct PropertyEditorArgs<'a> {
    /// Unique prefix for GPUI element IDs (e.g. `"level"`, `"prefab"`).
    pub id_prefix: &'a str,
    /// Reflection class name of the owning component.
    pub class_name: &'a str,
    /// Human-readable field label shown in the UI.
    pub display_name: &'a str,
    /// Raw field name (snake_case).
    pub prop_name: &'a str,
    /// Runtime type information for this field.
    pub type_info: &'static RuntimeTypeInfo,
    /// Current serialised value from the scene database.
    pub current_json: &'a Value,
    /// Pre-created numeric input state (for `f32` / `i32`). `None` otherwise.
    pub numeric_input: Option<Entity<InputState>>,
    /// Pre-created colour picker state (for `[f32; 4]`). `None` otherwise.
    pub color_picker: Option<Entity<ColorPickerState>>,
    /// Pre-created mesh asset picker state (for `MeshAssetPath`). `None` otherwise.
    pub mesh_picker: Option<Entity<MeshAssetPicker>>,
    /// Invoked when the user toggles a boolean field.
    pub on_bool_toggle: Arc<dyn Fn(bool, &mut Window, &mut App) + Send + Sync>,
    /// Invoked when the user selects an enum variant by index.
    pub on_enum_select: Arc<dyn Fn(usize, &mut Window, &mut App) + Send + Sync>,
}

// ── Concrete fn type — lives only in this GPUI-aware crate ───────────────────

/// Signature every registered property editor must satisfy.
pub type PropertyEditorRenderFn = fn(&PropertyEditorArgs<'_>, &App) -> AnyElement;

// ── Registry ──────────────────────────────────────────────────────────────────

/// Global registry populated from [`pulsar_reflection::UiPropertyEditorHint`]
/// inventory entries at startup.
pub struct PropertyEditorRegistry {
    editors: HashMap<TypeId, PropertyEditorRenderFn>,
}

impl PropertyEditorRegistry {
    fn new() -> Self {
        let mut editors = HashMap::new();

        for hint in pulsar_reflection::inventory::iter::<pulsar_reflection::UiPropertyEditorHint> {
            // SAFETY: By convention every `UiPropertyEditorHint` is created by
            // either `#[pulsar_type(editor = fn)]` or a manual `inventory::submit!`
            // where the author guarantees the function has exactly the
            // `PropertyEditorRenderFn` signature.  The cast is sound as long as
            // that contract is upheld — fn pointers have a stable, well-defined
            // representation and calling convention on all supported platforms.
            let f: PropertyEditorRenderFn = unsafe { std::mem::transmute(hint.fn_ptr) };
            editors.insert(hint.type_id, f);
        }

        tracing::info!(
            "PropertyEditorRegistry: {} registered editors",
            editors.len()
        );

        Self { editors }
    }

    /// Return the render function for `type_id`, if one is registered.
    pub fn get(&self, type_id: TypeId) -> Option<PropertyEditorRenderFn> {
        self.editors.get(&type_id).copied()
    }

    /// `true` when a custom editor exists for `type_id`.
    pub fn has(&self, type_id: TypeId) -> bool {
        self.editors.contains_key(&type_id)
    }

    /// Number of registered editors.
    pub fn len(&self) -> usize {
        self.editors.len()
    }

    /// `true` when no editors are registered.
    pub fn is_empty(&self) -> bool {
        self.editors.is_empty()
    }
}

/// Global singleton — lazily initialised on first access.
pub static PROPERTY_EDITOR_REGISTRY: LazyLock<PropertyEditorRegistry> =
    LazyLock::new(PropertyEditorRegistry::new);
