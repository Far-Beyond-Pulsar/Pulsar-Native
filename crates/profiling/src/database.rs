//! SQLite database storage for profiling events
//! Saves profiling sessions to .pulsar/profiling/flamegraph/ in the project directory

use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};
use crate::ProfileEvent;

/// Create the profiling database directory if it doesn't exist
pub fn ensure_profiling_dir(project_path: &str) -> std::io::Result<PathBuf> {
    let profiling_dir = Path::new(project_path)
        .join(".pulsar")
        .join("profiling")
        .join("flamegraph");
    
    std::fs::create_dir_all(&profiling_dir)?;
    Ok(profiling_dir)
}

/// Generate a database filename with timestamp
/// Format: "flamegraph_YYYY-MM-DD_HH-MM-SS.db"
pub fn generate_db_filename() -> String {
    let now = chrono::Local::now();
    format!("flamegraph_{}.db", now.format("%Y-%m-%d_%H-%M-%S"))
}

/// Create a new profiling database and initialize schema
pub fn create_database(db_path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(db_path)?;
    
    // Create schema
    conn.execute(
        "CREATE TABLE IF NOT EXISTS profile_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            thread_id INTEGER NOT NULL,
            thread_name TEXT,
            process_id INTEGER NOT NULL,
            parent_name TEXT,
            start_ns INTEGER NOT NULL,
            duration_ns INTEGER NOT NULL,
            depth INTEGER NOT NULL,
            location TEXT,
            metadata TEXT
        )",
        [],
    )?;
    
    // Create indices for fast queries
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_thread_time ON profile_events(thread_id, start_ns)",
        [],
    )?;
    
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_time ON profile_events(start_ns)",
        [],
    )?;
    
    // Store session metadata
    conn.execute(
        "CREATE TABLE IF NOT EXISTS session_metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )",
        [],
    )?;
    
    let now = chrono::Local::now();
    conn.execute(
        "INSERT OR REPLACE INTO session_metadata (key, value) VALUES (?, ?)",
        params!["timestamp", now.to_rfc3339()],
    )?;
    
    conn.execute(
        "INSERT OR REPLACE INTO session_metadata (key, value) VALUES (?, ?)",
        params!["version", env!("CARGO_PKG_VERSION")],
    )?;
    
    Ok(conn)
}

/// Save events to database
pub fn save_events(conn: &Connection, events: &[ProfileEvent]) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    
    let mut stmt = tx.prepare(
        "INSERT INTO profile_events 
         (name, thread_id, thread_name, process_id, parent_name, start_ns, duration_ns, depth, location, metadata)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
    )?;
    
    for event in events {
        stmt.execute(params![
            event.name,
            event.thread_id as i64,
            event.thread_name,
            event.process_id,
            event.parent_name,
            event.start_ns as i64,
            event.duration_ns as i64,
            event.depth,
            event.location,
            event.metadata,
        ])?;
    }
    
    drop(stmt);
    tx.commit()?;
    
    Ok(())
}

/// Load all events from database
pub fn load_events(conn: &Connection) -> rusqlite::Result<Vec<ProfileEvent>> {
    let mut stmt = conn.prepare(
        "SELECT name, thread_id, thread_name, process_id, parent_name, start_ns, duration_ns, depth, location, metadata
         FROM profile_events
         ORDER BY start_ns"
    )?;
    
    let events = stmt.query_map([], |row| {
        Ok(ProfileEvent {
            name: row.get(0)?,
            thread_id: row.get::<_, i64>(1)? as u64,
            thread_name: row.get(2)?,
            process_id: row.get(3)?,
            parent_name: row.get(4)?,
            start_ns: row.get::<_, i64>(5)? as u64,
            duration_ns: row.get::<_, i64>(6)? as u64,
            depth: row.get(7)?,
            location: row.get(8)?,
            metadata: row.get(9)?,
        })
    })?.collect::<rusqlite::Result<Vec<_>>>()?;
    
    Ok(events)
}

/// List all profiling databases in the project
pub fn list_profiling_sessions(project_path: &str) -> std::io::Result<Vec<PathBuf>> {
    let profiling_dir = Path::new(project_path)
        .join(".pulsar")
        .join("profiling")
        .join("flamegraph");
    
    if !profiling_dir.exists() {
        return Ok(Vec::new());
    }
    
    let mut sessions = Vec::new();
    for entry in std::fs::read_dir(&profiling_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("db") {
            sessions.push(path);
        }
    }
    
    // Sort by modification time (newest first)
    sessions.sort_by(|a, b| {
        let a_time = std::fs::metadata(a).and_then(|m| m.modified()).ok();
        let b_time = std::fs::metadata(b).and_then(|m| m.modified()).ok();
        b_time.cmp(&a_time)
    });
    
    Ok(sessions)
}
