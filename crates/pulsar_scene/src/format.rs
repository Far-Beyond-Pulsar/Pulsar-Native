//! Scene file format — the canonical on-disk representation of a Pulsar scene.
//!
//! This is the shared format used by both the editor (to save) and the runtime
//! (to load).  It is intentionally self-contained: all transform and property
//! data needed to reconstruct the scene in Helio is stored directly in the
//! file.  No live editor state is required.
//!
//! # File location
//!
//! Scenes live under `scenes/` in the project root.  The project settings key
//! `project.project.default_map` controls which scene is loaded on startup
//! (default: `"scenes/default_level.json"`).
//!
//! # Object graph
//!
//! Objects form a tree via `parent: Option<String>`.  The file must be ordered
//! so that parent objects appear before their children (depth-first).
//!
//! # Material properties (`props` keys for `Mesh` objects)
//!
//! | Key | Type | Default | Description |
//! |-----|------|---------|-------------|
//! | `base_color` | `[f32;4]` | `[0.5,0.5,0.5,1.0]` | RGBA albedo |
//! | `roughness` | `f32` | `0.5` | PBR roughness |
//! | `metallic` | `f32` | `0.0` | PBR metallic |
//! | `emissive` | `[f32;3]` | `[0,0,0]` | Emissive tint |
//! | `emissive_strength` | `f32` | `0.0` | Emissive multiplier |
//!
//! # Light properties (`props` keys for `Light` objects)
//!
//! | Key | Type | Default | Description |
//! |-----|------|---------|-------------|
//! | `color` | `[f32;3]` | `[1,1,1]` | RGB colour |
//! | `intensity` | `f32` | `1.0` | Brightness |
//! | `range` | `f32` | `10.0` | Falloff radius (point / spot) |
//! | `inner_angle` | `f32` | `30.0` | Inner cone degrees (spot only) |
//! | `outer_angle` | `f32` | `45.0` | Outer cone degrees (spot only) |

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

// ── Top-level file ─────────────────────────────────────────────────────────────

/// An entire scene read from a `scenes/*.json` file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SceneFile {
    /// Format version.  Always `1` for now; increment when breaking changes land.
    #[serde(default = "default_version")]
    pub version: u32,

    /// All objects in depth-first order (parents before children).
    #[serde(default)]
    pub objects: Vec<SceneObject>,
}

fn default_version() -> u32 { 1 }

impl SceneFile {
    /// Load a scene from a JSON file.
    pub fn load(path: &std::path::Path) -> Result<Self, SceneLoadError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| SceneLoadError::Io(e.to_string()))?;
        serde_json::from_str(&text)
            .map_err(|e| SceneLoadError::Parse(e.to_string()))
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

// ── Per-object ─────────────────────────────────────────────────────────────────

/// A single object entry in a scene file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SceneObject {
    /// Stable string identifier (unique within the scene).
    pub id: String,

    /// Human-readable name shown in the editor hierarchy.
    pub name: String,

    /// What kind of thing this is.
    pub object_type: ObjectType,

    /// World-space position (x, y, z) in metres.
    #[serde(default)]
    pub position: [f32; 3],

    /// Euler rotation in degrees, YXZ order (yaw, pitch, roll).
    #[serde(default)]
    pub rotation: [f32; 3],

    /// Scale (x, y, z), default `[1, 1, 1]`.
    #[serde(default = "default_scale")]
    pub scale: [f32; 3],

    /// Parent object `id`, or `None` for root-level objects.
    #[serde(default)]
    pub parent: Option<String>,

    /// Whether this object is rendered.
    #[serde(default = "default_true")]
    pub visible: bool,

    /// Type-specific properties (material, light, etc.).
    /// See module-level docs for the full key list per type.
    #[serde(default)]
    pub props: HashMap<String, serde_json::Value>,
}

fn default_scale() -> [f32; 3] { [1.0, 1.0, 1.0] }
fn default_true() -> bool { true }

impl Default for SceneObject {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            object_type: ObjectType::Empty,
            position: [0.0; 3],
            rotation: [0.0; 3],
            scale: [1.0, 1.0, 1.0],
            parent: None,
            visible: true,
            props: HashMap::new(),
        }
    }
}

// ── Object / mesh / light types ───────────────────────────────────────────────

/// Broad category of a scene object.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectType {
    /// Transform-only anchor (no render data).
    Empty,
    /// Organisational folder (not rendered).
    Folder,
    /// Camera (defines a view; does not render geometry).
    Camera,
    /// Renderable mesh.
    Mesh(MeshType),
    /// Light source.
    Light(LightType),
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

/// Convenience helpers for reading typed values out of `props`.
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
    pub fn mat_roughness(&self)        -> f32 { self.prop_f32("roughness", 0.5) }
    pub fn mat_metallic(&self)         -> f32 { self.prop_f32("metallic", 0.0) }
    pub fn mat_emissive(&self)         -> [f32; 3] {
        self.prop_f32_arr3("emissive", [0.0, 0.0, 0.0])
    }
    pub fn mat_emissive_strength(&self) -> f32 { self.prop_f32("emissive_strength", 0.0) }

    // ── Light accessors ───────────────────────────────────────────────────────

    pub fn light_color(&self)       -> [f32; 3] { self.prop_f32_arr3("color", [1.0, 1.0, 1.0]) }
    pub fn light_intensity(&self)   -> f32      { self.prop_f32("intensity", 1.0) }
    pub fn light_range(&self)       -> f32      { self.prop_f32("range", 10.0) }
    pub fn light_inner_angle(&self) -> f32      { self.prop_f32("inner_angle", 30.0) }
    pub fn light_outer_angle(&self) -> f32      { self.prop_f32("outer_angle", 45.0) }
}
