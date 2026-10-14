//! What the projector, budgeter and loop need to know about a model.
//!
//! A profile is resolved in layers, later layers winning:
//! 1. conservative defaults,
//! 2. the name-pattern table ([`PATTERNS`]; replaces the old
//!    `infer_context_tokens` ladder),
//! 3. the provider's [`ModelDescriptor`] (a known context size, tools support),
//! 4. a per-model user override ([`ProfileOverride`], persisted by
//!    [`ProfileOverrides`]).

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
    time::SystemTime,
};

use agent_chat_core::ModelDescriptor;
use serde::{Deserialize, Serialize};

/// Context assumed when neither the descriptor nor the table knows.
pub const UNKNOWN_CONTEXT_TOKENS: u32 = 8_192;
/// Models below this window run in Lite mode.
pub const LITE_CONTEXT_LIMIT: u32 = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCalling {
    /// Reliable structured tool calls.
    Native,
    /// Structured tool calls that small models often get wrong.
    Weak,
    /// No tool calling: the runtime speaks the fenced text protocol
    /// ([`crate::text_tools`]).
    None,
}

/// How the model exposes its reasoning.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Thinking {
    None,
    /// `<think>…</think>` inside the text stream.
    Tagged,
    /// Reasoning arrives through a separate API channel.
    Native,
}

/// The strongest output constraint the backend can enforce.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JsonMode {
    None,
    Object,
    Schema,
    Grammar,
}

/// Which tokenizer family to approximate when the provider reports no counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenizerHint {
    Generic,
    Tiktoken,
    Claude,
    Llama,
    Qwen,
    Gemini,
    Mistral,
}

impl TokenizerHint {
    /// Average characters per token for plain prose.
    pub fn chars_per_token(self) -> f32 {
        match self {
            TokenizerHint::Generic => 3.5,
            TokenizerHint::Tiktoken => 4.0,
            TokenizerHint::Claude => 3.5,
            TokenizerHint::Llama => 3.6,
            TokenizerHint::Qwen => 3.8,
            TokenizerHint::Gemini => 4.0,
            TokenizerHint::Mistral => 3.5,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Full,
    /// Small context: tool router, compact prompts, hard output caps.
    Lite,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelProfile {
    /// Context window in tokens. 0 means unknown (legacy char budgets apply).
    pub context_tokens: u32,
    pub tool_calling: ToolCalling,
    pub parallel_tools: bool,
    /// May a system message appear after the first turn?
    pub accepts_midstream_system: bool,
    pub thinking: Thinking,
    pub json_mode: JsonMode,
    pub tokenizer_hint: TokenizerHint,
    pub tier: Tier,
    pub temperature: f32,
    pub max_tokens: u32,
    /// Accepts image input. Tool results that carry captures (viewport,
    /// game frames) attach them only when this is set; other models get a
    /// text description instead.
    pub vision: bool,
}

impl ModelProfile {
    /// A profile for a model with unknown capabilities.
    pub fn unknown() -> Self {
        Self {
            context_tokens: 0,
            tool_calling: ToolCalling::Native,
            parallel_tools: true,
            accepts_midstream_system: true,
            thinking: Thinking::None,
            json_mode: JsonMode::None,
            tokenizer_hint: TokenizerHint::Generic,
            tier: Tier::Full,
            temperature: 0.2,
            max_tokens: 8_192,
            vision: false,
        }
    }

    pub fn new(context_tokens: u32, supports_tools: bool) -> Self {
        let mut profile = Self {
            context_tokens,
            tool_calling: if supports_tools {
                ToolCalling::Native
            } else {
                ToolCalling::None
            },
            ..Self::unknown()
        };
        profile.derive_sampling();
        profile
    }

    pub fn is_lite(&self) -> bool {
        self.tier == Tier::Lite
    }

    /// Context to budget against; unknown windows get a conservative guess.
    pub fn effective_context(&self) -> u32 {
        if self.context_tokens == 0 {
            UNKNOWN_CONTEXT_TOKENS
        } else {
            self.context_tokens
        }
    }

    /// Recompute tier, temperature and max_tokens from the context window.
    /// Called after the window or tier changes; explicit overrides are
    /// applied afterwards.
    pub fn derive_sampling(&mut self) {
        if self.context_tokens == 0 {
            self.tier = Tier::Full;
            self.temperature = 0.2;
            self.max_tokens = 8_192;
            return;
        }
        self.tier = if self.context_tokens < LITE_CONTEXT_LIMIT {
            Tier::Lite
        } else {
            Tier::Full
        };
        match self.tier {
            Tier::Lite => {
                self.temperature = 0.1;
                self.max_tokens = (self.context_tokens / 4).clamp(256, 2_048);
            }
            Tier::Full => {
                self.temperature = 0.2;
                self.max_tokens = (self.context_tokens / 4).clamp(256, 8_192);
            }
        }
    }
}

impl Default for ModelProfile {
    fn default() -> Self {
        Self::unknown()
    }
}

// ── Name-pattern table ──────────────────────────────────────────────────────

enum Match {
    /// Lower-cased id contains any of these.
    Any(&'static [&'static str]),
    Exact(&'static str),
}

/// What a row says; `None` fields leave the earlier layer alone.
#[derive(Default)]
struct Traits {
    context: Option<u32>,
    tool_calling: Option<ToolCalling>,
    parallel_tools: Option<bool>,
    midstream_system: Option<bool>,
    thinking: Option<Thinking>,
    json_mode: Option<JsonMode>,
    tokenizer: Option<TokenizerHint>,
}

struct Row {
    matcher: Match,
    traits: Traits,
}

macro_rules! row {
    ($matcher:expr, $($field:ident : $value:expr),* $(,)?) => {
        Row {
            matcher: $matcher,
            traits: Traits { $($field: Some($value),)* ..Traits::default() },
        }
    };
}

/// First matching row wins, so specific names come before general families.
fn patterns() -> &'static [Row] {
    use JsonMode as J;
    use Match::{Any, Exact};
    use Thinking as T;
    use TokenizerHint as H;
    use ToolCalling as C;
    static TABLE: std::sync::OnceLock<Vec<Row>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        vec![
            row!(Any(&["gpt-4.1"]), context: 1_047_576, tokenizer: H::Tiktoken, json_mode: J::Schema),
            row!(Any(&["gpt-4o"]), context: 128_000, tokenizer: H::Tiktoken, json_mode: J::Schema),
            row!(Any(&["o4-mini"]), context: 200_000, tokenizer: H::Tiktoken, thinking: T::Native, json_mode: J::Schema),
            row!(Exact("o3"), context: 200_000, tokenizer: H::Tiktoken, thinking: T::Native, json_mode: J::Schema),
            row!(Any(&["gpt-5"]), context: 200_000, tokenizer: H::Tiktoken, thinking: T::Native, json_mode: J::Schema),
            row!(Any(&["claude"]), context: 200_000, tokenizer: H::Claude, thinking: T::Native),
            row!(Any(&["gemini"]), context: 1_048_576, tokenizer: H::Gemini, json_mode: J::Schema),
            row!(Any(&["codestral"]), context: 256_000, tokenizer: H::Mistral),
            row!(Any(&["mistral", "ministral"]), context: 128_000, tokenizer: H::Mistral),
            row!(Any(&["mixtral"]), context: 32_768, tokenizer: H::Mistral),
            row!(Any(&["tinyllama"]), context: 2_048, tool_calling: C::None, tokenizer: H::Llama),
            row!(Any(&["llama"]), context: 131_072, tokenizer: H::Llama),
            row!(Any(&["qwq", "qwen3"]), context: 131_072, tokenizer: H::Qwen, thinking: T::Tagged),
            row!(Any(&["qwen"]), context: 131_072, tokenizer: H::Qwen),
            row!(Any(&["deepseek-reasoner", "deepseek-r1"]), context: 131_072, thinking: T::Tagged),
            row!(Any(&["deepseek"]), context: 65_536),
            row!(Any(&["grok"]), context: 131_072),
            row!(Any(&["command-a"]), context: 256_000),
            row!(Any(&["command-r"]), context: 128_000),
            row!(Any(&["sonar"]), context: 200_000),
            row!(Any(&["phi-4", "phi4"]), context: 16_384, midstream_system: true),
            row!(Any(&["phi-3", "phi3"]), context: 4_096, tool_calling: C::None),
            row!(Any(&["gemma"]), context: 32_768, midstream_system: false, tool_calling: C::Weak),
            row!(Any(&["smollm"]), context: 8_192, tool_calling: C::None),
        ]
    })
}

/// Context window for a model id from the table alone. Replaces the retired
/// `infer_context_tokens` ladder.
pub fn infer_context_tokens(model_id: &str) -> Option<u32> {
    let id = model_id.to_ascii_lowercase();
    patterns()
        .iter()
        .find(|row| row.matches(&id))
        .and_then(|row| row.traits.context)
}

impl Row {
    fn matches(&self, id: &str) -> bool {
        match &self.matcher {
            Match::Any(needles) => needles.iter().any(|needle| id.contains(needle)),
            Match::Exact(name) => id == *name,
        }
    }
}

/// Does the model id name a model that accepts images? A separate list from
/// the first-match table because vision cuts across families (qwen2.5-vl,
/// llama3.2-vision, gemma3). The user override has the last word.
pub fn infer_vision(model_id: &str) -> bool {
    let id = model_id.to_ascii_lowercase();
    // Known text-only members of vision families.
    const TEXT_ONLY: &[&str] = &[
        "claude-2",
        "claude-1",
        "claude-instant",
        "gemma3:1b",
        "gemma3:270m",
        "gemma-3-1b",
        "gemma-3-270m",
        "gpt-4o-audio",
        "gpt-4o-realtime",
        "embedding",
    ];
    if TEXT_ONLY.iter().any(|needle| id.contains(needle)) {
        return false;
    }
    const VISION: &[&str] = &[
        "gpt-4o",
        "gpt-4.1",
        "gpt-4-turbo",
        "gpt-4-vision",
        "gpt-5",
        "o4-mini",
        "claude",
        "gemini",
        "llava",
        "bakllava",
        "pixtral",
        "vision",
        "minicpm-v",
        "moondream",
        "internvl",
        "gemma3",
        "gemma-3",
        "llama-4",
        "llama4",
        "mistral-small-3",
        "mistral-medium",
        "-vl",
        "vl-",
        "qwen2.5vl",
        "qwen3vl",
    ];
    // `o3` and its dated/pro snapshots see images; `o3-mini` does not.
    id == "o3" || id.starts_with("o3-20") || id.starts_with("o3-pro") || VISION.iter().any(|needle| id.contains(needle))
}

/// Parameter count in billions from a name like `llama3.2:3b` or
/// `qwen2.5-coder-7b-instruct`.
pub fn parse_param_billions(model_id: &str) -> Option<f32> {
    let id = model_id.to_ascii_lowercase();
    let bytes = id.as_bytes();
    let mut best: Option<f32> = None;
    for (ix, &byte) in bytes.iter().enumerate() {
        if byte != b'b' {
            continue;
        }
        // A size is `<digits>[.<digits>]b` with a non-alphanumeric boundary
        // after the `b` ("4bit" and "bf16" are not sizes).
        let after = bytes.get(ix + 1).copied();
        if after.is_some_and(|c| c.is_ascii_alphanumeric()) {
            continue;
        }
        let mut start = ix;
        while start > 0 && (bytes[start - 1].is_ascii_digit() || bytes[start - 1] == b'.') {
            start -= 1;
        }
        if start == ix {
            continue;
        }
        if start > 0 && bytes[start - 1].is_ascii_alphabetic() {
            continue;
        }
        if let Ok(value) = id[start..ix].parse::<f32>() {
            best = Some(best.map_or(value, |b| b.max(value)));
        }
    }
    best
}

// ── Resolution ──────────────────────────────────────────────────────────────

/// Everything resolution may look at.
#[derive(Clone, Copy, Default)]
pub struct ResolveInput<'a> {
    /// Provider (or provider template) id; picks the constrained-decoding
    /// mode its backend offers. May be empty.
    pub provider_id: &'a str,
    pub model_id: &'a str,
    pub descriptor: Option<&'a ModelDescriptor>,
    /// Self-hosted models run in a window the user configured, usually far
    /// smaller than the family's maximum.
    pub local: bool,
    pub overrides: Option<&'a ProfileOverride>,
}

/// What the bundled provider backends can enforce (see
/// `agent_provider_openai`): Ollama takes a JSON schema in `format`,
/// llama.cpp a GBNF grammar, vLLM guided JSON, LM Studio a json_schema
/// response format, OpenAI json_schema.
pub fn provider_json_mode(provider_id: &str) -> Option<JsonMode> {
    match provider_id.split(':').next().unwrap_or("") {
        "ollama" | "vllm" | "lm_studio" | "openai" => Some(JsonMode::Schema),
        "llama_cpp" => Some(JsonMode::Grammar),
        _ => None,
    }
}

pub fn resolve_profile(input: ResolveInput<'_>) -> ModelProfile {
    let id = input.model_id.to_ascii_lowercase();
    let mut profile = ModelProfile::unknown();
    profile.context_tokens = UNKNOWN_CONTEXT_TOKENS;
    if input.local {
        profile.tool_calling = ToolCalling::Weak;
        profile.parallel_tools = false;
    }

    let mut table_context = None;
    if let Some(row) = patterns().iter().find(|row| row.matches(&id)) {
        let t = &row.traits;
        table_context = t.context;
        if let Some(v) = t.context {
            profile.context_tokens = v;
        }
        if let Some(v) = t.tool_calling {
            profile.tool_calling = v;
        }
        if let Some(v) = t.parallel_tools {
            profile.parallel_tools = v;
        }
        if let Some(v) = t.midstream_system {
            profile.accepts_midstream_system = v;
        }
        if let Some(v) = t.thinking {
            profile.thinking = v;
        }
        if let Some(v) = t.json_mode {
            profile.json_mode = v;
        }
        if let Some(v) = t.tokenizer {
            profile.tokenizer_hint = v;
        }
    }

    if let Some(mode) = provider_json_mode(input.provider_id) {
        profile.json_mode = mode;
    }
    profile.vision = infer_vision(&id);

    // Small self-hosted models are not served at their family's maximum.
    if input.local {
        if let Some(billions) = parse_param_billions(&id) {
            let cap = if billions <= 9.0 {
                8_192
            } else if billions <= 40.0 {
                32_768
            } else {
                u32::MAX
            };
            profile.context_tokens = profile.context_tokens.min(cap);
            if billions <= 4.0 && profile.tool_calling == ToolCalling::Native {
                profile.tool_calling = ToolCalling::Weak;
            }
        }
    }

    if let Some(descriptor) = input.descriptor {
        if descriptor.context_tokens > 0 {
            profile.context_tokens = descriptor.context_tokens;
        } else if table_context.is_none() && !input.local {
            profile.context_tokens = UNKNOWN_CONTEXT_TOKENS;
        }
        if !descriptor.supports_tools {
            profile.tool_calling = ToolCalling::None;
        }
    }

    profile.derive_sampling();
    if let Some(ov) = input.overrides {
        ov.apply(&mut profile);
    }
    profile
}

// ── User overrides ──────────────────────────────────────────────────────────

/// Fields the user pinned for one model. Absent fields follow resolution.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileOverride {
    pub context_tokens: Option<u32>,
    pub tool_calling: Option<ToolCalling>,
    pub parallel_tools: Option<bool>,
    pub accepts_midstream_system: Option<bool>,
    pub thinking: Option<Thinking>,
    pub json_mode: Option<JsonMode>,
    pub tokenizer_hint: Option<TokenizerHint>,
    pub tier: Option<Tier>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub vision: Option<bool>,
}

impl ProfileOverride {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn apply(&self, profile: &mut ModelProfile) {
        if let Some(v) = self.context_tokens {
            profile.context_tokens = v;
            // A new window changes the derived tier and sampling.
            profile.derive_sampling();
        }
        if let Some(v) = self.tool_calling {
            profile.tool_calling = v;
        }
        if let Some(v) = self.parallel_tools {
            profile.parallel_tools = v;
        }
        if let Some(v) = self.accepts_midstream_system {
            profile.accepts_midstream_system = v;
        }
        if let Some(v) = self.thinking {
            profile.thinking = v;
        }
        if let Some(v) = self.json_mode {
            profile.json_mode = v;
        }
        if let Some(v) = self.tokenizer_hint {
            profile.tokenizer_hint = v;
        }
        if let Some(v) = self.tier {
            profile.tier = v;
        }
        if let Some(v) = self.temperature {
            profile.temperature = v;
        }
        if let Some(v) = self.max_tokens {
            profile.max_tokens = v;
        }
        if let Some(v) = self.vision {
            profile.vision = v;
        }
    }
}

/// Overrides keyed by `provider_id/model_id`, persisted as JSON.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProfileOverrides {
    #[serde(default)]
    pub models: HashMap<String, ProfileOverride>,
}

impl ProfileOverrides {
    pub fn key(provider_id: &str, model_id: &str) -> String {
        format!("{provider_id}/{model_id}")
    }

    pub fn get(&self, provider_id: &str, model_id: &str) -> Option<&ProfileOverride> {
        self.models.get(&Self::key(provider_id, model_id))
    }

    /// An empty override removes the entry.
    pub fn set(&mut self, provider_id: &str, model_id: &str, value: ProfileOverride) {
        let key = Self::key(provider_id, model_id);
        if value.is_empty() {
            self.models.remove(&key);
        } else {
            self.models.insert(key, value);
        }
    }

    /// A missing or unreadable file is an empty set.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        std::fs::write(path, text)
    }
}

/// Process-wide cache of the overrides file, reloaded when it changes on
/// disk, so the panel can resolve a profile on every frame cheaply.
struct Cached {
    path: PathBuf,
    modified: Option<SystemTime>,
    overrides: ProfileOverrides,
}

static CACHE: Mutex<Option<Cached>> = Mutex::new(None);

/// Overrides stored at `path`, cached between calls.
pub fn cached_overrides(path: &Path) -> ProfileOverrides {
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let Ok(mut guard) = CACHE.lock() else {
        return ProfileOverrides::load(path);
    };
    if let Some(cached) = guard.as_ref() {
        if cached.path == path && cached.modified == modified {
            return cached.overrides.clone();
        }
    }
    let overrides = ProfileOverrides::load(path);
    *guard = Some(Cached {
        path: path.to_path_buf(),
        modified,
        overrides: overrides.clone(),
    });
    overrides
}

/// Resolve with the persisted overrides at `overrides_path` (if any).
pub fn resolve_with_store(
    overrides_path: Option<&Path>,
    provider_id: &str,
    input: ResolveInput<'_>,
) -> ModelProfile {
    let overrides = overrides_path.map(cached_overrides);
    let found = overrides
        .as_ref()
        .and_then(|set| set.get(provider_id, input.model_id));
    resolve_profile(ResolveInput {
        provider_id,
        overrides: found,
        ..input
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(id: &str, local: bool) -> ModelProfile {
        resolve_profile(ResolveInput {
            model_id: id,
            local,
            ..ResolveInput::default()
        })
    }

    #[test]
    fn vision_follows_the_name_table() {
        for id in [
            "gpt-4o-mini",
            "gpt-4.1",
            "claude-sonnet-4-5",
            "gemini-2.5-pro",
            "llava:7b",
            "llama3.2-vision:11b",
            "qwen2.5-vl-7b-instruct",
            "gemma3:4b",
            "o3",
        ] {
            assert!(resolve(id, false).vision, "{id} should see images");
        }
        for id in [
            "gpt-3.5-turbo",
            "llama3.1:8b",
            "qwen2.5-coder-7b",
            "deepseek-r1",
            "claude-2.1",
            "gemma3:1b",
            "tinyllama",
            "o3-mini",
        ] {
            assert!(!resolve(id, false).vision, "{id} should be text-only");
        }
        assert!(!ModelProfile::unknown().vision);
    }

    #[test]
    fn vision_override_wins_both_ways() {
        let on = ProfileOverride {
            vision: Some(true),
            ..ProfileOverride::default()
        };
        let off = ProfileOverride {
            vision: Some(false),
            ..ProfileOverride::default()
        };
        let resolve_with = |id: &str, ov: &ProfileOverride| {
            resolve_profile(ResolveInput {
                model_id: id,
                overrides: Some(ov),
                ..ResolveInput::default()
            })
        };
        assert!(resolve_with("llama3.1:8b", &on).vision);
        assert!(!resolve_with("gpt-4o", &off).vision);
        assert!(!on.is_empty());
        let json = serde_json::to_string(&on).unwrap();
        assert_eq!(serde_json::from_str::<ProfileOverride>(&json).unwrap(), on);
        // Old override files without the field still load.
        assert_eq!(
            serde_json::from_str::<ProfileOverride>("{}").unwrap().vision,
            None
        );
    }

    #[test]
    fn table_reproduces_the_old_ladder() {
        let cases = [
            ("gpt-4.1-mini", 1_047_576),
            ("gpt-4o", 128_000),
            ("o4-mini", 200_000),
            ("o3", 200_000),
            ("gpt-5-codex", 200_000),
            ("claude-sonnet-4", 200_000),
            ("gemini-2.5-pro", 1_048_576),
            ("codestral-latest", 256_000),
            ("ministral-8b", 128_000),
            ("mixtral-8x7b", 32_768),
            ("llama-3.3-70b", 131_072),
            ("qwen2.5-coder", 131_072),
            ("deepseek-reasoner", 131_072),
            ("deepseek-chat", 65_536),
            ("grok-4", 131_072),
            ("command-a-03", 256_000),
            ("command-r-plus", 128_000),
            ("sonar-pro", 200_000),
            ("phi-4", 16_384),
            ("gemma-2-9b", 32_768),
        ];
        for (id, expected) in cases {
            assert_eq!(infer_context_tokens(id), Some(expected), "{id}");
        }
        assert_eq!(infer_context_tokens("totally-unknown"), None);
        // "o3" is exact: it must not catch every id containing it.
        assert_eq!(infer_context_tokens("foo-o3-bar"), None);
    }

    #[test]
    fn specific_rows_beat_family_rows() {
        let qwen3 = resolve("qwen3-coder", false);
        assert_eq!(qwen3.thinking, Thinking::Tagged);
        assert_eq!(qwen3.tokenizer_hint, TokenizerHint::Qwen);
        assert_eq!(resolve("tinyllama", false).context_tokens, 2_048);
        assert_eq!(resolve("llama-3.1", false).context_tokens, 131_072);
    }

    #[test]
    fn cloud_models_are_native_and_full() {
        let profile = resolve("claude-sonnet-4", false);
        assert_eq!(profile.tool_calling, ToolCalling::Native);
        assert_eq!(profile.tier, Tier::Full);
        assert!(profile.parallel_tools);
        assert_eq!(profile.max_tokens, 8_192);
        assert_eq!(profile.thinking, Thinking::Native);
    }

    #[test]
    fn small_local_models_are_lite_and_weak() {
        let profile = resolve("llama3.2:3b", true);
        assert_eq!(profile.context_tokens, 8_192);
        assert_eq!(profile.tier, Tier::Lite);
        assert_eq!(profile.tool_calling, ToolCalling::Weak);
        assert!(!profile.parallel_tools);
        assert_eq!(profile.max_tokens, 2_048);
        assert!(profile.temperature < 0.2);
    }

    #[test]
    fn unknown_models_get_a_conservative_window() {
        let profile = resolve("my-finetune", true);
        assert_eq!(profile.context_tokens, UNKNOWN_CONTEXT_TOKENS);
        assert!(profile.is_lite());
        assert_eq!(profile.tool_calling, ToolCalling::Weak);
    }

    #[test]
    fn descriptor_beats_the_table() {
        let descriptor = ModelDescriptor {
            id: "llama3".into(),
            label: "llama3".into(),
            supports_tools: false,
            context_tokens: 4_096,
            compact_model: None,
        };
        let profile = resolve_profile(ResolveInput {
            model_id: "llama3",
            descriptor: Some(&descriptor),
            local: true,
            ..ResolveInput::default()
        });
        assert_eq!(profile.context_tokens, 4_096);
        assert_eq!(profile.tool_calling, ToolCalling::None);
        assert_eq!(profile.max_tokens, 1_024);
        assert!(profile.is_lite());
    }

    #[test]
    fn override_beats_everything() {
        let ov = ProfileOverride {
            context_tokens: Some(32_768),
            tool_calling: Some(ToolCalling::Native),
            temperature: Some(0.7),
            ..ProfileOverride::default()
        };
        let profile = resolve_profile(ResolveInput {
            model_id: "llama3.2:3b",
            local: true,
            overrides: Some(&ov),
            ..ResolveInput::default()
        });
        assert_eq!(profile.context_tokens, 32_768);
        assert_eq!(profile.tier, Tier::Full);
        assert_eq!(profile.tool_calling, ToolCalling::Native);
        assert_eq!(profile.temperature, 0.7);
        assert_eq!(profile.max_tokens, 8_192);
    }

    #[test]
    fn tier_can_be_forced() {
        let ov = ProfileOverride {
            tier: Some(Tier::Lite),
            ..ProfileOverride::default()
        };
        let profile = resolve_profile(ResolveInput {
            model_id: "gpt-4o",
            overrides: Some(&ov),
            ..ResolveInput::default()
        });
        assert!(profile.is_lite());
        assert_eq!(profile.context_tokens, 128_000);
    }

    #[test]
    fn parses_parameter_counts() {
        assert_eq!(parse_param_billions("llama3.2:3b"), Some(3.0));
        assert_eq!(parse_param_billions("qwen2.5-coder-7b-instruct"), Some(7.0));
        assert_eq!(parse_param_billions("phi-3.5-mini-3.8b"), Some(3.8));
        assert_eq!(parse_param_billions("model-70b-q4"), Some(70.0));
        assert_eq!(parse_param_billions("gemma-2-9b-it-4bit"), Some(9.0));
        assert_eq!(parse_param_billions("gpt-4o"), None);
        assert_eq!(parse_param_billions("bf16-model"), None);
    }

    #[test]
    fn overrides_persist_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".pulsar").join("agent_profiles.json");
        assert_eq!(ProfileOverrides::load(&path), ProfileOverrides::default());

        let mut set = ProfileOverrides::default();
        set.set(
            "ollama",
            "llama3.2:3b",
            ProfileOverride {
                context_tokens: Some(4_096),
                tool_calling: Some(ToolCalling::None),
                ..ProfileOverride::default()
            },
        );
        set.save(&path).unwrap();

        let loaded = ProfileOverrides::load(&path);
        assert_eq!(loaded, set);
        let profile = resolve_with_store(
            Some(&path),
            "ollama",
            ResolveInput {
                model_id: "llama3.2:3b",
                local: true,
                ..ResolveInput::default()
            },
        );
        assert_eq!(profile.context_tokens, 4_096);
        assert_eq!(profile.tool_calling, ToolCalling::None);

        // Another provider's model is unaffected.
        let other = resolve_with_store(
            Some(&path),
            "lm_studio",
            ResolveInput {
                model_id: "llama3.2:3b",
                local: true,
                ..ResolveInput::default()
            },
        );
        assert_eq!(other.context_tokens, 8_192);

        set.set("ollama", "llama3.2:3b", ProfileOverride::default());
        assert!(set.models.is_empty());
    }
}
