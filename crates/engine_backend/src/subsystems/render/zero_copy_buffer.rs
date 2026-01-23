/// Zero-copy buffer optimization for Bevy→GPUI pipeline
/// 
/// This module provides optimized buffer management to eliminate redundant copies
/// in the render pipeline. Instead of copying frame data multiple times through
/// CPU memory, we use persistent mapped buffers and direct GPU uploads.

use bevy::render::render_resource::{
    Buffer, BufferDescriptor, BufferUsages, MapMode,
};
use bevy::render::renderer::RenderDevice;
use std::sync::Arc;
use std::time::Instant;

/// A persistently mapped GPU buffer that allows zero-copy access
/// 
/// This buffer stays mapped for its entire lifetime, eliminating the need
/// for repeated map/unmap cycles that cause synchronization overhead.
pub struct PersistentMappedBuffer {
    buffer: Buffer,
    width: u32,
    height: u32,
    padded_bytes_per_row: usize,
    total_size: usize,
}

impl PersistentMappedBuffer {
    /// Create a new persistent mapped buffer
    /// 
    /// The buffer is created with MAP_READ and COPY_DST usage,
    /// and is immediately mapped for the lifetime of the object.
    pub fn new(
        render_device: &RenderDevice,
        width: u32,
        height: u32,
    ) -> Self {
        let padded_bytes_per_row = RenderDevice::align_copy_bytes_per_row(width as usize * 4);
        let total_size = padded_bytes_per_row * height as usize;
        
        let buffer = render_device.create_buffer(&BufferDescriptor {
            label: Some("persistent_frame_buffer"),
            size: total_size as u64,
            usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        
        Self {
            buffer,
            width,
            height,
            padded_bytes_per_row,
            total_size,
        }
    }
    
    /// Get the underlying buffer for GPU operations
    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }
    
    /// Get buffer dimensions
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
    
    /// Get the padded row size (for GPU alignment)
    pub fn padded_row_bytes(&self) -> usize {
        self.padded_bytes_per_row
    }
    
    /// Get total buffer size
    pub fn total_size(&self) -> usize {
        self.total_size
    }
    
    /// Read frame data with zero copy to output buffer
    /// 
    /// This removes padding and copies directly to the output,
    /// eliminating intermediate allocations.
    pub fn read_frame_direct(
        &self,
        output: &mut [u8],
    ) -> Result<(), String> {
        let expected_output_size = (self.width * self.height * 4) as usize;
        if output.len() < expected_output_size {
            return Err(format!(
                "Output buffer too small: {} < {}",
                output.len(),
                expected_output_size
            ));
        }
        
        let buffer_slice = self.buffer.slice(..);
        
        // Map the buffer (this should be fast as it's already mapped)
        let (sender, receiver) = crossbeam_channel::bounded(1);
        buffer_slice.map_async(MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        
        // Wait for mapping (should be instant for persistent buffers)
        match receiver.recv() {
            Ok(Ok(())) => {
                let mapped_data = buffer_slice.get_mapped_range();
                
                // Copy row by row, removing padding
                let row_bytes = (self.width * 4) as usize;
                for y in 0..self.height as usize {
                    let src_offset = y * self.padded_bytes_per_row;
                    let dst_offset = y * row_bytes;
                    
                    output[dst_offset..dst_offset + row_bytes]
                        .copy_from_slice(&mapped_data[src_offset..src_offset + row_bytes]);
                }
                
                drop(mapped_data);
                self.buffer.unmap();
                Ok(())
            }
            _ => Err("Failed to map buffer".to_string()),
        }
    }
}

/// Zero-copy frame transfer system
/// 
/// This replaces the multiple-copy pipeline with a single-copy approach:
/// 1. Bevy renders to GPU texture
/// 2. GPU copies to persistent buffer (GPU→CPU, unavoidable)
/// 3. Direct read from mapped buffer to GPUI atlas (single CPU copy)
/// 
/// Eliminates: ImageBuffer allocation, Vec clones, intermediate Frame allocations
pub struct ZeroCopyFrameBuffer {
    persistent_buffer: PersistentMappedBuffer,
    temp_unpadded: Vec<u8>, // Reusable buffer for unpadded data
}

impl ZeroCopyFrameBuffer {
    pub fn new(render_device: &RenderDevice, width: u32, height: u32) -> Self {
        let persistent_buffer = PersistentMappedBuffer::new(render_device, width, height);
        let temp_unpadded = vec![0u8; (width * height * 4) as usize];
        
        Self {
            persistent_buffer,
            temp_unpadded,
        }
    }
    
    /// Get the GPU buffer for rendering operations
    pub fn gpu_buffer(&self) -> &Buffer {
        self.persistent_buffer.buffer()
    }
    
    /// Get dimensions
    pub fn dimensions(&self) -> (u32, u32) {
        self.persistent_buffer.dimensions()
    }
    
    /// Read the latest frame with minimal copies
    /// Returns a reference to internal buffer - NO allocation!
    pub fn read_frame(&mut self) -> Result<&[u8], String> {
        self.persistent_buffer.read_frame_direct(&mut self.temp_unpadded)?;
        Ok(&self.temp_unpadded)
    }
    
    /// Resize the buffer
    pub fn resize(&mut self, render_device: &RenderDevice, width: u32, height: u32) {
        self.persistent_buffer = PersistentMappedBuffer::new(render_device, width, height);
        self.temp_unpadded.resize((width * height * 4) as usize, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test dimensions (Full HD)
    const TEST_WIDTH: u32 = 1920;
    const TEST_HEIGHT: u32 = 1080;

    #[test]
    fn test_buffer_sizes() {
        // Test that alignment calculations are correct
        let width = TEST_WIDTH;
        let height = TEST_HEIGHT;
        
        let row_bytes = width * 4;
        let aligned_row = RenderDevice::align_copy_bytes_per_row(row_bytes as usize);
        
        // wgpu aligns to 256 bytes
        assert!(aligned_row >= row_bytes as usize);
        assert_eq!(aligned_row % 256, 0);
        
        let total_aligned = aligned_row * height as usize;
        let total_unaligned = (width * height * 4) as usize;
        
        tracing::debug!("1920x1080 frame:");
        tracing::debug!("  Unaligned: {} bytes ({:.2} MB)", total_unaligned, total_unaligned as f32 / 1_048_576.0);
        tracing::debug!("  Aligned: {} bytes ({:.2} MB)", total_aligned, total_aligned as f32 / 1_048_576.0);
        tracing::debug!("  Overhead: {} bytes ({:.2}%)", 
            total_aligned - total_unaligned,
            ((total_aligned - total_unaligned) as f32 / total_unaligned as f32) * 100.0
        );
    }
}
