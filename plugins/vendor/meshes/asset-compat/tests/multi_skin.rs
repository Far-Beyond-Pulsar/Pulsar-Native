use helio_asset_compat::{load_scene_bytes_with_config, LoadConfig};

const FIXTURE: &[u8] = include_bytes!("fixtures/multi_skin.gltf");

#[test]
fn shared_mesh_keeps_each_skin_palette_inverse_binds_and_animation_targets() {
    for merge_meshes in [true, false] {
        let scene = load_scene_bytes_with_config(
            FIXTURE,
            "gltf",
            None,
            LoadConfig {
                merge_meshes,
                ..LoadConfig::default()
            },
        )
        .unwrap();
        assert_eq!(scene.rigs.len(), 2);
        let a = &scene.rigs[0];
        let b = &scene.rigs[1];
        assert_eq!((a.source_skin, b.source_skin), (0, 1));
        assert_eq!(a.name, b.name, "skin names are deliberately identical");
        for rig in &scene.rigs {
            assert_eq!(rig.joints.len(), 9);
            assert_eq!(rig.lods.len(), 1);
            assert_eq!(
                rig.lods[0].vertices.len(),
                6,
                "same-skin instances reuse geometry"
            );
            assert_eq!(rig.lods[0].sections.len(), 2);
            assert_eq!(rig.lods[0].sections[0].indices, [0, 1, 2]);
            assert_eq!(rig.lods[0].sections[1].indices, [3, 4, 5]);
            let vertex = rig.lods[0].vertices[0];
            assert_eq!(vertex.joints, [8, 6, 5, 4]);
            assert!((vertex.weights[0] - 0.625).abs() < 1e-6);
            assert!((vertex.weights.iter().sum::<f32>() - 1.0).abs() < 1e-6);
            assert_eq!(rig.animations.len(), 1);
            let track = &rig.animations[0].tracks[0];
            assert_eq!(rig.bones[track.bone].name, "A0");
        }
        assert_eq!(a.bones[a.joints[8].bone].name, "A8");
        assert_eq!(b.bones[b.joints[8].bone].name, "A0");
        assert_eq!(a.joints[0].inverse_bind[12], 0.0);
        assert_eq!(b.joints[0].inverse_bind[12], -20.0);
        assert_eq!(b.joints[8].inverse_bind[12], -2.0);
        assert_eq!(
            scene
                .parts
                .iter()
                .filter(|part| part.skin.is_none())
                .count(),
            1
        );
        let plain = scene.parts.iter().find(|part| part.skin.is_none()).unwrap();
        let positions: Vec<_> = plain
            .sections
            .iter()
            .flat_map(|section| &section.indices)
            .map(|&index| plain.vertices[index as usize].position)
            .collect();
        assert_eq!(
            positions,
            [
                [5.0, 0.0, 0.0],
                [6.0, 0.0, 0.0],
                [5.0, 1.0, 0.0],
                [5.0, 0.0, 0.0],
                [6.0, 0.0, 0.0],
                [5.0, 1.0, 0.0]
            ]
        );
    }
}
