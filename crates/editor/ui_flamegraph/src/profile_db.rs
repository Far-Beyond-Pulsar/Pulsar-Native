//! Canned read-only queries over a profiling recording (the SQLite file the
//! recorder writes, `profiling::database`), answering with JSON.
//!
//! A recording is one table, `profile_events`, one row per finished scope, plus
//! `session_metadata`. Nothing here keeps state or a copy of the data: every
//! function opens nothing itself and runs SQL over the connection it is given,
//! which the caller opens read-only ([`open_read_only`]).
//!
//! Times in answers are milliseconds. A scope's *self* time is its duration
//! less the durations of the rows whose `parent_scope_id` is its `scope_id`.
//! A *frame* is one row of the frame scope (the packaged game's `Game::Frame`,
//! else `helio_frame`, `__FRAME_MARKER__`, `TickLoop::tick`, `PIE::pie_tick`);
//! its *cost* is the row's duration and its *period* the start-to-start
//! distance to the next frame, which is what the player perceives.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use rusqlite::types::ValueRef;
use rusqlite::{params, Connection, OpenFlags};
use serde_json::{json, Value};

type Res<T> = Result<T, String>;

fn sql<T>(r: rusqlite::Result<T>) -> Res<T> {
    r.map_err(|e| format!("sql: {e}"))
}

/// Scopes whose name starts with `__` are markers (frame times), not work.
const NOT_MARKER: &str = "name NOT LIKE '\\_\\_%' ESCAPE '\\'";

/// The scopes that delimit a frame, most useful first.
pub const FRAME_CANDIDATES: &[&str] = &[
    "Game::Frame",
    "helio_frame",
    "__FRAME_MARKER__",
    "TickLoop::tick",
    "PIE::pie_tick",
];

/// Names containing one of these (lower case) are time spent waiting.
const WAIT_WORDS: &[&str] = &[
    "wait", "lock", "fence", "present", "acquire", "sleep", "idle", "park", "block", "recv",
    "vsync",
];

pub fn is_wait_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    WAIT_WORDS.iter().any(|w| lower.contains(w))
}

fn ms(ns: i64) -> f64 {
    ns as f64 / 1.0e6
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn ms3(ns: i64) -> f64 {
    round3(ms(ns))
}

/// Open a recording that cannot be written to.
pub fn open_read_only(path: &Path) -> Res<Connection> {
    let conn = sql(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ))?;
    sql(conn.execute_batch("PRAGMA query_only = ON;"))?;
    // A recording from before logical scope ids has no such columns.
    let has: i64 = sql(conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('profile_events') WHERE name IN ('scope_id','parent_scope_id','track_name')",
        [],
        |row| row.get(0),
    ))?;
    if has < 3 {
        return Err(format!(
            "{} is an older recording (no scope_id/parent_scope_id/track_name columns); open it once in the Flamegraph window, or record again",
            path.display()
        ));
    }
    Ok(conn)
}

// ---- finding recordings ----------------------------------------------------------

/// The recording files of a project, newest first.
pub fn recordings(project: &str) -> Vec<PathBuf> {
    profiling::database::list_profiling_sessions(project).unwrap_or_default()
}

/// `latest`, a file stem (`flamegraph_2026-11-25_10-00-00`), a file name or a
/// path.
pub fn resolve(project: Option<&str>, which: Option<&str>) -> Res<PathBuf> {
    let which = which.map(str::trim).filter(|s| !s.is_empty());
    if let Some(w) = which.filter(|w| *w != "latest") {
        let direct = Path::new(w);
        if direct.is_absolute() && direct.exists() {
            return Ok(direct.to_owned());
        }
    }
    let project = project.ok_or("no project is open, so there is no recordings folder: pass an absolute path")?;
    let all = recordings(project);
    match which {
        None | Some("latest") => all
            .into_iter()
            .next()
            .ok_or_else(|| "no recording yet: profile_start, drive the game, profile_stop".to_owned()),
        Some(w) => all
            .iter()
            .find(|p| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let stem = p.file_stem().and_then(|n| n.to_str()).unwrap_or("");
                name == w || stem == w
            })
            .cloned()
            .ok_or_else(|| format!("no recording {w:?} in the project (profile_list_recordings lists them)")),
    }
}

/// One line per recording: file, size, events, time span, frame stream.
pub fn describe(path: &Path) -> Value {
    let mut v = json!({
        "id": path.file_stem().and_then(|s| s.to_str()),
        "path": path.display().to_string(),
        "bytes": std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
    });
    match open_read_only(path) {
        Ok(conn) => {
            if let Ok(o) = overview(&conn) {
                v["overview"] = o;
            }
        }
        Err(e) => v["error"] = json!(e),
    }
    v
}

// ---- frames ------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Frame {
    pub index: usize,
    pub thread_id: i64,
    pub start_ns: i64,
    pub cost_ns: i64,
    /// To the next frame's start (the cost, for the last).
    pub period_ns: i64,
}

impl Frame {
    fn window_end(&self) -> i64 {
        self.start_ns + self.period_ns.max(self.cost_ns)
    }
}

/// The frame stream: the scope name and its frames.
pub fn frames(conn: &Connection, scope: Option<&str>, thread: Option<i64>) -> Res<Option<(String, Vec<Frame>)>> {
    let names: Vec<&str> = match scope {
        Some(s) => vec![s],
        None => FRAME_CANDIDATES.to_vec(),
    };
    for name in names {
        let busiest: Option<(i64, i64)> = match thread {
            Some(t) => Some((t, 0)),
            None => sql(conn
                .query_row(
                    "SELECT thread_id, COUNT(*) c FROM profile_events WHERE name = ?1 GROUP BY thread_id ORDER BY c DESC LIMIT 1",
                    [name],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map(Some)
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    e => Err(e),
                }))?,
        };
        let Some((thread_id, _)) = busiest else { continue };
        let marker = name == "__FRAME_MARKER__";
        let mut stmt = sql(conn.prepare(
            "SELECT start_ns, duration_ns FROM profile_events WHERE name = ?1 AND thread_id = ?2 ORDER BY start_ns",
        ))?;
        let rows = sql(stmt.query_map(params![name, thread_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        }))?;
        let mut frames: Vec<Frame> = Vec::new();
        for row in rows {
            let (start, dur) = sql(row)?;
            frames.push(Frame {
                index: frames.len(),
                thread_id,
                // A frame marker is published when the frame ends.
                start_ns: if marker { start - dur } else { start },
                cost_ns: dur,
                period_ns: dur,
            });
        }
        if frames.len() < if scope.is_some() { 1 } else { 2 } {
            continue;
        }
        for i in 0..frames.len().saturating_sub(1) {
            frames[i].period_ns = frames[i + 1].start_ns - frames[i].start_ns;
        }
        return Ok(Some((name.to_owned(), frames)));
    }
    Ok(None)
}

// ---- summaries ----------------------------------------------------------------------

#[derive(Clone, Copy, Default)]
struct Summary {
    count: usize,
    mean: f64,
    p50: f64,
    p95: f64,
    p99: f64,
    max: f64,
}

impl Summary {
    /// Of nanosecond samples, in milliseconds.
    fn of_ns(samples: &[i64]) -> Self {
        let mut v: Vec<f64> = samples.iter().map(|&n| ms(n)).collect();
        v.sort_by(|a, b| a.total_cmp(b));
        Self::of_sorted(&v)
    }

    fn of_sorted(v: &[f64]) -> Self {
        if v.is_empty() {
            return Self::default();
        }
        let pct = |p: f64| {
            let rank = p / 100.0 * (v.len() - 1) as f64;
            let (lo, hi) = (rank.floor() as usize, rank.ceil() as usize);
            v[lo] + (v[hi] - v[lo]) * (rank - lo as f64)
        };
        Self {
            count: v.len(),
            mean: v.iter().sum::<f64>() / v.len() as f64,
            p50: pct(50.0),
            p95: pct(95.0),
            p99: pct(99.0),
            max: v[v.len() - 1],
        }
    }

    fn json(&self) -> Value {
        json!({ "count": self.count, "mean": round3(self.mean), "p50": round3(self.p50),
                "p95": round3(self.p95), "p99": round3(self.p99), "max": round3(self.max) })
    }
}

/// What the recording holds: the head of every answer.
pub fn overview(conn: &Connection) -> Res<Value> {
    let (events, first, last): (i64, Option<i64>, Option<i64>) = sql(conn.query_row(
        "SELECT COUNT(*), MIN(start_ns), MAX(start_ns + duration_ns) FROM profile_events",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ))?;
    let threads: i64 = sql(conn.query_row("SELECT COUNT(DISTINCT thread_id) FROM profile_events", [], |r| r.get(0)))?;
    let mut meta = serde_json::Map::new();
    if let Ok(mut s) = conn.prepare("SELECT key, value FROM session_metadata") {
        if let Ok(rows) = s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))) {
            for (k, v) in rows.flatten() {
                meta.insert(k, json!(v));
            }
        }
    }
    let frame = frames(conn, None, None)?;
    let costs: Vec<i64> = frame.as_ref().map(|(_, f)| f.iter().map(|f| f.cost_ns).collect()).unwrap_or_default();
    Ok(json!({
        "events": events,
        "window_ms": ms3(last.unwrap_or(0) - first.unwrap_or(0)),
        "threads": threads,
        "frame_scope": frame.as_ref().map(|(n, _)| n),
        "frames": frame.as_ref().map(|(_, f)| f.len()).unwrap_or(0),
        "frame_cost_ms": Summary::of_ns(&costs).json(),
        "session": meta,
    }))
}

// ---- the scope table ------------------------------------------------------------------

#[derive(Clone, Debug)]
struct ScopeRow {
    thread_id: i64,
    thread_name: String,
    name: String,
    calls: i64,
    total_ns: i64,
    self_ns: i64,
    max_ns: i64,
}

/// One row per (thread, scope name) with own time computed from the parent links.
fn scope_table(conn: &Connection) -> Res<Vec<ScopeRow>> {
    let query = format!(
        "WITH child AS (SELECT parent_scope_id AS p, SUM(duration_ns) AS c FROM profile_events \
         WHERE parent_scope_id IS NOT NULL GROUP BY parent_scope_id) \
         SELECT e.thread_id, COALESCE(MAX(e.thread_name), ''), e.name, COUNT(*), SUM(e.duration_ns), \
                SUM(MAX(e.duration_ns - COALESCE(c.c, 0), 0)), MAX(e.duration_ns) \
         FROM profile_events e LEFT JOIN child c ON c.p = e.scope_id \
         WHERE e.{NOT_MARKER} GROUP BY e.thread_id, e.name"
    );
    let mut stmt = sql(conn.prepare(&query))?;
    let rows = sql(stmt.query_map([], |row| {
        Ok(ScopeRow {
            thread_id: row.get(0)?,
            thread_name: row.get(1)?,
            name: row.get(2)?,
            calls: row.get(3)?,
            total_ns: row.get(4)?,
            self_ns: row.get(5)?,
            max_ns: row.get(6)?,
        })
    }))?;
    rows.map(|r| sql(r)).collect()
}

fn thread_label(id: i64, name: &str) -> String {
    if !name.is_empty() {
        name.to_owned()
    } else if id == 0 {
        "GPU".to_owned()
    } else {
        format!("thread-{id}")
    }
}

#[derive(Clone, Debug)]
pub struct TopParams {
    pub by_thread: bool,
    pub sort: String,
    pub limit: usize,
    pub thread: Option<String>,
    pub name: Option<String>,
    pub frame_scope: Option<String>,
}

impl Default for TopParams {
    fn default() -> Self {
        Self { by_thread: false, sort: "self".into(), limit: 25, thread: None, name: None, frame_scope: None }
    }
}

/// Self and total time per scope, calls, per-call percentiles, per frame.
pub fn top_scopes(conn: &Connection, p: &TopParams) -> Res<Value> {
    let table = scope_table(conn)?;
    let frame = frames(conn, p.frame_scope.as_deref(), None)?;
    let nframes = frame.as_ref().map(|(_, f)| f.len()).unwrap_or(0).max(1) as f64;
    let window: i64 = sql(conn.query_row(
        "SELECT COALESCE(MAX(start_ns + duration_ns) - MIN(start_ns), 1) FROM profile_events",
        [],
        |r| r.get(0),
    ))?;
    let want_thread = p.thread.as_ref().map(|t| t.to_ascii_lowercase());
    let want_name = p.name.as_ref().map(|t| t.to_ascii_lowercase());
    // Group rows: by name, or by (thread, name).
    let mut groups: BTreeMap<(i64, String), ScopeRow> = BTreeMap::new();
    for row in table {
        let label = thread_label(row.thread_id, &row.thread_name);
        if want_thread.as_ref().is_some_and(|t| !label.to_ascii_lowercase().contains(t)) {
            continue;
        }
        if want_name.as_ref().is_some_and(|n| !row.name.to_ascii_lowercase().contains(n)) {
            continue;
        }
        let key = (if p.by_thread { row.thread_id } else { 0 }, row.name.clone());
        let g = groups.entry(key).or_insert_with(|| ScopeRow { thread_id: if p.by_thread { row.thread_id } else { 0 }, thread_name: if p.by_thread { label } else { String::new() }, name: row.name.clone(), calls: 0, total_ns: 0, self_ns: 0, max_ns: 0 });
        g.calls += row.calls;
        g.total_ns += row.total_ns;
        g.self_ns += row.self_ns;
        g.max_ns = g.max_ns.max(row.max_ns);
    }
    let mut rows: Vec<ScopeRow> = groups.into_values().collect();
    let key = |r: &ScopeRow| -> f64 {
        match p.sort.as_str() {
            "total" => r.total_ns as f64,
            "max" => r.max_ns as f64,
            "count" => r.calls as f64,
            "mean" => r.total_ns as f64 / r.calls.max(1) as f64,
            _ => r.self_ns as f64,
        }
    };
    if !["self", "total", "max", "count", "mean"].contains(&p.sort.as_str()) {
        return Err(format!("sort must be self, total, max, count or mean, not {}", p.sort));
    }
    rows.sort_by(|a, b| key(b).total_cmp(&key(a)));
    let total_rows = rows.len();
    rows.truncate(p.limit.clamp(1, 500));

    // Per-call percentiles: one pass over the events of the rows kept.
    let mut durations: HashMap<(i64, String), Vec<i64>> = HashMap::new();
    if !rows.is_empty() {
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        let placeholders = vec!["?"; names.len()].join(",");
        let mut stmt = sql(conn.prepare(&format!(
            "SELECT thread_id, name, duration_ns FROM profile_events WHERE name IN ({placeholders})"
        )))?;
        let it = sql(stmt.query_map(rusqlite::params_from_iter(names.iter()), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?))
        }))?;
        for row in it {
            let (t, n, d) = sql(row)?;
            durations.entry((if p.by_thread { t } else { 0 }, n)).or_default().push(d);
        }
    }
    let scopes: Vec<Value> = rows
        .iter()
        .map(|row| {
            let calls = durations.get(&(row.thread_id, row.name.clone())).map(|v| Summary::of_ns(v)).unwrap_or_default();
            let mut v = json!({
                "name": row.name,
                "calls": row.calls,
                "calls_per_frame": round3(row.calls as f64 / nframes),
                "total_ms": ms3(row.total_ns),
                "self_ms": ms3(row.self_ns),
                "self_pct_of_window": round3(row.self_ns as f64 / window.max(1) as f64 * 100.0),
                "per_frame_total_ms": round3(ms(row.total_ns) / nframes),
                "per_frame_self_ms": round3(ms(row.self_ns) / nframes),
                "per_call_ms": calls.json(),
            });
            if p.by_thread {
                v["thread"] = json!(row.thread_name);
            }
            v
        })
        .collect();
    Ok(json!({
        "overview": overview(conn)?,
        "rows_total": total_rows,
        "scopes": scopes,
        "worst_frames": worst_frames(conn, frame.as_ref().map(|(_, f)| f.as_slice()).unwrap_or(&[]), 5)?,
    }))
}

/// What ran (by own time, any thread) in `[from, to)`.
fn top_in_window(conn: &Connection, from: i64, to: i64, n: usize) -> Res<Vec<Value>> {
    let query = format!(
        "WITH child AS (SELECT parent_scope_id AS p, SUM(duration_ns) AS c FROM profile_events \
         WHERE parent_scope_id IS NOT NULL AND start_ns >= ?1 - 1000000000 AND start_ns < ?2 + 1000000000 GROUP BY parent_scope_id) \
         SELECT e.name, COALESCE(MAX(e.thread_name), ''), e.thread_id, SUM(MAX(e.duration_ns - COALESCE(c.c, 0), 0)) s \
         FROM profile_events e LEFT JOIN child c ON c.p = e.scope_id \
         WHERE e.start_ns >= ?1 AND e.start_ns < ?2 AND e.{NOT_MARKER} GROUP BY e.thread_id, e.name ORDER BY s DESC LIMIT ?3"
    );
    let mut stmt = sql(conn.prepare(&query))?;
    let rows = sql(stmt.query_map(params![from, to, n as i64], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?))
    }))?;
    rows.map(|r| {
        let (name, tn, tid, s) = sql(r)?;
        Ok(json!({ "name": name, "thread": thread_label(tid, &tn), "self_ms": ms3(s) }))
    })
    .collect()
}

fn worst_frames(conn: &Connection, frames: &[Frame], n: usize) -> Res<Vec<Value>> {
    let origin: i64 = sql(conn.query_row("SELECT COALESCE(MIN(start_ns), 0) FROM profile_events", [], |r| r.get(0)))?;
    let mut order: Vec<&Frame> = frames.iter().collect();
    order.sort_by_key(|f| std::cmp::Reverse(f.period_ns));
    order
        .into_iter()
        .take(n)
        .map(|f| {
            Ok(json!({
                "frame": f.index,
                "at_ms": ms3(f.start_ns - origin),
                "period_ms": ms3(f.period_ns),
                "cost_ms": ms3(f.cost_ns),
                "ran": top_in_window(conn, f.start_ns, f.window_end(), 4)?,
            }))
        })
        .collect()
}

// ---- frame times -------------------------------------------------------------------------

pub fn frame_times(
    conn: &Connection,
    frame_scope: Option<&str>,
    threshold_ms: Option<f64>,
    series_limit: usize,
    hitch_limit: usize,
) -> Res<Value> {
    let Some((scope, frames)) = frames(conn, frame_scope, None)? else {
        return Err("no frames in this recording (no Game::Frame, helio_frame or frame marker): name one with frame_scope".into());
    };
    let origin: i64 = sql(conn.query_row("SELECT COALESCE(MIN(start_ns), 0) FROM profile_events", [], |r| r.get(0)))?;
    let body = &frames[..frames.len().saturating_sub(1).max(1)];
    let periods: Vec<f64> = body.iter().map(|f| ms(f.period_ns)).collect();
    let costs: Vec<i64> = frames.iter().map(|f| f.cost_ns).collect();
    let mut sorted = periods.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let ps = Summary::of_sorted(&sorted);
    let threshold = threshold_ms.unwrap_or_else(|| (ps.p50 * 2.0).max(ps.p50 + 8.0));
    let edges = [2.0, 4.0, 6.0, 8.0, 10.0, 12.0, 14.0, 16.7, 20.0, 25.0, 33.4, 50.0, 100.0];
    let mut counts = vec![0usize; edges.len() + 1];
    for p in &periods {
        counts[edges.iter().position(|e| p < e).unwrap_or(edges.len())] += 1;
    }
    let mut hist = Vec::new();
    let mut lo = 0.0;
    for (i, c) in counts.iter().enumerate() {
        let label = match edges.get(i) {
            Some(hi) => format!("{lo}-{hi}"),
            None => format!(">{lo}"),
        };
        hist.push(json!({ "ms": label, "frames": c, "pct": round3(*c as f64 / periods.len().max(1) as f64 * 100.0) }));
        if let Some(hi) = edges.get(i) {
            lo = *hi;
        }
    }
    let stride = periods.len().div_ceil(series_limit.max(1)).max(1);
    let series: Vec<f64> = periods.chunks(stride).map(|c| round3(c.iter().copied().fold(0.0, f64::max))).collect();
    let mut hitches: Vec<&Frame> = body.iter().filter(|f| ms(f.period_ns) > threshold).collect();
    let hitch_count = hitches.len();
    hitches.sort_by_key(|f| std::cmp::Reverse(f.period_ns));
    let rows: Vec<Value> = hitches
        .into_iter()
        .take(hitch_limit)
        .map(|f| {
            Ok(json!({
                "frame": f.index,
                "at_ms": ms3(f.start_ns - origin),
                "period_ms": ms3(f.period_ns),
                "cost_ms": ms3(f.cost_ns),
                "outside_scope_ms": ms3((f.period_ns - f.cost_ns).max(0)),
                "ran": top_in_window(conn, f.start_ns, f.window_end(), 5)?,
            }))
        })
        .collect::<Res<_>>()?;
    // The other frame-like scopes (the simulation tick next to the render frame).
    let mut others = Vec::new();
    for name in FRAME_CANDIDATES.iter().filter(|n| **n != scope) {
        if let Some((n, fr)) = frames_of(conn, name)? {
            let p: Vec<i64> = fr[..fr.len() - 1].iter().map(|f| f.period_ns).collect();
            let c: Vec<i64> = fr.iter().map(|f| f.cost_ns).collect();
            others.push(json!({ "scope": n, "count": fr.len(), "period_ms": Summary::of_ns(&p).json(), "cost_ms": Summary::of_ns(&c).json() }));
        }
    }
    Ok(json!({
        "frame_scope": scope,
        "frames": frames.len(),
        "frame_period_ms": ps.json(),
        "frame_cost_ms": Summary::of_ns(&costs).json(),
        "fps_mean": round3(1000.0 / ps.mean.max(1e-9)),
        "fps_1pct_low": round3(1000.0 / ps.p99.max(1e-9)),
        "histogram": hist,
        "hitch_threshold_ms": round3(threshold),
        "hitches": hitch_count,
        "worst_hitches": rows,
        "series_ms": series,
        "series_stride_frames": stride,
        "other_frame_streams": others,
    }))
}

fn frames_of(conn: &Connection, name: &str) -> Res<Option<(String, Vec<Frame>)>> {
    Ok(frames(conn, Some(name), None)?.filter(|(_, f)| f.len() >= 2))
}

// ---- one frame -------------------------------------------------------------------------------

struct Node {
    id: i64,
    parent: Option<i64>,
    name: String,
    start: i64,
    dur: i64,
    own: i64,
    location: Option<String>,
}

pub fn frame_breakdown(
    conn: &Connection,
    frame_scope: Option<&str>,
    frame: Option<usize>,
    by_cost: bool,
    max_depth: usize,
    min_ms: f64,
) -> Res<Value> {
    let Some((scope, frames)) = frames(conn, frame_scope, None)? else {
        return Err("no frames in this recording: name the frame scope (frame_scope)".into());
    };
    let f = match frame {
        Some(i) => frames.get(i).ok_or_else(|| format!("frame {i} is out of range (0..{})", frames.len()))?,
        None => frames
            .iter()
            .max_by_key(|f| if by_cost { f.cost_ns } else { f.period_ns })
            .expect("frames is not empty"),
    };
    let origin: i64 = sql(conn.query_row("SELECT COALESCE(MIN(start_ns), 0) FROM profile_events", [], |r| r.get(0)))?;
    let (from, to) = (f.start_ns, f.start_ns + f.cost_ns);
    // Every scope of the frame's thread inside the frame, with own time.
    let mut stmt = sql(conn.prepare(
        "WITH child AS (SELECT parent_scope_id AS p, SUM(duration_ns) AS c FROM profile_events \
         WHERE thread_id = ?1 AND start_ns >= ?2 AND start_ns <= ?3 AND parent_scope_id IS NOT NULL GROUP BY parent_scope_id) \
         SELECT e.scope_id, e.parent_scope_id, e.name, e.start_ns, e.duration_ns, MAX(e.duration_ns - COALESCE(c.c, 0), 0), e.location \
         FROM profile_events e LEFT JOIN child c ON c.p = e.scope_id \
         WHERE e.thread_id = ?1 AND e.start_ns >= ?2 AND e.start_ns <= ?3 ORDER BY e.start_ns, e.duration_ns DESC",
    ))?;
    let nodes: Vec<Node> = sql(stmt.query_map(params![f.thread_id, from, to], |row| {
        Ok(Node {
            id: row.get(0)?,
            parent: row.get(1)?,
            name: row.get(2)?,
            start: row.get(3)?,
            dur: row.get(4)?,
            own: row.get(5)?,
            location: row.get(6)?,
        })
    }))?
    .collect::<rusqlite::Result<_>>()
    .map_err(|e| format!("sql: {e}"))?;
    let by_id: HashMap<i64, usize> = nodes.iter().enumerate().map(|(i, n)| (n.id, i)).collect();
    let mut children: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
    for (i, n) in nodes.iter().enumerate() {
        // Parents outside the frame (the marker case) make a node a root here.
        let parent = n.parent.and_then(|p| by_id.get(&p).copied());
        children.entry(parent).or_default().push(i);
    }
    let root = nodes.iter().position(|n| n.name == scope && n.start == f.start_ns);
    fn render(nodes: &[Node], children: &HashMap<Option<usize>, Vec<usize>>, i: usize, origin: i64, depth: usize, max_depth: usize, min_ms: f64) -> Value {
        let n = &nodes[i];
        let mut shown = Vec::new();
        let (mut hidden, mut hidden_ns) = (0usize, 0i64);
        for &c in children.get(&Some(i)).map(Vec::as_slice).unwrap_or(&[]) {
            if ms(nodes[c].dur) < min_ms || depth + 1 > max_depth {
                hidden += 1;
                hidden_ns += nodes[c].dur;
            } else {
                shown.push(render(nodes, children, c, origin, depth + 1, max_depth, min_ms));
            }
        }
        let mut v = json!({ "name": n.name, "offset_ms": ms3(n.start - origin), "total_ms": ms3(n.dur), "self_ms": ms3(n.own), "children": shown });
        if let Some(l) = &n.location {
            v["location"] = json!(l);
        }
        if hidden > 0 {
            v["hidden"] = json!({ "scopes": hidden, "ms": ms3(hidden_ns) });
        }
        v
    }
    let tree = match root {
        Some(i) => render(&nodes, &children, i, f.start_ns, 0, max_depth, min_ms),
        None => json!({
            "name": "(frame)", "total_ms": ms3(f.cost_ns),
            "children": children.get(&None).map(|v| v.iter().map(|&c| render(&nodes, &children, c, f.start_ns, 1, max_depth, min_ms)).collect::<Vec<_>>()).unwrap_or_default(),
        }),
    };
    let mut path = Vec::new();
    let mut at = root;
    while let Some(i) = at {
        let n = &nodes[i];
        path.push(json!({ "name": n.name, "total_ms": ms3(n.dur), "self_ms": ms3(n.own), "wait": is_wait_name(&n.name) }));
        at = children.get(&Some(i)).and_then(|c| c.iter().copied().max_by_key(|&c| nodes[c].dur));
    }
    // What the other threads did in the frame's window.
    let mut stmt = sql(conn.prepare(&format!(
        "SELECT thread_id, COALESCE(thread_name, ''), name, depth, MAX(start_ns, ?2), MIN(start_ns + duration_ns, ?3) \
         FROM profile_events WHERE thread_id != ?1 AND depth <= 1 AND start_ns < ?3 AND start_ns + duration_ns > ?2 AND {NOT_MARKER}"
    )))?;
    let end = f.window_end();
    let mut per_thread: BTreeMap<i64, (String, i64, Vec<(i64, i64, String, i64)>)> = BTreeMap::new();
    for row in sql(stmt.query_map(params![f.thread_id, from, end], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, i64>(3)?, row.get::<_, i64>(4)?, row.get::<_, i64>(5)?))
    }))? {
        let (tid, tn, name, depth, s, e) = sql(row)?;
        let entry = per_thread.entry(tid).or_insert_with(|| (thread_label(tid, &tn), 0, Vec::new()));
        if depth == 0 {
            entry.1 += e - s;
        }
        entry.2.push((s, e, name, depth));
    }
    let others: Vec<Value> = per_thread
        .into_values()
        .map(|(label, busy, mut items)| {
            items.sort_by_key(|(s, e, _, _)| std::cmp::Reverse(e - s));
            json!({ "thread": label, "busy_ms": ms3(busy), "scopes": items.iter().filter(|(s, e, _, _)| ms(e - s) >= min_ms).take(10).map(|(s, e, n, d)| json!({ "name": n, "depth": d, "offset_ms": ms3(s - f.start_ns), "ms_in_frame": ms3(e - s) })).collect::<Vec<_>>() })
        })
        .collect();
    Ok(json!({
        "frame_scope": scope,
        "frame": { "index": f.index, "at_ms": ms3(f.start_ns - origin), "cost_ms": ms3(f.cost_ns), "period_ms": ms3(f.period_ns), "outside_scope_ms": ms3((f.period_ns - f.cost_ns).max(0)) },
        "tree": tree,
        "critical_path": path,
        "other_threads": others,
    }))
}

// ---- threads -----------------------------------------------------------------------------------

pub fn threads(conn: &Connection) -> Res<Value> {
    let table = scope_table(conn)?;
    let (first, last): (i64, i64) = sql(conn.query_row(
        "SELECT COALESCE(MIN(start_ns), 0), COALESCE(MAX(start_ns + duration_ns), 1) FROM profile_events",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ))?;
    let window = (last - first).max(1);
    let busy: HashMap<i64, i64> = {
        let mut stmt = sql(conn.prepare(&format!(
            "SELECT thread_id, SUM(duration_ns) FROM profile_events WHERE depth = 0 AND {NOT_MARKER} GROUP BY thread_id"
        )))?;
        let rows = sql(stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))))?;
        rows.map(|r| sql(r)).collect::<Res<_>>()?
    };
    let mut by_thread: BTreeMap<i64, Vec<&ScopeRow>> = BTreeMap::new();
    for row in &table {
        by_thread.entry(row.thread_id).or_default().push(row);
    }
    let mut out = Vec::new();
    for (id, rows) in &by_thread {
        let label = thread_label(*id, rows.iter().map(|r| r.thread_name.as_str()).find(|n| !n.is_empty()).unwrap_or(""));
        let waits: i64 = rows.iter().filter(|r| is_wait_name(&r.name)).map(|r| r.self_ns).sum();
        let mut work: Vec<&&ScopeRow> = rows.iter().filter(|r| !is_wait_name(&r.name)).collect();
        work.sort_by_key(|r| std::cmp::Reverse(r.self_ns));
        let b = busy.get(id).copied().unwrap_or(0);
        out.push(json!({
            "thread": label,
            "id": id,
            "scopes": rows.iter().map(|r| r.calls).sum::<i64>(),
            "distinct_scopes": rows.len(),
            "busy_ms": ms3(b),
            "idle_ms": ms3((window - b).max(0)),
            "utilisation_pct": round3(b as f64 / window as f64 * 100.0),
            "wait_scope_self_ms": ms3(waits),
            "top_work_scopes": work.iter().take(5).map(|r| json!({ "name": r.name, "self_ms": ms3(r.self_ns), "calls": r.calls })).collect::<Vec<_>>(),
        }));
    }
    out.sort_by(|a, b| b["busy_ms"].as_f64().partial_cmp(&a["busy_ms"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    let mut waits: Vec<&ScopeRow> = table.iter().filter(|r| is_wait_name(&r.name)).collect();
    waits.sort_by_key(|r| std::cmp::Reverse(r.self_ns));
    Ok(json!({
        "note": "busy = the duration of the thread's outermost scopes; time between them is idle or uninstrumented. wait_scope_self_ms is the own time of scopes named like a wait (wait, lock, fence, present, acquire, sleep, idle...).",
        "window_ms": ms3(window),
        "threads": out,
        "waits": waits.iter().take(15).map(|r| json!({ "thread": thread_label(r.thread_id, &r.thread_name), "scope": r.name, "self_ms": ms3(r.self_ns), "calls": r.calls, "max_ms": ms3(r.max_ns) })).collect::<Vec<_>>(),
    }))
}

// ---- GPU ------------------------------------------------------------------------------------------

pub fn gpu(conn: &Connection) -> Res<Value> {
    let mut stmt = sql(conn.prepare(
        "SELECT name, depth, duration_ns, metadata FROM profile_events WHERE thread_name = 'GPU' OR track_name = 'GPU'",
    ))?;
    let rows = sql(stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, Option<String>>(3)?))
    }))?;
    let (mut totals, mut meta) = (Vec::new(), BTreeMap::new());
    let mut passes: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    for row in rows {
        let (name, depth, dur, metadata) = sql(row)?;
        if depth == 0 {
            totals.push(dur);
        } else {
            passes.entry(name).or_default().push(dur);
        }
        for kv in metadata.as_deref().unwrap_or("").split(';') {
            if let Some((k, v)) = kv.split_once('=') {
                meta.insert(k.to_owned(), v.to_owned());
            }
        }
    }
    if totals.is_empty() && passes.is_empty() {
        return Ok(json!({
            "available": false,
            "reason": "the recording holds no GPU events. Helio publishes per-pass GPU times (timestamp queries, read back asynchronously) on the GPU track while a recording runs, from the renderer that reports them (`helio_gpu_profiler_update`, `helio_emit_gpu_passes`). A process whose renderer does not report publishes none.",
        }));
    }
    let total_ns: i64 = totals.iter().sum();
    let mut rows: Vec<(String, Vec<i64>)> = passes.into_iter().collect();
    rows.sort_by_key(|(_, v)| std::cmp::Reverse(v.iter().sum::<i64>()));
    Ok(json!({
        "available": true,
        "gpu_frames": totals.len(),
        "gpu_frame_ms": Summary::of_ns(&totals).json(),
        "passes": rows.iter().map(|(n, v)| json!({
            "pass": n,
            "mean_ms_per_gpu_frame": round3(ms(v.iter().sum::<i64>()) / totals.len().max(1) as f64),
            "per_sample_ms": Summary::of_ns(v).json(),
            "share_pct": round3(v.iter().sum::<i64>() as f64 / total_ns.max(1) as f64 * 100.0),
        })).collect::<Vec<_>>(),
        "last_readback": { "lag_frames": meta.get("lag_frames"), "readback_drops": meta.get("readback_drops"), "query_overflows": meta.get("query_overflows") },
        "note": "pass times are timestamp-query durations; their placement on the timeline is an estimate.",
    }))
}

// ---- input latency ------------------------------------------------------------------------------------

/// The track the game publishes the input-latency chain on, and the root and
/// stage names (`pulsar_game::latency`).
pub const LATENCY_TRACK: &str = "Input latency";
pub const LATENCY_ROOT: &str = "Latency::input_to_present";

pub fn latency(conn: &Connection) -> Res<Value> {
    let mut stmt = sql(conn.prepare(
        "SELECT e.name, e.duration_ns, e.start_ns - p.start_ns FROM profile_events e \
         JOIN profile_events p ON p.scope_id = e.parent_scope_id WHERE e.track_name = ?1",
    ))?;
    let mut stages: BTreeMap<String, (Vec<i64>, i64)> = BTreeMap::new();
    for row in sql(stmt.query_map([LATENCY_TRACK], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
    }))? {
        let (name, dur, offset) = sql(row)?;
        let e = stages.entry(name).or_default();
        e.0.push(dur);
        e.1 += offset;
    }
    let totals: Vec<i64> = {
        let mut s = sql(conn.prepare("SELECT duration_ns FROM profile_events WHERE track_name = ?1 AND name = ?2"))?;
        let rows = sql(s.query_map(params![LATENCY_TRACK, LATENCY_ROOT], |r| r.get::<_, i64>(0)))?;
        rows.map(|r| sql(r)).collect::<Res<_>>()?
    };
    if totals.is_empty() {
        return Ok(json!({
            "available": false,
            "reason": "the recording holds no input-latency samples. The game publishes one chain (track `Input latency`) per followed input while it is recorded: press keys or move the mouse during the recording.",
        }));
    }
    let total = Summary::of_ns(&totals);
    let mut ordered: Vec<(String, (Vec<i64>, i64))> = stages.into_iter().collect();
    ordered.sort_by_key(|(_, (v, off))| off / v.len().max(1) as i64);
    Ok(json!({
        "samples": totals.len(),
        "input_to_present_ms": total.json(),
        "stages": ordered.iter().map(|(n, (v, _))| {
            let s = Summary::of_ns(v);
            json!({ "stage": n.trim_start_matches("Latency::"), "mean_ms": round3(s.mean), "p50_ms": round3(s.p50), "p95_ms": round3(s.p95), "p99_ms": round3(s.p99), "worst_ms": round3(s.max),
                    "share_of_mean_pct": round3(s.mean / total.mean.max(1e-9) * 100.0) })
        }).collect::<Vec<_>>(),
        "note": "the chain ends when present() returns; the display adds the present queue depth times the frame time (and vsync) on top.",
    }))
}

// ---- gaps ----------------------------------------------------------------------------------------------------

/// Scopes that are mostly their own time: where an inner scope is missing.
pub fn gaps(conn: &Connection, min_self_ms: f64, limit: usize) -> Res<Value> {
    let table = scope_table(conn)?;
    let frame = frames(conn, None, None)?;
    let nframes = frame.as_ref().map(|(_, f)| f.len()).unwrap_or(0).max(1) as f64;
    let mut rows: Vec<&ScopeRow> = table
        .iter()
        .filter(|r| ms(r.self_ns) >= min_self_ms && r.self_ns * 10 >= r.total_ns * 6)
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.self_ns));
    let frame_self = frame.as_ref().map(|(n, _)| {
        let own: i64 = table.iter().filter(|r| &r.name == n).map(|r| r.self_ns).sum();
        let total: i64 = table.iter().filter(|r| &r.name == n).map(|r| r.total_ns).sum();
        json!({ "scope": n, "own_ms_per_frame": round3(ms(own) / nframes), "of_frame_cost_pct": round3(own as f64 / total.max(1) as f64 * 100.0),
                "meaning": "time inside the frame scope that no inner scope covers" })
    });
    Ok(json!({
        "frame_scope_self": frame_self,
        "mostly_own_time": rows.iter().take(limit).map(|r| json!({
            "thread": thread_label(r.thread_id, &r.thread_name),
            "scope": r.name,
            "self_ms_total": ms3(r.self_ns),
            "self_ms_per_frame": round3(ms(r.self_ns) / nframes),
            "self_pct_of_scope": round3(r.self_ns as f64 / r.total_ns.max(1) as f64 * 100.0),
            "calls": r.calls,
            "advice": "60% or more of this scope is its own time: scope the steps inside it to see which one costs",
        })).collect::<Vec<_>>(),
        "note": "a thread that does work but has no scopes does not appear in a recording at all; compare with profile_threads and the editor's Threads view (threads_list)",
    }))
}

// ---- compare ----------------------------------------------------------------------------------------------------

/// Two recordings, scope by scope: per-frame own time before and after.
pub fn compare(a: &Connection, b: &Connection, limit: usize) -> Res<Value> {
    fn per_name(conn: &Connection) -> Res<(HashMap<String, (i64, i64, i64)>, f64, Value)> {
        let table = scope_table(conn)?;
        let fr = frames(conn, None, None)?;
        let n = fr.as_ref().map(|(_, f)| f.len()).unwrap_or(0).max(1) as f64;
        let mut map: HashMap<String, (i64, i64, i64)> = HashMap::new();
        for row in table {
            let e = map.entry(row.name).or_default();
            e.0 += row.self_ns;
            e.1 += row.total_ns;
            e.2 += row.calls;
        }
        let times = match &fr {
            Some((_, f)) => {
                let p: Vec<i64> = f[..f.len().saturating_sub(1).max(1)].iter().map(|f| f.period_ns).collect();
                json!({ "frame_scope": fr.as_ref().map(|(n, _)| n), "frames": f.len(), "period_ms": Summary::of_ns(&p).json() })
            }
            None => json!(null),
        };
        Ok((map, n, times))
    }
    let (ma, na, ta) = per_name(a)?;
    let (mb, nb, tb) = per_name(b)?;
    let mut names: Vec<&String> = ma.keys().chain(mb.keys()).collect();
    names.sort();
    names.dedup();
    let mut rows: Vec<(f64, Value)> = names
        .into_iter()
        .map(|n| {
            let (sa, ta_, ca) = ma.get(n).copied().unwrap_or_default();
            let (sb, tb_, cb) = mb.get(n).copied().unwrap_or_default();
            let (fa, fb) = (ms(sa) / na, ms(sb) / nb);
            (
                (fb - fa).abs(),
                json!({
                    "scope": n,
                    "self_ms_per_frame": { "a": round3(fa), "b": round3(fb), "delta": round3(fb - fa) },
                    "total_ms_per_frame": { "a": round3(ms(ta_) / na), "b": round3(ms(tb_) / nb) },
                    "calls_per_frame": { "a": round3(ca as f64 / na), "b": round3(cb as f64 / nb) },
                }),
            )
        })
        .collect();
    rows.sort_by(|x, y| y.0.total_cmp(&x.0));
    Ok(json!({
        "a": ta,
        "b": tb,
        "note": "delta = b - a, per frame; negative is faster in b",
        "scopes": rows.into_iter().take(limit.clamp(1, 200)).map(|(_, v)| v).collect::<Vec<_>>(),
    }))
}

// ---- raw SQL ------------------------------------------------------------------------------------------------------

/// Run one read-only statement; at most `limit` rows.
pub fn query(conn: &Connection, statement: &str, limit: usize) -> Res<Value> {
    let limit = limit.clamp(1, 5000);
    // SQLite compiles only the first statement and ignores the rest: say so.
    if statement.trim().trim_end_matches(';').contains(';') {
        return Err("send one statement (a ';' inside it is refused, also inside a string: use char(59))".into());
    }
    let mut stmt = sql(conn.prepare(statement))?;
    if !stmt.readonly() {
        return Err("only read-only statements (SELECT, WITH ... SELECT, PRAGMA table_info) are allowed".into());
    }
    let columns: Vec<String> = stmt.column_names().iter().map(|c| (*c).to_owned()).collect();
    let mut rows = sql(stmt.query([]))?;
    let mut out = Vec::new();
    let mut truncated = false;
    while let Some(row) = sql(rows.next())? {
        if out.len() == limit {
            truncated = true;
            break;
        }
        let mut cells = Vec::with_capacity(columns.len());
        for i in 0..columns.len() {
            cells.push(match sql(row.get_ref(i))? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(n) => json!(n),
                ValueRef::Real(f) => json!(f),
                ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                ValueRef::Blob(b) => json!(format!("<blob {} bytes>", b.len())),
            });
        }
        out.push(Value::Array(cells));
    }
    Ok(json!({ "columns": columns, "rows": out, "row_count": out.len(), "truncated": truncated, "limit": limit }))
}

/// The tables of a recording and what the columns mean.
pub fn schema(conn: &Connection) -> Res<Value> {
    let mut tables = Vec::new();
    let mut stmt = sql(conn.prepare("SELECT name, sql FROM sqlite_master WHERE type IN ('table','index') AND name NOT LIKE 'sqlite_%' ORDER BY type DESC, name"))?;
    for row in sql(stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))))? {
        let (name, ddl) = sql(row)?;
        tables.push(json!({ "name": name, "ddl": ddl }));
    }
    Ok(json!({
        "objects": tables,
        "columns": {
            "profile_events.scope_id": "identity of this finished scope (unique per process run)",
            "profile_events.parent_scope_id": "the scope this one ran inside (also across threads for spawned work); NULL for a root. own time = duration_ns - SUM(child.duration_ns) over rows whose parent_scope_id = scope_id",
            "profile_events.name": "scope name; names starting __ are markers (__FRAME_MARKER__: start_ns is when the frame ended, duration_ns the frame time)",
            "profile_events.thread_id/thread_name": "the OS thread (thread_id 0 and thread_name GPU: the renderer's GPU pass timings)",
            "profile_events.start_ns": "wall-clock nanoseconds since the Unix epoch, comparable across threads and processes",
            "profile_events.duration_ns": "scope duration",
            "profile_events.depth": "nesting on its thread (0 = outermost)",
            "profile_events.location": "file:line when the scope recorded one",
            "profile_events.metadata": "free text; GPU rows carry `lag_frames`, `readback_drops`, `query_overflows`",
            "profile_events.track_name": "logical track (`GPU`, `Input latency`) for work that is not a thread's",
            "session_metadata": "key/value: timestamp, version",
        },
        "examples": [
            "SELECT name, COUNT(*) n, SUM(duration_ns)/1e6 ms FROM profile_events GROUP BY name ORDER BY ms DESC LIMIT 20",
            "SELECT thread_name, COUNT(*) FROM profile_events GROUP BY thread_id",
            "SELECT start_ns, duration_ns/1e6 FROM profile_events WHERE name = 'Game::Frame' AND duration_ns > 30e6",
        ],
    }))
}

// ---- export ----------------------------------------------------------------------------------------------------------------

/// Write the recording as a Chrome trace (chrome://tracing, Perfetto, speedscope)
/// next to it in `<recordings>/exports/`. Returns the path.
pub fn export_chrome_trace(conn: &Connection, recording: &Path) -> Res<PathBuf> {
    use std::io::Write;
    let dir = recording.parent().unwrap_or(Path::new(".")).join("exports");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let out = dir.join(format!("{}.trace.json", recording.file_stem().and_then(|s| s.to_str()).unwrap_or("recording")));
    let file = std::fs::File::create(&out).map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    let mut w = std::io::BufWriter::new(file);
    let origin: i64 = sql(conn.query_row("SELECT COALESCE(MIN(start_ns), 0) FROM profile_events", [], |r| r.get(0)))?;
    let io = |e: std::io::Error| format!("write failed: {e}");
    w.write_all(b"{\"displayTimeUnit\":\"ms\",\"traceEvents\":[").map_err(io)?;
    let mut stmt = sql(conn.prepare(
        "SELECT name, thread_id, thread_name, process_id, start_ns, duration_ns, location, metadata FROM profile_events ORDER BY start_ns",
    ))?;
    let mut rows = sql(stmt.query([]))?;
    let mut threads: BTreeMap<(i64, i64), String> = BTreeMap::new();
    let mut first = true;
    while let Some(row) = sql(rows.next())? {
        let name: String = sql(row.get(0))?;
        let (tid, pid): (i64, i64) = (sql(row.get(1))?, sql(row.get(3))?);
        let tname: Option<String> = sql(row.get(2))?;
        let (start, dur): (i64, i64) = (sql(row.get(4))?, sql(row.get(5))?);
        let (loc, meta): (Option<String>, Option<String>) = (sql(row.get(6))?, sql(row.get(7))?);
        threads.entry((pid, tid)).or_insert_with(|| thread_label(tid, tname.as_deref().unwrap_or("")));
        let event = json!({ "name": name, "ph": "X", "ts": (start - origin) as f64 / 1000.0, "dur": dur as f64 / 1000.0, "pid": pid, "tid": tid,
                            "args": { "location": loc, "metadata": meta } });
        if !first {
            w.write_all(b",").map_err(io)?;
        }
        first = false;
        serde_json::to_writer(&mut w, &event).map_err(|e| e.to_string())?;
    }
    for ((pid, tid), name) in threads {
        let event = json!({ "name": "thread_name", "ph": "M", "pid": pid, "tid": tid, "args": { "name": name } });
        w.write_all(b",").map_err(io)?;
        serde_json::to_writer(&mut w, &event).map_err(|e| e.to_string())?;
    }
    w.write_all(b"]}").map_err(io)?;
    w.flush().map_err(io)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use profiling::ProfileEvent;

    fn ev(id: u64, parent: Option<u64>, name: &str, thread: u64, start: u64, dur: u64, depth: u32) -> ProfileEvent {
        ProfileEvent {
            scope_id: id,
            parent_scope_id: parent,
            name: name.into(),
            thread_id: thread,
            thread_name: Some(format!("t{thread}")),
            process_id: 1,
            parent_name: None,
            start_ns: start,
            duration_ns: dur,
            depth,
            location: None,
            metadata: None,
            track_name: None,
        }
    }

    /// A recording written by the recorder's own functions: `frames` frames of
    /// 16 ms on thread 1 (frame 2 is `slow` times longer), each holding
    /// `render` (7 ms of 10) holding `wait present` (half of that), a 1 ms
    /// physics step on thread 2, and one latency chain.
    fn recording(path: &Path, frames: u64, slow: u64) {
        let conn = profiling::database::create_database(path).unwrap();
        let mut events = Vec::new();
        let (mut t, mut id) = (1_000_000_000u64, 1u64);
        for i in 0..frames {
            let k = if i == 2 { slow } else { 1 };
            let cost = 10_000_000 * k;
            let frame = id;
            events.push(ev(frame, None, "Game::Frame", 1, t, cost, 0));
            events.push(ev(frame + 1, Some(frame), "render", 1, t + 100_000, cost * 7 / 10, 1));
            events.push(ev(frame + 2, Some(frame + 1), "wait present", 1, t + 100_000, cost * 7 / 20, 2));
            events.push(ev(frame + 3, None, "Physics::Step", 2, t + 200_000, 1_000_000, 0));
            id += 4;
            t += 16_000_000 * k;
        }
        let mut gpu = ev(900, None, "helio_frame (estimated GPU placement)", 0, 1_000_000_000, 6_000_000, 0);
        gpu.thread_name = Some("GPU".into());
        gpu.track_name = Some("GPU".into());
        let mut pass = ev(901, Some(900), "Lighting", 0, 1_000_000_000, 4_000_000, 1);
        pass.thread_name = Some("GPU".into());
        pass.track_name = Some("GPU".into());
        events.extend([gpu, pass]);
        let mut root = ev(950, None, LATENCY_ROOT, 77, 1_000_000_000, 12_000_000, 0);
        root.track_name = Some(LATENCY_TRACK.into());
        let mut a = ev(951, Some(950), "Latency::wait_for_tick", 77, 1_000_000_000, 8_000_000, 1);
        a.track_name = Some(LATENCY_TRACK.into());
        let mut b = ev(952, Some(950), "Latency::present", 77, 1_008_000_000, 4_000_000, 1);
        b.track_name = Some(LATENCY_TRACK.into());
        events.extend([root, a, b]);
        profiling::database::save_events(&conn, &events).unwrap();
    }

    fn open(frames: u64, slow: u64) -> (tempfile::TempDir, PathBuf, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flamegraph_test.db");
        recording(&path, frames, slow);
        let conn = open_read_only(&path).unwrap();
        (dir, path, conn)
    }

    #[test]
    fn the_overview_finds_the_frame_stream() {
        let (_d, _p, conn) = open(10, 1);
        let o = overview(&conn).unwrap();
        assert_eq!(o["frame_scope"], "Game::Frame");
        assert_eq!(o["frames"], 10);
        assert_eq!(o["frame_cost_ms"]["p50"], 10.0);
    }

    #[test]
    fn top_scopes_rank_by_own_time_with_percentiles() {
        let (_d, _p, conn) = open(10, 1);
        let v = top_scopes(&conn, &TopParams::default()).unwrap();
        let rows = v["scopes"].as_array().unwrap();
        // render 3.5 ms own and wait 3.5 ms own per frame beat the frame's 3 ms.
        assert_eq!(rows[0]["self_ms"], 35.0);
        let wait = rows.iter().find(|r| r["name"] == "wait present").unwrap();
        assert_eq!(wait["calls"], 10);
        assert_eq!(wait["per_frame_self_ms"], 3.5);
        assert_eq!(wait["per_call_ms"]["p95"], 3.5);
        let by_thread = top_scopes(&conn, &TopParams { by_thread: true, thread: Some("t2".into()), ..Default::default() }).unwrap();
        assert_eq!(by_thread["scopes"].as_array().unwrap().len(), 1);
        assert_eq!(by_thread["scopes"][0]["thread"], "t2");
        assert!(top_scopes(&conn, &TopParams { sort: "nope".into(), ..Default::default() }).is_err());
    }

    #[test]
    fn frame_times_find_the_hitch_and_what_ran() {
        let (_d, _p, conn) = open(20, 5);
        let v = frame_times(&conn, None, None, 100, 5).unwrap();
        assert_eq!(v["hitches"], 1);
        assert_eq!(v["worst_hitches"][0]["frame"], 2);
        assert_eq!(v["worst_hitches"][0]["period_ms"], 80.0);
        assert!(v["worst_hitches"][0]["ran"].as_array().unwrap().len() >= 3);
        assert_eq!(v["frame_period_ms"]["p50"], 16.0);
    }

    #[test]
    fn the_breakdown_has_a_tree_a_critical_path_and_other_threads() {
        let (_d, _p, conn) = open(10, 4);
        let v = frame_breakdown(&conn, None, None, false, 8, 0.0).unwrap();
        assert_eq!(v["frame"]["index"], 2);
        assert_eq!(v["frame"]["cost_ms"], 40.0);
        assert_eq!(v["tree"]["children"][0]["name"], "render");
        let path: Vec<&str> = v["critical_path"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
        assert_eq!(path, ["Game::Frame", "render", "wait present"]);
        assert_eq!(v["critical_path"][2]["wait"], true);
        assert_eq!(v["other_threads"][0]["thread"], "t2");
        assert!(frame_breakdown(&conn, None, Some(99), false, 8, 0.0).is_err());
    }

    #[test]
    fn threads_gpu_latency_and_gaps() {
        let (_d, _p, conn) = open(10, 1);
        let t = threads(&conn).unwrap();
        let t1 = t["threads"].as_array().unwrap().iter().find(|x| x["thread"] == "t1").unwrap();
        assert_eq!(t1["wait_scope_self_ms"], 35.0);
        assert_eq!(t["waits"][0]["scope"], "wait present");
        let g = gpu(&conn).unwrap();
        assert_eq!(g["available"], true);
        assert_eq!(g["passes"][0]["pass"], "Lighting");
        let l = latency(&conn).unwrap();
        assert_eq!(l["samples"], 1);
        assert_eq!(l["stages"][0]["stage"], "wait_for_tick");
        assert_eq!(l["stages"][0]["mean_ms"], 8.0);
        assert_eq!(l["stages"][1]["stage"], "present");
        let gp = gaps(&conn, 1.0, 10).unwrap();
        let names: Vec<&str> = gp["mostly_own_time"].as_array().unwrap().iter().map(|r| r["scope"].as_str().unwrap()).collect();
        assert!(names.contains(&"wait present") && !names.contains(&"Game::Frame"));
    }

    #[test]
    fn compare_reports_per_frame_deltas() {
        let (_d1, _p1, a) = open(10, 1);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b.db");
        // b: the same frames with twice the physics step.
        recording(&path, 10, 1);
        {
            let w = Connection::open(&path).unwrap();
            w.execute("UPDATE profile_events SET duration_ns = 2000000 WHERE name = 'Physics::Step'", []).unwrap();
        }
        let b = open_read_only(&path).unwrap();
        let v = compare(&a, &b, 5).unwrap();
        assert_eq!(v["scopes"][0]["scope"], "Physics::Step");
        assert_eq!(v["scopes"][0]["self_ms_per_frame"]["delta"], 1.0);
    }

    #[test]
    fn sql_is_read_only_limited_and_the_schema_explains_itself() {
        let (_d, path, conn) = open(10, 1);
        let q = query(&conn, "SELECT name, COUNT(*) n FROM profile_events GROUP BY name ORDER BY n DESC", 3).unwrap();
        assert_eq!(q["row_count"], 3);
        assert_eq!(q["truncated"], true);
        assert!(query(&conn, "DELETE FROM profile_events", 10).is_err());
        assert!(query(&conn, "DROP TABLE profile_events", 10).is_err());
        assert!(query(&conn, "SELECT 1; SELECT 2", 10).is_err());
        // The connection itself refuses writes, whatever the statement says.
        assert!(conn.execute("DELETE FROM profile_events", []).is_err());
        let s = schema(&conn).unwrap();
        assert!(s["objects"].as_array().unwrap().iter().any(|o| o["name"] == "profile_events"));
        drop(conn);
        assert!(path.exists());
    }

    #[test]
    fn a_recording_is_found_by_stem_or_latest_and_exported() {
        let project = tempfile::tempdir().unwrap();
        let dir = profiling::database::ensure_profiling_dir(&project.path().display().to_string()).unwrap();
        let path = dir.join("flamegraph_2026-01-01_00-00-00.db");
        recording(&path, 5, 1);
        let p = project.path().display().to_string();
        assert_eq!(resolve(Some(&p), None).unwrap(), path);
        assert_eq!(resolve(Some(&p), Some("flamegraph_2026-01-01_00-00-00")).unwrap(), path);
        assert!(resolve(Some(&p), Some("missing")).is_err());
        assert!(resolve(None, None).is_err());
        let conn = open_read_only(&path).unwrap();
        let trace = export_chrome_trace(&conn, &path).unwrap();
        let doc: Value = serde_json::from_slice(&std::fs::read(trace).unwrap()).unwrap();
        let events = doc["traceEvents"].as_array().unwrap();
        assert!(events.iter().any(|e| e["ph"] == "M" && e["args"]["name"] == "GPU"));
        assert!(events.iter().any(|e| e["name"] == "Game::Frame"));
        assert_eq!(describe(&path)["overview"]["frames"], 5);
    }
}
