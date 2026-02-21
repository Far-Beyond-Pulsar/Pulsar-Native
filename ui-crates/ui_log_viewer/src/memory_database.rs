//! SQLite database for memory allocation tracking
//! Batches writes asynchronously to avoid blocking the allocator

use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use parking_lot::Mutex;
use crossbeam_channel::{Sender, Receiver, unbounded};
use crate::AllocationSite;

/// Batch of allocation sites to write
pub struct AllocationBatch {
    pub sites: Vec<AllocationSite>,
}

/// Background writer that batches insertions
pub struct MemoryDatabaseWriter {
    sender: Sender<AllocationBatch>,
    _handle: std::thread::JoinHandle<()>,
}

impl MemoryDatabaseWriter {
    pub fn new(db_path: PathBuf) -> rusqlite::Result<Self> {
        let (sender, receiver) = unbounded();
        
        let handle = std::thread::spawn(move || {
            if let Err(e) = writer_thread(db_path, receiver) {
                tracing::error!("[MEMORY] Database writer thread error: {}", e);
            }
        });
        
        Ok(Self {
            sender,
            _handle: handle,
        })
    }
    
    /// Send a batch of sites to be written (non-blocking)
    pub fn write_batch(&self, sites: Vec<AllocationSite>) {
        let _ = self.sender.send(AllocationBatch { sites });
    }
}

/// Writer thread that processes batches
fn writer_thread(db_path: PathBuf, receiver: Receiver<AllocationBatch>) -> rusqlite::Result<()> {
    let conn = create_memory_database(&db_path)?;
    
    // Process batches as they arrive
    while let Ok(batch) = receiver.recv() {
        save_allocation_sites(&conn, &batch.sites)?;
    }
    
    Ok(())
}

/// Create memory tracking database
fn create_memory_database(db_path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(db_path)?;
    
    conn.execute(
        "CREATE TABLE IF NOT EXISTS allocation_sites (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            type_signature TEXT NOT NULL,
            size INTEGER NOT NULL,
            align INTEGER NOT NULL,
            count INTEGER NOT NULL,
            total_bytes INTEGER NOT NULL,
            timestamp INTEGER NOT NULL
        )",
        [],
    )?;
    
    // Index for querying top allocations
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_total_bytes ON allocation_sites(total_bytes DESC)",
        [],
    )?;
    
    // Index for time-series queries
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_timestamp ON allocation_sites(timestamp)",
        [],
    )?;
    
    Ok(conn)
}

/// Save allocation sites in a transaction (batched)
fn save_allocation_sites(conn: &Connection, sites: &[AllocationSite]) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    
    let mut stmt = tx.prepare(
        "INSERT INTO allocation_sites 
         (type_signature, size, align, count, total_bytes, timestamp)
         VALUES (?, ?, ?, ?, ?, ?)"
    )?;
    
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    
    for site in sites {
        stmt.execute(params![
            site.type_signature,
            site.size as i64,
            site.align as i64,
            site.count as i64,
            site.total_bytes as i64,
            timestamp,
        ])?;
    }
    
    drop(stmt);
    tx.commit()?;
    
    Ok(())
}

/// Query top N allocations from database
pub fn query_top_allocations(db_path: &Path, limit: usize) -> rusqlite::Result<Vec<AllocationSite>> {
    let conn = Connection::open(db_path)?;
    
    let mut stmt = conn.prepare(
        "SELECT DISTINCT type_signature, size, align, count, total_bytes 
         FROM allocation_sites 
         WHERE timestamp = (SELECT MAX(timestamp) FROM allocation_sites)
         ORDER BY total_bytes DESC 
         LIMIT ?"
    )?;
    
    let sites = stmt.query_map([limit], |row| {
        Ok(AllocationSite {
            type_signature: row.get(0)?,
            size: row.get::<_, i64>(1)? as usize,
            align: row.get::<_, i64>(2)? as usize,
            count: row.get::<_, i64>(3)? as usize,
            total_bytes: row.get::<_, i64>(4)? as usize,
        })
    })?
    .collect::<Result<Vec<_>, _>>()?;
    
    Ok(sites)
}

/// Ensure memory database directory exists
pub fn ensure_memory_db_dir() -> std::io::Result<PathBuf> {
    let db_dir = std::env::current_dir()?.join(".pulsar").join("memory");
    std::fs::create_dir_all(&db_dir)?;
    Ok(db_dir)
}

/// Generate database path for current session
pub fn get_memory_db_path() -> std::io::Result<PathBuf> {
    let dir = ensure_memory_db_dir()?;
    Ok(dir.join("memory_tracking.db"))
}
