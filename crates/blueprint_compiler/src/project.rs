//! # Blueprint Output Generator
//!
//! Turns a collection of compiled blueprints into blueprint-owned Rust source
//! files only. Core project/bootstrap files (like `Cargo.toml` or `main.rs`)
//! are intentionally out of scope and must be handled by the core build system.
//!
//! ## Usage
//!
//! ```no_run
//! use blueprint_compiler::{compile_blueprint, GraphDescription};
//! use blueprint_compiler::project::{ProjectSpec, CompiledBlueprint, generate_project};
//!
//! // Compile your graphs first.
//! let graph = GraphDescription::new("player_controller");
//! let source = compile_blueprint(&graph).unwrap();
//!
//! // Describe the project and add blueprints.
//! let spec = ProjectSpec::new("my_game")
//!     .version("0.1.0")
//!     .description("My Pulsar game")
//!     .add_blueprint(CompiledBlueprint::new("player_controller", source));
//!
//! // Generate blueprint files and write to disk.
//! let project = generate_project(&spec);
//! project.write_to_dir("./output/my_game").unwrap();
//! ```

use std::collections::BTreeMap;
use std::path::Path;

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Convert `some_name` / `SomeName` to `some_name` (stable, allocation-free).
fn to_snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    let mut prev_upper = false;
    for (i, ch) in name.char_indices() {
        if ch.is_uppercase() {
            if i != 0 && !prev_upper {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
            prev_upper = true;
        } else if ch == '-' || ch == ' ' {
            out.push('_');
            prev_upper = false;
        } else {
            out.push(ch);
            prev_upper = false;
        }
    }
    out
}

/// Convert `some_name` / `some-name` to `SomeName`.
fn to_pascal_case(name: &str) -> String {
    name.split(|c: char| c == '_' || c == '-' || c == ' ')
        .filter(|s| !s.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => {
                    let mut s = first.to_uppercase().to_string();
                    s.push_str(chars.as_str());
                    s
                }
            }
        })
        .collect()
}

// ── Types ─────────────────────────────────────────────────────────────────────

// ── CompiledBlueprint ─────────────────────────────────────────────────────────

/// A blueprint that has already been compiled to Rust source by `compile_blueprint`.
#[derive(Debug, Clone)]
pub struct CompiledBlueprint {
    /// Original name as given to the blueprint graph.
    pub name: String,
    /// Rust source emitted by the blueprint compiler.
    pub source: String,
    /// Whether this blueprint has a `tick` event entry point in its source.
    pub has_tick: bool,
    /// Whether this blueprint has a `begin_play` event entry point in its source.
    pub has_begin_play: bool,
}

impl CompiledBlueprint {
    /// Create from a name and compiled source, auto-detecting event entry points.
    pub fn new(name: impl Into<String>, source: impl Into<String>) -> Self {
        let source = source.into();
        let has_tick = source.contains("fn tick") || source.contains("fn on_tick");
        let has_begin_play =
            source.contains("fn begin_play") || source.contains("fn on_begin_play");
        Self {
            name: name.into(),
            source,
            has_tick,
            has_begin_play,
        }
    }

    /// Override tick detection.
    pub fn with_tick(mut self, has_tick: bool) -> Self {
        self.has_tick = has_tick;
        self
    }

    /// Override begin_play detection.
    pub fn with_begin_play(mut self, has_begin_play: bool) -> Self {
        self.has_begin_play = has_begin_play;
        self
    }
}

// ── ProjectSpec ───────────────────────────────────────────────────────────────

/// Everything needed to generate blueprint output files.
pub struct ProjectSpec {
    pub name: String,
    pub version: String,
    pub description: String,
    pub blueprints: Vec<CompiledBlueprint>,
}

impl ProjectSpec {
    /// Start building a project spec with the given crate name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: "0.1.0".into(),
            description: String::new(),
            blueprints: Vec::new(),
        }
    }

    /// Set the crate version (defaults to `"0.1.0"`).
    pub fn version(mut self, v: impl Into<String>) -> Self {
        self.version = v.into();
        self
    }

    /// Set the crate description.
    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = d.into();
        self
    }

    /// Add a compiled blueprint to the project.
    pub fn add_blueprint(mut self, bp: CompiledBlueprint) -> Self {
        self.blueprints.push(bp);
        self
    }
}

// ── GeneratedProject ──────────────────────────────────────────────────────────

/// The output of `generate_project` — blueprint files ready to write to disk.
pub struct GeneratedProject {
    /// Map of relative file path → file content.
    pub files: BTreeMap<String, String>,
}

impl GeneratedProject {
    /// Write every file to `<dir>/<relative_path>`, creating directories as needed.
    pub fn write_to_dir(&self, dir: impl AsRef<Path>) -> std::io::Result<()> {
        let base = dir.as_ref();
        for (rel_path, content) in &self.files {
            let full = base.join(rel_path);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&full, content)?;
        }
        Ok(())
    }

    /// Iterate over all generated file paths (relative).
    pub fn file_paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(|s| s.as_str())
    }
}

// ── generate_project ─────────────────────────────────────────────────────────

/// Generate blueprint-owned source files from a [`ProjectSpec`].
///
/// Returns a [`GeneratedProject`] whose files can be written anywhere with
/// [`GeneratedProject::write_to_dir`].
pub fn generate_project(spec: &ProjectSpec) -> GeneratedProject {
    let mut files = BTreeMap::new();
    files.insert("src/blueprints/mod.rs".into(), gen_blueprints_mod(spec));

    for bp in &spec.blueprints {
        let ident = to_snake_case(&bp.name);
        files.insert(
            format!("src/blueprints/{ident}.rs"),
            gen_blueprint_actor(bp),
        );
    }

    GeneratedProject { files }
}

// ── File generators ───────────────────────────────────────────────────────────

fn gen_blueprints_mod(spec: &ProjectSpec) -> String {
    let mod_decls: String = spec
        .blueprints
        .iter()
        .map(|bp| {
            let ident = to_snake_case(&bp.name);
            format!("pub mod {ident};\n")
        })
        .collect();

    let use_decls: String = spec
        .blueprints
        .iter()
        .map(|bp| {
            let ident = to_snake_case(&bp.name);
            let ty = to_pascal_case(&bp.name);
            format!("pub use {ident}::{ty};\n")
        })
        .collect();

    let class_name_matches: String = spec
        .blueprints
        .iter()
        .map(|bp| {
            let class_name = to_pascal_case(&bp.name);
            let ty = to_pascal_case(&bp.name);
            format!(
                "        \"{class_name}\" => Some(actors.register({ty}::new(), world)),\n"
            )
        })
        .collect();

    let class_names: String = spec
        .blueprints
        .iter()
        .map(|bp| format!("\"{}\"", to_pascal_case(&bp.name)))
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        r#"//! Blueprint actor registry — generated by Pulsar Blueprint Compiler.
//!
//! Blueprint class structs auto-register with `pulsar_reflection` via
//! `#[derive(EngineClass)]` in each generated file.

{mod_decls}
{use_decls}
use pulsar_game::{{ActorRegistry, Entity, World}};

/// List all compiled blueprint class names.
pub fn compiled_class_names() -> &'static [&'static str] {{
    &[{class_names}]
}}

/// Spawn a compiled blueprint class by name.
///
/// Returns the spawned entity if the class is known.
pub fn spawn_compiled_class(
    class_name: &str,
    world: &mut World,
    actors: &mut ActorRegistry,
) -> Option<Entity> {{
    match class_name {{
{class_name_matches}        _ => None,
    }}
}}
"#,
        mod_decls = mod_decls,
        use_decls = use_decls,
        class_names = class_names,
        class_name_matches = class_name_matches,
    )
}

fn gen_blueprint_actor(bp: &CompiledBlueprint) -> String {
    let ident = to_snake_case(&bp.name);
    let ty = to_pascal_case(&bp.name);

    let begin_play_body = if bp.has_begin_play {
        format!("        logic::begin_play();\n")
    } else {
        "        // No begin_play event in this blueprint.\n".into()
    };

    let tick_body = if bp.has_tick {
        format!("        logic::tick();\n")
    } else {
        "        // No tick event in this blueprint.\n".into()
    };

    // Indent the raw compiled source into the logic module.
    let indented_source: String = bp
        .source
        .lines()
        .map(|line| {
            if line.trim().is_empty() {
                "\n".into()
            } else {
                format!("    {line}\n")
            }
        })
        .collect();

    format!(
        r#"//! Blueprint actor: `{ident}`
//!
//! Generated by Pulsar Blueprint Compiler.
//! Edit the blueprint graph in the editor; do not hand-edit this file.

use pulsar_game::prelude::*;
use engine_class_derive::EngineClass;

// ── Actor ─────────────────────────────────────────────────────────────────────

/// Actor for blueprint `{ident}`.
#[derive(Clone, EngineClass)]
pub struct {ty} {{}}

impl {ty} {{
    pub fn new() -> Self {{
        Self {{}}
    }}
}}

impl Default for {ty} {{
    fn default() -> Self {{
        Self::new()
    }}
}}

impl Actor for {ty} {{
    fn begin_play(&mut self, _entity: Entity, _world: &mut World) {{
{begin_play_body}    }}

    fn tick(&mut self, _entity: Entity, _world: &mut World, _time: GameTime) {{
{tick_body}    }}
}}

// ── Blueprint logic ───────────────────────────────────────────────────────────
//
// Everything below is the raw output of the blueprint graph compiler.
// You can read it to understand what your graph does, but changes here
// will be overwritten the next time the blueprint is compiled.

mod logic {{
{indented_source}}}
"#,
        ident = ident,
        ty = ty,
        begin_play_body = begin_play_body,
        tick_body = tick_body,
        indented_source = indented_source,
    )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_spec() -> ProjectSpec {
        let source = r#"
pub fn begin_play() {
    let x = add(1.0, 2.0);
    print_number(x);
}
"#;
        ProjectSpec::new("my_game")
            .version("0.1.0")
            .description("A test Pulsar game")
            .add_blueprint(CompiledBlueprint::new("player_controller", source))
            .add_blueprint(CompiledBlueprint::new("enemy_ai", "pub fn tick() { }")
                .with_tick(true))
    }

    #[test]
    fn generates_expected_files() {
        let project = generate_project(&sample_spec());
        let paths: Vec<&str> = project.file_paths().collect();

        assert!(paths.contains(&"src/blueprints/mod.rs"));
        assert!(paths.contains(&"src/blueprints/player_controller.rs"));
        assert!(paths.contains(&"src/blueprints/enemy_ai.rs"));
    }

    #[test]
    fn actor_file_contains_struct_and_impl() {
        let project = generate_project(&sample_spec());
        let actor = &project.files["src/blueprints/player_controller.rs"];
        assert!(actor.contains("pub struct PlayerController"));
        assert!(actor.contains("impl Actor for PlayerController"));
        assert!(actor.contains("logic::begin_play()"));
        // tick body should be a no-op comment because source has no fn tick
        assert!(actor.contains("No tick event in this blueprint"));
    }

    #[test]
    fn enemy_actor_wires_tick() {
        let project = generate_project(&sample_spec());
        let actor = &project.files["src/blueprints/enemy_ai.rs"];
        assert!(actor.contains("logic::tick()"));
    }

    #[test]
    fn mod_file_exports_all_actors() {
        let project = generate_project(&sample_spec());
        let modfile = &project.files["src/blueprints/mod.rs"];
        assert!(modfile.contains("pub mod player_controller"));
        assert!(modfile.contains("pub mod enemy_ai"));
        assert!(modfile.contains("pub use player_controller::PlayerController"));
        assert!(modfile.contains("pub use enemy_ai::EnemyAi"));
        assert!(modfile.contains("fn compiled_class_names"));
        assert!(modfile.contains("fn spawn_compiled_class"));
    }

    #[test]
    fn mod_file_exposes_class_registry_helpers() {
        let project = generate_project(&sample_spec());
        let modfile = &project.files["src/blueprints/mod.rs"];
        assert!(modfile.contains("fn compiled_class_names"));
        assert!(modfile.contains("fn spawn_compiled_class"));
    }

    #[test]
    fn snake_to_pascal() {
        assert_eq!(to_pascal_case("player_controller"), "PlayerController");
        assert_eq!(to_pascal_case("enemy_ai"), "EnemyAi");
        assert_eq!(to_pascal_case("my_cool_actor"), "MyCoolActor");
    }

    #[test]
    fn pascal_to_snake() {
        assert_eq!(to_snake_case("PlayerController"), "player_controller");
        assert_eq!(to_snake_case("EnemyAI"), "enemy_ai");
        assert_eq!(to_snake_case("my_cool_actor"), "my_cool_actor");
    }

    #[test]
    fn write_to_dir() {
        let project = generate_project(&sample_spec());
        let dir = std::env::temp_dir().join("pulsar_project_gen_test");
        project.write_to_dir(&dir).unwrap();

        assert!(dir.join("src/blueprints/mod.rs").exists());
        assert!(dir.join("src/blueprints/player_controller.rs").exists());

        // Cleanup
        std::fs::remove_dir_all(&dir).ok();
    }
}
