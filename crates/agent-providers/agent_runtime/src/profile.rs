//! What the projector and loop need to know about a model. Phase 3 grows
//! this (parallel tools, thinking, json mode, tokenizer hint).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolCalling {
    /// Reliable structured tool calls.
    Native,
    /// Structured tool calls that small models often get wrong.
    Weak,
    /// No tool calling: exchanges are rendered as plain text.
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelProfile {
    pub context_tokens: u32,
    pub tool_calling: ToolCalling,
}

impl ModelProfile {
    /// A profile for a model with unknown capabilities.
    pub fn unknown() -> Self {
        Self {
            context_tokens: 0,
            tool_calling: ToolCalling::Native,
        }
    }

    pub fn new(context_tokens: u32, supports_tools: bool) -> Self {
        Self {
            context_tokens,
            tool_calling: if supports_tools {
                ToolCalling::Native
            } else {
                ToolCalling::None
            },
        }
    }
}

impl Default for ModelProfile {
    fn default() -> Self {
        Self::unknown()
    }
}
