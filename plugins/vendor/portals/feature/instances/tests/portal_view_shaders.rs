//! The opening mask and the editor overlay validate with the portals' row
//! (`helio_pass_portal_cull::PORTAL_VIEW_SNIPPET`), as their passes compile
//! them. (The duplicate draw validates per material class in
//! `material_texture_shaders`.)

#[test]
fn the_mask_and_overlay_shaders_validate_with_the_portal_row() {
    for (name, text) in [
        (
            "portal_mask.wgsl",
            include_str!("../shaders/portal_mask.wgsl"),
        ),
        (
            "portal_editor_overlay.wgsl",
            include_str!("../shaders/portal_editor_overlay.wgsl"),
        ),
    ] {
        let source =
            helio_core::shader::resolve_with(text, &[helio_pass_portal_cull::PORTAL_VIEW_SNIPPET]);
        let module = naga::front::wgsl::parse_str(&source)
            .unwrap_or_else(|error| panic!("{name}: {}", error.emit_to_string(&source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|error| panic!("{name}: {error:?}"));
    }
}
