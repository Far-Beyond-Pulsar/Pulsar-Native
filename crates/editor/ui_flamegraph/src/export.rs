//! Exporting a session: the statistics table as CSV, the whole trace as a
//! Chrome trace (`chrome://tracing`, Perfetto, Speedscope all open it).

use crate::analysis::{FunctionStats, aggregate, compute_self_times};
use crate::trace_data::TraceFrame;
use std::fmt::Write as _;

fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

fn ms(ns: u64) -> String {
    format!("{:.6}", ns as f64 / 1_000_000.0)
}

/// The statistics table, sorted by total time, as CSV.
pub fn stats_csv(frame: &TraceFrame) -> String {
    let self_ns = compute_self_times(frame);
    let mut stats: Vec<FunctionStats> = aggregate(frame, &self_ns);
    stats.sort_by(|a, b| b.total_ns.cmp(&a.total_ns).then_with(|| a.name.cmp(&b.name)));

    let mut out = String::from("Function,Calls,Total (ms),Self (ms),Avg (ms),Min (ms),Max (ms)\n");
    for stat in &stats {
        let _ = writeln!(
            out,
            "{},{},{},{},{},{},{}",
            csv_field(&stat.name),
            stat.calls,
            ms(stat.total_ns),
            ms(stat.self_ns),
            ms(stat.avg_ns()),
            ms(stat.min_ns),
            ms(stat.max_ns),
        );
    }
    out
}

fn json_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string())
}

/// The trace in Chrome's trace event format: one complete ("X") event per
/// span, timestamps in microseconds from the start of the session, one track
/// per thread, plus thread names and frame markers.
pub fn chrome_trace(frame: &TraceFrame) -> String {
    let mut out = String::with_capacity(frame.spans.len() * 96 + 256);
    out.push_str("{\"displayTimeUnit\":\"ms\",\"traceEvents\":[");
    let mut first = true;
    let mut separator = |out: &mut String| {
        if first {
            first = false;
        } else {
            out.push(',');
        }
    };

    let mut threads: Vec<_> = frame.threads.values().collect();
    threads.sort_by_key(|thread| thread.id);
    for thread in threads {
        separator(&mut out);
        let _ = write!(
            out,
            "{{\"name\":\"thread_name\",\"ph\":\"M\",\"pid\":1,\"tid\":{},\"args\":{{\"name\":{}}}}}",
            thread.id,
            json_string(&thread.name)
        );
    }

    let origin = frame.min_time_ns;
    let micros = |ns: u64| ns as f64 / 1000.0;
    for span in &frame.spans {
        separator(&mut out);
        let _ = write!(
            out,
            "{{\"name\":{},\"cat\":\"span\",\"ph\":\"X\",\"ts\":{:.3},\"dur\":{:.3},\"pid\":1,\"tid\":{}}}",
            json_string(&span.name),
            micros(span.start_ns.saturating_sub(origin)),
            micros(span.duration_ns),
            span.thread_id
        );
    }

    for (index, boundary) in frame.frame_boundaries_ns.iter().enumerate() {
        separator(&mut out);
        let _ = write!(
            out,
            "{{\"name\":\"Frame {}\",\"cat\":\"frame\",\"ph\":\"i\",\"s\":\"g\",\"ts\":{:.3},\"pid\":1,\"tid\":0}}",
            index,
            micros(boundary.saturating_sub(origin))
        );
    }
    out.push_str("]}");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace_data::TraceSpan;

    fn sample() -> TraceFrame {
        let mut threads = std::collections::HashMap::new();
        threads.insert(7u64, "Main, \"render\"".to_string());
        let spans = vec![
            TraceSpan {
                name: "update".into(),
                start_ns: 1_000_000,
                duration_ns: 2_000_000,
                depth: 0,
                thread_id: 7,
                color_index: 0,
            },
            TraceSpan {
                name: "physics, step".into(),
                start_ns: 1_500_000,
                duration_ns: 500_000,
                depth: 1,
                thread_id: 7,
                color_index: 0,
            },
        ];
        TraceFrame::with_data(spans, threads)
    }

    #[test]
    fn csv_quotes_fields_and_reports_self_time() {
        let csv = stats_csv(&sample());
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "Function,Calls,Total (ms),Self (ms),Avg (ms),Min (ms),Max (ms)");
        assert_eq!(lines[1], "update,1,2.000000,1.500000,2.000000,2.000000,2.000000");
        assert!(lines[2].starts_with("\"physics, step\",1,0.500000"));
    }

    #[test]
    fn chrome_trace_is_valid_json_with_relative_microseconds() {
        let trace = chrome_trace(&sample());
        let value: serde_json::Value = serde_json::from_str(&trace).expect("valid json");
        let events = value["traceEvents"].as_array().expect("events");
        let name = events.iter().find(|e| e["ph"] == "M").expect("thread name");
        assert_eq!(name["args"]["name"], "Main, \"render\"");
        let update = events.iter().find(|e| e["name"] == "update").expect("span");
        assert_eq!(update["ts"].as_f64(), Some(0.0));
        assert_eq!(update["dur"].as_f64(), Some(2000.0));
        assert_eq!(update["tid"], 7);
    }
}
