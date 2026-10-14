//! Per-project scene-sandbox settings (`.pulsar/agent_scene.json`) and how a
//! run's sandbox mode is resolved.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const SCENE_SETTINGS_FILE: &str = "agent_scene.json";

/// Largest scene a `copy` sandbox forks unless the project says otherwise.
pub const DEFAULT_COPY_MAX_OBJECTS: usize = 50_000;

/// How a sandboxed run's scene edits are kept away from the live scene.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxMode {
    /// No sandbox: a sandbox run writes the live scene like an ordinary
    /// `engine-scene-write` run (still journaled, checkpointed and confirmed).
    Off,
    /// Recorded operations over a read-through view of the live scene. No
    /// scene copy, so the cost does not grow with the scene.
    #[default]
    Overlay,
    /// A full private copy of the scene ([`crate::sandbox::Fork`]); fully
    /// isolated, but O(scene) in time and memory.
    Copy,
}

impl SandboxMode {
    pub fn label(self) -> &'static str {
        match self {
            SandboxMode::Off => "off",
            SandboxMode::Overlay => "overlay",
            SandboxMode::Copy => "copy",
        }
    }

    /// `off`, `overlay` or `copy` (case-insensitive).
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_lowercase().as_str() {
            "off" => Some(SandboxMode::Off),
            "overlay" => Some(SandboxMode::Overlay),
            "copy" => Some(SandboxMode::Copy),
            _ => None,
        }
    }

    /// The next mode when cycling a control: off, overlay, copy, off...
    pub fn next(self) -> Self {
        match self {
            SandboxMode::Off => SandboxMode::Overlay,
            SandboxMode::Overlay => SandboxMode::Copy,
            SandboxMode::Copy => SandboxMode::Off,
        }
    }
}

/// The mode a run uses: the harness's own `sandbox:` choice wins over the
/// project setting, which wins over the built-in default. An unrecognised
/// harness value (for example the plain `true`) falls through to the project.
pub fn resolve_mode(harness: Option<&str>, project: SandboxMode) -> SandboxMode {
    harness.and_then(SandboxMode::parse).unwrap_or(project)
}

/// Persisted as `.pulsar/agent_scene.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SceneSettings {
    /// Project default for harnesses that ask for a sandbox.
    pub sandbox_mode: SandboxMode,
    /// Object cap of the `copy` mode; 0 = unlimited.
    pub copy_max_objects: usize,
}

impl Default for SceneSettings {
    fn default() -> Self {
        Self {
            sandbox_mode: SandboxMode::default(),
            copy_max_objects: DEFAULT_COPY_MAX_OBJECTS,
        }
    }
}

impl SceneSettings {
    pub fn path(project_root: &Path) -> PathBuf {
        project_root.join(".pulsar").join(SCENE_SETTINGS_FILE)
    }

    /// Missing or unreadable files give the defaults.
    pub fn load(project_root: &Path) -> Self {
        std::fs::read_to_string(Self::path(project_root))
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, project_root: &Path) -> std::io::Result<()> {
        let path = Self::path(project_root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, body)
    }
}
