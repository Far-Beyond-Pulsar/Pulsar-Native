//! Per-chat store of child runs. One file per chat, next to the chat file, so
//! a sub-agent's full transcript survives a restart and `read_run` can page
//! through it. The main chat's own ledger is not stored here.

use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Condvar, Mutex, MutexGuard,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::ledger::{EntryKind, Ledger, SubagentStatus};
use crate::run::CancelToken;

/// Characters kept from one tool result in a stored transcript line.
const TRANSCRIPT_RESULT_CHARS: usize = 4_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Pending,
    Running,
    Success,
    Error,
    Cancelled,
}

impl RunStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(self, RunStatus::Pending | RunStatus::Running)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            RunStatus::Pending => "pending",
            RunStatus::Running => "running",
            RunStatus::Success => "success",
            RunStatus::Error => "error",
            RunStatus::Cancelled => "cancelled",
        }
    }

    /// `None` while the run is still going.
    pub fn report_status(self) -> Option<SubagentStatus> {
        match self {
            RunStatus::Success => Some(SubagentStatus::Success),
            RunStatus::Error => Some(SubagentStatus::Error),
            RunStatus::Cancelled => Some(SubagentStatus::Cancelled),
            RunStatus::Pending | RunStatus::Running => None,
        }
    }
}

/// The structured result of a child run. Only facts: nothing in it addresses
/// the reader.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildReport {
    pub summary: String,
    pub findings: Vec<String>,
    pub refs: Vec<String>,
    pub open_questions: Vec<String>,
    /// Sections of a custom `result_schema` beyond the four standard ones.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<(String, String)>,
    /// Set when the report still failed its harness's `verify:` checks after
    /// one retry; holds the reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unverified: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: String,
    pub parent: Option<String>,
    pub harness: String,
    pub task: String,
    pub model: String,
    pub depth: u32,
    pub status: RunStatus,
    pub background: bool,
    pub created_at_ms: u64,
    pub started_at_ms: Option<u64>,
    pub finished_at_ms: Option<u64>,
    /// Latest progress note ("running list_files").
    #[serde(default)]
    pub progress: String,
    pub report: Option<ChildReport>,
    pub error: Option<String>,
    /// One line per ledger entry of the child; see [`render_transcript`].
    #[serde(default)]
    pub transcript: Vec<String>,
    /// The parent has seen the report (as a tool result or a ledger entry).
    #[serde(default)]
    pub delivered: bool,
}

impl RunRecord {
    pub fn is_active(&self) -> bool {
        !self.status.is_terminal()
    }
}

/// Why [`RunStore::insert_checked`] said no.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InsertRefused {
    FanOut { limit: usize },
    Total { limit: usize },
}

#[derive(Serialize, Deserialize)]
struct StoreFile {
    version: u32,
    runs: Vec<RunRecord>,
}

#[derive(Default)]
struct Inner {
    runs: Vec<RunRecord>,
}

pub struct RunStore {
    path: Option<PathBuf>,
    inner: Mutex<Inner>,
    changed: Condvar,
}

static RUN_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl RunStore {
    /// A store that is never written to disk.
    pub fn in_memory() -> Arc<Self> {
        Arc::new(Self {
            path: None,
            inner: Mutex::new(Inner::default()),
            changed: Condvar::new(),
        })
    }

    /// The file that goes next to `<chat_id>.json`.
    pub fn path_for_chat(chats_dir: &std::path::Path, chat_id: &str) -> PathBuf {
        chats_dir.join(format!("{chat_id}.runs"))
    }

    /// Load `path` if it exists. Runs that were still going when the file was
    /// written belong to a process that is gone, so they load as cancelled.
    pub fn open(path: PathBuf) -> Arc<Self> {
        let mut runs: Vec<RunRecord> = fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<StoreFile>(&raw).ok())
            .map(|file| file.runs)
            .unwrap_or_default();
        for run in &mut runs {
            if run.is_active() {
                run.status = RunStatus::Cancelled;
                run.finished_at_ms = Some(now_ms());
                run.error = Some("Interrupted: the editor closed before this run finished.".to_string());
                run.delivered = false;
                run.report = Some(ChildReport {
                    summary: "The run was interrupted before it finished.".to_string(),
                    ..ChildReport::default()
                });
            }
        }
        Arc::new(Self {
            path: Some(path),
            inner: Mutex::new(Inner { runs }),
            changed: Condvar::new(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn next_id(&self) -> String {
        let seq = RUN_SEQUENCE.fetch_add(1, Ordering::SeqCst);
        format!("run-{}-{seq}", now_ms())
    }

    pub fn insert(&self, record: RunRecord) {
        self.lock().runs.push(record);
        self.changed.notify_all();
    }

    /// Insert unless the parent already has `per_parent` children or the
    /// store already holds `total` runs. The checks and the insert are one
    /// step, so concurrent spawns cannot overshoot.
    pub fn insert_checked(
        &self,
        record: RunRecord,
        per_parent: usize,
        total: usize,
    ) -> Result<(), InsertRefused> {
        {
            let mut guard = self.lock();
            if let Some(parent) = &record.parent {
                let children = guard
                    .runs
                    .iter()
                    .filter(|r| r.parent.as_ref() == Some(parent))
                    .count();
                if children >= per_parent {
                    return Err(InsertRefused::FanOut { limit: per_parent });
                }
            }
            if guard.runs.len() >= total {
                return Err(InsertRefused::Total { limit: total });
            }
            guard.runs.push(record);
        }
        self.changed.notify_all();
        Ok(())
    }

    /// Mutate a record and wake anyone waiting on it. Call [`flush`] at
    /// points worth keeping (spawn, finish).
    ///
    /// [`flush`]: RunStore::flush
    pub fn update<R>(&self, id: &str, f: impl FnOnce(&mut RunRecord) -> R) -> Option<R> {
        let result = {
            let mut guard = self.lock();
            guard.runs.iter_mut().find(|r| r.id == id).map(f)
        };
        self.changed.notify_all();
        result
    }

    pub fn get(&self, id: &str) -> Option<RunRecord> {
        self.lock().runs.iter().find(|r| r.id == id).cloned()
    }

    /// Creation order.
    pub fn list(&self) -> Vec<RunRecord> {
        self.lock().runs.clone()
    }

    pub fn len(&self) -> usize {
        self.lock().runs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lock().runs.is_empty()
    }

    pub fn active(&self) -> Vec<RunRecord> {
        self.lock().runs.iter().filter(|r| r.is_active()).cloned().collect()
    }

    pub fn children_of(&self, parent: &str) -> Vec<RunRecord> {
        self.lock()
            .runs
            .iter()
            .filter(|r| r.parent.as_deref() == Some(parent))
            .cloned()
            .collect()
    }

    pub fn child_count(&self, parent: &str) -> usize {
        self.lock()
            .runs
            .iter()
            .filter(|r| r.parent.as_deref() == Some(parent))
            .count()
    }

    /// Finished children of `parent` whose report the parent has not seen,
    /// oldest finish first. Marks them delivered.
    pub fn take_undelivered(&self, parent: &str) -> Vec<RunRecord> {
        let mut taken: Vec<RunRecord> = {
            let mut guard = self.lock();
            guard
                .runs
                .iter_mut()
                .filter(|r| {
                    r.parent.as_deref() == Some(parent) && r.status.is_terminal() && !r.delivered
                })
                .map(|r| {
                    r.delivered = true;
                    r.clone()
                })
                .collect()
        };
        taken.sort_by_key(|r| r.finished_at_ms.unwrap_or(0));
        if !taken.is_empty() {
            self.flush();
        }
        taken
    }

    /// Block until `id` is terminal. Returns `None` if `cancel` fires first
    /// or the run does not exist.
    pub fn wait_terminal(&self, id: &str, cancel: &CancelToken) -> Option<RunRecord> {
        let mut guard = self.lock();
        loop {
            match guard.runs.iter().find(|r| r.id == id) {
                None => return None,
                Some(run) if run.status.is_terminal() => return Some(run.clone()),
                Some(_) => {}
            }
            if cancel.is_cancelled() {
                return None;
            }
            guard = match self.changed.wait_timeout(guard, Duration::from_millis(100)) {
                Ok((guard, _)) => guard,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
    }

    /// Lines `[start, end)` of a run's transcript, plus the total count.
    pub fn read_transcript(&self, id: &str, start: usize, end: usize) -> Option<(Vec<String>, usize)> {
        let guard = self.lock();
        let run = guard.runs.iter().find(|r| r.id == id)?;
        let total = run.transcript.len();
        let start = start.min(total);
        let end = end.clamp(start, total);
        Some((run.transcript[start..end].to_vec(), total))
    }

    /// Write the store next to its chat. Failures are logged, not fatal.
    pub fn flush(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let file = StoreFile {
            version: 1,
            runs: self.lock().runs.clone(),
        };
        let write = serde_json::to_string(&file)
            .map_err(|err| err.to_string())
            .and_then(|json| {
                if file.runs.is_empty() && !path.exists() {
                    return Ok(());
                }
                let tmp = path.with_extension("runs.tmp");
                fs::write(&tmp, json)
                    .and_then(|_| fs::rename(&tmp, path))
                    .map_err(|err| err.to_string())
            });
        if let Err(err) = write {
            tracing::error!("[agent_runtime] could not save run store {}: {err}", path.display());
        }
    }
}

/// One line per ledger entry, for `read_run`. Long tool results are cut.
pub fn render_transcript(ledger: &Ledger) -> Vec<String> {
    ledger
        .entries
        .iter()
        .enumerate()
        .filter_map(|(ix, entry)| {
            let line = match &entry.kind {
                EntryKind::System { .. } | EntryKind::Pinned { .. } => return None,
                EntryKind::UserTurn { text } => format!("[{ix}] task: {text}"),
                EntryKind::AssistantTurn { text } => {
                    if text.trim().is_empty() {
                        return None;
                    }
                    format!("[{ix}] assistant: {text}")
                }
                EntryKind::ToolExchange(exchange) => format!(
                    "[{ix}] tool {} {}\n  -> {}{}",
                    exchange.call.name,
                    exchange.call.arguments_json,
                    cut(&exchange.result.content, TRANSCRIPT_RESULT_CHARS),
                    if exchange.result.is_error { " (error)" } else { "" },
                ),
                EntryKind::SubagentReport(report) => {
                    format!("[{ix}] sub-agent {} ({}): {}", report.run_id, report.status.as_str(), report.summary)
                }
                EntryKind::EngineNotice { text } => format!("[{ix}] engine: {text}"),
                EntryKind::Summary { text, .. } => format!("[{ix}] summary of earlier steps: {text}"),
            };
            Some(line)
        })
        .collect()
}

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max).collect();
    format!("{kept}... [cut, {} chars total]", text.chars().count())
}
