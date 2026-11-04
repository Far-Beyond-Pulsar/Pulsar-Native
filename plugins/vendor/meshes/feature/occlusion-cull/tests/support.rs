//! Shared headless-GPU test scaffolding for this crate's integration tests.
//! Mirrors `helio-core/tests/support.rs`'s `SceneInputAdapter` pattern, with
//! a settable camera (the resize contract test doesn't need one, but the
//! occlusion-correctness test does).

use std::sync::Arc;

use bytemuck::Zeroable;
use helio_core::{GpuCameraUniforms, SceneInput};

/// `SceneInput` carries exactly one `GpuCameraUniforms`-sized camera row in
/// production (`Renderer::camera_buffer`), but every WGSL shader in this
/// codebase declares `cameras: array<Camera, 2>` (XR stereo support) and
/// binds it with `min_binding_size: None` -- so the buffer itself must be
/// sized for two rows or a strict backend can reject the bind group even
/// though only `cameras[0]` is ever read here.
pub struct TestSceneInput {
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    pub frame_count: u64,
    camera_buf: wgpu::Buffer,
    camera_data: GpuCameraUniforms,
    camera_generation: u64,
}

impl TestSceneInput {
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) -> Self {
        let camera_data = GpuCameraUniforms::zeroed();
        let camera_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Test Camera Buffer (2 rows for array<Camera, 2>)"),
            size: 2 * std::mem::size_of::<GpuCameraUniforms>() as u64,
            // STORAGE (not just UNIFORM): `OcclusionCullPass`'s bind group
            // layout declares the camera binding as
            // `BufferBindingType::Storage { read_only: true }`, matching
            // `occlusion_cull.wgsl`'s `var<storage, read> cameras`.
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::UNIFORM
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            device,
            queue,
            frame_count: 0,
            camera_buf,
            camera_data,
            camera_generation: 0,
        }
    }

    /// Writes `data` into `cameras[0]`'s slot and remembers it as this
    /// frame's CPU-side mirror.
    pub fn set_camera(&mut self, data: GpuCameraUniforms) {
        self.camera_generation += 1;
        self.camera_data = data;
        self.queue
            .write_buffer(&self.camera_buf, 0, bytemuck::bytes_of(&data));
    }
}

impl SceneInput for TestSceneInput {
    fn world_origin(&self) -> glam::DVec3 {
        glam::DVec3::ZERO
    }
    fn device(&self) -> &Arc<wgpu::Device> {
        &self.device
    }
    fn queue(&self) -> &Arc<wgpu::Queue> {
        &self.queue
    }
    fn frame_count(&self) -> u64 {
        self.frame_count
    }
    fn camera(&self) -> &wgpu::Buffer {
        &self.camera_buf
    }
    fn camera_data(&self) -> &GpuCameraUniforms {
        &self.camera_data
    }
    fn camera_generation(&self) -> u64 {
        self.camera_generation
    }
    fn scene_buffers(&self) -> &helio_core::SceneBufferProjection {
        static EMPTY: std::sync::OnceLock<helio_core::SceneBufferProjection> =
            std::sync::OnceLock::new();
        EMPTY.get_or_init(helio_core::SceneBufferProjection::empty)
    }
}

/// Requests a GPU adapter/device, trying a fallback adapter if a hardware
/// one isn't available (e.g. headless CI). Returns `None` (never panics) so
/// callers can skip gracefully -- matches
/// `helio-core/tests/render_graph_resize_contract.rs`'s `request_test_adapter`.
pub async fn request_test_device(label: &str) -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let mut adapter = None;
    for force_fallback_adapter in [false, true] {
        if let Ok(found) = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter,
                apply_limit_buckets: false,
            })
            .await
        {
            adapter = Some(found);
            break;
        }
    }
    let adapter = adapter?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some(label),
            required_features: wgpu::Features::INDIRECT_FIRST_INSTANCE,
            required_limits: adapter.limits(),
            ..Default::default()
        })
        .await
        .unwrap_or_else(|error| panic!("{label}: adapter must create a device: {error}"));
    let owned_label = label.to_string();
    device.on_uncaptured_error(Arc::new(move |error| {
        panic!("{owned_label} validation error: {error:?}");
    }));
    Some((device, queue))
}

pub fn frame_views(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::TextureView, wgpu::TextureView) {
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Test Target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Test Depth"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    (
        target.create_view(&wgpu::TextureViewDescriptor::default()),
        depth.create_view(&wgpu::TextureViewDescriptor::default()),
    )
}
