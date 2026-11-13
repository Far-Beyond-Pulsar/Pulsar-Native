//! Owned snapshot of the engine state shown by the developer popover.
//!
//! Owns reading `EngineContext` into plain values (`DevSnapshot`,
//! `MultiuserSnapshot`) so the popover's render code never touches engine
//! locks, plus the pure formatting helpers behind them (`format_byte_size`,
//! `multiuser_status_text`, `MultiuserSnapshot::dot`). Entry point:
//! [`DevSnapshot::capture`].

use std::sync::Arc;

use engine_state::EngineContext;

/// Placeholder shown when a value is unavailable.
const NONE: &str = "—";

/// Status-dot color category for the multiuser row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StatusDot {
    Muted,
    Success,
    Warning,
    Danger,
}

/// Multiuser session values for the popover.
pub(super) struct MultiuserSnapshot {
    pub active: bool,
    pub status: String,
    pub is_connected: bool,
    pub is_host: bool,
    pub server_url: String,
    pub session_id: String,
    pub peer_id: String,
    pub join_token: String,
    pub participants_count: usize,
}

impl MultiuserSnapshot {
    /// Snapshot used when multiuser is not enabled (or there is no engine context).
    fn inactive(status: &str) -> Self {
        Self {
            active: false,
            status: status.into(),
            is_connected: false,
            is_host: false,
            server_url: NONE.into(),
            session_id: NONE.into(),
            peer_id: NONE.into(),
            join_token: NONE.into(),
            participants_count: 0,
        }
    }

    /// Reads the multiuser context; inactive when multiuser is not enabled.
    fn capture(ctx: &EngineContext) -> Self {
        let mu = ctx.multiuser.read();
        let Some(mu) = mu.as_ref() else {
            return Self::inactive("Not enabled");
        };
        Self {
            active: true,
            status: multiuser_status_text(&mu.status),
            is_connected: mu.is_connected(),
            is_host: mu.is_host,
            server_url: mu.server_url.clone(),
            session_id: mu.session_id.clone(),
            peer_id: mu.peer_id.clone(),
            join_token: mu.join_token.clone().unwrap_or_else(|| NONE.into()),
            participants_count: mu.participants.len(),
        }
    }

    /// Dot color: muted when inactive, green when connected, red on error, amber while connecting.
    pub fn dot(&self) -> StatusDot {
        if !self.active {
            StatusDot::Muted
        } else if self.is_connected {
            StatusDot::Success
        } else if self.status.starts_with("Error") {
            StatusDot::Danger
        } else if self.status.starts_with("Connecting") {
            StatusDot::Warning
        } else {
            StatusDot::Muted
        }
    }

    /// True when the join token is a real value (not empty or the placeholder).
    pub fn has_join_token(&self) -> bool {
        !self.join_token.is_empty() && self.join_token != NONE
    }
}

/// Everything the developer popover displays, copied out of the engine context.
pub(super) struct DevSnapshot {
    pub engine_version: String,
    pub build_info: String,
    pub exe_path: String,
    pub is_source_build: bool,
    pub source_path: String,
    pub project_path: String,
    pub project_window_id: String,
    pub uri_project_path: String,
    pub window_count: usize,
    pub renderer_count: usize,
    pub verbose_launch: bool,
    pub default_level_size: String,
    pub has_discord: bool,
    pub has_window_manager: bool,
    pub multiuser: MultiuserSnapshot,
    pub window_rows: Vec<String>,
    pub renderer_rows: Vec<String>,
}

impl DevSnapshot {
    /// Reads the current engine state, falling back to placeholders without an engine context.
    pub fn capture() -> Self {
        let exe_path = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| NONE.into());

        match EngineContext::global() {
            Some(ctx) => Self::from_engine(&ctx, exe_path),
            None => Self::without_engine(exe_path),
        }
    }

    /// Snapshot shown when no engine context exists.
    fn without_engine(exe_path: String) -> Self {
        Self {
            engine_version: "?".into(),
            build_info: "?".into(),
            exe_path,
            is_source_build: false,
            source_path: NONE.into(),
            project_path: NONE.into(),
            project_window_id: NONE.into(),
            uri_project_path: NONE.into(),
            window_count: 0,
            renderer_count: 0,
            verbose_launch: false,
            default_level_size: NONE.into(),
            has_discord: false,
            has_window_manager: false,
            multiuser: MultiuserSnapshot::inactive("No engine context"),
            window_rows: Vec::new(),
            renderer_rows: Vec::new(),
        }
    }

    /// Reads build, project, launch, subsystem, multiuser, window and renderer state.
    fn from_engine(ctx: &EngineContext, exe_path: String) -> Self {
        let (is_source_build, source_path) = {
            let dev = ctx.store.get_or_init::<engine_state::DevContext>().get();
            (
                dev.is_source_build,
                dev.source_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| NONE.into()),
            )
        };

        let (project_path, project_window_id) = {
            let ph = ctx
                .store
                .get_or_init::<Option<engine_state::ProjectContext>>();
            let project = ph
                .read()
                .as_ref()
                .map(|p| {
                    (
                        p.path.display().to_string(),
                        p.window_id
                            .map(|id| id.to_string())
                            .unwrap_or_else(|| NONE.into()),
                    )
                })
                .unwrap_or_else(|| (NONE.into(), NONE.into()));
            project
        };

        let (uri_project_path, verbose_launch) = {
            let lc = ctx.store.get_or_init::<engine_state::LaunchContext>().get();
            (
                lc.uri_project_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| NONE.into()),
                lc.verbose,
            )
        };

        let default_level_size = ctx
            .store
            .get_or_init::<engine_state::NewProjectContent>()
            .read()
            .level
            .as_ref()
            .map(|b| format_byte_size(b.len()))
            .unwrap_or_else(|| NONE.into());

        let has_discord = ctx
            .store
            .get_or_init::<Option<engine_state::DiscordPresence>>()
            .read()
            .is_some();
        let has_window_manager = ctx
            .store
            .get_or_init::<Option<window_manager::WindowManager>>()
            .read()
            .is_some();

        let mut window_rows: Vec<String> = ctx
            .windows
            .iter()
            .map(|e| format!("Win #{} · {:?}", e.key(), e.value().window_type))
            .collect();
        window_rows.sort();

        // The engine's render runtimes, as the Tasks system lists them.
        let mut renderer_rows: Vec<String> = editor_task_queue::global()
            .snapshots()
            .into_iter()
            .filter(|task| {
                task.category == editor_task_queue::categories::RENDER_RUNTIME
                    && !task.status.is_finished()
            })
            .map(|task| task.title)
            .collect();
        renderer_rows.sort();

        Self {
            engine_version: env!("CARGO_PKG_VERSION").to_string(),
            build_info: option_env!("BUILD_INFO").unwrap_or("dev").to_string(),
            exe_path,
            is_source_build,
            source_path,
            project_path,
            project_window_id,
            uri_project_path,
            window_count: ctx.windows.len(),
            renderer_count: renderer_rows.len(),
            verbose_launch,
            default_level_size,
            has_discord,
            has_window_manager,
            multiuser: MultiuserSnapshot::capture(ctx),
            window_rows,
            renderer_rows,
        }
    }
}

/// Human-readable byte count: `B`, `KB` or `MB` with one decimal for the larger units.
pub(super) fn format_byte_size(n: usize) -> String {
    if n >= 1_048_576 {
        format!("{:.1} MB", n as f64 / 1_048_576.0)
    } else if n >= 1_024 {
        format!("{:.1} KB", n as f64 / 1_024.0)
    } else {
        format!("{} B", n)
    }
}

/// Display text for a multiuser connection status.
pub(super) fn multiuser_status_text(status: &engine_state::MultiuserStatus) -> String {
    match status {
        engine_state::MultiuserStatus::Connected { relay_mode } => relay_mode
            .as_ref()
            .map(|m| match m {
                engine_state::RelayConnectionMode::DirectP2P => "Connected · P2P",
                engine_state::RelayConnectionMode::BinaryProxy => "Connected · Proxy",
                engine_state::RelayConnectionMode::JsonFallback => "Connected · JSON",
            })
            .unwrap_or("Connected")
            .to_string(),
        engine_state::MultiuserStatus::DegradedMode { .. } => "Degraded".into(),
        engine_state::MultiuserStatus::Connecting => "Connecting…".into(),
        engine_state::MultiuserStatus::Disconnected => "Disconnected".into(),
        engine_state::MultiuserStatus::Error(e) => format!("Error: {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_state::{MultiuserStatus, RelayConnectionMode};

    #[test]
    fn byte_sizes_pick_the_largest_unit() {
        assert_eq!(format_byte_size(0), "0 B");
        assert_eq!(format_byte_size(1_023), "1023 B");
        assert_eq!(format_byte_size(1_024), "1.0 KB");
        assert_eq!(format_byte_size(1_536), "1.5 KB");
        assert_eq!(format_byte_size(1_048_576), "1.0 MB");
        assert_eq!(format_byte_size(5 * 1_048_576 + 524_288), "5.5 MB");
    }

    #[test]
    fn status_text_per_variant() {
        assert_eq!(
            multiuser_status_text(&MultiuserStatus::Disconnected),
            "Disconnected"
        );
        assert_eq!(
            multiuser_status_text(&MultiuserStatus::Connecting),
            "Connecting…"
        );
        assert_eq!(
            multiuser_status_text(&MultiuserStatus::Error("boom".into())),
            "Error: boom"
        );
        assert_eq!(
            multiuser_status_text(&MultiuserStatus::Connected { relay_mode: None }),
            "Connected"
        );
        assert_eq!(
            multiuser_status_text(&MultiuserStatus::Connected {
                relay_mode: Some(RelayConnectionMode::DirectP2P)
            }),
            "Connected · P2P"
        );
        assert_eq!(
            multiuser_status_text(&MultiuserStatus::DegradedMode {
                relay_mode: RelayConnectionMode::JsonFallback
            }),
            "Degraded"
        );
    }

    fn active(status: &str, connected: bool) -> MultiuserSnapshot {
        let mut snap = MultiuserSnapshot::inactive(status);
        snap.active = true;
        snap.is_connected = connected;
        snap
    }

    #[test]
    fn dot_reflects_connection_state() {
        assert_eq!(
            MultiuserSnapshot::inactive("Not enabled").dot(),
            StatusDot::Muted
        );
        assert_eq!(active("Connected", true).dot(), StatusDot::Success);
        assert_eq!(active("Error: x", false).dot(), StatusDot::Danger);
        assert_eq!(active("Connecting…", false).dot(), StatusDot::Warning);
        assert_eq!(active("Disconnected", false).dot(), StatusDot::Muted);
    }

    #[test]
    fn join_token_placeholder_is_not_a_token() {
        let mut snap = MultiuserSnapshot::inactive("x");
        assert!(!snap.has_join_token());
        snap.join_token = String::new();
        assert!(!snap.has_join_token());
        snap.join_token = "abc".into();
        assert!(snap.has_join_token());
    }
}
