//! Cascade routing: a small primary model does the work; a larger escalation
//! model takes over for a step, or for the rest of the run, when the small one
//! gets stuck.
//!
//! Escalation triggers (each can be switched off by setting it to 0 / false):
//! - repeated malformed text-protocol tool calls (strikes),
//! - a streak of failed tool calls,
//! - loop detection (the same call, same arguments, repeated),
//! - an explicit low-confidence marker in the reply,
//! - a context overflow reported by the provider.
//!
//! The whole feature is off by default ([`CascadeSettings::enabled`]) and
//! costs nothing when off: `RunConfig::cascade` is `None` and the loop never
//! builds a [`CascadeState`].

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use agent_chat_core::{ChatProvider, ToolCall};
use serde::{Deserialize, Serialize};

use crate::profile::ModelProfile;

pub const CASCADE_SETTINGS_FILE: &str = "agent_cascade.json";

/// A reply containing this marker (case-insensitive) is a self-reported
/// low-confidence answer. [`LOW_CONFIDENCE_INSTRUCTION`] teaches it.
pub const LOW_CONFIDENCE_MARKER: &str = "[low-confidence]";

pub const LOW_CONFIDENCE_INSTRUCTION: &str = "If you are not confident in an answer or a tool call, \
start your reply with [low-confidence] and a larger model will take over.";

// ---------------------------------------------------------------------------
// Settings

/// How long an escalation lasts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EscalationScope {
    /// The escalation model answers the next few requests, then the primary
    /// model resumes.
    #[default]
    Step,
    /// The escalation model finishes the run.
    Run,
}

impl EscalationScope {
    pub fn label(self) -> &'static str {
        match self {
            EscalationScope::Step => "step",
            EscalationScope::Run => "run",
        }
    }
}

/// Persisted next to the other agent settings as `.pulsar/agent_cascade.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CascadeSettings {
    /// Off by default.
    pub enabled: bool,
    /// The larger model to escalate to. Without one nothing escalates.
    pub escalation_model: Option<String>,
    pub scope: EscalationScope,
    /// Requests an escalated *step* lasts.
    pub step_requests: u32,
    /// Malformed text-protocol replies in a row before escalating; 0 = never.
    pub malformed_strikes: u32,
    /// Failed tool calls in a row before escalating; 0 = never.
    pub failure_streak: u32,
    /// The same call this many times in a row is a loop; 0 = never.
    pub loop_repeats: u32,
    /// Honour [`LOW_CONFIDENCE_MARKER`].
    pub low_confidence: bool,
    /// Escalate (for the rest of the run) when the provider reports that the
    /// request did not fit the context window.
    pub context_overflow: bool,
}

impl Default for CascadeSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            escalation_model: None,
            scope: EscalationScope::Step,
            step_requests: 3,
            malformed_strikes: 2,
            failure_streak: 3,
            loop_repeats: 3,
            low_confidence: true,
            context_overflow: true,
        }
    }
}

impl CascadeSettings {
    pub fn path(project_root: &Path) -> PathBuf {
        project_root.join(".pulsar").join(CASCADE_SETTINGS_FILE)
    }

    /// Missing or unreadable files give the defaults (off).
    pub fn load(project_root: &Path) -> Self {
        std::fs::read_to_string(Self::path(project_root))
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, project_root: &Path) -> std::io::Result<()> {
        let path = Self::path(project_root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, body)
    }

    /// The runtime policy, or `None` when cascade routing is off or has no
    /// escalation model.
    pub fn policy(&self) -> Option<CascadePolicy> {
        let model = self.escalation_model.clone().filter(|m| !m.trim().is_empty())?;
        self.enabled.then(|| CascadePolicy::new(self.clone(), model))
    }
}

/// What a run needs to cascade. Build with [`CascadeSettings::policy`].
#[derive(Clone)]
pub struct CascadePolicy {
    pub settings: CascadeSettings,
    pub escalation_model: String,
    /// Provider that serves the escalation model; `None` uses the run's own.
    pub provider: Option<Arc<dyn ChatProvider>>,
    /// Profile of the escalation model (window, tool-calling mode); `None`
    /// keeps the run's profile.
    pub profile: Option<ModelProfile>,
}

impl CascadePolicy {
    pub fn new(settings: CascadeSettings, escalation_model: impl Into<String>) -> Self {
        Self {
            settings,
            escalation_model: escalation_model.into(),
            provider: None,
            profile: None,
        }
    }

    pub fn with_provider(mut self, provider: Arc<dyn ChatProvider>) -> Self {
        self.provider = Some(provider);
        self
    }

    pub fn with_profile(mut self, profile: ModelProfile) -> Self {
        self.profile = Some(profile);
        self
    }
}

// ---------------------------------------------------------------------------
// Events

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EscalationReason {
    MalformedToolCalls,
    ToolFailureStreak,
    LoopDetected,
    LowConfidence,
    ContextOverflow,
}

impl EscalationReason {
    pub fn describe(self) -> &'static str {
        match self {
            EscalationReason::MalformedToolCalls => "repeated malformed tool calls",
            EscalationReason::ToolFailureStreak => "a streak of failed tool calls",
            EscalationReason::LoopDetected => "the same call repeated in a loop",
            EscalationReason::LowConfidence => "a low-confidence answer",
            EscalationReason::ContextOverflow => "the context did not fit",
        }
    }
}

/// One escalation, as shown to the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Escalation {
    pub reason: EscalationReason,
    pub scope: EscalationScope,
    pub from_model: String,
    pub to_model: String,
}

impl Escalation {
    /// The line shown as an engine notice.
    pub fn notice(&self) -> String {
        let scope = match self.scope {
            EscalationScope::Step => "for the next step",
            EscalationScope::Run => "for the rest of the run",
        };
        format!(
            "Escalated from {} to {} {scope}: {}.",
            self.from_model,
            self.to_model,
            self.reason.describe()
        )
    }
}

// ---------------------------------------------------------------------------
// State

/// Which model the next request uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CascadeTier {
    Primary,
    Escalated,
}

/// Tracks the triggers across a run. Pure: no I/O, so each trigger is unit
/// tested without a provider.
pub struct CascadeState {
    policy: CascadePolicy,
    primary_model: String,
    malformed: u32,
    failures: u32,
    recent: Vec<String>,
    run_escalated: bool,
    step_left: u32,
    escalations: u32,
}

impl CascadeState {
    /// `None` when the escalation model is the primary one (nothing to
    /// escalate to).
    pub fn new(policy: &CascadePolicy, primary_model: &str) -> Option<Self> {
        if policy.escalation_model == primary_model {
            return None;
        }
        Some(Self {
            policy: policy.clone(),
            primary_model: primary_model.to_string(),
            malformed: 0,
            failures: 0,
            recent: Vec::new(),
            run_escalated: false,
            step_left: 0,
            escalations: 0,
        })
    }

    pub fn policy(&self) -> &CascadePolicy {
        &self.policy
    }

    pub fn escalation_count(&self) -> u32 {
        self.escalations
    }

    /// Whether the next request goes to the escalation model.
    pub fn is_escalated(&self) -> bool {
        self.run_escalated || self.step_left > 0
    }

    /// Call once per request: picks the model and spends one step request.
    pub fn begin_request(&mut self) -> CascadeTier {
        if self.run_escalated {
            CascadeTier::Escalated
        } else if self.step_left > 0 {
            self.step_left -= 1;
            CascadeTier::Escalated
        } else {
            CascadeTier::Primary
        }
    }

    /// The model a request of `tier` is sent to.
    pub fn model_for(&self, tier: CascadeTier) -> &str {
        match tier {
            CascadeTier::Primary => &self.primary_model,
            CascadeTier::Escalated => &self.policy.escalation_model,
        }
    }

    fn escalate(&mut self, reason: EscalationReason, scope: EscalationScope) -> Option<Escalation> {
        // Already on the bigger model: nobody is left to hand over to.
        if self.is_escalated() {
            return None;
        }
        self.malformed = 0;
        self.failures = 0;
        self.recent.clear();
        self.escalations += 1;
        match scope {
            EscalationScope::Run => self.run_escalated = true,
            EscalationScope::Step => self.step_left = self.policy.settings.step_requests.max(1),
        }
        Some(Escalation {
            reason,
            scope,
            from_model: self.primary_model.clone(),
            to_model: self.policy.escalation_model.clone(),
        })
    }

    /// A reply whose tool calls could not be parsed.
    pub fn note_malformed(&mut self) -> Option<Escalation> {
        if self.is_escalated() {
            return None;
        }
        let limit = self.policy.settings.malformed_strikes;
        self.malformed += 1;
        (limit > 0 && self.malformed >= limit)
            .then(|| self.escalate(EscalationReason::MalformedToolCalls, self.policy.settings.scope))
            .flatten()
    }

    /// A reply that parsed; ends the malformed streak.
    pub fn note_well_formed(&mut self) {
        self.malformed = 0;
    }

    /// One round of executed tool calls with whether each failed.
    pub fn note_tool_round(&mut self, round: &[(ToolCall, bool)]) -> Option<Escalation> {
        if self.is_escalated() {
            // Keep the streaks honest for when the primary model resumes.
            self.failures = 0;
            self.recent.clear();
            return None;
        }
        let settings = &self.policy.settings;
        let (fail_limit, loop_limit) = (settings.failure_streak, settings.loop_repeats);
        let scope = settings.scope;
        let mut reason = None;
        for (call, failed) in round {
            if *failed {
                self.failures += 1;
            } else {
                self.failures = 0;
            }
            self.recent.push(call_key(call));
            if loop_limit > 0 && repeats_at_tail(&self.recent) >= loop_limit as usize {
                reason.get_or_insert(EscalationReason::LoopDetected);
            }
            if fail_limit > 0 && self.failures >= fail_limit {
                reason.get_or_insert(EscalationReason::ToolFailureStreak);
            }
        }
        let keep = self.recent.len().saturating_sub(32);
        self.recent.drain(..keep);
        reason.and_then(|reason| self.escalate(reason, scope))
    }

    /// The assistant text of a reply, checked for the low-confidence marker.
    pub fn note_reply_text(&mut self, text: &str) -> Option<Escalation> {
        if !self.policy.settings.low_confidence || self.is_escalated() {
            return None;
        }
        has_low_confidence_marker(text)
            .then(|| self.escalate(EscalationReason::LowConfidence, self.policy.settings.scope))
            .flatten()
    }

    /// A provider error. `Some` means the request should be retried on the
    /// escalation model instead of ending the run.
    pub fn note_provider_error(&mut self, error: &str) -> Option<Escalation> {
        if !self.policy.settings.context_overflow || !is_context_overflow(error) {
            return None;
        }
        // The small window is the problem; a one-step escalation would only
        // overflow again afterwards.
        self.escalate(EscalationReason::ContextOverflow, EscalationScope::Run)
    }

    /// Whether the request should carry [`LOW_CONFIDENCE_INSTRUCTION`].
    pub fn wants_confidence_instruction(&self, tier: CascadeTier) -> bool {
        tier == CascadeTier::Primary && self.policy.settings.low_confidence
    }
}

fn call_key(call: &ToolCall) -> String {
    format!("{}\u{1f}{}", call.name, call.arguments_json)
}

/// How many identical keys end the list.
fn repeats_at_tail(keys: &[String]) -> usize {
    match keys.last() {
        Some(last) => keys.iter().rev().take_while(|key| *key == last).count(),
        None => 0,
    }
}

pub fn has_low_confidence_marker(text: &str) -> bool {
    text.to_ascii_lowercase().contains(LOW_CONFIDENCE_MARKER)
}

/// Provider error text that means the request did not fit the window.
pub fn is_context_overflow(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    [
        "context length",
        "context_length",
        "context window",
        "maximum context",
        "prompt is too long",
        "too many tokens",
        "exceeds the context",
        "input is too long",
        "token limit",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// `text` without the marker, for the history when an answer is kept.
pub fn strip_marker(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    match lower.find(LOW_CONFIDENCE_MARKER) {
        Some(at) => {
            let mut out = text[..at].to_string();
            out.push_str(&text[at + LOW_CONFIDENCE_MARKER.len()..]);
            out.trim().to_string()
        }
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn policy() -> CascadePolicy {
        CascadePolicy::new(
            CascadeSettings {
                enabled: true,
                escalation_model: Some("big".into()),
                ..CascadeSettings::default()
            },
            "big",
        )
    }

    fn call(name: &str, arg: i32) -> ToolCall {
        ToolCall {
            id: "1".into(),
            name: name.into(),
            arguments_json: json!({ "n": arg }),
        }
    }

    #[test]
    fn off_by_default() {
        assert!(CascadeSettings::default().policy().is_none());
        let mut settings = CascadeSettings::default();
        settings.enabled = true;
        assert!(settings.policy().is_none(), "no escalation model");
        settings.escalation_model = Some("big".into());
        assert!(settings.policy().is_some());
    }

    #[test]
    fn same_model_has_nothing_to_escalate_to() {
        assert!(CascadeState::new(&policy(), "big").is_none());
        assert!(CascadeState::new(&policy(), "small").is_some());
    }

    #[test]
    fn malformed_strikes_escalate_one_step_then_resume() {
        let mut state = CascadeState::new(&policy(), "small").unwrap();
        assert_eq!(state.begin_request(), CascadeTier::Primary);
        assert!(state.note_malformed().is_none());
        let esc = state.note_malformed().expect("second strike escalates");
        assert_eq!(esc.reason, EscalationReason::MalformedToolCalls);
        assert_eq!(esc.scope, EscalationScope::Step);
        assert!(esc.notice().contains("small") && esc.notice().contains("big"));
        for _ in 0..3 {
            assert_eq!(state.begin_request(), CascadeTier::Escalated);
        }
        assert_eq!(state.begin_request(), CascadeTier::Primary);
    }

    #[test]
    fn well_formed_reply_resets_strikes() {
        let mut state = CascadeState::new(&policy(), "small").unwrap();
        assert!(state.note_malformed().is_none());
        state.note_well_formed();
        assert!(state.note_malformed().is_none());
    }

    #[test]
    fn failure_streak_escalates_and_success_resets() {
        let mut state = CascadeState::new(&policy(), "small").unwrap();
        assert!(state.note_tool_round(&[(call("a", 1), true), (call("a", 2), true)]).is_none());
        assert!(state.note_tool_round(&[(call("a", 3), false)]).is_none());
        assert!(state.note_tool_round(&[(call("a", 4), true), (call("b", 5), true)]).is_none());
        let esc = state.note_tool_round(&[(call("c", 6), true)]).unwrap();
        assert_eq!(esc.reason, EscalationReason::ToolFailureStreak);
    }

    #[test]
    fn identical_calls_are_a_loop() {
        let mut state = CascadeState::new(&policy(), "small").unwrap();
        assert!(state.note_tool_round(&[(call("read", 1), false)]).is_none());
        assert!(state.note_tool_round(&[(call("read", 1), false)]).is_none());
        let esc = state.note_tool_round(&[(call("read", 1), false)]).unwrap();
        assert_eq!(esc.reason, EscalationReason::LoopDetected);
    }

    #[test]
    fn varied_calls_are_not_a_loop() {
        let mut state = CascadeState::new(&policy(), "small").unwrap();
        for n in 0..10 {
            assert!(state.note_tool_round(&[(call("read", n), false)]).is_none());
        }
    }

    #[test]
    fn low_confidence_marker_escalates() {
        let mut state = CascadeState::new(&policy(), "small").unwrap();
        assert!(state.note_reply_text("All good.").is_none());
        let esc = state.note_reply_text("[Low-Confidence] maybe the answer is 4").unwrap();
        assert_eq!(esc.reason, EscalationReason::LowConfidence);
        assert_eq!(strip_marker("[low-confidence] maybe"), "maybe");
    }

    #[test]
    fn context_overflow_escalates_for_the_run() {
        let mut state = CascadeState::new(&policy(), "small").unwrap();
        assert!(state.note_provider_error("HTTP 500 boom").is_none());
        let esc = state
            .note_provider_error("This model's maximum context length is 4096 tokens")
            .unwrap();
        assert_eq!(esc.reason, EscalationReason::ContextOverflow);
        assert_eq!(esc.scope, EscalationScope::Run);
        for _ in 0..10 {
            assert_eq!(state.begin_request(), CascadeTier::Escalated);
        }
        // Once on the big model another overflow is a real error.
        assert!(state.note_provider_error("context length exceeded").is_none());
    }

    #[test]
    fn triggers_can_be_disabled() {
        let mut p = policy();
        p.settings.malformed_strikes = 0;
        p.settings.failure_streak = 0;
        p.settings.loop_repeats = 0;
        p.settings.low_confidence = false;
        p.settings.context_overflow = false;
        let mut state = CascadeState::new(&p, "small").unwrap();
        for _ in 0..10 {
            assert!(state.note_malformed().is_none());
            assert!(state.note_tool_round(&[(call("x", 1), true)]).is_none());
        }
        assert!(state.note_reply_text("[low-confidence]").is_none());
        assert!(state.note_provider_error("context length").is_none());
    }

    #[test]
    fn settings_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!CascadeSettings::load(dir.path()).enabled);
        let settings = CascadeSettings {
            enabled: true,
            escalation_model: Some("big".into()),
            scope: EscalationScope::Run,
            ..CascadeSettings::default()
        };
        settings.save(dir.path()).unwrap();
        assert_eq!(CascadeSettings::load(dir.path()), settings);
    }
}
