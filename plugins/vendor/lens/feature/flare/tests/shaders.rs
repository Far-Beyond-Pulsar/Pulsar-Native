//! The flare's response shader validates on its own (it was part of
//! `helio_pass_shadow_matrix`'s production shader check while it lived in
//! the Helio repo).

#[test]
fn the_lens_response_shader_validates() {
    let source = include_str!("../shaders/lens_response.wgsl");
    let module = naga::front::wgsl::parse_str(source)
        .unwrap_or_else(|error| panic!("{}", error.emit_to_string(source)));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|error| panic!("{error:?}"));
}
