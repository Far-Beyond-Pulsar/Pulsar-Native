//! The engine's GPU device: the headless fallback and the one factory every
//! host uses so the device carries what Helio needs.
pub(super) fn headless_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    request_engine_device(
        &adapter,
        &wgpu::DeviceDescriptor {
            label: Some("Pulsar engine device"),
            ..Default::default()
        },
    )
    .map_err(|error| tracing::warn!("No GPU device for the engine: {error}"))
    .ok()
}

/// Requests the device the engine renders with from `adapter`: what `host`
/// asks for (a window's own needs, or nothing for a headless process), raised
/// to what Helio needs. Every device the engine draws on comes from here: a
/// windowed host (the editor's window, which [`RenderRuntimes::offer_device`]
/// shares with the engine) passes this as its device factory, so the shared
/// device carries Helio's features and the adapter's full binding-array
/// limits, which size the material texture table
/// (`helio_core::texture_table_capacity`).
///
/// Hardware ray queries are the engine's decision, the same for every host:
/// on where the adapter runs them (`helio::usable_adapter_features`: not a
/// software adapter, whatever it advertises) unless the user turned
/// `editor.renderer.hardware_ray_queries` off. What `host` asked for them is
/// not read.
pub fn request_engine_device(
    adapter: &wgpu::Adapter,
    host: &wgpu::DeviceDescriptor<'_>,
) -> Result<(wgpu::Device, wgpu::Queue), wgpu::RequestDeviceError> {
    // SceneDB names itself the owner of what it allocates on any engine device
    // (`services::vram`; wgpu's allocation owners).
    pulsar_scenedb::gpu::set_owner_hook(|| {
        Box::new(wgpu::wgt::allocation_owner::root_scope("SceneDB"))
    });
    let ray_query = wgpu::Features::EXPERIMENTAL_RAY_QUERY;
    let adapter_features = helio::usable_adapter_features(adapter);
    let info = adapter.get_info();
    tracing::info!(
        "Engine device: {} through {:?} (driver {} {}), pipeline cache {}",
        info.name,
        info.backend,
        info.driver,
        info.driver_info,
        if adapter_features.contains(wgpu::Features::PIPELINE_CACHE) {
            "available"
        } else {
            "not available on this backend"
        }
    );
    let wanted = engine_state::settings::hardware_ray_queries();
    let helio_features = if wanted {
        helio::required_wgpu_features(adapter_features)
    } else {
        helio::required_wgpu_features(adapter_features - ray_query)
    };
    // Cooked textures are BC7 where the device samples it
    // (`helio_component::cooked_texture::CookFormat::best_for`); without the
    // feature every texture is cooked and held as RGBA8, four times the video
    // memory and the bandwidth. Every engine device asks for it, whatever host
    // made it.
    let texture_compression = adapter_features & wgpu::Features::TEXTURE_COMPRESSION_BC;
    let features = (host.required_features - ray_query) | helio_features | texture_compression;
    // Each limit the better of the host's and Helio's (the texture table holds
    // as many textures as the binding arrays allow), except the buffer size:
    // wgpu-core asserts `max_buffer_size <= u32::MAX` at device creation, so
    // it is Helio's clamp of the adapter's (`helio::required_wgpu_limits`).
    let helio_limits = helio::required_wgpu_limits(adapter.limits());
    let required_limits = wgpu::Limits {
        max_buffer_size: helio_limits.max_buffer_size,
        ..host
            .required_limits
            .clone()
            .or_better_values_from(&helio_limits)
    };
    // The acknowledgement wgpu wants beside an experimental feature: Helio's
    // for ray queries, else whatever the host passed for its own.
    let ray_queries = features.contains(ray_query);
    let experimental_features = if ray_queries {
        helio::required_experimental_features(features)
    } else {
        host.experimental_features
    };
    if ray_queries {
        tracing::info!("Engine device: hardware ray queries on");
    } else if adapter_features.contains(ray_query) {
        tracing::info!(
            "Engine device: hardware ray queries turned off; rendering on the tier without ray \
             tracing (Editor settings > Renderer > Hardware Ray Queries, then restart)"
        );
    } else {
        tracing::info!(
            "Engine device: this adapter runs no hardware ray queries; rendering on the tier \
             without ray tracing"
        );
    }
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: host.label,
        required_features: features,
        required_limits,
        experimental_features,
        memory_hints: host.memory_hints.clone(),
        trace: host.trace.clone(),
    }))
}
