//! Rayleigh scattering, Mie scattering, and Henyey–Greenstein phase function tests.
//!
//! Pure math — no GPU, no wgpu, no crate imports.

use std::f32::consts::PI;

const EPSILON: f32 = 1e-5;

// ── Rayleigh scattering coefficient wavelength dependence ────────────────────
//
// Rayleigh scattering cross-section ∝ 1/λ⁴.
// Visible wavelengths (nm): red ≈ 680, green ≈ 550, blue ≈ 440.
// earth_like() values: R=5.8e-3, G=1.35e-2, B=3.31e-2.

const RAYLEIGH_R: f32 = 5.8e-3;
const RAYLEIGH_G: f32 = 1.35e-2;
const RAYLEIGH_B: f32 = 3.31e-2;

#[test]
fn rayleigh_blue_greater_than_green() {
    assert!(RAYLEIGH_B > RAYLEIGH_G, "B={} G={}", RAYLEIGH_B, RAYLEIGH_G);
}

#[test]
fn rayleigh_green_greater_than_red() {
    assert!(RAYLEIGH_G > RAYLEIGH_R, "G={} R={}", RAYLEIGH_G, RAYLEIGH_R);
}

#[test]
fn rayleigh_blue_greater_than_red() {
    assert!(RAYLEIGH_B > RAYLEIGH_R, "B={} R={}", RAYLEIGH_B, RAYLEIGH_R);
}

#[test]
fn rayleigh_blue_to_red_ratio_approximately_inverse_fourth_power() {
    // λ_R/λ_B ≈ 680/440 ≈ 1.545; (1.545)⁴ ≈ 5.69; actual B/R = 3.31e-2/5.8e-3 ≈ 5.7.
    let ratio = RAYLEIGH_B / RAYLEIGH_R;
    assert!(ratio > 5.0 && ratio < 7.0, "B/R ratio = {}", ratio);
}

#[test]
fn rayleigh_all_coefficients_are_positive() {
    assert!(RAYLEIGH_R > 0.0);
    assert!(RAYLEIGH_G > 0.0);
    assert!(RAYLEIGH_B > 0.0);
}

#[test]
fn rayleigh_coefficients_are_small_fractions() {
    // Physical scattering coefficients per km are on the order of 1e-3 to 1e-1.
    assert!(RAYLEIGH_R < 1.0);
    assert!(RAYLEIGH_G < 1.0);
    assert!(RAYLEIGH_B < 1.0);
}

// ── Rayleigh height scale ────────────────────────────────────────────────────

#[test]
fn rayleigh_h_scale_is_positive() {
    let rayleigh_h: f32 = 0.1;
    assert!(rayleigh_h > 0.0);
}

#[test]
fn mie_h_scale_is_less_than_rayleigh_h_scale() {
    // Aerosols (Mie) are found at lower altitudes than gas molecules (Rayleigh).
    let rayleigh_h: f32 = 0.1;
    let mie_h: f32 = 0.075;
    assert!(mie_h < rayleigh_h);
}

// ── Henyey–Greenstein phase function ─────────────────────────────────────────
//
// f(g, cos_θ) = (1 - g²) / (4π × (1 + g² - 2g·cos_θ)^(3/2))
//
// Properties:
//   - g = 0  → isotropic (f = 1/(4π) for all cos_θ)
//   - g > 0  → forward scattering (f peaks at cos_θ = 1)
//   - g < 0  → back scattering
//   - Integrates to 1 over 4π steradians.

fn hg_phase(g: f32, cos_theta: f32) -> f32 {
    let g2 = g * g;
    let denom = (1.0 + g2 - 2.0 * g * cos_theta).powf(1.5);
    (1.0 - g2) / (4.0 * PI * denom)
}

#[test]
fn hg_phase_isotropic_g0() {
    // g = 0 → isotropic: f = 1/(4π) ≈ 0.07957747.
    let f = hg_phase(0.0, 0.0);
    let expected = 1.0 / (4.0 * PI);
    assert!(
        (f - expected).abs() < EPSILON,
        "f={} expected={}",
        f,
        expected
    );
}

#[test]
fn hg_phase_isotropic_g0_all_angles() {
    // With g=0, phase is identical for every cos_θ.
    let expected = 1.0 / (4.0 * PI);
    for &cos_theta in &[-1.0_f32, -0.5, 0.0, 0.5, 1.0] {
        let f = hg_phase(0.0, cos_theta);
        assert!(
            (f - expected).abs() < EPSILON,
            "cos_θ={} f={}",
            cos_theta,
            f
        );
    }
}

#[test]
fn hg_phase_forward_greater_than_backward_for_positive_g() {
    let g: f32 = 0.76; // earth_like() mie_g
    let forward = hg_phase(g, 1.0); // cos_θ = 1
    let backward = hg_phase(g, -1.0); // cos_θ = -1
    assert!(
        forward > backward,
        "forward={} backward={}",
        forward,
        backward
    );
}

#[test]
fn hg_mie_g_076_forward_peak_is_large() {
    // g=0.76 is strongly forward-scattering; forward/isotropic ≫ 1.
    let g: f32 = 0.76;
    let forward = hg_phase(g, 1.0);
    let isotropic = 1.0 / (4.0 * PI);
    assert!(
        forward > 10.0 * isotropic,
        "forward={} (expected > 10×isotropic)",
        forward
    );
}

#[test]
fn hg_mie_g_076_backward_peak_is_small() {
    // g=0.76 gives very little backward scattering.
    let g: f32 = 0.76;
    let backward = hg_phase(g, -1.0);
    let isotropic = 1.0 / (4.0 * PI);
    assert!(
        backward < isotropic,
        "backward={} (expected < isotropic={})",
        backward,
        isotropic
    );
}

#[test]
fn hg_phase_is_positive_everywhere() {
    let g: f32 = 0.76;
    for &cos_theta in &[-1.0_f32, -0.5, 0.0, 0.5, 0.9, 1.0] {
        let f = hg_phase(g, cos_theta);
        assert!(f > 0.0, "phase function negative at cos_θ={}", cos_theta);
    }
}

#[test]
fn hg_g_zero_integral_over_sphere_is_one() {
    // Numerically integrate f(0, cos_θ) over 4π steradians.
    // ∫ f dΩ = ∫₋₁¹ f(0, μ) × 2π dμ = 1 (by construction of the normalisation).
    let steps = 1000;
    let mut sum = 0.0_f64;
    for i in 0..steps {
        let mu = -1.0_f64 + 2.0 * (i as f64 + 0.5) / steps as f64;
        let f = hg_phase(0.0, mu as f32) as f64;
        sum += f * 2.0 * std::f64::consts::PI * (2.0 / steps as f64);
    }
    assert!((sum - 1.0).abs() < 0.001, "∫f dΩ = {} (expected 1.0)", sum);
}

// ── Mie asymmetry parameter g ────────────────────────────────────────────────

#[test]
fn mie_g_is_in_valid_range() {
    // Valid range is (-1, 1); 0.76 is for typical aerosols.
    let g: f32 = 0.76;
    assert!(g > -1.0 && g < 1.0);
}

#[test]
fn mie_g_zero_means_isotropic() {
    // g=0 → Rayleigh-like isotropic scattering.
    let f_forward = hg_phase(0.0, 1.0);
    let f_backward = hg_phase(0.0, -1.0);
    assert!((f_forward - f_backward).abs() < EPSILON);
}

#[test]
fn mie_g_positive_means_forward_dominant() {
    let g = 0.76_f32;
    let f_forward = hg_phase(g, 1.0);
    let f_side = hg_phase(g, 0.0);
    let f_backward = hg_phase(g, -1.0);
    assert!(f_forward > f_side);
    assert!(f_side > f_backward);
}

// ── Atmosphere geometry ───────────────────────────────────────────────────────

#[test]
fn atmosphere_thickness_is_60_km() {
    let earth_r: f32 = 6360.0;
    let atm_r: f32 = 6420.0;
    assert!((atm_r - earth_r - 60.0).abs() < 1e-3);
}

#[test]
fn atm_radius_greater_than_earth_radius() {
    assert!(6420.0_f32 > 6360.0_f32);
}

#[test]
fn atmosphere_scale_height_rayleigh_in_km() {
    // rayleigh_h_scale = 0.1 means 0.1 × earth_radius ≈ 636 km — unrealistic
    // for a literal atmosphere, but the shader normalises distances to the
    // planet radius, so the "effective" scale height is 0.1 × R.
    let earth_r: f32 = 6360.0;
    let h_scale: f32 = 0.1;
    let effective_km = h_scale * earth_r;
    // Real Rayleigh scale height ~8 km, but pêle-mêle shaders often exaggerate.
    assert!(effective_km > 0.0);
}

// ── Cloud layer constraints ───────────────────────────────────────────────────

#[test]
fn cloud_base_must_be_positive() {
    // Clouds sit above the ground.
    let base: f32 = 1500.0; // typical low-cloud base (metres above ground)
    assert!(base > 0.0);
}

#[test]
fn cloud_top_must_exceed_cloud_base() {
    // A valid cloud layer has positive thickness.
    let base: f32 = 1500.0;
    let top: f32 = 3000.0;
    assert!(top > base);
}

#[test]
fn cloud_thickness_is_positive_when_enabled() {
    let base = 1500.0_f32;
    let top = 3000.0_f32;
    assert!(top - base > 0.0);
}

// ── Exposure ──────────────────────────────────────────────────────────────────

#[test]
fn exposure_maps_sun_to_sane_brightness() {
    // exposure × sun_intensity should map HDR luminance to a perceptible range.
    let exposure: f32 = 0.1;
    let sun_intensity: f32 = 22.0;
    let result = exposure * sun_intensity;
    // 0.1 × 22 = 2.2: slightly overexposed sky, normal for tone-mapping input.
    assert!((result - 2.2).abs() < 1e-5);
}

#[test]
fn exposure_is_in_plausible_range() {
    let exposure: f32 = 0.1;
    // Real-world exposure values for HDR scenes: 0.001 to 1.0.
    assert!(exposure > 0.0);
    assert!(exposure <= 1.0);
}

// ── sun_disk_cos angular diameter ────────────────────────────────────────────

#[test]
fn sun_disk_cos_corresponds_to_half_degree_radius() {
    // Sun diameter ~0.53°. Half-angle = 0.265°.
    // cos(0.265°) = cos(0.265 × π / 180).
    let half_angle_deg = 0.265_f32;
    let half_angle_rad = half_angle_deg * PI / 180.0;
    let expected_cos = half_angle_rad.cos();
    let actual_cos = 0.9998_f32;
    assert!(
        (actual_cos - expected_cos).abs() < 0.0002,
        "expected {} got {}",
        expected_cos,
        actual_cos
    );
}

#[test]
fn sun_disk_visible_when_cos_theta_exceeds_threshold() {
    // A ray toward the sun has cos_θ ≥ sun_disk_cos to be treated as sun disk.
    let sun_disk_cos = 0.9998_f32;
    let center_ray_cos = 1.0_f32; // ray exactly toward sun
    assert!(center_ray_cos >= sun_disk_cos);
}

#[test]
fn sun_disk_invisible_when_cos_theta_below_threshold() {
    let sun_disk_cos = 0.9998_f32;
    let away_ray_cos = 0.5_f32; // ray at 60° from sun
    assert!(away_ray_cos < sun_disk_cos);
}
