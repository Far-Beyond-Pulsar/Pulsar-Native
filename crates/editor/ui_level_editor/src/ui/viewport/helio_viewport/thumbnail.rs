use super::*;

/// Render a high-resolution image as GPU-sized tiles, freezing scene time for
/// the whole sequence and streaming rows to a PNG so output size is not bounded
/// by a single texture or a full-frame CPU allocation.
pub(super) fn capture_viewport_png_tiled(
    engine: &mut GpuRenderer,
    surface: &WgpuSurfaceHandle,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    out_path: &std::path::Path,
    mut on_tile_complete: impl FnMut(u64, u64),
) -> Result<(), String> {
    let device = surface.device();
    let queue = surface.queue();
    if width == 0 || height == 0 {
        return Err("Screenshot dimensions must be non-zero".into());
    }
    let max_dimension = device.limits().max_texture_dimension_2d;
    let tile_side = 8192u32.min(max_dimension).max(1);
    // Bound assembled CPU memory while preserving reasonably sized GPU jobs.
    const BAND_BYTES: u64 = 512 * 1024 * 1024;
    let band_height = (BAND_BYTES / (u64::from(width) * 4))
        .max(1)
        .min(u64::from(tile_side)) as u32;
    let columns = width.div_ceil(tile_side);
    let rows = height.div_ceil(band_height);
    let total_tiles = u64::from(columns) * u64::from(rows);
    let mut completed_tiles = 0;

    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out_path)
        .map_err(|e| format!("Could not create temporary PNG: {e}"))?;
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let writer = encoder
        .write_header()
        .map_err(|e| format!("Could not start PNG encoding: {e}"))?;
    let mut writer = writer
        .into_stream_writer()
        .map_err(|e| format!("Could not stream PNG data: {e}"))?;

    let mut band_y = 0;
    while band_y < height {
        let tile_height = band_height.min(height - band_y);
        let mut tiles = Vec::new();
        let mut tile_x = 0;
        while tile_x < width {
            let tile_width = tile_side.min(width - tile_x);
            let pixels = capture_viewport_tile(
                engine,
                surface,
                format,
                [width, height],
                [tile_x, band_y],
                [tile_width, tile_height],
            )
            .map_err(|error| {
                format!(
                    "Screenshot tile at ({tile_x}, {band_y}) with size {tile_width} × {tile_height} failed: {error}"
                )
            })?;
            tiles.push((tile_width, pixels));
            completed_tiles += 1;
            on_tile_complete(completed_tiles, total_tiles);
            tile_x += tile_width;
        }

        let mut row = Vec::with_capacity(width as usize * 4);
        for y in 0..tile_height as usize {
            row.clear();
            for (tile_width, pixels) in &tiles {
                let row_bytes = *tile_width as usize * 4;
                let start = y * row_bytes;
                row.extend_from_slice(&pixels[start..start + row_bytes]);
            }
            std::io::Write::write_all(&mut writer, &row)
                .map_err(|e| format!("Could not write PNG rows: {e}"))?;
        }
        band_y += tile_height;
    }
    writer
        .finish()
        .map_err(|e| format!("Could not finish PNG file: {e}"))
}

fn capture_viewport_tile(
    engine: &mut GpuRenderer,
    surface: &WgpuSurfaceHandle,
    format: wgpu::TextureFormat,
    full_size: [u32; 2],
    origin: [u32; 2],
    tile_size: [u32; 2],
) -> Result<Vec<u8>, String> {
    let [width, height] = tile_size;
    let device = surface.device();
    let queue = surface.queue();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("high-res-screenshot-tile"),
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
    let crop = engine_backend::subsystems::render::FrameCrop {
        full_size,
        origin,
        tile_size,
    };
    engine
        .render_capture_tile(device, queue, &view, width, height, format, crop)
        .ok_or_else(|| "Helio did not submit the screenshot tile".to_string())?;

    let bytes_per_row = align_up(width.checked_mul(4).ok_or("Tile row is too wide")?, 256);
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("high-res-screenshot-tile-readback"),
        size: u64::from(bytes_per_row) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("high-res-screenshot-tile-copy"),
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
        Ok(Err(error)) => return Err(format!("GPU tile readback failed: {error}")),
        Err(error) => return Err(format!("GPU tile readback callback failed: {error}")),
    }
    let data = slice
        .get_mapped_range()
        .map_err(|e| format!("Could not map screenshot tile: {e}"))?;
    let row_bytes = width as usize * 4;
    let mut pixels = Vec::with_capacity(row_bytes * height as usize);
    for row in 0..height {
        let start = row as usize * bytes_per_row as usize;
        pixels.extend_from_slice(&data[start..start + row_bytes]);
    }
    drop(data);
    staging.unmap();
    Ok(pixels)
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
