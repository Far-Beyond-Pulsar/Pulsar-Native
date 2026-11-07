//! Phase 5 on the GPU: baked prescriptions and user elements, through the
//! production WGSL.
//!
//! The expectations come from the bakes themselves (`helio-lens-optics` is the
//! CPU side of every table the shader reads) and from closed forms where the
//! bake has one (the ideal thin lens is the identity with `cos^4` falloff).
//! A scene sits on the focal plane (3 m), so the defocus stages change nothing
//! and every difference is the ray map, the transmission or the point spread.

#![allow(clippy::needless_range_loop)]

use super::*;
use crate::ray_map::spectral_samples;
use helio_lens_optics::{examples, BakeConfig, LensBake};
use std::sync::OnceLock;

/// Bakes cheap enough for a debug build, with footprint fields dense enough
/// to interpolate between.
fn config() -> BakeConfig {
    BakeConfig {
        field_samples: 17,
        footprint_fields: vec![0.0, 0.5, 0.75, 1.0],
        footprint_resolution: 32,
        footprint_supersample: 2,
        ..BakeConfig::coarse()
    }
}

fn thin_lens() -> &'static LensBake {
    static BAKE: OnceLock<LensBake> = OnceLock::new();
    BAKE.get_or_init(|| LensBake::bake(&examples::thin_lens(), &config()).expect("thin lens bakes"))
}

fn double_gauss() -> &'static LensBake {
    static BAKE: OnceLock<LensBake> = OnceLock::new();
    BAKE.get_or_init(|| {
        LensBake::bake(&examples::double_gauss(), &config()).expect("double Gauss bakes")
    })
}

fn cooke_triplet() -> &'static LensBake {
    static BAKE: OnceLock<LensBake> = OnceLock::new();
    BAKE.get_or_init(|| {
        LensBake::bake(&examples::cooke_triplet(), &config()).expect("Cooke triplet bakes")
    })
}

fn install(rig: &mut Rig, bakes: &[&LensBake]) {
    rig.prescriptions = Some(PrescriptionBank::new(&rig.device, bakes));
}

fn full_frame_prescription() -> [LensElementRow; 1] {
    [LensElementRow::prescription(0, LensRegion::FullFrame)]
}

/// A lens of `n` elements with the f-number `f_number`, focused on the 3 m
/// plane. Its focal length is overridden by a full-frame prescription.
fn lens_with(n: usize, f_number: f32) -> LensRow {
    let mut lens = lens();
    lens.element_count = n as u32;
    lens.f_number = f_number;
    lens
}

fn record_with(rig: &mut Rig, scene: &Scene, lens: LensRow, elements: &[LensElementRow]) -> Frame {
    rig.record(
        scene,
        Some(&[lens]),
        (!elements.is_empty()).then_some(elements),
    )
}

fn flat_white() -> Scene {
    Scene::new(|_, _| [1.0; 3], |_, _| 3.0)
}

/// Sensor position of pixel index `i` in image heights (centres at integers).
fn q_of(i: f32) -> f32 {
    (i + 0.5) / SIZE as f32 - 0.5
}

/// Pixel index of image-height position `q`.
fn index_of(q: f32) -> f32 {
    (q + 0.5) * SIZE as f32 - 0.5
}

// ── CPU readings of the bake, mirroring the shader's interpolation ───────────

/// `tan(theta)` at normalised radius `r` and wavelength `lambda`: linear in the
/// field (continued past the corner), linear between wavelengths, clamped.
fn bake_tan(bake: &LensBake, r: f32, lambda: f32) -> f32 {
    let ray = &bake.ray_map;
    let p = r.max(0.0) * (ray.n_field - 1) as f32;
    let i0 = (p.floor() as usize).min(ray.n_field - 2);
    let f = p - i0 as f32;
    let row = |w: usize| ray.tan_theta(w, i0) * (1.0 - f) + ray.tan_theta(w, i0 + 1) * f;
    let waves = &ray.wavelengths_nm;
    if waves.len() == 1 {
        return row(0);
    }
    let lower = (0..waves.len() - 1)
        .rev()
        .find(|&k| waves[k] <= lambda)
        .unwrap_or(0);
    let t = ((lambda - waves[lower]) / (waves[lower + 1] - waves[lower])).clamp(0.0, 1.0);
    row(lower) * (1.0 - t) + row(lower + 1) * t
}

/// Relative illumination at `r` for f-number `n`: linear in the field and in
/// log2 of the f-number.
fn bake_vignette(bake: &LensBake, r: f32, n: f32) -> f32 {
    let vig = &bake.vignetting;
    let p = r.clamp(0.0, 1.0) * (vig.n_field - 1) as f32;
    let i0 = (p.floor() as usize).min(vig.n_field - 2);
    let f = p - i0 as f32;
    let row = |j: usize| vig.at(j, i0) * (1.0 - f) + vig.at(j, i0 + 1) * f;
    if vig.n_fnum == 1 {
        return row(0);
    }
    let lower = (0..vig.n_fnum - 1)
        .rev()
        .find(|&k| vig.f_numbers[k] <= n)
        .unwrap_or(0);
    let (n0, n1) = (vig.f_numbers[lower].log2(), vig.f_numbers[lower + 1].log2());
    let t = ((n.log2() - n0) / (n1 - n0)).clamp(0.0, 1.0);
    row(lower) * (1.0 - t) + row(lower + 1) * t
}

/// A rig's frame is square, so the sensor is `sensor_w` mm each way.
struct Geometry {
    focal: f32,
    sensor_w: f32,
    half_diagonal: f32,
}

impl Geometry {
    fn new(bake: &LensBake, rig: &Rig) -> Self {
        let resolved = rig.resolved();
        Self {
            focal: resolved.focal_length_mm,
            sensor_w: resolved.sensor_width_mm,
            half_diagonal: bake.ray_map.sensor_half_diagonal_mm,
        }
    }

    /// Sensor position of pixel `(x, y)` in mm.
    fn mm(&self, x: f32, y: f32) -> [f32; 2] {
        [q_of(x) * self.sensor_w, q_of(y) * self.sensor_w]
    }

    /// Normalised radius of pixel `(x, y)`.
    fn radius(&self, x: f32, y: f32) -> f32 {
        let mm = self.mm(x, y);
        mm[0].hypot(mm[1]) / self.half_diagonal
    }

    /// Scene pixel index the sensor pixel `(x, y)` looks at for `lambda`.
    fn scene_pixel(&self, bake: &LensBake, x: f32, y: f32, lambda: f32) -> [f32; 2] {
        let mm = self.mm(x, y);
        let len = mm[0].hypot(mm[1]);
        let tan = bake_tan(bake, len / self.half_diagonal, lambda);
        // Focal-length units per image height.
        let scale = self.sensor_w / self.focal;
        [
            index_of(mm[0] / len * tan / scale),
            index_of(mm[1] / len * tan / scale),
        ]
    }
}

// ── The bank ─────────────────────────────────────────────────────────────────

#[test]
fn the_bank_packs_each_bake_for_the_shader() {
    use crate::prescriptions::{pack, word, ANALYSIS_STRIDE, FLAG_CHROMATIC, FLAG_PSF};
    let bakes = [thin_lens(), double_gauss(), cooke_triplet()];
    let words = pack(&bakes);
    assert_eq!(words[0], 3);
    let header = |layer: usize| words[1 + layer] as usize;
    let f = |layer: usize, slot: usize| f32::from_bits(words[header(layer) + slot]);
    // Effective focal lengths: the ideal lens exactly, the others as scaled.
    assert!(
        (f(0, word::EFL) - 50.0).abs() < 1.0e-3,
        "thin lens {}",
        f(0, word::EFL)
    );
    assert!(
        (f(1, word::EFL) - 50.0).abs() < 1.0,
        "double Gauss {}",
        f(1, word::EFL)
    );
    assert!(
        f(2, word::EFL) > 30.0 && f(2, word::EFL) < 70.0,
        "Cooke {}",
        f(2, word::EFL)
    );
    let flags = |layer: usize| words[header(layer) + word::FLAGS];
    assert_eq!(
        flags(0) & (FLAG_CHROMATIC | FLAG_PSF),
        0,
        "the ideal lens is perfect"
    );
    assert_ne!(flags(1) & FLAG_CHROMATIC, 0, "glass disperses");
    assert_ne!(flags(1) & FLAG_PSF, 0, "a real lens has a point spread");
    // Tables sit behind the headers, the baked ranges as the bake has them.
    assert_eq!(f(1, word::F_MIN), double_gauss().vignetting.f_numbers[0]);
    assert_eq!(
        words[header(1) + word::RAY_FIELDS] as usize,
        double_gauss().ray_map.n_field
    );
    assert_eq!(words[header(1) + word::FOOTPRINT_FIELDS] as usize, 4);
    // Field curvature is measured from the axis: zero there.
    let analysis = words[header(1) + word::ANALYSIS_DATA] as usize;
    assert_eq!(
        f32::from_bits(words[analysis]),
        0.0,
        "first analysis field is the axis"
    );
    assert_eq!(
        f32::from_bits(words[analysis + 1]),
        0.0,
        "no field curvature on the axis"
    );
    assert_eq!(
        words[header(1) + word::ANALYSIS_FIELDS] as usize,
        double_gauss().focus.n_field
    );
    assert!(analysis + double_gauss().focus.n_field * ANALYSIS_STRIDE <= words.len());
    // An empty bank is only a count.
    assert_eq!(pack(&[]), vec![0]);
}

// ── Ideal thin lens: the identity ────────────────────────────────────────────

#[test]
fn an_ideal_thin_lens_prescription_is_the_identity_with_cos4_falloff() {
    let mut rig = Rig::new();
    install(&mut rig, &[thin_lens()]);
    let bake = thin_lens();
    let scene = Scene::new(
        |x, y| {
            let (u, v) = (x as f32 / SIZE as f32, y as f32 / SIZE as f32);
            [
                0.2 + 3.0 * u,
                0.4 + 2.0 * v * v,
                1.0 + (7.0 * u).sin() * (5.0 * v).cos(),
            ]
        },
        |_, _| 3.0,
    );
    let element = full_frame_prescription();
    let plain = record_with(&mut rig, &scene, lens_with(0, 2.0), &[]);
    let frame = record_with(&mut rig, &scene, lens_with(1, 2.0), &element);
    let resolved = rig.resolved();
    assert_eq!(
        resolved.map_flags,
        MAP_REMAP | MAP_TRANSMISSION | MAP_PRESCRIPTION,
        "no spectral path: no CA"
    );
    assert_eq!(resolved.spectral_samples, 1);
    assert_eq!(
        resolved.aberration_flags, 0,
        "an ideal lens has no point spread"
    );
    assert!((resolved.focal_length_mm - 50.0).abs() < 1.0e-3);
    assert!(
        (resolved.natural_vignette - 1.0).abs() < 1.0e-6,
        "flare sources get the bake's cos^4"
    );

    let geometry = Geometry::new(bake, &rig);
    let parametric = {
        let natural = [LensElementRow::natural_vignette(1.0, LensRegion::FullFrame)];
        record_with(&mut rig, &flat_white(), lens_with(1, 2.0), &natural)
    };
    let prescribed_white = record_with(&mut rig, &flat_white(), lens_with(1, 2.0), &element);
    let mut checked = 0;
    for y in 0..SIZE {
        for x in 0..SIZE {
            if geometry.radius(x as f32, y as f32) > 0.97 {
                continue;
            }
            checked += 1;
            // The map is the identity: only the falloff differs from the
            // plain lens, by the bake's relative illumination.
            let t = bake_vignette(bake, geometry.radius(x as f32, y as f32), 2.0);
            let (a, b) = (frame.pixel(x, y), plain.pixel(x, y));
            for c in 0..3 {
                assert!(
                    (a[c] - b[c] * t).abs() <= 2.0e-3 * b[c].max(1.0),
                    "({x},{y}) channel {c}: {} vs plain {} x {t}",
                    a[c],
                    b[c]
                );
            }
            // And it is the parametric cos^4.
            let (p, q) = (prescribed_white.pixel(x, y)[1], parametric.pixel(x, y)[1]);
            assert!((p - q).abs() < 2.0e-3, "({x},{y}): bake {p}, cos^4 {q}");
        }
    }
    assert!(checked > 8000, "{checked} pixels compared");
}

// ── Double Gauss ─────────────────────────────────────────────────────────────

/// Position the sensor pixel `(x, y)` looks at, measured per colour channel
/// with ramps whose value is the scene pixel index (offset so the values stay
/// small and f16 keeps sub-hundredth precision).
fn measured_scene_position(
    rig: &mut Rig,
    lens: LensRow,
    elements: &[LensElementRow],
    at: (u32, u32),
) -> [[f32; 2]; 3] {
    const OFFSET: f32 = 8.0;
    let t = record_with(rig, &flat_white(), lens, elements).pixel(at.0, at.1);
    let ramp = |rig: &mut Rig, axis: usize| {
        // Centre the ramp on the neighbourhood: scene pixel indices near here.
        let origin = [at.0 as f32, at.1 as f32][axis];
        let scene = Scene::new(
            |x, y| [([x, y][axis] as f32 - origin + OFFSET).max(0.0); 3],
            |_, _| 3.0,
        );
        let out = record_with(rig, &scene, lens, elements).pixel(at.0, at.1);
        [0, 1, 2].map(|c| out[c] / t[c] - OFFSET + origin)
    };
    let (xs, ys) = (ramp(rig, 0), ramp(rig, 1));
    [0, 1, 2].map(|c| [xs[c], ys[c]])
}

#[test]
fn double_gauss_places_points_and_splits_colours_as_baked() {
    let mut rig = Rig::new();
    install(&mut rig, &[double_gauss()]);
    let bake = double_gauss();
    let lens = lens_with(1, 2.0);
    let _ = record_with(&mut rig, &flat_white(), lens, &full_frame_prescription());
    let resolved = rig.resolved();
    assert_eq!(
        resolved.map_flags,
        MAP_REMAP | MAP_SPECTRAL | MAP_TRANSMISSION | MAP_PRESCRIPTION
    );
    assert_eq!(resolved.spectral_samples, 3);
    assert_ne!(
        resolved.aberration_flags & AB_ACTIVE,
        0,
        "a real lens has a point spread"
    );
    // The point spread is left out below: it moves a point's centroid a little.
    let element = [LensElementRow::prescription(0, LensRegion::FullFrame).with_psf_amount(0.0)];
    let _ = record_with(&mut rig, &flat_white(), lens, &element);
    assert_eq!(rig.resolved().aberration_flags, 0);
    let geometry = Geometry::new(bake, &rig);
    let samples = spectral_samples(3);

    let mut worst_split = 0.0f32;
    for at in [(100u32, 40u32), (112, 64), (118, 20)] {
        let measured = measured_scene_position(&mut rig, lens, &element, at);
        // The weighted position each channel records: the bake's chief ray
        // for each wavelength sample, combined with the channel's weights.
        let mut expected = [[0.0f32; 2]; 3];
        for sample in &samples {
            let lambda = 550.0 + 150.0 * sample.t;
            let p = geometry.scene_pixel(bake, at.0 as f32, at.1 as f32, lambda);
            for c in 0..3 {
                expected[c][0] += sample.weights[c] * p[0];
                expected[c][1] += sample.weights[c] * p[1];
            }
        }
        for c in 0..3 {
            assert!(
                (measured[c][0] - expected[c][0]).abs() < 0.08
                    && (measured[c][1] - expected[c][1]).abs() < 0.08,
                "pixel {at:?} channel {c}: measured {:?}, baked {:?}",
                measured[c],
                expected[c]
            );
        }
        // The distortion is the bake's: at this field the lens looks farther
        // out than an ideal lens (barrel), by the baked amount.
        let r = geometry.radius(at.0 as f32, at.1 as f32);
        let mm = geometry.mm(at.0 as f32, at.1 as f32);
        let ideal_tan = mm[0].hypot(mm[1]) / geometry.focal;
        let baked_tan = bake_tan(bake, r, 550.0);
        assert!(
            baked_tan > ideal_tan,
            "barrel: looks beyond the ideal ray ({baked_tan} vs {ideal_tan})"
        );
        // Lateral colour: red and blue are not in the same place.
        let split = (measured[0][0] - measured[2][0]).hypot(measured[0][1] - measured[2][1]);
        let expected_split =
            (expected[0][0] - expected[2][0]).hypot(expected[0][1] - expected[2][1]);
        println!(
            "pixel {at:?}: r {r:.2}, red-blue split {split:.3} px (baked {expected_split:.3})"
        );
        assert!(
            (split - expected_split).abs() < 0.1,
            "split {split} vs baked {expected_split}"
        );
        worst_split = worst_split.max(expected_split);
    }
    assert!(
        worst_split > 0.02,
        "the double Gauss has measurable lateral colour ({worst_split})"
    );
}

#[test]
fn the_overscan_of_a_prescription_is_the_baked_corner_reach() {
    let mut rig = Rig::new();
    install(&mut rig, &[double_gauss()]);
    let bake = double_gauss();
    let element = full_frame_prescription();
    let _ = record_with(&mut rig, &flat_white(), lens_with(1, 2.0), &element);
    let geometry = Geometry::new(bake, &rig);
    // The shader's 64 boundary samples at the spectrum's ends and middle.
    let scale = geometry.sensor_w / geometry.focal;
    let mut reach = 1.0f32;
    for lid in 0..64u32 {
        let k = (lid % 16) as f32 / 16.0;
        let uv = match lid / 16 {
            0 => [k, 0.0],
            1 => [1.0, k],
            2 => [1.0 - k, 1.0],
            _ => [0.0, 1.0 - k],
        };
        let q = [uv[0] - 0.5, uv[1] - 0.5];
        let mm = [q[0] * geometry.sensor_w, q[1] * geometry.sensor_w];
        let len = mm[0].hypot(mm[1]);
        for t in [-1.0f32, 0.0, 1.0] {
            let tan = bake_tan(bake, len / geometry.half_diagonal, 550.0 + 150.0 * t);
            let s = [mm[0] / len * tan / scale, mm[1] / len * tan / scale];
            reach = reach.max((s[0].abs() / 0.5).max(s[1].abs() / 0.5));
        }
    }
    let resolved = rig.resolved();
    println!(
        "required overscan {} (baked corner {reach})",
        resolved.required_overscan
    );
    assert!(reach > 1.0, "the barrel looks past the frame at the corner");
    assert!(
        (resolved.required_overscan - reach).abs() < 2.0e-3,
        "GPU {} vs CPU {reach}",
        resolved.required_overscan
    );
}

#[test]
fn the_cpu_overscan_of_prescriptions_is_the_gpus() {
    use crate::ray_map::{applied_overscan, required_overscan_with_bank, RayMap};
    let mut rig = Rig::new();
    install(&mut rig, &[thin_lens(), cooke_triplet(), double_gauss()]);
    let bank = rig.prescriptions.take().unwrap();
    let region = LensRegion::Ellipse {
        center: [0.0, 0.0],
        radii: [0.6, 0.45],
        rotation: 0.3,
    };
    // Whole-frame lenses, one after the other; a lens over an ellipse with a
    // parametric distortion under it; and a prescription replacing a projection.
    let stacks: Vec<(f32, Vec<LensElementRow>)> = vec![
        (
            2.0,
            vec![LensElementRow::prescription(0, LensRegion::FullFrame)],
        ),
        (
            5.0,
            vec![LensElementRow::prescription(1, LensRegion::FullFrame)],
        ),
        (
            2.0,
            vec![LensElementRow::prescription(2, LensRegion::FullFrame)],
        ),
        (
            2.0,
            vec![
                LensElementRow::division(-0.3, 0.0, LensRegion::FullFrame),
                LensElementRow::prescription(2, region),
            ],
        ),
        (
            2.0,
            vec![
                LensElementRow::projection(ProjectionModel::Equidistant, LensRegion::FullFrame),
                LensElementRow::prescription(2, LensRegion::FullFrame),
            ],
        ),
    ];
    rig.prescriptions = Some(bank);
    for (i, (f_number, stack)) in stacks.iter().enumerate() {
        let lens = lens_with(stack.len(), *f_number);
        let _ = record_with(&mut rig, &flat_white(), lens, stack);
        let gpu = rig.resolved().required_overscan;
        let bank = rig.prescriptions.as_ref().unwrap();
        let cpu = required_overscan_with_bank(&lens, stack, 1.0, bank);
        println!("stack {i}: GPU {gpu}, CPU {cpu}");
        assert!(
            (gpu - cpu).abs() < 2.0e-3,
            "stack {i}: GPU {gpu} vs CPU {cpu}"
        );
        // The bank's own and the free-standing rays agree, and the applied
        // factor is the capped required one.
        let from_rays = RayMap::with_rays(&lens, stack, 1.0, bank.rays()).required_overscan();
        assert_eq!(from_rays, cpu);
        assert_eq!(
            applied_overscan(&lens, stack, 1.0, bank.rays()),
            if cpu > 1.001 { cpu.min(2.0) } else { 1.0 }
        );
    }
    // Without the bank the prescription is inert, as on the GPU.
    let stack = [LensElementRow::prescription(2, LensRegion::FullFrame)];
    assert_eq!(
        crate::ray_map::required_overscan(&lens_with(1, 2.0), &stack, 1.0),
        1.0
    );
}

#[test]
fn a_full_frame_prescription_sets_the_focal_length_and_the_widest_aperture() {
    let mut rig = Rig::new();
    install(&mut rig, &[double_gauss()]);
    let element = full_frame_prescription();
    let mut lens = lens_with(1, 1.0);
    lens.focal_length_mm = 85.0;
    let _ = record_with(&mut rig, &flat_white(), lens, &element);
    let resolved = rig.resolved();
    let bank = rig.prescriptions.as_ref().unwrap();
    assert!(
        (resolved.focal_length_mm - bank.efl_mm(0)).abs() < 1.0e-4,
        "{resolved:?}"
    );
    assert!((bank.efl_mm(0) - 50.0).abs() < 1.0);
    assert_eq!(
        resolved.f_number,
        bank.min_f_number(0),
        "f/1 is not available on an f/2 design"
    );
    assert!(resolved.t_stop >= resolved.f_number);
    // Stopped down it is left alone.
    let _ = record_with(&mut rig, &flat_white(), lens_with(1, 5.6), &element);
    assert_eq!(rig.resolved().f_number, 5.6);

    // A prescription over part of the frame leaves the view's optics alone.
    let half = [LensElementRow::prescription(
        0,
        LensRegion::HalfPlane {
            angle: 0.0,
            offset: 0.0,
        },
    )];
    let _ = record_with(&mut rig, &flat_white(), lens, &half);
    let resolved = rig.resolved();
    assert_eq!((resolved.focal_length_mm, resolved.f_number), (85.0, 1.0));
    assert_eq!(resolved.natural_vignette, 0.0);
}

#[test]
fn double_gauss_vignetting_is_stronger_wide_open() {
    let mut rig = Rig::new();
    install(&mut rig, &[double_gauss()]);
    let bake = double_gauss();
    let element = full_frame_prescription();
    let wide = record_with(&mut rig, &flat_white(), lens_with(1, 2.0), &element);
    let geometry = Geometry::new(bake, &rig);
    let stopped = record_with(&mut rig, &flat_white(), lens_with(1, 8.0), &element);
    let mut darker = 0;
    for (x, y) in [(112u32, 64u32), (118, 20), (20, 100), (64, 64)] {
        let r = geometry.radius(x as f32, y as f32);
        let (a, b) = (wide.pixel(x, y)[1], stopped.pixel(x, y)[1]);
        let (ea, eb) = (bake_vignette(bake, r, 2.0), bake_vignette(bake, r, 8.0));
        println!("r {r:.2}: f/2 {a:.4} (baked {ea:.4}), f/8 {b:.4} (baked {eb:.4})");
        assert!(
            (a - ea).abs() < 3.0e-3 && (b - eb).abs() < 3.0e-3,
            "r {r}: GPU {a} / {b}, baked {ea} / {eb}"
        );
        assert!(a <= b + 1.0e-3, "wide open is never brighter off axis");
        if a < b - 0.01 {
            darker += 1;
        }
    }
    assert!(
        darker >= 2,
        "optical vignetting darkens the corners wide open"
    );
    // The axis is the reference.
    assert!((wide.pixel(64, 64)[1] - 1.0).abs() < 2.0e-3);
}

/// Extent of a footprint mask along x (radial) and along y, in texels, from
/// its covered texels.
fn mask_extent(bake: &LensBake, field: usize) -> (f32, f32) {
    let fp = &bake.footprints;
    let res = fp.resolution;
    let mask = fp.mask(field);
    let (mut x0, mut x1, mut y0, mut y1) = (res, 0, res, 0);
    for v in 0..res {
        for u in 0..res {
            if mask[v * res + u] >= 0.5 {
                x0 = x0.min(u);
                x1 = x1.max(u);
                y0 = y0.min(v);
                y1 = y1.max(v);
            }
        }
    }
    ((x1 + 1 - x0) as f32, (y1 + 1 - y0) as f32)
}

/// The 10-90% width of a blurred edge in `frame` divided by the flat white
/// frame `flat` of the same lens (the vignetting is no part of the edge),
/// averaged over `along` the edge.
fn normalised_edge_width(
    frame: &Frame,
    flat: &Frame,
    vertical: bool,
    along: std::ops::Range<u32>,
    channel: usize,
) -> f32 {
    let profile: Vec<f32> = (0..SIZE)
        .map(|t| {
            let sum: f32 = along
                .clone()
                .map(|s| {
                    let (x, y) = if vertical { (t, s) } else { (s, t) };
                    frame.pixel(x, y)[channel] / flat.pixel(x, y)[channel].max(1.0e-3)
                })
                .sum();
            sum / along.len() as f32
        })
        .collect();
    let cross = |level: f32| {
        let i = profile
            .iter()
            .position(|&v| v < level)
            .expect("edge never falls below the level");
        let (a, b) = (profile[i - 1], profile[i]);
        (i - 1) as f32 + (a - level) / (a - b)
    };
    cross(0.1) - cross(0.9)
}

/// Edge widths (x, y) through the centre and (radial, tangential) at the
/// middle of the right edge region, for a lens focused so near that an
/// infinitely far edge blurs by tens of pixels.
fn bokeh_widths(
    rig: &mut Rig,
    lens: LensRow,
    elements: &[LensElementRow],
) -> ((f32, f32), (f32, f32)) {
    let flat = record_with(
        rig,
        &Scene::new(|_, _| [1.0; 3], infinity()),
        lens,
        elements,
    );
    let mut width = |vertical: bool, at: u32, along: std::ops::Range<u32>| {
        let frame = record_with(rig, &edge_scene(vertical, at), lens, elements);
        normalised_edge_width(&frame, &flat, vertical, along, 1)
    };
    let axis = (width(true, 64, 52..76), width(false, 64, 52..76));
    let off_axis = (width(true, 108, 56..72), width(false, 64, 100..116));
    (axis, off_axis)
}

#[test]
fn double_gauss_bokeh_is_a_cats_eye_off_axis_only() {
    let mut rig = Rig::new();
    install(&mut rig, &[double_gauss()]);
    let bake = double_gauss();
    let element = full_frame_prescription();
    // Strongly defocused: the edge at x = 108 sits at r ~ 0.6.
    let mut lens = bokeh_lens();
    lens.f_number = 2.0;
    lens.focus_distance_m = 0.3;
    lens.quality = 0;
    lens.element_count = 1;
    let ((on_x, on_y), (radial, tangential)) = bokeh_widths(&mut rig, lens, &element);
    println!("axis {on_x} x {on_y}; off axis radial {radial}, tangential {tangential}");
    // The same lens without the cat's eye: a plain round iris.
    let mut plain = lens;
    plain.element_count = 0;
    plain.focal_length_mm = 50.0;
    let (_, (plain_radial, plain_tangential)) = bokeh_widths(&mut rig, plain, &[]);
    println!("plain: radial {plain_radial}, tangential {plain_tangential}");
    assert!(
        (plain_radial / plain_tangential - 1.0).abs() < 0.08,
        "control {plain_radial} x {plain_tangential}"
    );
    assert!(
        (on_x / on_y - 1.0).abs() < 0.08,
        "round on the axis: {on_x} x {on_y}"
    );
    // The baked mask at this field is narrower along the radius, and so is the bokeh.
    let geometry = Geometry::new(bake, &rig);
    let r = geometry.radius(108.0, 64.0);
    // Fields 0.5 and 0.75 bracket it.
    let (a, b) = (mask_extent(bake, 1), mask_extent(bake, 2));
    let t = ((r - 0.5) / 0.25).clamp(0.0, 1.0);
    let (ex, ey) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
    let (cx, cy) = mask_extent(bake, 0);
    println!("r {r:.2}: mask extent {ex} x {ey} (axis {cx} x {cy})");
    assert!(
        ex < 0.95 * ey,
        "the baked footprint is clipped radially ({ex} x {ey})"
    );
    assert!(
        radial < 0.95 * tangential,
        "off axis: radial {radial}, tangential {tangential}"
    );
    assert!(
        (radial / tangential - ex / ey).abs() < 0.2,
        "bokeh ratio {} vs footprint {}",
        radial / tangential,
        ex / ey
    );
}

#[test]
fn a_prescription_is_applied_once_not_twice() {
    let mut rig = Rig::new();
    install(&mut rig, &[double_gauss()]);
    let rx = LensElementRow::prescription(0, LensRegion::FullFrame);
    let natural = LensElementRow::natural_vignette(1.0, LensRegion::FullFrame);
    let optical = LensElementRow::optical_vignette(1.0, 1.0, 0.0, LensRegion::FullFrame);
    let distortion = LensElementRow::brown_conrady(
        [-0.2, 0.05, 0.0, 0.0, 0.0, 0.0],
        [0.0; 2],
        LensRegion::FullFrame,
    );
    let ca = LensElementRow::lateral_ca(0.05, 0.0, LensRegion::FullFrame);
    let scene = Scene::new(
        |x, y| {
            [
                0.5 + x as f32 * 0.01,
                1.0 + y as f32 * 0.01,
                2.0 + ((x ^ y) & 7) as f32 * 0.1,
            ]
        },
        |_, _| 3.0,
    );
    let alone = record_with(&mut rig, &scene, lens_with(1, 2.0), &[rx]);
    // Every parametric equivalent in the stack, before and after: nothing
    // changes where the prescription covers.
    for stack in [
        vec![rx, natural],
        vec![natural, rx],
        vec![natural, optical, rx],
        vec![distortion, rx, ca],
        vec![ca, natural, rx, distortion, optical],
    ] {
        let frame = record_with(&mut rig, &scene, lens_with(stack.len(), 2.0), &stack);
        assert_eq!(
            frame.output, alone.output,
            "a parametric equivalent leaked through the prescription"
        );
    }
    let natural_only = record_with(&mut rig, &scene, lens_with(1, 2.0), &[natural]);
    assert_ne!(
        natural_only.output, alone.output,
        "the control differs from the prescription"
    );

    // Over half the frame the prescription replaces the natural falloff
    // there and the parametric one stays on the other half.
    let left = LensElementRow::prescription(
        0,
        LensRegion::HalfPlane {
            angle: std::f32::consts::PI,
            offset: 0.0,
        },
    )
    .with_feather(0.0);
    let stack = [left, natural];
    let both = record_with(&mut rig, &scene, lens_with(2, 2.0), &stack);
    let rx_left = record_with(&mut rig, &scene, lens_with(1, 2.0), &[left]);
    // The half-plane at angle pi covers x < 0: the left half is the
    // prescription alone, the natural falloff does not touch it.
    for y in (0..SIZE).step_by(7) {
        for x in 0..SIZE / 2 - 2 {
            assert_eq!(
                both.pixel(x, y),
                rx_left.pixel(x, y),
                "({x},{y}) left half is the prescription alone"
            );
        }
    }
    // The right half is darkened by cos^4 exactly once.
    let white = flat_white();
    let both_white = record_with(&mut rig, &white, lens_with(2, 2.0), &stack);
    let natural_white = record_with(&mut rig, &white, lens_with(1, 2.0), &[natural]);
    for (x, y) in [(100u32, 30u32), (120, 100), (90, 64)] {
        let (a, b) = (both_white.pixel(x, y)[1], natural_white.pixel(x, y)[1]);
        assert!((a - b).abs() < 1.0e-3 && a < 0.99, "({x},{y}): {a} vs {b}");
    }
}

#[test]
fn cooke_triplet_is_f5_and_its_edges_fall_off() {
    let mut rig = Rig::new();
    install(&mut rig, &[cooke_triplet()]);
    let bake = cooke_triplet();
    let element = full_frame_prescription();
    // Asked for f/2, the triplet is only f/5 (or whatever it was baked at).
    let _ = record_with(&mut rig, &flat_white(), lens_with(1, 2.0), &element);
    let resolved = rig.resolved();
    assert!(
        (resolved.f_number - bake.vignetting.f_numbers[0]).abs() < 1.0e-4,
        "{resolved:?}"
    );
    assert!(
        (resolved.focal_length_mm - rig.prescriptions.as_ref().unwrap().efl_mm(0)).abs() < 1.0e-4
    );
    let geometry = Geometry::new(bake, &rig);
    let frame = record_with(&mut rig, &flat_white(), lens_with(1, 2.0), &element);
    let (centre, edge) = (frame.pixel(64, 64)[1], frame.pixel(118, 20)[1]);
    let r = geometry.radius(118.0, 20.0);
    let expected = bake_vignette(bake, r, resolved.f_number);
    println!("Cooke: centre {centre}, r {r:.2}: {edge} (baked {expected})");
    assert!((centre - 1.0).abs() < 2.0e-3);
    assert!(edge < 0.95 && (edge - expected).abs() < 3.0e-3);
}

// ── Point spread ─────────────────────────────────────────────────────────────

/// Mass, centroid and variances (x, y) of one channel around `at`.
fn blob_moments(frame: &Frame, at: (u32, u32), radius: i32) -> (f32, [f32; 2], [f32; 2]) {
    let (mut sum, mut sx, mut sy) = (0.0f32, 0.0f32, 0.0f32);
    let mut samples = Vec::new();
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let x = (at.0 as i32 + dx).clamp(0, SIZE as i32 - 1) as u32;
            let y = (at.1 as i32 + dy).clamp(0, SIZE as i32 - 1) as u32;
            let v = frame.pixel(x, y)[1];
            sum += v;
            sx += v * x as f32;
            sy += v * y as f32;
            samples.push((x as f32, y as f32, v));
        }
    }
    let (cx, cy) = (sx / sum, sy / sum);
    let (mut vx, mut vy) = (0.0, 0.0);
    for (x, y, v) in samples {
        vx += v * (x - cx) * (x - cx);
        vy += v * (y - cy) * (y - cy);
    }
    (sum, [cx, cy], [vx / sum, vy / sum])
}

/// The ideal thin lens with a made-up point spread, to isolate the plumbing
/// of the PSF table: radial rms `radial` and sagittal rms `sagittal` (mm, at
/// the corner, growing linearly with the field), a coma `shift` and `edge`.
fn thin_lens_with_spot(radial: f32, sagittal: f32, shift: f32, edge: f32) -> LensBake {
    let mut bake = thin_lens().clone();
    let psf = &mut bake.psf;
    for i in 0..psf.n_field {
        let r = psf.field_r[i];
        for k in 0..psf.n_defocus {
            let at = (i * psf.n_defocus + k) * psf.stride;
            psf.data[at] = (radial * radial + sagittal * sagittal).sqrt() * r;
            psf.data[at + 1] = radial * r;
            psf.data[at + 2] = sagittal * r;
            psf.data[at + 3] = shift * r;
            psf.data[at + 4] = edge * r.max(1.0e-3);
        }
    }
    bake
}

fn spot_scene() -> Scene {
    let sigma = 1.3f32;
    let centre = (110.0f32, 64.0f32);
    Scene::new(
        |x, y| {
            let d2 = (x as f32 - centre.0).powi(2) + (y as f32 - centre.1).powi(2);
            [100.0 * (-d2 / (2.0 * sigma * sigma)).exp(); 3]
        },
        |_, _| 3.0,
    )
}

#[test]
fn the_baked_point_spread_blurs_along_and_across_the_radius() {
    let mut rig = Rig::new();
    let bake = thin_lens_with_spot(0.7, 0.35, 0.0, 1.0);
    install(&mut rig, &[&bake]);
    let scene = spot_scene();
    let element = full_frame_prescription();
    let plain = record_with(&mut rig, &scene, lens_with(0, 2.0), &[]);
    let frame = record_with(&mut rig, &scene, lens_with(1, 2.0), &element);
    let resolved = rig.resolved();
    assert_ne!(
        resolved.aberration_flags & AB_ACTIVE,
        0,
        "a point spread wakes the aberration stage"
    );
    let geometry = Geometry::new(&bake, &rig);
    let r = geometry.radius(110.0, 64.0);
    let px_per_mm = SIZE as f32 / geometry.sensor_w;
    // The core Gaussian is the table's rms at this field, in pixels.
    let (sigma_r, sigma_t) = (0.7 * r * px_per_mm, 0.35 * r * px_per_mm);
    let (mass0, _, var0) = blob_moments(&plain, (110, 64), 12);
    let (mass1, _, var1) = blob_moments(&frame, (110, 64), 12);
    println!("r {r:.2}: sigma {sigma_r:.2} x {sigma_t:.2} px; variance {var0:?} -> {var1:?}");
    // x is radial on this row, y is across it. The Gaussian is drawn from
    // taps out to 2.4 sigma, which leaves about 0.83 of its variance.
    let gain = [var1[0] - var0[0], var1[1] - var0[1]];
    assert!(
        gain[0] > 0.7 * sigma_r * sigma_r && gain[0] < 1.1 * sigma_r * sigma_r,
        "radial {var0:?} -> {var1:?}"
    );
    assert!(
        gain[1] > 0.6 * sigma_t * sigma_t && gain[1] < 1.5 * sigma_t * sigma_t,
        "tangential {var0:?} -> {var1:?}"
    );
    assert!(
        gain[0] > 2.5 * gain[1],
        "elongated along the radius: {gain:?}"
    );
    // The spread conserves light; the falloff of the lens (cos^4 here) takes its share.
    let falloff = bake_vignette(&bake, r, 2.0);
    assert!(
        (mass1 / (mass0 * falloff) - 1.0).abs() < 0.03,
        "the spread conserves light ({mass0} x {falloff} -> {mass1})"
    );
    // The axis is perfect.
    let on_axis = Scene::new(
        |x, y| {
            let d2 = (x as f32 - 63.5).powi(2) + (y as f32 - 63.5).powi(2);
            [100.0 * (-d2 / (2.0f32 * 1.3 * 1.3)).exp(); 3]
        },
        |_, _| 3.0,
    );
    let a = record_with(&mut rig, &on_axis, lens_with(0, 2.0), &[]);
    let b = record_with(&mut rig, &on_axis, lens_with(1, 2.0), &element);
    let (_, _, va) = blob_moments(&a, (64, 64), 12);
    let (_, _, vb) = blob_moments(&b, (64, 64), 12);
    assert!(
        (vb[0] - va[0]).abs() < 0.05 * va[0] && (vb[1] - va[1]).abs() < 0.05 * va[1],
        "axis {va:?} -> {vb:?}"
    );

    // PSF_AMOUNT 0 turns the stage off without touching the map.
    let off = [LensElementRow::prescription(0, LensRegion::FullFrame).with_psf_amount(0.0)];
    let frame = record_with(&mut rig, &scene, lens_with(1, 2.0), &off);
    assert_eq!(rig.resolved().aberration_flags, 0);
    assert_eq!(
        frame.output,
        record_with(&mut rig, &scene, lens_with(1, 2.0), &off).output
    );
}

#[test]
fn the_baked_coma_tail_points_outward_for_a_positive_shift() {
    let mut rig = Rig::new();
    let scene = spot_scene();
    let element = full_frame_prescription();
    let mut shifts = Vec::new();
    for shift in [0.0f32, 0.3, -0.3] {
        let bake = thin_lens_with_spot(0.2, 0.2, shift, 2.4);
        install(&mut rig, &[&bake]);
        let frame = record_with(&mut rig, &scene, lens_with(1, 2.0), &element);
        let (_, centroid, _) = blob_moments(&frame, (110, 64), 14);
        shifts.push(centroid[0] - 110.0);
    }
    let geometry_r = {
        let mm = q_of(110.0) * 36.0;
        mm / thin_lens().ray_map.sensor_half_diagonal_mm
    };
    let px_per_mm = SIZE as f32 / 36.0;
    println!("coma centroid shifts {shifts:?} px at r {geometry_r:.2}");
    // No coma: the centre stays. A positive shift pulls the centroid outward
    // (+x on this row) by about the baked shift at this field, a negative one inward.
    assert!(shifts[0].abs() < 0.05, "no coma: {}", shifts[0]);
    let expected = 0.3 * geometry_r * px_per_mm;
    assert!(
        shifts[1] > 0.5 * expected && shifts[1] < 1.5 * expected,
        "outward {} vs {expected}",
        shifts[1]
    );
    assert!(
        shifts[2] < -0.5 * expected && shifts[2] > -1.5 * expected,
        "inward {} vs -{expected}",
        shifts[2]
    );
}

#[test]
fn the_parametric_point_spreads_give_way_to_the_baked_one() {
    let mut rig = Rig::new();
    let bake = thin_lens_with_spot(0.7, 0.35, 0.0, 1.0);
    install(&mut rig, &[&bake]);
    let scene = spot_scene();
    let rx = LensElementRow::prescription(0, LensRegion::FullFrame);
    let astig = LensElementRow::astigmatism(0.02, 0.5, LensRegion::FullFrame);
    let soft = LensElementRow::corner_softness(0.02, 0.0, LensRegion::FullFrame);
    let coma = LensElementRow::coma(0.5, 0.05, LensRegion::FullFrame);
    let alone = record_with(&mut rig, &scene, lens_with(1, 2.0), &[rx]);
    let stacked = record_with(
        &mut rig,
        &scene,
        lens_with(4, 2.0),
        &[astig, soft, coma, rx],
    );
    assert_eq!(
        stacked.output, alone.output,
        "parametric aberrations were applied on top of the bake"
    );
    let control = record_with(&mut rig, &scene, lens_with(3, 2.0), &[astig, soft, coma]);
    assert_ne!(control.output, alone.output);
}

// ── Focus: field curvature and longitudinal colour ───────────────────────────

/// The ideal thin lens with made-up focus data: best focus `curvature * r^2`
/// mm along +z at field `r`, and `axial` mm of axial focus shift at the
/// bake's three wavelengths (450, 550, 650 nm).
fn thin_lens_with_focus(curvature: f32, axial: [f32; 3]) -> LensBake {
    let mut bake = thin_lens().clone();
    for i in 0..bake.focus.n_field {
        let r = bake.focus.field_r[i];
        bake.focus.best_mm[i] = curvature * r * r;
    }
    bake.focus.axial_real_mm = axial.to_vec();
    bake.focus.axial_paraxial_mm = axial.to_vec();
    bake
}

#[test]
fn baked_field_curvature_defocuses_the_corners_of_the_focal_plane() {
    let mut rig = Rig::new();
    // The best focus moves in front of the sensor toward the corners, which
    // is a nearer subject in focus there: the 3 m plane is behind it.
    let bake = thin_lens_with_focus(-2.0, [0.0; 3]);
    install(&mut rig, &[&bake]);
    let element = full_frame_prescription();
    let lens = lens_with(1, 2.0);
    let frame = record_with(
        &mut rig,
        &Scene::new(|_, _| [1.0; 3], |_, _| 3.0),
        lens,
        &element,
    );
    let geometry = Geometry::new(&bake, &rig);
    let focal_m = geometry.focal * 1.0e-3;
    for (x, y) in [(64u32, 64u32), (96, 64), (118, 20)] {
        let r = geometry.radius(x as f32, y as f32);
        // Linear between the analysis fields, as the shader reads them.
        let focus = &bake.focus;
        let upper = focus
            .field_r
            .iter()
            .position(|&f| f >= r)
            .unwrap_or(focus.n_field - 1)
            .max(1);
        let t = ((r - focus.field_r[upper - 1])
            / (focus.field_r[upper] - focus.field_r[upper - 1]))
            .clamp(0.0, 1.0);
        let best = focus.best_mm[upper - 1] * (1.0 - t) + focus.best_mm[upper] * t;
        let dioptres = -(best - focus.best_mm[0]) * 1.0e-3 / (focal_m * focal_m);
        let focus_distance = 1.0 / (1.0 / 3.0 + dioptres);
        let expected = expected_coc(&lens, focus_distance, 3.0);
        let measured = frame.coc(x, y)[0];
        println!("({x},{y}) r {r:.2}: {dioptres:.3} dpt, circle {measured:.2} px (expected {expected:.2})");
        assert!(
            (measured - expected).abs() <= 0.03 * expected.abs() + 0.05,
            "({x},{y}): {measured} vs {expected}"
        );
    }
    assert!(
        frame.coc(118, 20)[0] > 1.5,
        "the corner of the 3 m plane is out of focus"
    );
}

#[test]
fn baked_longitudinal_colour_gives_each_channel_its_own_circle() {
    let mut rig = Rig::new();
    // Red focuses behind the sensor and blue in front of it: for a subject
    // beyond the focus red has the smaller circle and blue the larger.
    let bake = thin_lens_with_focus(0.0, [-1.0, 0.0, 1.0]);
    install(&mut rig, &[&bake]);
    let element = full_frame_prescription();
    let mut lens = bokeh_lens();
    lens.f_number = 2.0;
    lens.focal_length_mm = 50.0;
    lens.focus_distance_m = 1.0;
    lens.quality = 0;
    let flat = record_with(
        &mut rig,
        &Scene::new(|_, _| [1.0; 3], infinity()),
        lens,
        &element,
    );
    let frame = record_with(&mut rig, &edge_scene(true, 64), lens, &element);
    let width = |channel| normalised_edge_width(&frame, &flat, true, 52..76, channel);
    let (red, green, blue) = (width(0), width(1), width(2));
    println!("edge widths: red {red}, green {green}, blue {blue}");
    assert!(
        red < 0.85 * green && green < 0.85 * blue,
        "{red} < {green} < {blue}"
    );
    // Without the prescription's shifts all three agree.
    let neutral = thin_lens_with_focus(0.0, [0.0; 3]);
    install(&mut rig, &[&neutral]);
    let flat = record_with(
        &mut rig,
        &Scene::new(|_, _| [1.0; 3], infinity()),
        lens,
        &element,
    );
    let frame = record_with(&mut rig, &edge_scene(true, 64), lens, &element);
    let widths = [0, 1, 2].map(|c| normalised_edge_width(&frame, &flat, true, 52..76, c));
    assert!(
        widths.iter().all(|w| (w / widths[1] - 1.0).abs() < 0.05),
        "{widths:?}"
    );
}

// ── Without prescriptions and user elements nothing changes ─────────────────

#[test]
fn a_bank_nobody_uses_and_inert_user_elements_change_nothing() {
    let mut rig = Rig::new();
    let scene = Scene::new(
        |x, y| {
            [
                x as f32 * 0.37 + 0.1,
                y as f32 * 3.1 + 0.2,
                40.0 + (x ^ y) as f32,
            ]
        },
        |_, _| 3.0,
    );
    let color = rig.color_view(&scene);
    let input = rig.read_texture(&color);
    let base = record_with(&mut rig, &scene, lens_with(0, 2.0), &[]);
    assert_eq!(base.output, input);
    let base_args = rig.args();
    let base_resolved = rig.resolved();

    // A bank is bound but nothing names it.
    install(&mut rig, &[double_gauss(), thin_lens()]);
    let bound = record_with(&mut rig, &scene, lens_with(0, 2.0), &[]);
    assert_eq!(bound.output, base.output);
    assert_eq!(
        rig.args(),
        base_args,
        "the bank alone dispatches nothing new"
    );
    assert_eq!(rig.resolved(), base_resolved);
    // An element for a layer the bank lacks is inert.
    let missing = [LensElementRow::prescription(7, LensRegion::FullFrame)];
    let frame = record_with(&mut rig, &scene, lens_with(1, 2.0), &missing);
    assert_eq!(frame.output, base.output);
    assert_eq!(rig.resolved().map_flags, 0);
    rig.prescriptions = None;
    let frame = record_with(
        &mut rig,
        &scene,
        lens_with(1, 2.0),
        &[LensElementRow::prescription(0, LensRegion::FullFrame)],
    );
    assert_eq!(
        frame.output, base.output,
        "no bank: a prescription element covers nothing"
    );

    // USER elements in a lens built without a snippet are ignored entirely.
    let user = [
        LensElementRow::user(
            user::MAP | user::TRANSMISSION | user::FOCUS | user::SPECTRAL,
            [1.0; 16],
            LensRegion::FullFrame,
        ),
        LensElementRow::user(user::MAP, [0.0; 16], LensRegion::FullFrame),
    ];
    let frame = record_with(&mut rig, &scene, lens_with(2, 2.0), &user);
    assert_eq!(
        frame.output, base.output,
        "a USER element without a snippet changed the image"
    );
    assert_eq!(rig.resolved().map_flags, 0, "and it costs no stage");
    assert_eq!(rig.args(), base_args);
}

// ── User elements ────────────────────────────────────────────────────────────

const MIRROR: &str = "
fn user_ray_map(uv: vec2<f32>, params: array<f32, 16>, t: f32) -> vec2<f32> {
    return vec2(1.0 - uv.x, uv.y);
}";

#[test]
fn a_user_ray_map_that_flips_u_mirrors_the_image() {
    let mut rig = Rig::with_user(Some(MIRROR));
    let scene = Scene::new(
        |x, y| {
            [
                x as f32 * 0.37 + 0.1,
                y as f32 * 3.1 + 0.2,
                40.0 + (x ^ y) as f32,
            ]
        },
        |_, _| 3.0,
    );
    let color = rig.color_view(&scene);
    let input = rig.read_texture(&color);
    let element = [LensElementRow::user(
        user::MAP,
        [0.0; 16],
        LensRegion::FullFrame,
    )];
    let frame = record_with(&mut rig, &scene, lens_with(1, 2.8), &element);
    assert_eq!(rig.resolved().map_flags, MAP_REMAP);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (got, want) = (frame.pixel(x, y), {
                let i = ((y * SIZE + (SIZE - 1 - x)) * 4) as usize;
                [
                    f16_to_f32(input[i]),
                    f16_to_f32(input[i + 1]),
                    f16_to_f32(input[i + 2]),
                ]
            });
            for c in 0..3 {
                assert!(
                    (got[c] - want[c]).abs() <= 1.0e-3 * want[c].abs().max(1.0),
                    "({x},{y}) {got:?} vs mirrored {want:?}"
                );
            }
        }
    }
    // A hook the element does not select does not run.
    let off = [LensElementRow::user(
        user::TRANSMISSION,
        [0.0; 16],
        LensRegion::FullFrame,
    )];
    let frame = record_with(&mut rig, &scene, lens_with(1, 2.8), &off);
    assert_eq!(
        frame.output, input,
        "the default transmission hook is the identity"
    );
    // A region limits the flip to it, the rest is the image.
    let half = [LensElementRow::user(
        user::MAP,
        [0.0; 16],
        LensRegion::HalfPlane {
            angle: 0.0,
            offset: 0.25,
        },
    )
    .with_feather(0.0)];
    let frame = record_with(&mut rig, &scene, lens_with(1, 2.8), &half);
    assert_eq!(frame.pixel(8, 40), {
        let i = ((40 * SIZE + 8) * 4) as usize;
        [
            f16_to_f32(input[i]),
            f16_to_f32(input[i + 1]),
            f16_to_f32(input[i + 2]),
        ]
    });
    let mirrored = frame.pixel(120, 40);
    let i = ((40 * SIZE + (SIZE - 1 - 120)) * 4) as usize;
    assert!((mirrored[0] - f16_to_f32(input[i])).abs() < 1.0e-3 * 50.0);
}

const TINT: &str = "
fn user_transmission(uv: vec2<f32>, params: array<f32, 16>) -> vec3<f32> {
    return vec3(uv.x, params[0], 1.0);
}
fn user_focus(uv: vec2<f32>, params: array<f32, 16>, s: f32) -> f32 {
    return s + params[1];
}";

#[test]
fn user_transmission_and_focus_hooks_run_and_missing_hooks_are_the_identity() {
    let mut rig = Rig::with_user(Some(TINT));
    let mut params = [0.0; 16];
    params[0] = 0.5;
    let element = [LensElementRow::user(
        user::TRANSMISSION,
        params,
        LensRegion::FullFrame,
    )];
    let frame = record_with(&mut rig, &flat_white(), lens_with(1, 2.8), &element);
    assert_eq!(rig.resolved().map_flags, MAP_TRANSMISSION);
    for x in [10u32, 64, 117] {
        let [r, g, b] = frame.pixel(x, 50);
        let u = (x as f32 + 0.5) / SIZE as f32;
        assert!(
            (r - u).abs() < 1.5e-3 && (g - 0.5).abs() < 1.0e-3 && (b - 1.0).abs() < 1.0e-3,
            "x {x}: {r} {g} {b}"
        );
    }
    // The snippet defines no ray map: selecting the hook leaves the image.
    let scene = Scene::new(Scene::checker, |_, _| 3.0);
    let map = [LensElementRow::user(
        user::MAP,
        [0.0; 16],
        LensRegion::FullFrame,
    )];
    let plain = record_with(&mut rig, &scene, lens_with(0, 2.8), &[]);
    let frame = record_with(&mut rig, &scene, lens_with(1, 2.8), &map);
    assert_eq!(frame.output, plain.output);

    // The focus hook adds dioptres: the 3 m plane is no longer in focus.
    let mut focus_params = [0.0; 16];
    focus_params[1] = 1.5;
    let focus = [LensElementRow::user(
        user::FOCUS,
        focus_params,
        LensRegion::FullFrame,
    )];
    let mut lens = lens_with(1, 1.4);
    lens.focal_length_mm = 100.0;
    let frame = record_with(&mut rig, &scene, lens, &focus);
    let [coc, ..] = frame.coc(64, 64);
    assert!(coc.abs() > 2.0, "defocused by the hook: {coc}");
    let frame = record_with(&mut rig, &scene, lens_with(0, 1.4), &[]);
    assert!(frame.coc(64, 64)[0].abs() < 0.05);
}
