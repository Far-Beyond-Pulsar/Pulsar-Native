//! Overscan on the GPU: a camera renders `overscan` times the frame's tangent
//! extent, the lens row carries the factor, and the pass maps the frame into
//! the wider image. The scenes are linear functions of the scene position, so
//! bilinear sampling is exact and the expected image is the function at the
//! CPU twin's scene position of every pixel.

#![allow(clippy::needless_range_loop)]

use super::*;
use crate::ray_map::{applied_overscan, required_overscan, RayMap, APPLIED_OVERSCAN_CAP};

/// A 35 mm lens on the square frame with an equidistant fisheye projection
/// over the whole frame: a map that looks past the frame, by a modest amount.
fn fisheye() -> (LensRow, [LensElementRow; 1]) {
    let mut lens = lens();
    lens.focal_length_mm = 35.0;
    lens.element_count = 1;
    (
        lens,
        [LensElementRow::projection(
            ProjectionModel::Equidistant,
            LensRegion::FullFrame,
        )],
    )
}

/// The scene function: linear in the scene position `s` (image-height units
/// about the frame centre), positive over the whole overscanned view.
fn radiance(s: [f32; 2]) -> [f32; 3] {
    [1.5 + s[0], 1.5 + s[1], 1.5 + 0.5 * (s[0] + s[1])]
}

/// The image a camera widened by `overscan` renders of [`radiance`]: pixel
/// `(x, y)` shows the scene position `((x + .5) / SIZE - .5) * overscan`.
fn scene(overscan: f32) -> Scene {
    Scene::new(
        |x, y| {
            radiance([
                ((x as f32 + 0.5) / SIZE as f32 - 0.5) * overscan,
                ((y as f32 + 0.5) / SIZE as f32 - 0.5) * overscan,
            ])
        },
        |_, _| 3.0,
    )
}

fn with_overscan(mut lens: LensRow, overscan: f32) -> LensRow {
    lens.overscan = overscan;
    lens
}

fn q_of(i: u32) -> f32 {
    (i as f32 + 0.5) / SIZE as f32 - 0.5
}

/// Largest channel difference between the frame and [`radiance`] at the CPU
/// twin's scene position of each pixel, over the pixels `keep` accepts.
fn worst_error(frame: &Frame, map: &RayMap, keep: impl Fn(u32, u32) -> bool) -> (f32, (u32, u32)) {
    let (mut worst, mut at) = (0.0f32, (0, 0));
    for y in 0..SIZE {
        for x in 0..SIZE {
            if !keep(x, y) {
                continue;
            }
            let expected = radiance(map.scene_position([q_of(x), q_of(y)], 0.0, None));
            let got = frame.pixel(x, y);
            let error = (0..3)
                .map(|c| (got[c] - expected[c]).abs())
                .fold(0.0, f32::max);
            if error > worst {
                (worst, at) = (error, (x, y));
            }
        }
    }
    (worst, at)
}

#[test]
fn a_lens_inside_the_frame_applies_no_overscan() {
    let identity = lens();
    assert_eq!(applied_overscan(&identity, &[], 1.0, &[]), 1.0);
    // Elements that never look past the frame (a vignette, a filter) too.
    let mut filtered = lens();
    filtered.element_count = 1;
    let nd = [LensElementRow::nd_filter(2.0, LensRegion::FullFrame)];
    assert_eq!(applied_overscan(&filtered, &nd, 1.0, &[]), 1.0);
    // A lens that is off asks for none, whatever its stack.
    let (mut off, stack) = fisheye();
    off.enabled = 0;
    assert_eq!(applied_overscan(&off, &stack, 1.0, &[]), 1.0);
}

#[test]
fn a_fisheye_asks_for_overscan_and_the_cap_bounds_it() {
    let (lens, stack) = fisheye();
    let required = required_overscan(&lens, &stack, 1.0);
    assert!(
        required > 1.05 && required < APPLIED_OVERSCAN_CAP,
        "{required}"
    );
    assert_eq!(applied_overscan(&lens, &stack, 1.0, &[]), required);
    // An 8 mm fisheye looks far past any frame: reported, but applied capped.
    let mut wide = lens;
    wide.focal_length_mm = 8.0;
    assert!(required_overscan(&wide, &stack, 1.0) > APPLIED_OVERSCAN_CAP);
    assert_eq!(
        applied_overscan(&wide, &stack, 1.0, &[]),
        APPLIED_OVERSCAN_CAP
    );
}

#[test]
fn the_resolved_lens_carries_the_overscan_it_was_given() {
    let (lens, stack) = fisheye();
    let overscan = applied_overscan(&lens, &stack, 1.0, &[]);
    let mut rig = Rig::new();
    let _ = rig.record(
        &scene(overscan),
        Some(&[with_overscan(lens, overscan)]),
        Some(&stack),
    );
    let resolved = rig.resolved();
    assert_eq!(resolved.overscan, overscan);
    // The GPU measures the same reach the camera widened by.
    assert!((resolved.required_overscan - overscan).abs() < 1.0e-3);
    // Out-of-range values are sanitised: a row cannot ask for more than the cap,
    // and a zeroed or bad row is no overscan.
    for (asked, expected) in [
        (0.0, 1.0),
        (0.5, 1.0),
        (f32::NAN, 1.0),
        (9.0, APPLIED_OVERSCAN_CAP),
    ] {
        let _ = rig.record(
            &scene(1.0),
            Some(&[with_overscan(lens, asked)]),
            Some(&stack),
        );
        assert_eq!(rig.resolved().overscan, expected, "asked {asked}");
    }
    // No lens: the neutral 1.
    let _ = rig.record(&scene(1.0), None, None);
    assert_eq!(rig.resolved().overscan, 1.0);
}

#[test]
fn an_overscanned_fisheye_fills_the_frame_with_the_real_view() {
    let (lens, stack) = fisheye();
    let overscan = applied_overscan(&lens, &stack, 1.0, &[]);
    assert!(overscan > 1.05);
    let map = RayMap::new(&lens, &stack, 1.0);
    let mut rig = Rig::new();

    let wide = rig.record(
        &scene(overscan),
        Some(&[with_overscan(lens, overscan)]),
        Some(&stack),
    );
    let (error, at) = worst_error(&wide, &map, |_, _| true);
    assert!(error < 4.0e-3, "overscanned: worst error {error} at {at:?}");

    // The same lens on an image that is only the frame clamps at the edges:
    // the corners read the edge pixel instead of the view beyond it.
    let narrow = rig.record(&scene(1.0), Some(&[lens]), Some(&stack));
    let corner = (SIZE - 1, SIZE - 1);
    let (error, _) = worst_error(&narrow, &map, |x, y| (x, y) == corner);
    assert!(
        error > 0.03,
        "without overscan the corner clamps: error {error}"
    );
    // Away from the edges the two agree where the view is the same; the
    // centre in particular is unmoved.
    let centre = (SIZE / 2, SIZE / 2);
    let (a, b) = (
        wide.pixel(centre.0, centre.1),
        narrow.pixel(centre.0, centre.1),
    );
    for c in 0..3 {
        assert!((a[c] - b[c]).abs() < 4.0e-3, "centre {a:?} vs {b:?}");
    }
}

#[test]
fn overscan_alone_crops_to_the_frame_without_a_map() {
    // A lens with no ray-map element: the image the camera widened is mapped
    // back to the frame, so the output is the scene at the frame's own positions.
    let mut rig = Rig::new();
    for overscan in [1.5, 2.0] {
        let frame = rig.record(
            &scene(overscan),
            Some(&[with_overscan(lens(), overscan)]),
            None,
        );
        let identity = RayMap::new(&lens(), &[], 1.0);
        let (error, at) = worst_error(&frame, &identity, |_, _| true);
        assert!(
            error < 4.0e-3,
            "overscan {overscan}: worst error {error} at {at:?}"
        );
        assert_ne!(
            rig.resolved().map_flags & MAP_REMAP,
            0,
            "an overscanned image is always mapped"
        );
    }
}

#[test]
fn a_feature_at_the_frame_centre_stays_at_the_same_pixel() {
    let (lens, stack) = fisheye();
    let overscan = applied_overscan(&lens, &stack, 1.0, &[]);
    let mut rig = Rig::new();
    // A bright disc at the scene's centre, 3 scene-heights wide in the frame's
    // own units whatever the overscan.
    let spot = |overscan: f32| {
        Scene::new(
            |x, y| {
                let s = [
                    ((x as f32 + 0.5) / SIZE as f32 - 0.5) * overscan,
                    ((y as f32 + 0.5) / SIZE as f32 - 0.5) * overscan,
                ];
                let v = if s[0].hypot(s[1]) < 0.03 { 10.0 } else { 0.1 };
                [v; 3]
            },
            |_, _| 3.0,
        )
    };
    let centroid = |frame: &Frame| {
        let (mut sum, mut sx, mut sy) = (0.0f32, 0.0f32, 0.0f32);
        for y in 0..SIZE {
            for x in 0..SIZE {
                let v = (frame.pixel(x, y)[1] - 0.1).max(0.0);
                sum += v;
                sx += v * (x as f32 + 0.5);
                sy += v * (y as f32 + 0.5);
            }
        }
        [sx / sum, sy / sum]
    };
    let widened = rig.record(
        &spot(overscan),
        Some(&[with_overscan(lens, overscan)]),
        Some(&stack),
    );
    let plain = rig.record(&spot(1.0), Some(&[lens]), Some(&stack));
    for frame in [&widened, &plain] {
        let [x, y] = centroid(frame);
        assert!(
            (x - SIZE as f32 / 2.0).abs() < 0.1 && (y - SIZE as f32 / 2.0).abs() < 0.1,
            "({x}, {y})"
        );
    }
}

#[test]
fn circles_of_confusion_are_those_of_the_frame_not_of_the_wider_image() {
    // One sensor width is the frame, which is 1 / overscan of the image: the
    // same blur is that many fewer pixels across.
    let mut near = lens();
    near.focus_distance_m = 3.0;
    let flat = Scene::new(|_, _| [1.0; 3], |_, _| 1.5);
    let mut rig = Rig::new();
    let plain = rig.record(&flat, Some(&[near]), None);
    let widened = rig.record(&flat, Some(&[with_overscan(near, 2.0)]), None);
    let c = (SIZE / 2, SIZE / 2);
    let (a, b) = (plain.coc(c.0, c.1)[0], widened.coc(c.0, c.1)[0]);
    assert!(a.abs() > 1.0, "the plain circle is measurable ({a})");
    assert!((b - a / 2.0).abs() < 0.05 * a.abs(), "{b} vs {a} / 2");
}

#[test]
fn element_regions_are_in_the_frame_not_in_the_wider_image() {
    // A diopter over the frame's central ellipse (radius 0.25): a pixel 0.2
    // image heights from the centre is inside it on a plain image and, with the
    // image twice the frame, 0.4 frame heights out - outside.
    let mut lens = lens();
    lens.element_count = 1;
    let diopter = [LensElementRow::diopter(
        0.5,
        LensRegion::Ellipse {
            center: [0.0, 0.0],
            radii: [0.25, 0.25],
            rotation: 0.0,
        },
    )];
    let flat = Scene::new(|_, _| [1.0; 3], |_, _| 3.0);
    let mut rig = Rig::new();
    let probe = (SIZE / 2 + (0.2 * SIZE as f32) as u32, SIZE / 2);
    let plain = rig.record(&flat, Some(&[lens]), Some(&diopter));
    assert!(
        plain.coc(probe.0, probe.1)[0].abs() > 1.0,
        "inside the diopter the focus moves"
    );
    let widened = rig.record(&flat, Some(&[with_overscan(lens, 2.0)]), Some(&diopter));
    assert!(
        widened.coc(probe.0, probe.1)[0].abs() < 0.1,
        "outside it the 3 m plane stays in focus"
    );
    let near_centre = (SIZE / 2 + (0.05 * SIZE as f32) as u32, SIZE / 2);
    assert!(
        widened.coc(near_centre.0, near_centre.1)[0].abs() > 1.0,
        "the centre is still inside"
    );
}
