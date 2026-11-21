use mocap_capture::{arrange, rig_from_placements, Arrangement};
use nalgebra::Vector3;

fn cams(n: usize) -> Vec<(String, u32, u32)> {
    (0..n).map(|i| (format!("camera:{i}"), 1280, 720)).collect()
}

#[test]
fn every_arrangement_aims_each_camera_at_the_subject() {
    for arrangement in Arrangement::ALL {
        let rig = rig_from_placements(&arrange(arrangement, &cams(4), 3.0, 1.5, 1.0, 70.0)).unwrap();
        assert_eq!(rig.cameras.len(), 4);
        for c in &rig.cameras {
            let p = c.project(Vector3::new(0.0, 1.0, 0.0)).expect("in front");
            assert!((p[0] - 640.0).abs() < 1.0 && (p[1] - 360.0).abs() < 1.0, "{arrangement:?}: {p:?}");
            let head = c.project(Vector3::new(0.0, 1.7, 0.0)).unwrap();
            assert!(head[1] < 360.0, "up is up in the image");
        }
    }
}

#[test]
fn a_ring_spreads_the_views_and_a_single_camera_is_refused() {
    let rig = rig_from_placements(&arrange(Arrangement::Ring, &cams(4), 3.0, 1.5, 1.0, 70.0)).unwrap();
    assert!(rig.parallax_deg > 60.0, "{}", rig.parallax_deg);
    assert!(rig_from_placements(&arrange(Arrangement::Ring, &cams(1), 3.0, 1.5, 1.0, 70.0)).is_none());
}

#[test]
fn field_of_view_sets_the_focal_length() {
    let rig = rig_from_placements(&arrange(Arrangement::Line, &cams(2), 3.0, 1.5, 1.0, 90.0)).unwrap();
    assert!((rig.cameras[0].focal_px - 640.0).abs() < 1e-6);
}
