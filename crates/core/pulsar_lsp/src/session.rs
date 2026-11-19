//! A server's state machine, free of the process and of GPUI: status,
//! work-done progress, and the per-spec readiness rule. The entity
//! ([`crate::LanguageServer`]) feeds it what the reader thread decoded and
//! emits the events it returns, so every rule here is testable without a
//! process.
//!
//! Readiness is a rule of the spec ([`ReadinessRule`]), never a timer: the
//! only clock is the spec's hard cap (`max_indexing`), which declares a
//! server ready (with a warning) when it has not said so by then, so no
//! status is ever stuck for good.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use pulsar_language::spec::{
    LanguageServerSpec, ProgressEvent, ProgressKind, ReadinessRule, ServerSignal,
};

/// Where a server is in its life.
#[derive(Clone, Debug, PartialEq)]
pub enum ServerStatus {
    /// Not started.
    Idle,
    /// Locating, spawning, handshaking.
    Starting,
    /// Working: `progress` 0.0 to 1.0, `message` what it is doing.
    Indexing { progress: f32, message: String },
    Ready,
    Error(String),
    /// The server, or the runtime it needs, is not installed: the message says
    /// what is missing and what to do. The footer offers the spec's install
    /// action next to it when the spec has one.
    Missing(String),
    Stopped,
}

impl ServerStatus {
    /// Whether a process is alive (starting, working or ready).
    pub fn is_running(&self) -> bool {
        matches!(
            self,
            Self::Starting | Self::Indexing { .. } | Self::Ready
        )
    }

    /// Whether the server has answered `initialize` (so it can take documents
    /// and requests).
    pub fn is_initialized(&self) -> bool {
        matches!(self, Self::Indexing { .. } | Self::Ready)
    }
}

/// Why a server stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// A person (or a restart) asked.
    Requested,
    /// Its plugin was disabled, or it was switched off in its settings.
    Disabled,
    /// The project changed.
    ProjectChanged,
    /// No file of it was open for the idle period.
    Idle,
    /// The process ended on its own.
    Exited,
}

/// What a server reports to whoever watches it.
#[derive(Clone, Debug, PartialEq)]
pub enum ServerEvent {
    StatusChanged(ServerStatus),
    /// Work in progress (coalesced: at most one per 100 ms unless progress
    /// crossed a 5% step).
    Progress { progress: f32, message: String },
    /// First time the server is ready after a (re)start.
    Ready,
    Error(String),
    Stopped(StopReason),
}

/// Minimum time between emitted `Progress` events: raw updates arrive far
/// faster than a subscriber can repaint.
pub const PROGRESS_EMIT_MIN_INTERVAL: Duration = Duration::from_millis(100);
/// An emission bypasses the window when progress crosses a multiple of this.
pub const PROGRESS_EMIT_MILESTONE_STEP: f32 = 0.05;

pub struct Session {
    spec: &'static LanguageServerSpec,
    status: ServerStatus,
    /// Open work-done tokens, with the title each began with.
    tokens: HashMap<String, String>,
    initialized_at: Option<Instant>,
    ever_ready: bool,
    /// Whether any token has begun since the server started.
    progress_seen: bool,
    last_emit: Option<Instant>,
    last_emitted: Option<f32>,
}

impl Session {
    pub fn new(spec: &'static LanguageServerSpec) -> Self {
        Self {
            spec,
            status: ServerStatus::Idle,
            tokens: HashMap::new(),
            initialized_at: None,
            ever_ready: false,
            progress_seen: false,
            last_emit: None,
            last_emitted: None,
        }
    }

    pub fn status(&self) -> &ServerStatus {
        &self.status
    }

    pub fn open_tokens(&self) -> usize {
        self.tokens.len()
    }

    fn reset(&mut self) {
        self.tokens.clear();
        self.initialized_at = None;
        self.ever_ready = false;
        self.progress_seen = false;
        self.last_emit = None;
        self.last_emitted = None;
    }

    fn set_status(&mut self, status: ServerStatus, out: &mut Vec<ServerEvent>) {
        if self.status != status {
            self.status = status.clone();
            out.push(ServerEvent::StatusChanged(status));
        }
    }

    /// A process is being started.
    pub fn starting(&mut self) -> Vec<ServerEvent> {
        self.reset();
        let mut out = Vec::new();
        self.status = ServerStatus::Starting;
        out.push(ServerEvent::StatusChanged(ServerStatus::Starting));
        out
    }

    /// Something to show while starting (locating, installing).
    pub fn starting_detail(&mut self, message: &str) -> Vec<ServerEvent> {
        vec![ServerEvent::Progress {
            progress: 0.0,
            message: message.to_string(),
        }]
    }

    /// The server answered `initialize` (and `initialized` is on the wire).
    pub fn initialized(&mut self, now: Instant) -> Vec<ServerEvent> {
        let mut out = Vec::new();
        self.initialized_at = Some(now);
        if self.spec.readiness == ReadinessRule::Immediate {
            self.become_ready(&mut out);
            return out;
        }
        // Progress that began before the reply decides the display.
        let working = ServerStatus::Indexing {
            progress: 0.0,
            message: if self.tokens.is_empty() { "Initializing..." } else { "Working..." }.to_string(),
        };
        self.set_status(working.clone(), &mut out);
        if let ServerStatus::Indexing { progress, message } = working {
            self.emit_progress(progress, message, now, true, &mut out);
        }
        // The server may already be done (all its work ended before it
        // replied).
        if self.spec.readiness == ReadinessRule::AllProgressEnded
            && self.progress_seen
            && self.tokens.is_empty()
        {
            self.become_ready(&mut out);
        }
        out
    }

    fn become_ready(&mut self, out: &mut Vec<ServerEvent>) {
        self.flush_progress(out);
        let first = !self.ever_ready;
        self.ever_ready = true;
        self.set_status(ServerStatus::Ready, out);
        if first {
            out.push(ServerEvent::Ready);
        }
    }

    /// A `$/progress` message.
    pub fn progress(
        &mut self,
        token: &str,
        kind: ProgressKind,
        title: Option<&str>,
        message: Option<&str>,
        percentage: Option<u32>,
        now: Instant,
    ) -> Vec<ServerEvent> {
        let mut out = Vec::new();
        match kind {
            ProgressKind::Begin => {
                self.progress_seen = true;
                self.tokens
                    .insert(token.to_string(), title.unwrap_or_default().to_string());
            }
            ProgressKind::Report => {
                self.tokens
                    .entry(token.to_string())
                    .or_insert_with(|| title.unwrap_or_default().to_string());
            }
            ProgressKind::End => {}
        }
        let token_title = self.tokens.get(token).cloned().unwrap_or_default();
        if kind != ProgressKind::End && self.status.is_initialized() {
            let event = ProgressEvent {
                token,
                kind,
                title: &token_title,
                message,
                percentage,
            };
            if let Some(text) = (self.spec.hooks.progress)(&event) {
                let progress = text
                    .fraction
                    .or(match &self.status {
                        ServerStatus::Indexing { progress, .. } => Some(*progress),
                        _ => None,
                    })
                    .unwrap_or(0.0);
                self.set_status(
                    ServerStatus::Indexing {
                        progress,
                        message: text.text.clone(),
                    },
                    &mut out,
                );
                self.emit_progress(progress, text.text, now, false, &mut out);
            }
        }
        if kind == ProgressKind::End {
            self.tokens.remove(token);
            if self.tokens.is_empty() && self.initialized_at.is_some() {
                let ready_by_rule = match self.spec.readiness {
                    ReadinessRule::AllProgressEnded => self.progress_seen,
                    ReadinessRule::Immediate => true,
                    ReadinessRule::Signal => false,
                };
                if self.ever_ready || ready_by_rule {
                    self.become_ready(&mut out);
                }
            }
        }
        out
    }

    /// A server-specific notification, already interpreted by the spec.
    pub fn signal(&mut self, signal: ServerSignal) -> Vec<ServerEvent> {
        let mut out = Vec::new();
        match signal {
            ServerSignal::Ready => {
                if self.initialized_at.is_some() && self.spec.readiness == ReadinessRule::Signal {
                    self.become_ready(&mut out);
                }
            }
        }
        out
    }

    /// Time passing: the hard cap on waiting for readiness.
    pub fn tick(&mut self, now: Instant) -> Vec<ServerEvent> {
        let mut out = Vec::new();
        if let Some(initialized_at) = self.initialized_at {
            if !self.ever_ready && now.duration_since(initialized_at) >= self.spec.max_indexing {
                tracing::warn!(
                    "{} did not report ready within {:?}; treating it as ready",
                    self.spec.display_name,
                    self.spec.max_indexing
                );
                self.become_ready(&mut out);
            }
        }
        out
    }

    /// The process failed to start or to initialize.
    pub fn failed(&mut self, error: String) -> Vec<ServerEvent> {
        let mut out = Vec::new();
        self.flush_progress(&mut out);
        self.initialized_at = None;
        self.tokens.clear();
        self.status = ServerStatus::Error(error.clone());
        out.push(ServerEvent::StatusChanged(ServerStatus::Error(error.clone())));
        out.push(ServerEvent::Error(error));
        out
    }

    /// The server (or what it runs on) is not installed. It is a state of its
    /// own, not a failure of a running server, so the footer can offer to
    /// install it; the error event still carries the message for the toast.
    pub fn missing(&mut self, message: String) -> Vec<ServerEvent> {
        let mut out = Vec::new();
        self.flush_progress(&mut out);
        self.initialized_at = None;
        self.tokens.clear();
        self.status = ServerStatus::Missing(message.clone());
        out.push(ServerEvent::StatusChanged(ServerStatus::Missing(message.clone())));
        out.push(ServerEvent::Error(message));
        out
    }

    /// The process ended without being asked to.
    pub fn exited(&mut self, how: &str) -> Vec<ServerEvent> {
        let error = format!("{} {how}", self.spec.display_name);
        self.failed(error)
    }

    /// The process was stopped on purpose.
    pub fn stopped(&mut self, reason: StopReason) -> Vec<ServerEvent> {
        self.reset();
        self.status = ServerStatus::Stopped;
        vec![
            ServerEvent::StatusChanged(ServerStatus::Stopped),
            ServerEvent::Stopped(reason),
        ]
    }

    fn emit_progress(
        &mut self,
        progress: f32,
        message: String,
        now: Instant,
        force: bool,
        out: &mut Vec<ServerEvent>,
    ) {
        let interval_elapsed = self
            .last_emit
            .map_or(true, |last| now.duration_since(last) >= PROGRESS_EMIT_MIN_INTERVAL);
        let milestone = |p: f32| (p.clamp(0.0, 1.0) / PROGRESS_EMIT_MILESTONE_STEP) as u32;
        let crossed = self
            .last_emitted
            .map_or(true, |last| milestone(progress) > milestone(last));
        if force || interval_elapsed || crossed {
            self.last_emit = Some(now);
            self.last_emitted = Some(progress);
            out.push(ServerEvent::Progress { progress, message });
        }
    }

    /// Emit progress that coalescing held back, so a subscriber sees the last
    /// value before a Ready or Error.
    pub fn flush_progress(&mut self, out: &mut Vec<ServerEvent>) {
        if let ServerStatus::Indexing { progress, message } = self.status.clone() {
            if self.last_emitted != Some(progress) {
                self.last_emit = Some(Instant::now());
                self.last_emitted = Some(progress);
                out.push(ServerEvent::Progress { progress, message });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_language::spec::{LocateContext, LocateError, RootStrategy, ServerCommand, ServerHooks};
    use serde_json::Value;

    fn spec_with(rule: ReadinessRule, cap: Duration) -> &'static LanguageServerSpec {
        Box::leak(Box::new(LanguageServerSpec {
            id: "fake",
            display_name: "Fake",
            find: |_: &LocateContext| -> Option<ServerCommand> { None },
            locate: |_: &LocateContext| -> Result<ServerCommand, LocateError> {
                Ok(ServerCommand::new("fake"))
            },
            roots: RootStrategy::ProjectRoot,
            initialization_options: |_| Value::Null,
            configuration: |_, _| Value::Null,
            client_capabilities: || Value::Null,
            hooks: ServerHooks::DEFAULT,
            readiness: rule,
            max_indexing: cap,
            eager: false,
            commands: &[],
            prepare_workspace: None,
            installer: None,
        }))
    }

    fn began(s: &mut Session, token: &str, now: Instant) -> Vec<ServerEvent> {
        s.progress(token, ProgressKind::Begin, Some("Indexing"), None, Some(0), now)
    }

    fn ended(s: &mut Session, token: &str, now: Instant) -> Vec<ServerEvent> {
        s.progress(token, ProgressKind::End, None, None, None, now)
    }

    fn has_ready(events: &[ServerEvent]) -> bool {
        events.contains(&ServerEvent::Ready)
    }

    #[test]
    fn immediate_servers_are_ready_when_initialized() {
        let mut s = Session::new(spec_with(ReadinessRule::Immediate, Duration::from_secs(60)));
        s.starting();
        let events = s.initialized(Instant::now());
        assert!(has_ready(&events));
        assert_eq!(s.status(), &ServerStatus::Ready);
    }

    #[test]
    fn all_progress_ended_waits_for_every_token_not_for_a_timer() {
        let mut s = Session::new(spec_with(
            ReadinessRule::AllProgressEnded,
            Duration::from_secs(600),
        ));
        let t0 = Instant::now();
        s.starting();
        s.initialized(t0);
        assert!(matches!(s.status(), ServerStatus::Indexing { .. }));
        began(&mut s, "a", t0);
        began(&mut s, "b", t0);
        // A long silence changes nothing: no timer decides readiness.
        assert!(s.tick(t0 + Duration::from_secs(300)).is_empty());
        assert!(!has_ready(&ended(&mut s, "a", t0)));
        assert!(matches!(s.status(), ServerStatus::Indexing { .. }));
        assert!(has_ready(&ended(&mut s, "b", t0)));
        assert_eq!(s.status(), &ServerStatus::Ready);
    }

    #[test]
    fn a_signal_server_ignores_progress_ending_and_waits_for_its_signal() {
        let mut s = Session::new(spec_with(ReadinessRule::Signal, Duration::from_secs(600)));
        let t0 = Instant::now();
        s.starting();
        s.initialized(t0);
        began(&mut s, "a", t0);
        assert!(!has_ready(&ended(&mut s, "a", t0)));
        assert!(matches!(s.status(), ServerStatus::Indexing { .. }));
        assert!(has_ready(&s.signal(ServerSignal::Ready)));
        assert_eq!(s.status(), &ServerStatus::Ready);
    }

    #[test]
    fn the_hard_cap_declares_a_silent_server_ready() {
        let mut s = Session::new(spec_with(ReadinessRule::Signal, Duration::from_secs(30)));
        let t0 = Instant::now();
        s.starting();
        s.initialized(t0);
        assert!(s.tick(t0 + Duration::from_secs(29)).is_empty());
        let events = s.tick(t0 + Duration::from_secs(30));
        assert!(has_ready(&events));
        assert_eq!(s.status(), &ServerStatus::Ready);
        assert!(s.tick(t0 + Duration::from_secs(90)).is_empty(), "once");
    }

    #[test]
    fn work_after_ready_shows_as_indexing_and_returns_to_ready_without_a_second_ready_event() {
        let mut s = Session::new(spec_with(ReadinessRule::Signal, Duration::from_secs(60)));
        let t0 = Instant::now();
        s.starting();
        s.initialized(t0);
        s.signal(ServerSignal::Ready);
        began(&mut s, "check", t0);
        assert!(matches!(s.status(), ServerStatus::Indexing { .. }));
        let events = ended(&mut s, "check", t0);
        assert!(!has_ready(&events));
        assert_eq!(s.status(), &ServerStatus::Ready);
    }

    #[test]
    fn progress_is_coalesced_but_the_last_value_is_flushed_before_ready() {
        let mut s = Session::new(spec_with(
            ReadinessRule::AllProgressEnded,
            Duration::from_secs(600),
        ));
        let t0 = Instant::now();
        s.starting();
        s.initialized(t0);
        began(&mut s, "a", t0);
        let mut emitted = 0;
        for pct in 1..=4u32 {
            // Within the window and below the next 5% step: suppressed.
            let events = s.progress(
                "a",
                ProgressKind::Report,
                None,
                Some("x"),
                Some(pct),
                t0 + Duration::from_millis(10),
            );
            emitted += events
                .iter()
                .filter(|e| matches!(e, ServerEvent::Progress { .. }))
                .count();
        }
        assert_eq!(emitted, 0);
        let events = ended(&mut s, "a", t0 + Duration::from_millis(20));
        let first_progress = events
            .iter()
            .position(|e| matches!(e, ServerEvent::Progress { .. }))
            .expect("the held-back progress is flushed");
        let ready = events.iter().position(|e| *e == ServerEvent::Ready).unwrap();
        assert!(first_progress < ready);
    }

    #[test]
    fn failure_and_stop_reset_the_machine() {
        let mut s = Session::new(spec_with(ReadinessRule::Signal, Duration::from_secs(60)));
        s.starting();
        s.initialized(Instant::now());
        let events = s.exited("exited with status 1");
        assert!(matches!(s.status(), ServerStatus::Error(_)));
        assert!(events.iter().any(|e| matches!(e, ServerEvent::Error(_))));
        let events = s.stopped(StopReason::Requested);
        assert_eq!(s.status(), &ServerStatus::Stopped);
        assert!(events.contains(&ServerEvent::Stopped(StopReason::Requested)));
        assert_eq!(s.open_tokens(), 0);
    }
}
