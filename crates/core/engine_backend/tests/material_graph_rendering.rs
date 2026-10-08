//! Regression coverage across PSGC, the runtime adapter, Radiant and GPU sampling.
//! Run: cargo test -p engine_backend --test material_graph_rendering -- --nocapture
//! GPU tests require an adapter; absence is a failure, never a silent pass.
#![cfg(feature = "render")]

// Material resolution lives with the mesh component (Pulsar-Native#1035
// Phase 2): `StaticMeshDraw` lowers graph slots through these modules.
use helio_component::{material_graph, material_textures};

use psgc::*;
use std::collections::HashMap;

fn node(id: &str, kind: &str) -> NodeInstance {
    let meta = get_shader_nodes()
        .into_iter()
        .find(|m| m.name == kind)
        .unwrap();
    let mut node = NodeInstance::new(id, kind, Position { x: 0.0, y: 0.0 });
    for p in meta.params {
        node.inputs.push(PinInstance::new(
            &p.name,
            Pin::new(
                &p.name,
                &p.name,
                DataType::Data(TypeInfo::new(&p.param_type)),
                PinType::Input,
            ),
        ));
    }
    node
}

fn connect(graph: &mut GraphDescription, from: &str, to: &str, pin: &str) {
    graph.add_connection(Connection::new(
        from,
        "result",
        to,
        pin,
        ConnectionType::Data,
    ));
}

fn texture_graph(factor: f64, opacity: Option<f64>, emissive: bool) -> GraphDescription {
    let mut graph = GraphDescription::new("texture regression");
    let mut output = node("output", "fragment_output");
    if let Some(value) = opacity {
        output
            .properties
            .insert("opacity".into(), serde_json::json!(value));
    }
    graph.add_node(output);
    graph.add_node(node("uv", "frag_uv"));
    let mut texture = node("texture", "sample_texture");
    texture
        .properties
        .insert("texture".into(), serde_json::json!("test.png"));
    graph.add_node(texture);
    let mut multiply = node("multiply", "color_multiply");
    multiply
        .properties
        .insert("factor".into(), serde_json::json!(factor));
    graph.add_node(multiply);
    connect(&mut graph, "uv", "texture", "uv");
    connect(&mut graph, "texture", "multiply", "color");
    connect(&mut graph, "multiply", "output", "base_color");
    if emissive {
        graph.add_node(node("rainbow", "rainbow"));
        connect(&mut graph, "uv", "rainbow", "uv");
        connect(&mut graph, "rainbow", "output", "emissive_color");
    }
    graph
}

fn adapted(graph: &GraphDescription, slot: u32) -> String {
    material_graph::compile_material_graph(
        graph,
        &HashMap::from([("test.png".into(), slot), ("second.png".into(), 1 - slot)]),
    )
    .unwrap()
}

#[test]
fn defaults_and_explicit_zero_are_distinct() {
    let mut graph = GraphDescription::new("defaults");
    graph.add_node(node("output", "fragment_output"));
    let source = compile_shader(&graph).unwrap();
    assert!(source.contains("return FragmentOutput(vec4<f32>(1.0, 1.0, 1.0, 1.0), 0.0, 0.5, vec4<f32>(0.0, 0.0, 0.0, 0.0), normal, 1.0, 1.0, 1.0)"), "{source}");
    graph
        .nodes
        .get_mut("output")
        .unwrap()
        .properties
        .insert("opacity".into(), serde_json::json!(0.0));
    let source = compile_shader(&graph).unwrap();
    assert!(source.contains("normal, 1.0, 0, 1.0)"), "{source}");
}

#[test]
fn connected_outputs_override_defaults() {
    let graph = texture_graph(2.0, Some(0.25), true);
    let source = adapted(&graph, 1);
    assert!(source.contains("scene_textures[1u]"));
    assert!(source.contains("scene_samplers[1u]"));
    assert!(!source.contains("texture_sampler"));
    assert!(!source.contains("test.png"));
    assert!(source.contains("let uv = input.tex_coords"));
    assert!(source.contains("graph_surface.emissive_color"));
}

#[test]
fn every_texture_sampling_variant_binds_its_own_sampler() {
    for kind in [
        "sample_texture",
        "sample_texture_level",
        "sample_texture_grad",
    ] {
        let mut graph = texture_graph(1.0, None, false);
        let mut texture = node("texture", kind);
        texture
            .properties
            .insert("texture".into(), serde_json::json!("test.png"));
        graph.nodes.insert("texture".into(), texture);
        let source = adapted(&graph, 1);
        assert!(source.contains("scene_samplers[1u]"), "{kind}: {source}");
        assert!(!source.contains("texture_sampler"), "{kind}: {source}");
    }
}

#[test]
fn missing_live_texture_is_an_error_but_disconnected_texture_is_not_loaded() {
    let mut graph = texture_graph(1.0, None, false);
    assert!(
        material_graph::compile_material_graph(&graph, &HashMap::new())
            .unwrap_err()
            .contains("no scene binding")
    );
    graph.add_node(node("unused", "sample_texture"));
    assert_eq!(
        material_graph::texture_assets(&graph).unwrap(),
        vec!["test.png"]
    );
    adapted(&graph, 0);
    graph.nodes.get_mut("texture").unwrap().properties.clear();
    assert!(material_graph::texture_assets(&graph)
        .unwrap_err()
        .contains("no texture asset"));
}

#[test]
fn texture_asset_names_cannot_rewrite_shader_identifiers() {
    let mut graph = texture_graph(1.0, None, false);
    graph
        .nodes
        .get_mut("texture")
        .unwrap()
        .properties
        .insert("texture".into(), serde_json::json!("normal"));
    let source =
        material_graph::compile_material_graph(&graph, &HashMap::from([("normal".into(), 1)]))
            .unwrap();
    assert!(source.contains("let normal = input.world_normal;"));
    assert!(source.contains("scene_textures[1u]"));
}

#[test]
fn overlapping_asset_names_are_not_replaced_inside_each_other() {
    let mut graph = texture_graph(1.0, None, false);
    let mut second = node("second", "sample_texture");
    second
        .properties
        .insert("texture".into(), serde_json::json!("folder/test.png"));
    graph.add_node(second);
    connect(&mut graph, "uv", "second", "uv");
    connect(&mut graph, "second", "output", "emissive_color");
    for _ in 0..32 {
        let bindings = HashMap::from([("test.png".into(), 0), ("folder/test.png".into(), 1)]);
        let source = material_graph::compile_material_graph(&graph, &bindings).unwrap();
        assert!(
            !source.contains("folder/"),
            "corrupted texture expression: {source}"
        );
        assert!(source.contains("scene_textures[1u]"));
    }
}

const PROBE: &str = r#"
@vertex fn probe_vertex(@builtin(vertex_index) index: u32) -> VertexOutput {
    let x = f32((index << 1u) & 2u);
    let y = f32(index & 2u);
    var result: VertexOutput;
    result.clip_position = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    result.tex_coords = vec2<f32>(x, y);
    result.world_normal = vec3<f32>(0.0, 1.0, 0.0);
    result.world_tangent = vec3<f32>(1.0, 0.0, 0.0);
    result.bitangent_sign = 1.0;
    return result;
}
@fragment fn probe_fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    var material: GpuMaterial;
    material.base_color = vec4<f32>(1.0);
    material.roughness_metallic = vec4<f32>(0.5, 0.0, 1.5, 1.0);
    var textures: MaterialTextureData;
    textures.base_color.texture_index = NO_TEXTURE;
    textures.normal.texture_index = NO_TEXTURE;
    textures.roughness_metallic.texture_index = NO_TEXTURE;
    textures.emissive.texture_index = NO_TEXTURE;
    textures.occlusion.texture_index = NO_TEXTURE;
    textures.specular_color.texture_index = NO_TEXTURE;
    textures.specular_weight.texture_index = NO_TEXTURE;
    let surface = radiant_eval_surface(material, textures, input);
    switch u32(input.clip_position.y) {
        case 0u: { return vec4<f32>(surface.albedo.rgb, surface.alpha); }
        case 1u: { return vec4<f32>(surface.ao, surface.roughness, surface.metallic, surface.alpha); }
        case 2u: { return vec4<f32>(surface.normal, surface.alpha); }
        default: { return vec4<f32>(surface.emissive, surface.alpha); }
    }
}
"#;

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
}
impl Gpu {
    fn new(bindless: bool) -> Self {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        if let Ok(backend) = std::env::var("PULSAR_MATERIAL_TEST_BACKEND") {
            descriptor.backends = match backend.as_str() {
                "dx12" => wgpu::Backends::DX12,
                "vulkan" => wgpu::Backends::VULKAN,
                _ => panic!("PULSAR_MATERIAL_TEST_BACKEND must be dx12 or vulkan"),
            };
        }
        let instance = wgpu::Instance::new(descriptor);
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .expect("GPU adapter required");
        eprintln!(
            "material regression adapter: {:?}; bindless={bindless}",
            adapter.get_info()
        );
        let features = if bindless {
            helio_mats::BINDLESS_MATERIAL_FEATURES
        } else {
            wgpu::Features::empty()
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: features,
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .unwrap();
        Self { device, queue }
    }

    fn render(&self, graph: &GraphDescription, slot: u32, bindless: bool) -> Vec<[f32; 4]> {
        self.render_with_uvs(graph, slot, bindless, false)
    }

    fn render_with_uvs(
        &self,
        graph: &GraphDescription,
        slot: u32,
        bindless: bool,
        missing_uvs: bool,
    ) -> Vec<[f32; 4]> {
        let template = helio_mats::RadiantTemplateRegistry::new();
        let mut source = template
            .get(helio_mats::MATERIAL_CLASS_DEFAULT)
            .unwrap()
            .build_shader_source(&adapted(graph, slot), 2);
        if !bindless {
            source = helio_mats::apply_webgpu_material_bindings(&source, 2);
        }
        source.push_str(&if missing_uvs {
            PROBE.replace(
                "result.tex_coords = vec2<f32>(x, y);",
                "result.tex_coords = vec2<f32>(0.0);",
            )
        } else {
            PROBE.to_string()
        });
        let device = &self.device;
        let binding_config = helio_mats::MaterialBindingConfig {
            mode: if bindless {
                helio_mats::MaterialBindingMode::BindingArray
            } else {
                helio_mats::MaterialBindingMode::Expanded
            },
            max_textures: 2,
        };
        let mut layout_entries = Vec::new();
        binding_config.append_layout_entries(&mut layout_entries, 2, wgpu::ShaderStages::FRAGMENT);
        let textures_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &layout_entries,
        });
        let empty_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&empty_layout), Some(&textures_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("real Radiant material with readback probe"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("material pixel regression"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("probe_vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("probe_fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba32Float,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let size = wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
        };
        let output = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("float readback"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let mut views = Vec::new();
        for pixels in [
            vec![128, 128, 128, 255, 255, 255, 0, 128],
            vec![255, 0, 0, 255, 0, 255, 0, 128],
        ] {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("two texel fixture"),
                size: wgpu::Extent3d {
                    width: 2,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.queue.write_texture(
                texture.as_image_copy(),
                &pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(8),
                    rows_per_image: Some(1),
                },
                texture.size(),
            );
            views.push(texture.create_view(&Default::default()));
        }
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
        let view_refs = [&views[0], &views[1]];
        let sampler_refs = [&sampler, &sampler];
        let entries = if bindless {
            vec![
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureViewArray(&view_refs),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::SamplerArray(&sampler_refs),
                },
            ]
        } else {
            vec![
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&views[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ]
        };
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(1),
            entries: &entries,
        });
        let empty = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[],
        });
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 1024,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        let target = output.create_view(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &empty, &[]);
            pass.set_bind_group(1, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            output.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(4),
                },
            },
            size,
        );
        self.queue.submit([encoder.finish()]);
        staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let data = staging.slice(..).get_mapped_range().unwrap();
        let pixels = (0..4)
            .flat_map(|y| (0..4).map(move |x| (y, x)))
            .map(|(y, x)| {
                std::array::from_fn(|c| {
                    f32::from_le_bytes(
                        data[y * 256 + x * 16 + c * 4..y * 256 + x * 16 + c * 4 + 4]
                            .try_into()
                            .unwrap(),
                    )
                })
            })
            .collect();
        drop(data);
        staging.unmap();
        pixels
    }
}

fn close(actual: [f32; 4], expected: [f32; 4]) {
    for (a, e) in actual.into_iter().zip(expected) {
        assert!(
            (a - e).abs() < 0.006,
            "actual {actual:?}, expected {expected:?}"
        );
    }
}

fn pixel_suite(bindless: bool) {
    let gpu = Gpu::new(bindless);
    for slot in [0, 1] {
        for factor in [0.5, 1.0, 2.0] {
            for opacity in [None, Some(0.0), Some(0.25), Some(-1.0), Some(2.0)] {
                let pixels = gpu.render(&texture_graph(factor, opacity, false), slot, bindless);
                let opacity = opacity.unwrap_or(1.0).clamp(0.0, 1.0) as f32;
                let factor = factor as f32;
                let alpha = factor.min(1.0) * opacity;
                let gray = 0.21586 * factor; // sRGB 128 decoded to linear, before multiplication.
                let left = if slot == 0 {
                    [gray, gray, gray, alpha]
                } else {
                    [factor, 0.0, 0.0, alpha]
                };
                let right_alpha = (128.0 / 255.0 * factor).min(1.0) * opacity;
                let right = if slot == 0 {
                    [factor, factor, 0.0, right_alpha]
                } else {
                    [0.0, factor, 0.0, right_alpha]
                };
                close(pixels[0], left);
                close(pixels[3], right);
                close(pixels[4], [1.0, 0.5, 0.0, alpha]);
                close(pixels[8], [0.0, 1.0, 0.0, alpha]);
                close(pixels[12], [0.0, 0.0, 0.0, alpha]);
            }
        }
    }
    let pixels = gpu.render(&texture_graph(1.0, None, true), 1, bindless);
    close(pixels[0], [1.0, 0.0, 0.0, 1.0]);
    assert_ne!(pixels[12], pixels[15], "emissive must vary with mesh UV");
    assert!(pixels[12][..3].iter().any(|v| *v > 0.1));
    let missing = gpu.render_with_uvs(&texture_graph(1.0, None, true), 1, bindless, true);
    close(missing[0], missing[3]);
    close(missing[12], missing[15]);
    // rainbow(uv.x=0, shift=0) = .5 + .5*cos([0, 2*pi/3, 4*pi/3]).
    close(missing[12], [1.0, 0.25, 0.25, 1.0]); // The reported flat pink emission.
    for kind in ["sample_texture_level", "sample_texture_grad"] {
        let mut graph = texture_graph(1.0, None, false);
        let mut texture = node("texture", kind);
        texture
            .properties
            .insert("texture".into(), serde_json::json!("test.png"));
        graph.nodes.insert("texture".into(), texture);
        let pixels = gpu.render(&graph, 1, bindless);
        close(pixels[0], [1.0, 0.0, 0.0, 1.0]);
        close(pixels[3], [0.0, 1.0, 0.0, 128.0 / 255.0]);
    }
    let mut graph = texture_graph(1.0, None, false);
    let mut second = node("second", "sample_texture");
    second
        .properties
        .insert("texture".into(), serde_json::json!("second.png"));
    graph.add_node(second);
    connect(&mut graph, "uv", "second", "uv");
    connect(&mut graph, "second", "output", "emissive_color");
    let pixels = gpu.render(&graph, 1, bindless);
    close(pixels[0], [1.0, 0.0, 0.0, 1.0]);
    close(pixels[12], [0.21586, 0.21586, 0.21586, 1.0]);
}

#[test]
fn malformed_vector_properties_fail_during_graph_compilation() {
    for value in [
        serde_json::json!([1, 2]),
        serde_json::json!([1, 2, 3, "bad"]),
    ] {
        let mut graph = GraphDescription::new("invalid vector");
        let mut output = node("output", "fragment_output");
        output.properties.insert("base_color".into(), value);
        graph.add_node(output);
        assert!(compile_shader(&graph)
            .unwrap_err()
            .to_string()
            .contains("numeric components"));
    }
}

#[test]
fn gpu_mesh_upload_preserves_uv0_uv1_normals_and_stride() {
    use helio_component::components::StaticMeshComponent;
    use pulsar_scenedb::gpu::*;
    use std::sync::Arc;
    let gpu = Gpu::new(false);
    let context = EngineGpuContext::new(Arc::new(gpu.device), Arc::new(gpu.queue));
    let mut store = SceneGpuStore::new(
        &context,
        SceneGpuConfig {
            classes: vec![RegionClassConfig {
                capacity: 8,
                max_resident_cells: 1,
            }],
            tombstone_headroom: 1,
            max_cells_metadata: 8,
        },
    );
    StaticMeshComponent::register_gpu_columns_growable(&mut store, 8, context.device());
    let store = Arc::new(store);
    let mut world = pulsar_scenedb::World::new();
    world.attach_gpu_mirror(GpuMirrorHandle::new(
        store.clone(),
        Arc::clone(context.queue()),
    ));
    let vertices: Vec<_> = (0..3)
        .map(|i| {
            let mut v = helio::PackedVertex::from_components(
                [i as f32, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [i as f32 * 0.25, 0.75],
                [1.0, 0.0, 0.0],
                -1.0,
            );
            v.tex_coords1 = [0.125, i as f32 * 0.5];
            v
        })
        .collect();
    let entity = world.spawn();
    world.insert(
        entity,
        StaticMeshComponent {
            vertices: vertices.clone(),
            indices: vec![0, 1, 2],
            ..Default::default()
        },
    );
    world.flush_gpu_mirror(context.queue()).unwrap();
    let pool = store
        .interned_var_len_pool::<helio::PackedVertex>(BufferKey::of("builtin_mesh_vertex"))
        .unwrap()
        .underlying()
        .clone();
    assert_eq!(std::mem::size_of::<helio::PackedVertex>(), 40);
    assert_eq!(std::mem::offset_of!(helio::PackedVertex, tex_coords0), 16);
    assert_eq!(std::mem::offset_of!(helio::PackedVertex, tex_coords1), 24);
    assert_eq!(std::mem::offset_of!(helio::PackedVertex, normal), 32);
    let staging = context.device().create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 120,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = context.device().create_command_encoder(&Default::default());
    pool.with_buffer(&mut |b| encoder.copy_buffer_to_buffer(b, 0, &staging, 0, 120));
    context.queue().submit([encoder.finish()]);
    staging
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
    context
        .device()
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let data = staging.slice(..).get_mapped_range().unwrap();
    assert_eq!(&*data, bytemuck::cast_slice::<_, u8>(&vertices));
}

#[test]
fn gpu_expanded_material_pixels() {
    pixel_suite(false);
}
#[test]
fn gpu_bindless_material_pixels() {
    pixel_suite(true);
}

#[test]
fn gpu_authored_vector_constants_and_pbr_ranges() {
    let gpu = Gpu::new(false);
    let mut graph = GraphDescription::new("authored constants");
    let mut output = node("output", "fragment_output");
    for (name, value) in [
        ("base_color", serde_json::json!([0.25, 0.5, 0.75, 0.5])),
        ("normal", serde_json::json!([0.0, 0.0, 2.0])),
        ("emissive_color", serde_json::json!([2.0, 3.0, 4.0, 0.5])),
        ("roughness", serde_json::json!(2.0)),
        ("metallic", serde_json::json!(-1.0)),
        ("ambient_occlusion", serde_json::json!(2.0)),
    ] {
        output.properties.insert(name.into(), value);
    }
    graph.add_node(output);
    let pixels = gpu.render(&graph, 0, false);
    close(pixels[0], [0.25, 0.5, 0.75, 0.5]);
    close(pixels[4], [1.0, 1.0, 0.0, 0.5]);
    close(pixels[8], [0.0, 0.0, 1.0, 0.5]);
    close(pixels[12], [1.0, 1.5, 2.0, 0.5]);
}

#[test]
fn fbx_import_and_native_cache_preserve_uvs_normals_and_sections() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../assets/meshes/primitives/SM_Cube.fbx");
    let asset = helio_component::subsystems::load_mesh_asset_upload(&path).expect("bundled FBX");
    assert!(!asset.sections.is_empty());
    assert!(asset
        .geometry
        .vertices
        .iter()
        .any(|v| v.tex_coords0 != asset.geometry.vertices[0].tex_coords0));
    assert!(asset.geometry.vertices.iter().all(|v| v.normal != 0));
    let bytes = helio_component::mesh_cache::encode_asset(&asset, 123);
    let (decoded, id) = helio_component::mesh_cache::decode_asset(&bytes).unwrap();
    assert_eq!(id, 123);
    assert_eq!(decoded.sections, asset.sections);
    assert_eq!(decoded.geometry.indices, asset.geometry.indices);
    assert_eq!(
        bytemuck::cast_slice::<_, u8>(&decoded.geometry.vertices),
        bytemuck::cast_slice::<_, u8>(&asset.geometry.vertices)
    );
    for section in &decoded.sections {
        assert!((section.material_slot as usize) < decoded.material_slots.len());
        assert!(
            (section.first_index + section.index_count) as usize <= decoded.geometry.indices.len()
        );
    }
}

#[test]
fn texture_residency_is_scene_local_and_failed_decode_preserves_existing_texture() {
    use pulsar_scenedb::gpu::*;
    use std::sync::{Arc, RwLock};
    let gpu = Gpu::new(false);
    let context = EngineGpuContext::new(Arc::new(gpu.device), Arc::new(gpu.queue));
    let make_mirror = || {
        let store = SceneGpuStore::new(
            &context,
            SceneGpuConfig {
                classes: vec![RegionClassConfig {
                    capacity: 8,
                    max_resident_cells: 1,
                }],
                tombstone_headroom: 1,
                max_cells_metadata: 8,
            },
        );
        GpuMirrorHandle::new(Arc::new(store), Arc::clone(context.queue()))
            .with_texture_store(Arc::new(RwLock::new(TextureStore::new(8))))
            .unwrap()
    };
    let first = make_mirror();
    let second = make_mirror();
    let dir = tempfile::tempdir().unwrap();
    let shared = dir.path().join("shared.png");
    let filler = dir.path().join("filler.png");
    image::RgbaImage::from_pixel(2, 2, image::Rgba([100, 120, 140, 255]))
        .save(&shared)
        .unwrap();
    std::fs::copy(&shared, &filler).unwrap();
    let a = material_textures::register_graph_texture(&shared, &first).unwrap();
    assert_eq!(
        material_textures::register_graph_texture(&shared, &first).unwrap(),
        a
    );
    let occupied = material_textures::register_graph_texture(&filler, &second).unwrap();
    let b = material_textures::register_graph_texture(&shared, &second).unwrap();
    assert_ne!(
        b, occupied,
        "another scene's cached slot must not alias an unrelated texture"
    );
    assert!(second
        .texture_store()
        .unwrap()
        .read()
        .unwrap()
        .texture(b)
        .is_some());
    let old = first
        .texture_store()
        .unwrap()
        .read()
        .unwrap()
        .texture(a)
        .unwrap()
        .clone();
    std::fs::write(&shared, b"not a valid image").unwrap();
    assert!(material_textures::register_graph_texture(&shared, &first).is_err());
    assert_eq!(
        first.texture_store().unwrap().read().unwrap().texture(a),
        Some(&old)
    );
}

#[test]
#[ignore = "set PULSAR_MATERIAL_TEST_MESH to inspect a local FBX without checking it into source control"]
fn inspect_local_mesh_uvs_and_sections() {
    let path = std::env::var("PULSAR_MATERIAL_TEST_MESH").expect("PULSAR_MATERIAL_TEST_MESH");
    let asset = helio_component::subsystems::load_mesh_asset_upload(std::path::Path::new(&path))
        .expect("import FBX");
    let vertices = &asset.geometry.vertices;
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    for vertex in vertices {
        for c in 0..2 {
            min[c] = min[c].min(vertex.tex_coords0[c]);
            max[c] = max[c].max(vertex.tex_coords0[c]);
        }
    }
    eprintln!(
        "{path}: vertices={}, UV0={min:?}..{max:?}, sections={:?}",
        vertices.len(),
        asset.sections
    );
    let bytes = std::fs::read(&path).unwrap();
    let has_uv_layer = bytes
        .windows(b"LayerElementUV".len())
        .any(|w| w == b"LayerElementUV");
    eprintln!("Source FBX declares a UV layer: {has_uv_layer}");
    if has_uv_layer {
        assert!(
            max[0] - min[0] > 0.01 && max[1] - min[1] > 0.01,
            "source UV layer collapsed during import"
        );
    } else {
        assert_eq!(min, [0.0; 2]);
        assert_eq!(max, [0.0; 2]);
    }
    assert!(asset
        .geometry
        .indices
        .iter()
        .all(|i| (*i as usize) < vertices.len()));
}
