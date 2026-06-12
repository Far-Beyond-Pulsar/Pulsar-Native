use std::sync::Arc;
use std::time::Duration;

use super::{
    DevInspectEngineState, DevOpenWorkspaceRoot, DevReloadAssets, DevSaveAsDefaultLevel,
    DevShowBuildInfo, ToggleProblems,
};
use engine_fs::UserTypeRegistry;
use engine_state::EngineContext;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use ui::{
    button::Button, h_flex, v_flex, ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt,
};

// ── Component ─────────────────────────────────────────────────────────────────

pub struct DevPopover {
    focus_handle: FocusHandle,
    _refresh_task: Task<()>,
}

impl DevPopover {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let refresh_task = cx.spawn(async move |this, cx| loop {
            Timer::after(Duration::from_secs(1)).await;
            if let Some(this) = this.upgrade() {
                let _ = this.update(cx, |_, cx| cx.notify());
            } else {
                break;
            }
        });
        Self {
            focus_handle,
            _refresh_task: refresh_task,
        }
    }
}

impl EventEmitter<DismissEvent> for DevPopover {}

impl Focusable for DevPopover {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ── Layout helpers ────────────────────────────────────────────────────────────

fn card(bg: Hsla) -> Div {
    v_flex().gap_y_1().p_3().rounded_md().bg(bg)
}

fn section_header(title: &str, fg: Hsla, border: Hsla) -> Div {
    h_flex()
        .gap_2()
        .items_center()
        .pb_1()
        .child(div().text_xs().text_color(fg).child(title.to_string()))
        .child(div().flex_1().h(px(1.)).bg(border))
}

fn kv(label: &str, value: &str, label_color: Hsla, value_color: Hsla) -> Div {
    h_flex()
        .gap_2()
        .py(px(1.))
        .child(
            div()
                .text_xs()
                .text_color(label_color)
                .w(px(104.))
                .flex_shrink_0()
                .child(label.to_string()),
        )
        .child(
            div()
                .text_xs()
                .text_color(value_color)
                .flex_1()
                .overflow_hidden()
                .child(value.to_string()),
        )
}

fn status_row(
    label: &str,
    active: bool,
    on_label: &str,
    off_label: &str,
    label_color: Hsla,
    on_color: Hsla,
    off_color: Hsla,
) -> Div {
    let (dot_color, text) = if active {
        (on_color, on_label)
    } else {
        (off_color, off_label)
    };
    h_flex()
        .gap_2()
        .py(px(1.))
        .child(
            div()
                .text_xs()
                .text_color(label_color)
                .w(px(104.))
                .flex_shrink_0()
                .child(label.to_string()),
        )
        .child(
            div()
                .w(px(6.))
                .h(px(6.))
                .rounded_full()
                .bg(dot_color)
                .flex_shrink_0()
                .mt(px(1.)),
        )
        .child(
            div()
                .text_xs()
                .text_color(dot_color)
                .child(text.to_string()),
        )
}

fn peer_row(peer_id: &str, fg: Hsla, muted: Hsla) -> Div {
    h_flex()
        .gap_2()
        .py(px(1.))
        .child(
            div()
                .w(px(4.))
                .h(px(4.))
                .rounded_full()
                .bg(muted)
                .flex_shrink_0()
                .mt(px(2.)),
        )
        .child(div().text_xs().text_color(fg).child(peer_id.to_string()))
}

fn pill(label: &str, tone: Hsla, bg: Hsla) -> Div {
    div()
        .px_2()
        .py(px(2.))
        .rounded_full()
        .bg(bg)
        .child(div().text_xs().text_color(tone).child(label.to_string()))
}

// ── Render ────────────────────────────────────────────────────────────────────

impl Render for DevPopover {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        // Snapshot all theme colors up front (Hsla is Copy).
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let accent = theme.accent;
        let success = theme.success;
        let warning = theme.warning;
        let info = theme.info;
        let danger = theme.danger;
        let card_bg = theme.muted;
        let border = theme.border;
        let bg = theme.background;

        // ── Snapshot all engine state into plain owned values ──────────────────
        // All Strings / bools / usizes — no borrows escape this block.
        let engine_version: String;
        let build_info: String;
        let exe_path: String;
        let is_source_build: bool;
        let source_path: String;
        let project_path: String;
        let project_window_id: String;
        let uri_project_path: String;
        let window_count: usize;
        let renderer_count: usize;
        let has_user_types: bool;
        let verbose_launch: bool;
        let default_level_size: String;
        let has_discord: bool;
        let has_window_manager: bool;
        // Multiuser — always present, fields show "—" when no session.
        let mu_active: bool;
        let mu_status: String;
        let mu_is_connected: bool;
        let mu_is_host: bool;
        let mu_server_url: String;
        let mu_session_id: String;
        let mu_peer_id: String;
        let mu_host_peer_id: String;
        let mu_join_token: String;
        let mu_workspace_id: String;
        let mu_participants: Vec<String>;
        let window_rows: Vec<String>;
        let renderer_rows: Vec<String>;

        exe_path = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "—".to_string());

        if let Some(ctx) = EngineContext::global() {
            engine_version = env!("CARGO_PKG_VERSION").to_string();
            build_info = option_env!("BUILD_INFO").unwrap_or("dev").to_string();

            {
                let dev = ctx.store.get_or_init::<engine_state::DevContext>().get();
                is_source_build = dev.is_source_build;
                source_path = dev
                    .source_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "—".to_string());
            }

            {
                let project_handle = ctx.store.get_or_init::<Option<engine_state::ProjectContext>>();
                let (pp, pw) = project_handle
                    .read()
                    .as_ref()
                    .map(|p| {
                        (
                            p.path.display().to_string(),
                            p.window_id
                                .map(|id| id.to_string())
                                .unwrap_or_else(|| "—".to_string()),
                        )
                    })
                    .unwrap_or_else(|| ("—".to_string(), "—".to_string()));
                project_path = pp;
                project_window_id = pw;
            }

            {
                let launch = ctx.store.get_or_init::<engine_state::LaunchContext>().get();
                uri_project_path = launch
                    .uri_project_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "—".to_string());
                verbose_launch = launch.verbose;
            }

            default_level_size = ctx
                .store
                .get_or_init::<Option<Vec<u8>>>()
                .read()
                .as_ref()
                .map(|b| {
                    let n = b.len();
                    if n >= 1_048_576 {
                        format!("{:.1} MB", n as f64 / 1_048_576.0)
                    } else if n >= 1_024 {
                        format!("{:.1} KB", n as f64 / 1_024.0)
                    } else {
                        format!("{} B", n)
                    }
                })
                .unwrap_or_else(|| "—".to_string());

            has_discord = ctx.store.get_or_init::<Option<engine_state::DiscordPresence>>().read().is_some();
            has_window_manager = ctx.store.get_or_init::<Option<window_manager::WindowManager>>().read().is_some();
            has_user_types = ctx.store.get_or_init::<Option<Arc<engine_fs::UserTypeRegistry>>>().read().is_some();

            {
                let mu_guard = ctx.multiuser.read();
                if let Some(mu) = mu_guard.as_ref() {
                    mu_active = true;
                    mu_is_connected = mu.is_connected();
                    mu_status = match &mu.status {
                        engine_state::MultiuserStatus::Connected { relay_mode } => {
                            let mode_str = relay_mode
                                .as_ref()
                                .map(|m| match m {
                                    engine_state::RelayConnectionMode::DirectP2P => " (P2P)",
                                    engine_state::RelayConnectionMode::BinaryProxy => " (Proxy)",
                                    engine_state::RelayConnectionMode::JsonFallback => " (JSON)",
                                })
                                .unwrap_or("");
                            format!("Connected{}", mode_str)
                        }
                        engine_state::MultiuserStatus::DegradedMode { relay_mode } => {
                            let mode_str = match relay_mode {
                                engine_state::RelayConnectionMode::BinaryProxy => "Proxy",
                                engine_state::RelayConnectionMode::JsonFallback => "JSON",
                                engine_state::RelayConnectionMode::DirectP2P => "P2P",
                            };
                            format!("Degraded ({})", mode_str)
                        }
                        engine_state::MultiuserStatus::Connecting => "Connecting…".to_string(),
                        engine_state::MultiuserStatus::Disconnected => "Disconnected".to_string(),
                        engine_state::MultiuserStatus::Error(e) => format!("Error: {}", e),
                    };
                    mu_is_host = mu.is_host;
                    mu_server_url = mu.server_url.clone();
                    mu_session_id = mu.session_id.clone();
                    mu_peer_id = mu.peer_id.clone();
                    mu_host_peer_id = mu.host_peer_id.clone();
                    mu_join_token = mu.join_token.clone().unwrap_or_else(|| "—".to_string());
                    mu_workspace_id = mu.workspace_id.clone().unwrap_or_else(|| "—".to_string());
                    mu_participants = mu.participants.clone();
                } else {
                    mu_active = false;
                    mu_is_connected = false;
                    mu_is_host = false;
                    mu_status = "Not enabled".to_string();
                    mu_server_url = "—".to_string();
                    mu_session_id = "—".to_string();
                    mu_peer_id = "—".to_string();
                    mu_host_peer_id = "—".to_string();
                    mu_join_token = "—".to_string();
                    mu_workspace_id = "—".to_string();
                    mu_participants = Vec::new();
                }
            }

            window_count = ctx.windows.len();
            renderer_count = ctx.renderers.window_ids().len();

            let mut wr: Vec<String> = ctx
                .windows
                .iter()
                .map(|e| format!("#{:<4} {:?}", e.key(), e.value().window_type))
                .collect();
            wr.sort();
            window_rows = wr;

            let mut rr: Vec<String> = ctx
                .renderers
                .window_ids()
                .into_iter()
                .map(|id| format!("#{}", id))
                .collect();
            rr.sort();
            renderer_rows = rr;
        } else {
            engine_version = "?".to_string();
            build_info = "?".to_string();
            is_source_build = false;
            source_path = "—".to_string();
            project_path = "—".to_string();
            project_window_id = "—".to_string();
            uri_project_path = "—".to_string();
            verbose_launch = false;
            default_level_size = "—".to_string();
            has_discord = false;
            has_window_manager = false;
            has_user_types = false;
            window_count = 0;
            renderer_count = 0;
            mu_active = false;
            mu_is_connected = false;
            mu_is_host = false;
            mu_status = "No engine context".to_string();
            mu_server_url = "—".to_string();
            mu_session_id = "—".to_string();
            mu_peer_id = "—".to_string();
            mu_host_peer_id = "—".to_string();
            mu_join_token = "—".to_string();
            mu_workspace_id = "—".to_string();
            mu_participants = Vec::new();
            window_rows = Vec::new();
            renderer_rows = Vec::new();
        }

        // Determine multiuser status dot color.
        let mu_color = if !mu_active {
            muted
        } else if mu_is_connected {
            success
        } else if mu_status.starts_with("Error") {
            danger
        } else if mu_status.starts_with("Connecting") {
            warning
        } else {
            muted
        };

        // ── Build UI ───────────────────────────────────────────────────────────
        v_flex()
            .w(px(520.))
            .p_4()
            .gap_3()
            .bg(bg)
            .rounded_lg()
            .shadow_xl()
            .track_focus(&self.focus_handle)
            // Header
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(Icon::new(IconName::Bug).text_color(accent))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(fg)
                                    .font_semibold()
                                    .child("Dev Menu"),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(div().w(px(6.)).h(px(6.)).rounded_full().bg(success))
                            .child(div().text_xs().text_color(muted).child("live · 1s")),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(pill(&format!("v{}", engine_version), fg, card_bg))
                    .child(pill(&build_info, muted, card_bg))
                    .child(pill(
                        if is_source_build { "source" } else { "release" },
                        if is_source_build { success } else { muted },
                        card_bg,
                    )),
            )
            // Quick actions first.
            .child(
                card(card_bg)
                    .child(section_header("QUICK ACTIONS", fg, border))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("reload-assets")
                                    .label("Reload Assets")
                                    .icon(IconName::Activity)
                                    .small()
                                    .on_click(|_, _, cx| cx.dispatch_action(&DevReloadAssets)),
                            )
                            .child(
                                Button::new("show-build-info")
                                    .label("Build Info")
                                    .icon(IconName::Info)
                                    .small()
                                    .on_click(|_, _, cx| cx.dispatch_action(&DevShowBuildInfo)),
                            )
                            .child(
                                Button::new("open-problems")
                                    .label("Problems")
                                    .icon(IconName::TriangleAlert)
                                    .small()
                                    .on_click(|_, _, cx| cx.dispatch_action(&ToggleProblems)),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("open-workspace-root")
                                    .label("Workspace")
                                    .icon(IconName::Folder)
                                    .small()
                                    .on_click(|_, _, cx| cx.dispatch_action(&DevOpenWorkspaceRoot)),
                            )
                            .child(
                                Button::new("inspect-engine-state")
                                    .label("Inspect State")
                                    .icon(IconName::Code)
                                    .small()
                                    .on_click(|_, _, cx| {
                                        cx.dispatch_action(&DevInspectEngineState)
                                    }),
                            )
                            .child(
                                Button::new("save-default-level")
                                    .label("Save Level")
                                    .icon(IconName::Database)
                                    .small()
                                    .on_click(|_, _, cx| {
                                        cx.dispatch_action(&DevSaveAsDefaultLevel)
                                    }),
                            ),
                    ),
            )
            // Top-level status summary.
            .child(
                card(card_bg)
                    .child(section_header("STATUS", fg, border))
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(pill(&format!("Windows {}", window_count), fg, bg))
                            .child(pill(&format!("Renderers {}", renderer_count), fg, bg))
                            .child(pill(
                                if has_user_types {
                                    "User types loaded"
                                } else {
                                    "User types missing"
                                },
                                if has_user_types { success } else { warning },
                                bg,
                            ))
                            .child(pill(
                                if verbose_launch {
                                    "Verbose on"
                                } else {
                                    "Verbose off"
                                },
                                if verbose_launch { warning } else { muted },
                                bg,
                            )),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(pill(
                                if has_discord {
                                    "Discord active"
                                } else {
                                    "Discord none"
                                },
                                if has_discord { success } else { muted },
                                bg,
                            ))
                            .child(pill(
                                if has_window_manager {
                                    "Window mgr active"
                                } else {
                                    "Window mgr none"
                                },
                                if has_window_manager { success } else { muted },
                                bg,
                            ))
                            .child(pill(
                                if mu_active {
                                    "Multiuser enabled"
                                } else {
                                    "Multiuser disabled"
                                },
                                if mu_active { info } else { muted },
                                bg,
                            )),
                    )
                    .child(kv("Default Level", &default_level_size, muted, fg)),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        card(card_bg)
                            .flex_1()
                            .child(section_header("ENGINE", fg, border))
                            .child(kv("Executable", &exe_path, muted, fg))
                            .child(status_row(
                                "Source Build",
                                is_source_build,
                                "Yes",
                                "No",
                                muted,
                                success,
                                muted,
                            ))
                            .child(kv("Source Root", &source_path, muted, fg)),
                    )
                    .child(
                        card(card_bg)
                            .flex_1()
                            .child(section_header("PROJECT", fg, border))
                            .child(kv("Path", &project_path, muted, fg))
                            .child(kv("Window ID", &project_window_id, muted, fg))
                            .child(kv("URI Path", &uri_project_path, muted, fg)),
                    ),
            )
            // Multiuser details stay visible for diagnostics.
            .child(
                card(card_bg)
                    .child(section_header("MULTIUSER SESSION", fg, border))
                    .child(
                        h_flex()
                            .gap_2()
                            .py(px(1.))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .w(px(104.))
                                    .flex_shrink_0()
                                    .child("Status"),
                            )
                            .child(
                                div()
                                    .w(px(6.))
                                    .h(px(6.))
                                    .rounded_full()
                                    .bg(mu_color)
                                    .flex_shrink_0()
                                    .mt(px(1.)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(mu_color)
                                    .child(mu_status.clone()),
                            ),
                    )
                    .child(status_row(
                        "Role",
                        mu_is_host,
                        "Host",
                        "Participant",
                        muted,
                        accent,
                        fg,
                    ))
                    .child(kv(
                        "Server URL",
                        &mu_server_url,
                        muted,
                        if mu_active { fg } else { muted },
                    ))
                    .child(kv(
                        "Session ID",
                        &mu_session_id,
                        muted,
                        if mu_active { fg } else { muted },
                    ))
                    .child(kv(
                        "Our Peer ID",
                        &mu_peer_id,
                        muted,
                        if mu_active { fg } else { muted },
                    ))
                    .child(kv(
                        "Host Peer",
                        &mu_host_peer_id,
                        muted,
                        if mu_active { fg } else { muted },
                    ))
                    .child(kv(
                        "Join Token",
                        &mu_join_token,
                        muted,
                        if mu_join_token != "—" { fg } else { muted },
                    ))
                    .child(kv(
                        "Workspace ID",
                        &mu_workspace_id,
                        muted,
                        if mu_workspace_id != "—" { fg } else { muted },
                    ))
                    .when(!mu_participants.is_empty(), |el| {
                        let peers = mu_participants.clone();
                        let overflow = peers.len().saturating_sub(8);
                        el.child(
                            v_flex()
                                .pt_1()
                                .gap_y_0()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .pb_1()
                                        .child(format!("Participants ({})", peers.len())),
                                )
                                .children(
                                    peers.into_iter().take(8).map(|p| peer_row(&p, fg, muted)),
                                )
                                .when(overflow > 0, |el| {
                                    el.child(
                                        div()
                                            .text_xs()
                                            .text_color(muted)
                                            .child(format!("+ {} more", overflow)),
                                    )
                                }),
                        )
                    })
                    .when(mu_participants.is_empty() && mu_active, |el| {
                        el.child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .py(px(1.))
                                .child("No participants yet"),
                        )
                    }),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        card(card_bg)
                            .flex_1()
                            .child(section_header(
                                &format!("WINDOWS ({})", window_count),
                                fg,
                                border,
                            ))
                            .when(window_rows.is_empty(), |el| {
                                el.child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .child("No windows registered"),
                                )
                            })
                            .children(window_rows.iter().take(8).map(|r| {
                                div().text_xs().text_color(fg).py(px(1.)).child(r.clone())
                            }))
                            .when(window_rows.len() > 8, |el| {
                                el.child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(format!("+ {} more", window_rows.len() - 8)),
                                )
                            }),
                    )
                    .child(
                        card(card_bg)
                            .flex_1()
                            .child(section_header(
                                &format!("RENDERERS ({})", renderer_count),
                                fg,
                                border,
                            ))
                            .when(renderer_rows.is_empty(), |el| {
                                el.child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .child("No renderers registered"),
                                )
                            })
                            .children(renderer_rows.iter().take(8).map(|r| {
                                div().text_xs().text_color(fg).py(px(1.)).child(r.clone())
                            }))
                            .when(renderer_rows.len() > 8, |el| {
                                el.child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(format!("+ {} more", renderer_rows.len() - 8)),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("Pulsar Engine · toolbar dev menu · auto-refreshes every 1s"),
            )
    }
}
