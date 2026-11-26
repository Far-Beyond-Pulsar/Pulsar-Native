//! `viewport_screenshot`: a still of the level viewport as a file, by MCP.
//!
//! The editor already had a high resolution screenshot (Tools menu, High res
//! screenshot); it ends in a save dialog, so nothing but a person could finish
//! it. This tool asks for the same frame, rendered by the same tiled path with
//! scene time frozen, and writes it where the caller says.
//!
//! The call runs on an MCP server worker. It validates the arguments, leaves a
//! [`McpScreenshotRequest`] in the editor's build state under one short write
//! lock and waits on a channel; the viewport takes the request on its next
//! frame and renders and encodes it on an editor task (see
//! `ui::viewport::helio_viewport::mcp_screenshot`). The worker never holds a
//! scene lock while it waits, and never reads the scene: only the build
//! domain's flags, once.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pulsar_mcp::{Annotations, McpToolRegistration, ToolResult};
use serde_json::{json, Value};

use crate::state::build::{McpScreenshotOutcome, McpScreenshotRequest, ScreenshotFormat};
use crate::LevelEditorState;

type EditorState = Arc<parking_lot::RwLock<LevelEditorState>>;

/// The default size, and the longest side a capture may have. Larger frames
/// are rendered in tiles but the encoder and the callers' memory stop being
/// reasonable well before this.
const DEFAULT_SIZE: (u32, u32) = (2560, 1440);
const MAX_SIDE: u32 = 16_384;
const MIN_SIDE: u32 = 16;
/// A JPEG is encoded from the whole frame in memory (4 bytes a pixel).
const MAX_JPEG_PIXELS: u64 = 64 * 1024 * 1024;
const DEFAULT_TIMEOUT_SECS: u64 = 120;

static NEXT_NAME: AtomicU64 = AtomicU64::new(1);

/// What a call asked for, validated.
#[derive(Debug, PartialEq)]
pub(crate) struct ScreenshotPlan {
    pub destination: PathBuf,
    pub format: ScreenshotFormat,
    pub requested: (u32, u32),
    pub size: (u32, u32),
    pub hide_grid: bool,
    pub hide_gizmos: bool,
    pub hide_overlays: bool,
    pub timeout: Duration,
    /// The level whose viewport to shoot; `None` is the only open one.
    pub viewport: Option<PathBuf>,
}

fn integer(arguments: &Value, key: &str) -> Result<Option<i64>, String> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .or_else(|| {
                value
                    .as_f64()
                    .filter(|f| f.fract() == 0.0)
                    .map(|f| f as i64)
            })
            .map(Some)
            .ok_or_else(|| format!("`{key}` must be a whole number")),
    }
}

fn flag(arguments: &Value, key: &str, default: bool) -> Result<bool, String> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(format!("`{key}` must be true or false")),
    }
}

/// Validate the arguments. `project` is the open project's folder: relative
/// paths and the default destination (`.pulsar/screenshots/`) live under it.
pub(crate) fn plan(
    arguments: &Value,
    project: Option<&Path>,
    unique: u64,
) -> Result<ScreenshotPlan, String> {
    let requested = (
        integer(arguments, "width")?.unwrap_or(i64::from(DEFAULT_SIZE.0)),
        integer(arguments, "height")?.unwrap_or(i64::from(DEFAULT_SIZE.1)),
    );
    if requested.0 < 1 || requested.1 < 1 {
        return Err(format!(
            "width and height must be at least 1 (got {} x {})",
            requested.0, requested.1
        ));
    }
    let clamp = |side: i64| side.clamp(i64::from(MIN_SIDE), i64::from(MAX_SIDE)) as u32;
    let size = (clamp(requested.0), clamp(requested.1));
    let requested = (
        requested.0.min(i64::from(u32::MAX)) as u32,
        requested.1.min(i64::from(u32::MAX)) as u32,
    );

    let quality = match integer(arguments, "jpeg_quality")? {
        None => 90u8,
        Some(q) if (1..=100).contains(&q) => q as u8,
        Some(q) => return Err(format!("`jpeg_quality` must be 1 to 100 (got {q})")),
    };

    let destination = match arguments.get("path").and_then(Value::as_str) {
        Some(path) if !path.trim().is_empty() => {
            let path = PathBuf::from(path);
            if path.is_relative() {
                match project {
                    Some(root) => root.join(path),
                    None => {
                        return Err(
                            "`path` is relative and no project is open to resolve it against; \
                             pass an absolute path"
                                .into(),
                        )
                    }
                }
            } else {
                path
            }
        }
        _ => {
            let Some(root) = project else {
                return Err("no `path` was given and no project is open to hold the default \
                            `.pulsar/screenshots/` folder; pass an absolute path"
                    .into());
            };
            let millis = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            root.join(".pulsar")
                .join("screenshots")
                .join(format!("shot-{millis}-{unique}.png"))
        }
    };
    let format = match destination
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => ScreenshotFormat::Png,
        Some("jpg" | "jpeg") => ScreenshotFormat::Jpeg(quality),
        _ => {
            return Err(format!(
                "{} must end in .png, .jpg or .jpeg: the extension picks the format",
                destination.display()
            ))
        }
    };
    if matches!(format, ScreenshotFormat::Jpeg(_))
        && u64::from(size.0) * u64::from(size.1) > MAX_JPEG_PIXELS
    {
        return Err(format!(
            "a JPEG of {} x {} is too big to encode ({} megapixels at most); \
             ask for a PNG or a smaller size",
            size.0,
            size.1,
            MAX_JPEG_PIXELS / (1024 * 1024)
        ));
    }
    let timeout = match integer(arguments, "timeout_seconds")? {
        None => DEFAULT_TIMEOUT_SECS,
        Some(t) if (1..=1800).contains(&t) => t as u64,
        Some(t) => return Err(format!("`timeout_seconds` must be 1 to 1800 (got {t})")),
    };
    Ok(ScreenshotPlan {
        destination,
        format,
        requested,
        size,
        hide_grid: flag(arguments, "hide_grid", true)?,
        hide_gizmos: flag(arguments, "hide_gizmos", true)?,
        hide_overlays: flag(arguments, "hide_overlays", true)?,
        timeout: Duration::from_secs(timeout),
        viewport: arguments
            .get("viewport")
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty())
            .map(PathBuf::from),
    })
}

/// Hand the request to the editor's viewport and wait for its answer.
///
/// Holds the editor's state lock only to check it and to leave the request.
pub(crate) fn request_screenshot(
    state: &EditorState,
    plan: &ScreenshotPlan,
) -> Result<McpScreenshotOutcome, String> {
    let (reply, answer) = mpsc::channel();
    {
        let mut state = state.write();
        if state.build.viewport_size.0 == 0 || state.build.viewport_size.1 == 0 {
            return Err("this level has no viewport that has drawn a frame: open the level in \
                        the Level Editor and wait for the viewport to show the scene"
                .into());
        }
        if state.build.screenshot_processing || state.build.pending_mcp_screenshot.is_some() {
            return Err("a screenshot is already being taken in this editor; try again when it \
                        finishes"
                .into());
        }
        state.build.screenshot_processing = true;
        state.build.pending_mcp_screenshot = Some(McpScreenshotRequest {
            width: plan.size.0,
            height: plan.size.1,
            destination: plan.destination.clone(),
            format: plan.format,
            hide_grid: plan.hide_grid,
            hide_gizmos: plan.hide_gizmos,
            hide_overlays: plan.hide_overlays,
            reply,
        });
    }
    // A viewport that is not being redrawn (a minimised or covered window)
    // does not see the request until it is. Ask for a frame; the answer does
    // not matter, the viewport may already be drawing continuously.
    let _ = ui_common::ui_call::call(Duration::from_secs(2), |cx| cx.refresh_windows());

    let started = Instant::now();
    match answer.recv_timeout(plan.timeout) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => {
            let mut state = state.write();
            if state.build.pending_mcp_screenshot.take().is_some() {
                state.build.screenshot_processing = false;
                Err(format!(
                    "the viewport did not pick the request up within {} s: it is not drawing \
                     (is its window minimised, or the level's tab hidden?)",
                    started.elapsed().as_secs()
                ))
            } else {
                Err(format!(
                    "the viewport started the screenshot but it was not finished after {} s; \
                     {} may appear later. Ask for a smaller size or a longer `timeout_seconds`",
                    started.elapsed().as_secs(),
                    plan.destination.display()
                ))
            }
        }
        Err(RecvTimeoutError::Disconnected) => {
            let mut state = state.write();
            state.build.pending_mcp_screenshot = None;
            state.build.screenshot_processing = false;
            Err("the viewport closed before the screenshot was taken".into())
        }
    }
}

fn format_name(format: ScreenshotFormat) -> &'static str {
    match format {
        ScreenshotFormat::Png => "png",
        ScreenshotFormat::Jpeg(_) => "jpeg",
    }
}

fn viewport_screenshot(arguments: &str) -> ToolResult {
    let arguments: Value = if arguments.trim().is_empty() {
        json!({})
    } else {
        match serde_json::from_str(arguments) {
            Ok(value @ Value::Object(_)) => value,
            _ => return ToolResult::error("the arguments must be a JSON object"),
        }
    };
    let project = engine_state::get_project_path().map(PathBuf::from);
    let plan = match plan(
        &arguments,
        project.as_deref(),
        NEXT_NAME.fetch_add(1, Ordering::Relaxed),
    ) {
        Ok(plan) => plan,
        Err(message) => return ToolResult::error(message),
    };
    let state = match super::sessions::find_editor(plan.viewport.as_deref()) {
        Ok(state) => state,
        Err(message) => {
            return ToolResult::error(format!("no level viewport to shoot: {message}"));
        }
    };
    match request_screenshot(&state, &plan) {
        Ok(done) => ToolResult::json(&json!({
            "path": done.path.display().to_string(),
            "format": format_name(done.format),
            "width": done.width,
            "height": done.height,
            "requested_width": plan.requested.0,
            "requested_height": plan.requested.1,
            "clamped": plan.requested != plan.size,
            "bytes": done.bytes,
            "tiles": done.tiles,
            "milliseconds": done.millis,
            "hid": {
                "grid": plan.hide_grid,
                "gizmos": plan.hide_gizmos,
                "overlays": plan.hide_overlays,
            },
        })),
        Err(message) => ToolResult::error(format!("screenshot failed: {message}")),
    }
}

pulsar_plugins::inventory::submit! {
    McpToolRegistration {
        origin: pulsar_plugins::engine_origin!(),
        name: "viewport_screenshot",
        title: "Screenshot the level viewport",
        description: "Render the Level Editor's viewport as an image file: the same high resolution, tiled, scene-time-frozen frame as Tools > High res screenshot, with no dialog. Returns the written path, size, format, byte count and the time taken. Width and height default to 2560 x 1440 and are clamped to 16 to 16384 per side (the result says what was used). The grid, the transform gizmo and the editor's overlay lines (spline curves, volume bounds) are hidden by default so they do not appear in the picture; set hide_grid, hide_gizmos or hide_overlays to false to keep one. The extension of `path` picks the format: .png or .jpg/.jpeg. The level must be open with a viewport that has drawn a frame. Fails with a reason when no viewport is open, one screenshot is already running, the device is lost or the file cannot be written.",
        input_schema: r#"{"type":"object","properties":{"path":{"type":"string","description":"Where to write the image: an absolute path (parent folders are created) ending in .png, .jpg or .jpeg, or a path relative to the project. Omitted: <project>/.pulsar/screenshots/shot-<time>-<n>.png."},"width":{"type":"integer","description":"Image width in pixels. Default 2560. Clamped to 16..16384.","minimum":1},"height":{"type":"integer","description":"Image height in pixels. Default 1440. Clamped to 16..16384.","minimum":1},"viewport":{"type":"string","description":"Which level's viewport: the path of an open .level file. Omitted: the only open level (an error listing them when several are open)."},"hide_grid":{"type":"boolean","description":"Leave the editor's ground grid out. Default true."},"hide_gizmos":{"type":"boolean","description":"Leave the transform gizmo out. Default true."},"hide_overlays":{"type":"boolean","description":"Leave the editor's world-space overlay lines out (spline curves, volume bounds). Default true."},"jpeg_quality":{"type":"integer","description":"JPEG quality 1..100 for a .jpg path. Default 90.","minimum":1,"maximum":100},"timeout_seconds":{"type":"integer","description":"How long to wait for the render. Default 120, at most 1800.","minimum":1,"maximum":1800}}}"#,
        annotations: Annotations::WRITES,
        call: viewport_screenshot,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> PathBuf {
        PathBuf::from(if cfg!(windows) { "C:/proj" } else { "/proj" })
    }

    #[test]
    fn defaults_are_2560_by_1440_png_hiding_the_editor() {
        let plan = plan(&json!({}), Some(&project()), 7).unwrap();
        assert_eq!(plan.size, (2560, 1440));
        assert_eq!(plan.format, ScreenshotFormat::Png);
        assert!(plan.hide_grid && plan.hide_gizmos && plan.hide_overlays);
        let name = plan.destination.to_string_lossy().replace('\\', "/");
        assert!(name.contains("/.pulsar/screenshots/shot-"), "{name}");
        assert!(name.ends_with("-7.png"), "{name}");
        assert_eq!(plan.timeout, Duration::from_secs(120));
    }

    #[test]
    fn the_extension_picks_the_format_and_quality() {
        let plan = plan(
            &json!({"path": "C:/out/a.JPG", "jpeg_quality": 70}),
            None,
            1,
        )
        .unwrap();
        assert_eq!(plan.format, ScreenshotFormat::Jpeg(70));
        let error = super::plan(&json!({"path": "C:/out/a.bmp"}), None, 1).unwrap_err();
        assert!(error.contains(".png, .jpg or .jpeg"), "{error}");
    }

    #[test]
    fn sizes_are_clamped_and_the_request_is_kept() {
        let plan = plan(
            &json!({"path": "C:/o.png", "width": 40000, "height": 4}),
            None,
            1,
        )
        .unwrap();
        assert_eq!(plan.size, (MAX_SIDE, MIN_SIDE));
        assert_eq!(plan.requested, (40000, 4));
        assert!(super::plan(&json!({"path": "C:/o.png", "width": 0}), None, 1).is_err());
        assert!(super::plan(&json!({"path": "C:/o.png", "width": "wide"}), None, 1).is_err());
    }

    #[test]
    fn a_huge_jpeg_is_refused() {
        let error = plan(
            &json!({"path": "C:/o.jpg", "width": 16000, "height": 16000}),
            None,
            1,
        )
        .unwrap_err();
        assert!(error.contains("too big to encode"), "{error}");
    }

    #[test]
    fn relative_and_default_paths_need_a_project() {
        assert!(plan(&json!({"path": "shots/a.png"}), None, 1).is_err());
        assert!(plan(&json!({}), None, 1).is_err());
        let plan = plan(&json!({"path": "shots/a.png"}), Some(&project()), 1).unwrap();
        assert_eq!(plan.destination, project().join("shots/a.png"));
    }

    #[test]
    fn flags_can_keep_the_editor_in() {
        let plan = plan(
            &json!({"path": "C:/o.png", "hide_grid": false, "hide_overlays": false}),
            None,
            1,
        )
        .unwrap();
        assert!(!plan.hide_grid && plan.hide_gizmos && !plan.hide_overlays);
        assert!(super::plan(&json!({"path": "C:/o.png", "hide_grid": "no"}), None, 1).is_err());
    }

    fn editor(viewport: (u32, u32)) -> EditorState {
        let state: EditorState = Arc::new(parking_lot::RwLock::new(LevelEditorState::new()));
        state.write().build.viewport_size = viewport;
        state
    }

    fn test_plan(timeout: Duration) -> ScreenshotPlan {
        ScreenshotPlan {
            destination: PathBuf::from("C:/o.png"),
            format: ScreenshotFormat::Png,
            requested: (64, 64),
            size: (64, 64),
            hide_grid: true,
            hide_gizmos: true,
            hide_overlays: true,
            timeout,
            viewport: None,
        }
    }

    #[test]
    fn no_drawn_viewport_is_an_error_not_a_wait() {
        let state = editor((0, 0));
        let started = Instant::now();
        let error = request_screenshot(&state, &test_plan(Duration::from_secs(30))).unwrap_err();
        assert!(error.contains("no viewport that has drawn"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(!state.read().build.screenshot_processing);
    }

    #[test]
    fn the_viewport_takes_the_request_and_answers_it() {
        let state = editor((1280, 720));
        let viewport = {
            let state = state.clone();
            std::thread::spawn(move || loop {
                let request = state.write().build.pending_mcp_screenshot.take();
                if let Some(request) = request {
                    assert_eq!((request.width, request.height), (64, 64));
                    assert!(request.hide_grid && request.hide_gizmos && request.hide_overlays);
                    let _ = request.reply.send(Ok(McpScreenshotOutcome {
                        path: request.destination.clone(),
                        width: request.width,
                        height: request.height,
                        format: request.format,
                        bytes: 10,
                        tiles: 1,
                        millis: 3,
                    }));
                    state.write().build.screenshot_processing = false;
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            })
        };
        let done = request_screenshot(&state, &test_plan(Duration::from_secs(30))).unwrap();
        viewport.join().unwrap();
        assert_eq!(done.bytes, 10);
        assert!(!state.read().build.screenshot_processing);
    }

    #[test]
    fn a_viewport_that_never_draws_times_out_and_clears_the_request() {
        let state = editor((1280, 720));
        let error = request_screenshot(&state, &test_plan(Duration::from_secs(1))).unwrap_err();
        assert!(error.contains("did not pick the request up"), "{error}");
        let state = state.read();
        assert!(state.build.pending_mcp_screenshot.is_none());
        assert!(!state.build.screenshot_processing);
    }

    #[test]
    fn a_second_request_while_one_runs_is_refused() {
        let state = editor((1280, 720));
        state.write().build.screenshot_processing = true;
        let error = request_screenshot(&state, &test_plan(Duration::from_secs(1))).unwrap_err();
        assert!(error.contains("already being taken"), "{error}");
    }

    #[test]
    fn the_tool_is_registered() {
        assert!(pulsar_plugins::registered::<McpToolRegistration>()
            .any(|tool| tool.name == "viewport_screenshot"));
    }
}
