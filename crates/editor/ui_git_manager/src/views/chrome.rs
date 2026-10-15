//! Window chrome: top bar, status banners and the no-repository screens.

use super::common::pill;
use crate::{GitLayout, GitManager, RepoPhase};
use gpui::{prelude::FluentBuilder as _, *};
use ui::{
    ActiveTheme as _, Disableable as _, Icon, IconName,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::TextInput,
    v_flex,
};

pub(crate) const TOP_BAR_HEIGHT: f32 = 40.0;

const ENGINE_REPOSITORY_URL: &str = "https://github.com/Far-Beyond-Pulsar/Pulsar-Native";

fn is_source_build() -> bool {
    engine_state::EngineContext::global()
        .map(|ctx| {
            ctx.store
                .get_or_init::<engine_state::DevContext>()
                .read()
                .is_source_build
        })
        .unwrap_or(false)
}

pub(crate) fn render_top_bar(
    gm: &mut GitManager,
    cx: &mut Context<GitManager>,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let (border, tab_bar, muted_fg, foreground, primary, warning, radius, muted_bg) = (
        theme.border,
        theme.tab_bar,
        theme.muted_foreground,
        theme.foreground,
        theme.primary,
        theme.warning,
        theme.radius,
        theme.muted,
    );
    let ready = matches!(gm.phase, RepoPhase::Ready);
    let repo_name = gm
        .project_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| gm.project_path.display().to_string());

    let mut bar = h_flex()
        .w_full()
        .h(px(TOP_BAR_HEIGHT))
        .flex_shrink_0()
        .px_3()
        .gap_2()
        .items_center()
        .bg(tab_bar)
        .border_b_1()
        .border_color(border)
        .child(
            h_flex()
                .gap_1p5()
                .items_center()
                .child(Icon::new(IconName::Folder).size(px(14.)).text_color(muted_fg))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(foreground)
                        .child(repo_name),
                ),
        );

    if ready {
        let branch = gm.repo_state.current_branch.clone();
        let detached = gm.repo_state.detached;
        let (ahead, behind) = (gm.repo_state.ahead, gm.repo_state.behind);
        let has_commits = gm.repo_state.has_commits;
        let busy = gm.remote_busy;

        bar = bar
            .child(
                h_flex()
                    .id("branch-button")
                    .px_2()
                    .h(px(26.))
                    .gap_1p5()
                    .items_center()
                    .rounded(radius)
                    .border_1()
                    .border_color(border)
                    .bg(muted_bg.opacity(0.35))
                    .hover(move |s| s.bg(muted_bg.opacity(0.7)))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            this.toggle_branch_menu(window, cx);
                        }),
                    )
                    .child(Icon::new(IconName::GitBranch).size(px(13.)).text_color(primary))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(foreground)
                            .max_w(px(220.))
                            .overflow_hidden()
                            .child(if detached {
                                format!("detached @ {}", branch)
                            } else {
                                branch
                            }),
                    )
                    .child(
                        Icon::new(IconName::ChevronDown)
                            .size(px(12.))
                            .text_color(muted_fg),
                    ),
            )
            .when_some(gm.repo_state.operation, |bar, operation| {
                bar.child(pill(operation, warning, false))
            })
            .child(div().flex_1());

        if let Some(op) = busy {
            bar = bar.child(
                div()
                    .text_xs()
                    .text_color(muted_fg)
                    .child(op.progress_label()),
            );
        }

        let behind_label = (behind > 0).then(|| behind.to_string());
        let ahead_label = (ahead > 0).then(|| ahead.to_string());
        bar = bar
            .child(
                Button::new("refresh")
                    .icon(IconName::Refresh)
                    .ghost()
                    .compact()
                    .tooltip("Refresh")
                    .on_click(cx.listener(|this, _, _, cx| this.refresh_state(cx))),
            )
            .child(
                Button::new("fetch")
                    .label("Fetch")
                    .ghost()
                    .compact()
                    .disabled(busy.is_some())
                    .tooltip("Fetch from remote")
                    .on_click(cx.listener(|this, _, _, cx| this.fetch(cx))),
            )
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        Button::new("pull")
                            .icon(IconName::ArrowDown)
                            .label("Pull")
                            .ghost()
                            .compact()
                            .disabled(busy.is_some() || !has_commits)
                            .tooltip(if behind > 0 {
                                format!("Pull {} commit(s)", behind)
                            } else {
                                "Pull".to_string()
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.pull(cx))),
                    )
                    .children(behind_label.map(|n| pill(n, primary, false))),
            )
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        Button::new("push")
                            .icon(IconName::ArrowUp)
                            .label("Push")
                            .ghost()
                            .compact()
                            .disabled(busy.is_some() || !has_commits)
                            .tooltip(if ahead > 0 {
                                format!("Push {} commit(s)", ahead)
                            } else {
                                "Push".to_string()
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.push(cx))),
                    )
                    .children(ahead_label.map(|n| pill(n, primary, false))),
            )
            .child(div().w(px(1.)).h(px(18.)).bg(border))
            .child(layout_toggle(gm.layout, cx));
    } else {
        bar = bar.child(div().flex_1());
    }

    bar = bar.when(is_source_build(), |bar| {
        bar.child(
            Button::new("engine-repository")
                .icon(IconName::Github)
                .label("Engine Repository")
                .ghost()
                .compact()
                .tooltip("Open the Pulsar-Native engine repository on GitHub")
                .on_click(|_, _, cx| cx.open_url(ENGINE_REPOSITORY_URL)),
        )
    });
    bar
}

fn layout_toggle(layout: GitLayout, cx: &mut Context<GitManager>) -> impl IntoElement + use<> {
    let button = |id: &'static str, icon: IconName, tip: &'static str, active: bool| {
        let button = Button::new(id).icon(icon).compact().tooltip(tip);
        if active { button.primary() } else { button.ghost() }
    };
    h_flex()
        .gap_px()
        .child(
            button("layout-tree", IconName::Tree, "Tree layout", layout == GitLayout::Tree)
                .on_click(cx.listener(|this, _, _, cx| this.set_layout(GitLayout::Tree, cx))),
        )
        .child(
            button(
                "layout-desktop",
                IconName::List,
                "Desktop layout",
                layout == GitLayout::Desktop,
            )
            .on_click(cx.listener(|this, _, _, cx| this.set_layout(GitLayout::Desktop, cx))),
        )
}

/// The error banner and the credential prompt, under the top bar.
pub(crate) fn render_banners(
    gm: &mut GitManager,
    cx: &mut Context<GitManager>,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let (danger, warning, radius) = (theme.danger, theme.warning, theme.radius);

    if let Some(pending) = gm.pending_auth.as_ref() {
        let op = pending.op;
        return v_flex()
            .w_full()
            .mx_2()
            .my_1()
            .px_2()
            .py_2()
            .gap(px(6.))
            .rounded(radius)
            .bg(warning.opacity(0.08))
            .border_1()
            .border_color(warning.opacity(0.3))
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(Icon::new(IconName::TriangleAlert).size(px(12.)).text_color(warning))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(warning)
                            .child(format!("Authentication required for {}", op.label())),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(TextInput::new(&gm.auth_username_input)))
                    .child(div().flex_1().child(TextInput::new(&gm.auth_password_input)))
                    .child(
                        Button::new("auth-retry")
                            .label(op.label())
                            .primary()
                            .compact()
                            .on_click(cx.listener(|this, _, _, cx| this.retry_with_auth(cx))),
                    )
                    .child(
                        Button::new("auth-cancel")
                            .label("Cancel")
                            .ghost()
                            .compact()
                            .on_click(cx.listener(|this, _, _, cx| this.cancel_auth(cx))),
                    ),
            )
            .into_any_element();
    }

    if let Some(error) = gm.op_error.clone() {
        return h_flex()
            .w_full()
            .mx_2()
            .my_1()
            .px_2()
            .py(px(6.))
            .gap(px(6.))
            .items_center()
            .rounded(radius)
            .bg(danger.opacity(0.1))
            .border_1()
            .border_color(danger.opacity(0.25))
            .child(Icon::new(IconName::CircleX).size(px(12.)).text_color(danger))
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .text_color(danger)
                    .overflow_hidden()
                    .child(error),
            )
            .child(
                Button::new("dismiss-err")
                    .icon(IconName::Close)
                    .ghost()
                    .compact()
                    .on_click(cx.listener(|this, _, _, cx| this.dismiss_error(cx))),
            )
            .into_any_element();
    }
    div().into_any_element()
}

pub(crate) fn render_loading(cx: &mut Context<GitManager>) -> impl IntoElement + use<> {
    let muted = cx.theme().muted_foreground;
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_2()
        .child(Icon::new(IconName::Loader).size(px(22.)).text_color(muted))
        .child(div().text_xs().text_color(muted).child("Reading repository…"))
}

/// The folder is not a git repository: offer to turn it into one.
pub(crate) fn render_not_a_repository(
    gm: &mut GitManager,
    cx: &mut Context<GitManager>,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let (muted, foreground) = (theme.muted_foreground, theme.foreground);
    let path = gm.project_path.display().to_string();
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_3()
        .child(Icon::new(IconName::GitBranch).size(px(40.)).text_color(muted.opacity(0.6)))
        .child(
            div()
                .text_base()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(foreground)
                .child("This project is not a Git repository"),
        )
        .child(
            div()
                .max_w(px(520.))
                .text_xs()
                .text_color(muted)
                .text_center()
                .child(path),
        )
        .child(
            div()
                .max_w(px(480.))
                .text_xs()
                .text_color(muted)
                .text_center()
                .child("Initialize a repository to track changes, browse history and use branches. You can add a remote afterwards."),
        )
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new("init-repo")
                        .label("Initialize Repository")
                        .primary()
                        .on_click(cx.listener(|this, _, _, cx| this.initialize_repository(cx))),
                )
                .child(
                    Button::new("recheck-repo")
                        .label("Check Again")
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| this.refresh_state(cx))),
                ),
        )
}

pub(crate) fn render_failed(
    _gm: &mut GitManager,
    message: String,
    cx: &mut Context<GitManager>,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let (danger, muted) = (theme.danger, theme.muted_foreground);
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_3()
        .child(Icon::new(IconName::CircleX).size(px(36.)).text_color(danger))
        .child(
            div()
                .max_w(px(560.))
                .text_sm()
                .text_color(muted)
                .text_center()
                .child(message),
        )
        .child(
            Button::new("retry-load")
                .label("Retry")
                .primary()
                .on_click(cx.listener(|this, _, _, cx| this.refresh_state(cx))),
        )
}
