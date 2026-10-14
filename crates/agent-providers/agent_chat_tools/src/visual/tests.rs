use std::sync::Mutex;

use super::*;

/// A scripted editor: virtual time, a game that follows a timeline.
struct Mock {
    clock: Mutex<Duration>,
    /// Virtual time at which `start_game` was called.
    started_at: Mutex<Option<Duration>>,
    start_error: Option<String>,
    /// If set, Play is never picked up.
    ignore_start: bool,
    /// Build takes this long before the game runs.
    build_time: Duration,
    /// If set, the build fails at this offset from start.
    build_fails_after: Option<(Duration, String)>,
    /// If set, the running game exits this long after it began running.
    exits_after: Option<(Duration, Option<String>, Option<i32>)>,
    initially_running: bool,
    stopped: Mutex<bool>,
    log: Vec<String>,
    capture_fails: bool,
    captures_taken: Mutex<u32>,
}

impl Mock {
    fn new() -> Self {
        Self {
            clock: Mutex::new(Duration::ZERO),
            started_at: Mutex::new(None),
            start_error: None,
            ignore_start: false,
            build_time: Duration::from_secs(2),
            build_fails_after: None,
            exits_after: None,
            initially_running: false,
            stopped: Mutex::new(false),
            log: vec![],
            capture_fails: false,
            captures_taken: Mutex::new(0),
        }
    }
}

impl VisualBackend for Mock {
    fn capture_viewport(&self, dest: &Path) -> Result<CaptureInfo, String> {
        if self.capture_fails {
            return Err("no gpu".into());
        }
        *self.captures_taken.lock().unwrap() += 1;
        std::fs::write(dest, b"\x89PNG fake").map_err(|e| e.to_string())?;
        Ok(CaptureInfo {
            width: 640,
            height: 360,
        })
    }

    fn start_game(&self) -> Result<(), String> {
        if let Some(err) = &self.start_error {
            return Err(err.clone());
        }
        if !self.ignore_start {
            *self.started_at.lock().unwrap() = Some(*self.clock.lock().unwrap());
        }
        Ok(())
    }

    fn game_state(&self) -> GameState {
        let now = *self.clock.lock().unwrap();
        if self.initially_running && self.started_at.lock().unwrap().is_none() {
            return GameState::phase(GamePhase::Running);
        }
        let Some(start) = *self.started_at.lock().unwrap() else {
            return GameState::stopped();
        };
        if *self.stopped.lock().unwrap() {
            return GameState::stopped();
        }
        let since = now.saturating_sub(start);
        if let Some((at, message)) = &self.build_fails_after {
            if since >= *at {
                return GameState {
                    phase: GamePhase::Stopped,
                    error: Some(message.clone()),
                    exit_code: None,
                };
            }
        }
        if since < self.build_time {
            return GameState::phase(GamePhase::Building);
        }
        if let Some((after, error, code)) = &self.exits_after {
            if since >= self.build_time + *after {
                return GameState {
                    phase: GamePhase::Stopped,
                    error: error.clone(),
                    exit_code: *code,
                };
            }
        }
        GameState::phase(GamePhase::Running)
    }

    fn stop_game(&self) {
        *self.stopped.lock().unwrap() = true;
    }

    fn log_mark(&self) -> u64 {
        0
    }

    fn logs_since(&self, _mark: u64) -> Vec<String> {
        self.log.clone()
    }

    fn now(&self) -> Duration {
        *self.clock.lock().unwrap()
    }

    fn sleep(&self, duration: Duration) {
        *self.clock.lock().unwrap() += duration;
    }
}

fn opts(duration_s: u64, frames: u32) -> ObserveOptions {
    ObserveOptions {
        duration: Duration::from_secs(duration_s),
        frames,
        build_timeout: Duration::from_secs(60),
    }
}

fn store() -> (tempfile::TempDir, CaptureStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = CaptureStore::new(dir.path());
    (dir, store)
}

// ── log shaping ──────────────────────────────────────────────────────────

fn lines(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn error_lines_are_recognised() {
    for line in [
        "2026-01-01T00:00:00Z ERROR pulsar: shader failed",
        "thread 'main' panicked at src/main.rs:4:5:",
        "error[E0425]: cannot find value `x`",
        "\u{1b}[31mERROR\u{1b}[0m something",
        "Failed to load mesh cube.glb",
        "Access violation reading location 0x0",
    ] {
        assert!(is_error_line(line), "{line}");
    }
    for line in [
        "INFO loaded 12 meshes",
        "build finished with 0 errors",
        "error_count = 0",
        "WARN texture is large",
    ] {
        assert!(!is_error_line(line), "{line}");
    }
}

#[test]
fn errors_are_deduplicated_clipped_and_capped() {
    let mut input: Vec<String> = vec!["ERROR boom".into(); 5];
    input.push(format!("ERROR {}", "x".repeat(1_000)));
    for n in 0..30 {
        input.push(format!("ERROR distinct {n}"));
    }
    input.push("INFO fine".into());
    let (errors, total) = extract_errors(&input);
    assert_eq!(total, 36);
    assert_eq!(errors.len(), MAX_ERRORS);
    assert_eq!(errors[0], "ERROR boom (x5)");
    assert!(errors[1].chars().count() <= MAX_LINE_CHARS + 3);
    assert!(errors[1].ends_with("..."));
}

#[test]
fn log_tail_caps_lines_and_characters() {
    let many: Vec<String> = (0..500).map(|n| format!("line {n}")).collect();
    let (tail, truncated) = log_tail(&many);
    assert!(truncated);
    assert_eq!(tail.lines().count(), LOG_TAIL_LINES);
    assert!(tail.ends_with("line 499"));

    let wide: Vec<String> = (0..40).map(|_| "y".repeat(MAX_LINE_CHARS)).collect();
    let (tail, truncated) = log_tail(&wide);
    assert!(truncated);
    assert!(tail.chars().count() <= LOG_TAIL_CHARS);

    let (tail, truncated) = log_tail(&lines(&["a", "", "b"]));
    assert_eq!(tail, "a\nb");
    assert!(!truncated);
}

#[test]
fn ansi_is_stripped_from_the_tail() {
    let (tail, _) = log_tail(&lines(&["\u{1b}[32m INFO\u{1b}[0m ready"]));
    assert_eq!(tail, " INFO ready");
}

// ── refs and the store ───────────────────────────────────────────────────

#[test]
fn refs_are_validated_and_scanned() {
    assert!(is_capture_ref("cap_1_2"));
    for bad in ["cap_1", "cap_1_2_3", "cap__2", "cap_a_b", "../cap_1_2", "cap_1_2.png", "xcap_1_2"] {
        assert!(!is_capture_ref(bad), "{bad}");
    }
    let text = r#"{"captures":[{"ref":"cap_100_1","path":"C:\\p\\.pulsar\\agent_captures\\cap_100_1.png"},{"ref":"cap_100_2"}]} recap_9_9 cap_x_y"#;
    assert_eq!(scan_capture_refs(text), ["cap_100_1", "cap_100_2"]);
    // A cut-off fragment still yields the complete refs in it.
    assert_eq!(scan_capture_refs(r#"{"ref":"cap_5_6","ref":"cap_7"#), ["cap_5_6"]);
}

#[test]
fn path_for_refuses_missing_and_malformed_refs() {
    let (_dir, store) = store();
    let (id, path) = store.allocate().unwrap();
    assert!(store.path_for(&id).is_none());
    std::fs::write(&path, b"x").unwrap();
    assert_eq!(store.path_for(&id).as_deref(), Some(path.as_path()));
    assert!(store.path_for("../../etc/passwd").is_none());
}

#[test]
fn prune_removes_the_oldest_by_count_and_by_size() {
    let (_dir, store) = store();
    let mut ids = Vec::new();
    for _ in 0..6 {
        let (id, path) = store.allocate().unwrap();
        std::fs::write(&path, vec![0u8; 100]).unwrap();
        // Distinct mtimes without sleeping: set explicitly, oldest first.
        let age = Duration::from_secs(600 - 100 * ids.len() as u64);
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::now() - age).unwrap();
        ids.push(id);
    }
    // A foreign file is never touched.
    std::fs::write(store.dir().join("notes.txt"), b"keep").unwrap();

    assert_eq!(store.prune(4, u64::MAX), 2);
    assert!(store.path_for(&ids[0]).is_none());
    assert!(store.path_for(&ids[1]).is_none());
    assert!(store.path_for(&ids[2]).is_some());

    // 4 files * 100 bytes; allow 250 -> two more go.
    assert_eq!(store.prune(100, 250), 2);
    assert!(store.path_for(&ids[5]).is_some());
    assert!(store.path_for(&ids[3]).is_none());
    assert!(store.dir().join("notes.txt").exists());
}

// ── capture_viewport ─────────────────────────────────────────────────────

#[test]
fn capture_viewport_returns_a_ref_and_a_file() {
    let (_dir, store) = store();
    let mock = Mock::new();
    let result = capture_viewport_result(&mock, &store).unwrap();
    assert_eq!(result["ok"], true);
    let capture = &result["captures"][0];
    let id = capture["ref"].as_str().unwrap();
    assert!(is_capture_ref(id));
    assert_eq!(capture["width"], 640);
    assert!(store.path_for(id).is_some());
    // The path the model sees is the real file.
    assert!(Path::new(capture["path"].as_str().unwrap()).is_file());
}

#[test]
fn capture_viewport_failure_leaves_no_file_and_is_an_error() {
    let (_dir, store) = store();
    let mut mock = Mock::new();
    mock.capture_fails = true;
    let err = capture_viewport_result(&mock, &store).unwrap_err().to_string();
    assert!(err.contains("no gpu"), "{err}");
    let leftovers = std::fs::read_dir(store.dir()).unwrap().count();
    assert_eq!(leftovers, 0);
}

// ── run_and_observe ──────────────────────────────────────────────────────

#[test]
fn a_clean_run_reports_captures_log_and_stops_the_game() {
    let (_dir, store) = store();
    let mut mock = Mock::new();
    mock.log = lines(&["INFO boot", "ERROR missing texture grass.png", "INFO tick"]);
    let result = run_and_observe_result(&mock, &store, opts(8, 3));

    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["status"], "ran");
    assert_eq!(result["exit"]["state"], "stopped_by_tool");
    assert_eq!(result["captures"].as_array().unwrap().len(), 3);
    assert_eq!(result["error_count"], 1);
    assert_eq!(result["errors"][0], "ERROR missing texture grass.png");
    assert_eq!(result["log"]["lines"], 3);
    assert!(result["log"]["tail"].as_str().unwrap().contains("INFO tick"));
    assert!(*mock.stopped.lock().unwrap());
    // Frames are spread across the window: 2s, 4s, 6s into an 8s run.
    let times: Vec<f64> = result["captures"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["t_s"].as_f64().unwrap())
        .collect();
    assert_eq!(times, [2.0, 4.0, 6.0]);
    assert_eq!(result["build_s"], 2.0);
}

#[test]
fn frames_zero_takes_no_captures_and_frames_are_capped() {
    let (_dir, store) = store();
    let mock = Mock::new();
    let result = run_and_observe_result(&mock, &store, opts(4, 0));
    assert!(result["captures"].as_array().unwrap().is_empty());

    let o = ObserveOptions::from_args(Some(1e9), Some(99), Some(f64::NAN));
    assert_eq!(o.frames, MAX_FRAMES);
    assert_eq!(o.duration, Duration::from_secs_f64(MAX_DURATION_S));
    assert_eq!(o.build_timeout, Duration::from_secs_f64(DEFAULT_BUILD_WAIT_S));
    let o = ObserveOptions::from_args(Some(-5.0), None, None);
    assert_eq!(o.duration, Duration::ZERO);
    assert_eq!(o.frames, 1);
}

#[test]
fn a_crash_is_reported_with_the_error_and_code() {
    let (_dir, store) = store();
    let mut mock = Mock::new();
    mock.exits_after = Some((Duration::from_secs(3), Some("exit status 101".into()), Some(101)));
    mock.log = lines(&["thread 'main' panicked at game.rs:1:1:"]);
    let result = run_and_observe_result(&mock, &store, opts(10, 2));

    assert_eq!(result["ok"], false);
    assert_eq!(result["status"], "crashed");
    assert_eq!(result["exit"]["state"], "crashed");
    assert_eq!(result["exit"]["code"], 101);
    assert_eq!(result["error_count"], 1);
    // The first frame (at 10/3 s) is after the crash at 3 s: none captured.
    assert!(result["captures"].as_array().unwrap().is_empty());
    assert!(!*mock.stopped.lock().unwrap(), "nothing left to stop");
}

#[test]
fn a_silent_early_exit_is_flagged_but_not_a_crash() {
    let (_dir, store) = store();
    let mut mock = Mock::new();
    mock.exits_after = Some((Duration::from_secs(1), None, Some(0)));
    let result = run_and_observe_result(&mock, &store, opts(10, 1));
    assert_eq!(result["ok"], true);
    assert_eq!(result["status"], "exited_early");
    assert!(result["note"].as_str().unwrap().contains("exited by itself"));
}

#[test]
fn a_failed_build_is_reported_with_the_compiler_output() {
    let (_dir, store) = store();
    let mut mock = Mock::new();
    mock.build_time = Duration::from_secs(30);
    mock.build_fails_after = Some((Duration::from_secs(5), "error[E0432]: unresolved import".into()));
    mock.log = lines(&["error[E0432]: unresolved import `foo`"]);
    let result = run_and_observe_result(&mock, &store, opts(8, 1));
    assert_eq!(result["ok"], false);
    assert_eq!(result["status"], "build_failed");
    assert!(result["message"].as_str().unwrap().contains("E0432"));
    assert_eq!(result["error_count"], 1);
}

#[test]
fn an_ignored_play_request_times_out_with_a_hint() {
    let (_dir, store) = store();
    let mut mock = Mock::new();
    mock.ignore_start = true;
    let result = run_and_observe_result(&mock, &store, opts(8, 1));
    assert_eq!(result["status"], "start_timeout");
    assert!(result["message"].as_str().unwrap().contains("Level Editor"));
}

#[test]
fn a_slow_build_hits_the_build_timeout() {
    let (_dir, store) = store();
    let mut mock = Mock::new();
    mock.build_time = Duration::from_secs(10_000);
    let mut o = opts(8, 1);
    o.build_timeout = Duration::from_secs(20);
    let result = run_and_observe_result(&mock, &store, o);
    assert_eq!(result["status"], "build_timeout");
}

#[test]
fn launch_errors_are_results_not_failures() {
    let (_dir, store) = store();
    let mut mock = Mock::new();
    mock.start_error = Some("no project open".into());
    let result = run_and_observe_result(&mock, &store, opts(8, 1));
    assert_eq!(result["status"], "launch_failed");
    assert!(result["message"].as_str().unwrap().contains("no project open"));
}

#[test]
fn an_already_running_game_is_observed_and_left_running() {
    let (_dir, store) = store();
    let mut mock = Mock::new();
    mock.initially_running = true;
    let result = run_and_observe_result(&mock, &store, opts(4, 1));
    assert_eq!(result["status"], "ran");
    assert_eq!(result["exit"]["state"], "left_running");
    assert!(!*mock.stopped.lock().unwrap());
    assert!(mock.started_at.lock().unwrap().is_none(), "must not restart it");
}

#[test]
fn capture_failures_during_a_run_are_listed_not_fatal() {
    let (_dir, store) = store();
    let mut mock = Mock::new();
    mock.capture_fails = true;
    let result = run_and_observe_result(&mock, &store, opts(4, 2));
    assert_eq!(result["ok"], true);
    assert_eq!(result["captures"].as_array().unwrap().len(), 0);
    assert_eq!(result["capture_errors"].as_array().unwrap().len(), 2);
}

#[test]
fn the_result_stays_compact_for_a_noisy_game() {
    let (_dir, store) = store();
    let mut mock = Mock::new();
    mock.log = (0..20_000).map(|n| format!("ERROR frame {n} dropped because {}", "z".repeat(400))).collect();
    let result = run_and_observe_result(&mock, &store, opts(4, 6));
    let size = result.to_string().len();
    assert!(size < 12_000, "result is {size} bytes");
    assert_eq!(result["error_count"], 20_000);
    assert_eq!(result["errors"].as_array().unwrap().len(), MAX_ERRORS);
    assert_eq!(result["captures"].as_array().unwrap().len(), 6);
}

#[test]
fn captures_are_pruned_after_a_run() {
    let (_dir, store) = store();
    for _ in 0..(MAX_CAPTURES_KEPT + 10) {
        let (_, path) = store.allocate().unwrap();
        std::fs::write(path, b"old").unwrap();
    }
    let mock = Mock::new();
    run_and_observe_result(&mock, &store, opts(4, 2));
    let count = std::fs::read_dir(store.dir()).unwrap().count();
    assert!(count <= MAX_CAPTURES_KEPT, "{count} files kept");
}
