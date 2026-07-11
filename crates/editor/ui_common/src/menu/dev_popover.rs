use std::sync::Arc;
use std::time::Duration;

use super::{
    DevInspectEngineState, DevOpenWorkspaceRoot, DevReloadAssets, DevSaveAsDefaultLevel,
    DevShowBuildInfo, ToggleProblems,
};
use engine_state::EngineContext;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, Icon, IconName, Sizable as _,
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

// ── Layout primitives ─────────────────────────────────────────────────────────

/// Full-width section separator with ALL-CAPS label.
fn section_header(label: &str, fg: Hsla, border: Hsla) -> impl IntoElement {
    v_flex()
        .w_full()
        .gap_0()
        .pt(px(2.))
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(fg.opacity(0.45))
                .child(label.to_uppercase()),
        )
        .child(div().w_full().h(px(1.)).mt(px(4.)).bg(border))
}

/// Compact key → value row.
fn kv_row(
    label: &str,
    value: impl Into<SharedString>,
    label_c: Hsla,
    value_c: Hsla,
) -> impl IntoElement {
    let value: SharedString = value.into();
    h_flex()
        .w_full()
        .justify_between()
        .items_center()
        .py(px(2.))
        .child(
            div()
                .text_xs()
                .text_color(label_c)
                .flex_shrink_0()
                .w(px(110.))
                .child(label.to_string()),
        )
        .child(
            div()
                .text_xs()
                .text_color(value_c)
                .flex_1()
                .overflow_hidden()
                .child(value.to_string()),
        )
}

/// Status dot + label row.
fn status_row(
    label: &str,
    active: bool,
    on_label: &str,
    off_label: &str,
    label_c: Hsla,
    on_c: Hsla,
    off_c: Hsla,
) -> impl IntoElement {
    let (dot_c, text) = if active {
        (on_c, on_label)
    } else {
        (off_c, off_label)
    };
    h_flex()
        .w_full()
        .justify_between()
        .items_center()
        .py(px(2.))
        .child(
            div()
                .text_xs()
                .text_color(label_c)
                .w(px(110.))
                .child(label.to_string()),
        )
        .child(
            h_flex()
                .gap_1()
                .items_center()
                .child(
                    div()
                        .w(px(5.))
                        .h(px(5.))
                        .rounded_full()
                        .bg(dot_c)
                        .flex_shrink_0(),
                )
                .child(div().text_xs().text_color(dot_c).child(text.to_string())),
        )
}

/// Inline pill badge.
fn pill(label: impl Into<String>, text_c: Hsla, bg_c: Hsla) -> impl IntoElement {
    div()
        .px(px(6.))
        .py(px(2.))
        .rounded_full()
        .bg(bg_c)
        .text_xs()
        .text_color(text_c)
        .child(label.into())
}

// ── Render ────────────────────────────────────────────────────────────────────

impl Render for DevPopover {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let bg = theme.background;
        let border = theme.border;
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let accent = theme.accent;
        let success = theme.success;
        let warning = theme.warning;
        let danger = theme.danger;
        let info = theme.info;
        drop(theme);

        // ── Snapshot engine state into plain owned values ─────────────────────
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
        let mu_active: bool;
        let mu_status: String;
        let mu_is_connected: bool;
        let mu_is_host: bool;
        let mu_server_url: String;
        let mu_session_id: String;
        let mu_peer_id: String;
        let mu_join_token: String;
        let mu_participants_count: usize;
        let window_rows: Vec<String>;
        let renderer_rows: Vec<String>;

        exe_path = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "—".into());

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
                    .unwrap_or_else(|| "—".into());
            }

            {
                let ph = ctx
                    .store
                    .get_or_init::<Option<engine_state::ProjectContext>>();
                let (pp, pw) = ph
                    .read()
                    .as_ref()
                    .map(|p| {
                        (
                            p.path.display().to_string(),
                            p.window_id
                                .map(|id| id.to_string())
                                .unwrap_or_else(|| "—".into()),
                        )
                    })
                    .unwrap_or_else(|| ("—".into(), "—".into()));
                project_path = pp;
                project_window_id = pw;
            }

            {
                let lc = ctx.store.get_or_init::<engine_state::LaunchContext>().get();
                uri_project_path = lc
                    .uri_project_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "—".into());
                verbose_launch = lc.verbose;
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
                .unwrap_or_else(|| "—".into());

            has_discord = ctx
                .store
                .get_or_init::<Option<engine_state::DiscordPresence>>()
                .read()
                .is_some();
            has_window_manager = ctx
                .store
                .get_or_init::<Option<window_manager::WindowManager>>()
                .read()
                .is_some();
            has_user_types = ctx
                .store
                .get_or_init::<Option<Arc<engine_fs::UserTypeRegistry>>>()
                .read()
                .is_some();

            {
                let mu = ctx.multiuser.read();
                if let Some(mu) = mu.as_ref() {
                    mu_active = true;
                    mu_is_connected = mu.is_connected();
                    mu_is_host = mu.is_host;
                    mu_server_url = mu.server_url.clone();
                    mu_session_id = mu.session_id.clone();
                    mu_peer_id = mu.peer_id.clone();
                    mu_join_token = mu.join_token.clone().unwrap_or_else(|| "—".into());
                    mu_participants_count = mu.participants.len();
                    mu_status = match &mu.status {
                        engine_state::MultiuserStatus::Connected { relay_mode } => relay_mode
                            .as_ref()
                            .map(|m| match m {
                                engine_state::RelayConnectionMode::DirectP2P => "Connected · P2P",
                                engine_state::RelayConnectionMode::BinaryProxy => {
                                    "Connected · Proxy"
                                }
                                engine_state::RelayConnectionMode::JsonFallback => {
                                    "Connected · JSON"
                                }
                            })
                            .unwrap_or("Connected")
                            .to_string(),
                        engine_state::MultiuserStatus::DegradedMode { .. } => "Degraded".into(),
                        engine_state::MultiuserStatus::Connecting => "Connecting…".into(),
                        engine_state::MultiuserStatus::Disconnected => "Disconnected".into(),
                        engine_state::MultiuserStatus::Error(e) => format!("Error: {}", e),
                    };
                } else {
                    mu_active = false;
                    mu_is_connected = false;
                    mu_is_host = false;
                    mu_status = "Not enabled".into();
                    mu_server_url = "—".into();
                    mu_session_id = "—".into();
                    mu_peer_id = "—".into();
                    mu_join_token = "—".into();
                    mu_participants_count = 0;
                }
            }

            window_count = ctx.windows.len();
            renderer_count = ctx.renderers.window_ids().len();

            let mut wr: Vec<String> = ctx
                .windows
                .iter()
                .map(|e| format!("Win #{} · {:?}", e.key(), e.value().window_type))
                .collect();
            wr.sort();
            window_rows = wr;

            let mut rr: Vec<String> = ctx
                .renderers
                .window_ids()
                .into_iter()
                .map(|id| format!("Renderer #{}", id))
                .collect();
            rr.sort();
            renderer_rows = rr;
        } else {
            engine_version = "?".into();
            build_info = "?".into();
            is_source_build = false;
            source_path = "—".into();
            project_path = "—".into();
            project_window_id = "—".into();
            uri_project_path = "—".into();
            verbose_launch = false;
            default_level_size = "—".into();
            has_discord = false;
            has_window_manager = false;
            has_user_types = false;
            window_count = 0;
            renderer_count = 0;
            mu_active = false;
            mu_is_connected = false;
            mu_is_host = false;
            mu_status = "No engine context".into();
            mu_server_url = "—".into();
            mu_session_id = "—".into();
            mu_peer_id = "—".into();
            mu_join_token = "—".into();
            mu_participants_count = 0;
            window_rows = Vec::new();
            renderer_rows = Vec::new();
        }

        let mu_dot = if !mu_active {
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

        // ── Build UI ──────────────────────────────────────────────────────────
        v_flex()
            .id("dev-popover-root")
            .w(px(420.))
            .max_h(px(640.))
            .overflow_y_scroll()
            .bg(bg)
            .rounded_xl()
            .shadow_xl()
            .border_1()
            .border_color(border)
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            // ── Header ───────────────────────────────────────────────────────
            .child(
                h_flex()
                    .px_4()
                    .py_3()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(Icon::new(IconName::Bug).size(px(15.)).text_color(accent))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(fg)
                                    .child("Developer"),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(pill(
                                format!("v{}", engine_version),
                                muted,
                                border.opacity(0.5),
                            ))
                            .child(pill(
                                if is_source_build {
                                    "source"
                                } else {
                                    &build_info
                                },
                                if is_source_build { success } else { muted },
                                border.opacity(0.5),
                            ))
                            // live dot
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(div().w(px(5.)).h(px(5.)).rounded_full().bg(success))
                                    .child(div().text_xs().text_color(muted).child("live")),
                            ),
                    ),
            )
            // ── Quick actions ─────────────────────────────────────────────────
            .child(
                v_flex()
                    .px_4()
                    .pt_3()
                    .pb_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(border)
                    .child(section_header("Quick Actions", fg, border))
                    .child(
                        h_flex()
                            .gap_1()
                            .flex_wrap()
                            .child(
                                Button::new("reload-assets")
                                    .label("Reload Assets")
                                    .icon(IconName::Activity)
                                    .small()
                                    .ghost()
                                    .on_click(|_, _, cx| cx.dispatch_action(&DevReloadAssets)),
                            )
                            .child(
                                Button::new("show-build-info")
                                    .label("Build Info")
                                    .icon(IconName::Info)
                                    .small()
                                    .ghost()
                                    .on_click(|_, _, cx| cx.dispatch_action(&DevShowBuildInfo)),
                            )
                            .child(
                                Button::new("open-problems")
                                    .label("Problems")
                                    .icon(IconName::TriangleAlert)
                                    .small()
                                    .ghost()
                                    .on_click(|_, _, cx| cx.dispatch_action(&ToggleProblems)),
                            )
                            .child(
                                Button::new("open-workspace-root")
                                    .label("Workspace")
                                    .icon(IconName::Folder)
                                    .small()
                                    .ghost()
                                    .on_click(|_, _, cx| cx.dispatch_action(&DevOpenWorkspaceRoot)),
                            )
                            .child(
                                Button::new("inspect-state")
                                    .label("Inspect State")
                                    .icon(IconName::Code)
                                    .small()
                                    .ghost()
                                    .on_click(|_, _, cx| {
                                        cx.dispatch_action(&DevInspectEngineState)
                                    }),
                            )
                            .child(
                                Button::new("save-level")
                                    .label("Save Level")
                                    .icon(IconName::Database)
                                    .small()
                                    .ghost()
                                    .on_click(|_, _, cx| {
                                        cx.dispatch_action(&DevSaveAsDefaultLevel)
                                    }),
                            ),
                    ),
            )
            // ── Status ────────────────────────────────────────────────────────
            .child(
                v_flex()
                    .px_4()
                    .pt_3()
                    .pb_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(border)
                    .child(section_header("Status", fg, border))
                    .child(
                        h_flex()
                            .gap_1()
                            .pt_1()
                            .flex_wrap()
                            .child(pill(
                                format!("{} windows", window_count),
                                fg,
                                border.opacity(0.4),
                            ))
                            .child(pill(
                                format!("{} renderers", renderer_count),
                                fg,
                                border.opacity(0.4),
                            ))
                            .child(pill(
                                if has_user_types {
                                    "Types ✓"
                                } else {
                                    "Types ✗"
                                },
                                if has_user_types { success } else { warning },
                                border.opacity(0.4),
                            ))
                            .child(pill(
                                if has_discord { "Discord" } else { "No Discord" },
                                if has_discord { info } else { muted },
                                border.opacity(0.4),
                            ))
                            .child(pill(
                                if has_window_manager {
                                    "WinMgr"
                                } else {
                                    "No WinMgr"
                                },
                                if has_window_manager { info } else { muted },
                                border.opacity(0.4),
                            ))
                            .child(pill(
                                if verbose_launch { "Verbose" } else { "Silent" },
                                if verbose_launch { warning } else { muted },
                                border.opacity(0.4),
                            )),
                    )
                    .child(kv_row("Default level", default_level_size, muted, fg)),
            )
            // ── Engine + Project ──────────────────────────────────────────────
            .child(
                h_flex()
                    .px_4()
                    .pt_3()
                    .pb_2()
                    .gap_4()
                    .items_start()
                    .border_b_1()
                    .border_color(border)
                    // Engine column
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_1()
                            .child(section_header("Engine", fg, border))
                            .child(status_row(
                                "Source build",
                                is_source_build,
                                "Yes",
                                "No",
                                muted,
                                success,
                                muted,
                            ))
                            .child(kv_row("Source root", source_path, muted, fg))
                            .child(kv_row("Executable", exe_path, muted, fg)),
                    )
                    // Project column
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_1()
                            .child(section_header("Project", fg, border))
                            .child(kv_row("Path", project_path, muted, fg))
                            .child(kv_row("Window ID", project_window_id, muted, fg))
                            .child(kv_row("URI path", uri_project_path, muted, fg)),
                    ),
            )
            // ── Multiuser ─────────────────────────────────────────────────────
            .child(
                v_flex()
                    .px_4()
                    .pt_3()
                    .pb_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(border)
                    .child(section_header("Multiuser Session", fg, border))
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .items_center()
                            .py(px(2.))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .w(px(110.))
                                    .child("Status"),
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(div().w(px(5.)).h(px(5.)).rounded_full().bg(mu_dot))
                                    .child(
                                        div().text_xs().text_color(mu_dot).child(mu_status.clone()),
                                    ),
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
                    .child(kv_row(
                        "Server URL",
                        mu_server_url.clone(),
                        muted,
                        if mu_active { fg } else { muted },
                    ))
                    .child(kv_row(
                        "Session ID",
                        mu_session_id.clone(),
                        muted,
                        if mu_active { fg } else { muted },
                    ))
                    .child(kv_row(
                        "Peer ID",
                        mu_peer_id.clone(),
                        muted,
                        if mu_active { fg } else { muted },
                    ))
                    .child(kv_row(
                        "Join token",
                        mu_join_token.clone(),
                        muted,
                        if !mu_join_token.is_empty() && mu_join_token != "—" {
                            fg
                        } else {
                            muted
                        },
                    ))
                    .when(mu_active, |el| {
                        el.child(kv_row(
                            "Participants",
                            mu_participants_count.to_string(),
                            muted,
                            fg,
                        ))
                    }),
            )
            // ── Windows & Renderers ───────────────────────────────────────────
            .child(
                h_flex()
                    .px_4()
                    .pt_3()
                    .pb_3()
                    .gap_4()
                    .items_start()
                    // Windows column
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_1()
                            .child(section_header(
                                &format!("Windows ({})", window_count),
                                fg,
                                border,
                            ))
                            .when(window_rows.is_empty(), |el| {
                                el.child(div().text_xs().text_color(muted).py(px(2.)).child("None"))
                            })
                            .children(
                                window_rows
                                    .into_iter()
                                    .take(6)
                                    .map(|r| div().text_xs().text_color(fg).py(px(1.)).child(r)),
                            ),
                    )
                    // Renderers column
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_1()
                            .child(section_header(
                                &format!("Renderers ({})", renderer_count),
                                fg,
                                border,
                            ))
                            .when(renderer_rows.is_empty(), |el| {
                                el.child(div().text_xs().text_color(muted).py(px(2.)).child("None"))
                            })
                            .children(
                                renderer_rows
                                    .into_iter()
                                    .take(6)
                                    .map(|r| div().text_xs().text_color(fg).py(px(1.)).child(r)),
                            ),
                    ),
            )
    }
}
