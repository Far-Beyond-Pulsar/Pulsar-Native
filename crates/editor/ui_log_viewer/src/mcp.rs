//! The engine log as MCP tools: `logs_query` and `logs_wait`.
//!
//! Both read the live log (`utils::live_logs`), the buffer the log drawer shows,
//! so an AI client sees exactly the lines a person would. They are registered
//! like any tool (`pulsar_mcp::McpToolRegistration`); the MCP server knows
//! nothing about them.
//!
//! `logs_query` answers a question about what already happened (filters, a tail,
//! a time range, a cursor). `logs_wait` is the way to watch: pass the cursor a
//! previous call returned and it returns as soon as a matching line is published
//! (or when the timeout is up), which works with every MCP client, with or
//! without notifications.

use std::collections::BTreeSet;
use std::time::Duration;

use pulsar_mcp::{Annotations, McpToolRegistration, ToolResult};
use serde_json::{Value, json};

use crate::utils::live_logs::{self, LiveEntry};

/// A log line split into its parts: `YYYY-MM-DD HH:MM:SS.mmm LEVEL target: message`.
#[derive(Debug, PartialEq, Eq)]
pub struct ParsedLine<'a> {
    pub time: &'a str,
    pub level: &'a str,
    pub target: &'a str,
    pub message: &'a str,
}

/// Split a live-log line; text that does not fit the format is all message.
pub fn parse_line(line: &str) -> ParsedLine<'_> {
    let fallback = ParsedLine {
        time: "",
        level: "",
        target: "",
        message: line,
    };
    let Some((time, rest)) = line.get(..23).zip(line.get(24..)) else {
        return fallback;
    };
    if !time.as_bytes().first().is_some_and(u8::is_ascii_digit) {
        return fallback;
    }
    let Some((level, rest)) = rest.split_once(' ') else {
        return fallback;
    };
    let (target, message) = rest.split_once(": ").unwrap_or(("", rest));
    ParsedLine {
        time,
        level,
        target,
        message,
    }
}

/// 0 = error ... 4 = trace; `None` for text that is not a level.
pub fn level_rank(level: &str) -> Option<u8> {
    match level.to_ascii_lowercase().as_str() {
        "error" => Some(0),
        "warn" | "warning" => Some(1),
        "info" => Some(2),
        "debug" => Some(3),
        "trace" => Some(4),
        _ => None,
    }
}

/// What a call asks for.
#[derive(Debug, Default)]
pub struct Filter {
    /// Show this level and the more severe.
    pub min_level: Option<u8>,
    pub target: Option<String>,
    pub text: Option<String>,
    pub exclude_text: Option<String>,
    /// Only lines emitted in these editor tasks.
    pub tasks: Option<BTreeSet<u64>>,
    /// `YYYY-MM-DD HH:MM:SS[.mmm]` local time bounds (lexicographic on the line's
    /// timestamp, which is how they sort).
    pub since: Option<String>,
    pub until: Option<String>,
}

impl Filter {
    pub fn matches(&self, entry: &LiveEntry) -> bool {
        let parsed = parse_line(&entry.line);
        if let Some(max_rank) = self.min_level {
            if level_rank(parsed.level).is_some_and(|rank| rank > max_rank) {
                return false;
            }
        }
        if let Some(target) = &self.target {
            if !parsed
                .target
                .to_ascii_lowercase()
                .contains(&target.to_ascii_lowercase())
            {
                return false;
            }
        }
        if let Some(text) = &self.text {
            if !parsed
                .message
                .to_ascii_lowercase()
                .contains(&text.to_ascii_lowercase())
            {
                return false;
            }
        }
        if let Some(text) = &self.exclude_text {
            if parsed
                .message
                .to_ascii_lowercase()
                .contains(&text.to_ascii_lowercase())
            {
                return false;
            }
        }
        if let Some(tasks) = &self.tasks {
            if !entry.task.is_some_and(|task| tasks.contains(&task)) {
                return false;
            }
        }
        if let Some(since) = &self.since {
            if parsed.time < since.as_str() {
                return false;
            }
        }
        if let Some(until) = &self.until {
            if parsed.time > until.as_str() {
                return false;
            }
        }
        true
    }

    /// Read the filter from a call's arguments.
    pub fn from_arguments(arguments: &Value) -> Result<Self, String> {
        let text = |key: &str| {
            arguments
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        };
        let min_level = match text("min_level") {
            None => None,
            Some(level) => Some(level_rank(&level).ok_or_else(|| {
                format!("`min_level` is error, warn, info, debug or trace, not `{level}`")
            })?),
        };
        let mut tasks: Option<BTreeSet<u64>> = None;
        if let Some(id) = arguments.get("task_id").and_then(Value::as_u64) {
            tasks.get_or_insert_with(BTreeSet::new).insert(id);
        }
        if let Some(title) = text("task_title") {
            let wanted = title.to_ascii_lowercase();
            let found: Vec<u64> = editor_task_queue::global()
                .snapshots()
                .into_iter()
                .filter(|task| task.title.to_ascii_lowercase().contains(&wanted))
                .map(|task| task.id.get())
                .collect();
            if found.is_empty() {
                return Err(format!(
                    "no task has a title containing `{title}`; tasks_list shows the tasks there are"
                ));
            }
            tasks.get_or_insert_with(BTreeSet::new).extend(found);
        }
        let mut since = text("since");
        if let Some(seconds) = arguments.get("since_seconds_ago").and_then(Value::as_f64) {
            let from = chrono::Local::now() - chrono::Duration::milliseconds((seconds * 1000.0) as i64);
            since = Some(from.format("%Y-%m-%d %H:%M:%S%.3f").to_string());
        }
        Ok(Self {
            min_level,
            target: text("target"),
            text: text("text"),
            exclude_text: text("exclude_text"),
            tasks,
            since,
            until: text("until"),
        })
    }
}

fn entry_json(entry: &LiveEntry) -> Value {
    let parsed = parse_line(&entry.line);
    json!({
        "seq": entry.seq,
        "time": parsed.time,
        "level": parsed.level.trim(),
        "target": parsed.target,
        "task_id": entry.task,
        "message": parsed.message,
    })
}

fn limit_of(arguments: &Value, default: usize) -> usize {
    arguments
        .get("tail")
        .or_else(|| arguments.get("max_entries"))
        .and_then(Value::as_u64)
        .map_or(default, |n| n as usize)
        .clamp(1, 5_000)
}

fn answer(read: live_logs::LiveRead, waited: bool) -> ToolResult {
    let entries: Vec<Value> = read.entries.iter().map(entry_json).collect();
    let summary = format!(
        "{} line(s); next_cursor {}{}{}",
        entries.len(),
        read.next_cursor,
        if read.dropped > 0 {
            format!("; {} older line(s) had left the buffer", read.dropped)
        } else {
            String::new()
        },
        if waited && entries.is_empty() {
            "; nothing new matched before the timeout"
        } else {
            ""
        },
    );
    let mut result = ToolResult::json(&json!({
        "summary": summary,
        "entries": entries,
        "next_cursor": read.next_cursor,
        "dropped": read.dropped,
        "buffered": read.buffered,
    }));
    // A text-only client reads the lines, not the envelope.
    if let Some(pulsar_mcp::Content::Text(text)) = result.content.first_mut() {
        let mut lines = format!("{summary}\n");
        for entry in &read.entries {
            lines.push_str(&entry.line);
            lines.push('\n');
        }
        *text = lines;
    }
    result
}

/// The filter properties both tools share, as a literal `concat!` can splice into
/// each tool's schema.
macro_rules! filter_properties {
    () => {
        r#""min_level":{"type":"string","enum":["error","warn","info","debug","trace"],"description":"Show this level and the more severe ones (warn shows warnings and errors)."},
"target":{"type":"string","description":"Keep lines whose target (the module that logged: `pulsar_package::compile`, `pack`, `game`) contains this text, case-insensitive."},
"text":{"type":"string","description":"Keep lines whose message contains this text, case-insensitive."},
"exclude_text":{"type":"string","description":"Drop lines whose message contains this text (hide noisy sources)."},
"task_id":{"type":"integer","description":"Only lines logged inside this editor task (ids from tasks_list; a build's id is in build_status)."},
"task_title":{"type":"string","description":"Only lines logged inside tasks whose title contains this text (`Build`, `Game: Run`)."},
"since":{"type":"string","description":"Local time `YYYY-MM-DD HH:MM:SS` (milliseconds optional): not older than this."},
"until":{"type":"string","description":"Local time `YYYY-MM-DD HH:MM:SS`: not newer than this."},
"since_seconds_ago":{"type":"number","description":"Not older than this many seconds."}"#
    };
}

/// `logs_query`.
fn query(arguments: &str) -> ToolResult {
    let arguments: Value = serde_json::from_str(arguments).unwrap_or_else(|_| json!({}));
    let filter = match Filter::from_arguments(&arguments) {
        Ok(filter) => filter,
        Err(message) => return ToolResult::error(message),
    };
    let cursor = arguments
        .get("after_cursor")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let tail = limit_of(&arguments, 100);
    answer(
        live_logs::read_since(cursor, tail, |entry| filter.matches(entry)),
        false,
    )
}

/// `logs_wait`.
fn wait(arguments: &str) -> ToolResult {
    let arguments: Value = serde_json::from_str(arguments).unwrap_or_else(|_| json!({}));
    let filter = match Filter::from_arguments(&arguments) {
        Ok(filter) => filter,
        Err(message) => return ToolResult::error(message),
    };
    // Without a cursor: from now on.
    let cursor = arguments
        .get("after_cursor")
        .and_then(Value::as_u64)
        .unwrap_or_else(live_logs::end_cursor);
    let timeout = arguments
        .get("timeout_seconds")
        .and_then(Value::as_f64)
        .unwrap_or(30.0)
        .clamp(0.0, 120.0);
    let limit = limit_of(&arguments, 200);
    answer(
        live_logs::wait_for_new(cursor, limit, Duration::from_secs_f64(timeout), |entry| {
            filter.matches(entry)
        }),
        true,
    )
}

pulsar_plugins::inventory::submit! {
    McpToolRegistration {
        origin: pulsar_plugins::engine_origin!(),
        name: "logs_query",
        title: "Query the engine log",
        description: "Read the engine's recent log (the buffer the Mission Control log drawer shows; the last 20000 lines). Filter by level, target (module), message text, editor task, and time, and take the newest N. Returns each line with seq, time, level, target, task_id and message, plus next_cursor: pass it back as after_cursor to read only newer lines. To watch while you act, use logs_wait.",
        input_schema: concat!(
            r#"{"type":"object","properties":{"#,
            filter_properties!(),
            r#","after_cursor":{"type":"integer","description":"Only lines after this cursor (the next_cursor of an earlier call). Omit to read from the oldest buffered line."},"tail":{"type":"integer","description":"At most this many lines, the newest ones (default 100, at most 5000)."}}}"#
        ),
        annotations: Annotations::READ_ONLY,
        call: query,
    }
}

pulsar_plugins::inventory::submit! {
    McpToolRegistration {
        origin: pulsar_plugins::engine_origin!(),
        name: "logs_wait",
        title: "Wait for new log lines",
        description: "Watch the engine log: returns as soon as a line matching the filter is published after the cursor, or when timeout_seconds is up (then with no lines). Pass the next_cursor of a previous logs_query or logs_wait as after_cursor to continue exactly where it stopped; omit it to wait only for lines from now on. Typical use: start a build or an action, then call logs_wait with target or task_title filters to follow its output.",
        input_schema: concat!(
            r#"{"type":"object","properties":{"#,
            filter_properties!(),
            r#","after_cursor":{"type":"integer","description":"Wait for lines after this cursor (the next_cursor of an earlier logs_query or logs_wait). Omit to wait for lines published from now on."},"timeout_seconds":{"type":"number","description":"Give up after this long (default 30, at most 120); the call then returns no lines and a cursor to continue from."},"max_entries":{"type":"integer","description":"At most this many lines (default 200)."}}}"#
        ),
        annotations: Annotations::READ_ONLY,
        call: wait,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(line: &str, task: Option<u64>) -> LiveEntry {
        LiveEntry {
            seq: 0,
            line: line.to_owned(),
            task,
        }
    }

    #[test]
    fn a_line_splits_into_time_level_target_and_message() {
        let parsed =
            parse_line("2026-11-23 21:43:21.393 WARN pack: scene/default.level: asset `None` not found");
        assert_eq!(parsed.time, "2026-11-23 21:43:21.393");
        assert_eq!(parsed.level, "WARN");
        assert_eq!(parsed.target, "pack");
        assert_eq!(parsed.message, "scene/default.level: asset `None` not found");
        // Text that is not a log line is all message.
        assert_eq!(parse_line("script errors:").message, "script errors:");
        assert_eq!(parse_line("  error [A] x").target, "");
    }

    #[test]
    fn filters_combine() {
        let lines = [
            entry("2026-11-23 21:00:00.000 INFO pack: Packed 1 of 2 MiB", Some(7)),
            entry("2026-11-23 21:00:01.000 WARN pack: asset not found", Some(7)),
            entry("2026-11-23 21:00:02.000 ERROR game: crashed", Some(9)),
            entry("2026-11-23 21:00:03.000 DEBUG graphy::logging: routing", None),
        ];
        let keep = |filter: &Filter| -> Vec<usize> {
            lines
                .iter()
                .enumerate()
                .filter(|(_, e)| filter.matches(e))
                .map(|(i, _)| i)
                .collect()
        };
        let by_level = Filter {
            min_level: level_rank("warn"),
            ..Filter::default()
        };
        assert_eq!(keep(&by_level), [1, 2], "warn and worse");
        let by_target = Filter {
            target: Some("PACK".into()),
            ..Filter::default()
        };
        assert_eq!(keep(&by_target), [0, 1], "case-insensitive target");
        let by_task = Filter {
            tasks: Some([9].into()),
            ..Filter::default()
        };
        assert_eq!(keep(&by_task), [2]);
        let by_text = Filter {
            text: Some("not found".into()),
            exclude_text: Some("zzz".into()),
            ..Filter::default()
        };
        assert_eq!(keep(&by_text), [1]);
        let by_time = Filter {
            since: Some("2026-11-23 21:00:01".into()),
            until: Some("2026-11-23 21:00:02.500".into()),
            ..Filter::default()
        };
        assert_eq!(keep(&by_time), [1, 2], "an inclusive time range");
        assert_eq!(keep(&Filter::default()), [0, 1, 2, 3]);
    }

    #[test]
    fn bad_arguments_say_what_is_allowed() {
        let error = Filter::from_arguments(&json!({"min_level": "loud"})).unwrap_err();
        assert!(error.contains("error, warn, info, debug or trace"), "{error}");
        let error =
            Filter::from_arguments(&json!({"task_title": "no such task title anywhere"})).unwrap_err();
        assert!(error.contains("tasks_list"), "{error}");
    }

    #[test]
    fn the_tools_answer_with_a_cursor_and_wait_for_new_lines() {
        let start = live_logs::end_cursor();
        live_logs::publish_live_log("2026-11-23 21:00:00.000 INFO mcp_test: first line");
        let result = query(&json!({"after_cursor": start, "text": "first line"}).to_string());
        let value: Value = serde_json::from_str(result.structured.as_deref().unwrap()).unwrap();
        assert_eq!(value["entries"].as_array().unwrap().len(), 1);
        let cursor = value["next_cursor"].as_u64().unwrap();
        // Nothing new: the wait times out empty with a cursor to continue from.
        let waited = wait(
            &json!({"after_cursor": cursor, "text": "second line", "timeout_seconds": 0.2}).to_string(),
        );
        let value: Value = serde_json::from_str(waited.structured.as_deref().unwrap()).unwrap();
        assert!(value["entries"].as_array().unwrap().is_empty());
        live_logs::publish_live_log("2026-11-23 21:00:01.000 INFO mcp_test: second line");
        let waited = wait(
            &json!({"after_cursor": cursor, "text": "second line", "timeout_seconds": 5}).to_string(),
        );
        let value: Value = serde_json::from_str(waited.structured.as_deref().unwrap()).unwrap();
        assert_eq!(value["entries"].as_array().unwrap().len(), 1);
    }
}
