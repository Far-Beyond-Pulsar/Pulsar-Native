//! Run-start hooks for plan mode and project memory, kept out of `run.rs`.

use crate::ledger::Ledger;
use crate::memory::{self, REMEMBER};
use crate::plan::{self, PLAN_MODE_PROMPT};
use crate::run::RunConfig;
use crate::taint::ToolClass;
use agent_chat_core::ToolDefinition;

/// Everything that changes a run's tools or safety before it starts. Runs
/// before the delegation tools are installed.
pub(crate) fn prepare(config: &mut RunConfig) {
    // Plan mode first: it trims the registry, and `remember` (which writes
    // nothing itself) is registered after so it survives the trim.
    plan::enter(config);
    install_remember(config);
}

fn install_remember(config: &mut RunConfig) {
    let Some(ctx) = config.memory.clone() else {
        return;
    };
    config.safety.classifier.set(REMEMBER, ToolClass::Read);
    let taint = config.safety.taint.clone();
    config.tool_registry.register_fn(
        REMEMBER,
        memory::REMEMBER_DESCRIPTION,
        memory::remember_schema(),
        move |args, _ctx| memory::run_remember(&args, &ctx, taint.is_tainted()),
    );
    config.tools.retain(|tool| tool.name != REMEMBER);
    config.tools.push(ToolDefinition {
        name: REMEMBER.to_string(),
        description: Some(memory::REMEMBER_DESCRIPTION.to_string()),
        parameters_json_schema: memory::remember_schema(),
    });
}

/// Append the memory block and the plan-mode instructions to the system
/// prompt, in that fixed order. Returns the prompt as it was, for
/// [`Ledger::set_system_prompt`] after the run. The text added depends only
/// on the memory files and the mode, so it is byte-stable across requests.
pub(crate) fn extend_system_prompt(config: &RunConfig, ledger: &mut Ledger) -> Option<String> {
    let mut extra: Vec<String> = Vec::new();
    if let Some(ctx) = &config.memory {
        if let Some(block) = ctx.store.block(config.profile.is_lite()) {
            extra.push(block);
        }
    }
    if config.plan_mode.is_some() {
        extra.push(PLAN_MODE_PROMPT.to_string());
    }
    if extra.is_empty() {
        return None;
    }
    let original = ledger.system_prompt()?.to_string();
    ledger.set_system_prompt(format!("{original}\n\n{}", extra.join("\n\n")));
    Some(original)
}
