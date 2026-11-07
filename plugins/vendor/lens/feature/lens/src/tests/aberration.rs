//! Phase 4 on the GPU: in-focus aberrations, diffusion and star filters,
//! halation and contaminants, through the production WGSL.
//!
//! Scatterers are checked for what physics demands of them: the mean
//! radiance of the frame is unchanged, a star filter makes as many spikes as
//! it has points, coma points away from the axis, astigmatism's ellipse turns
//! with the sign. Scenes sit on the focal plane (3 m), so the defocus stages
//! change nothing and every difference comes from the aberration stage.

#![allow(clippy::needless_range_loop)]

use super::*;

fn stacked(elements: &[LensElementRow]) -> LensRow {
    let mut lens = lens();
    lens.element_count = elements.len() as u32;
    lens
}

/// A frame of `floor` with rectangles `(x, y, width, height, colour)` on it.
fn patches(floor: [f32; 3], rects: &[(u32, u32, u32, u32, [f32; 3])]) -> Scene {
    Scene::new(
        |x, y| {
            for &(rx, ry, w, h, color) in rects {
                if (rx..rx + w).contains(&x) && (ry..ry + h).contains(&y) {
                    return color;
                }
            }
            floor
        },
        |_, _| 3.0,
    )
}

/// A black frame with Gaussian blobs of the given peak at `(x, y)`.
fn blobs(points: &[(f32, f32, f32)], sigma: f32) -> Scene {
    Scene::new(
        |x, y| {
            let v: f32 = points
                .iter()
                .map(|&(px, py, peak)| {
                    let d2 = (x as f32 - px).powi(2) + (y as f32 - py).powi(2);
                    peak * (-d2 / (2.0 * sigma * sigma)).exp()
                })
                .sum();
            [v; 3]
        },
        |_, _| 3.0,
    )
}

fn pixels() -> impl Iterator<Item = (u32, u32)> {
    (0..SIZE).flat_map(|y| (0..SIZE).map(move |x| (x, y)))
}

fn mean(frame: &Frame, channel: usize) -> f32 {
    pixels()
        .map(|(x, y)| frame.pixel(x, y)[channel])
        .sum::<f32>()
        / (SIZE * SIZE) as f32
}

fn mean_of_scene(scene: &Scene, channel: usize) -> f32 {
    scene.color.iter().map(|c| c[channel]).sum::<f32>() / scene.color.len() as f32
}

fn record(rig: &mut Rig, scene: &Scene, elements: &[LensElementRow]) -> Frame {
    rig.record(
        scene,
        Some(&[stacked(elements)]),
        (!elements.is_empty()).then_some(elements),
    )
}

/// Mass, centroid and covariance (x, y, xy) of one channel above `floor`
/// in a window.
fn moments(
    frame: &Frame,
    at: (f32, f32),
    radius: i32,
    channel: usize,
    floor: f32,
) -> (f32, [f32; 2], [f32; 3]) {
    let (mut sum, mut sx, mut sy) = (0.0f32, 0.0f32, 0.0f32);
    let mut samples = Vec::new();
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let x = (at.0.round() as i32 + dx).clamp(0, SIZE as i32 - 1) as u32;
            let y = (at.1.round() as i32 + dy).clamp(0, SIZE as i32 - 1) as u32;
            let v = (frame.pixel(x, y)[channel] - floor).max(0.0);
            sum += v;
            sx += v * x as f32;
            sy += v * y as f32;
            samples.push((x as f32, y as f32, v));
        }
    }
    let (cx, cy) = (sx / sum, sy / sum);
    let (mut vxx, mut vyy, mut vxy) = (0.0, 0.0, 0.0);
    for (x, y, v) in samples {
        vxx += v * (x - cx) * (x - cx);
        vyy += v * (y - cy) * (y - cy);
        vxy += v * (x - cx) * (y - cy);
    }
    (sum, [cx, cy], [vxx / sum, vyy / sum, vxy / sum])
}

/// A scene with a bright and a dimmer coloured patch near the centre on a
/// dim floor: every scatterer here keeps its light inside the frame.
fn energy_scene() -> Scene {
    patches(
        [0.02, 0.025, 0.03],
        &[
            (59, 59, 10, 10, [8.0, 5.0, 3.0]),
            (50, 66, 6, 6, [2.0, 4.0, 8.0]),
        ],
    )
}

#[test]
fn without_aberration_elements_the_stage_is_idle_and_the_image_untouched() {
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

    let frame = record(&mut rig, &scene, &[]);
    assert_eq!(frame.output, input, "a plain lens changed the image");
    let resolved = rig.resolved();
    assert_eq!(resolved.aberration_flags, 0);
    let args = rig.args();
    assert!(
        args[18..].iter().all(|&n| n == 0),
        "the aberration stages must not dispatch: {args:?}"
    );

    // Other element kinds do not wake it either.
    let element = [LensElementRow::natural_vignette(0.0, LensRegion::FullFrame)];
    let frame = record(&mut rig, &scene, &element);
    assert_eq!(frame.output, input);
    assert_eq!(rig.resolved().aberration_flags, 0);
    assert!(rig.args()[18..].iter().all(|&n| n == 0));

    // An aberration element wakes it; with nothing to scatter it still
    // reproduces the image exactly.
    let element = [LensElementRow::diffusion(0.0, 0.01, LensRegion::FullFrame)];
    let frame = record(&mut rig, &scene, &element);
    assert_eq!(rig.resolved().aberration_flags, AB_ACTIVE);
    let args = rig.args();
    // A 128-pixel frame has a 64-pixel first level and seven levels in all; the
    // eighth slot stays empty.
    assert!(
        args[18..39].iter().all(|&n| n > 0),
        "the pyramid levels dispatch: {args:?}"
    );
    assert_eq!(args[39..42], [0, 0, 0]);
    assert!(
        args[42..45].iter().all(|&n| n > 0),
        "the filters dispatch: {args:?}"
    );
    assert_eq!(
        frame.output, input,
        "a zero-strength element changed the image"
    );

    // A disabled lens never runs it, even with the element rows present.
    let mut disabled = stacked(&element);
    disabled.enabled = 0;
    let frame = rig.record(&scene, Some(&[disabled]), Some(&element));
    assert_eq!(frame.output, input);
    assert!(rig.args()[18..].iter().all(|&n| n == 0));
}

#[test]
fn scattering_elements_preserve_mean_radiance() {
    let mut rig = Rig::new();
    let scene = energy_scene();
    let full = LensRegion::FullFrame;
    let cases: Vec<(&str, LensElementRow)> = vec![
        ("diffusion", LensElementRow::diffusion(0.4, 0.02, full)),
        ("pro-mist", LensElementRow::pro_mist(0.4, full)),
        ("black pro-mist", LensElementRow::black_pro_mist(0.4, full)),
        ("glimmerglass", LensElementRow::glimmerglass(0.4, full)),
        ("fog", LensElementRow::fog(0.4, full)),
        (
            "spherical glow",
            LensElementRow::spherical_glow(0.4, 2.8, 2.0, 0.02, full),
        ),
        (
            "star 4",
            LensElementRow::star_filter(4, 0.2, 0.3, 0.4, full),
        ),
        (
            "star 6",
            LensElementRow::star_filter(6, 0.0, 0.3, 0.4, full),
        ),
        (
            "star 8",
            LensElementRow::star_filter(8, 0.1, 0.3, 0.4, full),
        ),
        ("halation", LensElementRow::halation(0.5, 0.03, full)),
        ("smear", LensElementRow::smear(0.5, 0.05, 0.4, full)),
        ("dirt", LensElementRow::dirt(0.4, 0.02, 0.0, full)),
        ("coma", LensElementRow::coma(0.3, 0.2, full)),
    ];
    for (name, element) in cases {
        let frame = record(&mut rig, &scene, &[element]);
        for channel in 0..3 {
            let (before, after) = (mean_of_scene(&scene, channel), mean(&frame, channel));
            assert!(
                (after / before - 1.0).abs() < 0.03,
                "{name}: channel {channel} mean {before} -> {after}"
            );
        }
        // And it did something.
        let changed = pixels()
            .filter(|&(x, y)| frame.pixel(x, y) != scene.color[(y * SIZE + x) as usize])
            .count();
        assert!(
            changed > 200,
            "{name} left the image alone ({changed} pixels)"
        );
    }
}

#[test]
fn a_stack_of_scatterers_conserves_energy_too() {
    let mut rig = Rig::new();
    let scene = energy_scene();
    let full = LensRegion::FullFrame;
    let elements = [
        LensElementRow::pro_mist(0.3, full),
        LensElementRow::star_filter(4, 0.0, 0.3, 0.3, full),
        LensElementRow::halation(0.4, 0.03, full),
        LensElementRow::smear(0.3, 0.04, 1.0, full),
    ];
    let frame = record(&mut rig, &scene, &elements);
    for channel in 0..3 {
        let (before, after) = (mean_of_scene(&scene, channel), mean(&frame, channel));
        assert!(
            (after / before - 1.0).abs() < 0.03,
            "channel {channel}: {before} -> {after}"
        );
    }
}

/// Energy per angular bin (5 degrees) of the light of a point source in the
/// annulus `r0..r1` pixels, red channel above the dark floor.
fn angular_histogram(frame: &Frame, centre: (f32, f32), r0: f32, r1: f32) -> [f32; 72] {
    let mut bins = [0.0f32; 72];
    for (x, y) in pixels() {
        let (dx, dy) = (x as f32 - centre.0, y as f32 - centre.1);
        let r = (dx * dx + dy * dy).sqrt();
        if r < r0 || r > r1 {
            continue;
        }
        let angle = dy.atan2(dx).rem_euclid(std::f32::consts::TAU);
        let bin = ((angle / std::f32::consts::TAU * 72.0) as usize).min(71);
        bins[bin] += frame.pixel(x, y)[0] * r;
    }
    bins
}

/// Peaks of a circular histogram: bins at least `level` of the maximum that
/// beat their neighbours, as (bin centre angle, height).
fn peaks(bins: &[f32; 72], level: f32) -> Vec<(f32, f32)> {
    let smooth: Vec<f32> = (0..72)
        .map(|i| bins[(i + 71) % 72] + bins[i] + bins[(i + 1) % 72])
        .collect();
    let max = smooth.iter().cloned().fold(0.0, f32::max);
    (0..72)
        .filter(|&i| {
            let (prev, next) = (smooth[(i + 71) % 72], smooth[(i + 1) % 72]);
            smooth[i] >= level * max && smooth[i] > prev && smooth[i] >= next
        })
        .map(|i| ((i as f32 + 0.5) * std::f32::consts::TAU / 72.0, smooth[i]))
        .collect()
}

#[test]
fn a_star_filter_with_n_points_makes_n_spikes() {
    let mut rig = Rig::new();
    let scene = blobs(&[(64.0, 64.0, 60.0)], 1.0);
    for points in [2u32, 4, 6, 8] {
        for rotation in [0.0f32, 0.3] {
            let element = [LensElementRow::star_filter(
                points,
                rotation,
                0.45,
                0.5,
                LensRegion::FullFrame,
            )];
            let frame = record(&mut rig, &scene, &element);
            let found = peaks(&angular_histogram(&frame, (64.0, 64.0), 10.0, 52.0), 0.4);
            assert_eq!(
                found.len(),
                points as usize,
                "{points} points, rotation {rotation}: {found:?}"
            );
            // Each spike points where it was asked to.
            for k in 0..points {
                let wanted = (rotation + std::f32::consts::TAU * k as f32 / points as f32)
                    .rem_euclid(std::f32::consts::TAU);
                let nearest = found
                    .iter()
                    .map(|&(a, _)| {
                        let d = (a - wanted).abs();
                        d.min(std::f32::consts::TAU - d)
                    })
                    .fold(f32::MAX, f32::min);
                assert!(
                    nearest < 0.12,
                    "{points} points: no spike near {wanted} (nearest {nearest})"
                );
            }
        }
    }
    // Without the filter the light is round: no preferred direction.
    let frame = record(&mut rig, &scene, &[]);
    let found = peaks(&angular_histogram(&frame, (64.0, 64.0), 10.0, 52.0), 0.99);
    assert!(found.len() <= 1 || frame.pixel(80, 64)[0] < 1.0e-3);
}

#[test]
fn coma_drags_a_tail_away_from_the_axis() {
    let mut rig = Rig::new();
    // Points to the right of, above and diagonally from the axis, and a
    // control on it.
    let points = [(106.0, 64.0), (64.0, 22.0), (100.0, 100.0), (64.0, 64.0)];
    let scene = blobs(&points.map(|(x, y)| (x, y, 20.0)), 1.0);
    let element = [LensElementRow::coma(0.8, 0.5, LensRegion::FullFrame)];
    let frame = record(&mut rig, &scene, &element);
    let plain = record(&mut rig, &scene, &[]);
    for &(x, y) in &points[..3] {
        let (_, centre, _) = moments(&plain, (x, y), 22, 0, 0.0);
        let (_, shifted, _) = moments(&frame, (x, y), 22, 0, 0.0);
        let outward = {
            let (dx, dy) = (x - 64.0, y - 64.0);
            let n = (dx * dx + dy * dy).sqrt();
            [dx / n, dy / n]
        };
        let shift = [shifted[0] - centre[0], shifted[1] - centre[1]];
        let along = shift[0] * outward[0] + shift[1] * outward[1];
        let across = shift[0] * -outward[1] + shift[1] * outward[0];
        assert!(
            along > 2.0,
            "({x}, {y}): the tail must lead outward, shifted {shift:?}"
        );
        assert!(
            across.abs() < 0.3 * along,
            "({x}, {y}): the tail must be radial, along {along} across {across}"
        );
    }
    // On the axis there is no tail.
    let (_, centre, _) = moments(&plain, (64.0, 64.0), 22, 0, 0.0);
    let (_, shifted, _) = moments(&frame, (64.0, 64.0), 22, 0, 0.0);
    assert!((shifted[0] - centre[0]).abs() < 0.3 && (shifted[1] - centre[1]).abs() < 0.3);
}

#[test]
fn astigmatism_turns_the_ellipse_with_its_sign() {
    let mut rig = Rig::new();
    let at = (106.0f32, 90.0f32);
    let scene = blobs(&[(at.0, at.1, 20.0)], 1.0);
    let radial = {
        let (dx, dy) = (at.0 - 64.0, at.1 - 64.0);
        let n = (dx * dx + dy * dy).sqrt();
        [dx / n, dy / n]
    };
    let spread = |frame: &Frame| {
        let (_, _, [vxx, vyy, vxy]) = moments(frame, at, 22, 1, 0.0);
        // Variance along the radius and across it.
        let along = vxx * radial[0] * radial[0]
            + 2.0 * vxy * radial[0] * radial[1]
            + vyy * radial[1] * radial[1];
        let across = vxx * radial[1] * radial[1] - 2.0 * vxy * radial[0] * radial[1]
            + vyy * radial[0] * radial[0];
        (along, across)
    };
    let (plain_along, plain_across) = spread(&record(&mut rig, &scene, &[]));
    assert!(
        (plain_along - plain_across).abs() < 0.2,
        "the source is round"
    );
    let positive = [LensElementRow::astigmatism(
        0.15,
        0.2,
        LensRegion::FullFrame,
    )];
    let (along, across) = spread(&record(&mut rig, &scene, &positive));
    assert!(
        along > 2.0 * across && along > 2.0 * plain_along,
        "positive: along {along} across {across}"
    );
    let negative = [LensElementRow::astigmatism(
        -0.15,
        0.2,
        LensRegion::FullFrame,
    )];
    let (along, across) = spread(&record(&mut rig, &scene, &negative));
    assert!(
        across > 2.0 * along && across > 2.0 * plain_across,
        "negative: along {along} across {across}"
    );
}

#[test]
fn corner_softness_leaves_the_centre_sharp() {
    let mut rig = Rig::new();
    let scene = blobs(&[(64.0, 64.0, 20.0), (116.0, 116.0, 20.0)], 1.2);
    let plain = record(&mut rig, &scene, &[]);
    let element = [LensElementRow::corner_softness(
        0.04,
        0.3,
        LensRegion::FullFrame,
    )];
    let soft = record(&mut rig, &scene, &element);
    assert_eq!(
        soft.pixel(64, 64),
        plain.pixel(64, 64),
        "the axis is untouched"
    );
    let (peak_plain, peak_soft) = (plain.pixel(116, 116)[0], soft.pixel(116, 116)[0]);
    assert!(
        peak_soft < 0.5 * peak_plain,
        "the corner softens: {peak_plain} -> {peak_soft}"
    );
    // Blurring conserves the blob's light.
    let (mass_plain, ..) = moments(&plain, (116.0, 116.0), 11, 0, 0.0);
    let (mass_soft, ..) = moments(&soft, (116.0, 116.0), 11, 0, 0.0);
    assert!(
        (mass_soft / mass_plain - 1.0).abs() < 0.12,
        "{mass_plain} -> {mass_soft}"
    );
}

#[test]
fn spherical_glow_fades_as_the_lens_stops_down() {
    let mut rig = Rig::new();
    let scene = patches([0.0; 3], &[(60, 60, 8, 8, [10.0; 3])]);
    let element = [LensElementRow::spherical_glow(
        0.5,
        2.8,
        2.0,
        0.03,
        LensRegion::FullFrame,
    )];
    let halo = |rig: &mut Rig, f_number: f32| {
        let mut lens = stacked(&element);
        lens.f_number = f_number;
        let frame = rig.record(&scene, Some(&[lens]), Some(&element));
        // Light 10 to 24 pixels from the patch.
        pixels()
            .filter(|&(x, y)| {
                let d = (x as f32 - 63.5).abs().max((y as f32 - 63.5).abs());
                (14.0..28.0).contains(&d)
            })
            .map(|(x, y)| frame.pixel(x, y)[1])
            .sum::<f32>()
    };
    let wide = halo(&mut rig, 2.8);
    let stopped = halo(&mut rig, 5.6);
    let tiny = halo(&mut rig, 16.0);
    assert!(wide > 1.0, "the wide-open glow shows: {wide}");
    // (2.8 / 5.6)^2 = 1/4 and (2.8 / 16)^2 = 0.03 of the scattered light.
    assert!(
        (stopped / wide - 0.25).abs() < 0.08,
        "f/5.6 keeps a quarter: {}",
        stopped / wide
    );
    assert!(tiny / wide < 0.08, "f/16 is nearly clean: {}", tiny / wide);
}

#[test]
fn smear_spreads_along_its_direction() {
    let mut rig = Rig::new();
    let scene = blobs(&[(64.0, 64.0, 40.0)], 1.0);
    for angle in [0.0f32, std::f32::consts::FRAC_PI_2] {
        let element = [LensElementRow::smear(
            0.8,
            0.06,
            angle,
            LensRegion::FullFrame,
        )];
        let frame = record(&mut rig, &scene, &element);
        let (_, _, [vxx, vyy, _]) = moments(&frame, (64.0, 64.0), 30, 0, 0.0);
        let (along, across) = if angle == 0.0 { (vxx, vyy) } else { (vyy, vxx) };
        assert!(
            along > 4.0 * across,
            "angle {angle}: along {along} across {across}"
        );
    }
}

#[test]
fn black_pro_mist_lifts_the_shadows_less_than_pro_mist() {
    let mut rig = Rig::new();
    let floor = 0.02;
    let scene = patches([floor; 3], &[(56, 56, 16, 16, [6.0; 3])]);
    // Light far from the highlight, and just beside it.
    let lift = |frame: &Frame, near: bool| {
        let (mut sum, mut count) = (0.0, 0.0);
        for (x, y) in pixels() {
            let d = (x as f32 - 63.5).abs().max((y as f32 - 63.5).abs());
            if (near && (8.5..11.0).contains(&d)) || (!near && d >= 30.0) {
                sum += frame.pixel(x, y)[1] - floor;
                count += 1.0;
            }
        }
        sum / count
    };
    let pro = record(
        &mut rig,
        &scene,
        &[LensElementRow::pro_mist(0.4, LensRegion::FullFrame).with_feather(0.0)],
    );
    let black = record(
        &mut rig,
        &scene,
        &[LensElementRow::black_pro_mist(0.4, LensRegion::FullFrame).with_feather(0.0)],
    );
    let (pro_far, black_far) = (lift(&pro, false), lift(&black, false));
    assert!(
        black_far > 0.0 && pro_far > 1.4 * black_far,
        "far shadows: pro-mist {pro_far}, black {black_far}"
    );
    // Beside the highlight the black version keeps its halo.
    assert!(
        lift(&black, true) > 0.9 * lift(&pro, true),
        "near: black {} pro {}",
        lift(&black, true),
        lift(&pro, true)
    );
    // Both are the same share of the light, so the totals agree.
    for channel in 0..3 {
        assert!((mean(&pro, channel) / mean(&black, channel) - 1.0).abs() < 0.03);
    }
}

#[test]
fn halation_is_red_dominant() {
    let mut rig = Rig::new();
    let scene = patches([0.0; 3], &[(58, 58, 12, 12, [6.0; 3])]);
    let element = [LensElementRow::halation(0.6, 0.03, LensRegion::FullFrame)];
    let frame = record(&mut rig, &scene, &element);
    let mut halo = [0.0f32; 3];
    for (x, y) in pixels() {
        let d = (x as f32 - 63.5).abs().max((y as f32 - 63.5).abs());
        if (9.0..26.0).contains(&d) {
            for c in 0..3 {
                halo[c] += frame.pixel(x, y)[c];
            }
        }
    }
    assert!(
        halo[0] > 2.5 * halo[1] && halo[0] > 6.0 * halo[2],
        "halo {halo:?}"
    );
    assert!(halo[1] > halo[2], "orange before blue: {halo:?}");
    // The patch stays white-ish at its core; each channel's light is conserved.
    for channel in 0..3 {
        let (before, after) = (mean_of_scene(&scene, channel), mean(&frame, channel));
        assert!(
            (after / before - 1.0).abs() < 0.03,
            "channel {channel}: {before} -> {after}"
        );
    }
    // A custom tint works: a blue halation puts the halo in blue.
    let element = [LensElementRow::halation(0.6, 0.03, LensRegion::FullFrame)
        .with_halation_tint([0.0, 0.0, 1.0])];
    let frame = record(&mut rig, &scene, &element);
    let blue: f32 = pixels().map(|(x, y)| frame.pixel(x, y)[2]).sum();
    let red: f32 = pixels().map(|(x, y)| frame.pixel(x, y)[0]).sum();
    assert!(
        (red / blue - 1.0).abs() < 0.03,
        "totals stay equal: {red} {blue}"
    );
    assert!(
        frame.pixel(64, 64)[2] < frame.pixel(64, 64)[0],
        "the core loses blue to the halo"
    );
}

#[test]
fn dirt_loses_a_little_light_where_it_covers() {
    let mut rig = Rig::new();
    let scene = energy_scene();
    let element = [LensElementRow::dirt(0.3, 0.02, 0.1, LensRegion::FullFrame)];
    let frame = record(&mut rig, &scene, &element);
    for channel in 0..3 {
        let ratio = mean(&frame, channel) / mean_of_scene(&scene, channel);
        assert!(
            (ratio - 0.9).abs() < 0.03,
            "channel {channel}: kept {ratio}"
        );
    }
}

/// A 16 x 16 mask whose left half is black and right half white, in the red
/// channel (and alpha 255 throughout).
fn half_mask() -> Vec<u8> {
    (0..16 * 16)
        .flat_map(|i| {
            let v = if i % 16 < 8 { 0 } else { 255 };
            [v, 0, 0, 255]
        })
        .collect()
}

#[test]
fn a_mask_region_restricts_an_element_to_the_white_half() {
    let mut rig = Rig::new();
    rig.masks = Some(mask_array(&rig.device, &rig.queue, 16, &[&half_mask()]));
    let scene = blobs(&[(28.0, 64.0, 30.0), (100.0, 64.0, 30.0)], 2.0);
    let color = rig.color_view(&scene);
    let input = rig.read_texture(&color);
    let region = LensRegion::MaskChannel {
        layer: 0,
        channel: 0,
        invert: false,
    };
    let element = [LensElementRow::diffusion(0.8, 0.04, region)];
    let frame = record(&mut rig, &scene, &element);
    // The black half is exactly the input; the white half changed.
    for (x, y) in pixels().filter(|&(x, _)| x < 56) {
        let i = ((y * SIZE + x) * 4) as usize;
        assert_eq!(
            frame.output[i..i + 3],
            input[i..i + 3],
            "({x}, {y}) in the black half changed"
        );
    }
    let (peak_in, peak_out) = (
        scene.color[(64 * SIZE + 100) as usize][0],
        frame.pixel(100, 64)[0],
    );
    assert!(
        peak_out < 0.5 * peak_in,
        "the white half scatters: {peak_in} -> {peak_out}"
    );
    // Inverted, the roles swap.
    let region = LensRegion::MaskChannel {
        layer: 0,
        channel: 0,
        invert: true,
    };
    let element = [LensElementRow::diffusion(0.8, 0.04, region)];
    let frame = record(&mut rig, &scene, &element);
    for (x, y) in pixels().filter(|&(x, _)| x >= 72) {
        let i = ((y * SIZE + x) * 4) as usize;
        assert_eq!(
            frame.output[i..i + 3],
            input[i..i + 3],
            "({x}, {y}) in the inverted half changed"
        );
    }
    assert!(frame.pixel(28, 64)[0] < 0.5 * scene.color[(64 * SIZE + 28) as usize][0]);
    // Without a mask array bound the region covers nothing.
    rig.masks = None;
    let region = LensRegion::MaskChannel {
        layer: 0,
        channel: 0,
        invert: false,
    };
    let element = [LensElementRow::diffusion(0.8, 0.04, region)];
    let frame = record(&mut rig, &scene, &element);
    assert_eq!(frame.output, input);
}

#[test]
fn the_water_drop_generator_is_deterministic_and_well_formed() {
    let a = water_drop_mask(64, 6, [0.08, 0.16], 11);
    let b = water_drop_mask(64, 6, [0.08, 0.16], 11);
    let c = water_drop_mask(64, 6, [0.08, 0.16], 12);
    assert_eq!(a.len(), 64 * 64 * 4);
    assert_eq!(a, b, "the same seed must give the same drops");
    assert_ne!(a, c);
    let covered = a.as_chunks::<4>().0.iter().filter(|t| t[3] > 128).count();
    assert!(covered > 64, "drops have a footprint: {covered} texels");
    for t in a.as_chunks::<4>().0 {
        if t[3] == 0 {
            assert_eq!(t[..3], [128, 128, 0], "flat outside the drops");
        }
    }
    // The normal points outward from the drop's centre: a drop's right rim
    // leans right (red above one half), its left rim left.
    let single = water_drop_mask(64, 1, [0.2, 0.2], 3);
    let inside: Vec<(usize, usize)> = (0..64)
        .flat_map(|y| (0..64).map(move |x| (x, y)))
        .filter(|&(x, y)| single[(y * 64 + x) * 4 + 3] > 200)
        .collect();
    let cx = inside.iter().map(|p| p.0).sum::<usize>() as f32 / inside.len() as f32;
    for &(x, y) in &inside {
        let red = single[(y * 64 + x) * 4] as i32 - 128;
        if x as f32 > cx + 4.0 {
            assert!(red > 0, "right of the centre the normal leans right");
        } else if (x as f32) < cx - 4.0 {
            assert!(red < 0, "left of the centre the normal leans left");
        }
    }
}

#[test]
fn a_water_drop_displaces_content_inside_its_footprint_only() {
    const MASK: usize = 64;
    // A single drop near the middle of the frame (the generator is
    // deterministic, so the search is too).
    let (seed, mask) = (0u64..200)
        .map(|seed| (seed, water_drop_mask(MASK as u32, 1, [0.18, 0.18], seed)))
        .find(|(_, mask)| {
            let inside: Vec<(usize, usize)> = (0..MASK * MASK)
                .map(|i| (i % MASK, i / MASK))
                .filter(|&(x, y)| mask[(y * MASK + x) * 4 + 3] > 200)
                .collect();
            let (cx, cy) = (
                inside.iter().map(|p| p.0).sum::<usize>() / inside.len(),
                inside.iter().map(|p| p.1).sum::<usize>() / inside.len(),
            );
            (24..40).contains(&cx) && (24..40).contains(&cy)
        })
        .expect("a centred drop");
    let _ = seed;
    let mut rig = Rig::new();
    rig.masks = Some(mask_array(&rig.device, &rig.queue, MASK as u32, &[&mask]));
    // Brightness grows to the right: shifting the lookup shows as a change.
    let scene = Scene::new(|x, _| [0.1 + x as f32 * 0.05; 3], |_, _| 3.0);
    let element = [LensElementRow::water_drops(
        0.05,
        0,
        0.0,
        LensRegion::MaskChannel {
            layer: 0,
            channel: 3,
            invert: false,
        },
    )];
    let frame = record(&mut rig, &scene, &element);
    let texel = |x: u32, y: u32| {
        let (mx, my) = (
            (x as usize * MASK) / SIZE as usize,
            (y as usize * MASK) / SIZE as usize,
        );
        &mask[(my * MASK + mx) * 4..][..4]
    };
    let mut outside = 0;
    let mut right_ok = 0;
    let mut right = 0;
    let mut left_ok = 0;
    let mut left = 0;
    let mut sized = 0;
    for (x, y) in pixels() {
        let reference = scene.color[(y * SIZE + x) as usize].map(|v| f16_to_f32(f32_to_f16(v)));
        let out = frame.pixel(x, y);
        // Pixels whose neighbouring mask texels all lie outside every drop.
        let clear = (-2i32..=2).all(|dy| {
            (-2i32..=2).all(|dx| {
                let px = (x as i32 + dx * 2).clamp(0, SIZE as i32 - 1) as u32;
                let py = (y as i32 + dy * 2).clamp(0, SIZE as i32 - 1) as u32;
                texel(px, py)[3] == 0
            })
        });
        if clear {
            assert_eq!(
                out, reference,
                "({x}, {y}) lies outside every drop and must be untouched"
            );
            outside += 1;
            continue;
        }
        let t = texel(x, y);
        // Well inside the drop, away from its flat top and its rim.
        let nx = (t[0] as f32 - 127.5) / 127.5;
        let ny = (t[1] as f32 - 127.5) / 127.5;
        if t[3] == 255 && nx.abs() > 0.25 && nx.abs() < 0.8 && ny.abs() < 0.6 {
            let delta = out[0] - reference[0];
            if nx > 0.0 {
                right += 1;
                right_ok += (delta < -0.01) as u32;
            } else {
                left += 1;
                left_ok += (delta > 0.01) as u32;
            }
            // Displacement 0.05 image heights is 6.4 px, i.e. 0.32 brightness
            // at a unit normal.
            let expected = -nx * 0.32;
            sized += ((delta - expected).abs() < 0.12 + 0.3 * expected.abs()) as u32;
        }
    }
    assert!(
        outside > 5000,
        "most of the frame is clear glass: {outside}"
    );
    assert!(
        right > 30 && left > 30,
        "the drop's footprint was probed: {right} {left}"
    );
    assert!(
        right_ok as f32 > 0.9 * right as f32,
        "a drop inverts: {right_ok}/{right}"
    );
    assert!(
        left_ok as f32 > 0.9 * left as f32,
        "a drop inverts: {left_ok}/{left}"
    );
    assert!(
        sized as f32 > 0.8 * (left + right) as f32,
        "displacement follows the normal: {sized}/{}",
        left + right
    );
}
