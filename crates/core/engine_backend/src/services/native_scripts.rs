//! Discovery of user gameplay script crates under a project's `scripts/`
//! directory.
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
//!   `register_native_actor::<Type>(...)` calls, the documented
//!   authoring convention) to offer Rust actors in the add-object flow.
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
/// `game.register_native_actor::<Type>(...)` call in its sources.
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
    let Ok(entries) = engine_fs::virtual_fs::list_dir(&scripts_dir) else {
        return Vec::new();
    };

    let mut crates: Vec<ScriptCrate> = entries
        .into_iter()
        .filter(|e| e.is_dir)
        .filter_map(|e| {
            let manifest = engine_fs::virtual_fs::read_to_string(
                &scripts_dir.join(&e.name).join("Cargo.toml"),
            )
            .ok()?;
            let name = read_package_name(&manifest)?;
            Some(ScriptCrate {
                name,
                dir_name: e.name,
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
            let Ok(entries) = engine_fs::virtual_fs::list_dir(&d) else {
                continue;
            };
            for entry in entries {
                let path = d.join(&entry.name);
                if entry.is_dir {
                    stack.push(path);
                    continue;
                }
                if path.extension().map(|e| e == "rs").unwrap_or(false) {
                    if let Ok(text) = engine_fs::virtual_fs::read_to_string(&path) {
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

/// Capture every `<ident>` appearing as `register_native_actor::<ident>(` in `text`,
/// tolerating whitespace inside the turbofish. This IS the authoring contract:
/// the scaffolded `lib.rs` demonstrates it and the tutorial documents it.
fn scan_registered_types(text: &str) -> Vec<String> {
    const NEEDLES: [&str; 1] = ["register_native_actor::<"];
    let mut out = Vec::new();
    let mut rest = text;
    while let Some((pos, needle)) = NEEDLES
        .iter()
        .filter_map(|needle| rest.find(needle).map(|pos| (pos, *needle)))
        .min_by_key(|(pos, _)| *pos)
    {
        let tail = rest[pos + needle.len()..].trim_start();
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

// ---- whether a project has native gameplay -------------------------------

/// What in a script crate's code (comments left out) makes it native
/// gameplay: anything that must be compiled and linked to take effect.
const NATIVE_MARKERS: [&str; 5] = [
    "register_native_actor",
    "inventory::submit",
    "engine_class",
    "world_component",
    "no_mangle",
];

/// Why the project at `root` needs its own native code compiled to play, or
/// `None` when its gameplay is scripts only: every gameplay script crate
/// registers nothing, `Pulsar/level.json` names no native prefab, and the
/// game's setup is the generated one. Such a project plays on the engine the
/// editor already runs.
///
/// Textual and strict, like the rest of this module: anything it does not
/// recognise as empty counts as native.
pub fn native_gameplay(root: &Path) -> Option<String> {
    for script in discover_script_crates(root) {
        let source = root.join("scripts").join(&script.dir_name).join("src");
        if let Some(reason) = native_code_in(&source) {
            return Some(format!("scripts/{}: {reason}", script.dir_name));
        }
    }
    let level = root.join("Pulsar").join("level.json");
    if let Ok(text) = engine_fs::virtual_fs::read_to_string(&level) {
        let prefabs = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|value| value.get("prefabs")?.as_array().map(Vec::len));
        if prefabs != Some(0) {
            return Some("Pulsar/level.json names native prefabs".into());
        }
    }
    let setup = root.join("src").join("engine_main.rs");
    if let Ok(text) = engine_fs::virtual_fs::read_to_string(&setup) {
        if !text.contains("@generated-by-pulsar-engine_main") {
            return Some("src/engine_main.rs is the project's own".into());
        }
    }
    None
}

/// Why the sources under `dir` are native gameplay, if they are.
fn native_code_in(dir: &Path) -> Option<String> {
    let entries = match engine_fs::virtual_fs::list_dir(dir) {
        Ok(entries) => entries,
        Err(error) => return Some(format!("its sources could not be read ({error})")),
    };
    for entry in entries {
        let path = dir.join(&entry.name);
        if entry.is_dir {
            if let Some(reason) = native_code_in(&path) {
                return Some(reason);
            }
            continue;
        }
        if !entry.name.ends_with(".rs") {
            continue;
        }
        let Ok(text) = engine_fs::virtual_fs::read_to_string(&path) else {
            return Some(format!("{} could not be read", entry.name));
        };
        let code = without_comments(&text);
        if let Some(marker) = NATIVE_MARKERS.iter().find(|marker| code.contains(*marker)) {
            return Some(format!("{} uses `{marker}`", entry.name));
        }
        if entry.name == "lib.rs" && !registers_nothing(&code) {
            return Some("`register_scripts` is not empty".into());
        }
    }
    None
}

/// `code` without its line and block comments. String literals holding
/// comment markers are cut too, which can only make the answer stricter or
/// drop text no marker is in.
fn without_comments(code: &str) -> String {
    let mut out = String::with_capacity(code.len());
    let mut rest = code;
    loop {
        let line = rest.find("//");
        let block = rest.find("/*");
        let (at, closing, keep_newline) = match (line, block) {
            (Some(l), Some(b)) if l < b => (l, "\n", true),
            (Some(l), None) => (l, "\n", true),
            (_, Some(b)) => (b, "*/", false),
            (None, None) => break,
        };
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        match after.find(closing) {
            Some(end) => {
                if keep_newline {
                    out.push('\n');
                }
                rest = &after[end + closing.len()..];
            }
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Whether `register_scripts` in `code` does nothing but succeed.
fn registers_nothing(code: &str) -> bool {
    let Some(at) = code.find("fn register_scripts") else {
        return false;
    };
    let Some(open) = code[at..].find('{').map(|i| at + i) else {
        return false;
    };
    let mut depth = 0usize;
    for (i, c) in code[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    let body: String = code[open + 1..open + i]
                        .chars()
                        .filter(|c| !c.is_whitespace())
                        .collect();
                    return body == "Ok(())";
                }
            }
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(lib: &str) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let src = root.path().join("scripts/game_scripts/src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            root.path().join("scripts/game_scripts/Cargo.toml"),
            "[package]\nname = \"game_scripts\"\n",
        )
        .unwrap();
        std::fs::write(src.join("lib.rs"), lib).unwrap();
        root
    }

    #[test]
    fn a_script_crate_that_registers_nothing_is_not_native_gameplay() {
        let root = project(
            "//! Register with `game.register_native_actor::<Type>(...)`.\n\
             use pulsar_game::tick::TickLoop;\n\
             /* engine_class */\n\
             pub fn register_scripts(_game: &mut TickLoop) -> Result<(), String> {\n    Ok(())\n}\n",
        );
        assert_eq!(native_gameplay(root.path()), None);
    }

    #[test]
    fn registrations_prefabs_and_a_custom_setup_are_native_gameplay() {
        let registers = project(
            "pub fn register_scripts(game: &mut TickLoop) -> Result<(), String> {\n\
             game.register_native_actor::<Spinner>(Spinner);\n Ok(())\n}\n",
        );
        assert!(native_gameplay(registers.path()).is_some());

        let does_more = project(
            "pub fn register_scripts(game: &mut TickLoop) -> Result<(), String> {\n\
             game.add_system(spin); Ok(())\n}\n",
        );
        assert!(native_gameplay(does_more.path()).is_some());

        let empty = "pub fn register_scripts(_: &mut TickLoop) -> Result<(), String> { Ok(()) }";
        let prefabs = project(empty);
        std::fs::create_dir_all(prefabs.path().join("Pulsar")).unwrap();
        std::fs::write(
            prefabs.path().join("Pulsar/level.json"),
            r#"{ "prefabs": [ { "class": "Door" } ] }"#,
        )
        .unwrap();
        assert!(native_gameplay(prefabs.path()).is_some());

        let custom = project(empty);
        std::fs::create_dir_all(custom.path().join("src")).unwrap();
        std::fs::write(custom.path().join("src/engine_main.rs"), "pub fn setup() {}").unwrap();
        assert!(native_gameplay(custom.path()).is_some());
    }

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
                 game.register_native_actor::< Spinner >(Spinner::default());\n}\n",
            ),
            (
                "extra",
                "extra_scripts",
                "// game.register_native_actor::<Hidden>(x); in a comment still matches —\n\
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
