//! The flare's response shader, with the prelude and the lights' snippets
//! it opts into resolved, validates.

#[test]
fn the_lens_response_shader_validates() {
    let source = helio_core::shader::resolve_with(
        include_str!("../shaders/lens_response.wgsl"),
        &[
            helio_light_api::LIGHT_LIST_SNIPPET,
            helio_light_api::SHADOW_ATLAS_SNIPPET,
            helio_light_api::SHADOW_LOOKUP_SNIPPET,
        ],
    );
    let source = source.as_ref();
    let module = naga::front::wgsl::parse_str(source)
        .unwrap_or_else(|error| panic!("{}", error.emit_to_string(source)));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|error| panic!("{error:?}"));
}
