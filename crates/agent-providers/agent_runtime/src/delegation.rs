//! Delegation: a run starts child runs of a [`Harness`] that execute the same
//! [`run_loop`] with their own ledger.
//!
//! Tools (installed per run by [`install`]):
//! - `spawn_subagent(harness, task, background?)` returns the structured
//!   report as the tool result, or with `background: true` a handle.
//! - `await_subagent(id)` blocks until a run finishes and returns its report.
//! - `read_run(id, range)` pages through a run's stored transcript.
//! - `cancel_subagent(id)`.
//!
//! A background child that nobody awaits is delivered as a
//! [`LedgerEntry::subagent_report`] by [`deliver_pending`]; the projector
//! renders that as a synthetic `check_subagent` tool exchange (or as the data
//! envelope for tool-less profiles).

use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Condvar, Mutex, MutexGuard, RwLock},
    thread,
    time::Duration,
};

use agent_chat_core::ToolDefinition;
use serde_json::{json, Value};

use crate::budget_guard::{BudgetError, BudgetGuard, RunBudget, SpawnPermit};
use crate::checkpoint::child_checkpoint_id;
use crate::harness::{normalize_field, Harness, HarnessPermissions, HarnessRegistry, ModelPolicy};
use crate::ledger::{EntryKind, Ledger, LedgerEntry, SubagentReport, SubagentStatus};
use crate::profile::{resolve_profile, ModelProfile, ResolveInput, ToolCalling};
use crate::taint::TaintTracker;
use crate::run::{run_loop, CancelToken, EventSink, RunConfig, RunEvent, CANCELLED_MESSAGE};
use crate::run_store::{
    now_ms, render_transcript, ChildReport, InsertRefused, RunRecord, RunStatus, RunStore,
};

/// Id of the chat's own run, the parent of top-level sub-agents.
pub const ROOT_RUN_ID: &str = "main";

/// Gives the profile of a model id the way the chat resolves its own (name
/// table, the provider's model descriptor, the user's overrides). Installed by
/// the embedder; without one a child falls back to the name table alone.
pub type ProfileResolver = Arc<dyn Fn(&str) -> ModelProfile + Send + Sync>;

/// Separates the child run id from the tool call id in a forwarded
/// [`RunEvent::ConfirmRequest`].
const CONFIRM_ID_SEPARATOR: char = '\u{1f}';

/// Is this confirmation id one a child run produced? Those prompts outlive
/// the request that started the child.
pub fn is_child_confirm_id(id: &str) -> bool {
    id.contains(CONFIRM_ID_SEPARATOR)
}

/// Lines `read_run` returns when no range is given, and the most it returns.
const READ_RUN_DEFAULT_LINES: usize = 40;
const READ_RUN_MAX_LINES: usize = 200;
const READ_RUN_MAX_CHARS: usize = 16_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DelegationLimits {
    /// Levels of nesting below the chat's own run.
    pub max_depth: u32,
    /// Children one run may ever start.
    pub max_children_per_run: usize,
    /// Runs one chat may ever hold.
    pub max_total_runs: usize,
    /// Children executing at once; the rest wait for a slot.
    pub max_parallel: usize,
}

impl Default for DelegationLimits {
    fn default() -> Self {
        Self {
            max_depth: 2,
            max_children_per_run: 8,
            max_total_runs: 48,
            max_parallel: 3,
        }
    }
}

impl DelegationLimits {
    /// One child at a time for small local models, several for cloud ones.
    pub fn for_profile(profile: &ModelProfile) -> Self {
        let small = profile.tool_calling != ToolCalling::Native
            || (profile.context_tokens > 0 && profile.context_tokens <= 8_192);
        Self {
            max_parallel: if small { 1 } else { 3 },
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpawnError {
    UnknownHarness { name: String, available: Vec<String> },
    DepthLimit { max_depth: u32 },
    FanOut { limit: usize },
    TotalRuns { limit: usize },
    /// The run tree's [`BudgetGuard`] refused the spawn.
    Budget(BudgetError),
}

impl fmt::Display for SpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpawnError::Budget(err) => write!(f, "{err}"),
            SpawnError::UnknownHarness { name, available } => {
                write!(f, "Unknown harness `{name}`. Available: {}.", available.join(", "))
            }
            SpawnError::DepthLimit { max_depth } => write!(
                f,
                "Sub-agent depth limit reached (max depth {max_depth}); this run cannot start sub-agents."
            ),
            SpawnError::FanOut { limit } => {
                write!(f, "This run already started {limit} sub-agents, the most it may start.")
            }
            SpawnError::TotalRuns { limit } => {
                write!(f, "This chat already holds {limit} runs, the most it may keep.")
            }
        }
    }
}

impl std::error::Error for SpawnError {}

/// Counts running children per parent against a limit that can change
/// between requests. Per parent, so a child waiting on its own child never
/// starves it of a slot.
struct Permits {
    running: Mutex<HashMap<String, usize>>,
    changed: Condvar,
}

struct PermitGuard {
    permits: Arc<Permits>,
    parent: String,
}

impl Drop for PermitGuard {
    fn drop(&mut self) {
        if let Ok(mut running) = self.permits.running.lock() {
            if let Some(count) = running.get_mut(&self.parent) {
                *count = count.saturating_sub(1);
            }
        }
        self.permits.changed.notify_all();
    }
}

impl Permits {
    /// `None` if `cancel` fires while waiting for a slot.
    fn acquire(
        self: &Arc<Self>,
        parent: &str,
        limit: impl Fn() -> usize,
        cancel: &CancelToken,
    ) -> Option<PermitGuard> {
        let mut running = self.running.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if cancel.is_cancelled() {
                return None;
            }
            let count = running.entry(parent.to_string()).or_insert(0);
            if *count < limit().max(1) {
                *count += 1;
                return Some(PermitGuard {
                    permits: self.clone(),
                    parent: parent.to_string(),
                });
            }
            running = match self.changed.wait_timeout(running, Duration::from_millis(100)) {
                Ok((guard, _)) => guard,
                Err(p) => p.into_inner().0,
            };
        }
    }
}

/// Shared by every run of one chat: the store, the harness list, the limits
/// and the sink child events go to.
pub struct DelegationHost {
    store: Arc<RunStore>,
    harnesses: RwLock<Arc<HarnessRegistry>>,
    limits: RwLock<DelegationLimits>,
    /// Limits of one run tree; every root handle gets a fresh guard for it.
    budget: RwLock<RunBudget>,
    profile_resolver: RwLock<Option<ProfileResolver>>,
    events: EventSink,
    permits: Arc<Permits>,
    tokens: Mutex<HashMap<String, CancelToken>>,
}

impl DelegationHost {
    /// `events` receives `ChildSpawned`, `ChildProgress` and `ChildFinished`
    /// for every run of the chat, including ones that outlive the request
    /// that started them.
    pub fn new(
        store: Arc<RunStore>,
        harnesses: HarnessRegistry,
        limits: DelegationLimits,
        events: EventSink,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            harnesses: RwLock::new(Arc::new(harnesses)),
            limits: RwLock::new(limits),
            budget: RwLock::new(RunBudget::default()),
            profile_resolver: RwLock::new(None),
            events,
            permits: Arc::new(Permits {
                running: Mutex::new(HashMap::new()),
                changed: Condvar::new(),
            }),
            tokens: Mutex::new(HashMap::new()),
        })
    }

    pub fn store(&self) -> &Arc<RunStore> {
        &self.store
    }

    pub fn harnesses(&self) -> Arc<HarnessRegistry> {
        self.harnesses.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub fn set_harnesses(&self, harnesses: HarnessRegistry) {
        *self.harnesses.write().unwrap_or_else(|p| p.into_inner()) = Arc::new(harnesses);
    }

    pub fn limits(&self) -> DelegationLimits {
        self.limits.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub fn set_limits(&self, limits: DelegationLimits) {
        *self.limits.write().unwrap_or_else(|p| p.into_inner()) = limits;
    }

    pub fn budget(&self) -> RunBudget {
        *self.budget.read().unwrap_or_else(|p| p.into_inner())
    }

    /// Applies to run trees started after the call. The tighter of this and
    /// [`DelegationHost::limits`] wins on each axis.
    pub fn set_budget(&self, budget: RunBudget) {
        *self.budget.write().unwrap_or_else(|p| p.into_inner()) = budget;
    }

    pub fn set_profile_resolver(&self, resolver: Option<ProfileResolver>) {
        *self.profile_resolver.write().unwrap_or_else(|p| p.into_inner()) = resolver;
    }

    /// The profile a child running `model` gets. The parent's own profile
    /// when the model is the parent's; otherwise the embedder's resolver, or
    /// the name table without it.
    fn profile_for(&self, model: &str, template: &RunConfig) -> ModelProfile {
        if model == template.model {
            return template.profile;
        }
        let resolver = self
            .profile_resolver
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        match resolver {
            Some(resolve) => resolve(model),
            None => resolve_profile(ResolveInput {
                model_id: model,
                ..ResolveInput::default()
            }),
        }
    }

    /// The handle for the chat's own run. `template` is the config children
    /// are derived from (provider, model, registry, workspace, safety); it
    /// should not carry delegation itself. One call per request: the handle
    /// owns that request's [`BudgetGuard`], shared by every run below it, and
    /// the root run's taint flag (`template.safety.taint`) that children
    /// inherit.
    pub fn root_handle(self: &Arc<Self>, template: RunConfig) -> DelegationHandle {
        let mut template = template;
        template.delegation = None;
        DelegationHandle {
            host: self.clone(),
            run_id: ROOT_RUN_ID.to_string(),
            depth: 0,
            depth_budget: self.limits().max_depth,
            deliver_in_run: false,
            pinned: String::new(),
            guard: BudgetGuard::new(self.budget()),
            taint: template.safety.taint.clone(),
            safety_run_id: template.safety.run_id.clone(),
            root_safety_run_id: template.safety.run_id.clone(),
            ceiling: HarnessPermissions::EngineSceneWrite,
            template: Arc::new(template),
        }
    }

    /// Cancel one run (and, through the token chain, its descendants).
    /// `false` if it is unknown or already finished.
    pub fn cancel_run(&self, id: &str) -> bool {
        if self.store.get(id).is_none_or(|run| !run.is_active()) {
            return false;
        }
        match self.lock_tokens().get(id) {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    /// Cancel every unfinished run (the Stop button).
    pub fn cancel_all(&self) {
        for token in self.lock_tokens().values() {
            token.cancel();
        }
    }

    fn lock_tokens(&self) -> MutexGuard<'_, HashMap<String, CancelToken>> {
        self.tokens.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn emit(&self, event: RunEvent) {
        (self.events)(event);
    }
}

/// A run's way into the host.
#[derive(Clone)]
pub struct DelegationHandle {
    host: Arc<DelegationHost>,
    template: Arc<RunConfig>,
    run_id: String,
    depth: u32,
    /// Levels this run may still start below itself; 0 means no spawn tool.
    depth_budget: u32,
    deliver_in_run: bool,
    pinned: String,
    /// Shared by every run of the tree.
    guard: BudgetGuard,
    /// This run's own taint flag.
    taint: TaintTracker,
    /// Checkpoint id of this run, and of the tree's root.
    safety_run_id: String,
    root_safety_run_id: String,
    /// The most a child of this run may be allowed to change.
    ceiling: HarnessPermissions,
}

impl DelegationHandle {
    /// Deliver finished background children into this run's ledger before
    /// each request. Off for the chat's own run, whose ledger the panel keeps
    /// and delivers into itself with [`deliver_pending`].
    pub fn deliver_in_run(mut self, on: bool) -> Self {
        self.deliver_in_run = on;
        self
    }

    /// Never let this run's children change more than `ceiling` (plan mode).
    pub(crate) fn clamp_ceiling(&mut self, ceiling: HarnessPermissions) {
        self.ceiling = self.ceiling.min(ceiling);
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn host(&self) -> &Arc<DelegationHost> {
        &self.host
    }

    /// Start a child run of `harness_name`. Returns as soon as it is queued;
    /// use [`RunStore::wait_terminal`] to wait for it.
    pub fn spawn(
        &self,
        cancel: &CancelToken,
        harness_name: &str,
        task: &str,
        context: Option<&str>,
        background: bool,
    ) -> Result<String, SpawnError> {
        let host = &self.host;
        let limits = host.limits();
        let registry = host.harnesses();
        let harness = registry.get(harness_name).cloned().ok_or_else(|| {
            SpawnError::UnknownHarness {
                name: harness_name.to_string(),
                available: registry.all().iter().map(|h| h.name.clone()).collect(),
            }
        })?;

        if self.depth_budget == 0 {
            return Err(SpawnError::DepthLimit {
                max_depth: limits.max_depth,
            });
        }

        // The run tree's budget: held by the child until it finishes, so a
        // finished child frees its fan-out slot.
        let permit = self
            .guard
            .try_spawn(&self.run_id, self.depth)
            .map_err(SpawnError::Budget)?;

        let id = host.store.next_id();
        let model = resolve_model(&harness, &self.template);
        let record = RunRecord {
            id: id.clone(),
            parent: Some(self.run_id.clone()),
            harness: harness.name.clone(),
            task: task.to_string(),
            model,
            depth: self.depth + 1,
            status: RunStatus::Pending,
            background,
            created_at_ms: now_ms(),
            started_at_ms: None,
            finished_at_ms: None,
            progress: String::new(),
            report: None,
            error: None,
            transcript: Vec::new(),
            delivered: false,
        };
        host.store
            .insert_checked(record, limits.max_children_per_run, limits.max_total_runs)
            .map_err(|refused| match refused {
                InsertRefused::FanOut { limit } => SpawnError::FanOut { limit },
                InsertRefused::Total { limit } => SpawnError::TotalRuns { limit },
            })?;
        host.store.flush();

        let child_cancel = cancel.child();
        host.lock_tokens().insert(id.clone(), child_cancel.clone());
        host.emit(RunEvent::ChildSpawned {
            run_id: id.clone(),
            parent_id: self.run_id.clone(),
            harness: harness.name.clone(),
            task: task.to_string(),
            depth: self.depth + 1,
            background,
        });

        let job = ChildJob {
            parent: self.clone(),
            harness,
            id: id.clone(),
            task: task.to_string(),
            context: context.map(str::to_string),
            cancel: child_cancel,
            permit: Some(permit),
            parent_tainted: self.taint.is_tainted(),
        };
        thread::spawn(move || job.run());
        Ok(id)
    }
}

fn resolve_model(harness: &Harness, template: &RunConfig) -> String {
    match &harness.model_policy {
        ModelPolicy::Explicit(model) => model.clone(),
        ModelPolicy::Cheap => template.compact_model.clone(),
        ModelPolicy::Same => template.model.clone(),
    }
}

struct ChildJob {
    parent: DelegationHandle,
    harness: Harness,
    id: String,
    task: String,
    context: Option<String>,
    cancel: CancelToken,
    /// The run tree's slot for this child.
    permit: Option<SpawnPermit>,
    /// Whether the spawning run had untrusted content in context when it
    /// started this child.
    parent_tainted: bool,
}

/// A child's context window never exceeds what its harness asks for. The
/// tier stays the model's own: the budget is a limit on this run, not a
/// statement about the model.
fn cap_context(mut profile: ModelProfile, tokens: u32) -> ModelProfile {
    let tokens = tokens.max(1_024);
    if profile.context_tokens == 0 || profile.context_tokens > tokens {
        profile.context_tokens = tokens;
        profile.max_tokens = profile.max_tokens.min((tokens / 4).max(256));
    }
    profile
}

impl ChildJob {
    fn run(mut self) {
        let host = self.parent.host.clone();
        let store = host.store.clone();

        // Wait for a parallelism slot; a cancel while queued ends the run.
        let permit = host.permits.acquire(
            self.parent.run_id.as_str(),
            || host.limits().max_parallel,
            &self.cancel,
        );
        let outcome = match permit {
            None => Outcome::Cancelled(None),
            Some(_permit) => {
                store.update(&self.id, |run| {
                    run.status = RunStatus::Running;
                    run.started_at_ms = Some(now_ms());
                });
                self.execute()
            }
        };
        // Free the fan-out slot before the run reads as finished, so a parent
        // that awaits this child can start the next one at once.
        self.permit.take();
        self.finish(outcome);
    }

    fn execute(&self) -> Outcome {
        let host = &self.parent.host;
        let template = &self.parent.template;

        let mut config = (**template).clone();
        config.delegation = None;
        config.pre_request_hook = None;
        // The steward belongs to the chat, not to its helpers.
        config.steward = None;
        // Plan mode and project memory belong to the chat's own run.
        config.plan_mode = None;
        config.memory = None;
        config.ledger_out = None;

        // The child's own model gets its own profile (tool-calling mode,
        // window, tier, sampling); the parent's applies only when the child
        // runs the same model.
        let model = resolve_model(&self.harness, template);
        let mut profile = host.profile_for(&model, template);
        if let Some(tokens) = self.harness.context_budget_tokens {
            profile = cap_context(profile, tokens);
        }
        if model != template.model {
            config.compact_model = model.clone();
        }
        config.model = model;
        config = config.with_profile(profile);
        config.max_iterations = self.harness.max_steps;
        // The harness prompt carries the task and the report format; the
        // generic Lite prompt would replace it.
        config.lite_system_prompt = None;

        // What the child may change: its harness, capped by its parent's own
        // permissions. The tool list is trimmed with the run's classifier and
        // the safety layer refuses write calls again at run time.
        let permissions = self.harness.permissions.min(self.parent.ceiling);
        let available: Vec<String> = template.tool_registry.names().iter().map(|n| n.to_string()).collect();
        let allowed =
            self.harness
                .resolve_tools_with(&available, &template.safety.classifier, self.parent.ceiling);
        for name in &available {
            if !allowed.contains(name) {
                config.tool_registry.unregister(name);
            }
        }
        config.tools = definitions_of(&config.tool_registry);
        // Best-of-N is a per-harness option; it is off unless the harness asks.
        config.best_of = crate::best_of::BestOf::new(
            self.harness.best_of,
            crate::best_of::default_verifier(&config.tools, config.workspace_root.clone()),
        );

        config.safety = template.safety.for_child(
            child_checkpoint_id(&self.parent.safety_run_id, &self.id),
            self.parent_tainted,
            permissions == HarnessPermissions::ReadOnly,
        );
        config.safety.sandbox |= self.harness.sandbox;

        // Small-context (Lite) children do not delegate further.
        let child_budget = if profile.is_lite() {
            0
        } else {
            self.harness
                .max_depth
                .min(self.parent.depth_budget.saturating_sub(1))
        };
        if child_budget > 0 {
            // `Same` / `Cheap` below this child resolve from the child's own
            // model, not from the chat's.
            let mut nested = (**template).clone();
            nested.model = config.model.clone();
            nested.compact_model = config.compact_model.clone();
            nested.profile = config.profile;
            config.delegation = Some(DelegationHandle {
                host: host.clone(),
                template: Arc::new(nested),
                run_id: self.id.clone(),
                depth: self.parent.depth + 1,
                depth_budget: child_budget,
                deliver_in_run: true,
                pinned: String::new(),
                guard: self.parent.guard.clone(),
                taint: config.safety.taint.clone(),
                safety_run_id: config.safety.run_id.clone(),
                root_safety_run_id: self.parent.root_safety_run_id.clone(),
                ceiling: permissions,
            });
        }

        let system = self.harness.render_prompt(
            &config.workspace_root.display().to_string(),
            &self.task,
            &self.parent.pinned,
        );
        let mut ledger = Ledger::with_system_prompt(system);
        let first_turn = match &self.context {
            Some(context) if !context.trim().is_empty() => {
                format!("{}\n\nContext from the requesting agent:\n{}", self.task, context.trim())
            }
            _ => self.task.clone(),
        };
        ledger.push(LedgerEntry::user(first_turn));

        let finished: Arc<Mutex<Option<Result<Option<String>, String>>>> = Arc::new(Mutex::new(None));
        let sink: EventSink = {
            let host = host.clone();
            let id = self.id.clone();
            let finished = finished.clone();
            let harness_name = self.harness.name.clone();
            let root_checkpoint = self.parent.root_safety_run_id.clone();
            Arc::new(move |event| match event {
                // A child's approval prompts go to the chat's own prompt.
                RunEvent::ConfirmRequest {
                    id: call_id,
                    tool,
                    args,
                    tainted,
                    reply,
                } => {
                    progress(&host, &id, format!("waiting for approval of {tool}"));
                    host.emit(RunEvent::ConfirmRequest {
                        id: format!("{id}{CONFIRM_ID_SEPARATOR}{call_id}"),
                        tool: format!("{tool} (sub-agent {harness_name})"),
                        args,
                        tainted,
                        reply,
                    });
                }
                // Child checkpoints are found again from the root's id.
                RunEvent::CheckpointCreated { .. } => host.emit(RunEvent::CheckpointCreated {
                    run_id: root_checkpoint.clone(),
                }),
                RunEvent::ToolCallGroup(calls) => {
                    let names: Vec<&str> = calls.iter().map(|c| c.name.as_str()).collect();
                    progress(&host, &id, format!("running {}", names.join(", ")));
                }
                RunEvent::ContextCompacted(_) => progress(&host, &id, "compacted its context".to_string()),
                RunEvent::Finished(result) => {
                    *finished.lock().unwrap_or_else(|p| p.into_inner()) =
                        Some(result.map(|response| response.assistant_message));
                }
                _ => {}
            })
        };

        let final_ledger = run_loop(config, ledger, self.cancel.clone(), sink);
        let transcript = render_transcript(&final_ledger);
        let last_text = final_ledger.entries.iter().rev().find_map(|entry| match &entry.kind {
            EntryKind::AssistantTurn { text } if !text.trim().is_empty() => Some(text.clone()),
            _ => None,
        });
        let result = finished
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
            .unwrap_or_else(|| Err("The run ended without reporting a result.".to_string()));

        match result {
            Ok(message) => Outcome::Done {
                text: message.filter(|m| !m.trim().is_empty()).or(last_text),
                transcript,
            },
            Err(err) if err == CANCELLED_MESSAGE || self.cancel.is_cancelled() => {
                Outcome::Cancelled(Some(transcript))
            }
            Err(err) => Outcome::Failed { err, transcript },
        }
    }

    fn finish(&self, outcome: Outcome) {
        let host = &self.parent.host;
        let (status, report, error, transcript) = match outcome {
            Outcome::Done { text, transcript } => {
                let report = match text {
                    Some(text) => parse_report(&text, &self.harness.result_schema),
                    None => ChildReport {
                        summary: "The run ended without an answer.".to_string(),
                        ..ChildReport::default()
                    },
                };
                (RunStatus::Success, report, None, transcript)
            }
            Outcome::Failed { err, transcript } => (
                RunStatus::Error,
                ChildReport {
                    summary: format!("The run failed: {err}"),
                    ..ChildReport::default()
                },
                Some(err),
                transcript,
            ),
            Outcome::Cancelled(transcript) => (
                RunStatus::Cancelled,
                ChildReport {
                    summary: "The run was cancelled before it finished.".to_string(),
                    ..ChildReport::default()
                },
                None,
                transcript.unwrap_or_default(),
            ),
        };

        // Run extensions attach their own sections (a sandboxed run's scene diff).
        let mut report = report;
        report.extra.extend(crate::run_ext::extension_report_sections(&child_checkpoint_id(
            &self.parent.safety_run_id,
            &self.id,
        )));
        let summary = report.summary.clone();
        host.store.update(&self.id, |run| {
            run.status = status;
            run.finished_at_ms = Some(now_ms());
            run.report = Some(report);
            run.error = error;
            run.transcript = transcript;
        });
        host.store.flush();
        host.lock_tokens().remove(&self.id);
        host.emit(RunEvent::ChildFinished {
            run_id: self.id.clone(),
            status: status.report_status().unwrap_or(SubagentStatus::Error),
            summary,
        });
    }
}

enum Outcome {
    Done { text: Option<String>, transcript: Vec<String> },
    Failed { err: String, transcript: Vec<String> },
    Cancelled(Option<Vec<String>>),
}

fn progress(host: &DelegationHost, id: &str, note: String) {
    host.store.update(id, |run| run.progress = note.clone());
    host.emit(RunEvent::ChildProgress {
        run_id: id.to_string(),
        note,
    });
}

fn definitions_of(registry: &agent_chat_tools::ToolRegistry) -> Vec<ToolDefinition> {
    registry
        .definitions()
        .into_iter()
        .filter(|def| def.parameters_schema.get("type").and_then(Value::as_str) == Some("object"))
        .map(|def| ToolDefinition {
            name: def.name,
            description: Some(def.description),
            parameters_json_schema: def.parameters_schema,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Reports
// ---------------------------------------------------------------------------

/// Split an answer written in the harness's sections into a report. Text with
/// no recognised heading becomes the summary.
pub fn parse_report(text: &str, schema: &[String]) -> ChildReport {
    let mut preface: Vec<&str> = Vec::new();
    let mut sections: Vec<(String, Vec<&str>)> = Vec::new();

    for line in text.lines() {
        if let Some(name) = heading_name(line, schema) {
            sections.push((name, Vec::new()));
        } else if let Some((_, lines)) = sections.last_mut() {
            lines.push(line);
        } else {
            preface.push(line);
        }
    }

    let mut report = ChildReport::default();
    if sections.is_empty() {
        report.summary = text.trim().to_string();
        return report;
    }
    for (name, lines) in sections {
        let body = lines.join("\n").trim().to_string();
        match name.as_str() {
            "summary" => report.summary = body,
            "findings" => report.findings.extend(list_items(&lines)),
            "refs" => report.refs.extend(list_items(&lines)),
            "open_questions" => report.open_questions.extend(list_items(&lines)),
            _ => report.extra.push((name, body)),
        }
    }
    if report.summary.is_empty() {
        report.summary = preface.join("\n").trim().to_string();
    }
    report
}

fn heading_name(line: &str, schema: &[String]) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.len() > 48 || trimmed.starts_with("- ") {
        return None;
    }
    let name = normalize_field(trimmed);
    schema.contains(&name).then_some(name)
}

fn list_items(lines: &[&str]) -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match strip_bullet(trimmed) {
            Some(item) => items.push(item.to_string()),
            None => match items.last_mut() {
                Some(last) if line.starts_with(char::is_whitespace) => {
                    last.push(' ');
                    last.push_str(trimmed);
                }
                _ => items.push(trimmed.to_string()),
            },
        }
    }
    items.retain(|item| {
        let lower = item.trim().trim_end_matches('.').to_lowercase();
        !matches!(lower.as_str(), "none" | "n/a" | "nothing" | "-")
    });
    items
}

fn strip_bullet(line: &str) -> Option<&str> {
    for marker in ["- ", "* ", "\u{2022} "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return Some(rest.trim());
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        let rest = &line[digits..];
        if let Some(rest) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some(rest.trim());
        }
    }
    None
}

/// A finished record as the JSON a tool returns. Fields only.
pub fn report_value(run: &RunRecord) -> Value {
    let report = run.report.clone().unwrap_or_default();
    let mut value = json!({
        "run_id": run.id,
        "harness": run.harness,
        "status": run.status.as_str(),
        "summary": report.summary,
        "findings": report.findings,
        "refs": report.refs,
        "open_questions": report.open_questions,
        "transcript_entries": run.transcript.len(),
    });
    if !report.extra.is_empty() {
        value["extra"] = report
            .extra
            .iter()
            .map(|(name, body)| (name.clone(), Value::String(body.clone())))
            .collect::<serde_json::Map<_, _>>()
            .into();
    }
    value
}

/// The ledger form of a finished run: findings and open questions are folded
/// into the summary text, refs stay separate.
pub fn subagent_report_for(run: &RunRecord) -> SubagentReport {
    let report = run.report.clone().unwrap_or_default();
    let mut summary = format!(
        "Sub-agent '{}' finished with status {}.\n\n{}",
        run.harness,
        run.status.as_str(),
        report.summary
    );
    if !report.findings.is_empty() {
        summary.push_str("\n\nFindings:");
        for finding in &report.findings {
            summary.push_str("\n- ");
            summary.push_str(finding);
        }
    }
    if !report.open_questions.is_empty() {
        summary.push_str("\n\nOpen questions:");
        for question in &report.open_questions {
            summary.push_str("\n- ");
            summary.push_str(question);
        }
    }
    SubagentReport {
        run_id: run.id.clone(),
        status: run.status.report_status().unwrap_or(SubagentStatus::Error),
        summary,
        refs: report.refs,
    }
}

/// Append every finished, unseen child of `parent_run_id` to `ledger` as a
/// sub-agent report. Returns how many were added. No model request is made;
/// the next request carries them.
pub fn deliver_pending(store: &RunStore, parent_run_id: &str, ledger: &mut Ledger) -> usize {
    let runs = store.take_undelivered(parent_run_id);
    let count = runs.len();
    for run in runs {
        ledger.push(LedgerEntry::subagent_report(subagent_report_for(&run)));
    }
    count
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

/// Add the delegation tools to `config`. Called by [`run_loop`] when the
/// config carries a [`DelegationHandle`].
pub fn install(
    mut handle: DelegationHandle,
    config: &mut RunConfig,
    ledger: &Ledger,
    cancel: &CancelToken,
) {
    handle.pinned = pinned_text(ledger);
    let can_spawn = handle.depth_budget > 0;
    let wants_reads = can_spawn || handle.depth == 0;
    let host = handle.host.clone();

    if can_spawn {
        let description = format!(
            "Delegate a task to a sub-agent that runs in its own context window and returns a structured report \
             (summary, findings, refs, open_questions). Its full transcript stays in the run store; read it with read_run. \
             By default this call waits and returns the report. With background=true it returns a run id at once; \
             collect the report with await_subagent.\nHarnesses:\n{}",
            host.harnesses().catalog()
        );
        let spawn_handle = handle.clone();
        let spawn_cancel = cancel.clone();
        register(
            config,
            "spawn_subagent",
            &description,
            json!({
                "type": "object",
                "properties": {
                    "harness": {"type": "string", "description": "Harness name from the list above."},
                    "task": {"type": "string", "description": "Everything the sub-agent needs: it sees none of this conversation."},
                    "context": {"type": "string", "description": "Optional background to pass along."},
                    "background": {"type": "boolean", "description": "Return a run id immediately instead of waiting."}
                },
                "required": ["harness", "task"]
            }),
            move |args, _ctx| {
                let harness = required_str(&args, "harness")?;
                let task = required_str(&args, "task")?;
                let context = args.get("context").and_then(Value::as_str);
                let background = args.get("background").and_then(Value::as_bool).unwrap_or(false);
                let id = spawn_handle
                    .spawn(&spawn_cancel, harness, task, context, background)
                    .map_err(anyhow::Error::new)?;
                if background {
                    return Ok(json!({
                        "ok": true,
                        "run_id": id,
                        "harness": harness,
                        "status": "running",
                    }));
                }
                await_run(&spawn_handle.host.store, &id, &spawn_cancel)
            },
        );
    }

    if wants_reads {
        let store = host.store.clone();
        let await_cancel = cancel.clone();
        register(
            config,
            "await_subagent",
            "Wait for a sub-agent started with background=true and return its structured report.",
            json!({
                "type": "object",
                "properties": {"id": {"type": "string", "description": "Run id from spawn_subagent."}},
                "required": ["id"]
            }),
            move |args, _ctx| await_run(&store, required_str(&args, "id")?, &await_cancel),
        );

        let store = host.store.clone();
        register(
            config,
            "read_run",
            "Read part of a sub-agent's stored transcript. `range` is `start-end` (entry numbers, default the first 40).",
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Run id."},
                    "range": {"type": "string", "description": "Entries to read, for example \"0-40\" or \"40-\"."}
                },
                "required": ["id"]
            }),
            move |args, _ctx| read_run(&store, required_str(&args, "id")?, args.get("range")),
        );

        let cancel_host = host.clone();
        register(
            config,
            "cancel_subagent",
            "Cancel a running sub-agent and everything it started.",
            json!({
                "type": "object",
                "properties": {"id": {"type": "string", "description": "Run id."}},
                "required": ["id"]
            }),
            move |args, _ctx| {
                let id = required_str(&args, "id")?;
                if cancel_host.store.get(id).is_none() {
                    return Err(anyhow::anyhow!("No run with id {id}."));
                }
                Ok(json!({"ok": true, "run_id": id, "cancelled": cancel_host.cancel_run(id)}))
            },
        );
    }

    if handle.deliver_in_run {
        let store = host.store.clone();
        let run_id = handle.run_id.clone();
        let previous = config.pre_request_hook.take();
        config.pre_request_hook = Some(Arc::new(move |ledger: &mut Ledger| {
            if let Some(previous) = &previous {
                previous(ledger);
            }
            deliver_pending(&store, &run_id, ledger);
        }));
    }
}

fn register(
    config: &mut RunConfig,
    name: &str,
    description: &str,
    schema: Value,
    handler: impl Fn(Value, &agent_chat_tools::ToolContext) -> anyhow::Result<Value> + Send + Sync + 'static,
) {
    config
        .tool_registry
        .register_fn(name, description, schema.clone(), handler);
    config.tools.retain(|tool| tool.name != name);
    config.tools.push(ToolDefinition {
        name: name.to_string(),
        description: Some(description.to_string()),
        parameters_json_schema: schema,
    });
}

fn required_str<'a>(args: &'a Value, key: &str) -> anyhow::Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("`{key}` is required"))
}

fn await_run(store: &RunStore, id: &str, cancel: &CancelToken) -> anyhow::Result<Value> {
    if store.get(id).is_none() {
        return Err(anyhow::anyhow!("No run with id {id}."));
    }
    match store.wait_terminal(id, cancel) {
        Some(run) => {
            store.update(id, |run| run.delivered = true);
            Ok(report_value(&run))
        }
        None => Err(anyhow::anyhow!("Cancelled while waiting for run {id}.")),
    }
}

fn read_run(store: &RunStore, id: &str, range: Option<&Value>) -> anyhow::Result<Value> {
    let run = store
        .get(id)
        .ok_or_else(|| anyhow::anyhow!("No run with id {id}."))?;
    let (start, end) = parse_range(range);
    let (lines, total) = store
        .read_transcript(id, start, end)
        .ok_or_else(|| anyhow::anyhow!("No run with id {id}."))?;

    let mut used = 0usize;
    let mut kept: Vec<String> = Vec::new();
    for line in lines {
        used += line.len();
        if used > READ_RUN_MAX_CHARS && !kept.is_empty() {
            break;
        }
        kept.push(line);
    }
    let first = start.min(total);
    Ok(json!({
        "run_id": id,
        "status": run.status.as_str(),
        "total_entries": total,
        "start": first,
        "end": first + kept.len(),
        "entries": kept,
    }))
}

/// `"10-20"`, `"10..20"`, `"10-"`, `{"start":10,"end":20}` or nothing.
fn parse_range(range: Option<&Value>) -> (usize, usize) {
    let default = (0, READ_RUN_DEFAULT_LINES);
    let (start, end) = match range {
        Some(Value::String(raw)) => {
            let raw = raw.trim();
            let (a, b) = raw
                .split_once("..")
                .or_else(|| raw.split_once('-'))
                .or_else(|| raw.split_once(':'))
                .unwrap_or((raw, ""));
            let start = a.trim().parse::<usize>().ok();
            let end = b.trim().trim_start_matches('=').parse::<usize>().ok();
            match start {
                Some(start) => (start, end.unwrap_or(start + READ_RUN_DEFAULT_LINES)),
                None => default,
            }
        }
        Some(Value::Object(map)) => {
            let start = map.get("start").and_then(Value::as_u64).unwrap_or(0) as usize;
            let end = map
                .get("end")
                .and_then(Value::as_u64)
                .map(|e| e as usize)
                .unwrap_or(start + READ_RUN_DEFAULT_LINES);
            (start, end)
        }
        _ => default,
    };
    (start, end.min(start.saturating_add(READ_RUN_MAX_LINES)).max(start))
}

/// The user's durable context, handed to children through `{pinned}`.
fn pinned_text(ledger: &Ledger) -> String {
    let lines: Vec<String> = ledger
        .entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            EntryKind::Pinned { kind, text } if *kind != crate::ledger::PinKind::LiveState => {
                Some(format!("Pinned {}: {}", kind.label(), text.trim()))
            }
            _ => None,
        })
        .collect();
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_harness_budget_caps_the_window_in_tokens_without_changing_the_tier() {
        let big = ModelProfile::new(128_000, true);
        let capped = cap_context(big, 12_000);
        assert_eq!(capped.context_tokens, 12_000);
        assert_eq!(capped.max_tokens, 3_000);
        assert!(!capped.is_lite(), "the tier is the model's, not the budget's");

        // A budget larger than the window changes nothing.
        assert_eq!(cap_context(ModelProfile::new(8_192, true), 50_000), ModelProfile::new(8_192, true));

        // An unknown window (character budgets) becomes a token budget.
        assert_eq!(cap_context(ModelProfile::unknown(), 6_000).context_tokens, 6_000);
    }
}
