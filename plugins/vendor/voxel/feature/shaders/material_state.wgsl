// Display state a terrain program's material queries may read or report
// (`ground_material`); private to each invocation.

// Shade may low-pass procedural material detail for the current pixel.
// Other entry points and canonical material queries retain zero footprint.
var<private> material_footprint: f32 = 0.0;
// Natural surface coating is shade-only; canonical queries keep their IDs.
var<private> material_weathered_skin: bool = false;
// Radial height range of the material pixel, in metres; zero outside shade.
var<private> material_radial_span: f32 = 0.0;
// Display-only appearance a terrain program may report for the shaded cell
// (canonical IDs never change). A coverage between two materials (negative:
// none), a mix of four (negative first weight: none), and the material whose
// single-voxel flecks the filtered colour averages (air: none).
var<private> material_coverage: f32 = -1.0;
var<private> material_coverage_ids: vec2<u32> = vec2<u32>(0u);
var<private> material_mix: vec4<f32> = vec4<f32>(-1.0, 0.0, 0.0, 0.0);
var<private> material_mix_ids: vec4<u32> = vec4<u32>(0u);
var<private> material_fleck_base: u32 = 0u;

