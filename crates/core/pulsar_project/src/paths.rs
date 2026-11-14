//! Where generated things live in a project, relative to its root, with
//! forward slashes (the spelling Cargo and the generation record use).

use std::path::{Path, PathBuf};

/// The project descriptor, owned by the user.
pub const DESCRIPTOR: &str = pulsar_plugins::PROJECT_FILE;
/// User-owned gameplay crates: `scripts/<dir>/Cargo.toml`.
pub const SCRIPTS_DIR: &str = "scripts";
/// The engine SDK mirror: rebuilt on each machine, never committed.
pub const SDK_DIR: &str = ".pulsar/sdk";
/// The mirror's record (`sdk::mirror::MirrorRecord`).
pub const SDK_RECORD: &str = ".pulsar/sdk/sdk.json";
/// Everything else the generator owns and nobody commits.
pub const BUILD_DIR: &str = ".pulsar/build";
/// The generated game crate (glue: launcher, Play-In-Editor shim, plugin
/// links, setup, classes compiled to Rust).
pub const GAME_DIR: &str = ".pulsar/build/game";
/// The generation record (`apply::GenerationRecord`).
pub const GENERATION_RECORD: &str = ".pulsar/build/generated.json";
/// The root virtual-workspace manifest: generated, portable, committed.
pub const ROOT_MANIFEST: &str = "Cargo.toml";
/// Seeded from the engine's lockfile, then Cargo's: committed.
pub const LOCKFILE: &str = "Cargo.lock";
pub const GITIGNORE: &str = ".gitignore";

/// The `.gitignore` lines a project needs: build output and the per-machine
/// generated trees. Added when missing; the file is otherwise the user's.
pub const IGNORED: &[&str] = &["/target/", "/.pulsar/sdk/", "/.pulsar/build/"];

/// `root` joined with a forward-slash relative path.
pub fn join(root: &Path, rel: &str) -> PathBuf {
    rel.split('/')
        .filter(|part| !part.is_empty())
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

/// The relative path from directory `from` to `to`, both forward-slash and
/// relative to one common root (`a/b`, `a/c/d` → `../c/d`).
pub fn relative(from: &str, to: &str) -> String {
    let from: Vec<&str> = from.split('/').filter(|p| !p.is_empty()).collect();
    let to: Vec<&str> = to.split('/').filter(|p| !p.is_empty()).collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<&str> = vec![".."; from.len() - common];
    parts.extend(&to[common..]);
    if parts.is_empty() {
        ".".to_owned()
    } else {
        parts.join("/")
    }
}

/// `rel` with `.` and `..` segments resolved and forward slashes; `None` if
/// it climbs above its root.
pub fn normalize_rel(rel: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for part in rel.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            part => out.push(part),
        }
    }
    Some(out.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_climb_to_the_common_ancestor() {
        assert_eq!(relative("a/b", "a/c/d"), "../c/d");
        assert_eq!(relative("a/b", "a/b/c"), "c");
        assert_eq!(relative("a/b", "a/b"), ".");
        assert_eq!(relative("", "x/y"), "x/y");
        assert_eq!(
            relative(".pulsar/build/game", "scripts/s"),
            "../../../scripts/s"
        );
    }

    #[test]
    fn normalize_rejects_escapes() {
        assert_eq!(normalize_rel("a/./b/../c").as_deref(), Some("a/c"));
        assert_eq!(normalize_rel("a\\b").as_deref(), Some("a/b"));
        assert_eq!(normalize_rel("../a"), None);
    }
}
