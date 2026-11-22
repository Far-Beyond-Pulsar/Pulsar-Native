//! Shipped game content.
//!
//! A packaged game is an executable next to a `Content/` directory:
//!
//! ```text
//! MyGame/
//!   my_game_game(.exe)
//!   Content/
//!     game.toc, game_NNNN.pak  everything below, as a pak set: compressed,
//!                              encrypted, in containers of bounded size
//!     (or, with `--loose`, the same files as a directory tree:)
//!     Pulsar/project.json      runtime project settings (startup level, window, script limits)
//!     Pulsar/scripting.json    global scripts
//!     Pulsar/class_index.json  class GUID -> class directory
//!     Pulsar/asset_registry.json
//!     Pulsar/modules/<class id>.pvm  compiled class modules (`pulsar_class::modules`)
//!     src/classes/<Class>/{class.json, prefab.json}
//!     scenes/*.level           cooked levels (content-relative asset paths)
//!     assets/...               meshes, textures, materials
//! ```
//!
//! Paths inside content are **content-relative** and use `/`: the same
//! relative layout as the project, so a dev build reading loose files from
//! the project root and a packaged build reading its pak set resolve the
//! same names.
//!
//! | Module | What |
//! |--------|------|
//! | [`pak`] | the pak set format: [`write_pak_set`], [`PakSet`], [`ContentKey`] |
//! | [`root`] | [`ContentRoot`]: where content comes from, found at startup ([`ContentRoot::discover`]) |
//! | [`packer`] | [`ContentPacker`]: the host's cook-and-pack, for the editor's build |
//! | [`provider`] | [`ContentFsProvider`]: serves a pak through `engine_fs::virtual_fs`, so path-based loaders read it |
//! | [`settings`] | [`ProjectSettings`]: `Pulsar/project.json` |
//! | [`registry`] | [`AssetRegistry`]: `Pulsar/asset_registry.json` |
#![cfg_attr(not(test), warn(clippy::unwrap_used))]

pub mod packer;
pub mod pak;
pub mod provider;
pub mod registry;
pub mod root;
pub mod settings;

pub use packer::{packer, ContentPacker, PackJob};
pub use pak::{
    install_content_key, installed_content_key, write_pak_set, ContentKey, ContentKeyError,
    PackEvent, PackReport, PackSettings, PakEntry, PakError, PakSet, CONTENT_KEY_FILE, PAK_VERSION,
    TOC_FILE_NAME,
};
pub use provider::ContentFsProvider;
pub use registry::{AssetKind, AssetRecord, AssetRegistry, ASSET_REGISTRY_FILE};
pub use root::{
    current, set_current, ContentRoot, RootError, CONTENT_DIR_ENV, CONTENT_DIR_NAME,
    PROJECT_ROOT_ENV,
};
pub use settings::{
    BuildProfile, ProjectSettings, ScriptProfileLimits, ScriptSettings, WindowSettings,
    PROJECT_SETTINGS_FILE,
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
        assert_eq!(
            normalize_rel("./a\\b//c.mesh").as_deref(),
            Some("a/b/c.mesh")
        );
        assert_eq!(
            normalize_rel("/Pulsar/project.json").as_deref(),
            Some("Pulsar/project.json")
        );
        assert_eq!(normalize_rel("../x"), None);
        assert_eq!(normalize_rel("C:/x"), None);
        assert_eq!(normalize_rel(""), None);
    }
}
