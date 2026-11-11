//! `std::fs` (and the `Path` methods that ask the disk) are disallowed
//! methods across the workspace (the root `clippy.toml`, denied in
//! `[workspace.lints.clippy]`): every file goes through `engine_fs`. The
//! code that must touch the real disk says so with
//! `#[allow(clippy::disallowed_methods)]`, and each such file is listed with
//! its reason in `architecture/std-fs-allowed.txt`. This test keeps the two in
//! step: an allowance in an unlisted file fails, and so does a listed file
//! that no longer needs one (the list only shrinks).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const ALLOW: &str = "allow(clippy::disallowed_methods)";

/// What the root `clippy.toml` disallows for the disk (and `RustEmbed`, whose
/// derive reads the disk in a debug build): a file whose allowance names none
/// of these allows another rule's method (Helio's `create_shader_module`),
/// not `std::fs`.
const DISK_API: &[&str] = &[
    "RustEmbed",
    "fs::",
    "File::",
    "OpenOptions",
    ".exists()",
    ".try_exists()",
    ".is_file()",
    ".is_dir()",
    ".metadata()",
    ".symlink_metadata()",
    ".read_dir()",
    ".canonicalize()",
    ".read_link()",
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("engine_fs stays under crates/core")
        .to_path_buf()
}

/// Rust files of the workspace's own code (vendored and external trees,
/// which keep their own rules, are skipped).
fn rust_files(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if path.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.')
                || name == "target"
                || name == "node_modules"
                || [
                    "crates/ui",
                    "crates/renderer",
                    "crates/external",
                    "crates/third-party",
                ]
                .contains(&relative.as_str())
            {
                continue;
            }
            rust_files(&path, root, out);
        } else if relative.ends_with(".rs") && !relative.contains("/tests/") {
            // Integration tests are outside the rule (clippy lints the
            // libraries and binaries), so an allowance there is never needed.
            out.push(path);
        }
    }
}

fn listed(root: &Path) -> BTreeSet<String> {
    std::fs::read_to_string(root.join("architecture/std-fs-allowed.txt"))
        .expect("architecture/std-fs-allowed.txt")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.split('|').next().unwrap_or(line).trim().to_owned())
        .collect()
}

#[test]
fn every_std_fs_allowance_is_listed_and_every_listed_file_still_needs_one() {
    let root = workspace_root();
    let mut files = Vec::new();
    for top in ["crates", "plugins", "tools"] {
        rust_files(&root.join(top), &root, &mut files);
    }
    let allowing: BTreeSet<String> = files
        .iter()
        .filter(|path| {
            std::fs::read_to_string(path)
                .map(|text| text.contains(ALLOW) && DISK_API.iter().any(|api| text.contains(api)))
                .unwrap_or(false)
        })
        .map(|path| {
            path.strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    let listed = listed(&root);
    let unlisted: Vec<_> = allowing.difference(&listed).collect();
    let stale: Vec<_> = listed.difference(&allowing).collect();
    assert!(
        unlisted.is_empty(),
        "these files allow `std::fs` but are not in architecture/std-fs-allowed.txt; \
         use engine_fs::virtual_fs instead: {unlisted:#?}"
    );
    assert!(
        stale.is_empty(),
        "these listed files no longer allow `std::fs`: remove them from \
         architecture/std-fs-allowed.txt (it only shrinks): {stale:#?}"
    );
}
