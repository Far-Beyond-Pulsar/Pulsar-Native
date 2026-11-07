//! The first executable RT control: opaque world visibility, not a performance claim.
#![allow(dead_code)]
mod support;
use helio_core::{BlasGeometry, TlasInstanceInput};
use helio_pass_hlfs::{HlfsConfig, HlfsDebugMode, HlfsMode, HlfsPass};
use support::{mean, ortho_depth, point, Fixture};
use wgpu::util::DeviceExt;

fn transmission_buffer(device: &wgpu::Device, rows: &[[f32; 4]]) -> wgpu::Buffer {
    let mut bytes = bytemuck::cast_slice(&[0u32, rows.len() as u32, 0, 0]).to_vec();
    bytes.extend_from_slice(bytemuck::cast_slice(rows));
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("generic RT transmission"),
        contents: &bytes,
        usage: wgpu::BufferUsages::STORAGE,
    })
}

fn blocker(f: &mut Fixture, x: f32) {
    // Entirely offscreen to the right of the fixture's orthographic camera.
    let vertices = f
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("offscreen RT caster"),
            contents: bytemuck::cast_slice(&[
                x, -100.0f32, -100.0, x, 100.0, -100.0, x, 0.0, 100.0,
            ]),
            usage: wgpu::BufferUsages::BLAS_INPUT,
        });
    let mut encoder = f.device.create_command_encoder(&Default::default());
    f.scene
        .blas_manager
        .build_from_buffers(
            1,
            &mut encoder,
            BlasGeometry {
                revision: 0,
                vertices: &vertices,
                first_vertex: 0,
                vertex_count: 3,
                vertex_stride: 12,
                indices: None,
                first_index: 0,
                index_count: 0,
            },
        )
        .unwrap();
    f.scene
        .tlas_manager
        .build(
            &mut encoder,
            &[TlasInstanceInput {
                mesh_id: 1,
                transform: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            }],
            &f.scene.blas_manager,
        )
        .unwrap();
    f.queue.submit([encoder.finish()]);
}

fn empty_scene(f: &mut Fixture) {
    let mut encoder = f.device.create_command_encoder(&Default::default());
    f.scene
        .tlas_manager
        .build(&mut encoder, &[], &f.scene.blas_manager)
        .unwrap();
    f.queue.submit([encoder.finish()]);
}

fn light() -> helio_light_api::GpuLight {
    let mut light = point([10.0, 0.0, 2.0], [1.0, 1.0, 1.0], 10000.0);
    light.position_range[3] = 100.0;
    light.shadow_index = u32::MAX;
    light.set_ray_traced_shadows(true);
    light
}

#[test]
#[ignore = "requires Vulkan hardware ray queries"]
fn sparse_scenedb_light_slots_do_not_consume_the_sampling_budget() {
    pollster::block_on(async {
        for config in [
            HlfsConfig {
                mode: HlfsMode::RayTraced,
                ..Default::default()
            },
            HlfsConfig::ray_traced_presampled(),
        ] {
            let mut f = Fixture::new_rt(65, 49).await;
            f.config(config);
            empty_scene(&mut f);
            f.lights(vec![light()]);
            for _ in 0..8 {
                f.frame();
            }
            let dense = mean(&f.read());
            // Empty packed SceneDB rows have type zero (directional) and zero
            // power. The sole active light deliberately sits near the end.
            let mut inactive = light();
            inactive.color_intensity = [0.0; 4];
            inactive.light_type = 0;
            let mut sparse = vec![inactive; 64];
            sparse[61] = light();
            f.lights(sparse);
            for _ in 0..8 {
                f.frame();
            }
            let actual = mean(&f.read());
            assert!(
                dense > 1.0 && (actual / dense - 1.0).abs() < 0.01,
                "empty light slots changed illumination: {dense} -> {actual}"
            );
        }
    });
}

#[test]
#[ignore = "requires Vulkan hardware ray queries"]
fn offscreen_shadow_does_not_require_an_atlas_slot_and_removal_clears_it() {
    pollster::block_on(async {
        let mut f = Fixture::new_rt(65, 49).await;
        f.config(HlfsConfig {
            mode: HlfsMode::RayTraced,
            debug_mode: HlfsDebugMode::Reference,
            ..Default::default()
        });
        f.lights(vec![light()]);
        empty_scene(&mut f);
        f.frame();
        let lit = mean(&f.read());
        blocker(&mut f, 5.0);
        f.frame();
        let shadow = mean(&f.read());
        assert!(
            lit > 1.0 && shadow < lit * 0.05,
            "offscreen shadow without atlas: {lit} -> {shadow}"
        );
        // Moving the blocker beyond the light must not occlude the finite segment.
        blocker(&mut f, 15.0);
        f.frame();
        let beyond = mean(&f.read());
        assert!(
            (beyond / lit - 1.0).abs() < 0.01,
            "blocker beyond light: {beyond} vs {lit}"
        );
        empty_scene(&mut f);
        f.frame();
        assert!((mean(&f.read()) / lit - 1.0).abs() < 0.01);
        blocker(&mut f, 5.0);
        let mut unshadowed = light();
        unshadowed.set_ray_traced_shadows(false);
        f.lights(vec![unshadowed]);
        f.frame();
        assert!(
            (mean(&f.read()) / lit - 1.0).abs() < 0.01,
            "explicit unshadowed intent"
        );
    });
}

#[test]
#[ignore = "requires Vulkan hardware ray queries"]
fn reduced_resolution_repair_uses_hardware_visibility_in_every_phase() {
    pollster::block_on(async {
        let mut f = Fixture::new_rt(65, 49).await;
        f.config(HlfsConfig {
            mode: HlfsMode::RayTraced,
            ..HlfsConfig::compact()
        });
        f.lights(vec![light()]);
        let center = 24 * 65 + 32;
        let mut depths = vec![helio_core::depth::CLEAR; 65 * 49];
        depths[center] = ortho_depth(3.0);
        f.depth_values(&depths);
        empty_scene(&mut f);
        f.frame();
        let lit = mean(&[f.read()[center]]);
        blocker(&mut f, 5.0);
        for phase in 0..4 {
            f.graph
                .find_pass_mut::<HlfsPass>()
                .unwrap()
                .invalidate_history();
            f.frame();
            let shadow = mean(&[f.read()[center]]);
            assert!(
                lit > 1.0 && shadow < lit * 0.05,
                "phase {phase}: lit {lit}, shadow {shadow}"
            );
        }
    });
}

#[test]
#[ignore = "requires Vulkan hardware ray queries"]
fn small_directional_key_preserves_energy_with_empty_and_local_residuals() {
    pollster::block_on(async {
        let mut f = Fixture::new_rt(32, 24).await;
        empty_scene(&mut f);
        // Front-facing, back-facing and tangent keys must all agree with the
        // unextracted reference, including an otherwise empty local population.
        for (count, direction) in [1, 2, 17].into_iter().flat_map(|count| {
            [
                [0.0, 0.0, -1.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0, 0.0],
            ]
            .into_iter()
            .map(move |direction| (count, direction))
        }) {
            let mut lights = vec![point([1.0, 1.0, 2.0], [1.0; 3], 4.0); count];
            lights[0].light_type = 0;
            lights[0].direction_outer = direction;
            for light in &mut lights {
                light.set_ray_traced_shadows(true);
            }
            f.lights(lights);
            f.config(HlfsConfig {
                mode: HlfsMode::RayTraced,
                debug_mode: HlfsDebugMode::Reference,
                ..Default::default()
            });
            f.frame();
            let reference = f.read();
            f.config(HlfsConfig {
                sample_scale: 1,
                debug_mode: HlfsDebugMode::Unfiltered,
                ..HlfsConfig::ray_traced_presampled()
            });
            for frame in 0..4 {
                f.frame();
                for (actual, expected) in f.read().iter().zip(&reference) {
                    for c in 0..3 {
                        assert!((actual[c]-expected[c]).abs() < expected[c]*0.04+0.001,
                            "directional split count {count} frame {frame}: {actual:?}/{expected:?}");
                    }
                }
            }
        }
    });
}

#[test]
#[ignore = "requires Vulkan hardware ray queries"]
fn compact_light_populations_do_not_add_candidate_energy_noise() {
    pollster::block_on(async {
        let mut f = Fixture::new_rt(32, 24).await;
        empty_scene(&mut f);
        for count in [3, 17, 32] {
            // Co-located sources differ only in power: their summed energy is
            // independent of which shadow sample is selected. Randomly scoring
            // only a subset introduces avoidable estimator variance here.
            f.lights(
                (0..count)
                    .map(|i| {
                        let mut l = point([1.0, 1.0, 2.0], [1.0; 3], 1.0 + (i * i) as f32);
                        l.set_ray_traced_shadows(true);
                        l
                    })
                    .collect(),
            );
            f.config(HlfsConfig {
                mode: HlfsMode::RayTraced,
                debug_mode: HlfsDebugMode::Reference,
                ..Default::default()
            });
            f.frame();
            let reference = f.read();
            f.config(HlfsConfig {
                sample_scale: 1,
                debug_mode: HlfsDebugMode::Unfiltered,
                ..HlfsConfig::ray_traced_presampled()
            });
            for frame in 0..8 {
                f.frame();
                for (pixel, (actual, expected)) in f.read().iter().zip(&reference).enumerate() {
                    for c in 0..3 {
                        assert!((actual[c]-expected[c]).abs() < expected[c]*0.04+0.001,
                            "candidate noise, count {count} frame {frame} pixel {pixel}: {actual:?}/{expected:?}");
                    }
                }
            }
        }
    });
}

#[test]
#[ignore = "requires Vulkan hardware ray queries"]
fn uncovered_thin_edges_do_not_expose_small_population_sampling_noise() {
    pollster::block_on(async {
        for sparse_slots in [false, true] {
            let mut f = Fixture::new_rt(65, 49).await;
            empty_scene(&mut f);
            let center = 24 * 65 + 32;
            let mut depths = vec![helio_core::depth::CLEAR; 65 * 49];
            depths[center] = ortho_depth(3.0);
            f.depth_values(&depths);
            let active: Vec<_> = (0..17)
                .map(|i| {
                    let mut l = point(
                        [i as f32 - 8.0, 2.0, 2.0],
                        [0.2 + (i % 3) as f32 * 0.4, 0.7, 0.3],
                        20.0 + i as f32 * 30.0,
                    );
                    l.set_ray_traced_shadows(true);
                    l
                })
                .collect();
            // SceneDB uses sparse entity slots; allocation size is not population.
            let mut inactive = light();
            inactive.color_intensity = [0.0; 4];
            let mut sparse = vec![inactive; 4096];
            for (i, l) in active.into_iter().enumerate() {
                sparse[2000 + i * 37] = l;
            }
            f.lights(if sparse_slots {
                sparse
            } else {
                (0..17).map(|i| sparse[2000 + i * 37]).collect()
            });
            f.config(HlfsConfig {
                mode: HlfsMode::RayTraced,
                debug_mode: HlfsDebugMode::Reference,
                ..Default::default()
            });
            f.frame();
            let reference = f.read()[center];
            f.config(HlfsConfig::ray_traced_presampled());
            // This one-pixel surface is absent from the half-resolution samples
            // in phases 1/2/3. No history can hide the raw repair estimator's noise.
            for frame in (1..32).filter(|frame| frame % 4 != 0) {
                f.scene.frame_count = frame;
                f.graph
                    .find_pass_mut::<HlfsPass>()
                    .unwrap()
                    .invalidate_history();
                f.frame();
                let actual = f.read()[center];
                for channel in 0..3 {
                    assert!(
                        (actual[channel] - reference[channel]).abs()
                            < reference[channel] * 0.03 + 0.001,
                        "thin edge frame {frame}: {actual:?}, reference {reference:?}"
                    );
                }
            }
        }
    });
}

#[test]
#[ignore = "requires a GPU; ray-query support is intentionally not requested"]
fn unsupported_mode_change_is_rejected_without_changing_the_pass() {
    pollster::block_on(async {
        let mut f = Fixture::new(8, 8).await;
        let pass = f.graph.find_pass_mut::<HlfsPass>().unwrap();
        let output = pass.output_texture().clone();
        assert!(pass
            .try_set_config(
                &f.device,
                HlfsConfig {
                    mode: HlfsMode::RayTraced,
                    ..Default::default()
                }
            )
            .is_err());
        assert_eq!(pass.config().mode, HlfsMode::ScreenSpace);
        assert_eq!(pass.output_texture(), &output);
        f.frame();
    });
}

#[test]
#[ignore = "requires Vulkan hardware ray queries"]
fn missing_tlas_errors_and_mode_switch_preserves_output() {
    pollster::block_on(async {
        let mut f = Fixture::new_rt(8, 8).await;
        let output = f
            .graph
            .find_pass::<HlfsPass>()
            .unwrap()
            .output_texture()
            .clone();
        f.config(HlfsConfig {
            mode: HlfsMode::RayTraced,
            ..Default::default()
        });
        assert!(
            f.try_frame().is_err(),
            "capable device with missing TLAS must not render unoccluded"
        );
        empty_scene(&mut f);
        f.frame();
        f.config(HlfsConfig::default());
        f.frame();
        assert_eq!(
            f.graph.find_pass::<HlfsPass>().unwrap().output_texture(),
            &output
        );
    });
}

fn receiver_plane(f: &mut Fixture, z: f32) {
    let vertices = f
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("perspective receiver"),
            contents: bytemuck::cast_slice(&[
                -500.0f32, -500.0, z, 500.0, -500.0, z, 0.0, 500.0, z,
            ]),
            usage: wgpu::BufferUsages::BLAS_INPUT,
        });
    let mut encoder = f.device.create_command_encoder(&Default::default());
    f.scene
        .blas_manager
        .build_from_buffers(
            1,
            &mut encoder,
            BlasGeometry {
                revision: 0,
                vertices: &vertices,
                first_vertex: 0,
                vertex_count: 3,
                vertex_stride: 12,
                indices: None,
                first_index: 0,
                index_count: 0,
            },
        )
        .unwrap();
    f.scene
        .tlas_manager
        .build(
            &mut encoder,
            &[TlasInstanceInput {
                mesh_id: 1,
                transform: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            }],
            &f.scene.blas_manager,
        )
        .unwrap();
    f.queue.submit([encoder.finish()]);
}

#[test]
#[ignore = "requires Vulkan hardware ray queries"]
fn rasterized_depth_preserves_receiver_and_close_contact_shadows() {
    pollster::block_on(async {
        let mut f = Fixture::new_rt(65, 49).await;
        f.config(HlfsConfig {
            mode: HlfsMode::RayTraced,
            debug_mode: HlfsDebugMode::Reference,
            ..Default::default()
        });
        let proj = helio_core::depth::perspective(0.8, 65.0 / 49.0, 0.1);
        for camera in [
            glam::Vec3::new(2.0, 2.0, 6.0),
            glam::Vec3::new(2.0, 2.0, 24.0),
        ] {
            let view =
                glam::Mat4::look_at_rh(camera, glam::Vec3::new(0.0, 5.0, -20.0), glam::Vec3::Y);
            f.scene.camera.update(helio_core::GpuCameraUniforms::new(
                view,
                proj,
                camera,
                0.1,
                200.0,
                0,
                [0.0; 2],
                proj * view,
            ));
            let mut l = point(camera.to_array(), [1.0; 3], 100000.0);
            l.position_range[3] = 200.0;
            l.set_ray_traced_shadows(true);
            f.lights(vec![l]);
            for distance in [10.0, 30.0, 60.0, 100.0, 150.0] {
                let z = camera.z - distance;
                f.raster_plane_depth(view, proj, z);
                empty_scene(&mut f);
                f.scene.frame_count = 0;
                f.frame();
                let reference = f.read();
                receiver_plane(&mut f, z);
                f.scene.frame_count = 0;
                f.frame();
                for (index, (actual, expected)) in f.read().iter().zip(&reference).enumerate() {
                    for c in 0..3 {
                        assert!((actual[c] - expected[c]).abs() < expected[c] * 0.01 + 0.0001,
                            "raster self-shadow: camera {camera:?}, distance {distance}, pixel {index}: {actual:?}/{expected:?}");
                    }
                }
                receiver_plane(&mut f, z + 0.02);
                f.frame();
                assert!(
                    mean(&f.read()) < mean(&reference) * 0.05,
                    "lost raster contact shadow: camera {camera:?}, distance {distance}"
                );
            }
        }
    });
}

#[test]
#[ignore = "requires Vulkan hardware ray queries"]
fn perspective_depth_error_does_not_shadow_the_receiver_or_erase_nearby_blockers() {
    pollster::block_on(async {
        let mut f = Fixture::new_rt(32, 32).await;
        f.config(HlfsConfig {
            mode: HlfsMode::RayTraced,
            debug_mode: HlfsDebugMode::Reference,
            ..Default::default()
        });
        let proj = helio_core::depth::perspective(0.8, 1.0, 0.1);
        for camera in [glam::Vec3::ZERO, glam::Vec3::new(2.0, 2.0, 24.0)] {
            let view = if camera == glam::Vec3::ZERO {
                glam::Mat4::IDENTITY
            } else {
                glam::Mat4::look_at_rh(camera, glam::Vec3::new(0.0, 5.0, -20.0), glam::Vec3::Y)
            };
            let vp = proj * view;
            f.scene.camera.update(helio_core::GpuCameraUniforms::new(
                view, proj, camera, 0.1, 200.0, 0, [0.0; 2], vp,
            ));
            let mut light = point(camera.to_array(), [1.0; 3], 100000.0);
            light.position_range[3] = 200.0;
            light.set_ray_traced_shadows(true);
            f.lights(vec![light]);
            // Independently intersect camera rays with the known plane in f64,
            // then quantize the projected depth to the actual Depth32 format.
            let inverse = vp.as_dmat4().inverse();
            for distance in [10.0f32, 30.0, 60.0, 83.7, 100.0, 150.0] {
                let z = camera.z - distance;
                let mut depths = Vec::new();
                for y in 0..32 {
                    for x in 0..32 {
                        let h = inverse
                            * glam::DVec4::new(
                                (x as f64 + 0.5) / 16.0 - 1.0,
                                1.0 - (y as f64 + 0.5) / 16.0,
                                0.5,
                                1.0,
                            );
                        let direction = h.truncate() / h.w - camera.as_dvec3();
                        let world = camera.as_dvec3()
                            + direction * ((z as f64 - camera.z as f64) / direction.z);
                        let clip = vp.as_dmat4() * world.extend(1.0);
                        depths.push((clip.z / clip.w) as f32);
                    }
                }
                f.depth_values(&depths);
                empty_scene(&mut f);
                // Match stochastic HDR rounding as well as geometry between
                // the empty-TLAS control and receiver measurement.
                f.scene.frame_count = 0;
                f.frame();
                let lit_pixels = f.read();
                let lit = mean(&lit_pixels);
                receiver_plane(&mut f, z);
                f.scene.frame_count = 0;
                f.frame();
                let receiver_pixels = f.read();
                let receiver = mean(&receiver_pixels);
                for (index, (expected, actual)) in
                    lit_pixels.iter().zip(&receiver_pixels).enumerate()
                {
                    for channel in 0..3 {
                        assert!((expected[channel]-actual[channel]).abs()<=expected[channel]*0.01+0.0001,
                            "self-shadowed pixel {index} channel {channel}, distance {distance}, camera {camera:?}: {actual:?}/{expected:?}");
                    }
                }
                eprintln!("camera={camera:?} distance={distance} lit={lit} receiver={receiver}");
                assert!(
                    (receiver - lit).abs() < lit * 0.01,
                    "self shadow at distance {distance}, camera {camera:?}"
                );
                receiver_plane(&mut f, z + 0.02);
                f.frame();
                let blocked = mean(&f.read());
                assert!(
                    blocked < lit * 0.05,
                    "lost 2cm blocker at distance {distance}, camera {camera:?}: {blocked}/{lit}"
                );
            }
        }
    });
}

// Preliminary synthetic work accounting, not the frozen game-scene acceptance
// protocol: 1,024 moving lights, 256 moving triangle instances, cached BLAS.
#[test]
#[ignore = "explicit RT GPU benchmark; run alone with --ignored --nocapture"]
fn benchmark_rt_resolution_and_acceleration() {
    pollster::block_on(async {
        let focus = std::env::var("HLFS_RT_PROBE_FOCUS").ok().map(|value| {
            match value.as_str() {
                "1" | "1440p-reconstructed" => (2560, 2),
                "1440p-native" => (2560, 1),
                "4k-reconstructed" => (3840, 2),
                _ => panic!("HLFS_RT_PROBE_FOCUS must be 1, 1440p-reconstructed, 1440p-native or 4k-reconstructed"),
            }
        });
        let samples = std::env::var("HLFS_RT_PROBE_SAMPLES")
            .map(|value| value.parse::<u32>().expect("probe sample count"))
            .unwrap_or(2);
        assert!((1..=4).contains(&samples), "probe samples must be 1..=4");
        let candidate_override = std::env::var("HLFS_RT_PROBE_CANDIDATES")
            .ok()
            .map(|value| value.parse::<u32>().expect("probe candidate count"));
        assert!(candidate_override.is_none_or(|count| (1..=16).contains(&count)));
        assert!(
            candidate_override.is_none() || focus.is_some(),
            "candidate override requires a focused resolution"
        );
        let discovery = std::env::var("HLFS_RT_PROBE_DISCOVERY")
            .map(|value| value.parse::<f32>().expect("probe discovery fraction"))
            .unwrap_or(0.2);
        assert!(discovery.is_finite() && (0.05..=1.0).contains(&discovery));
        let tile_presampling = std::env::var_os("HLFS_RT_PROBE_PRESAMPLE").is_some();
        let shadowed = std::env::var_os("HLFS_RT_PROBE_UNSHADOWED").is_none();
        let reactive_history = std::env::var_os("HLFS_RT_PROBE_REACTIVE").is_some();
        let dense_geometry = std::env::var_os("HLFS_RT_PROBE_DENSE_GEOMETRY").is_some();
        let instance_count = if dense_geometry { 10_000u32 } else { 256 };
        let side = if dense_geometry { 100u32 } else { 16 };
        let warmup = if focus.is_some() { 120 } else { 16 };
        let measured = if focus.is_some() { 600 } else { 40 };
        for (width, height, scale, candidates) in [
            (2560, 1440, 1, 8),
            (2560, 1440, 2, 8),
            (2560, 1440, 2, 4),
            (2560, 1440, 2, 2),
            (3840, 2160, 2, 8),
            (3840, 2160, 2, 2),
        ] {
            if focus.is_some_and(|selected| (width, scale) != selected || candidates != 8) {
                continue;
            }
            let candidates = candidate_override.unwrap_or(candidates);
            let mut f = Fixture::new_rt(width, height).await;
            f.compact_output();
            let glossy = std::env::var_os("HLFS_RT_PROBE_GLOSSY").is_some();
            let dominant = std::env::var_os("HLFS_RT_PROBE_DOMINANT").is_some();
            if glossy {
                f.material([0.7, 0.4, 0.2, 1.0], [1.0, 0.1, 0.9, 1.0]);
            }
            f.config(HlfsConfig {
                mode: HlfsMode::RayTraced,
                sample_scale: scale,
                candidates_per_sample: candidates,
                samples_per_pixel: samples,
                discovery_fraction: discovery,
                tile_presampling,
                reactive_history,
                ..Default::default()
            });
            assert!(f
                .graph
                .find_pass_mut::<HlfsPass>()
                .unwrap()
                .enable_timing(&f.device));
            // 100 non-overlapping triangles per shared mesh, 10,000 rigid
            // instances: one million instanced triangles, not unique triangles.
            let caster_vertices: Vec<f32> = if dense_geometry {
                (0..100)
                    .flat_map(|i| {
                        let x = (i % 10) as f32 * 0.004;
                        let y = (i / 10) as f32 * 0.004;
                        [x, y, 0.5, x + 0.0038, y, 0.5, x, y + 0.0038, 0.5]
                    })
                    .collect()
            } else {
                vec![-0.1, -0.1, 0.5, 0.1, -0.1, 0.5, 0.0, 0.1, 0.5]
            };
            let vertices = f
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("benchmark caster"),
                    contents: bytemuck::cast_slice(&caster_vertices),
                    usage: wgpu::BufferUsages::BLAS_INPUT,
                });
            let mut encoder = f.device.create_command_encoder(&Default::default());
            f.scene
                .blas_manager
                .build_from_buffers(
                    1,
                    &mut encoder,
                    BlasGeometry {
                        revision: 0,
                        vertices: &vertices,
                        first_vertex: 0,
                        vertex_count: caster_vertices.len() as u32 / 3,
                        vertex_stride: 12,
                        indices: None,
                        first_index: 0,
                        index_count: 0,
                    },
                )
                .unwrap();
            f.queue.submit([encoder.finish()]);
            let query = f.device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("TLAS benchmark"),
                ty: wgpu::QueryType::Timestamp,
                count: 2,
            });
            let resolve = f.device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 16,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let read = f.device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 16,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut rows = Vec::new();
            for frame in 0..warmup + measured {
                let cpu = std::time::Instant::now();
                let shift = (frame as f32 * 0.1).sin() * 0.05;
                f.lights(
                    (0..1024)
                        .map(|i| {
                            let mut light = point(
                                [
                                    (i % 32) as f32 / 8.0 - 2.0 + shift,
                                    (i / 32) as f32 / 8.0 - 2.0,
                                    2.0,
                                ],
                                [1.0, 0.8, 0.5],
                                if dominant && i == 0 {
                                    8.0
                                } else {
                                    16.0 / 1024.0
                                },
                            );
                            light.set_ray_traced_shadows(shadowed);
                            light
                        })
                        .collect(),
                );
                let cpu_lights_ms = cpu.elapsed().as_secs_f64() * 1000.0;
                let instances: Vec<_> = (0..instance_count)
                    .map(|i| TlasInstanceInput {
                        mesh_id: 1,
                        transform: [
                            1.0,
                            0.0,
                            0.0,
                            (i % side) as f32 * 4.0 / side as f32 - 2.0 + shift,
                            0.0,
                            1.0,
                            0.0,
                            (i / side) as f32 * 4.0 / side as f32 - 2.0,
                            0.0,
                            0.0,
                            1.0,
                            0.0,
                        ],
                    })
                    .collect();
                let cpu_instances_ms = cpu.elapsed().as_secs_f64() * 1000.0 - cpu_lights_ms;
                let mut encoder = f.device.create_command_encoder(&Default::default());
                encoder.write_timestamp(&query, 0);
                f.scene
                    .tlas_manager
                    .build(&mut encoder, &instances, &f.scene.blas_manager)
                    .unwrap();
                encoder.write_timestamp(&query, 1);
                encoder.resolve_query_set(&query, 0..2, &resolve, 0);
                encoder.copy_buffer_to_buffer(&resolve, 0, &read, 0, 16);
                f.queue.submit([encoder.finish()]);
                let tlas_encode_submit_ms =
                    cpu.elapsed().as_secs_f64() * 1000.0 - cpu_lights_ms - cpu_instances_ms;
                f.frame();
                // The fixture owns its device, so RenderGraph waits for GPU
                // timestamp readback here. This is wall time, not CPU work.
                let frame_wall_ms = cpu.elapsed().as_secs_f64() * 1000.0;
                let stages = f.stage_milliseconds();
                // compact_output replaces the outer graph timestamp markers.
                // Its six internal stage intervals remain valid and cover HLFS.
                let graph_ms: f64 = stages.iter().sum();
                assert!(
                    graph_ms.is_finite() && graph_ms > 0.0,
                    "missing HLFS stage timestamps"
                );
                if std::env::var_os("HLFS_RT_PROBE_CAPTURE").is_some()
                    && frame >= warmup
                    && [63, 64, 65, 80, 81, 95].contains(&(frame - warmup))
                {
                    let directory = std::env::var("HLFS_RT_PROBE_OUTPUT")
                        .expect("capture requires HLFS_RT_PROBE_OUTPUT");
                    std::fs::create_dir_all(&directory).unwrap();
                    let pixels = f.read();
                    let mut image = image::RgbImage::new(width, height);
                    for (out, pixel) in image.pixels_mut().zip(pixels) {
                        *out = image::Rgb(pixel.map(|value| {
                            ((value.max(0.0) / (1.0 + value.max(0.0))).powf(1.0 / 2.2) * 255.0)
                                as u8
                        }));
                    }
                    image
                        .save(std::path::Path::new(&directory).join(format!(
                            "{width}x{height}-scale{scale}-spp{samples}-c{candidates}-frame{:03}.png",
                            frame - warmup
                        )))
                        .unwrap();
                }
                let (tx, rx) = std::sync::mpsc::channel();
                read.slice(..)
                    .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
                f.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
                rx.recv().unwrap().unwrap();
                let bytes = read.slice(..).get_mapped_range().unwrap();
                let start = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                let end = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
                let tlas_ms = (end - start) as f64 * f.queue.get_timestamp_period() as f64 / 1e6;
                drop(bytes);
                read.unmap();
                if frame >= warmup {
                    rows.push((
                        graph_ms + tlas_ms,
                        tlas_ms,
                        graph_ms,
                        frame_wall_ms,
                        stages,
                        [
                            cpu_lights_ms,
                            cpu_instances_ms,
                            tlas_encode_submit_ms,
                            frame_wall_ms
                                - cpu_lights_ms
                                - cpu_instances_ms
                                - tlas_encode_submit_ms,
                        ],
                    ));
                }
            }
            if let Ok(directory) = std::env::var("HLFS_RT_PROBE_OUTPUT") {
                std::fs::create_dir_all(&directory).unwrap();
                let mut csv=String::from("gpu_sum_ms,tlas_ms,hlfs_ms,frame_wall_ms,coarse_ms,fine_ms,sample_ms,temporal_ms,spatial_ms,composite_ms,lights_update_ms,instances_prepare_ms,tlas_encode_submit_ms,graph_execute_wait_ms\n");
                for row in &rows {
                    csv.push_str(&format!(
                        "{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
                        row.0,
                        row.1,
                        row.2,
                        row.3,
                        row.4[0],
                        row.4[1],
                        row.4[2],
                        row.4[3],
                        row.4[4],
                        row.4[5],
                        row.5[0],
                        row.5[1],
                        row.5[2],
                        row.5[3],
                    ));
                }
                std::fs::write(
                    std::path::Path::new(&directory).join(format!(
                        "{width}x{height}-scale{scale}-spp{samples}-candidates{candidates}-discovery{discovery}.csv"
                    )),
                    csv,
                )
                .unwrap();
            }
            let median = |mut values: Vec<f64>| {
                values.sort_by(f64::total_cmp);
                values[values.len() / 2]
            };
            let mut totals: Vec<_> = rows.iter().map(|r| r.0).collect();
            totals.sort_by(f64::total_cmp);
            let stages: [f64; 6] =
                std::array::from_fn(|i| median(rows.iter().map(|r| r.4[i]).collect()));
            let cpu_stages: [f64; 4] =
                std::array::from_fn(|i| median(rows.iter().map(|r| r.5[i]).collect()));
            eprintln!("RT_PROBE glossy={glossy} dominant={dominant} shadowed={shadowed} resolution={width}x{height} scale={scale} spp={samples} candidates={candidates} discovery={discovery} presample={tile_presampling} reactive={reactive_history} warmup={warmup} measured={measured} lights=1024 moving_instances={instance_count} unique_triangles={} instanced_triangles={} median_gpu_ms={:.4} p95_gpu_ms={:.4} tlas_median_ms={:.4} hlfs_median_ms={:.4} frame_wall_median_ms={:.4} stages={stages:?} host_wall_stages={cpu_stages:?}",caster_vertices.len()/9,instance_count as usize*caster_vertices.len()/9,totals[totals.len()/2],totals[totals.len()*95/100],median(rows.iter().map(|r|r.1).collect()),median(rows.iter().map(|r|r.2).collect()),median(rows.iter().map(|r|r.3).collect()));
        }
    });
}

// Atomic light-grid append must not change final pixels between fresh GPU runs
// with the same local lights, camera, and frame sequence.
#[test]
#[ignore = "requires Vulkan hardware ray queries"]
fn local_light_grid_output_is_repeatable() {
    pollster::block_on(async {
        for mixed in [false, true] {
            let mut baseline: Option<Vec<u32>> = None;
            for run in 0..3 {
                let mut f = Fixture::new_rt(65, 49).await;
                f.config(HlfsConfig {
                    mode: HlfsMode::RayTraced,
                    sample_scale: 1,
                    samples_per_pixel: 2,
                    candidates_per_sample: 8,
                    ..Default::default()
                });
                f.lights(
                    (0..48)
                        .map(|i| {
                            let mut light = point(
                                [
                                    (i % 32) as f32 * 0.25 - 4.0,
                                    (i / 32) as f32 * 0.25 - 4.0,
                                    2.0,
                                ],
                                [1.0, 0.7, 0.4],
                                if i % 13 == 0 { 0.0 } else { 4.0 },
                            );
                            if mixed && i % 7 == 0 {
                                light.light_type = 0;
                                light.direction_outer = [0.0, 0.0, -1.0, 0.0];
                            } else if mixed && i % 5 == 0 {
                                light.light_type = 2;
                                light.direction_outer = [0.0, 0.0, -1.0, 0.5];
                                light.inner_angle = 0.9;
                            }
                            light.set_ray_traced_shadows(true);
                            light
                        })
                        .collect(),
                );
                empty_scene(&mut f);
                let mut pixels = Vec::new();
                for _ in 0..4 {
                    f.frame();
                    pixels.extend(f.read().iter().flat_map(|rgb| rgb.map(f32::to_bits)));
                }
                if let Some(expected) = &baseline {
                    assert!(
                        pixels == *expected,
                        "mixed={mixed}, run={run}: identical local lights changed final output"
                    );
                } else {
                    baseline = Some(pixels);
                }
            }
        }
    });
}

// Explicit cross-build audit: compare exported f32 pixels from two shader
// implementations with identical lights, seeds, history and moving blockers.
#[test]
#[ignore = "explicit cross-build GPU output audit; requires HLFS_RT_AUDIT_OUTPUT"]
fn benchmark_candidate_output_audit() {
    let directory = std::env::var("HLFS_RT_AUDIT_OUTPUT").expect("audit output directory");
    let samples = std::env::var("HLFS_RT_AUDIT_SAMPLES")
        .map(|value| value.parse::<u32>().expect("audit sample count"))
        .unwrap_or(2);
    assert!((1..=4).contains(&samples), "audit samples must be 1..=4");
    let selected_case = std::env::var("HLFS_RT_AUDIT_CASE").ok();
    // Presampled output can differ between fresh runs due to atomic alias
    // ordering. Compare unchanged controls before using it for cross-build QA.
    let tile_presampling = std::env::var_os("HLFS_RT_AUDIT_PRESAMPLE").is_some();
    let reactive_history = std::env::var_os("HLFS_RT_AUDIT_REACTIVE").is_some();
    std::fs::create_dir_all(&directory).unwrap();
    pollster::block_on(async {
        for (case, count, mixed, scale, candidates) in [
            ("local-grid", 48, false, 1, 8),
            ("local-overflow", 1024, false, 2, 8),
            ("mixed-overflow", 1024, true, 2, 16),
            ("mixed-grid", 48, true, 1, 1),
            ("packed-id-overflow", 65536, true, 2, 8),
            ("hdr-material-overflow", 1024, false, 2, 8),
        ] {
            if selected_case
                .as_deref()
                .is_some_and(|selected| selected != case)
            {
                continue;
            }
            let mut f = Fixture::new_rt(65, 49).await;
            if case == "hdr-material-overflow" {
                f.material([2.0, 0.5, 1.4, 1.0], [1.0, 0.7, 0.5, 1.0]);
            }
            f.config(HlfsConfig {
                mode: HlfsMode::RayTraced,
                sample_scale: scale,
                samples_per_pixel: samples,
                candidates_per_sample: candidates,
                tile_presampling,
                reactive_history,
                ..Default::default()
            });
            let lights = (0..count)
                .map(|i| {
                    let mut light = point(
                        [
                            (i % 32) as f32 * 0.25 - 4.0,
                            (i / 32) as f32 * 0.25 - 4.0,
                            2.0,
                        ],
                        [1.0, 0.7, 0.4],
                        if i % 13 == 0 { 0.0 } else { 4.0 },
                    );
                    if mixed && i % 7 == 0 {
                        light.light_type = 0;
                        light.direction_outer = [0.0, 0.0, -1.0, 0.0];
                    } else if mixed && i % 5 == 0 {
                        light.light_type = 2;
                        light.direction_outer = [0.0, 0.0, -1.0, 0.5];
                        light.inner_angle = 0.9;
                    }
                    light.set_ray_traced_shadows(true);
                    light
                })
                .collect::<Vec<_>>();
            f.lights(lights.clone());
            let mut bytes = Vec::new();
            for frame in 0..16 {
                if frame == 0 || frame == 12 {
                    empty_scene(&mut f);
                }
                if frame == 4 {
                    receiver_plane(&mut f, 1.0);
                }
                if frame == 8 {
                    let mut moved = lights.clone();
                    for light in &mut moved {
                        light.position_range[0] += 0.2;
                    }
                    f.lights(moved);
                }
                f.frame();
                let pixels = f.read();
                assert!(pixels.iter().flatten().all(|v| v.is_finite()));
                if frame == 0 {
                    let unoccluded_mean = mean(&pixels);
                    assert!(
                        unoccluded_mean > 0.1,
                        "{case}: nonzero direct light required"
                    );
                }
                eprintln!(
                    "RT_AUDIT case={case} spp={samples} frame={frame} mean={}",
                    mean(&pixels)
                );
                bytes.extend_from_slice(bytemuck::cast_slice(&pixels));
            }
            std::fs::write(
                std::path::Path::new(&directory).join(format!("{case}.f32")),
                bytes,
            )
            .unwrap();
        }
    });
}

// Remove the reference's legitimate shadow edges before measuring residual
// pixel-scale variation in the display-space result.
fn display_residual_highpass_rms(
    sampled: &[[f32; 3]],
    reference: &[[f32; 3]],
    width: u32,
    height: u32,
) -> f64 {
    assert_eq!(sampled.len(), (width * height) as usize);
    assert_eq!(sampled.len(), reference.len());
    let encode = |v: f32| ((v.max(0.0) / (1.0 + v.max(0.0))).powf(1.0 / 2.2) * 255.0) as u8;
    let residual: Vec<[f32; 3]> = sampled
        .iter()
        .zip(reference)
        .map(|(a, b)| {
            std::array::from_fn(|channel| {
                (f32::from(encode(a[channel])) - f32::from(encode(b[channel]))) / 255.0
            })
        })
        .collect();
    let (width, height) = (width as i32, height as i32);
    let mut squared = 0.0f64;
    for y in 0..height {
        for x in 0..width {
            let mut local = [0.0f32; 3];
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let qx = (x + dx).clamp(0, width - 1);
                    let qy = (y + dy).clamp(0, height - 1);
                    let neighbor = residual[(qy * width + qx) as usize];
                    for channel in 0..3 {
                        local[channel] += neighbor[channel];
                    }
                }
            }
            let center = residual[(y * width + x) as usize];
            for channel in 0..3 {
                let high = f64::from(center[channel] - local[channel] / 9.0);
                squared += high * high;
            }
        }
    }
    (squared / (width as f64 * height as f64 * 3.0)).sqrt()
}

// Development frontier only: a small receiver/occluder scene, not the frozen
// million-triangle primary tier. Failed quality rows remain in the output.
#[test]
#[ignore = "explicit RT quality frontier; requires HLFS_RT_QUALITY_OUTPUT"]
fn benchmark_rt_quality_frontier() {
    let directory = std::env::var("HLFS_RT_QUALITY_OUTPUT").expect("quality output directory");
    // Match the shading resolution of the performance probe under review.
    let sample_scale = std::env::var("HLFS_RT_QUALITY_SAMPLE_SCALE")
        .map(|value| value.parse::<u32>().expect("quality sample scale"))
        .unwrap_or(2);
    assert!(
        (1..=2).contains(&sample_scale),
        "quality sample scale must be 1 or 2"
    );
    let discovery = std::env::var("HLFS_RT_QUALITY_DISCOVERY")
        .map(|value| value.parse::<f32>().expect("quality discovery fraction"))
        .unwrap_or(0.2);
    assert!(discovery.is_finite() && (0.05..=1.0).contains(&discovery));
    let tile_presampling = std::env::var_os("HLFS_RT_QUALITY_PRESAMPLE").is_some();
    let reactive_history = std::env::var_os("HLFS_RT_QUALITY_REACTIVE").is_some();
    let capture_motion = std::env::var_os("HLFS_RT_QUALITY_CAPTURE_MOTION").is_some();
    let glossy_motion = std::env::var_os("HLFS_RT_QUALITY_GLOSSY_MOTION").is_some();
    let camera_motion =
        glossy_motion || std::env::var_os("HLFS_RT_QUALITY_CAMERA_MOTION").is_some();
    // Freeze an additional validation fixture; do not replace the original.
    // Constant-depth plane stays geometrically valid under lateral camera motion.
    let camera_at = |frame: u32| {
        let x = if frame >= 64 {
            ((frame - 64) as f32 * 0.2).sin() * 0.8
        } else {
            0.0
        };
        let eye = glam::Vec3::new(x, 0.0, 3.0);
        let view = glam::Mat4::look_at_rh(eye, glam::Vec3::new(x, 0.0, 0.0), glam::Vec3::Y);
        (eye, view)
    };
    let update_camera = |f: &mut Fixture, frame: u32| {
        if !camera_motion {
            return;
        }
        let (eye, view) = camera_at(frame);
        let (_, previous) = camera_at(frame.saturating_sub(1));
        let proj = helio_core::depth::orthographic(-2.0, 2.0, -2.0, 2.0, 0.1, 10.0);
        f.scene.camera.update(helio_core::GpuCameraUniforms::new(
            view,
            proj,
            eye,
            0.1,
            10.0,
            frame,
            [0.0; 2],
            proj * previous,
        ));
    };
    let selected_setting = std::env::var("HLFS_RT_QUALITY_SETTING").ok();
    let settings = if let Some(setting) = &selected_setting {
        let (samples, candidates) = setting.split_once(':').expect("samples:candidates");
        let samples = samples.parse::<u32>().unwrap();
        let candidates = candidates.parse::<u32>().unwrap();
        assert!((1..=4).contains(&samples) && (1..=16).contains(&candidates));
        vec![(samples, candidates)]
    } else {
        vec![(1, 4), (1, 8), (2, 4), (2, 8), (4, 8)]
    };
    std::fs::create_dir_all(&directory).unwrap();
    pollster::block_on(async {
        let checkpoints = [
            0u32, 1, 3, 7, 15, 31, 63, 64, 65, 67, 71, 79, 80, 81, 83, 87, 95,
        ];
        let mut csv = String::from("seed,samples,candidates,sample_scale,discovery,tile_presampling,reactive_history,mode,frame,mask,pixels,relative_mean_error,nrmse,quality_pass\n");
        let mut final_failures = 0usize;
        let mut motion_csv = String::from(
            "seed,samples,candidates,reactive_history,static_frame_delta_rms,motion_pass\n",
        );
        let mut motion_failures = 0usize;
        let mut grain_csv = String::from(
            "seed,samples,candidates,frame,display_residual_highpass_rms,grain_pass\n",
        );
        let mut grain_failures = 0usize;
        // Fixed regression seeds. Both sets have now been exercised during
        // development; they are not an untouched holdout. Keep thresholds fixed.
        let seeds = if std::env::var_os("HLFS_RT_QUALITY_REVIEW_SEEDS").is_some() {
            [307u32, 401, 503, 601]
        } else if std::env::var_os("HLFS_RT_QUALITY_HELD_OUT").is_some() {
            [101u32, 131, 173, 211]
        } else {
            [11u32, 29, 47, 71]
        };
        let seeds = std::env::var("HLFS_RT_QUALITY_SEED")
            .map(|value| vec![value.parse::<u32>().expect("quality seed")])
            .unwrap_or_else(|_| seeds.to_vec());
        for seed in seeds {
            let lights_at = |frame: u32| {
                (0..1024)
                    .map(|i| {
                        let phase = seed as f32 * 0.17;
                        let shift = if frame >= 64 {
                            ((frame - 64) as f32 * 0.1 + phase).sin() * 0.25
                        } else {
                            0.0
                        };
                        let color = match (i + seed) % 3 {
                            0 => [1.0, 0.2, 0.1],
                            1 => [0.1, 1.0, 0.2],
                            _ => [0.2, 0.1, 1.0],
                        };
                        let mut light = point(
                            [
                                (i % 32) as f32 / 8.0 - 2.0 + shift,
                                (i / 32) as f32 / 8.0 - 2.0,
                                0.8 + ((i + seed) % 5) as f32 * 0.4,
                            ],
                            color,
                            if i == (if frame >= 80
                                && std::env::var_os("HLFS_RT_QUALITY_SWITCH_KEY").is_some()
                            {
                                31
                            } else {
                                0
                            }) {
                                8.0
                            } else {
                                0.025
                            },
                        );
                        light.set_ray_traced_shadows(true);
                        light
                    })
                    .collect::<Vec<_>>()
            };
            let (width, height) = match std::env::var("HLFS_RT_QUALITY_RESOLUTION").as_deref() {
                Ok("1440p") => (2560, 1440),
                Ok("4k") => (3840, 2160),
                Ok(other) => panic!("unknown quality resolution: {other}"),
                Err(_) => {
                    if camera_motion {
                        (257, 145)
                    } else {
                        (129, 73)
                    }
                }
            };
            let mut oracle = Fixture::new_rt(width, height).await;
            if std::env::var_os("HLFS_RT_QUALITY_DIRECT_ONLY").is_some() {
                oracle.ambient = [0.0; 3];
            }
            if glossy_motion {
                oracle.material([0.7, 0.4, 0.2, 1.0], [1.0, 0.1, 0.9, 1.0]);
            }
            oracle.config(HlfsConfig {
                mode: HlfsMode::RayTraced,
                debug_mode: HlfsDebugMode::Reference,
                ..Default::default()
            });
            let mut references = Vec::new();
            for frame in checkpoints {
                oracle.scene.frame_count = frame as u64;
                update_camera(&mut oracle, frame);
                oracle.lights(lights_at(frame));
                if (64..80).contains(&frame) {
                    blocker(&mut oracle, 0.35);
                } else {
                    empty_scene(&mut oracle);
                }
                oracle.frame();
                references.push(oracle.read());
            }
            let luminance =
                |p: &[f32; 3]| p[0] as f64 * 0.2126 + p[1] as f64 * 0.7152 + p[2] as f64 * 0.0722;
            let initial: Vec<_> = references[0].iter().map(luminance).collect();
            let peak = initial.iter().copied().fold(0.0, f64::max);
            let mut f = Fixture::new_rt(width, height).await;
            if std::env::var_os("HLFS_RT_QUALITY_DIRECT_ONLY").is_some() {
                f.ambient = [0.0; 3];
            }
            if glossy_motion {
                f.material([0.7, 0.4, 0.2, 1.0], [1.0, 0.1, 0.9, 1.0]);
            }
            f.compact_output();
            for &(samples, candidates) in &settings {
                for mode in [HlfsDebugMode::Final, HlfsDebugMode::Unfiltered] {
                    f.config(HlfsConfig {
                        mode: HlfsMode::RayTraced,
                        debug_mode: mode,
                        sample_scale,
                        samples_per_pixel: samples,
                        candidates_per_sample: candidates,
                        discovery_fraction: discovery,
                        tile_presampling,
                        temporal_resampling: std::env::var_os("HLFS_RT_QUALITY_TEMPORAL_RIS")
                            .is_some(),
                        reactive_history,
                        ..Default::default()
                    });
                    f.scene.frame_count = 0;
                    empty_scene(&mut f);
                    let mut previous_static_display: Option<Vec<u8>> = None;
                    let mut static_delta_squared = 0.0f64;
                    let mut static_delta_count = 0u64;
                    for frame in 0..96u32 {
                        update_camera(&mut f, frame);
                        f.lights(lights_at(frame));
                        if frame == 64 {
                            blocker(&mut f, 0.35);
                        }
                        if frame == 80 {
                            empty_scene(&mut f);
                        }
                        f.frame();
                        let motion_pixels = if capture_motion && mode == HlfsDebugMode::Final {
                            let pixels = f.read();
                            let mut image = image::RgbImage::new(f.width, f.height);
                            for (out, pixel) in image.pixels_mut().zip(&pixels) {
                                *out = image::Rgb(pixel.map(|v| {
                                    ((v.max(0.0) / (1.0 + v.max(0.0))).powf(1.0 / 2.2) * 255.0)
                                        as u8
                                }));
                            }
                            // Frames 32..63 have fixed camera, lights and TLAS.
                            // Display-space frame changes here are renderer flicker.
                            if (32..64).contains(&frame) {
                                if let Some(previous) = &previous_static_display {
                                    for (&a, &b) in image.as_raw().iter().zip(previous) {
                                        let delta = (f64::from(a) - f64::from(b)) / 255.0;
                                        static_delta_squared += delta * delta;
                                        static_delta_count += 1;
                                    }
                                }
                                previous_static_display = Some(image.as_raw().clone());
                            }
                            image
                                .save(std::path::Path::new(&directory).join(format!(
                                    "motion-seed{seed}-spp{samples}-c{candidates}-f{frame:03}.png"
                                )))
                                .unwrap();
                            Some(pixels)
                        } else {
                            None
                        };
                        let Some(reference_index) =
                            checkpoints.iter().position(|&value| value == frame)
                        else {
                            continue;
                        };
                        let reference = &references[reference_index];
                        let pixels = motion_pixels.unwrap_or_else(|| f.read());
                        assert!(pixels.iter().flatten().all(|v| v.is_finite()));
                        if capture_motion
                            && mode == HlfsDebugMode::Final
                            && [63, 64, 65, 71, 79, 80, 81, 95].contains(&frame)
                        {
                            let rms = display_residual_highpass_rms(
                                &pixels, reference, f.width, f.height,
                            );
                            // A smooth exact receiver should not gain visible
                            // color texture from the stochastic lighting pass.
                            let pass = rms < 0.005;
                            grain_csv.push_str(&format!(
                                "{seed},{samples},{candidates},{frame},{rms},{pass}\n"
                            ));
                            if !pass {
                                grain_failures += 1;
                                eprintln!("SPATIAL_GRAIN_FAIL seed={seed} spp={samples} candidates={candidates} frame={frame} rms={rms}");
                            }
                        }
                        for mask in ["all", "changed", "glossy"] {
                            if mask == "glossy" && !glossy_motion {
                                continue;
                            }
                            let mut count = 0u32;
                            let mut sum = 0.0;
                            let mut ref_sum = 0.0;
                            let mut squared = 0.0;
                            let mut ref_squared = 0.0;
                            for (index, (a, b)) in pixels.iter().zip(reference).enumerate() {
                                let a = luminance(a);
                                let b = luminance(b);
                                if mask == "changed" && (b - initial[index]).abs() <= peak * 0.1 {
                                    continue;
                                }
                                if mask == "glossy" && b <= peak * 0.2 {
                                    continue;
                                }
                                count += 1;
                                sum += a;
                                ref_sum += b;
                                squared += (a - b) * (a - b);
                                ref_squared += b * b;
                            }
                            if count == 0 {
                                continue;
                            }
                            let mean_error =
                                (sum - ref_sum).abs() / ref_sum.abs().max(1e-6 * count as f64);
                            let nrmse = (squared / ref_squared.max(1e-12 * count as f64)).sqrt();
                            // This numeric screen does not replace inspection
                            // of noise and shadow stability during motion.
                            let pass = mean_error < 0.08 && nrmse < 0.20;
                            if mode == HlfsDebugMode::Final && !pass {
                                final_failures += 1;
                                eprintln!("QUALITY_FAIL seed={seed} frame={frame} mask={mask} signed_mean={} nrmse={nrmse}", (sum-ref_sum)/ref_sum.max(1e-12));
                            }
                            csv.push_str(&format!("{seed},{samples},{candidates},{sample_scale},{discovery},{tile_presampling},{reactive_history},{mode:?},{frame},{mask},{count},{mean_error},{nrmse},{pass}\n"));
                        }
                        if mode == HlfsDebugMode::Final
                            && [63, 64, 65, 71, 79, 80, 81, 95].contains(&frame)
                        {
                            for (suffix, buffer) in [("sampled", &pixels), ("reference", reference)]
                            {
                                let mut image = image::RgbImage::new(f.width, f.height);
                                for (out, pixel) in image.pixels_mut().zip(buffer) {
                                    *out = image::Rgb(pixel.map(|v| {
                                        ((v.max(0.0) / (1.0 + v.max(0.0))).powf(1.0 / 2.2) * 255.0)
                                            as u8
                                    }));
                                }
                                image.save(std::path::Path::new(&directory).join(format!("seed{seed}-spp{samples}-c{candidates}-f{frame}-{suffix}.png"))).unwrap();
                            }
                        }
                    }
                    if capture_motion && mode == HlfsDebugMode::Final {
                        assert!(static_delta_count > 0);
                        let rms = (static_delta_squared / static_delta_count as f64).sqrt();
                        // Below roughly three display levels RMS on this static
                        // receiver; visual inspection is still required.
                        let pass = rms < 0.01;
                        motion_csv.push_str(&format!(
                            "{seed},{samples},{candidates},{reactive_history},{rms},{pass}\n"
                        ));
                        if !pass {
                            motion_failures += 1;
                            eprintln!("MOTION_FLICKER_FAIL seed={seed} spp={samples} candidates={candidates} rms={rms}");
                        }
                    }
                }
            }
            eprintln!("RT_QUALITY completed seed={seed}");
        }
        std::fs::write(std::path::Path::new(&directory).join("quality.csv"), csv).unwrap();
        if capture_motion {
            std::fs::write(
                std::path::Path::new(&directory).join("motion-metrics.csv"),
                motion_csv,
            )
            .unwrap();
            std::fs::write(
                std::path::Path::new(&directory).join("grain-metrics.csv"),
                grain_csv,
            )
            .unwrap();
        }
        // Write all failures before returning a failing gate, never a misleading
        // successful test exit for the new review-acceptance fixture.
        if camera_motion || selected_setting.is_some() {
            assert_eq!(
                final_failures, 0,
                "final-output quality gate failed; see quality.csv"
            );
        }
        assert_eq!(
            motion_failures, 0,
            "moving visual flicker gate failed; see motion-metrics.csv and captured frames"
        );
        assert_eq!(
            grain_failures, 0,
            "spatial grain gate failed; see grain-metrics.csv and captured frames"
        );
    });
}

#[test]
#[ignore = "requires Vulkan hardware ray queries"]
fn tile_proposals_preserve_energy_across_empty_strata_and_packed_overflow() {
    pollster::block_on(async {
        let mut f = Fixture::new_rt(65, 49).await;
        f.compact_output();
        empty_scene(&mut f);
        let make_light = |intensity| {
            let mut light = point([0.0, 0.0, 2.0], [1.0, 0.7, 0.3], intensity);
            light.set_ray_traced_shadows(true);
            light
        };
        f.config(HlfsConfig {
            mode: HlfsMode::RayTraced,
            debug_mode: HlfsDebugMode::Reference,
            ..Default::default()
        });
        f.lights(vec![make_light(2.0)]);
        f.frame();
        let reference = mean(&f.read());
        f.lights(vec![]);
        f.frame();
        let ambient = mean(&f.read());
        assert!(reference > ambient + 0.01);
        f.config(HlfsConfig {
            mode: HlfsMode::RayTraced,
            debug_mode: HlfsDebugMode::Unfiltered,
            tile_presampling: true,
            reactive_history: true,
            sample_scale: 2,
            samples_per_pixel: 1,
            candidates_per_sample: 8,
            ..Default::default()
        });
        for count in [0u32, 1, 63, 64, 65, 1024, 65536] {
            let active = if count >= 128 { count / 2 } else { count };
            f.lights(
                (0..count)
                    .map(|i| {
                        make_light(if count >= 128 && i % 64 < 32 {
                            0.0
                        } else {
                            2.0 / active.max(1) as f32
                        })
                    })
                    .collect(),
            );
            f.frame();
            let pixels = f.read();
            assert!(pixels.iter().flatten().all(|v| v.is_finite()));
            let expected = if count == 0 { ambient } else { reference };
            let error = (mean(&pixels) - expected).abs() / expected.max(0.001);
            eprintln!(
                "RT_PROPOSAL_ENERGY lights={count} expected={expected} measured={} error={error}",
                mean(&pixels)
            );
            assert!(
                error < 0.08,
                "proposal normalization lost energy at {count} lights"
            );
        }
        // Same-count extinction must not keep old IDs or old illumination.
        f.config(HlfsConfig {
            mode: HlfsMode::RayTraced,
            tile_presampling: true,
            reactive_history: true,
            sample_scale: 2,
            samples_per_pixel: 1,
            candidates_per_sample: 8,
            ..Default::default()
        });
        f.lights(vec![make_light(2.0 / 1024.0); 1024]);
        for _ in 0..8 {
            f.frame();
        }
        f.lights(vec![make_light(0.0); 1024]);
        for _ in 0..8 {
            f.frame();
        }
        assert!((mean(&f.read()) - ambient).abs() < (reference - ambient) * 0.01);
        f.config(HlfsConfig {
            mode: HlfsMode::RayTraced,
            sample_scale: 2,
            ..Default::default()
        });
        f.lights(vec![make_light(2.0)]);
        f.frame();
        assert!((mean(&f.read()) - reference).abs() < reference * 0.02);
    });
}

#[test]
#[ignore = "requires Vulkan hardware ray queries"]
fn colored_thin_sheets_multiply_and_opaque_blockers_still_occlude() {
    pollster::block_on(async {
        for debug_mode in [HlfsDebugMode::Reference, HlfsDebugMode::Unfiltered] {
            let mut f = Fixture::new_rt(65, 49).await;
            f.config(HlfsConfig {
                debug_mode,
                sample_scale: 1,
                ..HlfsConfig::ray_traced_presampled()
            });
            f.lights(vec![light()]);
            empty_scene(&mut f);
            f.frame();
            let clear = f.read();
            blocker(&mut f, 5.0);
            for (rows, expected) in [
                (vec![[1.0f32, 1.0, 1.0, 0.0]], [1.0, 1.0, 1.0]),
                (vec![[0.8, 0.2, 0.05, 0.0]], [0.8, 0.2, 0.05]),
                (
                    vec![[0.8, 0.2, 0.05, 0.0], [0.5, 0.6, 0.8, 0.0]],
                    [0.4, 0.12, 0.04],
                ),
                (vec![[0.8, 0.2, 0.05, 0.0], [0.0; 4]], [0.0; 3]),
                (vec![[0.0; 4], [0.8, 0.2, 0.05, 0.0]], [0.0; 3]),
            ] {
                let instances: Vec<_> = (0..rows.len())
                    .map(|i| TlasInstanceInput {
                        mesh_id: 1,
                        transform: [
                            1.0, 0.0, 0.0, i as f32, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0,
                        ],
                    })
                    .collect();
                let mut encoder = f.device.create_command_encoder(&Default::default());
                f.scene
                    .tlas_manager
                    .build(&mut encoder, &instances, &f.scene.blas_manager)
                    .unwrap();
                f.queue.submit([encoder.finish()]);
                f.transmission = Some(transmission_buffer(&f.device, &rows));
                f.frame();
                let actual = f.read();
                for channel in 0..3 {
                    let lit: f32 = clear.iter().map(|p| p[channel]).sum();
                    let transmitted: f32 = actual.iter().map(|p| p[channel]).sum();
                    assert!(
                        (transmitted / lit - expected[channel]).abs() < 0.015,
                        "{debug_mode:?}: channel {channel}, expected {}, actual {}",
                        expected[channel],
                        transmitted / lit
                    );
                }
            }
            // Dropping metadata restores the binary path, never stale tint.
            f.transmission = None;
            f.frame();
            assert!(mean(&f.read()) < mean(&clear) * 0.01);
        }
    });
}

#[test]
#[ignore = "requires Vulkan hardware ray queries"]
fn colored_visibility_cache_preserves_channels_with_many_lights() {
    pollster::block_on(async {
        let mut f = Fixture::new_rt(65, 49).await;
        let mut source = light();
        source.color_intensity[3] /= 33.0;
        f.lights(vec![source; 33]);
        blocker(&mut f, 5.0);
        f.transmission = Some(transmission_buffer(&f.device, &[[0.7, 0.3, 0.1, 0.0]]));
        f.config(HlfsConfig {
            debug_mode: HlfsDebugMode::Reference,
            sample_scale: 1,
            ..HlfsConfig::ray_traced_presampled()
        });
        f.frame();
        let reference = f.read();
        for (sample_scale, temporal_resampling) in [(1, false), (2, true)] {
            f.config(HlfsConfig {
                debug_mode: HlfsDebugMode::Unfiltered,
                sample_scale,
                temporal_resampling,
                ..HlfsConfig::ray_traced_presampled()
            });
            for _ in 0..8 {
                f.frame();
                let result = f.read();
                for channel in 0..3 {
                    let expected: f32 = reference.iter().map(|p| p[channel]).sum();
                    let actual: f32 = result.iter().map(|p| p[channel]).sum();
                    assert!(
                        (actual / expected - 1.0).abs() < 0.04,
                        "colored RIS channel {channel}: {actual}/{expected}"
                    );
                }
            }
        }
    });
}

#[test]
#[ignore = "requires Vulkan hardware ray queries"]
fn temporal_ris_rejects_same_capacity_light_reassignment() {
    pollster::block_on(async {
        let mut f = Fixture::new_rt(65, 49).await;
        f.compact_output();
        f.ambient = [0.0; 3];
        empty_scene(&mut f);
        let make_lights = |start: usize| {
            (0..1024)
                .map(|i| {
                    let mut light = point(
                        [0.0, 0.0, 2.0],
                        [1.0, 0.7, 0.3],
                        if (start..start + 128).contains(&i) {
                            2.0 / 128.0
                        } else {
                            0.0
                        },
                    );
                    light.set_ray_traced_shadows(true);
                    light
                })
                .collect::<Vec<_>>()
        };
        f.config(HlfsConfig {
            mode: HlfsMode::RayTraced,
            debug_mode: HlfsDebugMode::Reference,
            ..Default::default()
        });
        f.lights(make_lights(0));
        f.frame();
        let reference = mean(&f.read());
        assert!(reference > 0.001);
        f.config(HlfsConfig {
            temporal_resampling: true,
            debug_mode: HlfsDebugMode::Unfiltered,
            ..HlfsConfig::ray_traced_presampled()
        });
        for _ in 0..16 {
            f.frame();
        }
        for start in [512, 128, 768, 0] {
            f.lights(make_lights(start));
            f.frame();
            let error = (mean(&f.read()) - reference).abs() / reference;
            assert!(
                error < 0.08,
                "first frame after light-slot reassignment {start}: relative energy error {error}"
            );
        }
    });
}

#[test]
#[ignore = "requires Vulkan hardware ray queries"]
fn exact_transmission_sets_bypass_noisy_history_filters() {
    pollster::block_on(async {
        let mut f = Fixture::new_rt(65, 49).await;
        f.ambient = [0.0; 3];
        blocker(&mut f, 5.0);
        f.transmission = Some(transmission_buffer(&f.device, &[[0.7, 0.3, 0.1, 0.0]]));
        let sources: Vec<_> = (0..8)
            .map(|i| {
                let mut source = point([8.0, i as f32 * 0.3 - 1.0, 1.0], [1.0, 0.7, 0.4], 20.0);
                source.position_range[3] = 10.0;
                source.set_ray_traced_shadows(true);
                source
            })
            .collect();
        f.lights(sources);
        for frame in [0, 1, 8, 16] {
            let mut images = Vec::new();
            for debug_mode in [HlfsDebugMode::Final, HlfsDebugMode::Reference] {
                f.config(HlfsConfig {
                    debug_mode,
                    sample_scale: 1,
                    ..HlfsConfig::ray_traced_presampled()
                });
                f.scene.frame_count = frame;
                f.frame();
                images.push(f.read());
            }
            for (i, (actual, expected)) in images[0].iter().zip(&images[1]).enumerate() {
                for channel in 0..3 {
                    assert!((actual[channel]-expected[channel]).abs()<0.00001,
                        "exact colored lighting filtered at pixel {i}, frame {frame}: {actual:?}/{expected:?}");
                }
            }
        }
    });
}
