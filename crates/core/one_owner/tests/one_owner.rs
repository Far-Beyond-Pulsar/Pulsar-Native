//! The tree keeps one owner per concept, and the check catches a copy.

use one_owner::{copies, exceptions, read_tree, Source};

fn root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap()
}

/// What the rule covers: the frame (the Helio submodule), every plugin and
/// Pulsar's engine crates.
const DIRS: &[&str] = &[
    "crates/renderer/helio/crates",
    "plugins/vendor",
    "crates/core",
    "crates/subsystems",
];

#[test]
fn every_cross_kind_concept_has_one_owner() {
    let sources = read_tree(&root(), DIRS);
    assert!(
        sources.iter().any(|s| s.path.ends_with("prelude.wgsl")),
        "the Helio submodule is not checked out"
    );
    let recorded = exceptions(include_str!("../exceptions.txt"));
    let found = copies(&sources);
    let new: Vec<String> = found
        .iter()
        .filter(|c| !recorded.contains(&(c.path.clone(), c.symbol.clone())))
        .map(ToString::to_string)
        .collect();
    let gone: Vec<String> = recorded
        .iter()
        .filter(|(path, symbol)| !found.iter().any(|c| c.path == *path && c.symbol == *symbol))
        .map(|(path, symbol)| format!("{path} | {symbol}"))
        .collect();
    assert!(
        new.is_empty(),
        "private copies of another owner's concept (use the owner's api crate \
         or WGSL snippet instead):\n{}",
        new.join("\n")
    );
    assert!(
        gone.is_empty(),
        "exceptions.txt lists copies that are gone; remove them:\n{}",
        gone.join("\n")
    );
}

fn source(path: &str, text: &str) -> Source {
    Source {
        path: path.into(),
        text: text.into(),
    }
}

/// A planted copy of each kind is caught; the owner's own use is not.
#[test]
fn a_planted_copy_is_caught() {
    let frame = "crates/renderer/helio/crates";
    let tree = vec![
        // The frame's camera snippet and its key.
        source(
            &format!("{frame}/helio-core/src/shader/prelude.wgsl"),
            "struct Camera {\n    view: mat4x4<f32>,\n    proj: mat4x4<f32>,\n    view_proj: mat4x4<f32>,\n}\nconst MEDIA_NEAR: f32 = 0.5;\n",
        ),
        source(
            &format!("{frame}/helio-frame-api/src/color.rs"),
            "pub const SCENE_COLOR: &str = \"pre_aa\";\n",
        ),
        // The frame reading its own key by name is the owner's business.
        source(
            &format!("{frame}/helio-spine/src/background.rs"),
            "fn reads(&self) -> &'static [&'static str] {\n    &[\"pre_aa\"]\n}\n",
        ),
        // A plugin's copies: the struct by name, by layout, a const, a key.
        source(
            "plugins/vendor/fog/feature/shaders/fog.wgsl",
            "struct Camera { view: mat4x4<f32>, proj: mat4x4<f32>, view_proj: mat4x4<f32> }\n\
             struct FogView {\n    view: mat4x4<f32>,\n    proj: mat4x4<f32>,\n    view_proj: mat4x4<f32>,\n}\n\
             const MEDIA_NEAR: f32 = 0.5;\nstruct FogOwn { a: u32, b: u32, c: u32 }\n",
        ),
        source(
            "plugins/vendor/fog/feature/src/lib.rs",
            "fn run(r: &Registry) {\n    let v = r.get(helio_core::ResourceKey::new(\"pre_aa\"));\n    let label = Some(\"pre_aa\"); // a label is no key\n}\n#[gpu(buffer = \"pre_aa\")]\nstruct Row;\n",
        ),
        // Test code may name what it likes.
        source(
            "plugins/vendor/fog/feature/tests/fog.rs",
            "let v = r.get(ResourceKey::new(\"pre_aa\"));\n",
        ),
    ];
    let found: Vec<String> = copies(&tree)
        .iter()
        .map(|c| format!("{} | {}", c.path, c.symbol))
        .collect();
    assert_eq!(
        found,
        [
            "plugins/vendor/fog/feature/shaders/fog.wgsl | const MEDIA_NEAR",
            "plugins/vendor/fog/feature/shaders/fog.wgsl | struct Camera",
            "plugins/vendor/fog/feature/shaders/fog.wgsl | struct FogView",
            "plugins/vendor/fog/feature/src/lib.rs | \"pre_aa\"",
        ]
    );
}
