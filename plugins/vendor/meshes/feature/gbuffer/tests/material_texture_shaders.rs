#[test]
fn texture_consumers_validate_with_scene_owned_slot_defaults() {
    for (name, source) in [
        ("gbuffer", helio_mats::templates::DEFAULT_PBR),
        (
            "forward",
            include_str!("../../forward/shaders/forward_lit.wgsl"),
        ),
        ("transparent", helio_mats::templates::TRANSPARENT_BASE),
    ] {
        let resolved = helio_core::shader::resolve_with(
            source,
            &[
                helio_mats::PBR_EVAL_SNIPPET,
                helio_mats::SPECULAR_AA_SNIPPET,
                // The lights' row and forward function, which the forward
                // and transparent drawers compose in.
                helio_light_api::LIGHT_LIST_SNIPPET,
                helio_light_api::LIGHT_FORWARD_SNIPPET,
            ],
        );
        let module = naga::front::wgsl::parse_str(&resolved)
            .unwrap_or_else(|error| panic!("{name}: {}", error.emit_to_string(&resolved)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|error| panic!("{name}: {error:?}"));
    }
}

#[test]
fn depth_cutouts_validate_for_native_and_expanded_texture_bindings() {
    let material = include_str!("../shaders/depth_material.wgsl");
    for geometry in [include_str!("../../shadow/shaders/shadow.wgsl")] {
        let native = format!("{material}\n{geometry}");
        for source in [
            native.clone(),
            helio_mats::apply_webgpu_material_bindings(&native, 4),
        ] {
            let module = naga::front::wgsl::parse_str(&source)
                .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{e:?}"));
        }
    }
}
