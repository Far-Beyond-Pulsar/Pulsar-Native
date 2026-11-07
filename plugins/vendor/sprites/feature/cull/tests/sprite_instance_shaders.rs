//! Every sprite shader that reads the instance rows validates with the row's
//! one declaration (`SPRITE_INSTANCE_SNIPPET`), as the passes build them:
//! the cull here, the batch's draw shaders and the simulation beside it.

#[test]
fn the_sprite_shaders_validate_with_the_instance_row() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    for path in [
        "cull/shaders/sprite_cull.wgsl",
        "batch/shaders/sprite.wgsl",
        "batch/shaders/sprite_scene.wgsl",
        "simulate/shaders/sprite_simulate.wgsl",
    ] {
        let text = std::fs::read_to_string(root.join(path)).unwrap();
        let source = helio_core::shader::resolve_with(
            &text,
            &[helio_pass_sprite_cull::SPRITE_INSTANCE_SNIPPET],
        );
        let module = naga::front::wgsl::parse_str(&source)
            .unwrap_or_else(|error| panic!("{path}: {}", error.emit_to_string(&source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|error| panic!("{path}: {error:?}"));
    }
}
