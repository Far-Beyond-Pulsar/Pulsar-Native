//! GPU validation is intentional: a Rust build does not validate WGSL or bindings.
mod support;

#[test]
#[ignore = "requires a GPU adapter; run explicitly with --ignored"]
fn packed_hdr_rounding_preserves_energy_and_subnormals() {
    pollster::block_on(async {
        let f = support::Fixture::new(1, 1).await;
        let source = [include_str!("../shaders/common.wgsl"), r#"
@group(0) @binding(8) var<storage,read_write> results: array<vec4<f32>>;
@compute @workgroup_size(64)
fn check_packing(@builtin(global_invocation_id) id: vec3<u32>) {
    let values=array<f32,12>(0.0,0.0000001,0.00001,0.000061,0.00123,0.0317,0.137,0.993,1.017,31.75,1234.5,60000.0);
    let value=values[id.x/256u];
    let noise=(f32(id.x%256u)+0.5)/256.0;
    results[id.x]=vec4<f32>(value,unpack_ufloat(pack_ufloat(value,6u,noise),6u),unpack_ufloat(pack_ufloat(value,5u,noise),5u),unpack_moment(pack_moment(value*value,noise)));
}
"#].concat();
        let module = f.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Packed HDR numeric validation"),
            source: wgpu::ShaderSource::Wgsl(
                helio_core::shader::resolve(&source).into_owned().into(),
            ),
        });
        let pipeline = f
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Packed HDR validation"),
                layout: None,
                module: &module,
                entry_point: Some("check_packing"),
                compilation_options: Default::default(),
                cache: None,
            });
        let size = 12 * 256 * 16;
        let output = f.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let read = f.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = f.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 8,
                resource: output.as_entire_binding(),
            }],
        });
        let mut encoder = f.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(48, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &read, 0, size);
        f.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        read.slice(..)
            .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
        f.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        rx.recv().unwrap().unwrap();
        let mapped = read.slice(..).get_mapped_range().unwrap();
        let rows: &[[f32; 4]] = bytemuck::cast_slice(&mapped);
        for values in rows.chunks_exact(256) {
            let expected = values[0][0];
            for (channel, bits, expected, minimum_exponent) in [
                (1, 6, expected, -14),
                (2, 5, expected, -14),
                (3, 5, expected * expected, -30),
            ] {
                let step = 2f32
                    .powf(expected.max(2f32.powi(minimum_exponent)).log2().floor() - bits as f32);
                assert!(values.iter().all(|v| v[channel].is_finite()
                    && v[channel] >= 0.0
                    && (v[channel] - expected).abs() <= step * 1.001));
                let average = values.iter().map(|v| v[channel] as f64).sum::<f64>() / 256.0;
                assert!(
                    (average - expected as f64).abs() <= step as f64 / 256.0 + 1e-10,
                    "R{bits} mean drift: {expected} -> {average}"
                );
            }
        }
    });
}

#[test]
#[ignore = "requires a GPU adapter; run explicitly with --ignored"]
fn rendered_energy_survives_light_growth_overflow_and_removal() {
    rendered_energy_survives_light_growth_overflow_and_removal_preset(
        helio_pass_hlfs::HlfsConfig::default(),
    );
}

fn rendered_energy_survives_light_growth_overflow_and_removal_preset(
    preset: helio_pass_hlfs::HlfsConfig,
) {
    use helio_pass_hlfs::{HlfsConfig, HlfsDebugMode};
    use support::*;
    pollster::block_on(async {
        eprintln!("Dynamic preset: {preset:?}");
        let mut f = Fixture::new(65, 49).await;
        if preset.sample_scale == 2 {
            f.compact_output();
        }
        for count in [0usize, 1, 2, 65, 257, 1024] {
            f.lights(
                (0..count)
                    .map(|_| point([0.0, 0.0, 2.0], [1.0, 0.8, 0.5], 8.0 / count.max(1) as f32))
                    .collect(),
            );
            f.config(HlfsConfig {
                debug_mode: HlfsDebugMode::Reference,
                ..preset
            });
            f.frame();
            let reference = f.read();
            assert!(reference.iter().flatten().all(|v| v.is_finite()));
            if count > 0 {
                assert!(mean(&reference) > 0.05, "nonzero direct light required");
            }
            f.config(preset);
            for _ in 0..32 {
                f.frame();
            }
            let sampled = f.read();
            assert!(sampled.iter().flatten().all(|v| v.is_finite()));
            let error = (mean(&sampled) - mean(&reference)).abs() / mean(&reference).max(0.001);
            eprintln!(
                "lights={count} reference={} sampled={} relative mean error={error}",
                mean(&reference),
                mean(&sampled)
            );
            assert!(
                error < 0.08,
                "lighting energy drift at {count} lights: {error}"
            );
        }
        // Same population, extinguished intensities must reject old energy too.
        f.lights(vec![point([0.0, 0.0, 2.0], [1.0; 3], 0.0); 1024]);
        f.frame();
        assert!(
            mean(&f.read()) < 0.02,
            "same-count light changes leave stale energy"
        );
        f.lights(vec![]);
        f.frame();
        let dark = f.read();
        assert!(
            mean(&dark) < 0.02,
            "removed lights persist in history: {}",
            mean(&dark)
        );
        let ambient = mean(&dark);
        f.lights(vec![point([0.0, 0.0, 2.0], [1.0; 3], 0.005)]);
        let mut means = Vec::new();
        for mode in [HlfsDebugMode::Reference, HlfsDebugMode::Final] {
            f.config(HlfsConfig {
                debug_mode: mode,
                pre_exposure: 0.01,
                ..preset
            });
            let mut total = 0.0;
            for _ in 0..32 {
                f.frame();
                total += mean(&f.read()) / 32.0;
            }
            means.push(total - ambient);
        }
        eprintln!(
            "Low pre-exposure direct reference={} final={}",
            means[0], means[1]
        );
        assert!(
            means[0] > 0.00001 && means[1] > means[0] * 0.9,
            "exposure threshold removes perceptible direct light"
        );
    });
}

#[test]
#[ignore = "requires a GPU adapter; run explicitly with --ignored"]
fn hidden_strong_light_is_discovered_after_occlusion_changes() {
    hidden_strong_light_is_discovered_after_occlusion_changes_preset(
        helio_pass_hlfs::HlfsConfig::default(),
    );
}

fn hidden_strong_light_is_discovered_after_occlusion_changes_preset(
    preset: helio_pass_hlfs::HlfsConfig,
) {
    use helio_pass_hlfs::{HlfsConfig, HlfsDebugMode};
    use support::*;
    pollster::block_on(async {
        eprintln!("Dynamic preset: {preset:?}");
        let mut f = Fixture::new(65, 49).await;
        if preset.sample_scale == 2 {
            f.compact_output();
        }
        let mut lights: Vec<_> = (0..32)
            .map(|i| {
                point(
                    [(i % 8) as f32 / 2.0 - 2.0, (i / 8) as f32 - 2.0, 2.0],
                    [1.0, 0.8, 0.5],
                    0.2,
                )
            })
            .collect();
        let mut strong = point([0.0, 0.0, 2.0], [1.0; 3], 256.0);
        strong.shadow_index = 0;
        lights.push(strong);
        f.lights(lights);
        for visibility in [0.0, 1.0] {
            f.constant_shadow(visibility);
            f.config(HlfsConfig {
                debug_mode: HlfsDebugMode::Reference,
                ..preset
            });
            f.frame();
            let reference = mean(&f.read());
            f.config(preset);
            // Warm up with the strong light occluded, then reveal it without a
            // configuration or light-count change that could reset guiding.
            f.constant_shadow(0.0);
            for _ in 0..32 {
                f.frame();
            }
            f.constant_shadow(visibility);
            let mut measured = 0.0;
            for frame in 0..64 {
                f.frame();
                if frame >= 32 {
                    measured += mean(&f.read()) / 32.0;
                }
            }
            let error = (measured - reference).abs() / reference;
            eprintln!("Strong hidden light visibility={visibility}: reference={reference} measured={measured} relative_error={error}");
            assert!(
                error < 0.08,
                "visibility guiding fails after occlusion change"
            );
        }
    });
}

#[test]
#[ignore = "requires a GPU adapter; run explicitly with --ignored"]
fn mixed_lighting_converges_at_full_and_half_resolution() {
    use helio_pass_hlfs::{HlfsConfig, HlfsDebugMode};
    use support::*;
    pollster::block_on(async {
        let mut f = Fixture::new(129, 97).await;
        let mut lights: Vec<_> = (0..128)
            .map(|i| {
                let x = (i % 16) as f32 / 4.0 - 2.0;
                let y = (i / 16) as f32 / 2.0 - 2.0;
                point(
                    [x, y, 0.6 + (i % 3) as f32],
                    [
                        (i % 3 == 0) as u8 as f32,
                        (i % 3 == 1) as u8 as f32,
                        (i % 3 == 2) as u8 as f32,
                    ],
                    0.2,
                )
            })
            .collect();
        lights.push(helio_light_api::GpuLight {
            light_type: 0,
            direction_outer: [0.0, 0.0, -1.0, 0.0],
            color_intensity: [1.0, 0.8, 0.5, 3.0],
            ..Default::default()
        });
        f.lights(lights);
        f.config(HlfsConfig {
            debug_mode: HlfsDebugMode::Reference,
            ..Default::default()
        });
        f.frame();
        let reference = f.read();
        f.config(HlfsConfig {
            debug_mode: HlfsDebugMode::Unfiltered,
            ..Default::default()
        });
        let mut raw_mean = 0.0;
        for _ in 0..64 {
            f.frame();
            raw_mean += mean(&f.read()) / 64.0;
        }
        eprintln!(
            "reference mean={}, unfiltered mean={raw_mean}",
            mean(&reference)
        );
        assert!(
            (raw_mean - mean(&reference)).abs() / mean(&reference) < 0.01,
            "unfiltered estimator loses energy"
        );
        for (scale, samples, compact) in [
            (1, 2, false),
            (2, 2, false),
            (2, 4, false),
            (2, 2, true),
            (2, 4, true),
        ] {
            if compact {
                f.compact_output();
            }
            f.config(HlfsConfig {
                sample_scale: scale,
                samples_per_pixel: samples,
                ..Default::default()
            });
            for _ in 0..64 {
                f.frame();
            }
            let sampled = f.read();
            let error = (mean(&sampled) - mean(&reference)).abs() / mean(&reference);
            let rmse = (sampled
                .iter()
                .zip(&reference)
                .flat_map(|(a, b)| a.iter().zip(b).map(|(a, b)| (a - b) * (a - b)))
                .sum::<f32>()
                / (sampled.len() * 3) as f32)
                .sqrt()
                / mean(&reference);
            eprintln!(
                "mixed lights scale={scale} samples={samples} compact={compact}: relative mean error={error}, normalized RMSE={rmse}"
            );
            if let Ok(dir) = std::env::var("HLFS_CAPTURE_DIR") {
                std::fs::create_dir_all(&dir).unwrap();
                let save = |name: &str, pixels: &[[f32; 3]]| {
                    let mut image = image::RgbImage::new(f.width, f.height);
                    for (out, p) in image.pixels_mut().zip(pixels) {
                        *out =
                            image::Rgb(p.map(|v| ((v / (1.0 + v)).powf(1.0 / 2.2) * 255.0) as u8));
                    }
                    image.save(std::path::Path::new(&dir).join(name)).unwrap();
                };
                save("mixed-reference.png", &reference);
                save(
                    &format!("mixed-scale-{scale}-samples-{samples}-compact-{compact}.png"),
                    &sampled,
                );
                // Preserve linear HDR values for exact before/after shader comparisons.
                std::fs::write(
                    std::path::Path::new(&dir).join(format!(
                        "mixed-scale-{scale}-samples-{samples}-compact-{compact}.f32"
                    )),
                    bytemuck::cast_slice(&sampled),
                )
                .unwrap();
            }
            assert!(
                error < 0.08 && rmse < 0.2,
                "mixed lighting fails to converge"
            );
        }
        // Same population, changed intensities: temporal rejection must work
        // without relying on a light-count change to reset the pass.
        f.lights(
            (0..129)
                .map(|_| point([0.0, 0.0, 2.0], [1.0; 3], 0.0))
                .collect(),
        );
        f.frame();
        assert!(
            mean(&f.read()) < 0.02,
            "extinguished lights survive photometric rejection"
        );
    });
}

#[test]
#[ignore = "requires a GPU adapter; run explicitly with --ignored"]
fn creates_pipelines_on_available_adapter() {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor().with_env());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .expect("GPU adapter required for HLFS validation");
        eprintln!("HLFS adapter: {:?}", adapter.get_info());
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .unwrap();
        let mut pass = helio_pass_hlfs::HlfsPass::new(
            &device,
            &queue,
            1920,
            1080,
            wgpu::TextureFormat::Rgba16Float,
        );
        assert_eq!(pass.config().mode, helio_pass_hlfs::HlfsMode::ScreenSpace);
        eprintln!("1080p full allocation_bytes={}", pass.allocation_bytes());
        let output_before = pass.output_texture().clone();
        assert!(
            pass.allocation_bytes() < 128 * 1024 * 1024,
            "1080p exceeds the removed clip-stack allocation"
        );
        pass.set_config(
            &device,
            helio_pass_hlfs::HlfsConfig {
                sample_scale: 2,
                ..Default::default()
            },
        );
        assert_eq!(
            &output_before,
            pass.output_texture(),
            "shading-scale changes replace the published output"
        );
        eprintln!("1080p half allocation_bytes={}", pass.allocation_bytes());
        assert!(
            pass.allocation_bytes() < 64 * 1024 * 1024,
            "half-resolution budget exceeded"
        );
    });
}

#[test]
#[ignore = "requires a GPU adapter; run explicitly with --ignored"]
fn shadows_use_atlas_layers_independently_of_light_index() {
    use helio_pass_hlfs::{HlfsConfig, HlfsDebugMode};
    use support::*;
    pollster::block_on(async {
        let mut f = Fixture::new(33, 25).await;
        let mut lights = vec![point([0.0, 0.0, 2.0], [1.0; 3], 0.0); 65];
        lights[64] = point([0.0, 0.0, 2.0], [1.0; 3], 8.0);
        lights[64].shadow_index = 0;
        f.lights(lights);
        f.config(HlfsConfig {
            debug_mode: HlfsDebugMode::Reference,
            ..Default::default()
        });
        f.constant_shadow(1.0);
        f.frame();
        assert!(mean(&f.read()) > 0.1);
        f.constant_shadow(0.0);
        f.frame();
        assert!(mean(&f.read()) < 0.02, "light 64 ignores its shadow map");
    });
}

#[test]
#[ignore = "requires a GPU adapter; run explicitly with --ignored"]
fn offscreen_lights_and_minimal_targets_remain_valid() {
    use helio_pass_hlfs::{HlfsConfig, HlfsDebugMode, HlfsPass};
    use support::*;
    pollster::block_on(async {
        let mut f = Fixture::new(65, 49).await;
        let mut lights = vec![point([1000.0, 1000.0, 2.0], [1.0; 3], 10000.0); 64];
        lights[63] = point([-1.0, -1.0, 0.5], [1.0, 0.5, 0.2], 2.0);
        lights[63].position_range[3] = 1.2;
        f.lights(lights);
        f.config(HlfsConfig {
            debug_mode: HlfsDebugMode::Reference,
            ..Default::default()
        });
        f.frame();
        let reference = f.read();
        f.config(HlfsConfig::default());
        for _ in 0..16 {
            f.frame();
        }
        let sampled = f.read();
        let error = (mean(&sampled) - mean(&reference)).abs() / mean(&reference);
        assert!(error < 0.03, "spatial light culling loses energy: {error}");
        let mut tiny = Fixture::new(1, 1).await;
        tiny.lights(vec![point([0.0, 0.0, 2.0], [1.0; 3], 8.0)]);
        for scale in [2, 1, 2] {
            tiny.config(HlfsConfig {
                sample_scale: scale,
                ..Default::default()
            });
            tiny.graph
                .find_pass_mut::<HlfsPass>()
                .unwrap()
                .resize(&tiny.device, 0, 0);
            tiny.frame();
            assert!(mean(&tiny.read()).is_finite() && mean(&tiny.read()) > 0.1);
        }
    });
}

#[test]
#[ignore = "explicit GPU benchmark; run with --ignored --nocapture"]
fn benchmark_gpu_light_scaling() {
    use helio_pass_hlfs::{HlfsConfig, HlfsDebugMode, HlfsPass};
    use support::*;
    pollster::block_on(async {
        let mut f = Fixture::new(640, 360).await;
        f.constant_shadow(1.0);
        for shadowed in [false, true] {
            for count in [64, 256, 1024] {
                f.lights(
                    (0..count)
                        .map(|i| {
                            let mut light = point(
                                [
                                    (i % 16) as f32 / 4.0 - 2.0,
                                    (i / 16 % 16) as f32 / 4.0 - 2.0,
                                    2.0,
                                ],
                                [1.0, 0.8, 0.5],
                                8.0 / count as f32,
                            );
                            if shadowed {
                                light.shadow_index = 0;
                            }
                            light
                        })
                        .collect(),
                );
                for mode in [HlfsDebugMode::Reference, HlfsDebugMode::Final] {
                    f.config(HlfsConfig {
                        debug_mode: mode,
                        ..Default::default()
                    });
                    for _ in 0..16 {
                        f.frame();
                    }
                    let mut times = Vec::new();
                    for _ in 0..40 {
                        f.frame();
                        times.push(f.milliseconds());
                    }
                    times.sort_by(f64::total_cmp);
                    eprintln!("GPU lights={count} shadowed={shadowed} mode={mode:?} median_ms={:.3} p95_ms={:.3} allocation_bytes={}",times[20],times[38],f.graph.find_pass::<HlfsPass>().unwrap().allocation_bytes());
                }
            }
        }
    });
}

#[test]
#[ignore = "explicit 1080p stage benchmark; run serially"]
fn benchmark_gameplay_resolution_stages() {
    use helio_pass_hlfs::{HlfsConfig, HlfsPass};
    use support::*;
    pollster::block_on(async {
        let mut f = Fixture::new(1920, 1080).await;
        assert!(f
            .graph
            .find_pass_mut::<HlfsPass>()
            .unwrap()
            .enable_timing(&f.device));
        f.constant_shadow(1.0);
        for (scale, spp, compact) in [(1, 2, false), (2, 2, false), (2, 4, true), (2, 2, true)] {
            if compact {
                f.compact_output();
                assert!(f
                    .graph
                    .find_pass_mut::<HlfsPass>()
                    .unwrap()
                    .enable_timing(&f.device));
            }
            for count in [1, 8, 64, 1024] {
                f.lights(
                    (0..count)
                        .map(|i| {
                            let mut light = point(
                                [
                                    (i % 16) as f32 / 4.0 - 2.0,
                                    (i / 16 % 16) as f32 / 4.0 - 2.0,
                                    2.0,
                                ],
                                [1.0, 0.8, 0.5],
                                8.0 / count as f32,
                            );
                            light.shadow_index = 0;
                            light
                        })
                        .collect(),
                );
                f.config(HlfsConfig {
                    sample_scale: scale,
                    samples_per_pixel: spp,
                    ..Default::default()
                });
                for _ in 0..16 {
                    f.frame();
                }
                let mut samples = Vec::new();
                for _ in 0..40 {
                    f.frame();
                    samples.push(f.stage_milliseconds());
                }
                let mut totals: Vec<f64> =
                    samples.iter().map(|stages| stages.iter().sum()).collect();
                totals.sort_by(f64::total_cmp);
                let medians: [f64; 6] = std::array::from_fn(|i| {
                    let mut v: Vec<_> = samples.iter().map(|s| s[i]).collect();
                    v.sort_by(f64::total_cmp);
                    v[20]
                });
                eprintln!("STAGES resolution=1920x1080 scale={scale} spp={spp} compact={compact} lights={count} median_ms={:.3} p95_ms={:.3} coarse_fine_sample_temporal_spatial_composite={medians:?} allocation_bytes={}",totals[20],totals[38],f.graph.find_pass::<HlfsPass>().unwrap().allocation_bytes());
            }
        }
    });
}

#[test]
#[ignore = "requires a GPU adapter; run explicitly with --ignored"]
fn screen_contacts_follow_current_depth_and_clear_after_motion() {
    use helio_pass_hlfs::{HlfsConfig, HlfsDebugMode};
    use support::*;
    pollster::block_on(async {
        let mut f = Fixture::new(129, 65).await;
        let mut light = point([1.0, 0.0, 0.1], [1.0; 3], 1.0);
        light.shadow_index = 0;
        f.lights(vec![light]);
        f.constant_shadow(1.0);
        for occluder_width in [0.2, 4.0 / 129.0] {
            let depths: Vec<_> = (0..65)
                .flat_map(|_| {
                    (0..129).map(|x| {
                        let wx = (x as f32 + 0.5) / 129.0 * 4.0 - 2.0;
                        let z = if wx >= 0.0 && wx < occluder_width {
                            0.05
                        } else {
                            0.0
                        };
                        ortho_depth(3.0 - z)
                    })
                })
                .collect();
            f.depth_values(&depths);
            f.config(HlfsConfig {
                screen_trace_distance: 0.0,
                debug_mode: HlfsDebugMode::Reference,
                ..Default::default()
            });
            f.frame();
            let unshadowed = f.read();
            f.config(HlfsConfig {
                screen_trace_distance: 2.0,
                debug_mode: HlfsDebugMode::Reference,
                ..Default::default()
            });
            f.frame();
            let contact = f.read();
            let region = |pixels: &[[f32; 3]]| {
                (25..40)
                    .flat_map(|y| (52..64).map(move |x| pixels[y * 129 + x][0]))
                    .sum::<f32>()
            };
            eprintln!(
                "Contact region unshadowed={} occluded={}",
                region(&unshadowed),
                region(&contact)
            );
            assert!(
                region(&contact) < region(&unshadowed) * 0.8,
                "current-depth contact shadow missing"
            );
            f.depth_values(&vec![ortho_depth(3.0); 129 * 65]);
            f.frame();
            assert!(
                region(&f.read()) > region(&unshadowed) * 0.95,
                "moving contact occluder leaves stale shadow"
            );
            for preset in [HlfsConfig::compact(), HlfsConfig::performance()] {
                f.compact_output();
                f.config(HlfsConfig {
                    screen_trace_distance: 2.0,
                    ..preset
                });
                f.depth_values(&depths);
                for _ in 0..4 {
                    f.frame();
                    assert!(
                        region(&f.read()) < region(&unshadowed) * 0.8,
                        "reduced-resolution contact shadow missing"
                    );
                }
                f.depth_values(&vec![ortho_depth(3.0); 129 * 65]);
                f.frame();
                assert!(
                    region(&f.read()) > region(&unshadowed) * 0.95,
                    "reduced-resolution contact shadow persists after removal"
                );
            }
        }
    });
}

#[test]
#[ignore = "requires a GPU adapter; run explicitly with --ignored"]
fn half_resolution_keeps_isolated_one_pixel_geometry_lit() {
    use helio_pass_hlfs::{HlfsConfig, HlfsDebugMode};
    use support::*;
    pollster::block_on(async {
        for preset in [
            HlfsConfig::default(),
            HlfsConfig {
                sample_scale: 2,
                ..Default::default()
            },
            HlfsConfig::performance(),
        ] {
            eprintln!("Dynamic preset: {preset:?}");
            let mut f = Fixture::new(65, 49).await;
            if preset.sample_scale == 2 {
                f.compact_output();
            }
            f.lights(vec![point([0.0, 0.0, 2.0], [1.0; 3], 8.0)]);
            let index = 20 * 65 + 21;
            let mut depths = vec![helio_core::depth::CLEAR; 65 * 49];
            depths[index] = ortho_depth(3.0);
            f.depth_values(&depths);
            f.config(HlfsConfig {
                debug_mode: HlfsDebugMode::Reference,
                ..preset
            });
            f.frame();
            let reference = f.read()[index];
            assert!(reference[0] > 0.1);
            f.config(HlfsConfig {
                sample_scale: 2,
                ..preset
            });
            for phase in 0..4 {
                f.scene.frame_count = phase;
                f.frame();
                let pixels = f.read();
                assert!(
                (pixels[index][0] - reference[0]).abs() < reference[0] * (2.0 / 64.0) + 0.001,
                "thin surface exceeds two R11 rounding steps in phase {phase}: {:?}, reference={reference:?}",
                pixels[index]
            );
                assert!(
                    pixels
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| *i != index)
                        .all(|(_, p)| p[0] == 0.0),
                    "lighting leaks into sky"
                );
            }
        }
    });
}

#[test]
#[ignore = "requires a GPU adapter; run explicitly with --ignored"]
fn packed_grid_overflow_and_compact_output_preserve_energy() {
    use helio_pass_hlfs::{HlfsConfig, HlfsDebugMode, HlfsPass};
    use support::*;
    pollster::block_on(async {
        let mut f = Fixture::new(9, 9).await;
        f.lights(vec![point([0.0, 0.0, 2.0], [1.0; 3], 8.0)]);
        f.config(HlfsConfig {
            debug_mode: HlfsDebugMode::Reference,
            ..Default::default()
        });
        f.frame();
        let expected = mean(&f.read());
        f.lights(vec![point([0.0, 0.0, 2.0], [1.0; 3], 8.0 / 65536.0); 65536]);
        f.config(HlfsConfig {
            debug_mode: HlfsDebugMode::Unfiltered,
            ..Default::default()
        });
        let mut measured = 0.0;
        for _ in 0..32 {
            f.frame();
            measured += mean(&f.read()) / 32.0;
        }
        assert!(
            (measured - expected).abs() / expected < 0.04,
            "16-bit overflow loses light energy: {measured} versus {expected}"
        );
        f.lights(vec![point([0.0, 0.0, 2.0], [1.0; 3], 8.0)]);
        f.compact_output();
        for _ in 0..32 {
            f.frame();
        }
        let measured = mean(&f.read());
        assert!(
            (measured - expected).abs() / expected < 0.04,
            "compact output changes energy"
        );
        let mut pass = HlfsPass::new(
            &f.device,
            &f.queue,
            1920,
            1080,
            HlfsPass::preferred_output_format(&f.device),
        );
        pass.set_config(&f.device, HlfsConfig::performance());
        eprintln!(
            "1080p performance allocation_bytes={}",
            pass.allocation_bytes()
        );
        if f.device
            .features()
            .contains(wgpu::Features::RG11B10UFLOAT_RENDERABLE)
        {
            assert!(
                pass.allocation_bytes() < 34 * 1024 * 1024,
                "compact 1080p preset exceeds 34 MiB"
            );
        }
    });
}

#[test]
#[ignore = "requires a GPU adapter; run explicitly with --ignored"]
fn confidence_tracks_visible_energy_coverage() {
    use helio_pass_hlfs::{HlfsConfig, HlfsDebugMode};
    use support::*;
    pollster::block_on(async {
        let mut f = Fixture::new(33, 25).await;
        let mut coverage = Vec::new();
        for dominant in [false, true] {
            f.lights(
                (0..16)
                    .map(|i| {
                        point(
                            [0.0, 0.0, 2.0],
                            [1.0; 3],
                            if dominant && i == 0 { 256.0 } else { 1.0 },
                        )
                    })
                    .collect(),
            );
            f.config(HlfsConfig {
                debug_mode: HlfsDebugMode::Confidence,
                ..Default::default()
            });
            for _ in 0..32 {
                f.frame();
            }
            let mut sum = 0.0;
            for _ in 0..16 {
                f.frame();
                sum += mean(&f.read()) / 16.0;
            }
            coverage.push(sum);
        }
        eprintln!(
            "Confidence balanced={} dominant={}",
            coverage[0], coverage[1]
        );
        assert!(
            coverage[0] < 0.1 && coverage[1] > 0.8,
            "confidence does not reflect 80% visible energy coverage"
        );
    });
}

#[test]
#[ignore = "requires a GPU adapter; run explicitly with --ignored"]
fn reduced_resolution_preserves_energy_and_discovers_hidden_lights() {
    for preset in [
        helio_pass_hlfs::HlfsConfig::compact(),
        helio_pass_hlfs::HlfsConfig::performance(),
    ] {
        rendered_energy_survives_light_growth_overflow_and_removal_preset(preset);
        hidden_strong_light_is_discovered_after_occlusion_changes_preset(preset);
    }
}

#[test]
#[ignore = "requires a GPU adapter; run explicitly with --ignored"]
fn raw_and_filtered_energy_match_at_odd_and_even_sizes() {
    use helio_pass_hlfs::{HlfsConfig, HlfsDebugMode};
    use support::*;
    pollster::block_on(async {
        for (width, height, scale, spp) in [
            (65, 49, 1, 2),
            (65, 49, 2, 2),
            (65, 49, 2, 4),
            (64, 48, 2, 2),
            (64, 48, 2, 4),
        ] {
            let mut f = Fixture::new(width, height).await;
            f.compact_output();
            f.lights(vec![
                point([0.0, 0.0, 2.0], [1.0, 0.8, 0.5], 8.0 / 65.0);
                65
            ]);
            f.config(HlfsConfig {
                debug_mode: HlfsDebugMode::Reference,
                ..Default::default()
            });
            f.frame();
            let reference = mean(&f.read());
            for debug_mode in [HlfsDebugMode::Unfiltered, HlfsDebugMode::Final] {
                f.config(HlfsConfig {
                    sample_scale: scale,
                    samples_per_pixel: spp,
                    debug_mode,
                    ..Default::default()
                });
                f.scene.frame_count = 0;
                let mut sum = 0.0;
                for frame in 0..96 {
                    f.frame();
                    let m = mean(&f.read());
                    if frame >= 32 {
                        sum += m / 64.0;
                    }
                    if [0, 3, 15, 31, 63, 95].contains(&frame) {
                        eprintln!("PROBE size={width}x{height} scale={scale} spp={spp} mode={debug_mode:?} frame={frame} mean={m} reference={reference}");
                    }
                }
                assert!(
                    (sum - reference).abs() / reference < 0.08,
                    "raw/filtered energy exceeds existing 8% limit"
                );
                eprintln!("PROBE_AVG size={width}x{height} scale={scale} spp={spp} mode={debug_mode:?} mean={sum} reference={reference} relative_error={}",(sum-reference)/reference);
            }
        }
    });
}

#[test]
#[ignore = "requires a GPU adapter; run explicitly with --ignored"]
fn reduced_resolution_tracks_camera_motion() {
    use glam::{Mat4, Vec3};
    use helio_pass_hlfs::{HlfsConfig, HlfsDebugMode};
    use support::*;
    pollster::block_on(async {
        for (width, height) in [(65, 49), (128, 96)] {
            for spp in [2, 4] {
                let mut f = Fixture::new(width, height).await;
                let mut oracle = Fixture::new(width, height).await;
                f.compact_output();
                oracle.compact_output();
                let lights: Vec<_> = (0..65)
                    .map(|i| {
                        point(
                            [
                                (i % 8) as f32 / 2.0 - 2.0,
                                (i / 8) as f32 / 2.0 - 2.0,
                                1.0 + (i % 3) as f32,
                            ],
                            [1.0, 0.8, 0.5],
                            8.0 / 65.0,
                        )
                    })
                    .collect();
                f.lights(lights.clone());
                oracle.lights(lights);
                f.config(HlfsConfig {
                    sample_scale: 2,
                    samples_per_pixel: spp,
                    ..Default::default()
                });
                oracle.config(HlfsConfig {
                    debug_mode: HlfsDebugMode::Reference,
                    ..Default::default()
                });
                let proj = helio_core::depth::orthographic(-2.0, 2.0, -2.0, 2.0, 0.1, 10.0);
                let mut previous =
                    proj * Mat4::look_at_rh(Vec3::new(0.0, 0.0, 3.0), Vec3::ZERO, Vec3::Y);
                let mut worst_mean = 0.0f32;
                let mut worst_rmse = 0.0f32;
                for frame in 0..64 {
                    let x = if frame < 16 {
                        0.0
                    } else {
                        ((frame - 16) as f32 * 0.1).sin() * 0.75
                    };
                    let position = Vec3::new(x, 0.0, 3.0);
                    let view = Mat4::look_at_rh(position, Vec3::new(x, 0.0, 0.0), Vec3::Y);
                    for fixture in [&mut f, &mut oracle] {
                        fixture
                            .scene
                            .camera
                            .update(helio_core::GpuCameraUniforms::new(
                                view, proj, position, 0.1, 10.0, frame, [0.0; 2], previous,
                            ));
                        fixture.frame();
                    }
                    previous = proj * view;
                    let actual = f.read();
                    let reference = oracle.read();
                    if frame >= 16 {
                        let error = (mean(&actual) - mean(&reference)).abs() / mean(&reference);
                        let rmse = (actual
                            .iter()
                            .zip(&reference)
                            .flat_map(|(a, b)| a.iter().zip(b).map(|(a, b)| (a - b).powi(2)))
                            .sum::<f32>()
                            / (actual.len() * 3) as f32)
                            .sqrt()
                            / mean(&reference);
                        worst_mean = worst_mean.max(error);
                        worst_rmse = worst_rmse.max(rmse);
                    }
                }
                eprintln!("MOTION size={width}x{height} spp={spp} worst_mean_error={worst_mean} worst_nrmse={worst_rmse}");
                assert!(
                    worst_mean < 0.08 && worst_rmse < 0.2,
                    "moving camera exceeds existing energy/image limits"
                );
            }
        }
    });
}
