//! The portal draw's program composes with every material class and
//! validates, natively and with WebGPU's expanded texture bindings, as the
//! meshes' drawers do (`helio_pass_gbuffer`'s `material_texture_shaders`
//! test).

#[test]
fn the_portal_draw_validates_with_every_material_class() {
    let program = include_str!("../shaders/gbuffer_portal.wgsl");
    let registry = helio_mats::RadiantTemplateRegistry::new();
    for class in registry.keys() {
        let template = registry.get(class).unwrap();
        let native = template.compose(
            program,
            "",
            4,
            &helio_pass_portal_instances::PORTAL_PROGRAM_SNIPPETS,
        );
        for source in [
            native.clone(),
            helio_mats::apply_webgpu_material_bindings(&native, 4),
        ] {
            let module = naga::front::wgsl::parse_str(&source).unwrap_or_else(|error| {
                panic!("{}: {}", template.name, error.emit_to_string(&source))
            });
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap_or_else(|error| panic!("{}: {error:?}", template.name));
        }
    }
}
