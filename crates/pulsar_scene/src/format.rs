//! Scene file format — the canonical on-disk representation of a Pulsar scene.
//!
//! Supports both the v1 flat format and the v2.x nested-transform editor format.
//!
//! # v2.x format (editor output)
//! - `version` is a string (e.g. `"2.1"`)
//! - `transform` is a nested object: `{ "position": [...], "rotation": [...], "scale": [...] }`
//! - Unknown `object_type` values (e.g. `"ParticleSystem"`) are silently treated as `Empty`
//! - Light `color`, `intensity`, `range` live directly in `props`
//! - A `__component_instances` array in `props` may duplicate some data (ignored by loader)
//!
//! # v1 format (runtime-only, flat)
//! - `version` is an integer (`1`)
//! - `position`, `rotation`, `scale` are top-level fields on each object

use std::collections::HashMap;
use serde::{Deserialize, Serialize, Deserializer};
use serde_json::Value;

// ── Top-level file ─────────────────────────────────────────────────────────────

/// An entire scene read from a scene file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SceneFile {
    /// Format version — accepts both strings (`"2.1"`) and integers (`1`).
    #[serde(default = "default_version_value")]
    pub version: Value,

    /// All objects in depth-first order (parents before children).
    #[serde(default)]
    pub objects: Vec<SceneObject>,

    // Editor-only top-level sections — loaded but unused at runtime.
    #[serde(default, skip_serializing)]
    pub components: Value,
    #[serde(default, skip_serializing)]
    pub metadata: Value,
    #[serde(default, skip_serializing)]
    pub editor: Value,
}

fn default_version_value() -> Value { Value::Number(1.into()) }

impl SceneFile {
    /// Load a scene from a JSON file.
    pub fn load(path: &std::path::Path) -> Result<Self, SceneLoadError> {
        tracing::debug!(path = %path.display(), "Reading scene file from disk");
        let text = std::fs::read_to_string(path)
            .map_err(|e| SceneLoadError::Io(e.to_string()))?;
        tracing::debug!(bytes = text.len(), "Scene file read OK, parsing JSON");
        let scene: Self = serde_json::from_str(&text)
            .map_err(|e| SceneLoadError::Parse(e.to_string()))?;
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
            std::fs::create_dir_all(parent)
                .map_err(|e| SceneLoadError::Io(e.to_string()))?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| SceneLoadError::Parse(e.to_string()))?;
        std::fs::write(path, text)
            .map_err(|e| SceneLoadError::Io(e.to_string()))
    }
}

// ── Transform (nested, v2.x format) ───────────────────────────────────────────

/// World-space transform stored as a nested object (editor v2.x format).
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct SceneTransform {
    #[serde(default)]
    pub position: [f32; 3],

    /// Euler rotation in degrees, YXZ order (pitch, yaw, roll as stored by editor).
    #[serde(default)]
    pub rotation: [f32; 3],

    #[serde(default = "default_scale")]
    pub scale: [f32; 3],
}

fn default_scale() -> [f32; 3] { [1.0, 1.0, 1.0] }

// ── Per-object ─────────────────────────────────────────────────────────────────

/// A single object entry in a scene file.
///
/// Supports both v1 (flat `position`/`rotation`/`scale`) and v2.x (nested
/// `transform`) by merging: if `transform` is present its values win.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SceneObject {
    /// Stable string identifier (unique within the scene).
    pub id: String,

    /// Human-readable name shown in the editor hierarchy.
    pub name: String,

    /// What kind of thing this is.
    #[serde(deserialize_with = "deserialize_object_type")]
    pub object_type: ObjectType,

    // ── v2.x nested transform (takes priority when present) ───────────────────
    #[serde(default)]
    pub transform: SceneTransform,

    // ── v1 flat fields (fallback if no nested transform) ─────────────────────
    #[serde(default)]
    pub position: [f32; 3],
    #[serde(default)]
    pub rotation: [f32; 3],
    #[serde(default = "default_scale")]
    pub scale: [f32; 3],

    /// Parent object `id`, or `None` for root-level objects.
    #[serde(default)]
    pub parent: Option<String>,

    /// Whether this object is rendered.
    #[serde(default = "default_true")]
    pub visible: bool,

    /// Type-specific properties (material, light, etc.).
    #[serde(default)]
    pub props: HashMap<String, Value>,

    // Editor-only fields — silently accepted and ignored at runtime.
    #[serde(default, skip_serializing)]
    pub locked: bool,
    #[serde(default, skip_serializing)]
    pub children: Value,
    #[serde(default, skip_serializing)]
    pub scene_path: Option<String>,
}

fn default_true() -> bool { true }

impl Default for SceneObject {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            object_type: ObjectType::Empty,
            transform: SceneTransform::default(),
            position: [0.0; 3],
            rotation: [0.0; 3],
            scale: [1.0, 1.0, 1.0],
            parent: None,
            visible: true,
            props: HashMap::new(),
            locked: false,
            children: Value::Null,
            scene_path: None,
        }
    }
}

/// Resolved world-space position — prefers nested `transform` over flat field.
impl SceneObject {
    pub fn world_position(&self) -> [f32; 3] {
        // Nested transform is the v2.x format; the flat field is v1.
        // If the nested transform has a non-zero position use it, otherwise
        // fall back to the flat field.  (A v1 file never writes `transform`
        // so its zero-value default is safe to skip.)
        if self.transform.position != [0.0; 3] {
            self.transform.position
        } else {
            self.position
        }
    }

    pub fn world_rotation(&self) -> [f32; 3] {
        if self.transform.rotation != [0.0; 3] {
            self.transform.rotation
        } else {
            self.rotation
        }
    }

    pub fn world_scale(&self) -> [f32; 3] {
        // Default scale is [1,1,1] in both paths; prefer the nested one.
        let ns = self.transform.scale;
        if ns != [1.0, 1.0, 1.0] {
            ns
        } else {
            // If both are default, just use the flat field (also [1,1,1]).
            self.scale
        }
    }
}

// ── Object / mesh / light types ───────────────────────────────────────────────

/// Broad category of a scene object.
///
/// Unknown variants (e.g. `"ParticleSystem"`) deserialize as `Unknown` so that
/// the scene still loads even if the runtime doesn't support all editor types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ObjectType {
    Empty,
    Folder,
    Camera,
    Mesh(MeshType),
    Light(LightType),
    /// Any type the runtime doesn't recognise.  Treated as `Empty` by the loader.
    Unknown,
}

/// Custom deserializer for [`ObjectType`] that accepts both unit strings
/// (`"Empty"`, `"ParticleSystem"`, …) and tagged objects
/// (`{ "Mesh": "Cube" }`, `{ "Light": "Point" }`, …).
fn deserialize_object_type<'de, D>(de: D) -> Result<ObjectType, D::Error>
where
    D: Deserializer<'de>,
{
    let v: Value = Value::deserialize(de)?;
    Ok(object_type_from_value(&v))
}

fn object_type_from_value(v: &Value) -> ObjectType {
    match v {
        Value::String(s) => match s.as_str() {
            "Empty"  => ObjectType::Empty,
            "Folder" => ObjectType::Folder,
            "Camera" => ObjectType::Camera,
            other => {
                tracing::debug!(type_ = other, "Unknown ObjectType string — treating as Empty");
                ObjectType::Unknown
            }
        },
        Value::Object(map) => {
            if let Some(mesh_val) = map.get("Mesh") {
                let mt = mesh_type_from_value(mesh_val);
                ObjectType::Mesh(mt)
            } else if let Some(light_val) = map.get("Light") {
                let lt = light_type_from_value(light_val);
                ObjectType::Light(lt)
            } else {
                tracing::debug!(map = ?map, "Unknown tagged ObjectType map — treating as Empty");
                ObjectType::Unknown
            }
        }
        other => {
            tracing::debug!(value = ?other, "Unexpected ObjectType JSON value — treating as Empty");
            ObjectType::Unknown
        }
    }
}

fn mesh_type_from_value(v: &Value) -> MeshType {
    match v.as_str().unwrap_or("") {
        "Cube"     => MeshType::Cube,
        "Sphere"   => MeshType::Sphere,
        "Cylinder" => MeshType::Cylinder,
        "Plane"    => MeshType::Plane,
        "Custom"   => MeshType::Custom,
        other => {
            tracing::debug!(type_ = other, "Unknown MeshType — treating as Cube");
            MeshType::Cube
        }
    }
}

fn light_type_from_value(v: &Value) -> LightType {
    match v.as_str().unwrap_or("") {
        "Directional" => LightType::Directional,
        "Point"       => LightType::Point,
        "Spot"        => LightType::Spot,
        other => {
            tracing::debug!(type_ = other, "Unknown LightType — treating as Point");
            LightType::Point
        }
    }
}

// Manual Deserialize for ObjectType delegates to the custom fn above.
impl<'de> Deserialize<'de> for ObjectType {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let v: Value = Value::deserialize(de)?;
        Ok(object_type_from_value(&v))
    }
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LightType {
    Directional,
    Point,
    Spot,
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
            Self::Io(e)    => write!(f, "I/O error: {e}"),
            Self::Parse(e) => write!(f, "Parse error: {e}"),
        }
    }
}

impl std::error::Error for SceneLoadError {}

// ── Helper prop extractors ────────────────────────────────────────────────────

impl SceneObject {
    fn prop_f32(&self, key: &str, default: f32) -> f32 {
        self.props.get(key)
            .and_then(|v| v.as_f64())
            .map(|v| v as f32)
            .unwrap_or(default)
    }

    fn prop_f32_arr3(&self, key: &str, default: [f32; 3]) -> [f32; 3] {
        self.props.get(key)
            .and_then(|v| v.as_array())
            .and_then(|a| {
                if a.len() >= 3 {
                    Some([
                        a[0].as_f64().unwrap_or(0.0) as f32,
                        a[1].as_f64().unwrap_or(0.0) as f32,
                        a[2].as_f64().unwrap_or(0.0) as f32,
                    ])
                } else {
                    None
                }
            })
            .unwrap_or(default)
    }

    fn prop_f32_arr4(&self, key: &str, default: [f32; 4]) -> [f32; 4] {
        self.props.get(key)
            .and_then(|v| v.as_array())
            .and_then(|a| {
                if a.len() >= 4 {
                    Some([
                        a[0].as_f64().unwrap_or(0.0) as f32,
                        a[1].as_f64().unwrap_or(0.0) as f32,
                        a[2].as_f64().unwrap_or(0.0) as f32,
                        a[3].as_f64().unwrap_or(1.0) as f32,
                    ])
                } else if a.len() == 3 {
                    Some([
                        a[0].as_f64().unwrap_or(0.0) as f32,
                        a[1].as_f64().unwrap_or(0.0) as f32,
                        a[2].as_f64().unwrap_or(0.0) as f32,
                        1.0,
                    ])
                } else {
                    None
                }
            })
            .unwrap_or(default)
    }

    // ── Material accessors ────────────────────────────────────────────────────

    pub fn mat_base_color(&self) -> [f32; 4] {
        self.prop_f32_arr4("base_color", [0.5, 0.5, 0.5, 1.0])
    }
    pub fn mat_roughness(&self)         -> f32 { self.prop_f32("roughness", 0.5) }
    pub fn mat_metallic(&self)          -> f32 { self.prop_f32("metallic", 0.0) }
    pub fn mat_emissive(&self)          -> [f32; 3] {
        self.prop_f32_arr3("emissive", [0.0, 0.0, 0.0])
    }
    pub fn mat_emissive_strength(&self) -> f32 { self.prop_f32("emissive_strength", 0.0) }

    // ── Light accessors ───────────────────────────────────────────────────────
    //
    // In the v2.x editor format light properties live directly in `props` (color,
    // intensity, range).  If they are missing there, fall back to the first
    // `LightComponent` entry inside `__component_instances`.

    pub fn light_color(&self) -> [f32; 3] {
        // Direct prop first (may be 3 or 4 floats; we only want RGB).
        if let Some(v) = self.props.get("color") {
            if let Some(a) = v.as_array() {
                if a.len() >= 3 {
                    return [
                        a[0].as_f64().unwrap_or(1.0) as f32,
                        a[1].as_f64().unwrap_or(1.0) as f32,
                        a[2].as_f64().unwrap_or(1.0) as f32,
                    ];
                }
            }
        }
        // Fall back to __component_instances[LightComponent].data.color
        self.light_component_prop_arr3("color", [1.0, 1.0, 1.0])
    }

    pub fn light_intensity(&self) -> f32 {
        if let Some(v) = self.props.get("intensity").and_then(|v| v.as_f64()) {
            return v as f32;
        }
        self.light_component_prop_f32("intensity", 1.0)
    }

    pub fn light_range(&self) -> f32 {
        if let Some(v) = self.props.get("range").and_then(|v| v.as_f64()) {
            return v as f32;
        }
        self.light_component_prop_f32("range", 10.0)
    }

    pub fn light_inner_angle(&self) -> f32 { self.prop_f32("inner_angle", 30.0) }
    pub fn light_outer_angle(&self) -> f32 { self.prop_f32("outer_angle", 45.0) }

    // ── __component_instances helpers ─────────────────────────────────────────

    fn light_component_data(&self) -> Option<&Value> {
        let instances = self.props.get("__component_instances")?.as_array()?;
        for inst in instances {
            let class = inst.get("class_name")?.as_str()?;
            if class == "LightComponent" {
                return inst.get("data");
            }
        }
        None
    }

    fn light_component_prop_f32(&self, key: &str, default: f32) -> f32 {
        self.light_component_data()
            .and_then(|d| d.get(key))
            .and_then(|v| v.as_f64())
            .map(|v| v as f32)
            .unwrap_or(default)
    }

    fn light_component_prop_arr3(&self, key: &str, default: [f32; 3]) -> [f32; 3] {
        let data = match self.light_component_data() {
            Some(d) => d,
            None => return default,
        };
        let arr = match data.get(key).and_then(|v| v.as_array()) {
            Some(a) => a,
            None => return default,
        };
        if arr.len() >= 3 {
            [
                arr[0].as_f64().unwrap_or(1.0) as f32,
                arr[1].as_f64().unwrap_or(1.0) as f32,
                arr[2].as_f64().unwrap_or(1.0) as f32,
            ]
        } else {
            default
        }
    }
}
