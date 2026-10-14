//! Plan mode: a read-only run that ends in a reviewable plan.
//!
//! A run with [`RunConfig::plan_mode`] set cannot change anything:
//!
//! - its tool list is trimmed to read-class tools (the `planner` harness
//!   with every tool allowed, then the permission filter), so write tools are
//!   not advertised and not even registered;
//! - the safety layer refuses any write-class call again at run time;
//! - sub-agents it starts are capped at read-only too.
//!
//! The model finishes by calling `exit_plan_mode` with a structured [`Plan`].
//! The tool stores the plan in the run's [`PlanMode`] slot and the loop ends
//! the run. The panel shows the plan; only when the user approves does it
//! start a normal, edit-capable run, with [`Plan::apply_to`] having pinned
//! the approved plan (goal as a Requirement, steps as Decisions, risks as
//! Open questions) so it survives compaction.

use std::sync::{Arc, Mutex};

use agent_chat_core::ToolDefinition;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::harness::{builtin_harnesses, HarnessPermissions};
use crate::ledger::{Ledger, PinKind};
use crate::pins::{PinAdd, PinAuthor};
use crate::run::RunConfig;
use crate::taint::ToolClass;

/// The tool the model calls to present its plan.
pub const EXIT_PLAN_MODE: &str = "exit_plan_mode";

pub const PLAN_MODE_PROMPT: &str = "## Plan mode\n\
You are in plan mode and cannot change anything: edit, command and scene-write tools are not available and any such call is refused. \
Investigate with the read tools, then call `exit_plan_mode` exactly once with the goal, ordered steps (each with a title, detail and the files it touches) and the risks. \
Do not write the plan out in prose instead. After submitting, stop: the user reviews it and, only if they approve, a separate run carries it out.";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub title: String,
    pub detail: String,
    pub files: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub goal: String,
    pub steps: Vec<PlanStep>,
    pub risks: Vec<String>,
}

fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn str_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(flat)
            .filter(|s| !s.is_empty())
            .collect(),
        Some(Value::String(s)) => s
            .split(',')
            .map(flat)
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

impl Plan {
    /// Parse the `exit_plan_mode` arguments. Steps may be objects or plain
    /// strings.
    pub fn from_args(args: &Value) -> Result<Plan, String> {
        let goal = ["goal", "summary", "title"]
            .iter()
            .find_map(|k| args.get(*k).and_then(Value::as_str))
            .map(flat)
            .unwrap_or_default();
        let raw_steps = args
            .get("steps")
            .and_then(Value::as_array)
            .ok_or("`steps` must be a non-empty list")?;
        let mut steps = Vec::new();
        for raw in raw_steps {
            let step = match raw {
                Value::String(s) => PlanStep {
                    title: flat(s),
                    ..PlanStep::default()
                },
                Value::Object(_) => PlanStep {
                    title: ["title", "step", "name"]
                        .iter()
                        .find_map(|k| raw.get(*k).and_then(Value::as_str))
                        .map(flat)
                        .unwrap_or_default(),
                    detail: ["detail", "details", "description"]
                        .iter()
                        .find_map(|k| raw.get(*k).and_then(Value::as_str))
                        .map(flat)
                        .unwrap_or_default(),
                    files: str_list(raw.get("files")),
                },
                _ => continue,
            };
            if !step.title.is_empty() {
                steps.push(step);
            }
        }
        if steps.is_empty() {
            return Err("`steps` must be a non-empty list of steps with titles".to_string());
        }
        if goal.is_empty() {
            return Err("`goal` is required".to_string());
        }
        Ok(Plan {
            goal,
            steps,
            risks: str_list(args.get("risks")),
        })
    }

    /// Every file any step names, in first-mention order.
    pub fn files(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for file in self.steps.iter().flat_map(|s| s.files.iter()) {
            if !out.contains(file) {
                out.push(file.clone());
            }
        }
        out
    }

    /// The editable text form the panel shows ([`Plan::from_markdown`] reads
    /// it back).
    pub fn to_markdown(&self) -> String {
        let mut out = format!("Goal: {}\n\nSteps:\n", self.goal);
        for (i, step) in self.steps.iter().enumerate() {
            out.push_str(&format!("{}. {}\n", i + 1, step.title));
            if !step.detail.is_empty() {
                out.push_str(&format!("   {}\n", step.detail));
            }
            if !step.files.is_empty() {
                out.push_str(&format!("   files: {}\n", step.files.join(", ")));
            }
        }
        if !self.risks.is_empty() {
            out.push_str("\nRisks:\n");
            for risk in &self.risks {
                out.push_str(&format!("- {risk}\n"));
            }
        }
        out
    }

    /// Parse the text form. Lenient: the user is typing in a small box.
    pub fn from_markdown(text: &str) -> Result<Plan, String> {
        #[derive(PartialEq)]
        enum Section {
            Top,
            Steps,
            Risks,
        }
        let mut plan = Plan::default();
        let mut section = Section::Top;
        for raw in text.replace("\r\n", "\n").lines() {
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            let lower = line.to_ascii_lowercase();
            if let Some(rest) = lower.strip_prefix("goal:") {
                plan.goal = flat(&line[line.len() - rest.len()..]);
                section = Section::Top;
                continue;
            }
            if lower == "steps:" {
                section = Section::Steps;
                continue;
            }
            if lower == "risks:" {
                section = Section::Risks;
                continue;
            }
            match section {
                Section::Top => {
                    if plan.goal.is_empty() {
                        plan.goal = flat(line);
                    }
                }
                Section::Risks => {
                    let risk = flat(line.trim_start_matches(['-', '*', ' ']));
                    if !risk.is_empty() {
                        plan.risks.push(risk);
                    }
                }
                Section::Steps => {
                    let numbered = line
                        .split_once('.')
                        .filter(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
                        .map(|(_, rest)| flat(rest));
                    if let Some(title) = numbered {
                        plan.steps.push(PlanStep {
                            title,
                            ..PlanStep::default()
                        });
                    } else if let Some(step) = plan.steps.last_mut() {
                        if let Some(files) = lower.strip_prefix("files:") {
                            // Keep the user's casing.
                            let original = &line[line.len() - files.len()..];
                            step.files = original
                                .split(',')
                                .map(flat)
                                .filter(|f| !f.is_empty())
                                .collect();
                        } else {
                            if !step.detail.is_empty() {
                                step.detail.push(' ');
                            }
                            step.detail.push_str(&flat(line));
                        }
                    }
                }
            }
        }
        if plan.steps.is_empty() {
            return Err("The plan needs at least one numbered step under \"Steps:\".".to_string());
        }
        if plan.goal.is_empty() {
            return Err("The plan needs a \"Goal:\" line.".to_string());
        }
        Ok(plan)
    }

    /// The pins an approved plan becomes: `(kind, text)`.
    pub fn pins(&self) -> Vec<(PinKind, String)> {
        let total = self.steps.len();
        let mut out = vec![(
            PinKind::Requirement,
            format!("Carry out the approved plan: {}", self.goal),
        )];
        for (i, step) in self.steps.iter().enumerate() {
            let mut text = format!("Plan step {}/{}: {}", i + 1, total, step.title);
            if !step.detail.is_empty() {
                text.push_str(&format!(" - {}", step.detail));
            }
            if !step.files.is_empty() {
                text.push_str(&format!(" [{}]", step.files.join(", ")));
            }
            out.push((PinKind::Decision, text));
        }
        for risk in self.risks.iter().take(3) {
            out.push((PinKind::OpenQuestion, format!("Plan risk to watch: {risk}")));
        }
        out
    }

    /// Pin the approved plan. The user approved it, so the pins are theirs.
    /// Returns how many were added.
    pub fn apply_to(&self, ledger: &mut Ledger) -> usize {
        self.pins()
            .into_iter()
            .filter(|(kind, text)| {
                matches!(ledger.add_pin(*kind, text, PinAuthor::User), PinAdd::Added(_))
            })
            .count()
    }

    /// The user turn that starts the edit-capable run.
    pub fn approval_message(&self) -> String {
        format!(
            "I approved this plan. Carry it out now, step by step, following the pinned plan.\n\n{}",
            self.to_markdown()
        )
    }
}

/// Run state of plan mode. Cloning shares the slot, so the panel keeps one
/// handle and reads the plan after the run ends.
#[derive(Clone, Debug, Default)]
pub struct PlanMode {
    slot: Arc<Mutex<Option<Plan>>>,
}

impl PlanMode {
    pub fn new() -> Self {
        Self::default()
    }

    /// The submitted plan, leaving the slot empty.
    pub fn take(&self) -> Option<Plan> {
        self.slot.lock().ok().and_then(|mut slot| slot.take())
    }

    pub fn peek(&self) -> Option<Plan> {
        self.slot.lock().ok().and_then(|slot| slot.clone())
    }

    pub fn is_submitted(&self) -> bool {
        self.slot.lock().map(|slot| slot.is_some()).unwrap_or(false)
    }

    fn submit(&self, plan: Plan) {
        if let Ok(mut slot) = self.slot.lock() {
            *slot = Some(plan);
        }
    }
}

pub(crate) fn exit_plan_definition() -> ToolDefinition {
    ToolDefinition {
        name: EXIT_PLAN_MODE.to_string(),
        description: Some(
            "Present your finished plan to the user for approval and end plan mode. Call it once, after investigating."
                .to_string(),
        ),
        parameters_json_schema: exit_plan_schema(),
    }
}

fn exit_plan_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "goal": {"type": "string", "description": "One sentence: what the plan achieves."},
            "steps": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "title": {"type": "string"},
                        "detail": {"type": "string"},
                        "files": {"type": "array", "items": {"type": "string"}}
                    },
                    "required": ["title"]
                }
            },
            "risks": {"type": "array", "items": {"type": "string"}}
        },
        "required": ["goal", "steps"]
    })
}

/// Restrict `config` to read-only and register `exit_plan_mode`. Called by
/// `run_loop` before the delegation tools are installed, so a sub-agent this
/// run starts is capped at read-only as well.
pub(crate) fn enter(config: &mut RunConfig) {
    let Some(mode) = config.plan_mode.clone() else {
        return;
    };
    let available: Vec<String> = config
        .tool_registry
        .names()
        .iter()
        .map(|n| n.to_string())
        .collect();
    let allowed = match builtin_harnesses().into_iter().find(|h| h.name == "planner") {
        Some(mut planner) => {
            planner.tools = vec!["*".to_string()];
            planner.resolve_tools_with(
                &available,
                &config.safety.classifier,
                HarnessPermissions::ReadOnly,
            )
        }
        None => Vec::new(),
    };
    for name in &available {
        if !allowed.contains(name) {
            config.tool_registry.unregister(name);
        }
    }
    config.tools.retain(|tool| allowed.contains(&tool.name));

    config.safety.read_only = true;
    config.safety.classifier.set(EXIT_PLAN_MODE, ToolClass::Read);
    if let Some(handle) = config.delegation.as_mut() {
        handle.clamp_ceiling(HarnessPermissions::ReadOnly);
    }

    let definition = exit_plan_definition();
    config.tool_registry.register_fn(
        EXIT_PLAN_MODE,
        definition.description.as_deref().unwrap_or_default(),
        definition.parameters_json_schema.clone(),
        move |args, _ctx| match Plan::from_args(&args) {
            Ok(plan) => {
                let steps = plan.steps.len();
                mode.submit(plan);
                Ok(json!({
                    "ok": true,
                    "steps": steps,
                    "message": "Plan submitted for the user's review. Stop now and do not call other tools; a separate run carries it out only if they approve."
                }))
            }
            Err(err) => Err(anyhow::anyhow!(err)),
        },
    );
    config.tools.retain(|tool| tool.name != EXIT_PLAN_MODE);
    config.tools.push(definition);
}

/// The Lite router may leave `exit_plan_mode` out of a turn; the run must
/// always be able to finish.
pub(crate) fn ensure_exit_tool(config: &RunConfig, tools: &mut Vec<ToolDefinition>) {
    if config.plan_mode.is_some() && !tools.iter().any(|t| t.name == EXIT_PLAN_MODE) {
        tools.push(exit_plan_definition());
    }
}

/// Has the plan been submitted (so the run should end)?
pub(crate) fn submitted(config: &RunConfig) -> bool {
    config.plan_mode.as_ref().is_some_and(PlanMode::is_submitted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Plan {
        Plan {
            goal: "Add a health bar".to_string(),
            steps: vec![
                PlanStep {
                    title: "Add component".to_string(),
                    detail: "New Health component".to_string(),
                    files: vec!["src/health.rs".to_string(), "src/lib.rs".to_string()],
                },
                PlanStep {
                    title: "Draw it".to_string(),
                    detail: String::new(),
                    files: vec![],
                },
            ],
            risks: vec!["Breaks serialization".to_string()],
        }
    }

    #[test]
    fn markdown_round_trips() {
        let plan = sample();
        assert_eq!(Plan::from_markdown(&plan.to_markdown()).unwrap(), plan);
    }

    #[test]
    fn args_accept_loose_shapes_and_reject_empty_plans() {
        let plan = Plan::from_args(&json!({
            "goal": "g",
            "steps": ["read it", {"title": "do it", "files": "a.rs, b.rs"}],
        }))
        .unwrap();
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[1].files, vec!["a.rs", "b.rs"]);
        assert!(Plan::from_args(&json!({"goal": "g", "steps": []})).is_err());
        assert!(Plan::from_args(&json!({"steps": ["x"]})).is_err());
    }

    #[test]
    fn approved_plan_becomes_user_pins() {
        let mut ledger = Ledger::with_system_prompt("sys");
        assert_eq!(sample().apply_to(&mut ledger), 4);
        assert_eq!(ledger.pins_of(PinKind::Requirement).len(), 1);
        let decisions = ledger.pins_of(PinKind::Decision);
        assert_eq!(decisions.len(), 2);
        assert!(decisions[0].text.contains("src/health.rs"));
        assert!(decisions.iter().all(|p| p.user));
        // Approving twice does not duplicate.
        assert_eq!(sample().apply_to(&mut ledger), 0);
    }
}
