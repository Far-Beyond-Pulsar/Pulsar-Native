// OPTIMIZED: Wrapper around the Helio renderer (blade-graphics)
// Migrated from Bevy to Helio for better performance and control

use crate::subsystems::render::{HelioRenderer, RenderMetrics};
use std::sync::{Arc, Mutex, Once};
use std::time::Instant;

/// Simple framebuffer structure for compatibility
pub struct ViewportFramebuffer {
    pub buffer: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub generation: u64,
}

static INIT: Once = Once::new();
static mut RUNTIME: Option<tokio::runtime::Runtime> = None;

fn get_runtime() -> &'static tokio::runtime::Runtime {
    unsafe {
        INIT.call_once(|| {
            RUNTIME = Some(
                tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime")
            );
        });
        RUNTIME.as_ref().unwrap()
    }
}

/// OPTIMIZED GPU Renderer - uses Helio with blade-graphics
/// 
/// Migrated from Bevy to Helio for:
/// - Better performance with blade-graphics
/// - More direct control over rendering pipeline
/// - Simplified architecture
pub struct GpuRenderer {
    pub helio_renderer: Option<HelioRenderer>,
    render_width: u32,
    render_height: u32,
    display_width: u32,
    display_height: u32,
    frame_count: u64,
    start_time: Instant,
    last_metrics_print: Instant,
}

impl GpuRenderer {
    pub fn new(display_width: u32, display_height: u32) -> Self {
        Self::new_with_game_thread(display_width, display_height, None)
    }

    pub fn new_with_game_thread(
        display_width: u32, 
        display_height: u32,
        game_thread_state: Option<Arc<Mutex<crate::subsystems::game::GameState>>>,
    ) -> Self {
        let width = display_width;
        let height = display_height;
        
        tracing::info!("[GPU-RENDERER] 🚀 Initializing Helio renderer (blade-graphics) at {}x{}", width, height);
        
        let runtime = get_runtime();
        let game_state_for_renderer = game_thread_state.clone();
        let helio_renderer = runtime.block_on(async move {
            tracing::debug!("[GPU-RENDERER] Creating Helio renderer asynchronously...");
            match tokio::time::timeout(
                tokio::time::Duration::from_secs(10),
                HelioRenderer::new_with_game_thread(width, height, game_state_for_renderer)
            ).await {
                Ok(renderer) => {
                    tracing::info!("[GPU-RENDERER] ✅ Helio renderer created successfully!");
                    Some(renderer)
                }
                Err(_) => {
                    tracing::error!("[GPU-RENDERER] ⚠️  Helio renderer creation timed out!");
                    None
                }
            }
        });

        if helio_renderer.is_none() {
            tracing::error!("[GPU-RENDERER] Failed to create Helio renderer!");
        }

        Self {
            helio_renderer,
            render_width: width,
            render_height: height,
            display_width,
            display_height,
            frame_count: 0,
            start_time: Instant::now(),
            last_metrics_print: Instant::now(),
        }
    }

    pub fn render(&mut self, _framebuffer: &mut ViewportFramebuffer) {
        self.frame_count += 1;

        // IMMEDIATE MODE: No rendering here!
        // Viewport should call get_native_texture_handle() and use GPUI's immediate mode
        // This method is just a stub for compatibility

        if let Some(ref renderer) = self.helio_renderer {
            // Print metrics periodically
            if self.last_metrics_print.elapsed().as_secs() >= 5 {
                let metrics = renderer.get_metrics();
                let fps = self.get_fps();
                tracing::info!("\n[GPU-RENDERER] 🚀 Helio Renderer Stats:");
                tracing::info!("  Frames rendered: {}", metrics.frames_rendered);
                tracing::info!("  FPS: {:.1}", metrics.fps);
                tracing::info!("  Frame time: {:.2}ms", metrics.frame_time_ms);
                self.last_metrics_print = Instant::now();
            }
        }
    }

    /// TRUE ZERO-COPY: Get native GPU texture handle for immediate-mode rendering
    /// NO buffers, NO copies - just a raw pointer for GPUI to display!
    pub fn get_native_texture_handle(&self) -> Option<gpui::GpuTextureHandle> {
        let renderer = self.helio_renderer.as_ref()?;
        let shared_textures = renderer.shared_textures.lock().ok()?;
        let textures = shared_textures.as_ref()?;
        
        // Get the read handle from the double-buffered array
        let handles = textures.native_handles.lock().ok()?;
        let handle_array = handles.as_ref()?;
        
        let read_idx = textures.read_index.load(std::sync::atomic::Ordering::Relaxed);
        let handle = handle_array[read_idx].clone();
        
        tracing::debug!("[GPU-RENDERER] Returning DXGI handle[{}]: {:?}", read_idx, handle);
        Some(handle)
    }

    /// DEPRECATED: Use get_native_texture_handle() + immediate mode instead!
    /// This method does NOTHING in zero-copy mode
    pub fn render_to_buffer(&mut self, _gpu_buffer: &mut [u8]) {
        // NO-OP in TRUE zero-copy mode
        // Viewport should use get_native_texture_handle() and GPUI immediate rendering
    }

    fn render_fallback(&self, framebuffer: &mut ViewportFramebuffer) {
        // Render a simple animated pattern to show the system works
        let time = self.frame_count as f32 * 0.016;

        for y in 0..framebuffer.height {
            for x in 0..framebuffer.width {
                let idx = ((y * framebuffer.width + x) * 4) as usize;

                let u = x as f32 / framebuffer.width as f32;
                let v = y as f32 / framebuffer.height as f32;

                // Create a moving gradient pattern
                let r = ((u + time.sin() * 0.5).sin() * 128.0 + 127.0) as u8;
                let g = ((v + time.cos() * 0.5).cos() * 128.0 + 127.0) as u8;
                let b = (((u + v) * 2.0 + time).sin() * 128.0 + 127.0) as u8;

                if idx + 3 < framebuffer.buffer.len() {
                    framebuffer.buffer[idx] = r;
                    framebuffer.buffer[idx + 1] = g;
                    framebuffer.buffer[idx + 2] = b;
                    framebuffer.buffer[idx + 3] = 255;
                }
            }
        }

        framebuffer.generation += 1;
    }

    fn render_fallback_to_buffer(&self, buffer: &mut [u8]) {
        let time = self.frame_count as f32 * 0.016;
        let pixel_count = buffer.len() / 4;
        let width = self.display_width;

        for i in 0..pixel_count {
            let idx = i * 4;
            let x = (i as u32 % width) as f32;
            let y = (i as u32 / width) as f32;

            let u = x / width as f32;
            let v = y / self.display_height as f32;

            let r = ((u + time.sin() * 0.5).sin() * 128.0 + 127.0) as u8;
            let g = ((v + time.cos() * 0.5).cos() * 128.0 + 127.0) as u8;
            let b = (((u + v) * 2.0 + time).sin() * 128.0 + 127.0) as u8;

            if idx + 3 < buffer.len() {
                buffer[idx] = r;
                buffer[idx + 1] = g;
                buffer[idx + 2] = b;
                buffer[idx + 3] = 255;
            }
        }
    }

    pub fn get_frame_count(&self) -> u64 {
        self.frame_count
    }

    pub fn get_fps(&self) -> f32 {
        let elapsed = self.start_time.elapsed().as_secs_f32();
        if elapsed > 0.0 {
            self.frame_count as f32 / elapsed
        } else {
            0.0
        }
    }
    
    /// Get Helio renderer FPS (actual render engine frame rate)
    pub fn get_helio_fps(&self) -> f32 {
        if let Some(ref renderer) = self.helio_renderer {
            let metrics = renderer.get_metrics();
            metrics.fps
        } else {
            0.0
        }
    }
    
    /// Get comprehensive render metrics
    pub fn get_render_metrics(&self) -> Option<RenderMetrics> {
        self.helio_renderer.as_ref().map(|r| r.get_metrics())
    }
    
    /// Get pipeline time in microseconds
    pub fn get_pipeline_time_us(&self) -> u64 {
        if let Some(ref renderer) = self.helio_renderer {
            renderer.get_metrics().pipeline_time_us as u64
        } else {
            0
        }
    }
    
    /// Get GPU time in microseconds
    pub fn get_gpu_time_us(&self) -> u64 {
        if let Some(ref renderer) = self.helio_renderer {
            renderer.get_metrics().gpu_time_us as u64
        } else {
            0
        }
    }
    
    /// Get CPU time in microseconds
    pub fn get_cpu_time_us(&self) -> u64 {
        if let Some(ref renderer) = self.helio_renderer {
            renderer.get_metrics().cpu_time_us as u64
        } else {
            0
        }
    }
    
    /// Get detailed GPU pipeline profiling data
    pub fn get_gpu_profiler_data(&self) -> Option<crate::subsystems::render::GpuProfilerData> {
        self.helio_renderer.as_ref().map(|r| r.get_gpu_profiler_data())
    }

    /// Update camera input for Unreal-style controls
    pub fn update_camera_input(&mut self, input: crate::subsystems::render::CameraInput) {
        if let Some(ref renderer) = self.helio_renderer {
            // Update camera input in shared state
            if let Ok(mut camera_input) = renderer.camera_input.lock() {
                *camera_input = input;
            }
        }
    }

    pub fn resize(&mut self, display_width: u32, display_height: u32) {
        if self.display_width != display_width || self.display_height != display_height {
            self.render_width = display_width;
            self.render_height = display_height;
            self.display_width = display_width;
            self.display_height = display_height;
            
            tracing::info!("[GPU-RENDERER] Resizing to {}x{}", display_width, display_height);
            
            // TODO: Implement resize for Helio renderer
            // Need to recreate render targets at new resolution
        }
    }
}

unsafe impl Send for GpuRenderer {}
unsafe impl Sync for GpuRenderer {}
