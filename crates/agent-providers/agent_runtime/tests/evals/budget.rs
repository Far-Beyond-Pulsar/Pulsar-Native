//! A 4k-context profile stays under its token budget on every request, even
//! when tools return far more than fits.

use agent_chat_core::{ChatRequest, ChatRole, ToolDefinition};
use agent_runtime::{
    run_loop,
    testing::{MockProvider, ScriptedStep},
    Budget, CancelToken, Ledger, LedgerEntry, ModelProfile, TokenEstimator,
};
use serde_json::json;

use crate::common::{config, sink};

fn is_summary(request: &ChatRequest) -> bool {
    request
        .messages
        .first()
        .is_some_and(|m| m.content.contains("conversation summarizer"))
}

#[test]
fn a_4k_profile_never_sends_more_than_its_budget() {
    let provider = MockProvider::from_fn(|request, index| {
        if is_summary(request) {
            return Ok(ScriptedStep::say("Earlier turns read several large files.").into_response());
        }
        // Alternate: read a big file, then answer.
        let last = request.messages.last().map(|m| m.role);
        Ok(if last == Some(ChatRole::Tool) {
            ScriptedStep::say(format!("Summary of file {index}."))
        } else {
            ScriptedStep::call("read_big", &format!("call-{index}"), json!({}))
        }
        .into_response())
    });

    let profile = ModelProfile::new(4_096, true);
    let budget = Budget::for_profile(&profile);
    let estimator = TokenEstimator::for_profile(&profile);

    let mut registry = agent_chat_tools::ToolRegistry::new();
    registry.register_fn(
        "read_big",
        "Read a large file",
        json!({"type": "object", "properties": {}}),
        |_, _| Ok(json!({"content": "lorem ipsum dolor sit amet ".repeat(1_200)})),
    );
    let tools = vec![ToolDefinition {
        name: "read_big".to_string(),
        description: Some("Read a large file".to_string()),
        parameters_json_schema: json!({"type": "object", "properties": {}}),
    }];

    let mut ledger = Ledger::with_system_prompt("You are a helpful assistant.");
    for turn in 0..40 {
        ledger.push(LedgerEntry::user(format!("Turn {turn}: read the next file.")));
        let mut run_config = config(provider.clone(), profile);
        run_config.tool_registry = registry.clone();
        run_config.tools = tools.clone();
        let (sink, _events) = sink();
        ledger = run_loop(run_config, ledger, CancelToken::new(), sink);
    }

    let limit = budget.context_tokens - budget.reserve_output;
    let mut worst = 0;
    for request in provider.requests() {
        let used = estimator.request(&request.messages, &request.tools);
        worst = worst.max(used);
        assert!(used <= limit, "request used ~{used} tokens, limit {limit}");
    }
    assert!(worst > 0);
}
