//! Runs the sky texture loader and the composite pipeline on a real device.
//! Skipped (with a message) when no GPU adapter is available.
use helio_pass_sky::sky_texture::{self, source, SkyTextureCache};
use helio_pass_sky::AtmosphereCompositePass;

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor().with_env());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .ok()?;
        adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .ok()
    })
}

#[test]
fn the_composite_pipeline_builds_on_a_device() {
    let Some((device, _queue)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let _pass = AtmosphereCompositePass::new(&device, wgpu::TextureFormat::Rgba16Float);
}

#[test]
fn a_preset_becomes_resident_and_turning_it_off_frees_it() {
    if !sky_texture::presets_available() {
        eprintln!("presets not embedded; skipping");
        return;
    }
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut cache = SkyTextureCache::new(&device);
    let code = source::GALAXY_NEBULA;
    let start = std::time::Instant::now();
    while !cache.is_resident(code) {
        let _ = cache.view(&device, &queue, code);
        assert!(
            start.elapsed() < std::time::Duration::from_secs(120),
            "the preset never became resident"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    eprintln!("resident after {:?}", start.elapsed());
    let _ = cache.view(&device, &queue, source::NONE);
    assert!(
        !cache.is_resident(code),
        "turning the layer off frees the texture"
    );
}
