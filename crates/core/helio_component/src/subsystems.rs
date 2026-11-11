use std::path::{Path, PathBuf};

/// The engine's built-in assets (`assets/` at the repository root): the last
/// place [`resolve_asset_path`] looks.
const ENGINE_ASSETS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../assets");

fn project_exists(path: &Path) -> bool {
    engine_fs::virtual_fs::exists(path).unwrap_or(false)
}

// std::fs allowed: the working directory and the engine's own built-in assets are real
// machine paths, outside any project (and outside the project's provider)
#[allow(clippy::disallowed_methods)]
fn exists_on_machine(path: &Path) -> bool {
    path.exists()
}

/// Resolve an asset path to an existing file on disk.
///
/// Checks (in order):
///  1. absolute path
///  2. project-root-relative
///  3. working-directory-relative
///  4. `cwd/assets/` (editor convention)
///  5. engine built-in assets (embedded primitives dir)
pub fn resolve_asset_path(project_root: &Path, asset: &str) -> PathBuf {
    let norm = asset.replace('\\', "/");
    let p = Path::new(&norm);

    if p.is_absolute() && project_exists(p) {
        return p.to_path_buf();
    }

    let proj = project_root.join(&norm);
    if project_exists(&proj) {
        return proj;
    }

    if let Ok(cwd) = std::env::current_dir() {
        let cwd_path = cwd.join(&norm);
        if exists_on_machine(&cwd_path) {
            return cwd_path;
        }
        let cwd_assets = cwd.join("assets").join(&norm);
        if exists_on_machine(&cwd_assets) {
            return cwd_assets;
        }
    }

    let engine = Path::new(ENGINE_ASSETS_DIR).join(&norm);
    if exists_on_machine(&engine) {
        return engine;
    }

    proj
}
