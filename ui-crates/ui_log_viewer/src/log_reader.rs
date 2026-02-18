//! Log file reader with streaming and virtual scrolling support

use anyhow::{Context, Result};
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Represents a single log line with metadata
#[derive(Clone, Debug)]
pub struct LogLine {
    pub line_number: usize,
    pub content: String,
    pub file_offset: u64,
}

/// Log reader that streams lines from disk with windowing
pub struct LogReader {
    file_path: PathBuf,
    file: Arc<Mutex<BufReader<File>>>,
    total_lines: usize,
    line_index: Vec<u64>, // Byte offsets for each line
    window_size: usize,
}

impl LogReader {
    /// Create a new log reader for the given file
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = File::open(&path)
            .with_context(|| format!("Failed to open log file: {}", path.display()))?;
        let reader = BufReader::new(file);
        
        let mut log_reader = Self {
            file_path: path,
            file: Arc::new(Mutex::new(reader)),
            total_lines: 0,
            line_index: Vec::new(),
            window_size: 500, // Keep 500 lines in memory by default
        };
        
        // Build line index
        log_reader.build_index()?;
        
        Ok(log_reader)
    }
    
    /// Build an index of line offsets for fast random access
    fn build_index(&mut self) -> Result<()> {
        let file = File::open(&self.file_path)?;
        let mut reader = BufReader::new(file);
        
        let mut offset = 0u64;
        let mut line = String::new();
        
        self.line_index.clear();
        self.line_index.push(0); // First line at offset 0
        
        loop {
            line.clear();
            let bytes_read = reader.read_line(&mut line)?;
            if bytes_read == 0 {
                break;
            }
            offset += bytes_read as u64;
            self.line_index.push(offset);
        }
        
        self.total_lines = if self.line_index.len() > 0 {
            self.line_index.len() - 1
        } else {
            0
        };
        
        Ok(())
    }
    
    /// Get total number of lines in the log file
    pub fn total_lines(&self) -> usize {
        self.total_lines
    }
    
    /// Get the log file path
    pub fn file_path(&self) -> &Path {
        &self.file_path
    }
    
    /// Read a range of lines (start_line..end_line)
    pub fn read_lines(&self, start_line: usize, end_line: usize) -> Result<Vec<LogLine>> {
        if start_line >= self.total_lines {
            return Ok(Vec::new());
        }
        
        let end = end_line.min(self.total_lines);
        let count = end.saturating_sub(start_line);
        
        if count == 0 {
            return Ok(Vec::new());
        }
        
        // Get file offset for start line
        let start_offset = self.line_index[start_line];
        
        // Open new reader at position
        let file = File::open(&self.file_path)?;
        let mut reader = BufReader::new(file);
        reader.seek(SeekFrom::Start(start_offset))?;
        
        let mut lines = Vec::with_capacity(count);
        let mut line_buffer = String::new();
        
        for line_num in start_line..end {
            line_buffer.clear();
            let bytes_read = reader.read_line(&mut line_buffer)?;
            if bytes_read == 0 {
                break;
            }
            
            lines.push(LogLine {
                line_number: line_num + 1, // 1-indexed for display
                content: line_buffer.trim_end().to_string(),
                file_offset: self.line_index[line_num],
            });
        }
        
        Ok(lines)
    }
    
    /// Read the last N lines (for tail mode)
    pub fn read_tail(&self, count: usize) -> Result<Vec<LogLine>> {
        let start = self.total_lines.saturating_sub(count);
        self.read_lines(start, self.total_lines)
    }
    
    /// Reload the file and rebuild index (for live updates)
    pub fn reload(&mut self) -> Result<bool> {
        let old_total = self.total_lines;
        self.build_index()?;
        Ok(self.total_lines != old_total)
    }
    
    /// Get the latest log file path from the logs directory
    pub fn get_latest_log_path() -> Result<PathBuf> {
        use directories::ProjectDirs;
        
        let proj_dirs = ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
            .context("Could not determine app data directory")?;
        let logs_dir = proj_dirs.data_dir().join("logs");
        
        // Find the most recent timestamp folder
        let mut entries: Vec<_> = std::fs::read_dir(&logs_dir)
            .with_context(|| format!("Failed to read logs directory: {}", logs_dir.display()))?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
            .collect();
        
        entries.sort_by_key(|e| std::cmp::Reverse(e.file_name()));
        
        let latest_dir = entries
            .first()
            .context("No log directories found")?;
        
        let log_path = latest_dir.path().join("engine.log");
        
        if !log_path.exists() {
            anyhow::bail!("Log file does not exist: {}", log_path.display());
        }
        
        Ok(log_path)
    }
}

/// Find the latest log directory
pub fn find_latest_log_dir() -> Result<PathBuf> {
    use directories::ProjectDirs;
    
    let proj_dirs = ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
        .context("Could not determine app data directory")?;
    let logs_dir = proj_dirs.data_dir().join("logs");
    
    let mut entries: Vec<_> = std::fs::read_dir(&logs_dir)
        .with_context(|| format!("Failed to read logs directory: {}", logs_dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
        .collect();
    
    entries.sort_by_key(|e| std::cmp::Reverse(e.file_name()));
    
    let latest_dir = entries
        .first()
        .context("No log directories found")?
        .path();
    
    Ok(latest_dir)
}
