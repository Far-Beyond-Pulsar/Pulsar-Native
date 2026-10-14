//! User confirmation of tool calls, plus the per-run safety state the loop
//! consults before and after every tool call.

use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use agent_chat_core::ToolCall;

use crate::{
    checkpoint::CheckpointStore,
    ledger::{Ledger, LedgerEntry},
    run::{CancelToken, EventSink, RunEvent},
    taint::{entry_taints, ConfirmPolicy, TaintTracker, ToolClass, ToolClassifier},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfirmDecision {
    Approve,
    Deny,
}

/// The UI's answer to a [`RunEvent::ConfirmRequest`]. The tool thread blocks
/// in [`ConfirmReply::wait`] until [`ConfirmReply::reply`] is called.
#[derive(Clone, Default)]
pub struct ConfirmReply(Arc<(Mutex<Option<ConfirmDecision>>, Condvar)>);

impl ConfirmReply {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn approve(&self) {
        self.reply(ConfirmDecision::Approve);
    }

    pub fn deny(&self) {
        self.reply(ConfirmDecision::Deny);
    }

    pub fn reply(&self, decision: ConfirmDecision) {
        let (lock, cvar) = &*self.0;
        if let Ok(mut guard) = lock.lock() {
            *guard = Some(decision);
            cvar.notify_all();
        }
    }

    /// Block until answered. A cancelled run counts as a denial so the loop
    /// can wind down instead of hanging on a prompt nobody will answer.
    pub fn wait(&self, cancel: &CancelToken) -> ConfirmDecision {
        let (lock, cvar) = &*self.0;
        let Ok(mut guard) = lock.lock() else {
            return ConfirmDecision::Deny;
        };
        loop {
            if let Some(decision) = guard.take() {
                return decision;
            }
            if cancel.is_cancelled() {
                return ConfirmDecision::Deny;
            }
            guard = match cvar.wait_timeout(guard, Duration::from_millis(100)) {
                Ok((guard, _)) => guard,
                Err(_) => return ConfirmDecision::Deny,
            };
        }
    }
}

/// Safety settings for one run, carried in [`crate::RunConfig`].
#[derive(Clone, Debug)]
pub struct SafetyConfig {
    pub policy: ConfirmPolicy,
    pub classifier: ToolClassifier,
    /// Names the checkpoint directory; the panel passes it back to
    /// [`crate::revert_checkpoint`].
    pub run_id: String,
    /// Snapshot files before write tools run.
    pub checkpoints: bool,
    /// The run's taint flag. Clones of the config share it, so a child run
    /// gets its own with [`SafetyConfig::for_child`].
    pub taint: TaintTracker,
    /// Refuse every tool the classifier calls `Write`, whatever the policy.
    /// Set for runs of read-only harnesses.
    pub read_only: bool,
}

impl SafetyConfig {
    /// Settings for a child run: same policy, classifier and checkpoint
    /// setting, a distinct `run_id`, and a taint flag that starts as the
    /// parent's is now (what the child is told may come from untrusted
    /// content) but afterwards moves on its own.
    pub fn for_child(&self, run_id: String, parent_tainted: bool, read_only: bool) -> Self {
        Self {
            policy: self.policy,
            classifier: self.classifier.clone(),
            run_id,
            checkpoints: self.checkpoints,
            taint: TaintTracker::new(parent_tainted),
            read_only: self.read_only || read_only,
        }
    }

    pub fn new_run_id() -> String {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("run-{ms}-{n}")
    }
}

impl Default for SafetyConfig {
    fn default() -> Self {
        Self {
            policy: ConfirmPolicy::default(),
            classifier: ToolClassifier::default(),
            run_id: Self::new_run_id(),
            checkpoints: true,
            taint: TaintTracker::default(),
            read_only: false,
        }
    }
}

/// Live safety state of a running loop.
pub(crate) struct SafetyState {
    config: SafetyConfig,
    taint: TaintTracker,
    checkpoints: Option<CheckpointStore>,
    announced: AtomicBool,
}

impl SafetyState {
    pub(crate) fn new(config: &SafetyConfig, ledger: &Ledger, workspace_root: &Path) -> Self {
        // The config's flag carries what the run was started with (a child
        // inherits its parent's); the ledger adds whatever it already holds.
        let taint = config.taint.clone();
        if ledger.is_tainted(&config.classifier, workspace_root) {
            taint.mark();
        }
        let checkpoints = if config.checkpoints {
            match CheckpointStore::new(workspace_root, &config.run_id) {
                Ok(store) => Some(store),
                Err(err) => {
                    tracing::error!("[agent_chat] checkpoints disabled: {err}");
                    None
                }
            }
        } else {
            None
        };
        Self {
            config: config.clone(),
            taint,
            checkpoints,
            announced: AtomicBool::new(false),
        }
    }

    /// Calls in one batch run concurrently, so a write next to an external
    /// call counts as tainted even though the result has not landed yet.
    pub(crate) fn tainted_for(&self, call: &ToolCall, batch: &[ToolCall]) -> bool {
        self.taint.is_tainted()
            || batch.iter().any(|other| {
                other.id != call.id
                    && (self.config.classifier.classify(&other.name) == ToolClass::External
                        || crate::taint::is_mcp_tool(&other.name))
            })
    }

    fn needs_confirm(&self, class: ToolClass, tainted: bool) -> bool {
        match self.config.policy {
            ConfirmPolicy::Auto => false,
            ConfirmPolicy::AlwaysAsk => class != ToolClass::Read,
            ConfirmPolicy::AskWhenTainted => class == ToolClass::Write && tainted,
        }
    }

    /// Run before a tool executes. `Err` is the text of the tool error the
    /// model gets instead of running the tool.
    pub(crate) fn preflight(
        &self,
        call: &ToolCall,
        batch_tainted: bool,
        cancel: &CancelToken,
        sink: &EventSink,
    ) -> Result<(), String> {
        let class = self.config.classifier.classify(&call.name);

        if self.config.read_only && class == ToolClass::Write {
            return Err(format!(
                "'{}' was not run: this is a read-only run and the tool can change things.",
                call.name
            ));
        }

        if self.needs_confirm(class, batch_tainted) {
            let reply = ConfirmReply::new();
            sink(RunEvent::ConfirmRequest {
                id: call.id.clone(),
                tool: call.name.clone(),
                args: call.arguments_json.clone(),
                tainted: batch_tainted,
                reply: reply.clone(),
            });
            if reply.wait(cancel) == ConfirmDecision::Deny {
                return Err(format!(
                    "The user denied the call to '{}'. It was not run. Do not retry it unchanged; ask the user how to proceed.",
                    call.name
                ));
            }
        }

        if class == ToolClass::Write {
            if let Some(store) = &self.checkpoints {
                store
                    .snapshot_for_call(&call.arguments_json)
                    .map_err(|e| format!("'{}' was not run: {e}", call.name))?;
                if store.file_count() > 0 && !self.announced.swap(true, Ordering::SeqCst) {
                    sink(RunEvent::CheckpointCreated {
                        run_id: self.config.run_id.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Entries added to the ledger from outside the loop (finished background
    /// children delivered before a request) can carry untrusted content.
    pub(crate) fn observe_entries(&self, entries: &[LedgerEntry], workspace_root: &Path) {
        if entries
            .iter()
            .any(|entry| entry_taints(entry, &self.config.classifier, workspace_root))
        {
            self.taint.mark();
        }
    }

    /// Run after a tool finishes; flips the run to tainted when the result
    /// brought untrusted content in.
    pub(crate) fn observe(&self, call: &ToolCall, workspace_root: &Path) {
        if self
            .config
            .classifier
            .taints_on_result(&call.name, &call.arguments_json, workspace_root)
        {
            self.taint.mark();
        }
    }
}
