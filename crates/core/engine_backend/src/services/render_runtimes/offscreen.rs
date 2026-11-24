//! An offscreen output with an asynchronous readback (thumbnails, captures).
use std::sync::{Arc, Mutex};

use super::{global, FrameTarget};

/// An offscreen output: a texture on the engine's device, read back without
/// blocking the frame (thumbnails, captures). [`OffscreenTarget::take_image`]
/// returns the last frame's pixels once its copy finished.
pub struct OffscreenTarget {
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    size: (u32, u32),
    texture: wgpu::Texture,
    readback: wgpu::Buffer,
    padded_row: u32,
    state: Arc<Mutex<Readback>>,
}

#[derive(Default)]
enum Readback {
    #[default]
    Empty,
    Pending,
    Ready(Vec<u8>),
}

impl OffscreenTarget {
    /// A `width` x `height` RGBA8 (sRGB) output on the engine's device.
    pub fn new(width: u32, height: u32) -> Option<Self> {
        let (device, queue) = global().device()?;
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Offscreen runtime output"),
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
        let padded_row = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Offscreen runtime readback"),
            size: u64::from(padded_row) * u64::from(height),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Some(Self {
            device,
            queue,
            format,
            size: (width, height),
            texture,
            readback,
            padded_row,
            state: Arc::new(Mutex::new(Readback::Empty)),
        })
    }

    /// The last frame's RGBA8 pixels (tightly packed rows), once its readback
    /// finished; `None` before. The device is polled without waiting.
    pub fn take_image(&self) -> Option<Vec<u8>> {
        let _ = self.device.poll(wgpu::PollType::Poll);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match std::mem::take(&mut *state) {
            Readback::Ready(pixels) => Some(pixels),
            other => {
                *state = other;
                None
            }
        }
    }
}

impl FrameTarget for OffscreenTarget {
    fn device(&self) -> wgpu::Device {
        self.device.clone()
    }

    fn queue(&self) -> wgpu::Queue {
        self.queue.clone()
    }

    fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn ready(&self) -> bool {
        !matches!(
            *self.state.lock().unwrap_or_else(|e| e.into_inner()),
            Readback::Pending
        )
    }

    fn draw(
        &self,
        draw: &mut dyn FnMut(&wgpu::TextureView, u32, u32) -> Option<wgpu::SubmissionIndex>,
    ) -> bool {
        let view = self.texture.create_view(&Default::default());
        let (width, height) = self.size;
        if draw(&view, width, height).is_none() {
            return false;
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Offscreen runtime readback"),
            });
        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = Readback::Pending;
        let state = Arc::clone(&self.state);
        let buffer = self.readback.clone();
        let padded_row = self.padded_row as usize;
        let row = width as usize * 4;
        self.readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let pixels = result.ok().and_then(|()| {
                    let pixels = buffer.slice(..).get_mapped_range().ok().map(|mapped| {
                        let mut pixels = Vec::with_capacity(row * height as usize);
                        for line in mapped.chunks(padded_row) {
                            pixels.extend_from_slice(&line[..row]);
                        }
                        pixels
                    });
                    buffer.unmap();
                    pixels
                });
                *state.lock().unwrap_or_else(|e| e.into_inner()) = match pixels {
                    Some(pixels) => Readback::Ready(pixels),
                    None => Readback::Empty,
                };
            });
        true
    }
}
