//! Portal culling's WGSL validates with the meshes' rows it reads
//! (`helio_mesh_api::DRAW_ROWS_SNIPPET`) and the portals' own row
//! (`PORTAL_VIEW_SNIPPET`), as the pass compiles it.

#[test]
fn the_portal_cull_shader_validates_with_the_mesh_rows() {
    let source = helio_core::shader::resolve_with(
        include_str!("../shaders/portal_cull.wgsl"),
        &[
            helio_mesh_api::DRAW_ROWS_SNIPPET,
            helio_pass_portal_cull::PORTAL_VIEW_SNIPPET,
        ],
    );
    let module = naga::front::wgsl::parse_str(&source)
        .unwrap_or_else(|error| panic!("{}", error.emit_to_string(&source)));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|error| panic!("{error:?}"));
}
