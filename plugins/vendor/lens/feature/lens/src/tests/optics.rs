//! Phase 3 on the GPU: the ray map, spectral lateral chromatic aberration and
//! the transmission, through the production WGSL.
//!
//! Where physics gives a closed form the test checks the shader against it
//! (a point under the OpenCV model, a fisheye's radius against angle, a
//! flat port's refraction, `cos^4`, the iris area left by the barrel). The
//! rest of the elements are checked against the CPU twin in
//! [`crate::ray_map`] on a smooth image, where it is the code under test
//! and the closed forms vouch for the twin.

#![allow(clippy::needless_range_loop)]

use super::*;
use crate::ray_map::{required_overscan, spectral_samples, RayMap};

/// A lens with the stack `elements` (rows `0..n`), 50 mm f/2.8 focused on the
/// 3 m plane every scene here sits on, so nothing is out of focus.
fn stacked(elements: &[LensElementRow]) -> LensRow {
    let mut lens = lens();
    lens.element_count = elements.len() as u32;
    lens
}

fn rows(elements: &[LensElementRow]) -> Option<&[LensElementRow]> {
    Some(elements)
}

/// Sensor position of pixel index `i` (centres at integers), in image-height
/// units about the centre of the square test frame.
fn q_of(i: f32) -> f32 {
    (i + 0.5) / SIZE as f32 - 0.5
}

/// Pixel index of image-height position `q`.
fn index_of(q: f32) -> f32 {
    (q + 0.5) * SIZE as f32 - 0.5
}

/// Focal-length units per image-height unit of the default lens on the
/// square frame: a 36 mm sensor across the frame, 50 mm.
const SCALE: f32 = 36.0 / 50.0;

/// Black frame with a Gaussian blob of unit height at each (x, y) pixel
/// position.
fn blobs(points: &[(f32, f32)], sigma: f32) -> Scene {
    Scene::new(
        |x, y| {
            let v: f32 = points
                .iter()
                .map(|&(px, py)| {
                    let d2 = (x as f32 - px).powi(2) + (y as f32 - py).powi(2);
                    (-d2 / (2.0 * sigma * sigma)).exp()
                })
                .sum();
            [v; 3]
        },
        |_, _| 3.0,
    )
}

fn flat(value: [f32; 3]) -> Scene {
    Scene::new(|_, _| value, |_, _| 3.0)
}

/// The centroid (x, y) of one channel in a window around `at`, and its sum.
fn centroid(frame: &Frame, at: (f32, f32), radius: i32, channel: usize) -> (f32, f32, f32) {
    let (mut sx, mut sy, mut sum) = (0.0, 0.0, 0.0);
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let x = (at.0.round() as i32 + dx).clamp(0, SIZE as i32 - 1) as u32;
            let y = (at.1.round() as i32 + dy).clamp(0, SIZE as i32 - 1) as u32;
            let v = frame.pixel(x, y)[channel];
            sx += v * x as f32;
            sy += v * y as f32;
            sum += v;
        }
    }
    (sx / sum, sy / sum, sum)
}

fn total(frame: &Frame, channel: usize) -> f32 {
    (0..SIZE)
        .flat_map(|y| (0..SIZE).map(move |x| (x, y)))
        .map(|(x, y)| frame.pixel(x, y)[channel])
        .sum()
}

/// What the lens does to the frame without its elements.
fn plain(rig: &mut Rig, scene: &Scene) -> Frame {
    rig.record(scene, Some(&[lens()]), None)
}

#[test]
fn rectilinear_projection_is_an_exact_identity() {
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
    let reference = plain(&mut rig, &scene);
    let element = [LensElementRow::projection(
        ProjectionModel::Rectilinear,
        LensRegion::FullFrame,
    )];
    let frame = rig.record(&scene, Some(&[stacked(&element)]), rows(&element));
    assert_eq!(
        frame.output, reference.output,
        "an identity map changed the image"
    );
    let resolved = rig.resolved();
    assert_eq!(resolved.map_flags, MAP_REMAP);
    assert_eq!(resolved.spectral_samples, 1);
    assert!(
        (resolved.required_overscan - 1.0).abs() < 1.0e-4,
        "{resolved:?}"
    );
    // A lens with no map elements takes the texel-for-texel path.
    plain(&mut rig, &scene);
    assert_eq!(rig.resolved().map_flags, 0);
}

#[test]
fn brown_conrady_places_a_point_where_the_opencv_model_says() {
    let mut rig = Rig::new();
    let points = [(92.0, 38.0), (30.0, 100.0), (101.0, 104.0), (20.0, 24.0)];
    let scene = blobs(&points, 1.4);
    let k = [-0.25, 0.06, -0.01, 0.0, 0.0, 0.0];
    let p = [0.012, -0.009];
    let element = [LensElementRow::brown_conrady(k, p, LensRegion::FullFrame)];
    let frame = rig.record(&scene, Some(&[stacked(&element)]), rows(&element));
    for &(px, py) in &points {
        // The ideal point in focal-length units, then the OpenCV model.
        let ideal = [q_of(px) * SCALE, q_of(py) * SCALE];
        let r2 = ideal[0] * ideal[0] + ideal[1] * ideal[1];
        let radial = 1.0 + k[0] * r2 + k[1] * r2 * r2 + k[2] * r2 * r2 * r2;
        let (x, y) = (ideal[0], ideal[1]);
        let distorted = [
            x * radial + 2.0 * p[0] * x * y + p[1] * (r2 + 2.0 * x * x),
            y * radial + p[0] * (r2 + 2.0 * y * y) + 2.0 * p[1] * x * y,
        ];
        let expected = (
            index_of(distorted[0] / SCALE),
            index_of(distorted[1] / SCALE),
        );
        let (cx, cy, mass) = centroid(&frame, expected, 6, 1);
        assert!(mass > 1.0, "no point near {expected:?}");
        assert!(
            (cx - expected.0).abs() < 0.4 && (cy - expected.1).abs() < 0.4,
            "point {px},{py}: found {cx:.2},{cy:.2}, expected {:.2},{:.2}",
            expected.0,
            expected.1
        );
        // The OpenCV model itself, against the twin's.
        let mut params = [0.0; 16];
        params[..6].copy_from_slice(&k);
        params[6..8].copy_from_slice(&p);
        let twin = crate::ray_map::brown_conrady_distort(&params, ideal);
        assert!((twin[0] - distorted[0]).abs() < 1.0e-6 && (twin[1] - distorted[1]).abs() < 1.0e-6);
    }
}

#[test]
fn fisheye_projections_record_radius_by_angle() {
    let mut rig = Rig::new();
    // Scene points on the rectilinear image at these angles from the axis.
    let thetas: [f32; 3] = [0.15, 0.25, 0.32];
    let scene_points: Vec<(f32, f32)> = thetas
        .iter()
        .map(|t| (index_of(t.tan() / SCALE), 63.5 + 0.0))
        .collect();
    let scene = blobs(&scene_points, 1.4);
    type Radius = fn(f32) -> f32;
    let cases: [(ProjectionModel, Radius); 4] = [
        (ProjectionModel::Equidistant, |t| t),
        (ProjectionModel::Equisolid, |t| 2.0 * (t * 0.5).sin()),
        (ProjectionModel::Orthographic, |t| t.sin()),
        (ProjectionModel::Stereographic, |t| 2.0 * (t * 0.5).tan()),
    ];
    for (model, radius) in cases {
        let element = [LensElementRow::projection(model, LensRegion::FullFrame)];
        let frame = rig.record(&scene, Some(&[stacked(&element)]), rows(&element));
        let mut measured = Vec::new();
        for (&theta, &(sx, sy)) in thetas.iter().zip(&scene_points) {
            // The sensor records the ray at radius g(theta) from the centre.
            let (cx_scene, cy_scene) = (q_of(sx) * SCALE, q_of(sy) * SCALE);
            let r_scene = (cx_scene * cx_scene + cy_scene * cy_scene).sqrt();
            let scale = radius(theta) / r_scene;
            let expected = (
                index_of(cx_scene * scale / SCALE),
                index_of(cy_scene * scale / SCALE),
            );
            let (cx, cy, mass) = centroid(&frame, expected, 6, 1);
            assert!(mass > 1.0, "{model:?}: no point near {expected:?}");
            assert!(
                (cx - expected.0).abs() < 0.4 && (cy - expected.1).abs() < 0.4,
                "{model:?} theta {theta}: found {cx:.2},{cy:.2}, expected {:.2},{:.2}",
                expected.0,
                expected.1
            );
            let r_sensor = ((q_of(cx) * SCALE).powi(2) + (q_of(cy) * SCALE).powi(2)).sqrt();
            measured.push(r_sensor / theta);
        }
        if model == ProjectionModel::Equidistant {
            // Radius proportional to angle: the ratio is the focal length (1).
            for ratio in &measured {
                assert!(
                    (ratio - 1.0).abs() < 0.02,
                    "equidistant radius/angle {measured:?}"
                );
            }
        }
    }
}

#[test]
fn spectral_weights_reconstruct_white_at_every_sample_count() {
    for n in [1, 3, 7, 16] {
        let samples = spectral_samples(n);
        assert_eq!(samples.len(), n);
        for channel in 0..3 {
            let sum: f32 = samples.iter().map(|s| s.weights[channel]).sum();
            assert!(
                (sum - 1.0).abs() < 1.0e-5,
                "n {n} channel {channel}: weights sum to {sum}"
            );
        }
        assert!(samples
            .iter()
            .all(|s| s.weights.iter().all(|&w| w >= 0.0) && s.t.abs() <= 1.0));
        assert!(samples.windows(2).all(|w| w[0].t < w[1].t));
    }
    assert_eq!(
        [0, 1, 2, 3].map(crate::ray_map::spectral_sample_count),
        [3, 7, 16, 16]
    );
}

#[test]
fn a_neutral_spectral_lens_is_the_identity_at_every_quality() {
    let mut rig = Rig::new();
    let scene = Scene::new(
        |x, y| {
            let (u, v) = (x as f32 / SIZE as f32, y as f32 / SIZE as f32);
            [
                0.1 + 3.0 * u,
                0.2 + 2.0 * v * v,
                1.0 + (7.0 * u).sin() * (5.0 * v).cos(),
            ]
        },
        |_, _| 3.0,
    );
    let reference = plain(&mut rig, &scene);
    let element = [LensElementRow::lateral_ca(0.0, 0.0, LensRegion::FullFrame)];
    for (quality, samples) in [(0, 3), (1, 7), (2, 16)] {
        let mut lens = stacked(&element);
        lens.quality = quality;
        let frame = rig.record(&scene, Some(&[lens]), rows(&element));
        let resolved = rig.resolved();
        assert_eq!(
            (resolved.quality, resolved.spectral_samples),
            (quality, samples)
        );
        assert_eq!(resolved.map_flags, MAP_REMAP | MAP_SPECTRAL);
        let mut worst = 0.0f32;
        for y in 0..SIZE {
            for x in 0..SIZE {
                let (a, b) = (frame.pixel(x, y), reference.pixel(x, y));
                for c in 0..3 {
                    worst = worst.max((a[c] - b[c]).abs() / b[c].abs().max(1.0));
                }
            }
        }
        assert!(
            worst < 2.0e-3,
            "quality {quality}: {samples} samples changed the image by {worst}"
        );
    }
}

#[test]
fn lateral_chromatic_aberration_splits_a_white_line_off_axis() {
    let mut rig = Rig::new();
    // White vertical lines, two pixels wide: on the right, left and centre.
    let lines = [100.0f32, 26.0, 63.0];
    let scene = Scene::new(
        |x, _| {
            let on = lines.iter().any(|&l| x as f32 == l || x as f32 == l + 1.0);
            [if on { 1.0 } else { 0.0 }; 3]
        },
        |_, _| 3.0,
    );
    let d1 = 0.06;
    let element = [LensElementRow::lateral_ca(d1, 0.0, LensRegion::FullFrame)];
    let mut lens = stacked(&element);
    lens.quality = 1;
    let frame = rig.record(&scene, Some(&[lens]), rows(&element));
    let row = |channel: usize, line: f32| {
        // Centroid of the line in one row, window of 14 pixels.
        let (mut sx, mut sum) = (0.0, 0.0);
        for x in (line as i32 - 7)..=(line as i32 + 8) {
            let v = frame.pixel(x as u32, 64)[channel];
            sx += v * x as f32;
            sum += v;
        }
        sx / sum
    };
    let samples = spectral_samples(7);
    for &line in &lines {
        let centre = line + 0.5;
        let (r, g, b) = (row(0, line), row(1, line), row(2, line));
        // A scene line at q appears at q (1 + d1 t) for the samples that see
        // it, weighted by the channel.
        let q = q_of(centre);
        let expected = |channel: usize| {
            let at: f32 = samples
                .iter()
                .map(|s| s.weights[channel] * q * (1.0 + d1 * s.t))
                .sum();
            index_of(at)
        };
        for (channel, found) in [(0, r), (1, g), (2, b)] {
            assert!(
                (found - expected(channel)).abs() < 0.35,
                "line {line} channel {channel}: found {found:.2}, expected {:.2}",
                expected(channel)
            );
        }
        if (line - 63.0).abs() < 1.0 {
            // On the axis there is no lateral chromatic aberration.
            assert!((r - b).abs() < 0.1, "axis line split: {r} {b}");
        } else if line > 64.0 {
            // Positive dispersion: red outward (right), blue inward.
            assert!(
                r > g && g > b && r - b > 1.5,
                "right line: r {r:.2} g {g:.2} b {b:.2}"
            );
        } else {
            assert!(
                r < g && g < b && b - r > 1.5,
                "left line: r {r:.2} g {g:.2} b {b:.2}"
            );
        }
    }
    // Negative dispersion reverses it.
    let element = [LensElementRow::lateral_ca(-d1, 0.0, LensRegion::FullFrame)];
    let mut lens = stacked(&element);
    lens.quality = 1;
    let frame = rig.record(&scene, Some(&[lens]), rows(&element));
    let (r, b) = (
        centroid(&frame, (100.5, 64.0), 8, 0).0,
        centroid(&frame, (100.5, 64.0), 8, 2).0,
    );
    assert!(r < b, "negative dispersion should put red inward: {r} {b}");
}

#[test]
fn natural_vignette_follows_cos4_of_the_ray_angle() {
    let mut rig = Rig::new();
    let scene = flat([1.0; 3]);
    let element = [LensElementRow::natural_vignette(1.0, LensRegion::FullFrame)];
    let frame = rig.record(&scene, Some(&[stacked(&element)]), rows(&element));
    let resolved = rig.resolved();
    assert_eq!(
        (resolved.map_flags, resolved.natural_vignette),
        (MAP_TRANSMISSION, 1.0)
    );
    assert_eq!(resolved.required_overscan, 1.0);
    assert!((frame.pixel(64, 64)[0] - 1.0).abs() < 2.0e-3);
    let mut previous = 1.0;
    for &(x, y) in &[
        (64u32, 64u32),
        (96, 64),
        (127, 64),
        (110, 110),
        (3, 3),
        (0, 127),
    ] {
        let (qx, qy) = (q_of(x as f32) * SCALE, q_of(y as f32) * SCALE);
        let expected = 1.0 / ((1.0 + qx * qx + qy * qy) * (1.0 + qx * qx + qy * qy));
        let found = frame.pixel(x, y);
        for c in 0..3 {
            assert!(
                (found[c] / expected - 1.0).abs() < 0.005,
                "pixel {x},{y}: {} against cos^4 {expected}",
                found[c]
            );
        }
        if y == 64 {
            assert!(
                found[0] <= previous + 1.0e-3,
                "falloff must not rise outward"
            );
            previous = found[0];
        }
    }
    // The ray's real angle: an equidistant fisheye records angle theta at
    // radius theta, so the falloff is cos^4 of the sensor radius.
    let element = [
        LensElementRow::projection(ProjectionModel::Equidistant, LensRegion::FullFrame),
        LensElementRow::natural_vignette(1.0, LensRegion::FullFrame),
    ];
    let frame = rig.record(&scene, Some(&[stacked(&element)]), rows(&element));
    for &(x, y) in &[(96u32, 64u32), (120, 64), (110, 110)] {
        let r = ((q_of(x as f32) * SCALE).powi(2) + (q_of(y as f32) * SCALE).powi(2)).sqrt();
        let expected = r.cos().powi(4);
        assert!(
            (frame.pixel(x, y)[1] / expected - 1.0).abs() < 0.006,
            "fisheye pixel {x},{y}: {} against cos^4(theta) {expected}",
            frame.pixel(x, y)[1]
        );
    }
    // Half the amount lands halfway.
    let element = [LensElementRow::natural_vignette(0.5, LensRegion::FullFrame)];
    let frame = rig.record(&scene, Some(&[stacked(&element)]), rows(&element));
    let (qx, qy) = (q_of(3.0) * SCALE, q_of(3.0) * SCALE);
    let full = 1.0 / ((1.0 + qx * qx + qy * qy) * (1.0 + qx * qx + qy * qy));
    assert!((frame.pixel(3, 3)[1] - 0.5 * (1.0 + full)).abs() < 3.0e-3);
}

#[test]
fn graduated_nd_ramps_monotonically_hard_or_soft_and_tinted() {
    let mut rig = Rig::new();
    let scene = flat([1.0; 3]);
    // Dark toward +y (down), centred, two stops.
    let width = |rig: &mut Rig, softness: f32, tint: [f32; 3]| {
        let element = [LensElementRow::graduated_nd(
            2.0,
            std::f32::consts::FRAC_PI_2,
            0.0,
            softness,
            tint,
            LensRegion::FullFrame,
        )];
        let frame = rig.record(&scene, Some(&[stacked(&element)]), rows(&element));
        let column: Vec<[f32; 3]> = (0..SIZE).map(|y| frame.pixel(64, y)).collect();
        for c in 0..3 {
            assert!(
                column.windows(2).all(|w| w[1][c] <= w[0][c] + 1.0e-3),
                "softness {softness} channel {c} rises somewhere"
            );
        }
        // Clear above, the full density (times the tint) below.
        for c in 0..3 {
            assert!((column[2][c] - 1.0).abs() < 3.0e-3);
            let full = 0.25 * tint[c];
            assert!(
                (column[SIZE as usize - 3][c] - full).abs() < 3.0e-3 + 0.01 * full,
                "{column:?}"
            );
        }
        // 10-90% of the way down, in pixels (of the green channel's range).
        let green_full = 0.25 * tint[1];
        let cross = |share: f32| {
            let level = 1.0 - share * (1.0 - green_full);
            column.iter().position(|v| v[1] < level).unwrap() as f32
        };
        cross(0.9) - cross(0.1)
    };
    let hard = width(&mut rig, 0.01, [1.0; 3]);
    let soft = width(&mut rig, 0.6, [1.0; 3]);
    assert!(hard < 3.0, "a hard graduation is {hard} px wide");
    assert!(soft > 30.0, "a soft graduation is {soft} px wide");
    // A coloured graduation tints the dark side only.
    width(&mut rig, 0.3, [1.0, 0.6, 0.3]);
}

#[test]
fn t_stop_light_loss_scales_the_exposure() {
    let mut rig = Rig::new();
    let scene = flat([1.0; 3]);
    let mut lens = lens();
    lens.f_number = 2.8;
    lens.t_stop = 4.0;
    // Not applied until an element asks: t_stop is carried, not enforced.
    let frame = rig.record(&scene, Some(&[lens]), None);
    assert_eq!(frame.pixel(40, 40), [1.0; 3]);
    let element = [LensElementRow::t_stop_loss(1.0, LensRegion::FullFrame)];
    lens.element_count = 1;
    let frame = rig.record(&scene, Some(&[lens]), rows(&element));
    let expected = (2.8f32 / 4.0).powi(2);
    for c in 0..3 {
        assert!(
            (frame.pixel(40, 40)[c] - expected).abs() < 2.0e-3,
            "{:?}",
            frame.pixel(40, 40)
        );
    }
    // A T-stop equal to the f-number loses nothing.
    lens.t_stop = 2.8;
    let frame = rig.record(&scene, Some(&[lens]), rows(&element));
    assert!((frame.pixel(40, 40)[0] - 1.0).abs() < 1.0e-3);
}

#[test]
fn prism_copies_are_weighted_and_preserve_energy() {
    let mut rig = Rig::new();
    // One bright blob in the middle, far from the edges the copies reach.
    let scene = blobs(&[(63.5, 64.0)], 1.6);
    let reference = plain(&mut rig, &scene);
    let before: Vec<f32> = (0..3).map(|c| total(&reference, c)).collect();

    // Five copies in a line, one eighth of the frame apart.
    let spacing = 0.12;
    let element = [LensElementRow::prism(
        5,
        spacing,
        0.0,
        LensRegion::FullFrame,
    )];
    let frame = rig.record(&scene, Some(&[stacked(&element)]), rows(&element));
    for c in 0..3 {
        let after = total(&frame, c);
        assert!(
            (after / before[c] - 1.0).abs() < 0.005,
            "energy {} -> {after}",
            before[c]
        );
    }
    let peak = reference.pixel(63, 64)[1].max(reference.pixel(64, 64)[1]);
    for copy in 0..5 {
        let x = 63.5 + (copy as f32 - 2.0) * spacing * SIZE as f32;
        let found =
            frame.pixel(x.floor() as u32, 64)[1].max(frame.pixel(x.floor() as u32 + 1, 64)[1]);
        assert!(
            (found / (0.2 * peak) - 1.0).abs() < 0.15,
            "copy {copy} at {x}: {found} against a fifth of {peak}"
        );
    }

    // A ring of six, falling off, still sums to one; so does a kaleidoscope
    // that turns each copy.
    for element in [
        LensElementRow::prism(6, 0.2, 0.3, LensRegion::FullFrame)
            .with_prism_ring()
            .with_prism_falloff(0.7),
        LensElementRow::prism(4, 0.1, 0.0, LensRegion::FullFrame).with_prism_spin(0.4),
    ] {
        let frame = rig.record(&scene, Some(&[stacked(&[element])]), rows(&[element]));
        for c in 0..3 {
            let after = total(&frame, c);
            assert!(
                (after / before[c] - 1.0).abs() < 0.01,
                "ring energy {} -> {after}",
                before[c]
            );
        }
    }

    // Limited to the right half the prism leaves the left half alone, and
    // the energy still adds up.
    let scene = blobs(&[(30.0, 40.0), (96.0, 88.0)], 1.6);
    let reference = plain(&mut rig, &scene);
    let element = [LensElementRow::prism(
        3,
        0.1,
        0.0,
        LensRegion::HalfPlane {
            angle: 0.0,
            offset: 0.0,
        },
    )
    .with_feather(0.01)];
    let frame = rig.record(&scene, Some(&[stacked(&element)]), rows(&element));
    // The region `dot(q, (1, 0)) > 0` is the right half: the left is untouched.
    let left_changed = (0..SIZE)
        .flat_map(|y| (0..60).map(move |x| (x, y)))
        .any(|(x, y)| (frame.pixel(x, y)[1] - reference.pixel(x, y)[1]).abs() > 1.0e-3);
    assert!(
        !left_changed,
        "a prism limited to the right half changed the left"
    );
    let (after, before) = (total(&frame, 1), total(&reference, 1));
    assert!(
        (after / before - 1.0).abs() < 0.02,
        "region prism energy {before} -> {after}"
    );
}

#[test]
fn required_overscan_reports_how_far_the_map_reaches() {
    let mut rig = Rig::new();
    let scene = flat([1.0; 3]);
    let wide = |focal: f32| {
        let mut lens = lens();
        lens.focal_length_mm = focal;
        lens
    };
    struct Case {
        name: &'static str,
        lens: LensRow,
        elements: Vec<LensElementRow>,
        expect: fn(f32) -> bool,
    }
    let bc = |k1: f32| {
        LensElementRow::brown_conrady(
            [k1, 0.0, 0.0, 0.0, 0.0, 0.0],
            [0.0; 2],
            LensRegion::FullFrame,
        )
    };
    let cases = [
        Case {
            name: "no elements",
            lens: lens(),
            elements: vec![],
            expect: |o| (o - 1.0).abs() < 1.0e-5,
        },
        Case {
            name: "identity",
            lens: lens(),
            elements: vec![LensElementRow::projection(
                ProjectionModel::Rectilinear,
                LensRegion::FullFrame,
            )],
            expect: |o| (o - 1.0).abs() < 1.0e-4,
        },
        Case {
            name: "barrel",
            lens: lens(),
            elements: vec![bc(-0.5)],
            expect: |o| o > 1.2,
        },
        Case {
            name: "pincushion",
            lens: lens(),
            elements: vec![bc(0.4)],
            expect: |o| o == 1.0,
        },
        Case {
            name: "14 mm equidistant fisheye",
            lens: wide(14.0),
            elements: vec![LensElementRow::projection(
                ProjectionModel::Equidistant,
                LensRegion::FullFrame,
            )],
            expect: |o| o > 1.5,
        },
        Case {
            name: "2x squeeze",
            lens: lens(),
            elements: vec![LensElementRow::anamorphic_squeeze(
                2.0,
                LensRegion::FullFrame,
            )],
            expect: |o| (o - 2.0).abs() < 1.0e-3,
        },
        Case {
            name: "shift",
            lens: lens(),
            elements: vec![LensElementRow::shift(0.1, 0.0, LensRegion::FullFrame)],
            expect: |o| (o - 1.2).abs() < 1.0e-3,
        },
        Case {
            name: "lateral CA",
            lens: lens(),
            elements: vec![LensElementRow::lateral_ca(0.05, 0.0, LensRegion::FullFrame)],
            expect: |o| (o - 1.0 / 0.95).abs() < 1.0e-3,
        },
        Case {
            name: "prism",
            lens: lens(),
            elements: vec![LensElementRow::prism(3, 0.1, 0.0, LensRegion::FullFrame)],
            expect: |o| (o - 1.2).abs() < 1.0e-3,
        },
    ];
    for case in cases {
        let mut lens = case.lens;
        lens.element_count = case.elements.len() as u32;
        rig.record(
            &scene,
            Some(&[lens]),
            (!case.elements.is_empty()).then_some(&case.elements[..]),
        );
        let gpu = rig.resolved().required_overscan;
        let cpu = required_overscan(&lens, &case.elements, 1.0);
        assert!(
            (case.expect)(gpu),
            "{}: GPU overscan {gpu} (CPU {cpu})",
            case.name
        );
        assert!(
            (gpu - cpu).abs() < 2.0e-3 * cpu,
            "{}: GPU {gpu} and CPU {cpu} disagree",
            case.name
        );
    }
}

/// A smooth analytic image, so a map's resampling is predictable.
fn smooth_scene() -> Scene {
    Scene::new(
        |x, y| {
            let (u, v) = (
                (x as f32 + 0.5) / SIZE as f32,
                (y as f32 + 0.5) / SIZE as f32,
            );
            [
                0.5 + 0.4 * (6.0 * u).sin() * (4.0 * v).cos(),
                0.5 + 0.4 * (5.0 * u + 3.0 * v).cos(),
                0.4 + 0.3 * (7.0 * v - 2.0 * u).sin(),
            ]
        },
        |_, _| 3.0,
    )
}

/// The smooth scene sampled bilinearly at `uv`, clamped to its edge texels.
fn sample_scene(scene: &Scene, uv: [f32; 2]) -> [f32; 3] {
    let at = |x: i32, y: i32| {
        scene.color[(y.clamp(0, SIZE as i32 - 1) as u32 * SIZE + x.clamp(0, SIZE as i32 - 1) as u32)
            as usize]
    };
    let (px, py) = (
        uv[0].clamp(0.0, 1.0) * SIZE as f32 - 0.5,
        uv[1].clamp(0.0, 1.0) * SIZE as f32 - 0.5,
    );
    let (bx, by) = (px.floor(), py.floor());
    let (fx, fy) = (px - bx, py - by);
    let (x, y) = (bx as i32, by as i32);
    let (a, b, c, d) = (at(x, y), at(x + 1, y), at(x, y + 1), at(x + 1, y + 1));
    let mut out = [0.0; 3];
    for k in 0..3 {
        out[k] =
            (a[k] * (1.0 - fx) + b[k] * fx) * (1.0 - fy) + (c[k] * (1.0 - fx) + d[k] * fx) * fy;
    }
    out
}

/// The GPU image against the twin's map applied to the smooth scene.
fn assert_matches_the_twin(
    rig: &mut Rig,
    lens: LensRow,
    elements: &[LensElementRow],
    what: &str,
) -> Frame {
    let scene = smooth_scene();
    let frame = rig.record(&scene, Some(&[lens]), rows(elements));
    let map = RayMap::new(&lens, elements, 1.0);
    let mut worst = 0.0f32;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let q = [q_of(x as f32), q_of(y as f32)];
            let s = map.scene_position(q, 0.0, None);
            let expected = sample_scene(&scene, [s[0] + 0.5, s[1] + 0.5]);
            let found = frame.pixel(x, y);
            for c in 0..3 {
                worst = worst.max((found[c] - expected[c]).abs());
            }
        }
    }
    assert!(
        worst < 6.0e-3,
        "{what}: the GPU image differs from the twin's by {worst}"
    );
    frame
}

#[test]
fn every_map_element_matches_its_cpu_twin() {
    let mut rig = Rig::new();
    let full = LensRegion::FullFrame;
    let cases: Vec<(&str, LensRow, Vec<LensElementRow>)> = vec![
        (
            "shift",
            lens(),
            vec![LensElementRow::shift(0.07, -0.04, full)],
        ),
        (
            "squeeze",
            lens(),
            vec![LensElementRow::anamorphic_squeeze(1.5, full)],
        ),
        (
            "keystone",
            lens(),
            vec![LensElementRow::keystone(0.2, -0.3, full)],
        ),
        (
            "division",
            lens(),
            vec![LensElementRow::division(-0.4, 0.1, full)],
        ),
        (
            "anamorphic distortion",
            lens(),
            vec![LensElementRow::anamorphic_distortion(
                [-0.3, 0.0],
                [0.2, 0.0],
                full,
            )],
        ),
        (
            "rational brown-conrady",
            lens(),
            vec![LensElementRow::brown_conrady(
                [-0.2, 0.03, 0.0, 0.05, 0.0, 0.0],
                [0.01, 0.0],
                full,
            )],
        ),
        (
            "kannala-brandt",
            lens(),
            vec![
                LensElementRow::projection(ProjectionModel::KannalaBrandt, full)
                    .with_kannala_brandt([-0.02, 0.003, 0.0, 0.0]),
            ],
        ),
        (
            "panini",
            lens(),
            vec![LensElementRow::projection(ProjectionModel::Panini, full).with_panini(1.0, 0.8)],
        ),
        (
            "cylindrical",
            lens(),
            vec![LensElementRow::projection(
                ProjectionModel::Cylindrical,
                full,
            )],
        ),
        (
            "flat port",
            lens(),
            vec![LensElementRow::flat_port(1.333, 0.0, full)],
        ),
        (
            "dome port",
            lens(),
            vec![{
                let mut e = LensElementRow::dome_port(0.1, 0.04, full);
                e.params[dome_port::CURVATURE] = 0.0;
                e
            }],
        ),
        (
            "region-limited distortion",
            lens(),
            vec![LensElementRow::brown_conrady(
                [-0.3, 0.0, 0.0, 0.0, 0.0, 0.0],
                [0.0; 2],
                LensRegion::Ellipse {
                    center: [0.1, 0.0],
                    radii: [0.3, 0.25],
                    rotation: 0.4,
                },
            )
            .with_feather(0.1)],
        ),
        (
            "stack",
            lens(),
            vec![
                LensElementRow::shift(0.06, 0.02, full),
                LensElementRow::brown_conrady([-0.25, 0.0, 0.0, 0.0, 0.0, 0.0], [0.0; 2], full),
                LensElementRow::anamorphic_squeeze(1.3, full),
            ],
        ),
    ];
    for (name, mut lens, elements) in cases {
        lens.element_count = elements.len() as u32;
        assert_matches_the_twin(&mut rig, lens, &elements, name);
    }

    // The order of the stack matters.
    let shift = LensElementRow::shift(0.1, 0.0, full);
    let barrel = LensElementRow::brown_conrady([-0.5, 0.0, 0.0, 0.0, 0.0, 0.0], [0.0; 2], full);
    let a = [shift, barrel];
    let b = [barrel, shift];
    let (mut la, mut lb) = (lens(), lens());
    la.element_count = 2;
    lb.element_count = 2;
    let fa = assert_matches_the_twin(&mut rig, la, &a, "shift then barrel");
    let fb = assert_matches_the_twin(&mut rig, lb, &b, "barrel then shift");
    let differ = (0..SIZE)
        .flat_map(|y| (0..SIZE).map(move |x| (x, y)))
        .map(|(x, y)| (fa.pixel(x, y)[0] - fb.pixel(x, y)[0]).abs())
        .fold(0.0, f32::max);
    assert!(
        differ > 0.02,
        "swapping the elements changed nothing ({differ})"
    );
}

#[test]
fn outside_the_rendered_frame_the_image_clamps_to_its_edge() {
    let mut rig = Rig::new();
    let scene = Scene::new(|x, y| [x as f32 / 127.0, y as f32 / 127.0, 0.5], |_, _| 3.0);
    // The sensor looks 0.8 frames to the right: everything past the frame is
    // the last column.
    let element = [LensElementRow::shift(0.8, 0.0, LensRegion::FullFrame)];
    let frame = rig.record(&scene, Some(&[stacked(&element)]), rows(&element));
    for y in [10u32, 64, 120] {
        let edge = scene.color[(y * SIZE + SIZE - 1) as usize];
        for x in [64u32, 100, 127] {
            let found = frame.pixel(x, y);
            for c in 0..3 {
                assert!(
                    (found[c] - edge[c]).abs() < 2.0e-3,
                    "pixel {x},{y}: {found:?} against edge {edge:?}"
                );
            }
        }
    }
}

#[test]
fn a_flat_port_magnifies_by_the_refractive_index_and_disperses() {
    let mut rig = Rig::new();
    let n = 1.333f32;
    // Scene points on the rectilinear image. A ray leaving the water at
    // angle theta_w reaches the camera at sin(theta) = n sin(theta_w).
    let points = [(index_of(0.12), 64.0f32), (index_of(0.3), 64.0)];
    let scene = blobs(&points, 1.4);
    let element = [LensElementRow::flat_port(n, 0.0, LensRegion::FullFrame)];
    let frame = rig.record(&scene, Some(&[stacked(&element)]), rows(&element));
    for &(sx, sy) in &points {
        let x_scene = q_of(sx) * SCALE;
        let theta_w = x_scene.atan();
        let theta_air = (n * theta_w.sin()).asin();
        let expected = index_of(theta_air.tan() / SCALE);
        let (cx, _, mass) = centroid(&frame, (expected, sy), 6, 1);
        assert!(mass > 1.0);
        assert!(
            (cx - expected).abs() < 0.4,
            "flat port: found {cx:.2}, expected {expected:.2}"
        );
        // Magnified: further out than the same point without the port, and
        // by more than 1.25x near the axis.
        assert!(
            cx - 63.5 > (sx - 63.5) * 1.25,
            "no magnification: {cx} for {sx}"
        );
    }
    // Dispersion: the higher index of blue magnifies it more.
    let element = [LensElementRow::flat_port(n, 0.05, LensRegion::FullFrame)];
    let mut lens = stacked(&element);
    lens.quality = 1;
    let frame = rig.record(&scene, Some(&[lens]), rows(&element));
    // The point lands near the green position, with blue outside red.
    let green = {
        let theta_w = (q_of(points[1].0) * SCALE).atan();
        index_of((n * theta_w.sin()).asin().tan() / SCALE)
    };
    let (r, b) = (
        centroid(&frame, (green, 64.0), 8, 0).0,
        centroid(&frame, (green, 64.0), 8, 2).0,
    );
    assert!(
        b > r + 0.2,
        "blue should sit outside red: r {r:.2} b {b:.2}"
    );
}

#[test]
fn a_dome_port_refracts_off_centre_and_curves_the_focus() {
    let mut rig = Rig::new();
    let n = 1.333f32;
    // Centred (no pupil offset), the dome does not deviate any ray.
    let scene = smooth_scene();
    let mut centred = LensElementRow::dome_port(0.2, 0.0, LensRegion::FullFrame);
    centred.params[dome_port::CURVATURE] = 0.0;
    let reference = plain(&mut rig, &scene);
    let element = [centred];
    let frame = rig.record(&scene, Some(&[stacked(&element)]), rows(&element));
    let worst = (0..SIZE)
        .flat_map(|y| (0..SIZE).map(move |x| (x, y)))
        .map(|(x, y)| (frame.pixel(x, y)[0] - reference.pixel(x, y)[0]).abs())
        .fold(0.0, f32::max);
    assert!(
        worst < 2.0e-3,
        "a centred dome changed the image by {worst}"
    );

    // Off-centre: a point lands where the refraction at the glass puts it.
    let (radius, offset) = (0.1f32, 0.05f32);
    let points = [(index_of(0.3), 64.0f32)];
    let blob = blobs(&points, 1.4);
    let mut e = LensElementRow::dome_port(radius, offset, LensRegion::FullFrame);
    e.params[dome_port::CURVATURE] = 0.0;
    let element = [e];
    let frame = rig.record(&blob, Some(&[stacked(&element)]), rows(&element));
    let x_scene = q_of(points[0].0) * SCALE;
    let theta_w = x_scene.atan();
    // Invert theta_w = theta - i + asin(sin i / n), sin i = (e/R) sin(theta).
    let forward = |theta: f32| {
        let s = offset / radius * theta.sin();
        theta - s.asin() + (s / n).asin()
    };
    let (mut lo, mut hi) = (0.0f32, 1.4);
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        if forward(mid) < theta_w {
            lo = mid
        } else {
            hi = mid
        }
    }
    let expected = index_of(lo.tan() / SCALE);
    let (cx, _, mass) = centroid(&frame, (expected, 64.0), 6, 1);
    assert!(mass > 1.0);
    assert!(
        (cx - expected).abs() < 0.4,
        "dome: found {cx:.2}, expected {expected:.2}"
    );

    // The virtual image is a sphere: a flat wall in focus on the axis is
    // out of focus toward the corner. Camera: 0.8 rad vertical fov, square.
    let wall = Scene::new(Scene::checker, |_, _| 3.0);
    let element = [LensElementRow::dome_port(0.1, 0.0, LensRegion::FullFrame)];
    let frame = rig.record(&wall, Some(&[stacked(&element)]), rows(&element));
    let tan_half = (0.8f32 * 0.5).tan();
    let (cx, cy) = (3u32, 3u32);
    let d = [
        (q_of(cx as f32) * 2.0) * tan_half,
        (q_of(cy as f32) * 2.0) * tan_half,
    ];
    let secant = (1.0 + d[0] * d[0] + d[1] * d[1]).sqrt();
    let image_radius = 0.1 * n / (n - 1.0);
    let focus_dpt = 1.0 / 3.0 + (secant - 1.0) / image_radius;
    let expected_coc = expected_coc(&lens(), 1.0 / focus_dpt, 3.0);
    let found = frame.coc(cx, cy)[0];
    assert!(frame.coc(64, 64)[0].abs() < 0.03, "the axis stays in focus");
    assert!(
        expected_coc.abs() > 0.5 && (found - expected_coc).abs() < 0.1 * expected_coc.abs() + 0.05,
        "corner circle {found}, expected {expected_coc}"
    );
}

#[test]
fn focus_breathing_follows_the_focus_and_scales_the_circles() {
    let mut rig = Rig::new();
    // Magnification 1 + k (D - D_ref): k = -0.2 per dioptre.
    let points = [(index_of(0.3), 64.0f32)];
    let scene = blobs(&points, 1.4);
    let element = [LensElementRow::focus_breathing(-0.2, LensRegion::FullFrame)];
    for (focus, expected_scale) in [(2.0f32, 0.9f32), (0.5, 0.6)] {
        let mut lens = stacked(&element);
        lens.focus_distance_m = focus;
        // The blob sits at 3 m: well out of focus when the focus is close, so
        // the position is checked on the centroid of the blur.
        let frame = rig.record(&scene, Some(&[lens]), rows(&element));
        let expected = index_of(0.3 * expected_scale);
        let (cx, _, mass) = centroid(&frame, (expected, 64.0), 14, 1);
        assert!(mass > 1.0);
        assert!(
            (cx - expected).abs() < 0.6,
            "focus {focus}: found {cx:.2}, expected {expected:.2}"
        );
    }

    // The circle of confusion is on the sensor; the gather runs on the
    // scene, so a map that zooms in by 2 halves the scene circle. A wall at
    // 2 m seen by a 100 mm f/1.4 lens focused on 1 m: magnification
    // 1 + 1.0 * (1 - 0).
    let mut lens = lens();
    lens.focal_length_mm = 100.0;
    lens.f_number = 1.4;
    lens.focus_distance_m = 1.0;
    lens.max_coc_px = 64.0;
    let wall = Scene::new(Scene::checker, |_, _| 2.0);
    let before = rig.record(&wall, Some(&[lens]), None).coc(64, 64)[0];
    let element = [LensElementRow::focus_breathing(1.0, LensRegion::FullFrame)];
    lens.element_count = 1;
    let after = rig.record(&wall, Some(&[lens]), rows(&element)).coc(64, 64)[0];
    assert!(before > 3.0, "{before}");
    assert!(
        (after / before - 0.5).abs() < 0.03,
        "circle {before} became {after}, expected half"
    );
}

#[test]
fn transmission_elements_apply_in_their_regions() {
    let mut rig = Rig::new();
    let scene = flat([1.0; 3]);

    // Neutral density and a colour filter.
    let element = [
        LensElementRow::nd_filter(2.0, LensRegion::FullFrame),
        LensElementRow::color_filter([1.0, 0.5, 0.25], 1.0, LensRegion::FullFrame),
    ];
    let frame = rig.record(&scene, Some(&[stacked(&element)]), rows(&element));
    let found = frame.pixel(40, 70);
    for (c, expected) in [0.25, 0.125, 0.0625].into_iter().enumerate() {
        assert!((found[c] - expected).abs() < 1.0e-3, "{found:?}");
    }

    // A circular hood: open inside, black outside, half at the rim.
    let element = [LensElementRow::circular_hood(
        0.35,
        0.04,
        LensRegion::FullFrame,
    )];
    let frame = rig.record(&scene, Some(&[stacked(&element)]), rows(&element));
    assert!(frame.pixel(64, 64)[1] > 0.995 && frame.pixel(3, 3)[1] < 0.005);
    let rim = index_of(0.35);
    let at_rim = frame.pixel(rim.floor() as u32, 64)[1];
    assert!((at_rim - 0.5).abs() < 0.2, "rim {at_rim}");
    // A matte box cuts the corners but not the middle of the edges.
    let element = [LensElementRow::matte_box(
        [0.45, 0.45],
        0.0,
        0.02,
        LensRegion::FullFrame,
    )];
    let frame = rig.record(&scene, Some(&[stacked(&element)]), rows(&element));
    assert!(
        frame.pixel(64, 64)[1] > 0.99 && frame.pixel(2, 64)[1] < 0.2,
        "{}",
        frame.pixel(2, 64)[1]
    );

    // Optical vignetting is the area of the iris the barrel leaves: the unit
    // circle against one displaced toward the axis by the field radius.
    let element = [LensElementRow::optical_vignette(
        1.0,
        1.0,
        0.0,
        LensRegion::FullFrame,
    )];
    let frame = rig.record(&scene, Some(&[stacked(&element)]), rows(&element));
    assert!(
        (frame.pixel(64, 64)[1] - 1.0).abs() < 8.0e-3,
        "centre loses light"
    );
    let corner_r = (q_of(0.0) * q_of(0.0) * 2.0).sqrt() / (0.5 * 2.0f32.sqrt());
    let expected = crate::ray_map::circle_overlap(corner_r, 1.0);
    assert!(expected < 0.7);
    assert!(
        (frame.pixel(0, 0)[1] - expected).abs() < 0.01,
        "corner {} against {expected}",
        frame.pixel(0, 0)[1]
    );
    // The barrel is a fixed size: stopped down from its reference f/2.8 to
    // f/5.6 against a smaller iris, the corner is clear again.
    let mut lens = stacked(&element);
    lens.f_number = 5.6;
    let element = [LensElementRow::optical_vignette(
        1.0,
        1.0,
        2.8,
        LensRegion::FullFrame,
    )];
    let frame = rig.record(&scene, Some(&[lens]), rows(&element));
    assert!(
        frame.pixel(0, 0)[1] > 0.99,
        "stopped down corner {}",
        frame.pixel(0, 0)[1]
    );
    lens.f_number = 2.8;
    let frame = rig.record(&scene, Some(&[lens]), rows(&element));
    assert!((frame.pixel(0, 0)[1] - expected).abs() < 0.01);
}

#[test]
fn the_twin_agrees_with_the_shader_on_transmission() {
    let mut rig = Rig::new();
    let scene = flat([1.0; 3]);
    let full = LensRegion::FullFrame;
    let element = [
        LensElementRow::projection(ProjectionModel::Equisolid, full),
        LensElementRow::natural_vignette(0.8, full),
        LensElementRow::optical_vignette(0.6, 0.9, 0.0, full),
        LensElementRow::circular_hood(0.5, 0.1, full),
        LensElementRow::graduated_nd(1.5, 0.4, 0.05, 0.3, [1.0, 0.8, 0.6], full),
        LensElementRow::nd_filter(
            0.5,
            LensRegion::HalfPlane {
                angle: 1.0,
                offset: 0.1,
            },
        )
        .with_feather(0.1),
        LensElementRow::color_filter([0.9, 1.0, 0.7], 0.5, full),
        LensElementRow::t_stop_loss(1.0, full),
    ];
    let mut lens = stacked(&element);
    lens.t_stop = 3.5;
    let frame = rig.record(&scene, Some(&[lens]), rows(&element));
    let map = RayMap::new(&lens, &element, 1.0);
    let mut worst = 0.0f32;
    for y in (0..SIZE).step_by(5) {
        for x in (0..SIZE).step_by(5) {
            let expected = map.transmission([q_of(x as f32), q_of(y as f32)], 3.5);
            let found = frame.pixel(x, y);
            for c in 0..3 {
                worst = worst.max((found[c] - expected[c]).abs());
            }
        }
    }
    assert!(
        worst < 4.0e-3,
        "transmission differs from the twin's by {worst}"
    );
}

#[test]
fn the_new_elements_are_built_with_the_documented_slots() {
    let full = LensRegion::FullFrame;
    let e = LensElementRow::projection(ProjectionModel::Panini, full).with_panini(0.5, 0.25);
    assert_eq!(
        (e.kind, e.params[..3].to_vec()),
        (element_kind::PROJECTION, vec![6.0, 0.5, 0.25])
    );
    let e = LensElementRow::projection(ProjectionModel::KannalaBrandt, full)
        .with_kannala_brandt([1.0, 2.0, 3.0, 4.0]);
    assert_eq!(e.params[..5].to_vec(), vec![5.0, 1.0, 2.0, 3.0, 4.0]);
    let e = LensElementRow::brown_conrady([1.0, 2.0, 3.0, 4.0, 5.0, 6.0], [7.0, 8.0], full);
    assert_eq!(
        (e.kind, e.params[..8].to_vec()),
        (
            element_kind::BROWN_CONRADY,
            (1..=8).map(|n| n as f32).collect::<Vec<_>>()
        )
    );
    let e = LensElementRow::prism(4, 0.1, 0.5, full)
        .with_prism_ring()
        .with_prism_spin(0.3)
        .with_prism_falloff(0.8);
    assert_eq!(e.params[..6].to_vec(), vec![4.0, 0.1, 0.5, 1.0, 0.3, 0.8]);
    let e = LensElementRow::graduated_nd(2.0, 1.0, 0.1, 0.3, [0.5, 0.6, 0.7], full);
    assert_eq!(
        e.params[..7].to_vec(),
        vec![2.0, 1.0, 0.1, 0.3, 0.5, 0.6, 0.7]
    );
    assert_eq!(
        LensElementRow::lateral_ca(0.1, 0.02, full).params[..2].to_vec(),
        vec![0.1, 0.02]
    );
    assert_eq!(
        LensElementRow::dome_port(0.1, 0.02, full).params[dome_port::CURVATURE],
        1.0
    );
    // Every kind is distinct and in the documented order.
    let kinds = [
        element_kind::PROJECTION,
        element_kind::BROWN_CONRADY,
        element_kind::DIVISION,
        element_kind::ANAMORPHIC_DISTORTION,
        element_kind::ANAMORPHIC_SQUEEZE,
        element_kind::SHIFT,
        element_kind::KEYSTONE,
        element_kind::FOCUS_BREATHING,
        element_kind::PRISM,
        element_kind::LATERAL_CA,
        element_kind::FLAT_PORT,
        element_kind::DOME_PORT,
        element_kind::NATURAL_VIGNETTE,
        element_kind::OPTICAL_VIGNETTE,
        element_kind::MECHANICAL_VIGNETTE,
        element_kind::ND_FILTER,
        element_kind::GRADUATED_ND,
        element_kind::COLOR_FILTER,
        element_kind::T_STOP_LOSS,
    ];
    assert!(kinds.iter().enumerate().all(|(i, &k)| k == 5 + i as u32));
}
