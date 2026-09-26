//! Shipped game content (Pulsar-Native#926).
//!
//! A packaged game is an executable next to a `Content/` directory:
//!
//! ```text
//! MyGame/
//!   my_game_game(.exe)
//!   Content/
//!     game.pak                 everything below, in one indexed archive
//!     (or, with `--loose`, the same files as a directory tree:)
//!     Pulsar/project.json      runtime project settings (startup level, window, script limits)
//!     Pulsar/scripting.json    global scripts
//!     Pulsar/class_index.json  class GUID -> class directory
//!     Pulsar/asset_registry.json
//!     src/classes/<Class>/{class.json, prefab.json, events/.build/module.pvm}
//!     scenes/*.level           cooked levels (content-relative asset paths)
//!     assets/...               meshes, textures, materials
//! ```
//!
//! Paths inside content are **content-relative** and use `/`: the same
//! relative layout as the project, so a dev build reading loose files from
//! the project root and a packaged build reading `game.pak` resolve the
//! same names.
//!
//! | Module | What |
//! |--------|------|
//! | [`pak`] | the `game.pak` format: [`PakWriter`], [`PakReader`] |
//! | [`root`] | [`ContentRoot`]: where content comes from, found at startup ([`ContentRoot::discover`]) |
//! | [`provider`] | [`ContentFsProvider`]: serves a pak through `engine_fs::virtual_fs`, so path-based loaders read it |
//! | [`settings`] | [`ProjectSettings`]: `Pulsar/project.json` |
//! | [`registry`] | [`AssetRegistry`]: `Pulsar/asset_registry.json` |

pub mod pak;
pub mod provider;
pub mod registry;
pub mod root;
pub mod settings;

pub use pak::{PakEntry, PakError, PakReader, PakWriter, PAK_FILE_NAME, PAK_MAGIC, PAK_VERSION};
pub use provider::ContentFsProvider;
pub use registry::{AssetKind, AssetRecord, AssetRegistry, ASSET_REGISTRY_FILE};
pub use root::{current, set_current, ContentRoot, CONTENT_DIR_NAME, CONTENT_DIR_ENV, PROJECT_ROOT_ENV};
pub use settings::{
    BuildProfile, ProjectSettings, ScriptProfileLimits, ScriptSettings, WindowSettings, PROJECT_SETTINGS_FILE,
};

/// Class GUID -> class directory index a packaged game reads instead of
/// scanning `src/classes` (written by the packager).
pub const CLASS_INDEX_FILE: &str = "Pulsar/class_index.json";

/// Normalize a content-relative path: `/` separators, no leading `./` or
/// `/`, no empty or `.` components. `None` for paths that leave the content
/// root (`..`) or are absolute on Windows (`C:`).
pub fn normalize_rel(path: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split(['/', '\\']) {
        match part {
            "" | "." => continue,
            ".." => return None,
            p if p.contains(':') => return None,
            p => parts.push(p),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::normalize_rel;

    #[test]
    fn relative_paths_normalize() {
        assert_eq!(normalize_rel("./a\\b//c.mesh").as_deref(), Some("a/b/c.mesh"));
        assert_eq!(normalize_rel("/Pulsar/project.json").as_deref(), Some("Pulsar/project.json"));
        assert_eq!(normalize_rel("../x"), None);
        assert_eq!(normalize_rel("C:/x"), None);
        assert_eq!(normalize_rel(""), None);
    }
}
