//! Regression coverage for the sky texture layer's place in the composite
//! shader, and for the lights the sky keeps honouring.
use helio_pass_sky::sky_texture::{
    build_image, custom_path, custom_source, decode, direction_to_uv, presets_available, rotation,
    source, MAX_WIDTH,
};
use std::path::{Path, PathBuf};

const COMPOSITE: &str = include_str!("../src/shaders/atmosphere_composite.wgsl");

fn body_of<'a>(source: &'a str, function: &str) -> &'a str {
    let start = source
        .find(function)
        .unwrap_or_else(|| panic!("{function} exists"));
    let rest = &source[start..];
    let end = rest[1..].find("\nfn ").map(|i| i + 1).unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn composite_layers_the_panorama_under_the_procedural_sky() {
    let texture = COMPOSITE.find("sky_texture_radiance(d, pixel_angle)").unwrap();
    let disks = COMPOSITE
        .find("radiance += directional_disk_radiance(d, through)")
        .unwrap();
    let stars = COMPOSITE.find("radiance += night_radiance(d)").unwrap();
    assert!(
        texture < disks && disks < stars,
        "panorama, then emitter disks, then stars"
    );
    assert!(
        COMPOSITE.contains("radiance += sky_texture_radiance"),
        "additive: it never replaces the atmosphere"
    );
    // The panorama and the procedural stars are separate switches.
    assert!(!body_of(COMPOSITE, "fn sky_darkness").contains("night_sky_enabled"));
    assert!(!body_of(COMPOSITE, "fn sky_texture_radiance").contains("night_sky_enabled"));
    // It uses the wrapping sampler, not the clamped LUT sampler.
    assert!(body_of(COMPOSITE, "fn sky_texture_radiance").contains("sky_sampler"));
}

#[test]
fn composite_keeps_every_directional_light() {
    for function in [
        "fn march_from_space",
        "fn sky_is_night",
        "fn directional_disk_radiance",
        "fn emitter_disk_radiance",
        "fn sky_darkness",
    ] {
        assert!(
            body_of(COMPOSITE, function).contains("directional_light_count.x"),
            "{function} loops over all the lights"
        );
    }
    assert!(
        !COMPOSITE.contains("directional_lights[0]"),
        "no first-light-only lighting"
    );
}

#[test]
fn the_wgsl_orientation_matches_the_rust_reference() {
    let wgsl = body_of(COMPOSITE, "fn sky_texture_rotation");
    assert!(wgsl.contains("return ry * rx * rz;"));
    let sampled = body_of(COMPOSITE, "fn sky_texture_radiance");
    assert!(sampled.contains("transpose(sky_texture_rotation()) * d"));
    assert!(sampled.contains("atan2(t.x, -t.z)"));
    assert!(sampled.contains("0.5 - asin(clamp(t.y, -1.0, 1.0))"));
}

fn close(a: [f32; 2], b: [f32; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4
}

#[test]
fn equirect_mapping_covers_the_whole_sky() {
    let id = rotation(0.0, 0.0, 0.0);
    assert!(close(direction_to_uv(id, [0.0, 0.0, -1.0]), [0.5, 0.5]));
    assert!(close(direction_to_uv(id, [0.0, 1.0, 0.0]), [0.5, 0.0]));
    assert!(close(direction_to_uv(id, [0.0, -1.0, 0.0]), [0.5, 1.0]));
    assert!(close(direction_to_uv(id, [1.0, 0.0, 0.0]), [0.75, 0.5]));
    assert!(close(direction_to_uv(id, [-1.0, 0.0, 0.0]), [0.25, 0.5]));
}

#[test]
fn yaw_turns_the_panorama_about_the_vertical_axis() {
    let d = [0.0, 0.3, -1.0];
    let before = direction_to_uv(rotation(0.0, 0.0, 0.0), d);
    let after = direction_to_uv(rotation(std::f32::consts::FRAC_PI_2, 0.0, 0.0), d);
    assert!((before[1] - after[1]).abs() < 1e-5, "height is unchanged");
    let shift = (after[0] - before[0]).rem_euclid(1.0);
    assert!(
        (shift - 0.25).abs() < 1e-4 || (shift - 0.75).abs() < 1e-4,
        "a quarter turn moves u by a quarter: {shift}"
    );
}

#[test]
fn rotation_is_orthonormal() {
    let r = rotation(0.4, -0.7, 1.1);
    for a in 0..3 {
        for b in 0..3 {
            let dot: f32 = (0..3).map(|k| r[a][k] * r[b][k]).sum();
            assert!((dot - if a == b { 1.0 } else { 0.0 }).abs() < 1e-5);
        }
    }
}

#[test]
fn custom_files_get_stable_distinct_codes() {
    let a = custom_source(Path::new("sky_a.exr"));
    let b = custom_source(Path::new("sky_b.hdr"));
    assert_ne!(a, b);
    assert_eq!(a, custom_source(Path::new("sky_a.exr")));
    assert_ne!(a & source::CUSTOM, 0);
    assert_eq!(custom_path(a).unwrap(), PathBuf::from("sky_a.exr"));
    assert_eq!(custom_source(Path::new("")), source::NONE);
    assert!(custom_path(source::GALAXY).is_none());
}

#[test]
fn size_cap_and_mip_chain() {
    let image = build_image(2 * MAX_WIDTH, MAX_WIDTH, vec![0.5; (2 * MAX_WIDTH * MAX_WIDTH * 3) as usize]);
    assert_eq!((image.width, image.height), (MAX_WIDTH, MAX_WIDTH / 2));
    assert_eq!(image.mips.len(), 13);
    assert_eq!(image.mips[0].len() as u32, MAX_WIDTH * (MAX_WIDTH / 2) * 4);
    assert_eq!(half::f16::from_bits(image.mips[12][0]).to_f32(), 0.5);
}

#[test]
fn builtin_presets_decode_and_the_combination_is_brighter() {
    if !presets_available() {
        eprintln!("built-in sky presets were not embedded in this build; skipping");
        return;
    }
    let total = |image: &helio_pass_sky::sky_texture::SkyImage| -> f64 {
        image
            .mips
            .last()
            .unwrap()
            .chunks(4)
            .map(|p| half::f16::from_bits(p[0]).to_f32() as f64)
            .sum()
    };
    let galaxy = decode(source::GALAXY).unwrap();
    let nebula = decode(source::NEBULA).unwrap();
    let both = decode(source::GALAXY_NEBULA).unwrap();
    assert_eq!((galaxy.width, galaxy.height), (4096, 2048));
    assert_eq!((both.width, both.height), (4096, 2048));
    assert!(total(&nebula) > 0.0);
    assert!(total(&both) > total(&galaxy), "the nebula is added to the galaxy");
}

#[test]
fn a_missing_custom_file_is_an_error_not_a_panic() {
    assert!(decode(custom_source(Path::new("definitely/not/here.hdr"))).is_err());
}
