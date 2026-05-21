use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use engine_state::EngineContext;
use ui::{button::Button, h_flex, v_flex, ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt};
use super::{
    DevInspectEngineState, DevOpenWorkspaceRoot, DevReloadAssets, DevSaveAsDefaultLevel,
    DevShowBuildInfo, ToggleProblems,
};

// ── State snapshot types ──────────────────────────────────────────────────────

struct MultiuserSnapshot {
    is_connected: bool,
    is_host: bool,
    status: String,
    server_url: String,
    session_id: String,
    peer_id: String,
    host_peer_id: String,
    join_token: Option<String>,
    project_id: Option<String>,
    participants: Vec<String>,
}

struct EngineSnapshot {
    version: &'static str,
    build: &'static str,
    exe_path: String,
    is_source_build: bool,
    source_path: String,
    project_path: String,
    project_window_id: String,
    uri_project_path: String,
    window_count: usize,
    renderer_count: usize,
    has_type_db: bool,
    verbose_launch: bool,
    default_level_size: String,
    has_discord: bool,
    has_window_manager: bool,
    multiuser: Option<MultiuserSnapshot>,
    window_rows: Vec<String>,
    renderer_rows: Vec<String>,
}

impl EngineSnapshot {
    fn capture() -> Self {
        let exe_path = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "—".to_string());

        let Some(ctx) = EngineContext::global() else {
            return Self {
                version: "?",
                build: "?",
                exe_path,
                is_source_build: false,
                source_path: "—".to_string(),
                project_path: "—".to_string(),
                project_window_id: "—".to_string(),
                uri_project_path: "—".to_string(),
                window_count: 0,
                renderer_count: 0,
                has_type_db: false,
                verbose_launch: false,
                default_level_size: "—".to_string(),
                has_discord: false,
                has_window_manager: false,
                multiuser: None,
                window_rows: Vec::new(),
                renderer_rows: Vec::new(),
            };
        };

        let (project_path, project_window_id) = ctx
            .project
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

        let dev = ctx.dev.read();
        let source_path = dev
            .source_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "—".to_string());
        let is_source_build = dev.is_source_build;
        drop(dev);

        let launch = ctx.launch.read();
        let uri_project_path = launch
            .uri_project_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "—".to_string());
        let verbose_launch = launch.verbose;
        drop(launch);

        let default_level_size = ctx
            .default_level_bytes
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

        let multiuser = ctx.multiuser.read().as_ref().map(|mu| {
            let is_connected = mu.is_connected();
            let status = match &mu.status {
                engine_state::MultiuserStatus::Connected => "Connected".to_string(),
                engine_state::MultiuserStatus::Connecting => "Connecting…".to_string(),
                engine_state::MultiuserStatus::Disconnected => "Disconnected".to_string(),
                engine_state::MultiuserStatus::Error(e) => format!("Error: {}", e),
            };
            MultiuserSnapshot {
                is_connected,
                is_host: mu.is_host,
                status,
                server_url: mu.server_url.clone(),
                session_id: mu.session_id.clone(),
                peer_id: mu.peer_id.clone(),
                host_peer_id: mu.host_peer_id.clone(),
                join_token: mu.join_token.clone(),
                project_id: mu.project_id.clone(),
                participants: mu.participants.clone(),
            }
        });

        let mut window_rows: Vec<String> = ctx
            .windows
            .iter()
            .map(|e| format!("#{:<4} {:?}", e.key(), e.value().window_type))
            .collect();
        window_rows.sort();

        let mut renderer_rows: Vec<String> = ctx
            .renderers
            .window_ids()
            .into_iter()
            .map(|id| format!("#{}", id))
            .collect();
        renderer_rows.sort();

        Self {
            version: env!("CARGO_PKG_VERSION"),
            build: option_env!("BUILD_INFO").unwrap_or("dev"),
            exe_path,
            is_source_build,
            source_path,
            project_path,
            project_window_id,
            uri_project_path,
            window_count: ctx.windows.len(),
            renderer_count: ctx.renderers.window_ids().len(),
            has_type_db: ctx.type_database.read().is_some(),
            verbose_launch,
            default_level_size,
            has_discord: ctx.discord.read().is_some(),
            has_window_manager: ctx.window_manager.read().is_some(),
            multiuser,
            window_rows,
            renderer_rows,
        }
    }
}

// ── Component ─────────────────────────────────────────────────────────────────

pub struct DevPopover {
    focus_handle: FocusHandle,
    _refresh_task: Task<()>,
}

impl DevPopover {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let refresh_task = cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_secs(1)).await;
                if let Some(this) = this.upgrade() {
                    let _ = this.update(cx, |_, cx| cx.notify());
                } else {
                    break;
                }
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
        .child(
            div()
                .text_xs()
                .text_color(fg)
                .child(title.to_string()),
        )
        .child(div().flex_1().h(px(1.)).bg(border))
}

fn kv(label: &str, value: String, label_color: Hsla, value_color: Hsla) -> Div {
    h_flex()
        .gap_2()
        .py(px(1.))
        .child(
            div()
                .text_xs()
                .text_color(label_color)
                .w(px(100.))
                .flex_shrink_0()
                .child(label.to_string()),
        )
        .child(
            div()
                .text_xs()
                .text_color(value_color)
                .flex_1()
                .overflow_hidden()
                .child(value),
        )
}

fn kv_opt(label: &str, value: &Option<String>, label_color: Hsla, value_color: Hsla, muted: Hsla) -> Div {
    kv(
        label,
        value.clone().unwrap_or_else(|| "—".to_string()),
        label_color,
        if value.is_some() { value_color } else { muted },
    )
}

fn status_pill(active: bool, on_label: &str, off_label: &str, on_color: Hsla, off_color: Hsla) -> Div {
    let (color, label) = if active {
        (on_color, on_label)
    } else {
        (off_color, off_label)
    };
    h_flex()
        .gap_1()
        .items_center()
        .child(div().w(px(6.)).h(px(6.)).rounded_full().bg(color))
        .child(div().text_xs().text_color(color).child(label.to_string()))
}

fn inline_kv_status(
    label: &str,
    active: bool,
    on_label: &str,
    off_label: &str,
    label_color: Hsla,
    on_color: Hsla,
    off_color: Hsla,
) -> Div {
    h_flex()
        .gap_2()
        .py(px(1.))
        .flex_1()
        .child(
            div()
                .text_xs()
                .text_color(label_color)
                .w(px(100.))
                .flex_shrink_0()
                .child(label.to_string()),
        )
        .child(status_pill(active, on_label, off_label, on_color, off_color))
}

fn list_entry(text: String, fg: Hsla) -> Div {
    h_flex()
        .gap_2()
        .py(px(1.))
        .child(
            div()
                .w(px(6.))
                .h(px(6.))
                .rounded_full()
                .bg(fg)
                .flex_shrink_0()
                .mt(px(1.)),
        )
        .child(div().text_xs().text_color(fg).flex_1().overflow_hidden().child(text))
}

// ── Render ────────────────────────────────────────────────────────────────────

impl Render for DevPopover {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let snap = EngineSnapshot::capture();

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

        // Section headers use foreground for guaranteed visibility.
        let section_fg = fg;

        v_flex()
            .w(px(480.))
            .p_4()
            .gap_3()
            .bg(bg)
            .rounded_lg()
            .shadow_xl()
            .track_focus(&self.focus_handle)

            // ── Header ────────────────────────────────────────────
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(Icon::new(IconName::Bug).text_color(accent))
                            .child(div().text_sm().text_color(fg).child("Developer Tools")),
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
                div().text_xs().text_color(muted).child(format!(
                    "v{}  ·  {}  ·  {}",
                    snap.version,
                    snap.build,
                    if snap.is_source_build { "source build" } else { "release" },
                )),
            )

            // ── Engine ────────────────────────────────────────────
            .child(
                card(card_bg)
                    .child(section_header("ENGINE", section_fg, border))
                    .child(kv("Version", snap.version.to_string(), muted, fg))
                    .child(kv("Build", snap.build.to_string(), muted, fg))
                    .child(kv("Executable", snap.exe_path.clone(), muted, fg))
                    .child(inline_kv_status("Source Build", snap.is_source_build, "Yes", "No", muted, success, muted))
                    .child(kv("Source Root", snap.source_path.clone(), muted, fg)),
            )

            // ── Project ───────────────────────────────────────────
            .child(
                card(card_bg)
                    .child(section_header("PROJECT", section_fg, border))
                    .child(kv("Path", snap.project_path.clone(), muted, fg))
                    .child(kv("Window ID", snap.project_window_id.clone(), muted, fg))
                    .child(kv("URI Path", snap.uri_project_path.clone(), muted, fg)),
            )

            // ── Runtime ───────────────────────────────────────────
            .child(
                card(card_bg)
                    .child(section_header("RUNTIME", section_fg, border))
                    .child(
                        h_flex()
                            .gap_4()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .py(px(1.))
                                    .flex_1()
                                    .child(div().text_xs().text_color(muted).w(px(72.)).flex_shrink_0().child("Windows"))
                                    .child(div().text_xs().text_color(fg).child(snap.window_count.to_string())),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .py(px(1.))
                                    .flex_1()
                                    .child(div().text_xs().text_color(muted).w(px(72.)).flex_shrink_0().child("Renderers"))
                                    .child(div().text_xs().text_color(fg).child(snap.renderer_count.to_string())),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_4()
                            .child(inline_kv_status("Type DB", snap.has_type_db, "Loaded", "None", muted, success, muted))
                            .child(inline_kv_status("Verbose Log", snap.verbose_launch, "On", "Off", muted, warning, muted)),
                    )
                    .child(kv("Default Level", snap.default_level_size.clone(), muted, fg)),
            )

            // ── Services ──────────────────────────────────────────
            .child(
                card(card_bg)
                    .child(section_header("SERVICES", section_fg, border))
                    .child(
                        h_flex()
                            .gap_4()
                            .child(inline_kv_status("Discord", snap.has_discord, "Active", "None", muted, success, muted))
                            .child(inline_kv_status("Window Mgr", snap.has_window_manager, "Active", "None", muted, success, muted)),
                    )
                    .child(inline_kv_status(
                        "Multiuser",
                        snap.multiuser.is_some(),
                        "Enabled",
                        "Disabled",
                        muted,
                        info,
                        muted,
                    )),
            )

            // ── Multiuser (when active) ────────────────────────────
            .when(snap.multiuser.is_some(), |el| {
                let mu = snap.multiuser.as_ref().unwrap();
                let status_color = if mu.is_connected {
                    success
                } else if mu.status.starts_with("Error") {
                    danger
                } else if mu.status.starts_with("Connecting") {
                    warning
                } else {
                    muted
                };
                let participants_display = mu.participants.clone();
                let join_token = mu.join_token.clone();
                let project_id = mu.project_id.clone();

                el.child(
                    card(card_bg)
                        .child(section_header("MULTIUSER SESSION", section_fg, border))
                        .child(
                            h_flex()
                                .gap_2()
                                .py(px(1.))
                                .child(div().text_xs().text_color(muted).w(px(100.)).flex_shrink_0().child("Status"))
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .items_center()
                                        .child(div().w(px(6.)).h(px(6.)).rounded_full().bg(status_color))
                                        .child(div().text_xs().text_color(status_color).child(mu.status.clone())),
                                ),
                        )
                        .child(inline_kv_status("Role", mu.is_host, "Host", "Participant", muted, accent, fg))
                        .child(kv("Server URL", mu.server_url.clone(), muted, fg))
                        .child(kv("Session ID", mu.session_id.clone(), muted, fg))
                        .child(kv("Our Peer ID", mu.peer_id.clone(), muted, fg))
                        .child(kv("Host Peer ID", mu.host_peer_id.clone(), muted, fg))
                        .child(kv_opt("Join Token", &join_token, muted, fg, muted))
                        .child(kv_opt("Project ID", &project_id, muted, fg, muted))
                        .when(!participants_display.is_empty(), |el| {
                            el.child(
                                v_flex()
                                    .gap_y_1()
                                    .pt_1()
                                    .child(div().text_xs().text_color(muted).child(
                                        format!("Participants ({})", participants_display.len()),
                                    ))
                                    .children(
                                        participants_display.iter().take(8).map(|p| {
                                            list_entry(p.clone(), fg)
                                        }),
                                    )
                                    .when(participants_display.len() > 8, |el| {
                                        el.child(
                                            div().text_xs().text_color(muted).child(
                                                format!("+ {} more", participants_display.len() - 8),
                                            ),
                                        )
                                    }),
                            )
                        }),
                )
            })

            // ── Window Registry ────────────────────────────────────
            .when(!snap.window_rows.is_empty(), |el| {
                let rows = snap.window_rows.clone();
                el.child(
                    card(card_bg)
                        .child(section_header(
                            &format!("WINDOW REGISTRY  ({})", rows.len()),
                            section_fg,
                            border,
                        ))
                        .children(rows.iter().take(8).map(|r| list_entry(r.clone(), fg)))
                        .when(rows.len() > 8, |el| {
                            el.child(
                                div().text_xs().text_color(muted)
                                    .child(format!("+ {} more", rows.len() - 8)),
                            )
                        }),
                )
            })

            // ── Renderer Registry ──────────────────────────────────
            .when(!snap.renderer_rows.is_empty(), |el| {
                let rows = snap.renderer_rows.clone();
                el.child(
                    card(card_bg)
                        .child(section_header(
                            &format!("RENDERER REGISTRY  ({})", rows.len()),
                            section_fg,
                            border,
                        ))
                        .children(rows.iter().map(|r| list_entry(r.clone(), fg))),
                )
            })

            // ── Actions ───────────────────────────────────────────
            .child(
                v_flex()
                    .gap_2()
                    .child(section_header("ACTIONS", section_fg, border))
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
                                    .on_click(|_, _, cx| cx.dispatch_action(&DevInspectEngineState)),
                            )
                            .child(
                                Button::new("save-default-level")
                                    .label("Save Level")
                                    .icon(IconName::Database)
                                    .small()
                                    .on_click(|_, _, cx| cx.dispatch_action(&DevSaveAsDefaultLevel)),
                            ),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("Pulsar Engine · dev panel · refreshes every 1s"),
            )
    }
}
