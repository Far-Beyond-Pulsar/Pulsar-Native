//! Static mesh component for mesh asset assignment.

use engine_class_derive::{engine_class, register_runtime_behavior, register_scene_props_applier};
use glam::{EulerRot, Mat4, Quat, Vec3};
use helio::{GpuMaterial, GroupMask, Movability, ObjectDescriptor, Renderer, SceneActor};
use pulsar_reflection::{
    ComponentRuntimeBehavior, ComponentRuntimeContext, LiveKeySet, ReflectError,
    RuntimeComponentOwner, ScenePropsProjector, get_subsystem, scene_id_to_tag,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;

use crate::subsystems::{MeshCache, SceneObjectCache, load_mesh_upload, resolve_asset_path};
// Mat4/Quat/Vec3 used to build the transform passed to sync_mesh_object.

// ── MeshAssetPath ─────────────────────────────────────────────────────────────

/// Strongly-typed wrapper for mesh asset paths.
///
/// Using this as a field type causes the reflection property inspector to render
/// a mesh-asset search browser (via `MeshAssetPicker`) instead of a plain text box.
///
/// Serialises transparently as a JSON string so existing scene files require no
/// migration.
///
/// # Example
///
/// ```ignore
/// #[property]
/// pub mesh_asset: MeshAssetPath,
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MeshAssetPath(pub String);

impl MeshAssetPath {
    /// Create a new `MeshAssetPath` from any string-like value.
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    /// Borrow the inner path string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns `true` if the path is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Display for MeshAssetPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for MeshAssetPath {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for MeshAssetPath {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

// ── Reflection registration ───────────────────────────────────────────────────

fn serialize_mesh_asset_path_json(
    value: &MeshAssetPath,
) -> pulsar_reflection::ReflectResult<serde_json::Value> {
    Ok(serde_json::json!(value.0))
}

fn deserialize_mesh_asset_path_json(
    value: serde_json::Value,
) -> pulsar_reflection::ReflectResult<MeshAssetPath> {
    value
        .as_str()
        .map(|s| MeshAssetPath(s.to_string()))
        .ok_or_else(|| ReflectError::TypeMismatch {
            expected: "MeshAssetPath",
            found: format!("{:?}", value),
        })
}

// ── MeshAssetPath property editor ─────────────────────────────────────────────

/// Engine primitives that are always offered, even in an empty project.
const BUILTIN_MESHES: &[&str] = &[
    "meshes/primitives/SM_Cube.fbx",
    "meshes/primitives/SM_Sphere.fbx",
    "meshes/primitives/SM_Cylinder.fbx",
    "meshes/primitives/SM_Plane.fbx",
    "meshes/primitives/SM_Torus.fbx",
];

/// Property editor for [`MeshAssetPath`] — a searchable mesh-asset browser.
///
/// Owns its [`MeshAssetPicker`](ui_common::asset_picker::MeshAssetPicker) child
/// entity and the subscription that turns a pick into a write-back.
pub struct MeshAssetEditor {
    label: String,
    id_prefix: String,
    prop_name: String,
    picker: gpui::Entity<ui_common::asset_picker::MeshAssetPicker>,
    path: String,
    write_back: pulsar_reflection::PropertyWriteBack,
    _subs: Vec<gpui::Subscription>,
}

impl MeshAssetEditor {
    fn new(
        args: &pulsar_reflection::PropertyEditorArgs<'_>,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        use gpui::AppContext as _;
        use ui_common::asset_picker::{AssetPickedEvent, AssetQuery, MeshAssetPicker};

        let path = args
            .current_value
            .downcast_ref::<MeshAssetPath>()
            .map(|p| p.0.clone())
            .unwrap_or_default();

        let project_root = engine_state::get_project_path().map(std::path::PathBuf::from);
        let queries = vec![
            AssetQuery::extension("mesh"),
            AssetQuery::extension("fbx"),
            AssetQuery::extension("gltf"),
            AssetQuery::extension("glb"),
            AssetQuery::extension("obj"),
        ];

        let picker = cx.new(|cx| {
            MeshAssetPicker::new(
                path.clone(),
                BUILTIN_MESHES.iter().map(|s| s.to_string()).collect(),
                project_root,
                queries,
                window,
                cx,
            )
        });

        let subs = vec![cx.subscribe_in(
            &picker,
            window,
            |this: &mut Self, picker, _event: &AssetPickedEvent, window, cx| {
                let selected = picker.read(cx).selected_path().to_string();
                if this.path == selected {
                    return;
                }
                this.path = selected.clone();
                (this.write_back)(Box::new(MeshAssetPath(selected)), window, cx);
                cx.notify();
            },
        )];

        Self {
            label: args.display_name.to_string(),
            id_prefix: args.id_prefix.to_string(),
            prop_name: args.prop_name.to_string(),
            picker,
            path,
            write_back: args.write_back.clone(),
            _subs: subs,
        }
    }

    /// Accept a mesh assigned elsewhere — e.g. dropped straight onto the
    /// viewport, which writes `mesh_asset` without going through this row.
    fn set_value(&mut self, path: &MeshAssetPath, cx: &mut gpui::Context<Self>) {
        if self.path == path.0 {
            return;
        }
        self.path = path.0.clone();
        self.picker.update(cx, |picker, _| {
            picker.set_selected_path(path.0.clone());
        });
        cx.notify();
    }
}

impl gpui::Render for MeshAssetEditor {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::prelude::*;
        use ui::button::{Button, ButtonVariants as _};
        use ui::{ActiveTheme, Sizable, h_flex, popover::Popover};

        let display = if self.path.is_empty() {
            "No mesh selected".to_string()
        } else {
            std::path::Path::new(&self.path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&self.path)
                .to_string()
        };

        let picker = self.picker.clone();

        h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .gap_2()
            .py_1()
            .child(
                gpui::div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.label.clone()),
            )
            .child(
                Popover::<ui_common::asset_picker::MeshAssetPicker>::new(format!(
                    "mesh-asset-picker-{}-{}",
                    self.id_prefix, self.prop_name
                ))
                .anchor(gpui::Corner::BottomRight)
                .trigger(
                    Button::new(format!(
                        "mesh-asset-picker-btn-{}-{}",
                        self.id_prefix, self.prop_name
                    ))
                    .label(display)
                    .small()
                    .ghost()
                    .dropdown_caret(true),
                )
                .content(move |_window, _cx| picker.clone()),
            )
    }
}

fn mesh_asset_editor(
    args: &pulsar_reflection::PropertyEditorArgs<'_>,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> pulsar_reflection::BoundPropertyEditor {
    use gpui::AppContext as _;

    let entity = cx.new(|cx| MeshAssetEditor::new(args, window, cx));
    pulsar_reflection::BoundPropertyEditor::new(
        entity,
        |editor: &mut MeshAssetEditor, value: &MeshAssetPath, _window, cx| {
            editor.set_value(value, cx)
        },
    )
}

/// Register `MeshAssetPath` with the reflection system.
///
/// `structure = String` makes `type_info.is_string()` return `true`, so the
/// type round-trips through the JSON codec as a plain string; the mesh-browser
/// UI comes from the `editor` registration above.
#[pulsar_reflection::pulsar_type(
    primitive,
    structure = String,
    serialize_json_with = serialize_mesh_asset_path_json,
    deserialize_json_with = deserialize_mesh_asset_path_json,
    editor = mesh_asset_editor
)]
#[allow(dead_code)]
type RegisteredMeshAssetPath = MeshAssetPath;

/// Tracks which (scene_object_id, mesh_asset) pairs have already been reported
/// as errors, so we only log once per mesh-assignment cycle rather than every frame.
static MESH_ERROR_LOG: std::sync::LazyLock<Mutex<HashMap<String, String>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Returns `true` if an error was ALREADY logged for this exact (scene_id, mesh_asset) pair.
fn already_reported(scene_id: &str, mesh_asset: &str) -> bool {
    let Ok(mut map) = MESH_ERROR_LOG.lock() else {
        return false;
    };
    match map.get(scene_id) {
        Some(prev) if prev == mesh_asset => true,
        _ => {
            map.insert(scene_id.to_string(), mesh_asset.to_string());
            false
        }
    }
}

// ── StaticMeshComponent ───────────────────────────────────────────────────────

/// Attaches a mesh asset to a scene object.
#[engine_class(category = "Rendering", default, clone, debug, serialize, deserialize)]
pub struct StaticMeshComponent {
    /// Relative asset path to the mesh file (e.g. "meshes/primitives/SM_Cube.fbx").
    ///
    /// Typed as [`MeshAssetPath`] so the property inspector renders a mesh-asset
    /// search browser instead of a plain text input.
    #[property]
    pub mesh_asset: MeshAssetPath,
}

#[register_scene_props_applier]
impl ScenePropsProjector for StaticMeshComponent {
    const CLASS_NAME: &'static str = "StaticMeshComponent";

    fn apply_scene_props(props: &mut HashMap<String, Value>, component_data: Option<&Value>) {
        props.remove("mesh_asset");
        let Some(data) = component_data else { return };
        if let Some(path) = data
            .as_object()
            .and_then(|o| o.get("mesh_asset"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
        {
            props.insert("mesh_asset".to_string(), Value::from(path));
        }
    }
}

#[register_runtime_behavior]
impl ComponentRuntimeBehavior for StaticMeshComponent {
    const CLASS_NAME: &'static str = "StaticMeshComponent";

    fn sync_component(
        owner: &RuntimeComponentOwner,
        component_index: usize,
        component_data: &Value,
        context: &mut dyn ComponentRuntimeContext,
    ) {
        let mesh_asset = component_data
            .as_object()
            .and_then(|obj| obj.get("mesh_asset"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or_default()
            .to_string();

        if mesh_asset.is_empty() {
            if !already_reported(owner.scene_object_id, "") {
                context.report_error(format!(
                    "StaticMeshComponent on '{}' has no mesh_asset",
                    owner.scene_object_id
                ));
            }
            return;
        }

        let pr = context.project_root();
        let abs_path = resolve_asset_path(pr, &mesh_asset)
            .to_string_lossy()
            .replace('\\', "/");

        let q = Quat::from_euler(
            EulerRot::YXZ,
            owner.rotation[1].to_radians(),
            owner.rotation[0].to_radians(),
            owner.rotation[2].to_radians(),
        );
        let transform = Mat4::from_scale_rotation_translation(
            Vec3::from_array(owner.scale),
            q,
            Vec3::from_array(owner.position),
        );
        let pos = transform.w_axis.truncate();
        let radius = Vec3::from_array(owner.scale).length() * 0.5;

        let tag = scene_id_to_tag(owner.scene_object_id);

        // Phase 1: check mesh cache
        let cached = {
            let mc = get_subsystem!(context, MeshCache);
            mc.get(&abs_path)
        };

        let (mesh_id, mat_id) = if let Some(ids) = cached {
            ids
        } else {
            // Cache miss — load file and upload.
            let path = std::path::Path::new(&abs_path);
            let upload = match load_mesh_upload(path) {
                Some(u) => u,
                None => {
                    if !already_reported(owner.scene_object_id, &abs_path) {
                        tracing::warn!("[SMC] load_mesh_upload FAILED for {}", abs_path);
                        context.report_error(format!(
                            "StaticMeshComponent on '{}': failed to load '{}'",
                            owner.scene_object_id, abs_path
                        ));
                    }
                    return;
                }
            };
            let renderer = get_subsystem!(context, Renderer);
            let scene = renderer.scene_mut();
            let mid = match scene.insert_actor(SceneActor::mesh(upload)).as_mesh() {
                Some(m) => m,
                None => {
                    if !already_reported(owner.scene_object_id, &abs_path) {
                        tracing::warn!("[SMC] insert_actor returned no mesh id");
                    }
                    return;
                }
            };
            let mat = GpuMaterial {
                base_color: [0.6, 0.6, 0.65, 1.0],
                emissive: [0.0, 0.0, 0.0, 0.0],
                roughness_metallic: [0.7, 0.0, 1.5, 0.5],
                tex_base_color: GpuMaterial::NO_TEXTURE,
                tex_normal: GpuMaterial::NO_TEXTURE,
                tex_roughness: GpuMaterial::NO_TEXTURE,
                tex_emissive: GpuMaterial::NO_TEXTURE,
                tex_occlusion: GpuMaterial::NO_TEXTURE,
                workflow: 0,
                flags: 0,
                material_class: 0,
                class_params: [0.0; 4],
            };
            let matid = renderer.scene_mut().insert_material(mat);
            // Store in cache
            let mc = get_subsystem!(context, MeshCache);
            mc.insert(abs_path.clone(), (mid, matid));
            (mid, matid)
        };

        let scene_id = owner.scene_object_id;

        // Mark this component instance as live so stale-cleanup doesn't
        // remove its scene object cache entry between frames.
        get_subsystem!(context, LiveKeySet).insert(scene_id.to_string());

        // Phase 2: update or insert scene object via object-instance cache.
        // This avoids deleting+re-inserting unchanged objects every frame,
        // which would cascade-free meshes/materials in the helio scene.
        //
        // Each get_subsystem! mutably borrows context, so we must strictly
        // separate cache lookups from scene operations into distinct scopes.
        // Three outcomes when consulting the object-instance cache.
        enum SceneCacheAction {
            UpdateTransform {
                obj_id: helio::ObjectId,
            },
            Replace {
                old_id: helio::ObjectId,
                mesh_id: helio::MeshId,
                mat_id: helio::MaterialId,
                transform: Mat4,
                bounds: [f32; 4],
                tag: u64,
                abs_path: String,
            },
        }

        let mut action: Option<SceneCacheAction> = {
            let oc = get_subsystem!(context, SceneObjectCache);
            oc.get(scene_id).map(|(obj_id, cached_asset)| {
                if cached_asset == abs_path {
                    SceneCacheAction::UpdateTransform { obj_id }
                } else {
                    SceneCacheAction::Replace {
                        old_id: obj_id,
                        mesh_id,
                        mat_id,
                        transform,
                        bounds: [pos.x, pos.y, pos.z, radius.max(0.1)],
                        tag,
                        abs_path: abs_path.clone(),
                    }
                }
            })
        };
        if action.is_none() {
            // New object — we need mesh_id/mat_id from Phase 1, which doesn't
            // borrow context, so no conflict.  But the descriptor also needs
            // transform/bounds etc., so clone them here.
            let desc = ObjectDescriptor {
                mesh: mesh_id,
                material: mat_id,
                transform,
                bounds: [pos.x, pos.y, pos.z, radius.max(0.1)],
                flags: 0,
                groups: GroupMask::NONE,
                movability: Some(Movability::Movable),
                user_tag: tag,
            };
            let ob = get_subsystem!(context, Renderer)
                .scene_mut()
                .insert_actor(SceneActor::object(desc));
            if let Some(id) = ob.as_object() {
                let oc = get_subsystem!(context, SceneObjectCache);
                oc.insert(scene_id.to_string(), id, abs_path.clone());
            }
        } else if let Some(SceneCacheAction::UpdateTransform { obj_id }) = action.take() {
            let _ = get_subsystem!(context, Renderer)
                .scene_mut()
                .update_object_transform(obj_id, transform);
        } else if let Some(SceneCacheAction::Replace {
            old_id,
            mesh_id,
            mat_id,
            transform,
            bounds,
            tag,
            abs_path,
        }) = action.take()
        {
            let scene = get_subsystem!(context, Renderer).scene_mut();
            let _ = scene.remove_object(old_id);
            let ob = scene.insert_actor(SceneActor::object(ObjectDescriptor {
                mesh: mesh_id,
                material: mat_id,
                transform,
                bounds,
                flags: 0,
                groups: GroupMask::NONE,
                movability: Some(Movability::Movable),
                user_tag: tag,
            }));
            // update cache after scene operations (different scope)
            if let Some(id) = ob.as_object() {
                let oc = get_subsystem!(context, SceneObjectCache);
                oc.remove(scene_id);
                oc.insert(scene_id.to_string(), id, abs_path);
            }
        }
    }
}
