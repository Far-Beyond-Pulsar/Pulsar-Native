//! The editor behind the agent's `capture_viewport` and `run_and_observe`.
//!
//! Nothing here is new machinery: a capture is the viewport's existing
//! framebuffer readback (the one that writes the project thumbnail), and a run
//! is the toolbar's Play, sent as a `PlaybackCommand` and followed through the
//! engine-wide `PlaybackState`. The orchestration and result shaping live in
//! `agent_chat_tools::visual`, where a mock backend tests them.

use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use agent_chat_tools::visual::{CaptureInfo, GamePhase, GameState, VisualBackend};
use engine_state::playback::{playback, PlayPhase};
use pulsar_events::{publish_playback_command, PlaybackCommand};

/// Captures are scaled down to this width before a model sees them: a 4K
/// viewport costs many times the tokens and adds nothing for judging a scene.
const MAX_CAPTURE_WIDTH: u32 = 1280;
/// The viewport renders on the UI thread; give it this long to answer.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) struct EditorVisualBackend {
    origin: Instant,
}

pub(super) fn editor_backend() -> Arc<dyn VisualBackend> {
    Arc::new(EditorVisualBackend {
        origin: Instant::now(),
    })
}

/// Read the PNG the viewport wrote, scale it to [`MAX_CAPTURE_WIDTH`] if wider
/// and write it back. `None` while the file is missing or still being written.
fn finish_capture(path: &Path) -> Option<CaptureInfo> {
    let image = image::open(path).ok()?;
    let (width, height) = (image.width(), image.height());
    if width <= MAX_CAPTURE_WIDTH {
        return Some(CaptureInfo { width, height });
    }
    let scaled_height = (u64::from(height) * u64::from(MAX_CAPTURE_WIDTH) / u64::from(width)).max(1) as u32;
    let scaled = image.resize_exact(
        MAX_CAPTURE_WIDTH,
        scaled_height,
        image::imageops::FilterType::Triangle,
    );
    scaled.save(path).ok()?;
    Some(CaptureInfo {
        width: MAX_CAPTURE_WIDTH,
        height: scaled_height,
    })
}

impl VisualBackend for EditorVisualBackend {
    fn capture_viewport(&self, dest: &Path) -> Result<CaptureInfo, String> {
        let editor = ui_level_editor::ai_sessions::find_editor(None)?;
        ui_level_editor::request_viewport_capture(&editor, dest.to_path_buf());
        let started = Instant::now();
        while started.elapsed() < CAPTURE_TIMEOUT {
            std::thread::sleep(Duration::from_millis(100));
            if dest.is_file() {
                if let Some(info) = finish_capture(dest) {
                    return Ok(info);
                }
            }
        }
        // Withdraw the request so a late render does not write a stray file.
        {
            let mut state = editor.write();
            if state.build.pending_thumbnail_capture.as_deref() == Some(dest) {
                state.build.pending_thumbnail_capture = None;
            }
        }
        Err("the viewport did not render a frame in time (is the Level Editor visible?)".into())
    }

    fn start_game(&self) -> Result<(), String> {
        let editor = ui_level_editor::ai_sessions::find_editor(None)?;
        // A failure from an earlier run must not read as this run's.
        editor.write().play.pie.last_error = None;
        publish_playback_command(PlaybackCommand::Play);
        Ok(())
    }

    fn game_state(&self) -> GameState {
        let state = playback().get();
        let phase = match state.phase {
            PlayPhase::Playing => GamePhase::Running,
            PlayPhase::Building => GamePhase::Building,
            PlayPhase::Stopped if state.game_running => GamePhase::Running,
            PlayPhase::Stopped if state.build_running => GamePhase::Building,
            PlayPhase::Stopped => GamePhase::Stopped,
        };
        let error = ui_level_editor::ai_sessions::find_editor(None)
            .ok()
            .and_then(|editor| editor.read().play.pie.last_error.clone());
        GameState {
            phase,
            error,
            exit_code: None,
        }
    }

    fn stop_game(&self) {
        publish_playback_command(PlaybackCommand::Stop);
    }

    fn log_mark(&self) -> u64 {
        ui_log_viewer::live_log_mark()
    }

    fn logs_since(&self, mark: u64) -> Vec<String> {
        ui_log_viewer::live_logs_since(mark)
    }

    fn now(&self) -> Duration {
        self.origin.elapsed()
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}
