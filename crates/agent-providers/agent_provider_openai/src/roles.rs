//! Mapping chat roles to the role names the wire formats use.

use agent_chat_core::ChatRole;

pub(crate) fn map_role(role: ChatRole) -> &'static str {
    match role {
        ChatRole::System => "system",
        ChatRole::User => "user",
        ChatRole::Assistant => "assistant",
        ChatRole::Tool => "tool",
        ChatRole::AgentEvent => "system",
    }
}
