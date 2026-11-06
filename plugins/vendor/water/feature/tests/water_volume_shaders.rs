//! Every water shader that reads the volumes' rows validates with the row's
//! one declaration (`WATER_VOLUME_SNIPPET`) and the snippets its pass
//! composes it with, as the passes build them.

use helio_pass_water_sim::pipeline::{WATER_SHORE_SNIPPET, WATER_SKY_SNIPPET};
use helio_pass_water_sim::surface_query::WATER_QUERY_SNIPPET;
use helio_pass_water_sim::WATER_VOLUME_SNIPPET;

fn validate(name: &str, text: &str, snippets: &[helio_core::shader::ShaderSnippet]) {
    let source = helio_core::shader::resolve_with(text, snippets);
    let module = naga::front::wgsl::parse_str(&source)
        .unwrap_or_else(|error| panic!("{name}: {}", error.emit_to_string(&source)));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|error| panic!("{name}: {error:?}"));
}

#[test]
fn the_water_shaders_validate_with_the_volume_row() {
    let atmosphere = helio_frame_api::lighting::sky::ATMOSPHERE_SNIPPET;
    let hiz = helio_pass_hiz::HIZ_SNIPPET;
    let volume_only = [WATER_VOLUME_SNIPPET];
    for (name, text) in [
        ("caustics.wgsl", include_str!("../shaders/caustics.wgsl")),
        (
            "update.frag.wgsl",
            include_str!("../shaders/update.frag.wgsl"),
        ),
        (
            "hitbox.frag.wgsl",
            include_str!("../shaders/hitbox.frag.wgsl"),
        ),
        ("spray.wgsl", include_str!("../shaders/spray.wgsl")),
        (
            "transmission.wgsl",
            include_str!("../shaders/transmission.wgsl"),
        ),
    ] {
        validate(name, text, &volume_only);
    }
    validate(
        "underwater_fog.wgsl",
        include_str!("../shaders/underwater_fog.wgsl"),
        &[
            WATER_VOLUME_SNIPPET,
            WATER_SHORE_SNIPPET,
            atmosphere,
            WATER_SKY_SNIPPET,
        ],
    );
    validate(
        "underwater_shafts.wgsl",
        include_str!("../shaders/underwater_shafts.wgsl"),
        &[WATER_VOLUME_SNIPPET, WATER_SHORE_SNIPPET],
    );
    let surface = include_str!("../shaders/surface.wgsl");
    validate(
        "surface.wgsl",
        surface,
        &[
            WATER_VOLUME_SNIPPET,
            hiz,
            WATER_SHORE_SNIPPET,
            atmosphere,
            WATER_SKY_SNIPPET,
        ],
    );
    validate(
        "surface.wgsl + water_query",
        surface,
        &[
            WATER_VOLUME_SNIPPET,
            hiz,
            WATER_SHORE_SNIPPET,
            atmosphere,
            WATER_SKY_SNIPPET,
            WATER_QUERY_SNIPPET,
        ],
    );
}
