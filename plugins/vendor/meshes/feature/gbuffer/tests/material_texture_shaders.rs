//! The meshes' drawer programs compose with every material template (the
//! materials' built-in classes and the demo's custom ones) and validate,
//! natively and with WebGPU's expanded texture bindings.

use helio_core::shader::ShaderSnippet;
use helio_mats::RadiantTemplateRegistry;

fn validate(name: &str, source: &str) {
    let module = naga::front::wgsl::parse_str(source)
        .unwrap_or_else(|error| panic!("{name}: {}", error.emit_to_string(source)));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|error| panic!("{name}: {error:?}"));
}

/// `registry` (with `custom` templates registered too) composed into
/// `program`, every class, both binding tiers.
fn validate_program(
    name: &str,
    program: &str,
    snippets: &[ShaderSnippet],
    mut registry: RadiantTemplateRegistry,
    custom: &[(&str, &str)],
) {
    for (custom_name, source) in custom {
        registry.register_str(custom_name, source.to_string());
    }
    for class in registry.keys() {
        let template = registry.get(class).unwrap();
        let native = template.compose(program, "", 4, snippets);
        validate(&format!("{name} / {}", template.name), &native);
        let expanded = helio_mats::apply_webgpu_material_bindings(&native, 4);
        validate(&format!("{name} / {} (expanded)", template.name), &expanded);
    }
}

#[test]
fn every_material_class_composes_into_every_mesh_program() {
    let rows = [helio_mesh_api::DRAW_ROWS_SNIPPET];
    let lit = [
        helio_mesh_api::DRAW_ROWS_SNIPPET,
        helio_light_api::LIGHT_LIST_SNIPPET,
        helio_light_api::LIGHT_FORWARD_SNIPPET,
    ];
    let opaque_custom = [
        ("opal", helio_mats::templates::OPAL),
        ("glass", helio_mats::templates::GLASS),
        ("water", helio_mats::templates::WATER),
        (
            "iridescent",
            include_str!("../../../examples/shaders/radiant_iridescent.wgsl"),
        ),
    ];
    validate_program(
        "gbuffer",
        helio_pass_gbuffer::GBUFFER_PROGRAM,
        &rows,
        RadiantTemplateRegistry::new(),
        &opaque_custom,
    );
    validate_program(
        "forward",
        include_str!("../../forward/shaders/forward_lit.wgsl"),
        &lit,
        RadiantTemplateRegistry::new(),
        &opaque_custom,
    );
    validate_program(
        "transparent",
        include_str!("../../transparent/shaders/transparent.wgsl"),
        &lit,
        RadiantTemplateRegistry::new_transparent(),
        &[
            (
                "glass_transparent",
                helio_mats::templates::GLASS_TRANSPARENT,
            ),
            (
                "water_transparent",
                helio_mats::templates::WATER_TRANSPARENT,
            ),
        ],
    );
}

#[test]
fn depth_cutouts_validate_for_native_and_expanded_texture_bindings() {
    let material = include_str!("../shaders/depth_material.wgsl");
    for geometry in [include_str!("../../shadow/shaders/shadow.wgsl")] {
        let native = helio_core::shader::resolve_with(
            &format!("{material}\n{geometry}"),
            &helio_mats::MATERIAL_SNIPPETS,
        )
        .replace("256>", "4>");
        for source in [
            native.clone(),
            helio_mats::apply_webgpu_material_bindings(&native, 4),
        ] {
            validate("shadow cut-outs", &source);
        }
    }
}
