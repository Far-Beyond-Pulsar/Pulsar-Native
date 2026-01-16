//! Logging setup for Pulsar Engine
//
// This module provides logging initialization, formatting, and log file management for the engine.
// It supports colored console output, file logging, and environment-based log filtering.
//
// Usage:
//   Call `logging::init(verbose)` at the start of main().
//   Keep the returned guard alive for the program's duration.

use directories::ProjectDirs;
use chrono::Local;
use serde_json;
use std::fs;
use tracing_subscriber::fmt::{
    format::{FormatEvent, FormatFields, Writer},
    FmtContext,
};
use tracing_subscriber::registry::LookupSpan;
use tracing::Subscriber;

#[allow(dead_code)]
pub struct LogGuard(tracing_appender::non_blocking::WorkerGuard);

/// Initializes logging for the engine.
///
/// - `verbose`: If true, enables colored console output with detailed formatting.
/// - Returns: LogGuard, which must be kept alive for file logging.
pub fn init(verbose: bool) -> LogGuard {
    // --- Logging directory setup ---
    let proj_dirs = ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
        .expect("Could not determine app data directory");
    let appdata_dir = proj_dirs.data_dir();
    let logs_dir = appdata_dir.join("logs");
    if let Err(e) = fs::create_dir_all(&logs_dir) {
        tracing::error!("[Engine] Failed to create logs directory: {e}");
    }
    let now = Local::now();
    let log_folder = logs_dir.join(format!("{}", now.format("%Y-%m-%d_%H-%M-%S")));
    if let Err(e) = fs::create_dir_all(&log_folder) {
        tracing::error!("[Engine] Failed to create log timestamp folder: {e}");
    }
    let engine_log_path = log_folder.join("engine.log");

    // File appender for engine.log
    let engine_log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&engine_log_path)
        .expect("Failed to open engine.log for writing");
    let (non_blocking, guard) = tracing_appender::non_blocking(engine_log_file);

    // Set up tracing subscriber with file output (engine.log) and console
    use tracing_subscriber::prelude::*;
    let rust_log = std::env::var("RUST_LOG").ok();
    let env_filter = match rust_log {
        Some(val) => tracing_subscriber::EnvFilter::new(val),
        None => tracing_subscriber::EnvFilter::new("info,wgpu_hal=warn,wgpu_core=warn,naga=warn"),
    };
    // File log: plain formatting, no ANSI/color codes
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(true);

    let registry = tracing_subscriber::registry()
        .with(env_filter)
        .with(file_layer);

    if verbose {
        // Console log: keep GorgeousFormatter (with color)
        let console_layer = tracing_subscriber::fmt::layer()
            .with_writer(std::io::stdout)
            .with_target(true)
            .with_thread_ids(true)
            .event_format(GorgeousFormatter);
        registry.with(console_layer).init();
    } else {
        registry.init();
    }

    LogGuard(guard)
}

/// Custom event formatter for colored, pretty console output.
pub struct GorgeousFormatter;

impl<S, N> FormatEvent<S, N> for GorgeousFormatter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> std::fmt::Result {
        use std::fmt::Write as _;
        let meta = event.metadata();
        let level = *meta.level();
        let now = chrono::Local::now();
        // Elegant, dark-friendly, harmonious colors
        let (level_str, level_color) = match level {
            tracing::Level::ERROR => ("ERROR", "\x1b[1;91m"), // Bold Red
            tracing::Level::WARN => ("WARN ", "\x1b[1;93m"), // Bold Yellow
            tracing::Level::INFO => ("INFO ", "\x1b[1;94m"), // Bold Blue
            tracing::Level::DEBUG => ("DEBUG", "\x1b[1;92m"), // Bold Green
            tracing::Level::TRACE => ("TRACE", "\x1b[1;95m"), // Bold Magenta
        };
        // Timestamp: dim cyan
        write!(writer, "\x1b[2;36m{}\x1b[0m ", now.format("%Y-%m-%d %H:%M:%S"))?;
        // Level: bold, colored, padded
        write!(writer, "{}{}\x1b[0m ", level_color, level_str)?;
        // Thread ID: dim magenta
        let thread = std::thread::current();
        let thread_id = format!("{:?}", thread.id());
        write!(writer, "\x1b[2;35m[{}]\x1b[0m ", thread_id)?;
        // Target: dim yellow, underlined
        write!(writer, "\x1b[4;2;33m{}\x1b[0m: ", meta.target())?;

        // Capture the message into a string using a visitor
        struct MsgVisitor(String);
        impl tracing_subscriber::field::Visit for MsgVisitor {
            fn record_debug(
                &mut self,
                _field: &tracing::field::Field,
                value: &dyn std::fmt::Debug,
            ) {
                if !self.0.is_empty() {
                    self.0.push(' ');
                }
                use std::fmt::Write;
                let _ = write!(self.0, "{:?}", value);
            }
            fn record_str(&mut self, _field: &tracing::field::Field, value: &str) {
                if !self.0.is_empty() {
                    self.0.push(' ');
                }
                self.0.push_str(value);
            }
        }
        let mut visitor = MsgVisitor(String::new());
        event.record(&mut visitor);
        let msg_buf = visitor.0.trim();

        // Try to pretty-print and colorize JSON if possible, even if embedded
        let mut highlighted = false;
        if let Some(start) = msg_buf.find(|c| c == '{' || c == '[') {
            let (prefix, json_candidate) = msg_buf.split_at(start);
            if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(json_candidate) {
                // Print prefix as normal
                write!(writer, " {}\n", prefix.trim_end())?;
                fn color_json(val: &serde_json::Value, buf: &mut String, indent: usize) {
                    match val {
                        serde_json::Value::Object(map) => {
                            buf.push_str("{\n");
                            let len = map.len();
                            for (i, (k, v)) in map.iter().enumerate() {
                                buf.push_str(&"  ".repeat(indent + 1));
                                let _ = write!(buf, "\x1b[36m\"{}\"\x1b[0m: ", k);
                                color_json(v, buf, indent + 1);
                                if i + 1 != len {
                                    buf.push(',');
                                }
                                buf.push('\n');
                            }
                            buf.push_str(&"  ".repeat(indent));
                            buf.push('}');
                        }
                        serde_json::Value::Array(arr) => {
                            buf.push_str("[\n");
                            let len = arr.len();
                            for (i, v) in arr.iter().enumerate() {
                                buf.push_str(&"  ".repeat(indent + 1));
                                color_json(v, buf, indent + 1);
                                if i + 1 != len {
                                    buf.push(',');
                                }
                                buf.push('\n');
                            }
                            buf.push_str(&"  ".repeat(indent));
                            buf.push(']');
                        }
                        serde_json::Value::String(s) => {
                            let _ = write!(buf, "\x1b[32m\"{}\"\x1b[0m", s); // Green
                        }
                        serde_json::Value::Number(n) => {
                            let _ = write!(buf, "\x1b[33m{}\x1b[0m", n); // Yellow
                        }
                        serde_json::Value::Bool(b) => {
                            let _ = write!(buf, "\x1b[35m{}\x1b[0m", b); // Magenta
                        }
                        serde_json::Value::Null => {
                            buf.push_str("\x1b[90mnull\x1b[0m"); // Bright black
                        }
                    }
                }
                let mut json_buf = String::new();
                color_json(&json_val, &mut json_buf, 0);
                write!(writer, "{}", json_buf)?;
                highlighted = true;
            }
        }
        if !highlighted {
            // Not JSON, or no JSON found, print as normal
            write!(writer, " {}", msg_buf)?;
        }
        writeln!(writer)
    }
}
