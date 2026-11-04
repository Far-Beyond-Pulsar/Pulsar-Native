//! Every sky shader, with the snippets its passes build it with, parses and
//! validates (naga), so a WGSL mistake fails here rather than on a device.
use helio_frame_api::lighting::sun_transmittance::SNIPPET as SUN_TRANSMITTANCE_SNIPPET;
use helio_pass_sky::atmosphere::{ATMOSPHERE_SKY_TABLE_SNIPPET, ATMOSPHERE_SNIPPET};
use helio_pass_sky::clouds::CLOUDS_SNIPPET;

fn validate(label: &str, source: &str) {
    let module = naga::front::wgsl::parse_str(source)
        .unwrap_or_else(|e| panic!("{label}: {}", e.emit_to_string(source)));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|e| panic!("{label}: {}", e.emit_to_string(source)));
}

#[test]
fn sky_shaders_validate() {
    let snippets = [
        ATMOSPHERE_SNIPPET,
        CLOUDS_SNIPPET,
        ATMOSPHERE_SKY_TABLE_SNIPPET,
        SUN_TRANSMITTANCE_SNIPPET,
    ];
    for (label, source) in [
        (
            "atmosphere_luts",
            include_str!("../src/shaders/atmosphere_luts.wgsl"),
        ),
        (
            "atmosphere_composite",
            include_str!("../src/shaders/atmosphere_composite.wgsl"),
        ),
        (
            "cloud_noise",
            include_str!("../src/shaders/cloud_noise.wgsl"),
        ),
        (
            "cloud_planet_weather",
            include_str!("../src/shaders/cloud_planet_weather.wgsl"),
        ),
        (
            "cloud_shadow_map",
            include_str!("../src/shaders/cloud_shadow_map.wgsl"),
        ),
        (
            "cloud_trace",
            include_str!("../src/shaders/cloud_trace.wgsl"),
        ),
    ] {
        validate(label, &helio_core::shader::resolve_with(source, &snippets));
    }
}
