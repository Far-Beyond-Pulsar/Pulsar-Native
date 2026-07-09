//! Script component — attaches a blueprint actor script to a scene object.

use engine_class_derive::{engine_class, register_runtime_behavior, register_scene_props_applier};
use pulsar_events::{script_registry, ScriptRegistration};
use pulsar_reflection::{
    get_subsystem, ComponentRuntimeBehavior, ComponentRuntimeContext, LiveKeySet, ReflectError,
    RuntimeComponentOwner, ScenePropsProjector,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

// ── ScriptAssetPath ───────────────────────────────────────────────────────────

/// Strongly-typed wrapper for blueprint asset paths.
///
/// The underlying value is the absolute or project-relative path to the
/// blueprint directory — the directory that contains `graph_save.json`.
///
/// Serialises transparently as a JSON string so scene files require no
/// migration if the wrapper is introduced after the fact.
///
/// Using this as a field type causes the reflection property inspector to
/// render a blueprint-asset picker instead of a plain text box.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ScriptAssetPath(pub String);

impl ScriptAssetPath {
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Display for ScriptAssetPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for ScriptAssetPath {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for ScriptAssetPath {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

// ── Reflection registration ───────────────────────────────────────────────────

fn serialize_script_asset_path_json(
    value: &ScriptAssetPath,
) -> pulsar_reflection::ReflectResult<serde_json::Value> {
    Ok(serde_json::json!(value.0))
}

fn deserialize_script_asset_path_json(
    value: serde_json::Value,
) -> pulsar_reflection::ReflectResult<ScriptAssetPath> {
    value
        .as_str()
        .map(|s| ScriptAssetPath(s.to_string()))
        .ok_or_else(|| ReflectError::TypeMismatch {
            expected: "ScriptAssetPath",
            found: format!("{:?}", value),
        })
}

/// Register `ScriptAssetPath` with the reflection system.
///
/// `structure = String` makes `type_info.is_string()` return `true`, which
/// lets the property inspector detect this type and render a blueprint-asset
/// browser UI (analogous to the mesh-asset browser for `MeshAssetPath`).
fn render_script_asset_editor(
    args: &pulsar_reflection::PropertyEditorArgs<'_>,
    cx: &gpui::App,
) -> gpui::AnyElement {
    use gpui::{prelude::*, *};
    use plugin_editor_api::OpenAsset;
    use ui::button::{Button, ButtonVariants as _};
    use ui::{ActiveTheme, Disableable as _, Icon, IconName, Sizable, h_flex};

    let path_str = args.current_json.as_str().unwrap_or("").to_string();

    let file_name = if path_str.is_empty() {
        "None".to_string()
    } else {
        std::path::Path::new(&path_str)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&path_str)
            .to_string()
    };

    let open_path = std::path::PathBuf::from(&path_str);
    let has_asset = !path_str.is_empty();

    let id = format!(
        "script-asset-{}-{}-{}",
        args.id_prefix, args.class_name, args.prop_name
    );

    h_flex()
        .w_full()
        .justify_between()
        .items_center()
        .gap_2()
        .py_1()
        .child(
            // Field label
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(args.display_name.to_string()),
        )
        .child(
            // [Code icon] [filename] — clickable when an asset is assigned
            Button::new(id)
                .icon(Icon::new(IconName::Code).size(gpui::px(12.)))
                .label(file_name)
                .ghost()
                .small()
                .when(!has_asset, |b| b.disabled(true))
                .when(has_asset, move |b| {
                    b.on_click(move |_event, window, cx| {
                        window.dispatch_action(
                            Box::new(OpenAsset {
                                path: open_path.clone(),
                            }),
                            cx,
                        );
                    })
                }),
        )
        .into_any_element()
}

#[pulsar_reflection::pulsar_type(
    primitive,
    structure = String,
    serialize_json_with = serialize_script_asset_path_json,
    deserialize_json_with = deserialize_script_asset_path_json,
    editor = render_script_asset_editor
)]
type RegisteredScriptAssetPath = ScriptAssetPath;

// ── ScriptComponent ───────────────────────────────────────────────────────────

/// Attaches a blueprint script to a scene object.
///
/// Stores an immutable path to the backing blueprint directory (the one
/// containing `graph_save.json`).  Each sync pass `sync_component` registers
/// this object in the global [`SCRIPT_REGISTRY`], which the blueprint runtime
/// reads to dispatch `BeginPlay`, `Tick`, `EndPlay`, and any other events to
/// the correct bytecode instance.
///
/// The engine treats the presence of this component as the authoritative
/// signal that the scene object participates in the blueprint event loop —
/// no other flag or prop is required.
#[engine_class(category = "Scripting", default, clone, debug, serialize, deserialize)]
pub struct ScriptComponent {
    /// Path to the blueprint directory (`graph_save.json` must exist here).
    ///
    /// Typed as [`ScriptAssetPath`] so the property inspector renders a
    /// blueprint-asset browser instead of a plain text input.
    #[property]
    pub script_asset: ScriptAssetPath,
}

#[register_scene_props_applier]
impl ScenePropsProjector for ScriptComponent {
    const CLASS_NAME: &'static str = "ScriptComponent";

    fn apply_scene_props(props: &mut HashMap<String, Value>, component_data: Option<&Value>) {
        props.remove("script_asset");

        let Some(data) = component_data else { return };

        if let Some(path) = data
            .as_object()
            .and_then(|obj| obj.get("script_asset"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
        {
            props.insert("script_asset".to_string(), Value::from(path));
        }
    }
}

#[register_runtime_behavior]
impl ComponentRuntimeBehavior for ScriptComponent {
    const CLASS_NAME: &'static str = "ScriptComponent";

    fn sync_component(
        owner: &RuntimeComponentOwner,
        component_index: usize,
        component_data: &Value,
        context: &mut dyn ComponentRuntimeContext,
    ) {
        let script_path = component_data
            .as_object()
            .and_then(|obj| obj.get("script_asset"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or_default()
            .to_string();

        if script_path.is_empty() {
            context.report_error(format!(
                "ScriptComponent on '{}' has no script_asset",
                owner.scene_object_id
            ));
            return;
        }

        let actor_key = format!("{}::script::{}", owner.scene_object_id, component_index);

        // Register with the global script registry.  The blueprint runtime reads
        // this each frame to know which scene objects have live scripts.
        let registry = script_registry();
        registry.write().register(ScriptRegistration {
            actor_key: actor_key.clone(),
            scene_object_id: owner.scene_object_id.to_string(),
            script_path,
        });

        // Mark live so the context's stale-cleanup pass keeps this key.
        get_subsystem!(context, LiveKeySet).insert(actor_key);
    }
}
