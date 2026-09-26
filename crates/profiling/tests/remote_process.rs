//! A real second process publishes instrumentation events and this one
//! discovers it and pulls them through shared memory.
//!
//! The test binary re-runs itself as the target: `target_process` does
//! nothing unless `PROFILING_TEST_TARGET_DIR` is set.

use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use profiling::remote::{self, TargetConnection, TargetDescription};

const TARGET_DIR_ENV: &str = "PROFILING_TEST_TARGET_DIR";

/// The profiled process: publish, then run instrumented "frames" until
/// killed (at most 60 s).
#[test]
fn target_process() {
    let Some(dir) = std::env::var_os(TARGET_DIR_ENV) else { return };
    let description = TargetDescription { kind: "game".into(), name: "remote_test_game".into(), project: "/projects/test".into() };
    let _publisher = remote::serve_in(&PathBuf::from(dir), description, 1 << 20).unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        profiling::profile_scope!("frame");
        {
            profiling::profile_scope!("update");
            std::hint::black_box((0..1000u64).sum::<u64>());
        }
        profiling::record_frame_time(1.0);
        std::thread::sleep(Duration::from_millis(1));
    }
}

struct Target(Child);

impl Drop for Target {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_viewer_discovers_a_process_and_streams_its_scopes() {
    if std::env::var_os(TARGET_DIR_ENV).is_some() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("pulsar-profiler-it-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "target_process", "--nocapture"])
        .env(TARGET_DIR_ENV, &dir)
        .spawn()
        .unwrap();
    let child_pid = child.id();
    let mut target = Target(child);

    // Discovery: the process appears with its description.
    let mut found = None;
    wait_for("the target to be listed", || {
        found = remote::list_targets_in(&dir).into_iter().find(|t| t.pid == child_pid);
        found.is_some()
    });
    let info = found.unwrap();
    assert_eq!((info.kind.as_str(), info.name.as_str(), info.project.as_str()), ("game", "remote_test_game", "/projects/test"));
    assert!(info.responsive() && !info.recording && info.viewer_pid.is_none());

    // Nothing streams before a viewer asks.
    let mut connection = TargetConnection::open(&info.path).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let mut events = Vec::new();
    assert_eq!(connection.read_events(&mut events), 0, "idle targets publish nothing");

    // Record: the target acknowledges and streams its scopes.
    connection.start_recording(false).unwrap();
    wait_for("the target to start streaming", || connection.is_streaming());
    wait_for("frames and nested scopes", || {
        connection.read_events(&mut events);
        events.iter().filter(|e| e.name == "update").count() >= 50
    });
    let listed = remote::list_targets_in(&dir).into_iter().find(|t| t.pid == child_pid).unwrap();
    assert!(listed.recording && listed.viewer_pid == Some(std::process::id()));

    let update = events.iter().find(|e| e.name == "update").unwrap();
    let frame = events.iter().find(|e| e.scope_id == update.parent_scope_id.unwrap()).expect("parent frame streamed");
    assert_eq!((frame.name.as_str(), frame.depth, update.depth), ("frame", 0, 1));
    assert_eq!(update.parent_name.as_deref(), Some("frame"));
    assert_eq!(update.process_id, child_pid);
    assert!(frame.start_ns <= update.start_ns && update.duration_ns > 0 && frame.duration_ns >= update.duration_ns);
    assert!(events.iter().any(|e| e.name == "__FRAME_MARKER__"), "frame markers stream too");

    // A second viewer cannot take a target that is being recorded.
    let mut other = TargetConnection::open(&info.path).unwrap();
    assert!(other.start_recording(false).is_err());
    drop(other);

    // Stop: the target stops streaming and the ring goes quiet.
    connection.stop_recording();
    wait_for("the target to stop streaming", || !connection.is_streaming());
    std::thread::sleep(Duration::from_millis(50));
    connection.read_events(&mut events);
    let settled = events.len();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(connection.read_events(&mut events), 0, "nothing after stop ({settled} events recorded)");
    assert_eq!(connection.dropped(), 0);

    // A killed target is cleaned up from the listing.
    drop(connection);
    drop(target.0.kill());
    let _ = target.0.wait();
    wait_for("the dead target to be removed", || {
        remote::list_targets_in(&dir).iter().all(|t| t.pid != child_pid)
    });
    let _ = std::fs::remove_dir_all(&dir);
}
