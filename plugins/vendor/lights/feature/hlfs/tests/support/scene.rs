//! Focused GPU input fixture, implementing the same generic SceneInput boundary.
use bytemuck::Pod;
use helio_core::{
    BlasManager, BufferHandle, BufferKey, GpuCameraUniforms, SceneBufferProjection, SceneInput,
    TlasManager,
};
use std::sync::Arc;

pub struct Values<T: Pod> {
    pub buffer: wgpu::Buffer,
    values: Vec<T>,
    minimum_rows: usize,
}
impl<T: Pod> Values<T> {
    fn new(device: &wgpu::Device) -> Self {
        Self {
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("HLFS fixture input"),
                size: std::mem::size_of::<T>() as u64,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::UNIFORM
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            values: vec![T::zeroed()],
            minimum_rows: 1,
        }
    }
    pub fn set_data(&mut self, values: Vec<T>) {
        self.values = values;
    }
    pub fn update(&mut self, value: T) {
        self.values = vec![value];
    }
    pub fn data(&self) -> &T {
        &self.values[0]
    }
    fn flush(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let size = (self.values.len().max(self.minimum_rows) * std::mem::size_of::<T>()) as u64;
        if self.buffer.size() != size {
            self.buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("HLFS fixture input"),
                size,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::UNIFORM
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if self.values.is_empty() {
            queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&T::zeroed()));
        } else {
            queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&self.values));
        }
    }
}

pub struct TestScene {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    pub camera: Values<GpuCameraUniforms>,
    pub lights: Values<helio_light_api::GpuLight>,
    pub shadow_matrices: Values<helio_light_api::GpuShadowMatrix>,
    pub blas_manager: BlasManager,
    pub tlas_manager: TlasManager,
    pub frame_count: u64,
    pub width: u32,
    pub height: u32,
    pub movable_light_count: u32,
    pub movable_lights_generation: u64,
    projection: SceneBufferProjection,
}
impl TestScene {
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) -> Self {
        let mut camera = Values::new(&device);
        camera.minimum_rows = 2;
        Self {
            camera,
            lights: Values::new(&device),
            shadow_matrices: Values::new(&device),
            blas_manager: BlasManager::new(device.clone()),
            tlas_manager: TlasManager::new(device.clone(), 1),
            device,
            queue,
            frame_count: 0,
            width: 0,
            height: 0,
            movable_light_count: 0,
            movable_lights_generation: 0,
            projection: SceneBufferProjection::empty(),
        }
    }
    pub fn flush(&mut self) {
        self.camera.flush(&self.device, &self.queue);
        self.lights.flush(&self.device, &self.queue);
        self.shadow_matrices.flush(&self.device, &self.queue);
        self.projection = if self.movable_light_count == 0 {
            SceneBufferProjection::empty()
        } else {
            SceneBufferProjection::from_handles([(
                BufferKey::of(helio_light_api::LIGHT_LIST),
                BufferHandle {
                    buffer: self.lights.buffer.clone(),
                    epoch: self.movable_lights_generation,
                    row_bytes: std::mem::size_of::<helio_light_api::GpuLight>() as u64,
                    content_generation: self.movable_lights_generation,
                },
            )])
        };
    }
}
impl SceneInput for TestScene {
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
        &self.camera.buffer
    }
    fn camera_data(&self) -> &GpuCameraUniforms {
        self.camera.data()
    }
    fn camera_generation(&self) -> u64 {
        0
    }
    fn scene_buffers(&self) -> &SceneBufferProjection {
        &self.projection
    }
}
