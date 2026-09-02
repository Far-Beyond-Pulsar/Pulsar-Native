//! Discovery of user gameplay script crates under a project's `scripts/`
//! directory (#653).
//!
//! Convention (scaffolded by [`super::core_project_builder`]):
//!
//! ```text
//! <project>/scripts/<crate_name>/Cargo.toml   # package named <crate_name>
//! <project>/scripts/<crate_name>/src/lib.rs   # exposes pub fn register_scripts(&mut TickLoop)
//! ```
//!
//! Two consumers:
//! * the project builder — needs the CRATE list to emit `[dependencies]`
//!   path-deps into the game manifest and `register_scripts` calls into
//!   `engine_main.rs`;
//! * the level editor — needs the ACTOR TYPES (scanned from
//!   `register_actor::<Type>(...)` calls, the documented authoring
//!   convention) to offer Rust actors in the add-object flow.
//!
//! Scanning is deliberately textual and cheap: script crates are tiny,
//! the patterns are conventions this engine scaffolds and documents, and a
//! full rustc front-end would be wildly disproportionate. Unparseable or
//! half-written crates are skipped, never fatal.

use std::path::Path;

/// One discovered gameplay script crate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptCrate {
    /// The crate's Cargo `[package] name` (what generated code names).
    pub name: String,
    /// Directory name under `scripts/` (the path-dependency spelling).
    pub dir_name: String,
}

/// One actor type a script crate registers, as declared by a
/// `game.register_actor::<Type>(...)` call in its sources.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptActorType {
    /// Package name of the declaring crate.
    pub crate_name: String,
    /// Short type name as written in the turbofish (`Spinner`, not the full
    /// path; the runtime identity is the full `type_name`, matched by suffix
    /// at reload — see `pulsar_game::scripts`).
    pub type_name: String,
}

/// List every directory under `<root>/scripts/` that holds a parseable
/// `Cargo.toml` with a package name. Missing `scripts/` ⇒ empty vec.
pub fn discover_script_crates(project_root: &Path) -> Vec<ScriptCrate> {
    let scripts_dir = project_root.join("scripts");
    let Ok(entries) = std::fs::read_dir(&scripts_dir) else {
        return Vec::new();
    };

    let mut crates: Vec<ScriptCrate> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let manifest = std::fs::read_to_string(e.path().join("Cargo.toml")).ok()?;
            let name = read_package_name(&manifest)?;
            Some(ScriptCrate {
                name,
                dir_name: e.file_name().to_string_lossy().into_owned(),
            })
        })
        .collect();
    crates.sort_by(|a, b| a.name.cmp(&b.name));
    crates.dedup_by(|a, b| a.name == b.name);
    crates
}

/// List every actor type registered by any script crate, sorted by
/// (crate, type). Scans `src/**.rs` one directory deep — enough for the
/// flat-layout convention script crates follow.
pub fn discover_script_actors(project_root: &Path) -> Vec<ScriptActorType> {
    let mut out = Vec::new();
    for krate in discover_script_crates(project_root) {
        let dir = project_root.join("scripts").join(&krate.dir_name);
        let mut stack = vec![dir.join("src")];
        while let Some(d) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    stack.push(path);
                    continue;
                }
                if path.extension().map(|e| e == "rs").unwrap_or(false) {
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        for ty in scan_registered_types(&text) {
                            out.push(ScriptActorType {
                                crate_name: krate.name.clone(),
                                type_name: ty,
                            });
                        }
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| (&a.crate_name, &a.type_name).cmp(&(&b.crate_name, &b.type_name)));
    out.dedup();
    out
}

/// Extract `[package] name = "…"` using the same minimal line parser the
/// PiE build flow uses for the project manifest (`read_crate_name` in the
/// level editor): good enough for builder-authored manifests, and anything
/// exotic simply yields `None` and is skipped.
fn read_package_name(manifest: &str) -> Option<String> {
    let mut in_package = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("name") {
            let rest = rest.trim_start().strip_prefix('=')?.trim();
            let value = rest.trim_matches('"');
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Capture every `<ident>` appearing as `register_actor::<ident>(` in `text`,
/// tolerating whitespace inside the turbofish. This IS the authoring contract:
/// the scaffolded `lib.rs` demonstrates it and the tutorial documents it.
fn scan_registered_types(text: &str) -> Vec<String> {
    const NEEDLE: &str = "register_actor::<";
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(pos) = rest.find(NEEDLE) {
        let tail = rest[pos + NEEDLE.len()..].trim_start();
        let ident: String = tail
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !ident.is_empty()
            && out
                .last()
                .map(|last: &String| last != &ident)
                .unwrap_or(true)
        {
            out.push(ident);
        }
        rest = tail;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scaffold a minimal two-crate scripts tree and assert both crate and
    /// actor-type discovery see exactly what the builder/editor would emit.
    #[test]
    fn discovers_crates_and_registered_actor_types() {
        let root = tempfile::tempdir().unwrap();
        let scripts = root.path().join("scripts");

        for (dir, name, body) in [
            (
                "game_scripts",
                "game_scripts",
                "pub fn register_scripts(game: &mut TickLoop) {\n    \
                 game.register_actor::< Spinner >(Spinner::default());\n}\n",
            ),
            (
                "extra",
                "extra_scripts",
                "// game.register_actor::<Hidden>(x); in a comment still matches —\n\
                 // acceptable: discovery is advisory.\n",
            ),
        ] {
            let src = scripts.join(dir).join("src");
            std::fs::create_dir_all(&src).unwrap();
            std::fs::write(
                scripts.join(dir).join("Cargo.toml"),
                format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n"),
            )
            .unwrap();
            std::fs::write(src.join("lib.rs"), body).unwrap();
        }

        let crates = discover_script_crates(root.path());
        assert_eq!(
            crates,
            vec![
                ScriptCrate {
                    name: "extra_scripts".into(),
                    dir_name: "extra".into()
                },
                ScriptCrate {
                    name: "game_scripts".into(),
                    dir_name: "game_scripts".into()
                },
            ],
            "sorted by package name"
        );

        let actors = discover_script_actors(root.path());
        assert_eq!(
            actors,
            vec![
                ScriptActorType {
                    crate_name: "extra_scripts".into(),
                    type_name: "Hidden".into(),
                },
                ScriptActorType {
                    crate_name: "game_scripts".into(),
                    type_name: "Spinner".into(),
                },
            ],
            "turbofish with inner whitespace is captured; textual scan is \
             deliberately comment-blind (advisory discovery)"
        );
    }

    /// No scripts/ directory (fresh project mid-bootstrap) is empty, never an
    /// error — callers treat absence as "nothing wired yet".
    #[test]
    fn missing_scripts_directory_is_empty_not_an_error() {
        let root = tempfile::tempdir().unwrap();
        assert!(discover_script_crates(root.path()).is_empty());
        assert!(discover_script_actors(root.path()).is_empty());
    }

    /// The package-name parser scopes itself to `[package]`: a `[dependencies]`
    /// table containing another `name` key must not win.
    #[test]
    fn package_name_parser_ignores_other_tables() {
        let manifest = "[package]\nname = \"real\"\n\n[dependencies]\nname = { path = \"x\" }\n";
        assert_eq!(read_package_name(manifest).as_deref(), Some("real"));
    }
}
