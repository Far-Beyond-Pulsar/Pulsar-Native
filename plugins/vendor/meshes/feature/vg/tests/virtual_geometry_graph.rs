//! The virtual-geometry chain inside the default deferred graph: the pass set
//! is added in order when asked for, builds against a real device (pipelines,
//! storage-capable G-buffer targets, `vg_hiz_late`), and an idle chain
//! (nothing registered, so nothing VG-active) records nothing and produces no
//! validation errors. Also checks the classic graph is untouched without it.
use std::sync::Arc;

use helio::{
    required_experimental_features, required_wgpu_features, required_wgpu_limits, Camera,
    RendererBuilder, RendererConfig,
};
use helio_pass_vg_cull::VgCullEarlyPass;
use helio_pass_vg_shade::VgShadePass;
use helio_vg_stream::{VgRuntime, VgRuntimeSettings};
use pulsar_scenedb::gpu::{EngineGpuContext, GpuMirrorHandle, SceneGpuConfig, SceneGpuStore};

/// `VG_DEBUG_VIEW` is process-wide: tests that build a VG config hold this.
static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn device() -> Option<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
        let adapter = instance.request_adapter(&Default::default()).await.ok()?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features: required_wgpu_features(adapter.features()),
                required_limits: required_wgpu_limits(adapter.limits()),
                experimental_features: required_experimental_features(adapter.features()),
                ..Default::default()
            })
            .await
            .ok()?;
        Some((Arc::new(device), Arc::new(queue)))
    })
}

fn mirror(device: &Arc<wgpu::Device>, queue: &Arc<wgpu::Queue>) -> GpuMirrorHandle {
    let gpu_context = EngineGpuContext::new(Arc::clone(device), Arc::clone(queue));
    let mut gpu_store = SceneGpuStore::new(
        &gpu_context,
        SceneGpuConfig {
            classes: Vec::new(),
            tombstone_headroom: 0,
            max_cells_metadata: 0,
        },
    );
    helio_mesh_host::MeshComponent::register_gpu_columns_growable(&mut gpu_store, 16, device);
    helio_components::MaterialComponent::register_gpu_columns_growable(&mut gpu_store, 16, device);
    helio_mesh_host::StaticObjectComponent::register_gpu_columns_growable(
        &mut gpu_store,
        16,
        device,
    );
    helio_components::LightComponent::register_gpu_columns_growable(&mut gpu_store, 16, device);
    GpuMirrorHandle::new(Arc::new(gpu_store), Arc::clone(queue))
}

fn build(
    device: &Arc<wgpu::Device>,
    queue: &Arc<wgpu::Queue>,
    runtime: Option<&VgRuntime>,
) -> helio::Renderer {
    let mut config = RendererConfig::new(320, 180, wgpu::TextureFormat::Rgba8Unorm);
    if let Some(runtime) = runtime {
        config = helio_pass_vg::with_virtual_geometry(config, runtime);
    }
    RendererBuilder::new(config.clone(), mirror(device, queue))
        .with_feature(helio::AmbientLight {
            color: [0.5, 0.5, 0.6],
            intensity: 1.0,
        })
        .with_external_device()
        .with_pass_build_context(Box::new(helio_mesh_host::presets::deferred))
        .build(
            Arc::clone(device),
            Arc::clone(queue),
            320,
            180,
            config.surface_format,
        )
}

fn target(device: &wgpu::Device) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("vg graph smoke target"),
            size: wgpu::Extent3d {
                width: 320,
                height: 180,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

#[test]
fn the_chain_is_absent_by_default_and_idle_without_assets() {
    let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let Some((device, queue)) = device() else {
        eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: virtual geometry graph");
        return;
    };
    let camera = Camera::perspective_look_at(
        glam::Vec3::new(0.0, 1.0, 4.0),
        -glam::Vec3::Z,
        glam::Vec3::Y,
        std::f32::consts::FRAC_PI_4,
        320.0 / 180.0,
        0.1,
        100.0,
    );
    let view = target(&device);

    // Classic graph: no VG pass at all.
    let mut classic = build(&device, &queue, None);
    assert!(classic.find_pass::<VgCullEarlyPass>().is_none());
    assert!(classic.find_pass::<VgShadePass>().is_none());

    // Requested: present when the device supports it, absent otherwise.
    let runtime = VgRuntime::new(VgRuntimeSettings::default());
    let mut renderer = build(&device, &queue, Some(&runtime));
    let supported = helio_pass_vg::virtual_geometry_supported(&device);
    assert_eq!(renderer.find_pass::<VgCullEarlyPass>().is_some(), supported);
    assert_eq!(renderer.find_pass::<VgShadePass>().is_some(), supported);

    for mut renderer in [classic, renderer] {
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        for _ in 0..4 {
            renderer.render(&camera, &view).unwrap();
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        }
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "idle chain GPU validation: {error:?}");
    }
    // Nothing was registered, so the store was created lazily at most and no
    // asset is ready.
    assert!(!runtime.any_ready());
}

#[test]
fn vg_debug_view_env_var_selects_the_initial_debug_mode() {
    let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    use helio_vg_format::debug_view::{VgDebugView, ENV_VAR};
    // Env vars are process-wide: this is the only test of this binary that
    // touches it, and it restores the previous value.
    let previous = std::env::var_os(ENV_VAR);
    let runtime = VgRuntime::new(VgRuntimeSettings::default());
    let config = || RendererConfig::new(64, 64, wgpu::TextureFormat::Rgba8Unorm);

    std::env::set_var(ENV_VAR, "lod");
    assert_eq!(
        helio_pass_vg::with_virtual_geometry(config(), &runtime).debug_mode,
        VgDebugView::LodLevel.id()
    );
    std::env::set_var(ENV_VAR, "21");
    assert_eq!(
        helio_pass_vg::with_virtual_geometry(config(), &runtime).debug_mode,
        VgDebugView::ClusterId.id()
    );
    std::env::set_var(ENV_VAR, "not a view");
    assert_eq!(
        helio_pass_vg::with_virtual_geometry(config(), &runtime).debug_mode,
        0
    );
    // An explicit mode wins over the environment.
    std::env::set_var(ENV_VAR, "lod");
    let mut explicit = config();
    explicit.debug_mode = 5;
    assert_eq!(
        helio_pass_vg::with_virtual_geometry(explicit, &runtime).debug_mode,
        5
    );
    std::env::remove_var(ENV_VAR);
    assert_eq!(
        helio_pass_vg::with_virtual_geometry(config(), &runtime).debug_mode,
        0
    );

    if let Some(previous) = previous {
        std::env::set_var(ENV_VAR, previous);
    }
}

#[test]
fn the_chain_registers_every_vg_debug_view_with_its_category() {
    let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let Some((device, queue)) = device() else {
        eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: virtual geometry debug views");
        return;
    };
    if !helio_pass_vg::virtual_geometry_supported(&device) {
        return;
    }
    let runtime = VgRuntime::new(VgRuntimeSettings::default());
    let renderer = build(&device, &queue, Some(&runtime));
    let views = renderer.available_debug_views();
    let categories = renderer.available_debug_view_categories();
    for view in helio_vg_format::debug_view::ALL {
        let found = views.iter().find(|v| v.debug_mode == view.id());
        let found = found.unwrap_or_else(|| panic!("{view:?} is not registered"));
        assert_eq!(found.name, view.name());
        assert!(
            categories
                .iter()
                .any(|(mode, category)| *mode == view.id() && *category == "Virtual Geometry"),
            "{view:?} has no Virtual Geometry category"
        );
    }
    // No other pass reuses one of our ids.
    for view in helio_vg_format::debug_view::ALL {
        let count = views.iter().filter(|v| v.debug_mode == view.id()).count();
        assert_eq!(count, 1, "{view:?} is registered {count} times");
    }
}
