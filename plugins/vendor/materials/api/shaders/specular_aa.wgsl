//! Specular anti-aliasing: roughness widened by the normals a pixel covers
//! but shades as one, so detail smaller than a pixel shines as the rough
//! surface it makes up instead of sparkling as the jittered samples land on
//! one bump or another. Roughness here is perceptual (GGX alpha is its
//! square), as Helio's materials store it.

// Perceptual `roughness` widened by the spread of the normals under a normal
// map texel: `mean_length` is the length of their mean (the map's alpha:
// `helio_mats::texture_mips`), `strength` the scale on the map's slopes.
// The spread is taken as a von Mises-Fisher lobe of concentration kappa
// (Toksvig 2005; Neubelt & Pettineo 2013), whose slope variance, 1 / kappa
// on each axis, adds 2 / kappa to the GGX alpha squared. A map without the
// length in alpha stores 1 there and changes nothing.
fn normal_map_roughness(roughness: f32, mean_length: f32, strength: f32) -> f32 {
    // Within an 8-bit step of 1 is 1: filtering a stored 1 may round below.
    let r = select(clamp(mean_length, 0.0, 1.0), 1.0, mean_length > 0.998);
    let spread = 2.0 * (1.0 - r * r) / max(3.0 * r - r * r * r, 1e-4);
    let a = roughness * roughness;
    return sqrt(sqrt(min(a * a + strength * strength * spread, 1.0)));
}

// Perceptual `roughness` widened by how far the geometric normal `n` turns
// across a pixel (Kaplanyan et al. 2016, with Filament's variance and
// clamp): curved and finely tessellated surfaces stop sparkling. Screen
// derivatives: fragment stage, uniform control flow.
fn geometric_roughness(roughness: f32, n: vec3<f32>) -> f32 {
    let du = dpdx(n);
    let dv = dpdy(n);
    let kernel = min(2.0 * 0.15 * (dot(du, du) + dot(dv, dv)), 0.2);
    let a = roughness * roughness;
    return sqrt(sqrt(min(a * a + kernel, 1.0)));
}

// A mesh's perceptual `roughness` after both: its normal map's spread
// (`mean_length`, `strength`: `normal_map_roughness`) and its geometric
// normal `n`'s turn across the pixel.
fn specular_aa_roughness(roughness: f32, mean_length: f32, strength: f32, n: vec3<f32>) -> f32 {
    return geometric_roughness(normal_map_roughness(roughness, mean_length, strength), n);
}
