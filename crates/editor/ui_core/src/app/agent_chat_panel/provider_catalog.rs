use super::*;

impl AgentChatPanel {
    /// Leak a heap-allocated string into a `&'static str`.
    /// Used when dynamic provider metadata needs to live as long as the program.
    pub(super) fn static_str(value: String) -> &'static str {
        Box::leak(value.into_boxed_str())
    }
}
