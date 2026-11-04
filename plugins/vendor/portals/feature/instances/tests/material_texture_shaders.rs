//! The portal draw's shader validates with the scene materials' snippets, as
//! the G-buffer's own drawers do (`helio_pass_gbuffer`'s
//! `material_texture_shaders` test).

#[test]
fn the_portal_draw_validates_with_scene_owned_slot_defaults() {
    let source = include_str!("../shaders/gbuffer_portal.wgsl");
    let resolved = helio_core::shader::resolve_with(
        source,
        &[
            helio_mats::PBR_EVAL_SNIPPET,
            helio_mats::SPECULAR_AA_SNIPPET,
            helio_mesh_api::DRAW_ROWS_SNIPPET,
        ],
    );
    let module = naga::front::wgsl::parse_str(&resolved)
        .unwrap_or_else(|error| panic!("{}", error.emit_to_string(&resolved)));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|error| panic!("{error:?}"));
}
