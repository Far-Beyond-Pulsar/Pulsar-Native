//! The one recorder: what the Flamegraph window's Record button and the
//! `profile_start` / `profile_stop` MCP tools both drive.
//!
//! A recording is an [`InstrumentationCollector`] (this editor's own profiler
//! queue, or another process through its shared-memory ring: a game launched by
//! Build & Run, another editor) plus a SQLite file in the project's
//! `.pulsar/profiling/flamegraph/` that receives every event when the recording
//! stops (`profiling::database`). One recording runs at a time per editor
//! (a target has one viewer, and the in-process profiler is one queue), so the
//! state is one slot: whoever started it, the other can see and stop it.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;
use profiling::remote::{TargetConnection, TargetInfo};

use crate::{InstrumentationCollector, TraceData};

/// Who started the recording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Window,
    Mcp,
}

struct Active {
    id: u64,
    collector: Arc<InstrumentationCollector>,
    database: Option<(PathBuf, rusqlite::Connection)>,
    label: String,
    origin: Origin,
    started: Instant,
    pid: Option<u32>,
}

static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);
static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// What a recording that just started is.
#[derive(Clone, Debug)]
pub struct Started {
    /// Identifies this recording (for [`finish_if`]).
    pub id: u64,
    pub label: String,
    /// The file the events are written to when it stops, if the project has one.
    pub database: Option<PathBuf>,
    pub remote: bool,
}

/// What a recording that just stopped left behind.
#[derive(Clone, Debug)]
pub struct Finished {
    pub label: String,
    pub database: Option<PathBuf>,
    pub events: usize,
    pub seconds: f64,
    /// Writing the events failed (the recording is lost).
    pub save_error: Option<String>,
}

/// The running recording, for `profile_status` and the window.
#[derive(Clone, Debug)]
pub struct Running {
    pub id: u64,
    pub label: String,
    pub origin: Origin,
    pub seconds: f64,
    pub database: Option<PathBuf>,
    pub remote: bool,
    pub pid: Option<u32>,
}

pub fn running() -> Option<Running> {
    ACTIVE.lock().as_ref().map(|a| Running {
        id: a.id,
        label: a.label.clone(),
        origin: a.origin,
        seconds: a.started.elapsed().as_secs_f64(),
        database: a.database.as_ref().map(|(p, _)| p.clone()),
        remote: a.collector.is_remote(),
        pid: a.pid,
    })
}

/// Start recording `target` (`None`: this editor, in-process). Fails when a
/// recording is already running or the target cannot be claimed.
pub fn begin(
    target: Option<TargetInfo>,
    uncap_frame_rate: bool,
    origin: Origin,
    trace_data: Arc<TraceData>,
) -> Result<Started, String> {
    let mut slot = ACTIVE.lock();
    if let Some(active) = slot.as_ref() {
        return Err(format!(
            "a recording of {} is already running (started from {}, {:.0} s ago); stop it first",
            active.label,
            if active.origin == Origin::Mcp { "MCP" } else { "the Flamegraph window" },
            active.started.elapsed().as_secs_f64()
        ));
    }
    tracing::trace!("[PROFILER] Starting instrumentation collector");
    let (collector, label, pid) = match &target {
        None => (
            InstrumentationCollector::new(trace_data, 100),
            "this editor".to_owned(),
            None,
        ),
        Some(target) => {
            let connection = TargetConnection::open(&target.path)
                .map_err(|e| format!("cannot open {}: {e}", target.path.display()))?;
            (
                InstrumentationCollector::remote(trace_data, 100, connection, uncap_frame_rate),
                format!("{} (pid {})", target.name, target.pid),
                Some(target.pid),
            )
        }
    };
    collector.start()?;

    let mut database = None;
    if let Some(project) = engine_state::get_project_path() {
        match profiling::database::ensure_profiling_dir(&project) {
            Ok(dir) => {
                let path = dir.join(profiling::database::generate_db_filename());
                match profiling::database::create_database(&path) {
                    Ok(conn) => {
                        tracing::trace!("[PROFILER] Created database: {}", path.display());
                        database = Some((path, conn));
                    }
                    Err(e) => tracing::error!("[PROFILER] Failed to create database: {e}"),
                }
            }
            Err(e) => tracing::error!("[PROFILER] Failed to create profiling directory: {e}"),
        }
    }
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let started = Started {
        id,
        label: label.clone(),
        database: database.as_ref().map(|(p, _)| p.clone()),
        remote: collector.is_remote(),
    };
    *slot = Some(Active {
        id,
        collector: Arc::new(collector),
        database,
        label,
        origin,
        started: Instant::now(),
        pid,
    });
    Ok(started)
}

/// [`finish`] the recording `id` if it is still the running one (a timer
/// that outlived a manual stop must not stop the next recording).
pub fn finish_if(id: u64) -> Option<Finished> {
    if ACTIVE.lock().as_ref().is_none_or(|a| a.id != id) {
        return None;
    }
    finish().ok()
}

/// Stop the running recording and write its events to the project's database.
pub fn finish() -> Result<Finished, String> {
    let active = ACTIVE
        .lock()
        .take()
        .ok_or_else(|| "nothing is recording".to_owned())?;
    active.collector.stop();
    // Restore the normal frame-rate cap before anything slow (the save below).
    profiling::set_uncap_frame_rate(false);
    gpui::render_stats::set_uncapped_presentation(false);

    let seconds = active.started.elapsed().as_secs_f64();
    let mut events = 0;
    let mut save_error = None;
    let mut path = None;
    if let Some((db_path, conn)) = &active.database {
        let recorded = if active.collector.is_remote() {
            active.collector.session_events()
        } else {
            profiling::get_all_events()
        };
        events = recorded.len();
        match profiling::database::save_events(conn, &recorded) {
            Ok(()) => {
                tracing::trace!("[PROFILER] Saved {events} events to {}", db_path.display());
                path = Some(db_path.clone());
            }
            Err(e) => {
                tracing::error!("[PROFILER] Failed to save events to database: {e}");
                save_error = Some(e.to_string());
            }
        }
    }
    tracing::trace!("[PROFILER] Instrumentation profiling stopped");
    Ok(Finished {
        label: active.label.clone(),
        database: path,
        events,
        seconds,
        save_error,
    })
}
