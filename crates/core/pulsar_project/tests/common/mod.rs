//! A miniature engine checkout and project, built in a temp directory. The
//! engine's crates are stubs with only path dependencies, so Cargo resolves
//! everything offline.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

pub fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

pub fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

pub fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

/// A stub library package.
fn stub(root: &Path, dir: &str, name: &str, manifest_tail: &str) {
    write(
        &root.join(dir).join("Cargo.toml"),
        &format!(
            "[package]\nname = \"{name}\"\nversion.workspace = true\nedition.workspace = true\n\n\
             [lints]\nworkspace = true\n\n{manifest_tail}"
        ),
    );
    write(&root.join(dir).join("src/lib.rs"), "");
}

/// Stub crates the game crate names, at `crates/<name>`.
const STUBS: &[&str] = &[
    "pulsar_pie_abi",
    "tracing",
    "engine_class_derive",
    "inventory",
    "pulsar_reflection",
    "pulsar_scenedb",
    "pulsar_script_vm",
    "pulsar_world_registry",
    "serde",
    "serde_json",
    "unused_editor",
];

pub struct Engine {
    pub root: PathBuf,
}

impl Engine {
    pub fn new(root: PathBuf) -> Self {
        let engine = Self { root };
        let r = &engine.root;
        let mut deps = String::new();
        for name in STUBS {
            deps.push_str(&format!("{name} = {{ path = \"crates/{name}\" }}\n"));
        }
        write(
            &r.join("Cargo.toml"),
            &format!(
                r#"[workspace]
resolver = "2"
members = ["crates/*", "plugins/lights", "plugins/lights/component", "plugins/lights/editor", "plugins/tool"]
# A workspace of its own (as the renderer submodule is): its crates inherit
# from it, not from this one.
exclude = ["renderer"]

[workspace.package]
version = "0.4.0"
edition = "2021"

[workspace.lints.rust]
unsafe_code = "forbid"

[workspace.dependencies]
pulsar_game = {{ path = "crates/pulsar_game" }}
render = {{ path = "renderer/crates/render" }}
tracing-subscriber = {{ path = "crates/tracing-subscriber" }}
lights_plugin = {{ path = "plugins/lights" }}
tool_plugin = {{ path = "plugins/tool" }}
{deps}
[patch."https://example.com/upstream"]
tracing = {{ path = "crates/tracing" }}
unused_editor = {{ path = "crates/unused_editor" }}

[profile.dev.package]
tracing = {{ opt-level = 3 }}
not_in_the_game = {{ opt-level = 3 }}
"#
            ),
        );
        for name in STUBS {
            stub(r, &format!("crates/{name}"), name, "");
        }
        // A proc-macro: `cargo tree` prints these differently.
        stub(
            r,
            "crates/engine_class_derive",
            "engine_class_derive",
            "[lib]\nproc-macro = true\n",
        );
        stub(
            r,
            "crates/tracing-subscriber",
            "tracing-subscriber",
            "[features]\nfmt = []\nenv-filter = []\n",
        );
        stub(
            r,
            "crates/pulsar_game",
            "pulsar_game",
            "[dependencies]\ntracing.workspace = true\nserde = { workspace = true }\n\
             serde_json.workspace = true\ninventory.workspace = true\nrender.workspace = true\n\n\
             [dev-dependencies]\nunused_editor.workspace = true\n",
        );
        write(
            &r.join("renderer/Cargo.toml"),
            "[workspace]\nresolver = \"2\"\nmembers = [\"crates/*\"]\n\n\
             [workspace.package]\nversion = \"7.0.0\"\nedition = \"2021\"\n\n\
             [workspace.dependencies]\nrender_core = { path = \"crates/render_core\" }\n",
        );
        write(
            &r.join("renderer/crates/render/Cargo.toml"),
            "[package]\nname = \"render\"\nversion.workspace = true\nedition.workspace = true\n\n\
             [dependencies]\nrender_core.workspace = true\n",
        );
        write(&r.join("renderer/crates/render/src/lib.rs"), "");
        write(
            &r.join("renderer/crates/render_core/Cargo.toml"),
            "[package]\nname = \"render_core\"\nversion.workspace = true\nedition.workspace = true\n",
        );
        write(&r.join("renderer/crates/render_core/src/lib.rs"), "");
        // Not mirrored: tests, build output, version control.
        write(&r.join("crates/pulsar_game/tests/t.rs"), "#[test] fn t() {}");
        write(&r.join("crates/pulsar_game/target/junk.bin"), "junk");
        write(&r.join("crates/pulsar_game/.git/HEAD"), "ref");
        write(&r.join("crates/pulsar_game/shaders/sky.wgsl"), "// data");

        stub(
            r,
            "plugins/lights",
            "lights_plugin",
            "[features]\neditor = [\"dep:lights_editor\"]\n\n[dependencies]\n\
             lights_component = { path = \"component\" }\n\
             lights_editor = { path = \"editor\", optional = true }\n\
             pulsar_game.workspace = true\n",
        );
        write(
            &r.join("plugins/lights/pulsar-plugin.toml"),
            "id = \"pulsar.lights\"\nname = \"Lights\"\nroot = \"lights_plugin\"\n",
        );
        stub(r, "plugins/lights/component", "lights_component", "");
        stub(
            r,
            "plugins/lights/editor",
            "lights_editor",
            "[dependencies]\nunused_editor.workspace = true\n",
        );
        stub(r, "plugins/tool", "tool_plugin", "");
        write(
            &r.join("plugins/tool/pulsar-plugin.toml"),
            "id = \"pulsar.tool\"\nname = \"Tool\"\nroot = \"tool_plugin\"\neditor_only = true\n",
        );
        // A plugin in the workspace that the engine does not install (a
        // test fixture): never a game's.
        stub(r, "crates/fixture_plugin", "fixture_plugin", "");
        write(
            &r.join("crates/fixture_plugin/pulsar-plugin.toml"),
            "id = \"pulsar.fixture\"\nname = \"Fixture\"\nroot = \"fixture_plugin\"\n",
        );
        // The engine's installed set.
        stub(
            r,
            "crates/installed_plugins",
            "installed_plugins",
            "[dependencies]\nlights_plugin.workspace = true\n\
             tool_plugin = { workspace = true, optional = true }\n\n\
             [dev-dependencies]\nfixture_plugin = { path = \"../fixture_plugin\" }\n",
        );
        engine.lock();
        engine
    }

    /// Regenerate the engine's lockfile (offline: every dependency is a path).
    pub fn lock(&self) {
        let status = cargo()
            .args(["generate-lockfile", "--offline", "--manifest-path"])
            .arg(self.root.join("Cargo.toml"))
            .status()
            .unwrap();
        assert!(status.success(), "the fixture engine resolves");
    }
}

/// A project named "Star Fall" with one scripts crate.
pub fn project(root: &Path) {
    write(
        &root.join("Pulsar.toml"),
        "[project]\nname = \"Star Fall\"\nversion = \"1.2.0\"\n",
    );
    write(
        &root.join("scripts/fall/Cargo.toml"),
        "[package]\nname = \"fall_scripts\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\npulsar_game = { workspace = true }\n",
    );
    write(&root.join("scripts/fall/src/lib.rs"), "");
}

/// Every file under `dir`, relative, with its content.
pub fn tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path
                    .strip_prefix(dir)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, std::fs::read(&path).unwrap()));
            }
        }
    }
    out.sort();
    out
}
