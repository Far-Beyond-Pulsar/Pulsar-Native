//! The closed visual loop: `capture_viewport` and `run_and_observe`.
//!
//! An agent that changed a scene or a script can look at the result instead
//! of guessing. `capture_viewport` writes the editor viewport to a PNG under
//! `.pulsar/agent_captures/` and hands back a *ref*; `run_and_observe` runs the
//! game through the editor's existing Play machinery for a while, then returns
//! the log (tail and extracted errors), how the run ended and a few captures.
//!
//! This module is the engine-independent half: the [`VisualBackend`] trait
//! (what the host editor provides: a screenshot, starting/stopping the game,
//! the live log), the capture store (naming, caps, clean-up) and the shaping of
//! results. The real backend lives in the editor panel; tests use a mock, so
//! nothing here needs a GPU or a running game.
//!
//! Images reach the model by ref: a result lists `captures[].ref`, and the
//! agent runtime attaches those files to the tool result for vision models
//! (see `agent_runtime::vision`) or describes them in text for the rest.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

/// Where captures live, relative to the project root.
pub const CAPTURE_DIR: &str = ".pulsar/agent_captures";
/// Capture files are named `cap_<unix ms>_<seq>.png`; the stem is the ref.
pub const CAPTURE_PREFIX: &str = "cap_";
/// Oldest captures are deleted past this many files...
pub const MAX_CAPTURES_KEPT: usize = 40;
/// ...or this many bytes in total.
pub const MAX_CAPTURE_STORE_BYTES: u64 = 64 * 1024 * 1024;
/// Frames one `run_and_observe` may capture.
pub const MAX_FRAMES: u32 = 6;
/// Longest observation window, seconds.
pub const MAX_DURATION_S: f64 = 120.0;
pub const DEFAULT_DURATION_S: f64 = 8.0;
/// Longest wait for build + launch, seconds.
pub const MAX_BUILD_WAIT_S: f64 = 900.0;
pub const DEFAULT_BUILD_WAIT_S: f64 = 300.0;
/// Log lines and characters returned as the tail.
pub const LOG_TAIL_LINES: usize = 40;
pub const LOG_TAIL_CHARS: usize = 4_000;
/// Extracted error lines returned (the count is always the full count).
pub const MAX_ERRORS: usize = 12;
/// A single log line is cut to this many characters.
pub const MAX_LINE_CHARS: usize = 300;

/// How often the orchestrator looks at the game while it waits.
const POLL: Duration = Duration::from_millis(250);
/// Time the host gets to pick up a Play request before we give up on it.
const START_GRACE: Duration = Duration::from_secs(8);
/// Time a stopping game gets to wind down.
const STOP_WAIT: Duration = Duration::from_secs(12);

// ── Backend ──────────────────────────────────────────────────────────────

/// Pixel size of a written capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureInfo {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GamePhase {
    /// Nothing is building or running.
    Stopped,
    /// Play was requested; the game is being built or loaded.
    Building,
    Running,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameState {
    pub phase: GamePhase,
    /// The last build or run failure the host knows of, if any.
    pub error: Option<String>,
    /// Exit code of the last process, when the host has one.
    pub exit_code: Option<i32>,
}

impl GameState {
    pub fn stopped() -> Self {
        Self {
            phase: GamePhase::Stopped,
            error: None,
            exit_code: None,
        }
    }

    pub fn phase(phase: GamePhase) -> Self {
        Self {
            phase,
            error: None,
            exit_code: None,
        }
    }
}

/// What the host editor provides. Every method may block; the tools run on
/// their own thread.
pub trait VisualBackend: Send + Sync {
    /// Render the current viewport into a PNG at `dest` (downscaled to a
    /// size a model can take) and report its size.
    fn capture_viewport(&self, dest: &Path) -> Result<CaptureInfo, String>;
    /// Ask the editor to Build + Run (the toolbar's Play).
    fn start_game(&self) -> Result<(), String>;
    fn game_state(&self) -> GameState;
    fn stop_game(&self);
    /// An opaque position in the live log; pair with [`Self::logs_since`].
    fn log_mark(&self) -> u64;
    /// Log lines written after `mark`, oldest first.
    fn logs_since(&self, mark: u64) -> Vec<String>;
    /// Monotonic time since an arbitrary origin.
    fn now(&self) -> Duration;
    fn sleep(&self, duration: Duration);
}

// ── Capture store ────────────────────────────────────────────────────────

/// The `.pulsar/agent_captures/` directory of a project.
#[derive(Clone, Debug)]
pub struct CaptureStore {
    dir: PathBuf,
}

static CAPTURE_SEQ: AtomicU32 = AtomicU32::new(0);

impl CaptureStore {
    pub fn new(workspace_root: &Path) -> Self {
        Self {
            dir: workspace_root.join(CAPTURE_DIR),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// A fresh ref and the path to write it to. Creates the directory.
    pub fn allocate(&self) -> Result<(String, PathBuf)> {
        std::fs::create_dir_all(&self.dir)?;
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let seq = CAPTURE_SEQ.fetch_add(1, Ordering::Relaxed);
        let id = format!("{CAPTURE_PREFIX}{millis}_{seq}");
        let path = self.dir.join(format!("{id}.png"));
        Ok((id, path))
    }

    /// The file behind `id`, if `id` is a well-formed ref and the file exists.
    pub fn path_for(&self, id: &str) -> Option<PathBuf> {
        if !is_capture_ref(id) {
            return None;
        }
        let path = self.dir.join(format!("{id}.png"));
        path.is_file().then_some(path)
    }

    /// Delete the oldest captures until at most `max_files` files and
    /// `max_bytes` bytes remain. Returns how many were deleted.
    pub fn prune(&self, max_files: usize, max_bytes: u64) -> usize {
        let Ok(read) = std::fs::read_dir(&self.dir) else {
            return 0;
        };
        let mut files: Vec<(SystemTime, String, PathBuf, u64)> = read
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let stem = name.strip_suffix(".png")?;
                if !is_capture_ref(stem) {
                    return None;
                }
                let meta = entry.metadata().ok()?;
                let modified = meta.modified().unwrap_or(UNIX_EPOCH);
                Some((modified, name, entry.path(), meta.len()))
            })
            .collect();
        // Oldest first; the name (timestamp + sequence) breaks mtime ties.
        files.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        let mut total: u64 = files.iter().map(|f| f.3).sum();
        let mut count = files.len();
        let mut removed = 0;
        for (_, _, path, len) in files {
            if count <= max_files && total <= max_bytes {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
            count -= 1;
            total = total.saturating_sub(len);
        }
        removed
    }

    pub fn prune_default(&self) -> usize {
        self.prune(MAX_CAPTURES_KEPT, MAX_CAPTURE_STORE_BYTES)
    }
}

/// A ref is `cap_<digits>_<digits>`: no separators, so it cannot name a path
/// outside the capture directory.
pub fn is_capture_ref(id: &str) -> bool {
    let Some(rest) = id.strip_prefix(CAPTURE_PREFIX) else {
        return false;
    };
    let mut parts = rest.split('_');
    let (Some(a), Some(b), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let digits = |s: &str| !s.is_empty() && s.len() <= 20 && s.bytes().all(|c| c.is_ascii_digit());
    digits(a) && digits(b)
}

/// Every capture ref mentioned in `text`, in order, without repeats. Works on
/// JSON or on a cut-off fragment of it.
pub fn scan_capture_refs(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(found) = text[from..].find(CAPTURE_PREFIX) {
        let start = from + found;
        from = start + CAPTURE_PREFIX.len();
        // The ref must start a token, not sit inside a longer word.
        if start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
            continue;
        }
        let end = text[start..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .map_or(text.len(), |n| start + n);
        let candidate = &text[start..end];
        if is_capture_ref(candidate) && !out.iter().any(|r| r == candidate) {
            out.push(candidate.to_string());
        }
    }
    out
}

// ── Log shaping ──────────────────────────────────────────────────────────

/// Remove ANSI escape sequences (tracing output is usually coloured).
pub fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

fn clip_line(line: &str) -> String {
    let line = strip_ansi(line);
    let line = line.trim_end();
    if line.chars().count() <= MAX_LINE_CHARS {
        line.to_string()
    } else {
        let cut: String = line.chars().take(MAX_LINE_CHARS).collect();
        format!("{cut}...")
    }
}

/// Does this log line report a failure?
pub fn is_error_line(line: &str) -> bool {
    let line = strip_ansi(line);
    // Level tags as the engine's tracing output prints them.
    if line.contains(" ERROR ")
        || line.contains("[ERROR]")
        || line.starts_with("ERROR")
        || line.contains(" FATAL ")
    {
        return true;
    }
    let lower = line.to_ascii_lowercase();
    // "0 errors", "error_count" and the like are not failures.
    if lower.contains("0 errors") || lower.contains("no errors") || lower.contains("error_count") {
        return false;
    }
    [
        "panicked at",
        "error[e",
        "error:",
        "fatal error",
        "unhandled exception",
        "access violation",
        "stack overflow",
        "segmentation fault",
        "failed to ",
        "could not compile",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// The failing lines of a log: de-duplicated (a repeat becomes `(xN)`),
/// clipped, at most [`MAX_ERRORS`]. The second value is the full count.
pub fn extract_errors(lines: &[String]) -> (Vec<String>, usize) {
    let mut found: Vec<(String, usize)> = Vec::new();
    let mut total = 0;
    for line in lines.iter().filter(|l| is_error_line(l)) {
        total += 1;
        let clipped = clip_line(line);
        if let Some(entry) = found.iter_mut().find(|(text, _)| *text == clipped) {
            entry.1 += 1;
        } else {
            found.push((clipped, 1));
        }
    }
    let shaped = found
        .into_iter()
        .take(MAX_ERRORS)
        .map(|(text, n)| if n > 1 { format!("{text} (x{n})") } else { text })
        .collect();
    (shaped, total)
}

/// The last [`LOG_TAIL_LINES`] lines, no more than [`LOG_TAIL_CHARS`] in
/// total (the oldest are dropped first). The flag says whether anything was
/// left out.
pub fn log_tail(lines: &[String]) -> (String, bool) {
    let kept: Vec<String> = lines
        .iter()
        .map(|l| clip_line(l))
        .filter(|l| !l.trim().is_empty())
        .collect();
    let mut truncated = kept.len() > LOG_TAIL_LINES;
    let from = kept.len().saturating_sub(LOG_TAIL_LINES);
    let mut tail: Vec<&String> = kept[from..].iter().collect();
    let mut chars: usize = tail.iter().map(|l| l.chars().count() + 1).sum();
    while chars > LOG_TAIL_CHARS && tail.len() > 1 {
        chars -= tail[0].chars().count() + 1;
        tail.remove(0);
        truncated = true;
    }
    let text = tail.iter().map(|l| l.as_str()).collect::<Vec<_>>().join("\n");
    (text, truncated)
}

// ── capture_viewport ─────────────────────────────────────────────────────

fn capture_json(id: &str, path: &Path, info: CaptureInfo, t_s: Option<f64>) -> Value {
    let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut value = json!({
        "ref": id,
        "path": path.display().to_string(),
        "width": info.width,
        "height": info.height,
        "bytes": bytes,
    });
    if let Some(t) = t_s {
        value["t_s"] = json!((t * 10.0).round() / 10.0);
    }
    value
}

/// Capture the viewport once and describe the file.
pub fn capture_viewport_result(backend: &dyn VisualBackend, store: &CaptureStore) -> Result<Value> {
    let (id, path) = store.allocate()?;
    let info = match backend.capture_viewport(&path) {
        Ok(info) => info,
        Err(err) => {
            let _ = std::fs::remove_file(&path);
            return Err(anyhow!("viewport capture failed: {err}"));
        }
    };
    if !path.is_file() {
        return Err(anyhow!("viewport capture reported success but wrote no file"));
    }
    store.prune_default();
    let capture = capture_json(&id, &path, info, None);
    Ok(json!({
        "ok": true,
        "captures": [capture],
        "note": "The image is attached for vision-capable models. Otherwise it is only a file; \
                 do not guess its contents.",
    }))
}

// ── run_and_observe ──────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObserveOptions {
    /// How long the game is observed once it runs.
    pub duration: Duration,
    /// Captures spread over that time.
    pub frames: u32,
    /// How long build + launch may take.
    pub build_timeout: Duration,
}

impl ObserveOptions {
    /// Clamp raw tool arguments to the caps.
    pub fn from_args(duration_s: Option<f64>, frames: Option<u32>, build_s: Option<f64>) -> Self {
        let secs = |v: Option<f64>, default: f64, max: f64| {
            let v = v.filter(|v| v.is_finite()).unwrap_or(default);
            Duration::from_secs_f64(v.clamp(0.0, max))
        };
        Self {
            duration: secs(duration_s, DEFAULT_DURATION_S, MAX_DURATION_S),
            frames: frames.unwrap_or(1).min(MAX_FRAMES),
            build_timeout: secs(build_s, DEFAULT_BUILD_WAIT_S, MAX_BUILD_WAIT_S),
        }
    }
}

/// How the observed run ended.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Ending {
    /// We stopped a game that was still running.
    StoppedByTool,
    /// The game was already running; left running.
    LeftRunning,
    /// The game ended on its own.
    Exited { error: Option<String>, code: Option<i32> },
}

fn secs(d: Duration) -> f64 {
    (d.as_secs_f64() * 10.0).round() / 10.0
}

/// Build + run the game, observe it and report. Never returns `Err`: every
/// failure is a field of the result, which the model must be able to read.
pub fn run_and_observe_result(
    backend: &dyn VisualBackend,
    store: &CaptureStore,
    opts: ObserveOptions,
) -> Value {
    let begin = backend.now();
    let mark = backend.log_mark();
    let initial = backend.game_state();

    // Shape a result that never got as far as running.
    let early = |status: &str, message: String, backend: &dyn VisualBackend| -> Value {
        let lines = backend.logs_since(mark);
        let (errors, error_count) = extract_errors(&lines);
        let (tail, truncated) = log_tail(&lines);
        json!({
            "ok": false,
            "status": status,
            "message": message,
            "waited_s": secs(backend.now().saturating_sub(begin)),
            "errors": errors,
            "error_count": error_count,
            "log": { "lines": lines.len(), "tail": tail, "truncated": truncated },
            "captures": [],
        })
    };

    let attached = match initial.phase {
        GamePhase::Building => {
            return early(
                "busy",
                "A build is already in progress. Wait for it to finish and call again.".into(),
                backend,
            );
        }
        GamePhase::Running => true,
        GamePhase::Stopped => false,
    };

    if !attached {
        if let Err(err) = backend.start_game() {
            return early("launch_failed", format!("Could not start the game: {err}"), backend);
        }
        // Wait for Running. The host picks the request up asynchronously, so
        // `Stopped` right after the call is not yet a failure.
        let wait_start = backend.now();
        let mut seen_active = false;
        loop {
            let state = backend.game_state();
            match state.phase {
                GamePhase::Running => break,
                GamePhase::Building => seen_active = true,
                GamePhase::Stopped => {
                    let waited = backend.now().saturating_sub(wait_start);
                    if seen_active || state.error.is_some() {
                        let message = state
                            .error
                            .unwrap_or_else(|| "The build or launch stopped before the game ran.".into());
                        return early("build_failed", message, backend);
                    }
                    if waited >= START_GRACE {
                        return early(
                            "start_timeout",
                            "The editor never started a Play session. Is a level open in the \
                             Level Editor?"
                                .into(),
                            backend,
                        );
                    }
                }
            }
            if backend.now().saturating_sub(wait_start) >= opts.build_timeout {
                return early(
                    "build_timeout",
                    format!(
                        "The game was not running after {} s (still building).",
                        secs(opts.build_timeout)
                    ),
                    backend,
                );
            }
            backend.sleep(POLL);
        }
    }
    let build_s = secs(backend.now().saturating_sub(begin));

    // Observe: frames spread evenly through the window.
    let run_start = backend.now();
    let frames = opts.frames;
    let mut captures: Vec<Value> = Vec::new();
    let mut capture_errors: Vec<String> = Vec::new();
    let mut ended: Option<GameState> = None;
    let mut targets: Vec<Duration> = (1..=frames)
        .map(|i| opts.duration.mul_f64(i as f64 / (frames + 1) as f64))
        .collect();
    targets.push(opts.duration);
    let last = targets.len() - 1;
    'window: for (index, target) in targets.into_iter().enumerate() {
        loop {
            let state = backend.game_state();
            if state.phase != GamePhase::Running {
                ended = Some(state);
                break 'window;
            }
            let elapsed = backend.now().saturating_sub(run_start);
            if elapsed >= target {
                break;
            }
            backend.sleep(POLL.min(target - elapsed));
        }
        if index == last {
            break;
        }
        match store.allocate() {
            Ok((id, path)) => match backend.capture_viewport(&path) {
                Ok(info) if path.is_file() => {
                    let t = backend.now().saturating_sub(run_start).as_secs_f64();
                    captures.push(capture_json(&id, &path, info, Some(t)));
                }
                Ok(_) => capture_errors.push("capture wrote no file".to_string()),
                Err(err) => {
                    let _ = std::fs::remove_file(&path);
                    capture_errors.push(err);
                }
            },
            Err(err) => capture_errors.push(err.to_string()),
        }
    }

    let ending = match ended {
        Some(state) => Ending::Exited {
            error: state.error,
            code: state.exit_code,
        },
        None if attached => Ending::LeftRunning,
        None => {
            backend.stop_game();
            let stop_start = backend.now();
            let mut last_state = backend.game_state();
            while last_state.phase != GamePhase::Stopped
                && backend.now().saturating_sub(stop_start) < STOP_WAIT
            {
                backend.sleep(POLL);
                last_state = backend.game_state();
            }
            Ending::StoppedByTool
        }
    };
    let ran_s = secs(backend.now().saturating_sub(run_start));
    store.prune_default();

    let lines = backend.logs_since(mark);
    let (errors, error_count) = extract_errors(&lines);
    let (tail, truncated) = log_tail(&lines);
    let crashed = match &ending {
        Ending::Exited { error, code } => error.is_some() || code.is_some_and(|c| c != 0),
        _ => false,
    };
    let (exit_state, exit_error, exit_code) = match &ending {
        Ending::StoppedByTool => ("stopped_by_tool", None, None),
        Ending::LeftRunning => ("left_running", None, None),
        Ending::Exited { error, code } if crashed => ("crashed", error.clone(), *code),
        Ending::Exited { error, code } => ("exited", error.clone(), *code),
    };
    let status = match exit_state {
        "crashed" => "crashed",
        "exited" => "exited_early",
        _ => "ran",
    };
    let mut result = json!({
        "ok": !crashed,
        "status": status,
        "build_s": build_s,
        "ran_s": ran_s,
        "exit": { "state": exit_state, "error": exit_error, "code": exit_code },
        "errors": errors,
        "error_count": error_count,
        "log": { "lines": lines.len(), "tail": tail, "truncated": truncated },
        "captures": captures,
    });
    if !capture_errors.is_empty() {
        result["capture_errors"] = json!(capture_errors);
    }
    if attached {
        result["note"] = json!("The game was already running; it was observed and left running.");
    } else if exit_state == "exited" {
        result["note"] = json!(
            "The game exited by itself before the observation window ended (no error reported)."
        );
    }
    result
}

// ── Tools ────────────────────────────────────────────────────────────────

use super::{runtime_extras, runtime_workspace_root};
use tool_registry_macros::tool;

fn backend_and_store() -> Result<(std::sync::Arc<dyn VisualBackend>, CaptureStore)> {
    let backend = runtime_extras()?.visual.ok_or_else(|| {
        anyhow!("Viewport capture and run_and_observe are unavailable: no editor is attached to this chat")
    })?;
    Ok((backend, CaptureStore::new(&runtime_workspace_root()?)))
}

/// Capture the editor viewport (the current scene as the user sees it) to a PNG and return a
/// ref to it. Vision-capable models receive the image with the result; other models get only
/// its file path and size, so do not claim to have seen it. Use this to check how a scene
/// change looks. Captures are kept in .pulsar/agent_captures/ and old ones are deleted.
#[tool(category = "pulsar")]
pub fn capture_viewport() -> Result<Value> {
    let (backend, store) = backend_and_store()?;
    capture_viewport_result(backend.as_ref(), &store)
}

/// Build and run the game through the editor's Play, watch it for a while, then stop it and
/// report: how it ended (ran, crashed, build failed), the error lines and tail of the log, and
/// screenshots. Launches a process, so the user is asked to confirm. Slow: a build can take
/// minutes; keep duration_s short.
///
/// # Arguments
/// * `duration_s` - Seconds to observe once the game runs (default 8, max 120)
/// * `frames` - Screenshots spread across that time (default 1, max 6, 0 for none)
/// * `build_timeout_s` - Seconds to allow for build + launch (default 300, max 900)
#[tool(category = "pulsar")]
pub fn run_and_observe(
    duration_s: Option<f64>,
    frames: Option<u32>,
    build_timeout_s: Option<f64>,
) -> Result<Value> {
    let (backend, store) = backend_and_store()?;
    let opts = ObserveOptions::from_args(duration_s, frames, build_timeout_s);
    Ok(run_and_observe_result(backend.as_ref(), &store, opts))
}

#[cfg(test)]
mod tests;
