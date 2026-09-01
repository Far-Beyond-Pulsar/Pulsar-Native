//! The canonical Pulsar scene/level file schema (Pulsar-Native#557, Phase B6).
//!
//! This crate owns **one** serde implementation of the `.level`/scene JSON
//! wire format. Everything else aliases or re-exports it:
//!
//! | Consumer | Spelling |
//! |---|---|
//! | game runtime | `pulsar_scene::format::{SceneFile, SceneObject, …}` |
//! | editor | `ui_level_editor::…::{LevelFile, SceneObjectData, …}` |
//! | shared store | `engine_backend::scene::{ObjectType, LightType, MeshType, ComponentInstance}` |
//!
//! Before B6 the editor and the runtime each carried their own `#[derive(
//! Serialize, Deserialize)]` of this shape, and they had already drifted
//! (missing enum variants on one side, a v1 fallback only the other side
//! modelled). One definition means that drift can no longer happen.
//!
//! # Placement
//!
//! The canonical types must not force editor crates to depend on the
//! game-runtime crate (`pulsar_scene`, which pulls Helio) or vice versa, so
//! they live here rather than in either. This crate's whole dependency set
//! is serde + `engine_fs`'s virtual filesystem.
//!
//! # Format versions
//!
//! ## v2.x (editor output)
//! - `version` is a string (e.g. `"2.1"`)
//! - `transform` is a nested object: `{ "position": [...], "rotation": [...], "scale": [...] }`
//! - Light `color`, `intensity`, `range` live directly in `props`
//! - A `__component_instances` array in `props` may duplicate some data
//!
//! ## v1 (flat)
//! - `version` is an integer (`1`)
//! - `position`, `rotation`, `scale` are top-level fields on each object
//!
//! Both parse through [`SceneObject`]'s deserializer, which folds the v1 flat
//! fields into the nested [`SceneTransform`] so every consumer sees one
//! normalized in-memory shape (see [`SceneObject::world_position`]).
//!
//! # Leniency contract
//!
//! Deserialization is the **union** of what the two pre-B6 implementations
//! accepted, never the intersection:
//!
//! - unrecognised `object_type` / `MeshType` / `LightType` spellings degrade
//!   to `Empty` / `Cube` / `Point` instead of failing the parse, so a file
//!   written by a newer editor still opens;
//! - the editor-authored sections (`components`, `metadata`, `editor`,
//!   `children`, `scene_path`) are parsed leniently — a malformed section is
//!   dropped, never fatal;
//! - every field except `id`, `name` and `object_type` has a default.
//!
//! Serialization matches the **editor's** output exactly (it is the only
//! writer of level files in practice), field for field and attribute for
//! attribute.

use engine_fs::virtual_fs;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

// ── Ids ────────────────────────────────────────────────────────────────────

/// Stable, editor-facing object identifier. Plain `String`; aliased as
/// `EditorObjectId`/`ObjectId` by `engine_backend`.
pub type ObjectId = String;

// ── Top-level file ─────────────────────────────────────────────────────────

/// An entire scene/level read from a scene file.
///
/// Aliased as `pulsar_scene::SceneFile` (game runtime) and
/// `ui_level_editor::…::LevelFile` (editor).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SceneFile {
    /// Format version — accepts both strings (`"2.1"`) and integers (`1`).
    /// Read it through [`Self::version_string`] rather than matching on the
    /// `Value` directly.
    #[serde(default = "default_version_value")]
    pub version: Value,

    /// All objects in depth-first order (parents before children).
    #[serde(default)]
    pub objects: Vec<SceneObject>,

    /// Reflection component instances keyed by object id.
    ///
    /// Parsed leniently (see [`deserialize_components`]): entries without a
    /// `class_name` are skipped rather than failing the whole file, which is
    /// the tolerance the runtime loader had before B6.
    #[serde(default, deserialize_with = "deserialize_components")]
    pub components: HashMap<ObjectId, Vec<ComponentInstance>>,

    // ── Blueprint class bindings (#650) ───────────────────────────────────
    /// Per-object Blueprint class bindings keyed by the object's **StableId**
    /// ([`SceneObject::id`]), never its display name — so renaming an object
    /// never orphans a binding. Multiple classes may bind to one object.
    ///
    /// Additive extension: older level files lack the key entirely and
    /// deserialize to an empty map; files without bindings serialize without
    /// it, staying byte-compatible with the pre-#650 format.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub blueprint_bindings: BlueprintBindings,

    /// Authoring metadata. Absent (runtime-written) files get the default.
    #[serde(default, deserialize_with = "deserialize_lenient")]
    pub metadata: LevelMetadata,

    /// Editor-only state persisted with the level (currently the camera).
    #[serde(
        default,
        deserialize_with = "deserialize_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub editor: Option<LevelEditorFileState>,
}

fn default_version_value() -> Value {
    Value::Number(1.into())
}

impl Default for SceneFile {
    fn default() -> Self {
        Self {
            version: default_version_value(),
            objects: Vec::new(),
            components: HashMap::new(),
            blueprint_bindings: BlueprintBindings::new(),
            metadata: LevelMetadata::default(),
            editor: None,
        }
    }
}

impl SceneFile {
    /// Load a scene from a JSON file.
    pub fn load(path: &std::path::Path) -> Result<Self, SceneLoadError> {
        tracing::debug!(path = %path.display(), "Reading scene file from disk");
        let bytes = virtual_fs::read_file(path).map_err(|e| SceneLoadError::Io(e.to_string()))?;
        let text = String::from_utf8(bytes).map_err(|e| SceneLoadError::Io(e.to_string()))?;
        tracing::debug!(bytes = text.len(), "Scene file read OK, parsing JSON");
        let scene: Self =
            serde_json::from_str(&text).map_err(|e| SceneLoadError::Parse(e.to_string()))?;
        tracing::info!(
            path = %path.display(),
            version = %scene.version,
            objects = scene.objects.len(),
            "Scene file parsed"
        );
        Ok(scene)
    }

    /// Save a scene to a JSON file (pretty-printed).
    pub fn save(&self, path: &std::path::Path) -> Result<(), SceneLoadError> {
        if let Some(parent) = path.parent() {
            virtual_fs::create_dir_all(parent).map_err(|e| SceneLoadError::Io(e.to_string()))?;
        }
        let text =
            serde_json::to_string_pretty(self).map_err(|e| SceneLoadError::Parse(e.to_string()))?;
        virtual_fs::write_file(path, text.as_bytes()).map_err(|e| SceneLoadError::Io(e.to_string()))
    }

    /// The `version` field as a string, whether it was written as a JSON
    /// string (`"2.1"`, editor) or a number (`1`, v1 runtime files).
    ///
    /// Both load paths gate on this the same way: accept `1.x` and `2.x`.
    pub fn version_string(&self) -> String {
        match &self.version {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            other => other.to_string(),
        }
    }

    /// Whether [`Self::version_string`] names a format this engine reads
    /// (`1`, `1.x` or `2.x`).
    pub fn is_supported_version(&self) -> bool {
        let version = self.version_string();
        version == "1" || version.starts_with("1.") || version.starts_with("2.")
    }
}

// ── Component instances ────────────────────────────────────────────────────

/// One reflection-based component instance attached to a scene object.
///
/// Re-exported as `engine_backend::scene::ComponentInstance`; it lives here
/// because it is part of the level file's wire format (`components`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentInstance {
    /// Class name from the component registry (e.g. `"PhysicsComponent"`).
    pub class_name: String,

    /// Whether the component is active.
    ///
    /// Disabled components remain serialized but are ignored by scene-property
    /// projection and behave as if they were absent.
    #[serde(default = "default_component_enabled")]
    pub enabled: bool,

    /// Serialized component data, reconstructed via the registry on load.
    pub data: Value,
}

fn default_component_enabled() -> bool {
    true
}

/// Lenient parse of the `components` section:
/// `{ "<object_id>": [ { "class_name": …, "data": …, "enabled": … } ] }`.
///
/// Entries missing a class name are skipped, non-array values are dropped and
/// a missing `enabled` means enabled — preserving the tolerance the runtime
/// level loader implemented by hand before B6, now that the field is typed
/// rather than a raw `Value`.
fn deserialize_components<'de, D>(
    de: D,
) -> Result<HashMap<ObjectId, Vec<ComponentInstance>>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(de)?;
    let mut out = HashMap::new();
    let Some(map) = value.as_object() else {
        return Ok(out);
    };
    for (object_id, entries) in map {
        let Some(array) = entries.as_array() else {
            continue;
        };
        let records = array
            .iter()
            .filter_map(|entry| {
                Some(ComponentInstance {
                    class_name: entry.get("class_name")?.as_str()?.to_string(),
                    enabled: entry.get("enabled").and_then(Value::as_bool).unwrap_or(true),
                    data: entry.get("data").cloned().unwrap_or(Value::Null),
                })
            })
            .collect();
        out.insert(object_id.clone(), records);
    }
    Ok(out)
}

/// Parse `T` through `serde_json::Value`, falling back to `T::default()`
/// instead of failing the whole file.
///
/// Used for the editor-authored sections the runtime treated as opaque
/// `Value` before B6 (`metadata`, `editor`): typing them must not make a
/// previously-loadable file suddenly fail to parse.
fn deserialize_lenient<'de, D, T>(de: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    let value = Value::deserialize(de)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

// ── Authoring metadata / editor state ──────────────────────────────────────

/// When and by what the level was authored. Written by the editor; runtime
/// writers leave it at its default.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LevelMetadata {
    #[serde(default)]
    pub created: String,
    #[serde(default)]
    pub modified: String,
    #[serde(default)]
    pub editor_version: String,
}

/// The `editor` section: editor-only state that rides along with the level.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LevelEditorFileState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<LevelEditorCameraState>,
}

/// Saved editor camera — position plus yaw/pitch in radians, the same
/// convention `FreeCam::place` uses, so play mode can start where the editor
/// view was.
///
/// Every field defaults: a truncated `"camera": {}` yields the origin rather
/// than failing the level load.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LevelEditorCameraState {
    #[serde(default)]
    pub position: [f32; 3],
    #[serde(default)]
    pub yaw: f32,
    #[serde(default)]
    pub pitch: f32,
}

// ── Blueprint class bindings (#650) ───────────────────────────────────────────

/// One compiled-Blueprint class bound to one scene object (#650).
///
/// At play-mode load each binding becomes exactly one dispatcher instance
/// whose component ops address the bound object's entity. The class must
/// exist as compiled bytecode at
/// `<project>/src/classes/<class_name>/events/.build/bytecode.json` — the
/// same layout the generated `engine_main` auto-discovers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlueprintBinding {
    /// Compiled Blueprint class name — matches the bytecode's
    /// `source_class` and the class directory under `<project>/src/classes/`.
    pub class_name: String,

    /// Per-instance variable overrides: variable name → JSON value in the
    /// variable's natural JSON form (`7.5`, `"hello"`, `[1.0, 2.0]`, …).
    /// Applied to this instance's state arena at spawn; variables not listed
    /// keep their graph-authored defaults. Unknown names are ignored by the
    /// dispatcher (the variable layout is the authority).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub overrides: HashMap<String, Value>,
}

/// A whole file's bindings: object StableId → that object's class bindings.
///
/// A `BTreeMap` so load-time spawning order is deterministic regardless of
/// the JSON key order on disk.
pub type BlueprintBindings = BTreeMap<ObjectId, Vec<BlueprintBinding>>;

// ── Transform ─────────────────────────────────────────────────────────────────

/// World-space transform stored as a nested object (editor v2.x format).
///
/// Aliased as the editor's `Transform`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneTransform {
    #[serde(default)]
    pub position: [f32; 3],

    /// Euler rotation in degrees, YXZ order (pitch, yaw, roll as stored by editor).
    #[serde(default)]
    pub rotation: [f32; 3],

    #[serde(default = "default_scale")]
    pub scale: [f32; 3],
}

impl Default for SceneTransform {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0],
            scale: default_scale(),
        }
    }
}

fn default_scale() -> [f32; 3] {
    [1.0, 1.0, 1.0]
}

// ── Per-object ─────────────────────────────────────────────────────────────────

/// A single object entry in a scene file.
///
/// Aliased as `pulsar_scene::SceneObject` (game runtime) and
/// `ui_level_editor::SceneObjectData` (editor).
///
/// The v1 flat `position`/`rotation`/`scale` fields are **not** struct fields:
/// they are folded into [`Self::transform`] on the deserialize path (see
/// [`SceneObjectRepr`]), so in-memory state is always the normalized v2 shape
/// and a v1 file that is loaded and re-saved keeps its transform instead of
/// dropping it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(from = "SceneObjectRepr")]
pub struct SceneObject {
    /// Stable string identifier (unique within the scene).
    pub id: ObjectId,

    /// Human-readable name shown in the editor hierarchy.
    pub name: String,

    /// What kind of thing this is.
    pub object_type: ObjectType,

    /// World-space transform (v1 flat fields already folded in).
    pub transform: SceneTransform,

    /// Whether this object is rendered.
    pub visible: bool,

    /// Whether the editor allows selecting/moving it.
    pub locked: bool,

    /// Parent object `id`, or `None` for root-level objects.
    pub parent: Option<ObjectId>,

    /// Direct children (populated by the editor's `SceneDatabase` on read,
    /// ignored on write — the `parent` links are the authority).
    pub children: Vec<ObjectId>,

    /// Name-joined path from the root (`"Parent/Child"`), recomputed on read.
    pub scene_path: String,

    /// Type-specific properties (material, light, etc.).
    ///
    /// ⚠ This field does NOT contain `__component_instances`. Component data
    /// is carried in [`Self::component_instances`] / the file's `components`
    /// section; the key is only read as a fallback for older files.
    #[serde(default)]
    pub props: HashMap<String, Value>,

    /// Reflection-based component instances (v2.2+).
    /// Falls back to `props["__component_instances"]` for older scene files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_instances: Option<Value>,
}

fn default_true() -> bool {
    true
}

impl Default for SceneObject {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            object_type: ObjectType::Empty,
            transform: SceneTransform::default(),
            visible: true,
            locked: false,
            parent: None,
            children: Vec::new(),
            scene_path: String::new(),
            props: HashMap::new(),
            component_instances: None,
        }
    }
}

impl SceneObject {
    /// World-space position. v1 flat fields were folded into `transform` at
    /// parse time, so this is simply the nested value.
    pub fn world_position(&self) -> [f32; 3] {
        self.transform.position
    }

    /// World-space Euler rotation in degrees (YXZ).
    pub fn world_rotation(&self) -> [f32; 3] {
        self.transform.rotation
    }

    /// World-space scale.
    pub fn world_scale(&self) -> [f32; 3] {
        self.transform.scale
    }
}

/// Deserialize-side shape of [`SceneObject`], carrying the v1 flat transform
/// fields alongside the v2 nested one.
///
/// The fold rule reproduces exactly what the runtime's `world_position()` /
/// `world_rotation()` / `world_scale()` did before B6: a nested component
/// that differs from its default wins, otherwise the flat field is used. That
/// keeps files carrying *both* (never written by either tool, but tolerated)
/// resolving identically, and makes a pure v1 file (`transform` absent, so
/// defaulted) fall through to the flat fields wholesale.
#[derive(Deserialize)]
struct SceneObjectRepr {
    id: ObjectId,
    name: String,
    object_type: ObjectType,

    // ── v2.x nested transform (takes priority when present) ───────────────
    #[serde(default)]
    transform: SceneTransform,

    // ── v1 flat fields (fallback when the nested one is at its default) ───
    #[serde(default)]
    position: [f32; 3],
    #[serde(default)]
    rotation: [f32; 3],
    #[serde(default = "default_scale")]
    scale: [f32; 3],

    #[serde(default = "default_true")]
    visible: bool,
    #[serde(default, deserialize_with = "deserialize_lenient")]
    locked: bool,
    #[serde(default)]
    parent: Option<ObjectId>,
    #[serde(default, deserialize_with = "deserialize_lenient")]
    children: Vec<ObjectId>,
    #[serde(default, deserialize_with = "deserialize_lenient")]
    scene_path: String,
    #[serde(default)]
    props: HashMap<String, Value>,
    #[serde(default)]
    component_instances: Option<Value>,
}

impl From<SceneObjectRepr> for SceneObject {
    fn from(repr: SceneObjectRepr) -> Self {
        let nested = repr.transform;
        Self {
            id: repr.id,
            name: repr.name,
            object_type: repr.object_type,
            transform: SceneTransform {
                position: if nested.position != [0.0; 3] {
                    nested.position
                } else {
                    repr.position
                },
                rotation: if nested.rotation != [0.0; 3] {
                    nested.rotation
                } else {
                    repr.rotation
                },
                scale: if nested.scale != default_scale() {
                    nested.scale
                } else {
                    repr.scale
                },
            },
            visible: repr.visible,
            locked: repr.locked,
            parent: repr.parent,
            children: repr.children,
            scene_path: repr.scene_path,
            props: repr.props,
            component_instances: repr.component_instances,
        }
    }
}

// ── Object / mesh / light types ───────────────────────────────────────────────

/// Broad category of a scene object.
///
/// This is the **superset** of what the editor and the runtime each modelled
/// before B6: the editor's full coverage (`ParticleSystem`/`AudioSource`/
/// `Blueprint`) is canonical, and the runtime simply gains variants it may
/// not act on yet — forward compatibility, not a regression.
///
/// Re-exported as `engine_backend::scene::ObjectType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ObjectType {
    Empty,
    Folder,
    Camera,
    Light(LightType),
    Mesh(MeshType),
    ParticleSystem,
    AudioSource,
    Blueprint,
}

/// Built-in procedural mesh shapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeshType {
    Cube,
    Sphere,
    Cylinder,
    Plane,
    /// Custom asset; path provided in `props["asset_path"]`.
    Custom,
}

/// Light kinds recognised by Helio.
///
/// Includes `Area` (editor coverage) — the runtime ignores it for now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LightType {
    Directional,
    Point,
    Spot,
    Area,
}

/// Lenient [`ObjectType`] deserialization: accepts unit strings (`"Empty"`,
/// `"ParticleSystem"`, …) and tagged objects (`{ "Mesh": "Cube" }`,
/// `{ "Light": "Point" }`, …), and degrades anything unrecognised to `Empty`
/// rather than failing the parse, so a file written by a newer editor still
/// opens in an older runtime.
impl<'de> Deserialize<'de> for ObjectType {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let v: Value = Value::deserialize(de)?;
        Ok(object_type_from_value(&v))
    }
}

fn object_type_from_value(v: &Value) -> ObjectType {
    match v {
        Value::String(s) => match s.as_str() {
            "Empty" => ObjectType::Empty,
            "Folder" => ObjectType::Folder,
            "Camera" => ObjectType::Camera,
            "ParticleSystem" => ObjectType::ParticleSystem,
            "AudioSource" => ObjectType::AudioSource,
            "Blueprint" => ObjectType::Blueprint,
            other => {
                tracing::debug!(
                    type_ = other,
                    "Unknown ObjectType string — treating as Empty"
                );
                ObjectType::Empty
            }
        },
        Value::Object(map) => {
            if let Some(mesh_val) = map.get("Mesh") {
                ObjectType::Mesh(mesh_type_from_value(mesh_val))
            } else if let Some(light_val) = map.get("Light") {
                ObjectType::Light(light_type_from_value(light_val))
            } else {
                tracing::debug!(map = ?map, "Unknown tagged ObjectType map — treating as Empty");
                ObjectType::Empty
            }
        }
        other => {
            tracing::debug!(value = ?other, "Unexpected ObjectType JSON value — treating as Empty");
            ObjectType::Empty
        }
    }
}

fn mesh_type_from_value(v: &Value) -> MeshType {
    match v.as_str().unwrap_or("") {
        "Cube" => MeshType::Cube,
        "Sphere" => MeshType::Sphere,
        "Cylinder" => MeshType::Cylinder,
        "Plane" => MeshType::Plane,
        "Custom" => MeshType::Custom,
        other => {
            tracing::debug!(type_ = other, "Unknown MeshType — treating as Cube");
            MeshType::Cube
        }
    }
}

fn light_type_from_value(v: &Value) -> LightType {
    match v.as_str().unwrap_or("") {
        "Directional" => LightType::Directional,
        "Point" => LightType::Point,
        "Spot" => LightType::Spot,
        "Area" => LightType::Area,
        other => {
            tracing::debug!(type_ = other, "Unknown LightType — treating as Point");
            LightType::Point
        }
    }
}

// ── Error type ─────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum SceneLoadError {
    Io(String),
    Parse(String),
}

impl std::fmt::Display for SceneLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Parse(e) => write!(f, "Parse error: {e}"),
        }
    }
}

impl std::error::Error for SceneLoadError {}

#[cfg(test)]
mod tests;
