//! End-to-end correctness test for the Hi-Z occlusion algorithm itself:
//! runs the REAL `HiZBuildPass` -> `OcclusionCullPass` pipeline (exactly the
//! production pass pair, wired through a real `RenderGraph`) against a
//! synthetic scene with a known-correct answer, instead of only trusting
//! visual inspection of `indoor_cathedral`.
//!
//! Scene: a single, uniform depth value is cleared into the depth buffer,
//! standing in for a flat wall spanning the whole screen at a known distance
//! from the camera (`WALL_DIST`). Three instances, all dead-center on
//! screen (so all three share the wall's footprint):
//!   - `front`  -- well in front of the wall  -> must stay visible.
//!   - `behind` -- well behind the wall        -> must be culled.
//!   - `always_visible` -- behind the wall too, but flagged
//!     `INSTANCE_FLAG_ALWAYS_VISIBLE` -> must stay visible (flag override).
//!
//! A uniform depth value makes every Hi-Z mip level read back the same
//! number regardless of which one `pick_mip` selects, so this test is
//! insensitive to mip-selection details and isolates exactly the thing
//! that actually needs pinning down: the near/far comparison direction in
//! `instance_hiz_occluded`, the `ALWAYS_VISIBLE` bypass, and the real
//! resource wiring between the two passes (bind groups, `hiz_warmed_up`
//! gating, `coordinate_spaces`/`object_batch`/`indirect_dispatch`
//! contracts). Reads the result back from `indirect`'s `instance_count`
//! field per draw slot -- the documented "occluded draws get
//! instance_count=0" signal `OcclusionCullPass` mutates in place.

use std::sync::Arc;

use glam::{Mat4, Vec3};
use helio_core::{
    PassContext, RenderGraph, RenderPass, ResourceKey, ResourceRegistry, Result as HelioResult,
};
use helio_pass_gbuffer::{CoordinateSpacesFrameData, IndirectDispatchArgs, ObjectBatchFrameData};
use helio_pass_hiz::HiZBuildPass;
use helio_pass_indirect_dispatch::IndirectDispatchFrameData;
use helio_pass_object_batch::{
    DrawIndexedIndirectArgs, GpuDrawCall, GpuInstanceData, INSTANCE_FLAG_ALWAYS_VISIBLE,
};
use helio_pass_occlusion_cull::OcclusionCullPass;
use wgpu::util::DeviceExt;

mod support;

const WIDTH: u32 = 64;
const HEIGHT: u32 = 48;
const WALL_DIST: f32 = 5.0;
const FRONT_DIST: f32 = 2.0;
const BEHIND_DIST: f32 = 10.0;
const RADIUS: f32 = 0.3;

/// Publishes a hand-built `object_batch`/`indirect_dispatch`/
/// `coordinate_spaces`/`depth_texture` every frame, standing in for
/// `ObjectBatchPass`/`IndirectDispatchPass`/`GBufferPass`/`Renderer` so this
/// test can drive `HiZBuildPass`/`OcclusionCullPass` without SceneDB.
struct SceneInjectorPass {
    instances: Arc<wgpu::Buffer>,
    draw_calls: Arc<wgpu::Buffer>,
    indirect: Arc<wgpu::Buffer>,
    compacted_indices: Arc<wgpu::Buffer>,
    coordinate_spaces: Arc<wgpu::Buffer>,
    depth_texture: Arc<wgpu::Texture>,
    /// `(draw_count, 1, 1)`: what `ObjectBatchPass` writes on the GPU.
    group_dispatch: Arc<wgpu::Buffer>,
    draw_count: u32,
    instance_count: u32,
}

impl RenderPass for SceneInjectorPass {
    fn name(&self) -> &'static str {
        "SceneInjector"
    }

    fn reads(&self) -> &'static [&'static str] {
        &[]
    }

    fn writes(&self) -> &'static [&'static str] {
        // "depth" isn't an actual registry key this pass writes (the real
        // "depth_texture" typed resource is published in `publish` below,
        // by name, and read directly via `ResourceRegistry::get` -- there is
        // no scheduler-visible name for it). `HiZBuildPass` declares `reads:
        // ["depth"]`, so declaring the same name here is what makes the
        // scheduler put this pass in an earlier layer than `HiZBuildPass`
        // instead of racing them in the same parallel layer (no other
        // declared name links them, since "depth_texture" isn't one).
        &[
            "object_batch",
            "indirect_dispatch",
            "coordinate_spaces",
            "depth",
        ]
    }

    fn render_pass_descriptor<'a>(
        &'a self,
        _target: &'a wgpu::TextureView,
        _depth: &'a wgpu::TextureView,
        _resources: &'a ResourceRegistry<'a>,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        None
    }

    fn execute(&mut self, _ctx: &mut PassContext) -> HelioResult<()> {
        Ok(())
    }

    fn publish<'a>(&self, frame: &mut ResourceRegistry<'a>) {
        // SAFETY: every field here is owned (via `Arc`) by `self`, which the
        // graph keeps alive for the pass's entire lifetime (many frames) --
        // `'a` is always shorter than that. Same reasoning/pattern as
        // `HiZBuildPass::publish`'s own transmute for its `Arc` fields.
        let instances: &'a wgpu::Buffer = unsafe { std::mem::transmute(&*self.instances) };
        let draw_calls: &'a wgpu::Buffer = unsafe { std::mem::transmute(&*self.draw_calls) };
        let indirect: &'a wgpu::Buffer = unsafe { std::mem::transmute(&*self.indirect) };
        let compacted_indices: &'a wgpu::Buffer =
            unsafe { std::mem::transmute(&*self.compacted_indices) };
        let coordinate_spaces: &'a wgpu::Buffer =
            unsafe { std::mem::transmute(&*self.coordinate_spaces) };
        let depth_texture: &'a wgpu::Texture = unsafe { std::mem::transmute(&*self.depth_texture) };
        let group_dispatch: &'a wgpu::Buffer =
            unsafe { std::mem::transmute(&*self.group_dispatch) };

        frame.write(
            ResourceKey::new("object_batch"),
            ObjectBatchFrameData {
                instances,
                aabbs: instances, // unused by OcclusionCullPass; any valid buffer satisfies the type
                draw_calls,
                indirect,
                group_capacity: self.draw_count,
                group_dispatch: IndirectDispatchArgs {
                    buffer: group_dispatch,
                    offset: 0,
                },
                readback_draw_count: self.draw_count,
                readback_instance_count: self.instance_count,
                opaque_ranges: &[],
                transparent_ranges: &[],
                forward_ranges: &[],
                opaque_ranges_gpu: indirect,
                transparent_ranges_gpu: indirect,
                forward_ranges_gpu: indirect,
                range_counts_gpu: indirect,
                draw_counts_gpu: indirect,
                range_slot_capacity: 0,
                shadow_static_indirect: indirect,
                readback_shadow_static_draw_count: 0,
                shadow_movable_indirect: indirect,
                readback_shadow_movable_draw_count: 0,
                shadow_transmissive_indirect: indirect,
                readback_shadow_transmissive_draw_count: 0,
                shadow_static_generation: 0,
                draw_counts: None,
            },
            "SceneInjector",
        );
        frame.write(
            ResourceKey::new("indirect_dispatch"),
            IndirectDispatchFrameData {
                indirect,
                compacted_indices,
            },
            "SceneInjector",
        );
        frame.write(
            helio_core::resource_keys::coordinate_spaces(),
            CoordinateSpacesFrameData {
                coordinate_spaces,
                coordinate_spaces_prev: coordinate_spaces,
            },
            "SceneInjector",
        );
        frame.write(
            ResourceKey::new("depth_texture"),
            depth_texture,
            "SceneInjector",
        );
    }
}

/// NDC depth (WGPU's [0,1] range) of a point `dist` units in front of a
/// camera at the world origin looking down -Z, under `view_proj`.
fn ndc_depth_at_distance(view_proj: Mat4, dist: f32) -> f32 {
    let clip = view_proj * Vec3::new(0.0, 0.0, -dist).extend(1.0);
    clip.z / clip.w
}

fn make_instance(center_z: f32, flags: u32) -> GpuInstanceData {
    let model = Mat4::from_translation(Vec3::new(0.0, 0.0, center_z));
    GpuInstanceData {
        model: model.to_cols_array(),
        normal_mat: [
            1.0, 0.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0,
        ],
        bounds: [0.0, 0.0, center_z, RADIUS],
        prev_model: model.to_cols_array(),
        mesh_id: 0,
        material_id: 0,
        flags,
        lightmap_index: 0xFFFF_FFFF,
        skin_vertex_offset: 0,
        bone_palette_offset: 0,
        bone_palette_count: 0,
        _skin_pad: 0,
    }
}

fn draw_call(slot: u32) -> GpuDrawCall {
    GpuDrawCall {
        index_count: 3,
        first_index: 0,
        vertex_offset: 0,
        first_instance: slot,
        instance_count: 1,
    }
}

fn indirect_args(slot: u32) -> DrawIndexedIndirectArgs {
    DrawIndexedIndirectArgs {
        index_count: 3,
        instance_count: 1, // frustum-stage survivor count, as IndirectDispatchPass would leave it
        first_index: 0,
        base_vertex: 0,
        first_instance: slot,
    }
}

/// Blocking readback of a whole buffer's contents as `u32`s.
fn read_buffer_u32(device: &wgpu::Device, queue: &wgpu::Queue, buffer: &wgpu::Buffer) -> Vec<u32> {
    let size = buffer.size();
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Test Readback"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("Test Readback Encoder"),
    });
    encoder.copy_buffer_to_buffer(buffer, 0, &readback, 0, size);
    queue.submit(Some(encoder.finish()));

    let slice = readback.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        sender.send(result).expect("send map result");
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll readback");
    receiver
        .recv()
        .expect("receive map result")
        .expect("readback buffer must map");
    let mapped = slice
        .get_mapped_range()
        .expect("read mapped readback bytes");
    let data: Vec<u32> = bytemuck::cast_slice(&mapped).to_vec();
    drop(mapped);
    readback.unmap();
    data
}

#[test]
fn front_object_survives_behind_object_culled_always_visible_overrides() {
    pollster::block_on(async {
        let Some((device, queue)) = support::request_test_device("OcclusionCull Correctness").await
        else {
            eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: occlusion-cull correctness");
            return;
        };
        let device = Arc::new(device);
        let queue = Arc::new(queue);

        let view = Mat4::look_at_rh(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y);
        let proj =
            helio_core::depth::perspective(60f32.to_radians(), WIDTH as f32 / HEIGHT as f32, 0.1);
        let view_proj = proj * view;
        let camera = helio_core::GpuCameraUniforms::new(
            view,
            proj,
            Vec3::ZERO,
            0.1,
            1000.0,
            0,
            [0.0, 0.0],
            view_proj,
        );

        let mut scene_input = support::TestSceneInput::new(Arc::clone(&device), Arc::clone(&queue));
        scene_input.set_camera(camera);

        // Three draw-call groups, one instance each, all dead-center on
        // screen: `front` (closer than the wall), `behind` (farther than
        // the wall), `always_visible` (farther too, but flag-exempt).
        let mut off_axis = make_instance(-5.21, 0);
        off_axis.bounds = [3.9, 0.0, -5.21, 0.25];
        let instances = [
            make_instance(-FRONT_DIST, 0),
            make_instance(-BEHIND_DIST, 0),
            make_instance(-BEHIND_DIST, INSTANCE_FLAG_ALWAYS_VISIBLE),
            off_axis,
        ];
        let draw_calls = [draw_call(0), draw_call(1), draw_call(2), draw_call(3)];
        let indirect = [
            indirect_args(0),
            indirect_args(1),
            indirect_args(2),
            indirect_args(3),
        ];
        let compacted_indices: [u32; 4] = [0, 1, 2, 3];
        let identity = Mat4::IDENTITY.to_cols_array();

        let instances_buf = Arc::new(device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("Test Instances"),
                contents: bytemuck::cast_slice(&instances),
                usage: wgpu::BufferUsages::STORAGE,
            },
        ));
        let draw_calls_buf = Arc::new(device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("Test Draw Calls"),
                contents: bytemuck::cast_slice(&draw_calls),
                usage: wgpu::BufferUsages::STORAGE,
            },
        ));
        let indirect_buf = Arc::new(
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Test Indirect"),
                contents: bytemuck::cast_slice(&indirect),
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
            }),
        );
        let compacted_indices_buf = Arc::new(device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("Test Compacted Indices"),
                contents: bytemuck::cast_slice(&compacted_indices),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            },
        ));
        let coordinate_spaces_buf = Arc::new(device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("Test Coordinate Spaces"),
                contents: bytemuck::bytes_of(&identity),
                usage: wgpu::BufferUsages::STORAGE,
            },
        ));
        let depth_texture = Arc::new(device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Test Depth"),
            size: wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        }));
        let depth_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let wall_depth = ndc_depth_at_distance(view_proj, WALL_DIST);
        assert!(
            (0.0..=1.0).contains(&wall_depth),
            "test setup bug: wall NDC depth {wall_depth} must be inside [0,1]"
        );
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Seed Wall Depth"),
        });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Seed Wall Depth Clear"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wall_depth),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        queue.submit(Some(encoder.finish()));

        let injector = SceneInjectorPass {
            instances: instances_buf,
            draw_calls: draw_calls_buf,
            indirect: Arc::clone(&indirect_buf),
            compacted_indices: compacted_indices_buf,
            coordinate_spaces: coordinate_spaces_buf,
            depth_texture,
            group_dispatch: Arc::new(device.create_buffer_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some("Test Group Dispatch"),
                    contents: bytemuck::cast_slice(&[4u32, 1, 1]),
                    usage: wgpu::BufferUsages::INDIRECT,
                },
            )),
            draw_count: 4,
            instance_count: 4,
        };

        let hiz_pass = HiZBuildPass::new(&device, &queue, WIDTH, HEIGHT);
        let hiz_sampler = Arc::clone(&hiz_pass.hiz_sampler);
        let cull_stats_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Test Cull Stats"),
            size: 32,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let occlusion_pass =
            OcclusionCullPass::new(&device, hiz_sampler, WIDTH, HEIGHT, cull_stats_buf);

        let mut graph = RenderGraph::new(&device, &queue);
        graph.add_pass(Box::new(injector));
        graph.add_pass(Box::new(hiz_pass));
        graph.add_pass(Box::new(occlusion_pass));
        graph.lock(WIDTH, HEIGHT);

        let (target, depth) = support::frame_views(&device, WIDTH, HEIGHT);

        // Frame 1: `hiz_warmed_up` bypass -- passes `compacted_indices`
        // through unchanged, doesn't test anything yet.
        graph
            .execute(&scene_input, &target, &depth)
            .expect("warm-up frame must execute");
        // Frame 2: real Hi-Z test, against the pyramid built from the wall
        // depth published above (constant across frames, so no genuine
        // temporal lag in this synthetic setup).
        graph
            .execute(&scene_input, &target, &depth)
            .expect("real occlusion-test frame must execute");

        let words = read_buffer_u32(&device, &queue, &indirect_buf);
        // DrawIndexedIndirectArgs is 5 u32s; instance_count is word 1 of each.
        let instance_count = |slot: usize| words[slot * 5 + 1];

        assert_eq!(
            instance_count(0),
            1,
            "front object (in front of the wall) must stay visible"
        );
        assert_eq!(
            instance_count(1),
            0,
            "behind object (behind the wall) must be culled"
        );
        assert_eq!(instance_count(3), 1, "off-axis sphere intersects the front of the wall: camera-ray near point must not falsely cull it");
        assert_eq!(
            instance_count(2),
            1,
            "ALWAYS_VISIBLE object behind the wall must stay visible (flag override)"
        );
        // A camera-generation change invalidates old depth. Reset the input
        // survivors just as the real frustum pass does each frame.
        scene_input.set_camera(camera);
        queue.write_buffer(&indirect_buf, 0, bytemuck::cast_slice(&indirect));
        graph.execute(&scene_input, &target, &depth).unwrap();
        let words = read_buffer_u32(&device, &queue, &indirect_buf);
        assert_eq!(
            words[6], 1,
            "changed camera must not reject against stale history"
        );
        for _ in 0..8 {
            queue.write_buffer(&indirect_buf, 0, bytemuck::cast_slice(&indirect));
            graph.execute(&scene_input, &target, &depth).unwrap();
            let words = read_buffer_u32(&device, &queue, &indirect_buf);
            assert_eq!(words[6], 0, "stable camera resumes occlusion");
            assert_eq!(
                words[16], 1,
                "off-axis visible sphere must not alternate visibility"
            );
        }
    });
}
