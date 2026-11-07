//! Static mesh component for mesh asset assignment.

use engine_class_derive::{engine_class, register_scene_props_applier, register_world_component};
use helio_mesh_api::PackedVertex;
use pulsar_reflection::{ReflectError, ScenePropsProjector};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

use crate::mesh_cache::{MeshAssetUpload, MeshMaterialSlot, MeshSection};
use crate::mesh_files::load_mesh_asset_upload;
use helio_component::asset_component::AssetComponentRegistration;
use helio_component::subsystems::resolve_asset_path;
use pulsar_scenedb::gpu::{BufferKey, GpuMirrorHandle, InternedGpuLease};
use pulsar_scenedb::handle_ledger::HandleId;
use pulsar_scenedb::{AssetId, AssetPayload, AssetView, Prefetch, Residency};
use std::sync::{Mutex, OnceLock};

pulsar_reflection::inventory::submit! {
    AssetComponentRegistration {
        asset_kind: plugin_editor_api::AssetKind::Mesh,
        class_name: "StaticMeshComponent",
        value_for: |path| Box::new(StaticMeshComponent::for_mesh_asset(path)),
    }
}
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

    /// Names geometry generated at runtime (not an asset file) by its
    /// content id, so it interns like an asset's: equal content shares one
    /// GPU copy, and a changed shape is a new id. Never loaded from disk.
    pub fn generated(content_id: u128) -> Self {
        Self(format!("{GENERATED_MESH_PREFIX}{content_id:032x}"))
    }

    /// The content id of a [`Self::generated`] path.
    pub fn generated_content_id(&self) -> Option<u128> {
        let hex = self.0.strip_prefix(GENERATED_MESH_PREFIX)?;
        (hex.len() == 32)
            .then(|| u128::from_str_radix(hex, 16).ok())
            .flatten()
            .filter(|id| *id != 0)
    }
}

/// Prefix of [`MeshAssetPath::generated`] paths.
const GENERATED_MESH_PREFIX: &str = "generated-mesh:";

/// The content identity of the file a path names (Pulsar-Native#632/#659),
/// what [`MeshGeometryKey::for_path`] keys geometry by. Resolution: an empty
/// path is `HandleId::ZERO` (no asset); otherwise resolves the
/// project-relative path and defers to `mesh_cache::content_id_for_path`
/// (native `.mesh` v2-v5: a header read; anything else: a canonical-path +
/// mtime/size memoized hash — see that fn's own doc for why this converges
/// path aliases onto one id and mints a new one on a real edit). No project
/// path available, or the file can't be read at all, ALSO falls back to
/// `HandleId::ZERO`. A [`MeshAssetPath::generated`] path carries its id
/// itself.
impl pulsar_scenedb::handle_ledger::ContentAddressed for MeshAssetPath {
    fn content_id(&self) -> pulsar_scenedb::handle_ledger::HandleId {
        use pulsar_scenedb::handle_ledger::HandleId;

        if let Some(id) = self.generated_content_id() {
            return HandleId(id);
        }
        let path = self.0.trim();
        if path.is_empty() {
            return HandleId::ZERO;
        }
        let Some(project_root) = engine_state::get_project_path() else {
            return HandleId::ZERO;
        };
        let abs_path = helio_component::subsystems::resolve_asset_path(
            std::path::Path::new(&project_root),
            path,
        );
        crate::mesh_cache::content_id_for_path(&abs_path)
            .map(HandleId)
            .unwrap_or(HandleId::ZERO)
    }
}

// ── Mesh assets (SceneDB#84) ──────────────────────────────────────────────────

/// Which loaded geometry a [`StaticMeshComponent`]'s `vertices`/`indices`
/// hold: the content id SceneDB interns their GPU copy under. Zero until the
/// component's mesh asset is bound (an empty, private GPU row), then the id
/// of the content that was loaded, so components showing the same content
/// share one GPU copy and a re-import that changed the file is a new id.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct MeshGeometryKey(pub u128);

impl MeshGeometryKey {
    /// No geometry bound.
    pub const NONE: Self = Self(0);

    /// The key of the geometry `path` names as its file is now (or, for a
    /// [`MeshAssetPath::generated`] path, the id it carries): for geometry
    /// a caller fills in itself rather than through the mesh asset.
    pub fn for_path(path: &MeshAssetPath) -> Self {
        use pulsar_scenedb::handle_ledger::ContentAddressed;
        Self(path.content_id().0)
    }
}

impl pulsar_scenedb::handle_ledger::ContentAddressed for MeshGeometryKey {
    fn content_id(&self) -> pulsar_scenedb::handle_ledger::HandleId {
        pulsar_scenedb::handle_ledger::HandleId(self.0)
    }
}

/// The SceneDB asset kind of mesh files. A mesh asset's source is its
/// file's absolute path.
pub const MESH_ASSET_KIND: &str = "mesh";

/// A loaded mesh asset: what every component showing it is bound to.
pub struct LoadedMesh {
    pub upload: MeshAssetUpload,
    /// Local-space bounding sphere of `upload`'s vertices.
    pub bounds_local: [f32; 4],
    /// The content id the file was read as (0 when it has none).
    pub content_id: u128,
}

/// A view's lease on a streamed mesh's geometry in SceneDB's shared GPU
/// pools. It shares ranges with ordinary StaticMeshComponents and releases
/// them when this view stops selecting the asset.
pub struct LoadedMeshGpuLease {
    pub vertices: InternedGpuLease<PackedVertex>,
    pub indices: InternedGpuLease<u32>,
}

impl LoadedMeshGpuLease {
    pub fn vertex_range(&self) -> pulsar_scenedb::gpu::VarLenHandle {
        self.vertices.range()
    }

    pub fn index_range(&self) -> pulsar_scenedb::gpu::VarLenHandle {
        self.indices.range()
    }
}

impl LoadedMesh {
    fn resident_size_bytes(&self) -> usize {
        let geometry = self
            .upload
            .geometry
            .vertices
            .len()
            .saturating_mul(std::mem::size_of::<PackedVertex>())
            .saturating_add(
                self.upload
                    .geometry
                    .indices
                    .len()
                    .saturating_mul(std::mem::size_of::<u32>()),
            );
        let sections = self
            .upload
            .sections
            .len()
            .saturating_mul(std::mem::size_of::<MeshSection>());
        let material_metadata = self
            .upload
            .material_slots
            .iter()
            .map(|slot| slot.name.len())
            .sum::<usize>();
        geometry
            .saturating_add(sections)
            .saturating_add(std::mem::size_of::<crate::mesh_cache::MeshLocalBounds>())
            .saturating_add(material_metadata)
    }
}

/// Registers the mesh asset kind with SceneDB: its loader, and how a
/// [`StaticMeshComponent`] takes in a loaded mesh. Idempotent; done on the
/// first [`mesh_asset_id`].
pub fn register_mesh_assets() {
    static REGISTER: std::sync::Once = std::sync::Once::new();
    REGISTER.call_once(|| {
        pulsar_scenedb::register_loader(MESH_ASSET_KIND, load_mesh_asset);
        pulsar_scenedb::register_payload_size(MESH_ASSET_KIND, |payload| {
            payload
                .downcast_ref::<LoadedMesh>()
                .map_or(0, LoadedMesh::resident_size_bytes)
        });
        pulsar_scenedb::register_binding::<StaticMeshComponent>(MESH_ASSET_KIND, bind_mesh_asset);
    });
}

/// The shared SceneDB residency entry for a declared mesh asset.
///
/// LOD render views request the selected asset through this cache and retain
/// the returned `Arc` only while that level is active. Adjacent levels can
/// use [`prefetch_mesh_asset`] without becoming strong references in the
/// authored component's asset ledger.
pub fn mesh_asset_residency(asset: AssetId) -> Option<Residency<LoadedMesh>> {
    if asset.is_zero() {
        return None;
    }
    let (kind, source) = asset.source()?;
    if kind != MESH_ASSET_KIND {
        return None;
    }

    const MAX_MESH_RESIDENCIES: usize = 4096;
    static CACHE: OnceLock<Mutex<HashMap<(AssetId, u128), Residency<LoadedMesh>>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().unwrap_or_else(|poison| poison.into_inner());
    // AssetId names a source path and stays stable across reimports. Include
    // the source content id in the residency key so file reloads rebuild the
    // cooked mesh while unchanged content still shares one resident value.
    let content_id =
        crate::mesh_cache::content_id_for_path(std::path::Path::new(source.as_ref())).unwrap_or(0);
    // Reimports replace this asset's previous cache identity. Residency
    // handles already returned to active views remain valid after removal.
    cache.retain(|(cached_asset, cached_content), _| {
        *cached_asset != asset || *cached_content == content_id
    });
    if cache.len() >= MAX_MESH_RESIDENCIES && !cache.contains_key(&(asset, content_id)) {
        if let Some(victim) = cache.keys().next().copied() {
            cache.remove(&victim);
        }
    }
    Some(
        cache
            .entry((asset, content_id))
            .or_insert_with(|| {
                let source = source.to_string();
                Residency::lazy(LoadedMesh::resident_size_bytes, move || {
                    load_mesh_asset_data(&source)
                })
            })
            .clone(),
    )
}

/// Current source content identity used to invalidate a selected GPU lease
/// after the asset is reimported or edited in place.
pub fn mesh_asset_content_version(asset: AssetId) -> Option<u128> {
    let (kind, source) = asset.source()?;
    (kind == MESH_ASSET_KIND).then(|| {
        crate::mesh_cache::content_id_for_path(std::path::Path::new(source.as_ref())).unwrap_or(0)
    })
}

/// Start loading a declared LOD mesh on SceneDB's asset loader workers.
/// The caller keeps the returned prefetch while the view may soon need it.
pub fn prefetch_mesh_asset(asset: AssetId) -> Option<Prefetch<LoadedMesh>> {
    Some(mesh_asset_residency(asset)?.prefetch())
}

/// Make a selected LOD's cooked geometry resident in the same content-addressed
/// GPU pools ordinary mesh components use. The returned lease is view-owned;
/// dropping it returns its references to SceneDB's shared pool allocator.
pub fn acquire_mesh_asset_gpu_lease(
    mirror: &GpuMirrorHandle,
    asset: AssetId,
    mesh: &LoadedMesh,
) -> Result<LoadedMeshGpuLease, String> {
    let identity = HandleId(if mesh.content_id != 0 {
        mesh.content_id
    } else {
        asset.0
    });
    if identity.is_zero() {
        return Err("mesh geometry has no stable content identity".into());
    }
    let store = mirror.store();
    let vertex_pool = store
        .interned_var_len_pool::<PackedVertex>(BufferKey::of("builtin_mesh_vertex"))
        .ok_or_else(|| "SceneDB mesh vertex pool is not registered".to_owned())?;
    let vertices = vertex_pool
        .acquire_shared(mirror.queue(), identity, &mesh.upload.geometry.vertices)
        .map_err(|error| format!("could not allocate LOD vertices: {error}"))?;
    store.sync_var_len_pool(
        BufferKey::of("builtin_mesh_vertex"),
        vertex_pool.underlying(),
    );
    let index_pool = store
        .interned_var_len_pool::<u32>(BufferKey::of("builtin_mesh_index"))
        .ok_or_else(|| "SceneDB mesh index pool is not registered".to_owned())?;
    let indices = index_pool
        .acquire_shared(mirror.queue(), identity, &mesh.upload.geometry.indices)
        .map_err(|error| format!("could not allocate LOD indices: {error}"))?;
    store.sync_var_len_pool(BufferKey::of("builtin_mesh_index"), index_pool.underlying());
    Ok(LoadedMeshGpuLease { vertices, indices })
}

/// The asset id of the mesh `mesh_asset` names, declared so the world can
/// load it. [`AssetId::ZERO`] for no mesh, a generated mesh (never loaded
/// from disk), or when there is no project to resolve the path against.
pub fn mesh_asset_id(mesh_asset: &MeshAssetPath) -> AssetId {
    let path = mesh_asset.as_str().trim();
    if path.is_empty() || mesh_asset.generated_content_id().is_some() {
        return AssetId::ZERO;
    }
    let Some(project_root) = engine_state::get_project_path() else {
        tracing::warn!(
            "StaticMeshComponent: no project path available, cannot resolve mesh_asset '{path}'"
        );
        return AssetId::ZERO;
    };
    register_mesh_assets();
    AssetId::declare(MESH_ASSET_KIND, &mesh_source(&project_root, path))
}

/// The source string (absolute path) the mesh asset at the project-relative
/// `path` is declared under. A file event's path maps to the same string.
pub fn mesh_source(project_root: &str, path: &str) -> String {
    let abs = resolve_asset_path(std::path::Path::new(project_root), path);
    mesh_source_for_file(&abs)
}

/// The source string of the mesh file at `abs`.
pub fn mesh_source_for_file(abs: &std::path::Path) -> String {
    std::fs::canonicalize(abs)
        .unwrap_or_else(|_| abs.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

// A changed mesh file reloads its mesh asset.
inventory::submit! {
    helio_component::asset_component::AssetReloadRegistration {
        origin: pulsar_plugins::origin!(),
        kind: plugin_editor_api::AssetKind::Mesh,
        sources_for_file: |path| vec![mesh_source_for_file(path)],
    }
}

/// The mesh kind's loader: runs on a SceneDB worker thread.
fn load_mesh_asset(source: &str) -> Result<AssetPayload, String> {
    let asset = AssetId::of(MESH_ASSET_KIND, source);
    let residency = mesh_asset_residency(asset)
        .ok_or_else(|| format!("mesh asset source is not declared: '{source}'"))?;
    let loaded = residency.get()?;
    Ok(loaded)
}

fn load_mesh_asset_data(source: &str) -> Result<LoadedMesh, String> {
    let path = std::path::Path::new(source);
    let upload =
        load_mesh_asset_upload(path).ok_or_else(|| format!("could not load mesh '{source}'"))?;
    let content_id = crate::mesh_cache::content_id_for_path(path).unwrap_or(0);
    // The SceneDB asset loader runs this on an asset worker. Build the CPU
    // spatial index before publishing the payload so the first viewport pick
    // never has to build a BVH on the render thread.
    if content_id != 0 {
        crate::mesh_spatial::prewarm_mesh_bvh(
            MeshGeometryKey(content_id),
            &upload.geometry.vertices,
            &upload.geometry.indices,
        );
    }
    tracing::info!(
        "StaticMeshComponent: loaded '{}' ({} vertices, {} indices, {} material sections)",
        source,
        upload.geometry.vertices.len(),
        upload.geometry.indices.len(),
        upload.sections.len()
    );
    Ok(LoadedMesh {
        bounds_local: local_bounding_sphere_from_bounds(upload.bounds),
        content_id,
        upload,
    })
}

/// How a [`StaticMeshComponent`] takes in its loaded (or failed) mesh.
fn bind_mesh_asset(component: &mut StaticMeshComponent, view: &AssetView) {
    if view.id != component.mesh {
        return;
    }
    let Some(mesh) = view.payload::<LoadedMesh>() else {
        tracing::warn!(
            "StaticMeshComponent: mesh '{}' is unavailable ({:?})",
            component.mesh_asset,
            view.state
        );
        clear_mesh_geometry(component);
        return;
    };
    component.bounds_local = mesh.bounds_local;
    component.vertices = mesh.upload.geometry.vertices.clone();
    component.indices = mesh.upload.geometry.indices.clone();
    component.material_slots =
        reconcile_material_slots(&component.material_slots, &mesh.upload.material_slots);
    apply_legacy_material_override(component);
    component.mesh_sections = mesh.upload.sections.clone();
    component.material_slot_metadata = mesh.upload.material_slots.clone();
    component.geometry_key = MeshGeometryKey(if mesh.content_id != 0 {
        mesh.content_id
    } else {
        // No content id from the file: this load of this asset.
        (view.id.0 ^ u128::from(view.version).rotate_left(64)).max(1)
    });
}

/// The "no mesh" geometry: what a component without a loadable mesh shows.
fn clear_mesh_geometry(component: &mut StaticMeshComponent) {
    component.bounds_local = [0.0, 0.0, 0.0, 0.5];
    component.vertices.clear();
    component.indices.clear();
    component.mesh_sections.clear();
    component.material_slot_metadata.clear();
    component.geometry_key = MeshGeometryKey::NONE;
}

/// Points `component` at the mesh asset its `mesh_asset` names. The world
/// loads it off the write path and binds the geometry when it is ready; a
/// component naming no loadable mesh shows none.
fn attach_mesh_asset(component: &mut StaticMeshComponent) {
    let id = mesh_asset_id(&component.mesh_asset);
    if id.is_zero() && component.mesh_asset.generated_content_id().is_none() {
        clear_mesh_geometry(component);
    }
    component.mesh = id;
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

/// One component-level material choice corresponding to a mesh asset slot.
/// `source_material` is a stable matching key for reimport reconciliation;
/// it is not a GPU material index.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StaticMeshMaterialSlot {
    pub source_material: Option<u32>,
    pub name: String,
    #[serde(default)]
    pub material_asset: String,
    #[serde(default)]
    pub imported_surface: crate::mesh_cache::ImportedSurfaceMaterial,
    #[serde(default)]
    pub surface_override: Option<crate::mesh_cache::ImportedSurfaceMaterial>,
    /// The mesh asset's own default material for this slot (authored in the
    /// mesh viewer). Refreshed from the asset whenever it loads and never
    /// saved with the level; an empty `material_asset` inherits it.
    #[serde(skip)]
    pub mesh_default_material: String,
}

impl StaticMeshMaterialSlot {
    /// The material asset this slot draws with: its own assignment, else the
    /// mesh asset's default for the slot (empty: the imported surface).
    pub fn effective_material_asset(&self) -> &str {
        if self.material_asset.trim().is_empty() {
            &self.mesh_default_material
        } else {
            &self.material_asset
        }
    }
}

/// Temporary persistence adapter for levels authored with the deprecated
/// `MaterialOverrideComponent`. Scene loading migrates this into each mesh
/// slot and clears it before the component is saved again.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LegacyMaterialOverrideData {
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub emissive_color: [f32; 3],
    pub emissive_intensity: f32,
    pub alpha: f32,
}

impl LegacyMaterialOverrideData {
    pub fn into_surface(self) -> crate::mesh_cache::ImportedSurfaceMaterial {
        crate::mesh_cache::ImportedSurfaceMaterial {
            base_color: self.base_color,
            roughness: self.roughness,
            metallic: self.metallic,
            emissive: self.emissive_color,
            emissive_intensity: self.emissive_intensity,
            alpha: self.alpha,
        }
    }
}

/// Editor-facing dynamic list of per-slot material assignments. Kept as one
/// reflected value so a custom property editor can render a named picker for
/// every imported slot instead of exposing an unhelpful raw vector editor.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StaticMeshMaterialSlots {
    pub slots: Vec<StaticMeshMaterialSlot>,
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
    serialize_json_with = serialize_mesh_asset_path_json,
    deserialize_json_with = deserialize_mesh_asset_path_json,
    editor = mesh_asset_editor
)]
#[allow(dead_code)]
type RegisteredMeshAssetPath = MeshAssetPath;

fn serialize_material_slots_json(
    value: &StaticMeshMaterialSlots,
) -> pulsar_reflection::ReflectResult<serde_json::Value> {
    serde_json::to_value(value)
        .map_err(|error| ReflectError::SerializationFailed(error.to_string()))
}

fn deserialize_material_slots_json(
    value: serde_json::Value,
) -> pulsar_reflection::ReflectResult<StaticMeshMaterialSlots> {
    serde_json::from_value(value)
        .map_err(|error| ReflectError::DeserializationFailed(error.to_string()))
}

struct MaterialSlotPickerRow {
    key: (Option<u32>, usize),
    name: String,
    path: String,
    /// The mesh asset's default for this slot, shown while `path` is empty.
    inherited: String,
    picker: gpui::Entity<ui_common::asset_picker::MeshAssetPicker>,
}

struct StaticMeshMaterialSlotsEditor {
    label: String,
    id_prefix: String,
    prop_name: String,
    value: StaticMeshMaterialSlots,
    rows: Vec<MaterialSlotPickerRow>,
    write_back: pulsar_reflection::PropertyWriteBack,
    subscriptions: Vec<gpui::Subscription>,
}

impl StaticMeshMaterialSlotsEditor {
    fn new(
        args: &pulsar_reflection::PropertyEditorArgs<'_>,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let mut editor = Self {
            label: args.display_name.to_owned(),
            id_prefix: args.id_prefix.to_owned(),
            prop_name: args.prop_name.to_owned(),
            value: args
                .current_value
                .downcast_ref::<StaticMeshMaterialSlots>()
                .cloned()
                .unwrap_or_default(),
            rows: Vec::new(),
            write_back: args.write_back.clone(),
            subscriptions: Vec::new(),
        };
        let initial = editor.value.clone();
        editor.rebuild_rows(&initial, window, cx);
        editor
    }

    fn rebuild_rows(
        &mut self,
        value: &StaticMeshMaterialSlots,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        use gpui::AppContext as _;
        use ui_common::asset_picker::{AssetPickedEvent, AssetQuery, MeshAssetPicker};

        self.rows.clear();
        self.subscriptions.clear();
        self.value = value.clone();
        let project_root = engine_state::get_project_path().map(std::path::PathBuf::from);
        let queries = vec![
            AssetQuery::extension("mat"),
            AssetQuery::folder_marker(helio_mats::asset::SHADER_GRAPH_FILE),
        ];
        for (index, slot) in value.slots.iter().enumerate() {
            let key = (slot.source_material, index);
            let picker = cx.new(|cx| {
                MeshAssetPicker::new(
                    slot.material_asset.clone(),
                    Vec::new(),
                    project_root.clone(),
                    queries.clone(),
                    window,
                    cx,
                )
            });
            self.subscriptions.push(cx.subscribe_in(
                &picker,
                window,
                move |this: &mut Self, picker, _event: &AssetPickedEvent, window, cx| {
                    let selected = picker.read(cx).selected_path().to_owned();
                    this.set_slot_material(key, selected, window, cx);
                },
            ));
            self.rows.push(MaterialSlotPickerRow {
                key,
                name: if slot.name.is_empty() {
                    format!("Material Slot {}", index + 1)
                } else {
                    slot.name.clone()
                },
                path: slot.material_asset.clone(),
                inherited: slot.mesh_default_material.clone(),
                picker,
            });
        }
        cx.notify();
    }

    fn set_slot_material(
        &mut self,
        key: (Option<u32>, usize),
        path: String,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if let Some(slot) = self
            .value
            .slots
            .get_mut(key.1)
            .filter(|slot| slot.source_material == key.0)
        {
            if slot.material_asset == path {
                return;
            }
            slot.material_asset = path.clone();
            if let Some(row) = self.rows.get_mut(key.1) {
                row.path = path;
            }
            (self.write_back)(Box::new(self.value.clone()), window, cx);
            cx.notify();
        }
    }

    fn create_material_asset(
        &mut self,
        key: (Option<u32>, usize),
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(project_root) = engine_state::get_project_path().map(std::path::PathBuf::from)
        else {
            return;
        };
        let Some(slot) = self
            .value
            .slots
            .get(key.1)
            .filter(|slot| slot.source_material == key.0)
            .cloned()
        else {
            return;
        };
        let safe_name: String = slot
            .name
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
            .collect();
        let directory = project_root.join("materials");
        if let Err(error) = std::fs::create_dir_all(&directory) {
            tracing::warn!(%error, path = %directory.display(), "could not create materials directory");
            return;
        }
        let stem = if safe_name.trim_matches('_').is_empty() {
            format!("Material_{}", key.1 + 1)
        } else {
            safe_name
        };
        let (path, relative) = (0..)
            .map(|suffix| {
                let file_name = if suffix == 0 {
                    format!("{stem}.mat")
                } else {
                    format!("{stem}_{suffix}.mat")
                };
                (directory.join(&file_name), format!("materials/{file_name}"))
            })
            .find(|(path, _)| !path.exists())
            .expect("unbounded unique material filename search");
        let material = helio_mats::asset::SurfaceMaterialDocument::from(slot.imported_surface);
        let Ok(bytes) = serde_json::to_vec_pretty(&material) else {
            return;
        };
        if let Err(error) = std::fs::write(&path, bytes) {
            tracing::warn!(%error, path = %path.display(), "could not write surface material asset");
            return;
        }
        self.set_slot_material(key, relative.clone(), window, cx);
        if let Some(row) = self.rows.get(key.1) {
            row.picker
                .update(cx, |picker, _| picker.set_selected_path(relative));
        }
    }

    fn set_value(
        &mut self,
        value: &StaticMeshMaterialSlots,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        // The inspector pushes the current value every frame. An unchanged
        // value must not notify: a notify here invalidates this view's whole
        // ancestor chain (properties panel, dock, app, root), which is a
        // full UI rebuild per frame.
        if self.value == *value {
            return;
        }
        let same_slots = self.value.slots.len() == value.slots.len()
            && self
                .value
                .slots
                .iter()
                .zip(&value.slots)
                .all(|(a, b)| a.source_material == b.source_material && a.name == b.name);
        if !same_slots {
            self.rebuild_rows(value, window, cx);
            return;
        }
        self.value = value.clone();
        for (row, slot) in self.rows.iter_mut().zip(&value.slots) {
            row.inherited = slot.mesh_default_material.clone();
            if row.path != slot.material_asset {
                row.path = slot.material_asset.clone();
                row.picker.update(cx, |picker, _| {
                    picker.set_selected_path(slot.material_asset.clone())
                });
            }
        }
        cx.notify();
    }
}

impl gpui::Render for StaticMeshMaterialSlotsEditor {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::prelude::*;
        use ui::button::{Button, ButtonVariants as _};
        use ui::{ActiveTheme, Sizable, h_flex, popover::Popover, v_flex};

        let mut rows = v_flex().w_full().gap_1().child(
            gpui::div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(self.label.clone()),
        );
        if self.rows.is_empty() {
            rows = rows.child(
                gpui::div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Select a mesh to load its material slots"),
            );
        }
        for (index, row) in self.rows.iter().enumerate() {
            let picker = row.picker.clone();
            let row_key = row.key;
            let shown = if row.path.is_empty() {
                &row.inherited
            } else {
                &row.path
            };
            let file_name = std::path::Path::new(shown)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(shown)
                .to_owned();
            let display = match (row.path.is_empty(), shown.is_empty()) {
                (true, true) => "Use imported material".to_owned(),
                (true, false) => format!("{file_name} (mesh default)"),
                _ => file_name,
            };
            rows = rows.child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .gap_2()
                    .child(gpui::div().text_xs().child(row.name.clone()))
                    .child(
                        Popover::<ui_common::asset_picker::MeshAssetPicker>::new(format!(
                            "material-slot-picker-{}-{}-{index}",
                            self.id_prefix, self.prop_name
                        ))
                        .anchor(gpui::Corner::BottomRight)
                        .trigger(
                            Button::new(format!(
                                "material-slot-picker-btn-{}-{index}",
                                self.id_prefix
                            ))
                            .label(display)
                            .small()
                            .ghost()
                            .dropdown_caret(true),
                        )
                        .content(move |_window, _cx| picker.clone()),
                    )
                    .child(
                        Button::new(format!(
                            "create-material-{}-{}-{index}",
                            self.id_prefix, self.prop_name
                        ))
                        .label("Create")
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.create_material_asset(row_key, window, cx);
                            },
                        )),
                    ),
            );
        }
        rows
    }
}

fn material_slots_editor(
    args: &pulsar_reflection::PropertyEditorArgs<'_>,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> pulsar_reflection::BoundPropertyEditor {
    use gpui::AppContext as _;
    let entity = cx.new(|cx| StaticMeshMaterialSlotsEditor::new(args, window, cx));
    pulsar_reflection::BoundPropertyEditor::new(
        entity,
        |editor: &mut StaticMeshMaterialSlotsEditor,
         value: &StaticMeshMaterialSlots,
         window,
         cx| { editor.set_value(value, window, cx) },
    )
}

#[pulsar_reflection::pulsar_type(
    serialize_json_with = serialize_material_slots_json,
    deserialize_json_with = deserialize_material_slots_json,
    editor = material_slots_editor
)]
#[allow(dead_code)]
type RegisteredStaticMeshMaterialSlots = StaticMeshMaterialSlots;

// ── StaticMeshComponent ───────────────────────────────────────────────────────

/// Attaches a mesh asset to a scene object.
///
/// `scene_store` (Pulsar-Native#561 Phase D): opts this struct into
/// `#[gpu]`-mirrored fields via `#[engine_class]`'s delegation to
/// `pulsar_scenedb::SceneStore` -- see `vertices`/`indices` below, and
/// `decode_static_mesh_component`'s doc for how they get populated. A
/// `#[gpu] Vec<T>` field routes through SceneDB's variable-length codegen
/// path, which implies no `Copy`/`Pod` requirement on this struct (see
/// `engine_class_derive`'s `struct_has_gpu_vec_field` check) -- unlike
/// every OTHER `scene_store` struct so far, which are all plain fixed-size
/// `Pod` rows.
#[engine_class(
    category = "Rendering",
    default,
    clone,
    debug,
    serialize,
    deserialize,
    scene_store
)]
pub struct StaticMeshComponent {
    /// Relative asset path to the mesh file (e.g. "meshes/primitives/SM_Cube.fbx").
    ///
    /// Typed as [`MeshAssetPath`] so the property inspector renders a mesh-asset
    /// search browser instead of a plain text input.
    #[property]
    pub mesh_asset: MeshAssetPath,

    /// Material assets assigned to the imported mesh's named material slots.
    /// Empty assignments use the mesh's imported/default material.
    #[property]
    #[serde(default)]
    pub material_slots: StaticMeshMaterialSlots,

    /// One-load migration field; old levels store these surface values in a
    /// separate component record. Hydration fans them out over the imported
    /// slots, then clears this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_material_override: Option<LegacyMaterialOverrideData>,

    /// What may change about this mesh at runtime (Pulsar-Native#837). It
    /// sets the movable flag of the mesh's draw rows ([`super::StaticMeshDraw`])
    /// and the object's [`helio_component::components::object_movability`]; levels saved before it
    /// load as Static, which is how their rows were already flagged.
    #[property]
    #[serde(default)]
    pub movability: helio_component::components::ObjectMovability,

    /// The mesh asset `mesh_asset` names, as the world resolves it
    /// (SceneDB#84): set from `mesh_asset` when a value is decoded or
    /// `mesh_asset` is written, never authored or saved. The world loads it
    /// off the write path and binds its geometry into the fields below.
    #[serde(skip)]
    pub mesh: AssetId,

    /// Which loaded geometry `vertices`/`indices` hold, the id their GPU copy
    /// is interned under (see [`MeshGeometryKey`]). Set by the binding.
    #[serde(skip)]
    pub geometry_key: MeshGeometryKey,

    /// The mesh's actual vertex/index data -- not an indirect handle into a
    /// separate asset registry, the payload itself (per the governing rule:
    /// "it doesn't hold an int32 that points to the mesh, it holds the
    /// mesh"). Written by the mesh asset's binding when the world has loaded
    /// `mesh` -- never authored directly. Never round-tripped through JSON:
    /// mesh geometry lives in the asset file `mesh_asset` names, not
    /// duplicated into every saved scene.
    ///
    /// `content_id = "geometry_key"` (Pulsar-Native#632/#659): SceneDB routes
    /// this field's GPU allocation through its content-id-interned pool
    /// instead of one private allocation per entity -- ten components
    /// showing the same loaded mesh store the geometry ONCE, freed
    /// automatically when the last reference despawns/removes.
    /// `..._gpu_handle` accessors resolve to the shared range.
    #[gpu(buffer = "builtin_mesh_vertex", mirror = Once, content_id = "geometry_key")]
    #[serde(skip)]
    pub vertices: Vec<PackedVertex>,
    /// See [`Self::vertices`] -- same rules, the index half of the same
    /// upload, interned under the SAME `geometry_key` content id (a separate
    /// pool from `vertices`' own -- see `gpu::interned_pool`'s module doc
    /// on why sharing an id across two pools needs no coordination between
    /// them).
    #[gpu(buffer = "builtin_mesh_index", mirror = Once, content_id = "geometry_key")]
    #[serde(skip)]
    pub indices: Vec<u32>,

    /// Names and source material indices discovered while loading the mesh.
    /// These are asset metadata, not authored per-instance overrides.
    #[serde(skip)]
    pub material_slot_metadata: Vec<MeshMaterialSlot>,

    /// Index ranges that route imported geometry to material slots.
    #[serde(skip)]
    pub mesh_sections: Vec<MeshSection>,

    /// Conservative local-space bounding sphere (xyz = center, w = radius),
    /// derived from the mesh asset's canonical AABB. Uploaded with the derived draw rows
    /// ([`super::StaticMeshDraw`]); the renderer's scene join transforms it
    /// by the owner's transform for culling. Not derived from
    /// `transform.scale` -- a thin mesh at scale 1.0 and a cube at scale 1.0
    /// have different real extents and must not collapse to the same bound.
    #[serde(skip)]
    pub bounds_local: [f32; 4],
}

#[register_scene_props_applier]
impl ScenePropsProjector for StaticMeshComponent {
    const CLASS_NAME: &'static str = "StaticMeshComponent";

    /// Clears the keys this class's values once occupied in an object's
    /// props (the level-file migration); the values live in the component.
    fn apply_scene_props(props: &mut HashMap<String, Value>, _component_data: Option<&Value>) {
        props.remove("mesh_asset");
    }
}

fn reconcile_material_slots(
    existing: &StaticMeshMaterialSlots,
    asset_slots: &[MeshMaterialSlot],
) -> StaticMeshMaterialSlots {
    let slots = asset_slots
        .iter()
        .enumerate()
        .map(|(index, source)| {
            let previous = existing
                .slots
                .iter()
                .find(|slot| {
                    source.source_material.is_some()
                        && slot.source_material == source.source_material
                })
                .or_else(|| {
                    existing
                        .slots
                        .iter()
                        .find(|slot| slot.name == source.name && !source.name.is_empty())
                });
            StaticMeshMaterialSlot {
                source_material: source.source_material,
                name: if source.name.is_empty() {
                    format!("Material Slot {}", index + 1)
                } else {
                    source.name.clone()
                },
                material_asset: previous
                    .map(|slot| slot.material_asset.clone())
                    .unwrap_or_default(),
                imported_surface: source.surface,
                surface_override: previous.and_then(|slot| slot.surface_override),
                mesh_default_material: source.material_asset.clone(),
            }
        })
        .collect();
    StaticMeshMaterialSlots { slots }
}

fn apply_legacy_material_override(component: &mut StaticMeshComponent) {
    let Some(legacy) = component.legacy_material_override.take() else {
        return;
    };
    let surface = legacy.into_surface();
    for slot in &mut component.material_slots.slots {
        slot.surface_override = Some(surface);
    }
}

impl StaticMeshComponent {
    /// A component showing the mesh asset at the project-relative `path`, as
    /// decoding `{"mesh_asset": path}` would give. Its geometry is bound once
    /// it is in a world and the world has loaded the mesh.
    pub fn for_mesh_asset(path: &str) -> Self {
        let mut component = Self {
            mesh_asset: MeshAssetPath::new(path),
            ..Default::default()
        };
        attach_mesh_asset(&mut component);
        component
    }
}

/// `StaticMeshComponent`'s JSON boundary decoder (Pulsar-Native#561 Phase
/// D). The serialized form references its mesh by `mesh_asset`; decoding
/// points the value at that mesh asset, which the world loads and binds
/// (SceneDB#84). Nothing is read from disk here.
fn decode_static_mesh_component(data: &serde_json::Value) -> Result<StaticMeshComponent, String> {
    let mut component: StaticMeshComponent = pulsar_world_registry::decode_json(data)?;
    attach_mesh_asset(&mut component);
    Ok(component)
}

/// `property_written` hook: a reflected write that changed `mesh_asset`
/// (the properties panel's mesh picker, a script) points the component at
/// the newly named mesh asset; the world loads and binds it. Writes to any
/// other property change nothing here.
fn static_mesh_property_written(component: &mut StaticMeshComponent, property: Option<&str>) {
    if matches!(property, None | Some("mesh_asset")) {
        attach_mesh_asset(component);
    }
}

/// Derive the legacy GPU sphere from the mesh asset's authoritative AABB.
/// PMSH v5 stores this metadata; older formats have the same AABB derived
/// once by `decode_asset`, so hydration never rescans the vertex payload.
fn local_bounding_sphere_from_bounds(bounds: crate::mesh_cache::MeshLocalBounds) -> [f32; 4] {
    if bounds.is_empty || !bounds.is_valid() {
        return [0.0, 0.0, 0.0, 0.5];
    }
    let center = [
        (bounds.min[0] + bounds.max[0]) * 0.5,
        (bounds.min[1] + bounds.max[1]) * 0.5,
        (bounds.min[2] + bounds.max[2]) * 0.5,
    ];
    let extent = [
        bounds.max[0] - center[0],
        bounds.max[1] - center[1],
        bounds.max[2] - center[2],
    ];
    let radius = extent[0].hypot(extent[1]).hypot(extent[2]);
    [center[0], center[1], center[2], radius.max(0.001)]
}

// Phase B4 (Pulsar-Native#555): the first component migrated onto
// pulsar_world_registry's World bridge.
#[register_world_component(
    decode = decode_static_mesh_component,
    property_written = static_mesh_property_written
)]
impl StaticMeshComponent {}
