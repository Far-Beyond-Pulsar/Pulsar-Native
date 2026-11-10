//! Developer popover: a live engine-state panel opened from the Dev menu.
//!
//! Owns the `DevPopover` view (refreshes once a second), its layout primitives
//! and one `render_*` helper per region. Engine state is read once per render
//! into a [`snapshot::DevSnapshot`]; the entry point is `DevPopover::new`.

mod snapshot;

use std::time::Duration;

use super::{
    DevInspectEngineState, DevOpenWorkspaceRoot, DevReloadAssets, DevSaveAsDefaultLevel,
    DevShowBuildInfo, ToggleProblems,
};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use snapshot::{DevSnapshot, MultiuserSnapshot, StatusDot};
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

/// Theme colors used by every region of the popover.
#[derive(Clone, Copy)]
struct Palette {
    bg: Hsla,
    border: Hsla,
    fg: Hsla,
    muted: Hsla,
    accent: Hsla,
    success: Hsla,
    warning: Hsla,
    danger: Hsla,
    info: Hsla,
}

impl Palette {
    /// Color for a multiuser status-dot category.
    fn dot_color(&self, dot: StatusDot) -> Hsla {
        match dot {
            StatusDot::Muted => self.muted,
            StatusDot::Success => self.success,
            StatusDot::Warning => self.warning,
            StatusDot::Danger => self.danger,
        }
    }
}

impl Render for DevPopover {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let palette = Palette {
            bg: theme.background,
            border: theme.border,
            fg: theme.foreground,
            muted: theme.muted_foreground,
            accent: theme.accent,
            success: theme.success,
            warning: theme.warning,
            danger: theme.danger,
            info: theme.info,
        };

        // ── Snapshot engine state into plain owned values ─────────────────────
        let snapshot = DevSnapshot::capture();

        // ── Build UI ──────────────────────────────────────────────────────────
        v_flex()
            .id("dev-popover-root")
            .w(px(420.))
            .max_h(px(640.))
            .overflow_y_scroll()
            .bg(palette.bg)
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .child(render_header(&snapshot, &palette))
            .child(render_quick_actions(&palette))
            .child(render_status_section(&snapshot, &palette))
            .child(render_engine_and_project(&snapshot, &palette))
            .child(render_multiuser_section(&snapshot.multiuser, &palette))
            .child(render_windows_and_renderers(snapshot, &palette))
    }
}

/// Title bar: bug icon, "Developer", version / build pills and the live dot.
fn render_header(snapshot: &DevSnapshot, p: &Palette) -> impl IntoElement {
    let Palette {
        border,
        fg,
        muted,
        accent,
        success,
        ..
    } = *p;

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
                    format!("v{}", snapshot.engine_version),
                    muted,
                    border.opacity(0.5),
                ))
                .child(pill(
                    if snapshot.is_source_build {
                        "source"
                    } else {
                        &snapshot.build_info
                    },
                    if snapshot.is_source_build {
                        success
                    } else {
                        muted
                    },
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
        )
}

/// "Quick Actions" section: buttons that dispatch the developer actions.
fn render_quick_actions(p: &Palette) -> impl IntoElement {
    v_flex()
        .px_4()
        .pt_3()
        .pb_2()
        .gap_2()
        .border_b_1()
        .border_color(p.border)
        .child(section_header("Quick Actions", p.fg, p.border))
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
                        .on_click(|_, _, cx| cx.dispatch_action(&DevInspectEngineState)),
                )
                .child(
                    Button::new("save-level")
                        .label("Save Level")
                        .icon(IconName::Database)
                        .small()
                        .ghost()
                        .on_click(|_, _, cx| cx.dispatch_action(&DevSaveAsDefaultLevel)),
                ),
        )
}

/// "Status" section: subsystem pills and the default level size.
fn render_status_section(snapshot: &DevSnapshot, p: &Palette) -> impl IntoElement {
    let Palette {
        border,
        fg,
        muted,
        success,
        warning,
        info,
        ..
    } = *p;

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
                    format!("{} windows", snapshot.window_count),
                    fg,
                    border.opacity(0.4),
                ))
                .child(pill(
                    format!("{} renderers", snapshot.renderer_count),
                    fg,
                    border.opacity(0.4),
                ))
                .child(pill(
                    if snapshot.has_user_types {
                        "Types ✓"
                    } else {
                        "Types ✗"
                    },
                    if snapshot.has_user_types {
                        success
                    } else {
                        warning
                    },
                    border.opacity(0.4),
                ))
                .child(pill(
                    if snapshot.has_discord {
                        "Discord"
                    } else {
                        "No Discord"
                    },
                    if snapshot.has_discord { info } else { muted },
                    border.opacity(0.4),
                ))
                .child(pill(
                    if snapshot.has_window_manager {
                        "WinMgr"
                    } else {
                        "No WinMgr"
                    },
                    if snapshot.has_window_manager {
                        info
                    } else {
                        muted
                    },
                    border.opacity(0.4),
                ))
                .child(pill(
                    if snapshot.verbose_launch {
                        "Verbose"
                    } else {
                        "Silent"
                    },
                    if snapshot.verbose_launch {
                        warning
                    } else {
                        muted
                    },
                    border.opacity(0.4),
                )),
        )
        .child(kv_row(
            "Default level",
            snapshot.default_level_size.clone(),
            muted,
            fg,
        ))
}

/// Side-by-side "Engine" and "Project" columns.
fn render_engine_and_project(snapshot: &DevSnapshot, p: &Palette) -> impl IntoElement {
    let Palette {
        border,
        fg,
        muted,
        success,
        ..
    } = *p;

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
                    snapshot.is_source_build,
                    "Yes",
                    "No",
                    muted,
                    success,
                    muted,
                ))
                .child(kv_row(
                    "Source root",
                    snapshot.source_path.clone(),
                    muted,
                    fg,
                ))
                .child(kv_row("Executable", snapshot.exe_path.clone(), muted, fg)),
        )
        // Project column
        .child(
            v_flex()
                .flex_1()
                .gap_1()
                .child(section_header("Project", fg, border))
                .child(kv_row("Path", snapshot.project_path.clone(), muted, fg))
                .child(kv_row(
                    "Window ID",
                    snapshot.project_window_id.clone(),
                    muted,
                    fg,
                ))
                .child(kv_row(
                    "URI path",
                    snapshot.uri_project_path.clone(),
                    muted,
                    fg,
                )),
        )
}

/// "Multiuser Session" section: status dot, role and connection details.
fn render_multiuser_section(mu: &MultiuserSnapshot, p: &Palette) -> impl IntoElement {
    let Palette {
        border,
        fg,
        muted,
        accent,
        ..
    } = *p;
    let mu_dot = p.dot_color(mu.dot());
    let value_color = if mu.active { fg } else { muted };

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
                        .child(div().text_xs().text_color(mu_dot).child(mu.status.clone())),
                ),
        )
        .child(status_row(
            "Role",
            mu.is_host,
            "Host",
            "Participant",
            muted,
            accent,
            fg,
        ))
        .child(kv_row(
            "Server URL",
            mu.server_url.clone(),
            muted,
            value_color,
        ))
        .child(kv_row(
            "Session ID",
            mu.session_id.clone(),
            muted,
            value_color,
        ))
        .child(kv_row("Peer ID", mu.peer_id.clone(), muted, value_color))
        .child(kv_row(
            "Join token",
            mu.join_token.clone(),
            muted,
            if mu.has_join_token() { fg } else { muted },
        ))
        .when(mu.active, |el| {
            el.child(kv_row(
                "Participants",
                mu.participants_count.to_string(),
                muted,
                fg,
            ))
        })
}

/// Side-by-side "Windows" and "Renderers" lists (first six rows each).
fn render_windows_and_renderers(snapshot: DevSnapshot, p: &Palette) -> impl IntoElement {
    h_flex()
        .px_4()
        .pt_3()
        .pb_3()
        .gap_4()
        .items_start()
        // Windows column
        .child(render_row_list(
            &format!("Windows ({})", snapshot.window_count),
            snapshot.window_rows,
            p,
        ))
        // Renderers column
        .child(render_row_list(
            &format!("Renderers ({})", snapshot.renderer_count),
            snapshot.renderer_rows,
            p,
        ))
}

/// Titled column listing up to six rows, or "None" when empty.
fn render_row_list(title: &str, rows: Vec<String>, p: &Palette) -> impl IntoElement {
    let Palette {
        border, fg, muted, ..
    } = *p;

    v_flex()
        .flex_1()
        .gap_1()
        .child(section_header(title, fg, border))
        .when(rows.is_empty(), |el| {
            el.child(div().text_xs().text_color(muted).py(px(2.)).child("None"))
        })
        .children(
            rows.into_iter()
                .take(6)
                .map(|r| div().text_xs().text_color(fg).py(px(1.)).child(r)),
        )
}
