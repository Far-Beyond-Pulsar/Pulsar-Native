//! The voxel shaders parse and validate: the files that stand alone as they
//! are, and those this crate composes (the terrain program, the world
//! helpers, row strides) exactly as the pass compiles them.

use naga::valid::{Capabilities, ValidationFlags, Validator};

/// The snippets voxel shaders may include, from the crates that own them.
const SNIPPETS: &[helio_core::shader::ShaderSnippet] = &[
    helio_mats::PBR_EVAL_SNIPPET,
    helio_mats::SPECULAR_AA_SNIPPET,
    helio_gi_api::GI_COMMON_SNIPPET,
    helio_gi_api::GI_SAMPLE_SNIPPET,
    helio_pass_shadow_matrix::SUN_OCCLUSION_SNIPPET,
];

#[test]
fn composed_shaders_parse_and_validate() {
    let sources = helio_pass_voxel_planet::engine::validation_sources();
    let failures: Vec<String> = sources
        .iter()
        .filter_map(|(name, source)| {
            let source = helio_core::shader::resolve_with(source, SNIPPETS).into_owned();
            let module = match naga::front::wgsl::parse_str(&source) {
                Ok(module) => module,
                Err(e) => return Some(format!("{name}:\n{}", e.emit_to_string(&source))),
            };
            Validator::new(ValidationFlags::all(), Capabilities::all())
                .validate(&module)
                .err()
                .map(|e| format!("{name}: {e:?}"))
        })
        .collect();
    assert!(sources.len() >= 4, "{} composed shaders", sources.len());
    assert!(
        failures.is_empty(),
        "{} composed shaders failed:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// Assembled by `chunks::shader_sources` with a terrain program generated
/// in Rust: validated composed, above.
const COMPOSED: &[&str] = &["chunk_generate.wgsl", "chunk_draw.wgsl"];

#[test]
fn shader_files_parse_and_validate() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders");
    let mut checked = 0;
    let mut failures = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        let path = entry.path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.extension().is_none_or(|e| e != "wgsl") || COMPOSED.contains(&name.as_str()) {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap();
        // A fragment (no entry point) is joined into a host shader first.
        if !["@vertex", "@fragment", "@compute"]
            .iter()
            .any(|stage| source.contains(stage))
        {
            continue;
        }
        checked += 1;
        let source = helio_core::shader::resolve_with(&source, SNIPPETS).into_owned();
        match naga::front::wgsl::parse_str(&source) {
            Ok(module) => {
                if let Err(e) = Validator::new(ValidationFlags::all(), Capabilities::all())
                    .validate(&module)
                {
                    failures.push(format!("{name}: {e:?}"));
                }
            }
            Err(e) => failures.push(format!("{name}:\n{}", e.emit_to_string(&source))),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    eprintln!("{checked} voxel shader files validated");
}
