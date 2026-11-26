//! The viewport's side of the `viewport_screenshot` MCP tool.
//!
//! The tool (`crate::ai::screenshot_tool`) leaves a [`McpScreenshotRequest`] in
//! the level editor's build state and waits on its reply channel. The viewport
//! takes the request on its next frame (`start_pending_screenshot`), renders it
//! with the same tiled, frozen-time path as the High res screenshot window and
//! writes the file on an editor task. No dialog opens and the UI thread only
//! hands the work over: the GPU readback and the encode run on the task.

use super::*;
use crate::state::build::{McpScreenshotOutcome, McpScreenshotRequest, ScreenshotFormat};
use engine_backend::subsystems::render::CaptureView;

impl HelioViewport {
    /// Render `request` on an editor task and answer it. Runs on the UI thread
    /// for as long as it takes to start the task.
    pub(super) fn start_mcp_screenshot(
        &mut self,
        request: McpScreenshotRequest,
        format: wgpu::TextureFormat,
    ) {
        let state = self.shared_state.clone();
        let Some(surface) = self.surface.clone() else {
            finish(
                &state,
                &request,
                Err("the viewport has no GPU surface yet: it has not drawn a frame".into()),
            );
            return;
        };
        let engine = self.gpu_engine.clone();
        let temp_path = std::env::temp_dir().join(format!(
            "pulsar-mcp-screenshot-{}-{}.png",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        state.write().build.screenshot_progress = Some((0, 1));
        editor_task_queue::global().submit(
            editor_task_queue::TaskDescription::new(
                "Viewport screenshot (MCP)",
                "Render",
                editor_task_queue::TaskDuration::Short,
            ),
            move |task| {
                task.set_phase("Rendering the frame");
                let started = Instant::now();
                let rendered =
                    render_to_temp(&engine, &surface, &request, format, &temp_path, &state);
                let result = rendered.and_then(|tiles| {
                    task.set_phase("Writing the file");
                    write_destination(&temp_path, &request, tiles, started)
                });
                remove_temp_screenshot(&temp_path);
                if let Err(error) = &result {
                    tracing::warn!("MCP viewport screenshot failed: {error}");
                }
                let status = result.as_ref().map(|_| ()).map_err(Clone::clone);
                finish(&state, &request, result);
                status
            },
        );
    }
}

/// Clear the editor's processing flags and answer the tool.
fn finish(
    state: &Arc<parking_lot::RwLock<LevelEditorState>>,
    request: &McpScreenshotRequest,
    result: Result<McpScreenshotOutcome, String>,
) {
    {
        let mut state = state.write();
        state.build.screenshot_processing = false;
        state.build.screenshot_progress = None;
    }
    // The tool may have timed out and gone; nobody to tell then.
    let _ = request.reply.send(result);
}

/// Draw the tiles into a temporary PNG with the grid, gizmo and overlay lines
/// left out as asked, and put them back before the renderer is released.
/// Returns the tile count.
fn render_to_temp(
    engine: &Arc<Mutex<GpuRenderer>>,
    surface: &WgpuSurfaceHandle,
    request: &McpScreenshotRequest,
    format: wgpu::TextureFormat,
    temp_path: &std::path::Path,
    state: &Arc<parking_lot::RwLock<LevelEditorState>>,
) -> Result<u64, String> {
    let mut engine = engine
        .lock()
        .map_err(|_| "the Helio renderer lock was poisoned".to_string())?;
    engine.set_capture_view(CaptureView {
        hide_grid: request.hide_grid,
        hide_gizmos: request.hide_gizmos,
        hide_overlays: request.hide_overlays,
    });
    let mut tiles = 0u64;
    let progress_state = state.clone();
    let captured = catch_unwind(AssertUnwindSafe(|| {
        capture_viewport_png_tiled(
            &mut engine,
            surface,
            request.width,
            request.height,
            format,
            temp_path,
            |completed, total| {
                tiles = total;
                progress_state.write().build.screenshot_progress = Some((completed, total));
            },
        )
    }));
    // Always put the editor's overlays back, whatever happened.
    engine.set_capture_view(CaptureView::default());
    drop(engine);
    match captured {
        Ok(Ok(())) => Ok(tiles),
        Ok(Err(error)) => Err(error),
        Err(payload) => {
            let message = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&'static str>().copied())
                .unwrap_or("unknown panic");
            Err(format!(
                "Helio panicked while rendering a screenshot tile (the GPU device may be lost): {message}"
            ))
        }
    }
}

// Allowed std::fs: the tool writes where the caller asked (an absolute path on
// this machine, or a file under the project's `.pulsar/screenshots/`), from a
// temp file in the OS temp dir; neither is a project asset the index tracks.
#[allow(clippy::disallowed_methods)]
fn write_destination(
    temp_path: &std::path::Path,
    request: &McpScreenshotRequest,
    tiles: u64,
    started: Instant,
) -> Result<McpScreenshotOutcome, String> {
    if let Some(parent) = request.destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    match request.format {
        ScreenshotFormat::Png => {
            std::fs::copy(temp_path, &request.destination)
                .map_err(|e| format!("could not write {}: {e}", request.destination.display()))?;
        }
        ScreenshotFormat::Jpeg(quality) => {
            let image = image::open(temp_path)
                .map_err(|e| format!("could not read the rendered frame back to encode it: {e}"))?
                .to_rgb8();
            let file = std::fs::File::create(&request.destination)
                .map_err(|e| format!("could not create {}: {e}", request.destination.display()))?;
            let mut writer = std::io::BufWriter::new(file);
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut writer, quality)
                .encode(
                    image.as_raw(),
                    image.width(),
                    image.height(),
                    image::ExtendedColorType::Rgb8,
                )
                .map_err(|e| format!("JPEG encoding failed: {e}"))?;
            std::io::Write::flush(&mut writer)
                .map_err(|e| format!("could not write {}: {e}", request.destination.display()))?;
        }
    }
    let bytes = std::fs::metadata(&request.destination)
        .map(|m| m.len())
        .map_err(|e| format!("could not stat {}: {e}", request.destination.display()))?;
    Ok(McpScreenshotOutcome {
        path: request.destination.clone(),
        width: request.width,
        height: request.height,
        format: request.format,
        bytes,
        tiles,
        millis: started.elapsed().as_millis() as u64,
    })
}
