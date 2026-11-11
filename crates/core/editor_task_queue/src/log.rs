//! Per-task logs.
//!
//! Every task owns a bounded log buffer. Lines reach it two ways, and neither
//! needs the task's code to cooperate:
//!
//! * **Spans.** Each task runs inside an `editor_task` tracing span that is
//!   re-entered on every poll of its future. [`TaskLogLayer`] routes any event
//!   emitted inside that span (or a child span) to the task's buffer, so a
//!   plain `tracing::info!` in task code lands in the right task even when many
//!   tasks interleave on one thread. A line can only be attributed to the task
//!   whose future was being polled, which is why logs cannot get mixed up.
//! * **Lifecycle lines.** The queue writes queued/started/finished/stalled
//!   lines straight into the buffer, so they exist even where no subscriber is
//!   installed (tests, tools).
//!
//! Lines use Mission Control's format (`<time> <LEVEL> <target>: <message>`),
//! so its log viewer renders them unchanged.

use crate::TaskId;
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use tracing::span;
use tracing::Subscriber;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

/// Lines kept per task. Older lines are dropped and counted.
pub const MAX_LOG_LINES: usize = 5_000;

/// Name of the span every task runs in.
pub(crate) const SPAN_NAME: &str = "editor_task";

/// Field that marks an event the queue already wrote into the task's buffer,
/// so [`TaskLogLayer`] must not add it a second time.
pub(crate) const DIRECT_FIELD: &str = "task_direct";

#[derive(Default)]
struct LogInner {
    lines: VecDeque<String>,
    /// Sequence number of `lines[0]`.
    first_seq: u64,
    next_seq: u64,
}

/// A task's bounded log buffer.
pub struct TaskLog {
    inner: Mutex<LogInner>,
    /// Milliseconds since the process epoch of the latest line.
    last_line_ms: AtomicU64,
}

/// A read of a task's log from a cursor.
#[derive(Clone, Debug, Default)]
pub struct LogRead {
    pub lines: Vec<String>,
    /// Pass this back as the cursor to read only what came after.
    pub next_cursor: u64,
    /// Lines that fell out of the buffer before they were read.
    pub dropped: u64,
}

impl TaskLog {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(LogInner::default()),
            last_line_ms: AtomicU64::new(crate::now_ms()),
        }
    }

    pub(crate) fn push(&self, line: String) {
        let mut inner = self.inner.lock();
        if inner.lines.len() >= MAX_LOG_LINES {
            inner.lines.pop_front();
            inner.first_seq += 1;
        }
        inner.lines.push_back(line);
        inner.next_seq += 1;
        drop(inner);
        self.last_line_ms.store(crate::now_ms(), Ordering::Release);
    }

    pub(crate) fn last_line_ms(&self) -> u64 {
        self.last_line_ms.load(Ordering::Acquire)
    }

    /// Total lines ever written (including dropped ones).
    pub fn total(&self) -> u64 {
        self.inner.lock().next_seq
    }

    /// Lines at or after `cursor` (a sequence number, 0 for "from the start").
    pub fn read_from(&self, cursor: u64) -> LogRead {
        let inner = self.inner.lock();
        let dropped = inner.first_seq.saturating_sub(cursor);
        let start = cursor.max(inner.first_seq);
        let skip = (start - inner.first_seq) as usize;
        LogRead {
            lines: inner.lines.iter().skip(skip).cloned().collect(),
            next_cursor: inner.next_seq,
            dropped,
        }
    }
}

type Registry = RwLock<HashMap<u64, Arc<TaskLog>>>;

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}

pub(crate) fn register(id: TaskId, log: Arc<TaskLog>) {
    registry().write().insert(id.get(), log);
}

pub(crate) fn unregister(id: TaskId) {
    registry().write().remove(&id.get());
}

fn lookup(id: u64) -> Option<Arc<TaskLog>> {
    registry().read().get(&id).cloned()
}

pub(crate) fn format_line(level: tracing::Level, target: &str, message: &str) -> String {
    format!(
        "{} {} {}: {}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
        level,
        target,
        message
    )
}

/// Collects an event's message and fields into one line body.
#[derive(Default)]
struct LineVisitor {
    message: String,
    fields: String,
    direct: bool,
}

impl LineVisitor {
    fn body(mut self) -> String {
        if !self.fields.is_empty() {
            if !self.message.is_empty() {
                self.message.push(' ');
            }
            self.message.push_str(self.fields.trim_start());
        }
        self.message
    }
}

impl tracing::field::Visit for LineVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write as _;
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            let _ = write!(self.fields, " {}={:?}", field.name(), value);
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        use std::fmt::Write as _;
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            let _ = write!(self.fields, " {}={}", field.name(), value);
        }
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        if field.name() == DIRECT_FIELD {
            self.direct = value;
        } else {
            use std::fmt::Write as _;
            let _ = write!(self.fields, " {}={}", field.name(), value);
        }
    }
}

struct TaskIdVisitor(Option<u64>);

impl tracing::field::Visit for TaskIdVisitor {
    fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        if field.name() == "task_id" {
            self.0 = Some(value);
        }
    }
}

/// Span extension that ties a span to its task's log.
struct TaskSpan(Arc<TaskLog>);

/// The tracing layer that gives every task its own log.
///
/// Install it once beside the file and live-log layers
/// (`registry().with(TaskLogLayer)`); the engine does this in its logging
/// setup. It is a no-op for events outside any task.
pub struct TaskLogLayer;

impl<S> Layer<S> for TaskLogLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        if attrs.metadata().name() != SPAN_NAME {
            return;
        }
        let mut visitor = TaskIdVisitor(None);
        attrs.record(&mut visitor);
        let Some(task_id) = visitor.0 else { return };
        let Some(log) = lookup(task_id) else { return };
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(TaskSpan(log));
        }
    }

    fn on_event(&self, event: &tracing::Event<'_>, ctx: Context<'_, S>) {
        let Some(scope) = ctx.event_scope(event) else {
            return;
        };
        // Leaf to root: the innermost task span owns the line.
        for span in scope {
            let extensions = span.extensions();
            let Some(task) = extensions.get::<TaskSpan>() else {
                continue;
            };
            let mut visitor = LineVisitor::default();
            event.record(&mut visitor);
            if visitor.direct {
                return;
            }
            let meta = event.metadata();
            task.0
                .push(format_line(*meta.level(), meta.target(), &visitor.body()));
            return;
        }
    }
}
