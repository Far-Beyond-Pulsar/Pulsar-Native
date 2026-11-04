use super::*;

/// Render a one-shot image at the requested dimensions and synchronously read
/// it back. Helio's next live frame resizes its graph back to the viewport.
pub(super) fn capture_viewport_image(
    engine: &mut GpuRenderer,
    surface: &WgpuSurfaceHandle,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> Option<image::RgbaImage> {
    let device = surface.device();
    let queue = surface.queue();
    let max_dimension = device.limits().max_texture_dimension_2d;
    if width > max_dimension || height > max_dimension {
        tracing::warn!(
            "High-resolution capture {width}x{height} exceeds GPU texture limit {max_dimension}"
        );
        return None;
    }
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("high-res-screenshot"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    engine.render_frame_to_surface(device, queue, &view, width, height, format)?;
    let bytes_per_row = align_up(width * 4, 256);
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("high-res-screenshot-readback"),
        size: (bytes_per_row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("high-res-screenshot-copy"),
    });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    if !matches!(rx.recv(), Ok(Ok(()))) {
        return None;
    }
    let data = slice.get_mapped_range().ok()?;
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        let start = (row * bytes_per_row) as usize;
        pixels.extend_from_slice(&data[start..start + (width * 4) as usize]);
    }
    drop(data);
    staging.unmap();
    // Preserve the renderer's encoded sRGB bytes in the exported PNG. The
    // conversion used by the project-thumbnail path below compensates for the
    // editor's composite path and would visibly darken a normal screenshot.
    Some(image::RgbaImage::from_raw(width, height, pixels)?)
}

/// Renders the current Helio scene into an offscreen texture, reads it back
/// from the GPU, and writes it to `out_path` as a PNG. Used to capture
/// project thumbnails on scene save.
pub(super) fn capture_viewport_thumbnail(
    engine: &mut GpuRenderer,
    surface: &WgpuSurfaceHandle,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    out_path: &std::path::Path,
) {
    let device = surface.device();
    let queue = surface.queue();

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("thumbnail-capture"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    let _ = engine.render_frame_to_surface(device, queue, &view, width, height, format);

    let bytes_per_row = align_up(width * 4, 256);
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("thumbnail-staging"),
        size: (bytes_per_row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("thumbnail-readback"),
    });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);

    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    let _ = device.poll(wgpu::PollType::wait_indefinitely());

    match rx.recv() {
        Ok(Ok(())) => {}
        _ => {
            tracing::warn!("[THUMBNAIL] Failed to map readback buffer");
            return;
        }
    }

    let data = match slice.get_mapped_range() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("[THUMBNAIL] Failed to get mapped range: {:?}", e);
            return;
        }
    };
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        let start = (row * bytes_per_row) as usize;
        let end = start + (width * 4) as usize;
        pixels.extend_from_slice(&data[start..end]);
    }
    drop(data);
    staging.unmap();

    let Some(rgba) = image::RgbaImage::from_raw(width, height, pixels) else {
        tracing::warn!(
            "[THUMBNAIL] Pixel buffer size mismatch for {}x{}",
            width,
            height
        );
        return;
    };

    if let Some(parent) = out_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match rgba.save(out_path) {
        Ok(()) => tracing::info!(
            "[THUMBNAIL] Saved viewport thumbnail to {}",
            out_path.display()
        ),
        Err(e) => tracing::warn!("[THUMBNAIL] Failed to save {}: {}", out_path.display(), e),
    }
}

fn align_up(n: u32, align: u32) -> u32 {
    (n + align - 1) & !(align - 1)
}
