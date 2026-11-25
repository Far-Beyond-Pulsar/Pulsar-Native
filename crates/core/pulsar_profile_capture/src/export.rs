//! A capture written out: the raw events as JSON, and a Chrome trace
//! (`chrome://tracing`, Perfetto, speedscope) of the scopes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::buffer::Capture;
use crate::index::is_marker;

/// The capture as a Chrome trace-event document (complete events, microseconds
/// from the first event).
pub fn chrome_trace(capture: &Capture) -> Value {
    let origin = capture.events.iter().map(|e| e.start_ns).min().unwrap_or(0);
    let mut threads: BTreeMap<u64, String> = BTreeMap::new();
    let mut events = Vec::with_capacity(capture.events.len() + 8);
    for e in &capture.events {
        let name = e
            .thread_name
            .clone()
            .or_else(|| e.track_name.clone())
            .unwrap_or_else(|| format!("thread-{}", e.thread_id));
        threads.entry(e.thread_id).or_insert(name);
        let mut args = serde_json::Map::new();
        if let Some(l) = &e.location {
            args.insert("location".into(), json!(l));
        }
        if let Some(m) = &e.metadata {
            args.insert("metadata".into(), json!(m));
        }
        events.push(json!({
            "name": e.name,
            "cat": if is_marker(e) { "marker" } else { "scope" },
            "ph": "X",
            "ts": (e.start_ns - origin) as f64 / 1000.0,
            "dur": e.duration_ns as f64 / 1000.0,
            "pid": e.process_id,
            "tid": e.thread_id,
            "args": args,
        }));
    }
    for (tid, name) in threads {
        events.push(json!({
            "name": "thread_name", "ph": "M", "pid": capture.target.pid, "tid": tid,
            "args": { "name": name },
        }));
    }
    json!({ "traceEvents": events, "displayTimeUnit": "ms" })
}

/// The capture as plain JSON: the target, the counters, every event.
pub fn raw_json(capture: &Capture) -> Value {
    json!({
        "capture_id": capture.id,
        "target": {
            "pid": capture.target.pid, "kind": capture.target.kind,
            "name": capture.target.name, "project": capture.target.project,
        },
        "dropped_by_buffer": capture.dropped_overflow,
        "dropped_by_target": capture.dropped_in_target,
        "events": capture.events.iter().map(|e| json!({
            "name": e.name, "thread": e.thread_id, "thread_name": e.thread_name,
            "track": e.track_name, "start_ns": e.start_ns, "duration_ns": e.duration_ns,
            "depth": e.depth, "location": e.location, "metadata": e.metadata,
        })).collect::<Vec<_>>(),
    })
}

/// Write `<dir>/capture-<id>.json` and `<dir>/capture-<id>.trace.json`;
/// returns both paths.
pub fn write(capture: &Capture, dir: &Path) -> std::io::Result<(PathBuf, PathBuf)> {
    std::fs::create_dir_all(dir)?;
    let raw = dir.join(format!("capture-{}.json", capture.id));
    let trace = dir.join(format!("capture-{}.trace.json", capture.id));
    std::fs::write(&raw, serde_json::to_vec(&raw_json(capture))?)?;
    std::fs::write(&trace, serde_json::to_vec(&chrome_trace(capture))?)?;
    Ok((raw, trace))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::game;

    #[test]
    fn a_trace_names_its_threads_and_starts_at_zero() {
        let capture = Capture {
            id: 7,
            events: game(2, 16, 10, None),
            ..Default::default()
        };
        let t = chrome_trace(&capture);
        let events = t["traceEvents"].as_array().unwrap();
        assert_eq!(events[0]["ts"], 0.0);
        assert!(events.iter().any(|e| e["ph"] == "M" && e["args"]["name"] == "t2"));
    }

    #[test]
    fn both_files_are_written() {
        let dir = tempfile::tempdir().unwrap();
        let capture = Capture {
            id: 3,
            events: game(2, 16, 10, None),
            ..Default::default()
        };
        let (raw, trace) = write(&capture, dir.path()).unwrap();
        let back: Value = serde_json::from_slice(&std::fs::read(raw).unwrap()).unwrap();
        assert_eq!(back["events"].as_array().unwrap().len(), 8);
        assert!(std::fs::metadata(trace).unwrap().len() > 100);
    }
}
