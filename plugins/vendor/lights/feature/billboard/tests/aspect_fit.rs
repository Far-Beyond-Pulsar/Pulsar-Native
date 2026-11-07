//! The billboard pipeline with a non-square sprite: the vertex stage's
//! texture-dimensions binding (aspect fitting) and the shader validate.
//! Needs a GPU adapter (lavapipe works); skips without one.

#[test]
fn billboard_aspect_fit_shader_and_texture_bindings_validate() {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let (device, queue) =
        pollster::block_on(adapter.request_device(&Default::default())).expect("test device");
    let camera = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Billboard test cameras"),
        size: 368 * 2,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
    let _pass = helio_pass_billboard::BillboardPass::new_with_sprite_rgba(
        &device,
        &queue,
        &camera,
        wgpu::TextureFormat::Rgba16Float,
        &[255; 8 * 4 * 4],
        8,
        4,
    );
}
