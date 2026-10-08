//! The toolbar's Bloom toggle reaches the renderer: after each rendered frame
//! the post-process resolver's baseline (its defaults, a renderer setting)
//! carries the toggle's state; nothing is written into the scene
//! (Pulsar-Native#1035, Phase 4).

use engine_backend::subsystems::render::HelioRenderer;

// Large enough for the lens-flare mip chain (it rejects very small targets).
const SIZE: u32 = 320;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    // Without ray queries: Mesa's lavapipe, which headless machines use,
    // loses the device compiling the radiance-cascades ray-query pipeline.
    // Bloom does not depend on them.
    let features = adapter.features() - wgpu::Features::EXPERIMENTAL_RAY_QUERY;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: helio::required_wgpu_features(features),
        required_limits: helio::required_wgpu_limits(adapter.limits()),
        experimental_features: helio::required_experimental_features(features),
        ..Default::default()
    }))
    .ok()
}

fn render(renderer: &mut HelioRenderer, device: &wgpu::Device, queue: &wgpu::Queue) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    renderer.render_frame(device, queue, &view, SIZE, SIZE, FORMAT);
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
}

/// The bloom flag of the resolver's baseline.
fn baseline_bloom(renderer: &mut HelioRenderer) -> Option<bool> {
    renderer
        .postprocess_defaults()
        .map(|settings| settings.bloom_enabled)
}

#[test]
fn toolbar_bloom_toggle_updates_the_resolver_baseline() {
    let Some((device, queue)) = device() else {
        eprintln!("skipping: no GPU adapter available");
        return;
    };
    let scene: engine_backend::scene::SharedScene =
        std::sync::Arc::new(parking_lot::RwLock::new(engine_backend::scene::new_scene()));
    let mut renderer = HelioRenderer::new(scene.clone());
    let mailbox = renderer.editor_mailbox();

    // No project settings registered: the schema defaults (bloom on).
    render(&mut renderer, &device, &queue);
    render(&mut renderer, &device, &queue);
    assert_eq!(
        baseline_bloom(&mut renderer),
        Some(true),
        "the toolbar defaults to bloom on"
    );

    mailbox.set_viewport_bloom(false);
    render(&mut renderer, &device, &queue);
    assert_eq!(
        baseline_bloom(&mut renderer),
        Some(false),
        "toggle off reaches the resolver next frame"
    );

    mailbox.set_viewport_bloom(true);
    render(&mut renderer, &device, &queue);
    assert_eq!(baseline_bloom(&mut renderer), Some(true), "toggle back on");

    assert_eq!(
        scene
            .read()
            .world
            .query::<&helio_component::components::CameraPostProcessComponent>()
            .count(),
        0,
        "the editor writes no camera row into the scene"
    );
}
