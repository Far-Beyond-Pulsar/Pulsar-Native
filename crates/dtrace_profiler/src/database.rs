//! SQLite storage for DTrace samples

use std::path::{Path, PathBuf};
use rusqlite::{Connection, params};
use anyhow::{Result, Context};
use crate::{Sample, StackFrame};

/// SQLite database for storing profiling samples
pub struct TraceDatabase {
    conn: Connection,
    db_path: PathBuf,
}

impl TraceDatabase {
    /// Create or open a trace database at the given path
    pub fn create(path: impl AsRef<Path>) -> Result<Self> {
        let db_path = path.as_ref().to_path_buf();
        let conn = Connection::open(&db_path)
            .context("Failed to open database")?;

        // Try to enable WAL mode for concurrent reads during writes
        // This is optional - if it fails, we continue with default mode
        let _ = conn.execute_batch("PRAGMA journal_mode=WAL;");

        // Create schema
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS traces (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                start_time INTEGER NOT NULL,
                end_time INTEGER,
                sample_frequency INTEGER NOT NULL,
                process_id INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS samples (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                trace_id INTEGER NOT NULL,
                timestamp_ns INTEGER NOT NULL,
                thread_id INTEGER NOT NULL,
                process_id INTEGER NOT NULL,
                FOREIGN KEY(trace_id) REFERENCES traces(id)
            );

            CREATE TABLE IF NOT EXISTS stack_frames (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                sample_id INTEGER NOT NULL,
                depth INTEGER NOT NULL,
                function_name TEXT NOT NULL,
                module_name TEXT NOT NULL,
                address INTEGER NOT NULL,
                FOREIGN KEY(sample_id) REFERENCES samples(id)
            );

            CREATE INDEX IF NOT EXISTS idx_samples_trace ON samples(trace_id, timestamp_ns);
            CREATE INDEX IF NOT EXISTS idx_frames_sample ON stack_frames(sample_id);
            CREATE INDEX IF NOT EXISTS idx_samples_timestamp ON samples(timestamp_ns);
            "#
        ).context("Failed to create schema")?;

        Ok(Self { conn, db_path })
    }

    /// Start a new trace session
    pub fn start_trace(&mut self, frequency: u32, process_id: u32) -> Result<i64> {
        let start_time = current_timestamp_ns();
        
        self.conn.execute(
            "INSERT INTO traces (start_time, sample_frequency, process_id) VALUES (?1, ?2, ?3)",
            params![start_time, frequency, process_id],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    /// End a trace session
    pub fn end_trace(&mut self, trace_id: i64) -> Result<()> {
        let end_time = current_timestamp_ns();
        
        self.conn.execute(
            "UPDATE traces SET end_time = ?1 WHERE id = ?2",
            params![end_time, trace_id],
        )?;

        Ok(())
    }

    /// Insert a batch of samples
    pub fn insert_samples(&mut self, trace_id: i64, samples: &[Sample]) -> Result<()> {
        let tx = self.conn.transaction()?;

        {
            let mut sample_stmt = tx.prepare_cached(
                "INSERT INTO samples (trace_id, timestamp_ns, thread_id, process_id) VALUES (?1, ?2, ?3, ?4)"
            )?;

            let mut frame_stmt = tx.prepare_cached(
                "INSERT INTO stack_frames (sample_id, depth, function_name, module_name, address) VALUES (?1, ?2, ?3, ?4, ?5)"
            )?;

            for sample in samples {
                sample_stmt.execute(params![
                    trace_id,
                    sample.timestamp_ns as i64,
                    sample.thread_id as i64,
                    sample.process_id as i64,
                ])?;

                let sample_id = tx.last_insert_rowid();

                for (depth, frame) in sample.stack_frames.iter().enumerate() {
                    frame_stmt.execute(params![
                        sample_id,
                        depth as i64,
                        &frame.function_name,
                        &frame.module_name,
                        frame.address as i64,
                    ])?;
                }
            }
        }

        tx.commit()?;
        Ok(())
    }

    /// Get all samples since a given timestamp
    pub fn get_samples_since(&self, trace_id: i64, since_ns: u64) -> Result<Vec<Sample>> {
        let mut samples = Vec::new();

        let mut sample_stmt = self.conn.prepare_cached(
            "SELECT id, timestamp_ns, thread_id, process_id FROM samples WHERE trace_id = ?1 AND timestamp_ns > ?2 ORDER BY timestamp_ns"
        )?;

        let mut frame_stmt = self.conn.prepare_cached(
            "SELECT depth, function_name, module_name, address FROM stack_frames WHERE sample_id = ?1 ORDER BY depth"
        )?;

        let sample_rows = sample_stmt.query_map(params![trace_id, since_ns as i64], |row| {
            Ok((
                row.get::<_, i64>(0)?, // id
                row.get::<_, i64>(1)?, // timestamp_ns
                row.get::<_, i64>(2)?, // thread_id
                row.get::<_, i64>(3)?, // process_id
            ))
        })?;

        for sample_row in sample_rows {
            let (sample_id, timestamp_ns, thread_id, process_id) = sample_row?;

            let frame_rows = frame_stmt.query_map(params![sample_id], |row| {
                Ok(StackFrame {
                    function_name: row.get(1)?,
                    module_name: row.get(2)?,
                    address: row.get::<_, i64>(3)? as u64,
                })
            })?;

            let stack_frames: Vec<StackFrame> = frame_rows
                .collect::<Result<Vec<_>, _>>()?;

            samples.push(Sample {
                thread_id: thread_id as u64,
                process_id: process_id as u64,
                timestamp_ns: timestamp_ns as u64,
                stack_frames,
                thread_name: None, // Database doesn't store thread names yet
            });
        }

        Ok(samples)
    }

    /// Get all samples for a trace
    pub fn get_all_samples(&self, trace_id: i64) -> Result<Vec<Sample>> {
        self.get_samples_since(trace_id, 0)
    }

    /// Get trace metadata
    pub fn get_trace_info(&self, trace_id: i64) -> Result<TraceInfo> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT start_time, end_time, sample_frequency, process_id FROM traces WHERE id = ?1"
        )?;

        let info = stmt.query_row(params![trace_id], |row| {
            Ok(TraceInfo {
                trace_id,
                start_time: row.get::<_, i64>(0)? as u64,
                end_time: row.get::<_, Option<i64>>(1)?.map(|t| t as u64),
                sample_frequency: row.get::<_, i64>(2)? as u32,
                process_id: row.get::<_, i64>(3)? as u32,
            })
        })?;

        Ok(info)
    }

    /// Get the database file path
    pub fn path(&self) -> &Path {
        &self.db_path
    }
}

/// Metadata about a trace session
#[derive(Debug, Clone)]
pub struct TraceInfo {
    pub trace_id: i64,
    pub start_time: u64,
    pub end_time: Option<u64>,
    pub sample_frequency: u32,
    pub process_id: u32,
}

fn current_timestamp_ns() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}
