use std::sync::Arc;

use helio_pass_occlusion_cull::OcclusionCullPass;

async fn create_pass() -> Option<OcclusionCullPass> {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let mut adapter = None;
    for force_fallback_adapter in [false, true] {
        if let Ok(candidate) = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter,
                apply_limit_buckets: false,
            })
            .await
        {
            adapter = Some(candidate);
            break;
        }
    }
    let adapter = adapter?;
    let (device, _queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("Occlusion Cull Portability Test Device"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            ..Default::default()
        })
        .await
        .expect("adapter must create a device");
    device.on_uncaptured_error(Arc::new(|error| {
        panic!("occlusion-cull GPU validation error: {error:?}");
    }));

    let hiz_sampler = Arc::new(device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("Occlusion Cull Portability Test Hi-Z Sampler"),
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    }));
    let cull_stats = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Occlusion Cull Portability Test Stats"),
        size: 5 * std::mem::size_of::<u32>() as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });

    Some(OcclusionCullPass::new(
        &device,
        hiz_sampler,
        1280,
        720,
        cull_stats,
    ))
}

#[test]
fn pipeline_compiles_on_the_available_backend() {
    let Some(_pass) = pollster::block_on(create_pass()) else {
        eprintln!("skipping occlusion-cull portability test: no GPU adapter available");
        return;
    };
}
