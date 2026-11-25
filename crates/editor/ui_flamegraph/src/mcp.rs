//! The profiler as MCP tools: an AI agent records the running game or this
//! editor with the same recorder as the Flamegraph window's Record button
//! (`recorder`) and reads the recording, the SQLite file in
//! `<project>/.pulsar/profiling/flamegraph/`, through canned read-only queries
//! (`profile_db`).
//!
//! `profile_status`, `profile_start`, `profile_stop`, `profile_list_recordings`,
//! `profile_query_sql`, `profile_top_scopes`, `profile_frame_times`,
//! `profile_frame_breakdown`, `profile_threads`, `profile_gpu`,
//! `profile_latency`, `profile_gaps`, `profile_compare`, `profile_export`.
//!
//! Every reading tool takes `recording` (default `latest`: a file stem, a file
//! name or an absolute path) and opens it read-only.

use std::time::Duration;

use profiling::remote;
use pulsar_mcp::{Annotations, McpToolRegistration, ToolResult};
use rusqlite::Connection;
use serde_json::{json, Value};

use crate::profile_db::{self as db, TopParams};
use crate::recorder::{self, Origin};
use crate::TraceData;

fn args(arguments: &str) -> Value {
    serde_json::from_str(arguments).unwrap_or_else(|_| json!({}))
}

fn text<'a>(a: &'a Value, key: &str) -> Option<&'a str> {
    a.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

fn num(a: &Value, key: &str) -> Option<f64> {
    a.get(key).and_then(Value::as_f64)
}

fn uint(a: &Value, key: &str) -> Option<usize> {
    a.get(key).and_then(Value::as_u64).map(|n| n as usize)
}

fn project() -> Option<String> {
    engine_state::get_project_path()
}

/// Open the recording a reading tool names.
fn open(a: &Value, key: &str) -> Result<(std::path::PathBuf, Connection), String> {
    let path = db::resolve(project().as_deref(), text(a, key))?;
    let conn = db::open_read_only(&path)?;
    Ok((path, conn))
}

fn reading(arguments: &str, f: impl FnOnce(&Connection, &Value) -> Result<Value, String>) -> ToolResult {
    let a = args(arguments);
    let (path, conn) = match open(&a, "recording") {
        Ok(x) => x,
        Err(e) => return ToolResult::error(e),
    };
    match f(&conn, &a) {
        Ok(mut v) => {
            if let Value::Object(map) = &mut v {
                map.insert("recording".into(), json!(path.file_stem().and_then(|s| s.to_str())));
            }
            ToolResult::json(&v)
        }
        Err(e) => ToolResult::error(e),
    }
}

// ---- recording ------------------------------------------------------------------------

fn running_json() -> Value {
    match recorder::running() {
        Some(r) => json!({
            "id": r.id,
            "target": r.label,
            "started_from": if r.origin == Origin::Mcp { "mcp" } else { "flamegraph window" },
            "seconds": r.seconds,
            "saving": r.saving,
            "pid": r.pid,
            "writes_to": r.database.map(|p| p.display().to_string()),
        }),
        None => Value::Null,
    }
}

fn status(_: &str) -> ToolResult {
    let targets: Vec<Value> = remote::list_targets()
        .into_iter()
        .map(|t| {
            json!({
                "pid": t.pid, "kind": t.kind, "name": t.name, "project": t.project,
                "responsive": t.responsive(), "being_recorded": t.recording,
                "this_editor": t.is_current_process(),
            })
        })
        .collect();
    let project = project();
    ToolResult::json(&json!({
        "recording": running_json(),
        "targets": targets,
        "this_editor_is_recordable": true,
        "recordings_folder": project.as_deref().map(|p| format!("{p}/.pulsar/profiling/flamegraph")),
        "recordings": project.as_deref().map(|p| db::recordings(p).len()),
        "how_to": "profile_start {target: game|editor|<pid>, duration_seconds: 20} (a game is recordable while Build & Run runs it), drive it, then profile_top_scopes / profile_frame_times / profile_threads / profile_latency / profile_frame_breakdown / profile_gpu / profile_gaps, or profile_query_sql. Nothing is recorded until profile_start: an instrumented scope costs one atomic load meanwhile.",
    }))
}

fn start(arguments: &str) -> ToolResult {
    let a = args(arguments);
    let which = text(&a, "target").unwrap_or("auto").to_ascii_lowercase();
    let targets = remote::list_targets();
    let live: Vec<&remote::TargetInfo> = targets.iter().filter(|t| t.responsive() && !t.is_current_process()).collect();
    let target = match which.as_str() {
        "editor" | "this" => None,
        "auto" => live.iter().find(|t| t.kind == "game").map(|t| (*t).clone()),
        "game" => match live.iter().find(|t| t.kind == "game") {
            Some(t) => Some((*t).clone()),
            None => return ToolResult::error("no game is recordable: start one with Build & Run (it is published when it runs), or target \"editor\""),
        },
        other => match other.parse::<u32>() {
            Ok(pid) if pid == std::process::id() => None,
            Ok(pid) => match live.iter().find(|t| t.pid == pid) {
                Some(t) => Some((*t).clone()),
                None => return ToolResult::error(format!("no recordable process with pid {pid}; profile_status lists them")),
            },
            Err(_) => return ToolResult::error("target must be game, editor, auto or a pid"),
        },
    };
    let seconds = num(&a, "duration_seconds").unwrap_or(10.0).clamp(0.0, 600.0);
    let uncap = a.get("uncap_frame_rate").and_then(Value::as_bool).unwrap_or(false);
    let started = match recorder::begin(target, uncap, Origin::Mcp, std::sync::Arc::new(TraceData::new())) {
        Ok(s) => s,
        Err(e) => return ToolResult::error(e),
    };
    if seconds > 0.0 {
        let id = started.id;
        let timer = editor_task_queue::spawn_thread(
            editor_task_queue::ThreadSpec::new("profile recording timer", editor_task_queue::ThreadRole::Io)
                .owner("ui_flamegraph"),
            move || {
                std::thread::sleep(Duration::from_secs_f64(seconds));
                recorder::finish_if(id);
            },
        );
        if let Err(e) = timer {
            let _ = recorder::finish();
            return ToolResult::error(format!("could not start the recording timer: {e}"));
        }
    }
    let wait = a.get("wait").and_then(Value::as_bool).unwrap_or(false);
    if wait && seconds > 0.0 {
        let end = std::time::Instant::now() + Duration::from_secs_f64(seconds + 60.0);
        while recorder::running().is_some_and(|r| r.id == started.id) {
            if std::time::Instant::now() > end {
                return ToolResult::error("the recording did not finish in time; profile_stop it");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        return summarize(started.database.as_deref());
    }
    ToolResult::json(&json!({
        "recording_id": started.id,
        "target": started.label,
        "remote_process": started.remote,
        "duration_seconds": seconds,
        "will_be_saved_to": started.database.map(|p| p.display().to_string()),
        "next": if seconds > 0.0 { "The recording stops by itself; read it then, or profile_stop to end it early. Drive the game now." } else { "Runs until profile_stop. Drive the game now." },
    }))
}

fn summarize(path: Option<&std::path::Path>) -> ToolResult {
    let Some(path) = path else {
        return ToolResult::error("the recording was not saved: the project has no .pulsar/profiling folder (is a project open?)");
    };
    match db::open_read_only(path).and_then(|conn| {
        Ok(json!({
            "recording": path.file_stem().and_then(|s| s.to_str()),
            "path": path.display().to_string(),
            "overview": db::overview(&conn)?,
            "top_scopes": db::top_scopes(&conn, &TopParams { limit: 10, ..Default::default() })?["scopes"],
            "next": "profile_frame_times, profile_threads, profile_latency, profile_frame_breakdown, profile_gpu, profile_gaps, profile_query_sql",
        }))
    }) {
        Ok(v) => ToolResult::json(&v),
        Err(e) => ToolResult::error(e),
    }
}

fn stop(_: &str) -> ToolResult {
    match recorder::finish() {
        Ok(done) => {
            if let Some(e) = done.save_error {
                return ToolResult::error(format!("the recording ended but could not be saved: {e}"));
            }
            summarize(done.database.as_deref())
        }
        Err(e) => ToolResult::error(e),
    }
}

fn list_recordings(_: &str) -> ToolResult {
    let Some(project) = project() else {
        return ToolResult::error("no project is open");
    };
    let all: Vec<Value> = db::recordings(&project).iter().take(30).map(|p| db::describe(p)).collect();
    ToolResult::json(&json!({ "recordings": all, "newest_first": true }))
}

// ---- reading ---------------------------------------------------------------------------

fn query_sql(arguments: &str) -> ToolResult {
    reading(arguments, |conn, a| match text(a, "sql") {
        Some(sql) => db::query(conn, sql, uint(a, "limit").unwrap_or(200)),
        None => db::schema(conn),
    })
}

fn top_scopes(arguments: &str) -> ToolResult {
    reading(arguments, |conn, a| {
        let mut p = TopParams::default();
        p.by_thread = match text(a, "group") {
            None | Some("name") => false,
            Some("thread") | Some("thread_and_name") => true,
            Some(o) => return Err(format!("group must be name or thread, not {o}")),
        };
        if let Some(s) = text(a, "sort") {
            p.sort = s.to_owned();
        }
        p.limit = uint(a, "limit").unwrap_or(25);
        p.thread = text(a, "thread").map(str::to_owned);
        p.name = text(a, "name").map(str::to_owned);
        p.frame_scope = text(a, "frame_scope").map(str::to_owned);
        db::top_scopes(conn, &p)
    })
}

fn frame_times(arguments: &str) -> ToolResult {
    reading(arguments, |conn, a| {
        db::frame_times(
            conn,
            text(a, "frame_scope"),
            num(a, "hitch_threshold_ms"),
            uint(a, "series_limit").unwrap_or(300),
            uint(a, "hitch_limit").unwrap_or(10),
        )
    })
}

fn frame_breakdown(arguments: &str) -> ToolResult {
    reading(arguments, |conn, a| {
        let by_cost = match text(a, "by") {
            None | Some("period") => false,
            Some("cost") => true,
            Some(o) => return Err(format!("by must be period or cost, not {o}")),
        };
        db::frame_breakdown(
            conn,
            text(a, "frame_scope"),
            uint(a, "frame"),
            by_cost,
            uint(a, "max_depth").unwrap_or(8),
            num(a, "min_ms").unwrap_or(0.05),
        )
    })
}

fn threads(arguments: &str) -> ToolResult {
    reading(arguments, |conn, _| db::threads(conn))
}

fn gpu(arguments: &str) -> ToolResult {
    reading(arguments, |conn, _| db::gpu(conn))
}

fn latency(arguments: &str) -> ToolResult {
    reading(arguments, |conn, _| db::latency(conn))
}

fn gaps(arguments: &str) -> ToolResult {
    reading(arguments, |conn, a| db::gaps(conn, num(a, "min_self_ms").unwrap_or(0.5), uint(a, "limit").unwrap_or(25)))
}

fn compare(arguments: &str) -> ToolResult {
    let a = args(arguments);
    let (Some(_), Some(_)) = (text(&a, "a"), text(&a, "b")) else {
        return ToolResult::error("pass a and b: two recordings (profile_list_recordings), a the before and b the after");
    };
    let (ra, rb) = match (open(&a, "a"), open(&a, "b")) {
        (Ok(x), Ok(y)) => (x, y),
        (Err(e), _) | (_, Err(e)) => return ToolResult::error(e),
    };
    match db::compare(&ra.1, &rb.1, uint(&a, "limit").unwrap_or(25)) {
        Ok(mut v) => {
            v["a_recording"] = json!(ra.0.file_stem().and_then(|s| s.to_str()));
            v["b_recording"] = json!(rb.0.file_stem().and_then(|s| s.to_str()));
            ToolResult::json(&v)
        }
        Err(e) => ToolResult::error(e),
    }
}

fn export(arguments: &str) -> ToolResult {
    let a = args(arguments);
    let (path, conn) = match open(&a, "recording") {
        Ok(x) => x,
        Err(e) => return ToolResult::error(e),
    };
    match db::export_chrome_trace(&conn, &path) {
        Ok(trace) => ToolResult::json(&json!({
            "recording_sqlite": path.display().to_string(),
            "chrome_trace": trace.display().to_string(),
            "open_with": "chrome://tracing, https://ui.perfetto.dev or speedscope; the .db is the full-fidelity export (any SQLite tool)",
        })),
        Err(e) => ToolResult::error(e),
    }
}

// ---- registration ---------------------------------------------------------------------------

macro_rules! recording_prop {
    () => {
        r#""recording":{"type":"string","description":"Which recording: latest (default), a file stem from profile_list_recordings, a file name or an absolute path."}"#
    };
}

macro_rules! frame_props {
    () => {
        r#""frame_scope":{"type":"string","description":"The scope that delimits a frame. Default: Game::Frame (the packaged game's render frame), else helio_frame (the editor viewport), else the frame marker. TickLoop::tick is the simulation tick stream."}"#
    };
}

macro_rules! tool {
    ($name:literal, $title:literal, $desc:literal, $schema:expr, $ann:expr, $call:expr) => {
        pulsar_plugins::inventory::submit! {
            McpToolRegistration {
                origin: pulsar_plugins::engine_origin!(),
                name: $name,
                title: $title,
                description: $desc,
                input_schema: $schema,
                annotations: $ann,
                call: $call,
            }
        }
    };
}

tool!(
    "profile_status",
    "Profiler status",
    "Is a recording running (and from where), which processes can be recorded (the running game, this editor), and where recordings are stored. Start here.",
    r#"{"type":"object","properties":{}}"#,
    Annotations::READ_ONLY,
    status
);

tool!(
    "profile_start",
    "Start a profiler recording",
    "Start the profiler's recorder (the Flamegraph window's Record button) on target: game (the running packaged game; Build & Run publishes it), editor (this editor), auto (the game if one runs, else the editor) or a pid. It stops by itself after duration_seconds (default 10, 0 = until profile_stop) and is written to a SQLite file in <project>/.pulsar/profiling/flamegraph/. wait true blocks until it ends and returns the overview and top scopes; otherwise it returns at once and you drive the game meanwhile (walk, turn, jump). One recording at a time. A game recording keeps up to 1,000,000 events (about 20 s of a busy game).",
    r#"{"type":"object","properties":{"target":{"type":"string","description":"game, editor, auto or a pid (default auto)."},"duration_seconds":{"type":"number","description":"Default 10; 0 records until profile_stop."},"wait":{"type":"boolean","description":"Block until it ends and return a summary (default false)."},"uncap_frame_rate":{"type":"boolean","description":"Lift the target's frame cap while recording (default false: measure what the player gets)."}}}"#,
    Annotations::WRITES,
    start
);

tool!(
    "profile_stop",
    "Stop the profiler recording",
    "Stop the running recording (whoever started it), write it to the project's recordings folder and return its overview and top scopes. Errors when nothing is recording.",
    r#"{"type":"object","properties":{}}"#,
    Annotations::WRITES_IDEMPOTENT,
    stop
);

tool!(
    "profile_list_recordings",
    "List profiler recordings",
    "The project's recordings (newest first, at most 30) with size, event count, time span, frame scope and frame count.",
    r#"{"type":"object","properties":{}}"#,
    Annotations::READ_ONLY,
    list_recordings
);

tool!(
    "profile_query_sql",
    "SQL over a recording",
    "Run one read-only SQL statement against a recording's SQLite file (table profile_events, one row per finished scope: scope_id, parent_scope_id, name, thread_id, thread_name, start_ns, duration_ns, depth, location, metadata, track_name; table session_metadata). Without sql it returns the schema, what every column means and example queries. At most limit rows (default 200, 5000 max). Writes are refused by the connection itself.",
    concat!(r#"{"type":"object","properties":{"sql":{"type":"string","description":"One SELECT (or WITH ... SELECT); omit for the schema."},"limit":{"type":"integer","description":"Row limit (default 200)."},"#, recording_prop!(), r#"}}"#),
    Annotations::READ_ONLY,
    query_sql
);

tool!(
    "profile_top_scopes",
    "Top scopes of a recording",
    "The scopes that cost the most: per scope its calls, calls per frame, total and self time (self = own time without children), per-frame figures, per-call mean/p50/p95/p99/max, and the 5 worst frames with what ran in them. group: name (all threads together) or thread (one row per thread and scope). sort: self (default), total, max, count, mean. Filter by thread or name substring.",
    concat!(r#"{"type":"object","properties":{"group":{"type":"string","description":"name (default) or thread."},"sort":{"type":"string","description":"self (default), total, max, count, mean."},"limit":{"type":"integer","description":"Rows (default 25, at most 500)."},"thread":{"type":"string","description":"Only threads whose name contains this (case-insensitive)."},"name":{"type":"string","description":"Only scopes whose name contains this."},"#, frame_props!(), ",", recording_prop!(), r#"}}"#),
    Annotations::READ_ONLY,
    top_scopes
);

tool!(
    "profile_frame_times",
    "Frame time histogram, series and hitches",
    "The frame period (start to next start: what the player gets) and cost (the frame scope's own time) with mean/p50/p95/p99/max, fps and 1% low, a histogram, a downsampled series (max per bucket), the hitches above a threshold (default: twice the median) with what ran in each, and the other frame-like streams (the simulation tick next to the render frame).",
    concat!(r#"{"type":"object","properties":{"hitch_threshold_ms":{"type":"number","description":"A frame longer than this is a hitch (default max(2 x median, median + 8))."},"series_limit":{"type":"integer","description":"Series points (default 300)."},"hitch_limit":{"type":"integer","description":"Hitches listed (default 10)."},"#, frame_props!(), ",", recording_prop!(), r#"}}"#),
    Annotations::READ_ONLY,
    frame_times
);

tool!(
    "profile_frame_breakdown",
    "One frame's scope tree and critical path",
    "The tree of scopes of one frame with offset, total and self time each (small ones folded into a hidden count), the critical path (the longest child at each level; wait scopes flagged), and what every other thread did during the frame (physics, input, present). frame: an index (from profile_frame_times), or omitted for the longest. by: period (default; includes time outside the frame scope) or cost.",
    concat!(r#"{"type":"object","properties":{"frame":{"type":"integer","description":"Frame index. Default: the worst."},"by":{"type":"string","description":"period (default) or cost: how the worst frame is chosen."},"max_depth":{"type":"integer","description":"Tree depth (default 8)."},"min_ms":{"type":"number","description":"Fold scopes shorter than this (default 0.05)."},"#, frame_props!(), ",", recording_prop!(), r#"}}"#),
    Annotations::READ_ONLY,
    frame_breakdown
);

tool!(
    "profile_threads",
    "Thread utilisation and waits",
    "Per thread: scopes recorded, busy time (its outermost scopes), idle time, utilisation, the own time of wait-like scopes (wait, lock, fence, present, acquire, sleep, idle), its top work scopes; and the waits ranked across threads. Compare the game (tick), render, physics, present and input threads. A thread with no scopes is absent: use threads_list for every thread.",
    concat!(r#"{"type":"object","properties":{"#, recording_prop!(), r#"}}"#),
    Annotations::READ_ONLY,
    threads
);

tool!(
    "profile_gpu",
    "GPU pass timings",
    "Per-pass GPU time from Helio's timestamp queries (mean per GPU frame, per-sample percentiles, share of the GPU frame) and the readback lag and drops. Says plainly when the recording holds no GPU events and why.",
    concat!(r#"{"type":"object","properties":{"#, recording_prop!(), r#"}}"#),
    Annotations::READ_ONLY,
    gpu
);

tool!(
    "profile_latency",
    "Input-to-present latency chain",
    "The game timestamps each followed input through the chain (OS event, tick start, systems, scripts, scene write, frame start, surface acquire, extract and upload, render and submit, present) and publishes one chain per input on the `Input latency` track while recorded. Returns the total's mean/p50/p95/p99/worst and every stage's with its share of the mean. Press keys or move the mouse during the recording. The display adds the present queue and vsync after present returns.",
    concat!(r#"{"type":"object","properties":{"#, recording_prop!(), r#"}}"#),
    Annotations::READ_ONLY,
    latency
);

tool!(
    "profile_gaps",
    "Where instrumentation is missing",
    "Scopes that are mostly their own time (60% or more): the cost is real but nothing inside is scoped, so add scopes around their steps (profiling::profile_scope!). Also the share of each frame in the frame scope's own time. Use it to decide where to add scopes before optimising.",
    concat!(r#"{"type":"object","properties":{"min_self_ms":{"type":"number","description":"Ignore scopes with less own time in total (default 0.5)."},"limit":{"type":"integer","description":"Rows (default 25)."},"#, recording_prop!(), r#"}}"#),
    Annotations::READ_ONLY,
    gaps
);

tool!(
    "profile_compare",
    "Compare two recordings",
    "Before and after: per scope, own time per frame in a and in b (delta = b - a, negative is faster), total time and calls per frame, ranked by the size of the change, with both recordings' frame periods. Recordings are named as in profile_list_recordings.",
    r#"{"type":"object","properties":{"a":{"type":"string","description":"The before recording."},"b":{"type":"string","description":"The after recording."},"limit":{"type":"integer","description":"Rows (default 25)."}},"required":["a","b"]}"#,
    Annotations::READ_ONLY,
    compare
);

tool!(
    "profile_export",
    "Export a recording as a Chrome trace",
    "Write the recording as a Chrome trace (chrome://tracing, Perfetto, speedscope) next to it in <recordings>/exports/ and return the path. The SQLite file is the full-fidelity export.",
    concat!(r#"{"type":"object","properties":{"#, recording_prop!(), r#"}}"#),
    Annotations::WRITES,
    export
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_profile_tool_has_a_json_object_schema() {
        let mut found = 0;
        for r in pulsar_plugins::inventory::iter::<McpToolRegistration> {
            if !r.name.starts_with("profile_") {
                continue;
            }
            let v: Value = serde_json::from_str(r.input_schema).unwrap_or_else(|e| panic!("{}: {e}", r.name));
            assert_eq!(v["type"], "object", "{}", r.name);
            found += 1;
        }
        assert_eq!(found, 14);
    }

    /// A reading tool names a recording, runs its query and answers JSON; a
    /// missing recording is an error that says what to do.
    #[test]
    fn a_reading_tool_reads_the_named_recording() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.db");
        {
            let conn = profiling::database::create_database(&path).unwrap();
            let e = |name: &str, id: u64, start: u64, dur: u64| profiling::ProfileEvent {
                scope_id: id,
                parent_scope_id: None,
                name: name.into(),
                thread_id: 1,
                thread_name: Some("main".into()),
                process_id: 1,
                parent_name: None,
                start_ns: start,
                duration_ns: dur,
                depth: 0,
                location: None,
                metadata: None,
                track_name: None,
            };
            profiling::database::save_events(&conn, &[e("Game::Frame", 1, 0, 10_000_000), e("Game::Frame", 2, 16_000_000, 10_000_000)]).unwrap();
        }
        let call = |tool: &str, arguments: Value| {
            let r = pulsar_plugins::inventory::iter::<McpToolRegistration>
                .into_iter()
                .find(|r| r.name == tool)
                .unwrap();
            (r.call)(&arguments.to_string())
        };
        let p = path.display().to_string();
        let r = call("profile_top_scopes", json!({ "recording": p }));
        assert!(!r.is_error);
        let v: Value = serde_json::from_str(r.structured.as_deref().unwrap()).unwrap();
        assert_eq!(v["scopes"][0]["name"], "Game::Frame");
        assert_eq!(v["overview"]["frames"], 2);
        let r = call("profile_query_sql", json!({ "recording": p, "sql": "SELECT COUNT(*) FROM profile_events" }));
        let v: Value = serde_json::from_str(r.structured.as_deref().unwrap()).unwrap();
        assert_eq!(v["rows"][0][0], 2);
        assert!(call("profile_query_sql", json!({ "recording": p, "sql": "DELETE FROM profile_events" })).is_error);
        let missing = call("profile_top_scopes", json!({ "recording": dir.path().join("nope.db").display().to_string() }));
        assert!(missing.is_error);
        assert!(call("profile_compare", json!({ "a": p })).is_error);
    }
}
