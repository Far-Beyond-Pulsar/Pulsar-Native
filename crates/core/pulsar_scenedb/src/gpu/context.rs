use std::sync::Arc;

/// Engine-level owner of the wgpu device/queue (C0: the device outlives any
/// renderer). M2a defines the type and constructs it in tests; M4 wires the
/// engine (`engine_backend`) as the single runtime owner, above both SceneDB's
/// GPU layer and any renderer.
pub struct EngineGpuContext {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
}

impl EngineGpuContext {
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) -> Self {
        Self { device, queue }
    }

    pub fn device(&self) -> &Arc<wgpu::Device> {
        &self.device
    }

    pub fn queue(&self) -> &Arc<wgpu::Queue> {
        &self.queue
    }
}
