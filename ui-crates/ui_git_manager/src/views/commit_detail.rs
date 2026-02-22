//! Commit detail panel: shows files changed in the selected commit (left)
//! and content of the selected file from that commit (right).

use crate::{GitManager, models::*, FileContentResult};
use gpui::*;
use ui::{
    h_flex, v_flex, Icon, IconName, ActiveTheme as _, StyledExt,
    scroll::ScrollbarAxis,
    input::TextInput,
};

pub fn render_commit_detail(git_manager: &GitManager, cx: &mut Context<GitManager>) -> impl IntoElement {
    // Copy theme values before any listener() calls
    let border = cx.theme().border;
    let foreground = cx.theme().foreground;
    let muted_fg = cx.theme().muted_foreground;
    let danger = cx.theme().danger;
    let success = cx.theme().success;
    let warning = cx.theme().warning;
    let radius = cx.theme().radius;
    let list_active = cx.theme().list_active;
    let list_hover = cx.theme().list_hover;
    let primary = cx.theme().primary;

    // No commit selected — placeholder
    let Some(_) = &git_manager.selected_commit else {
        return v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .child(Icon::new(IconName::GitBranch).size(px(32.)).text_color(muted_fg))
            .child(div().text_xs().text_color(muted_fg).child("Select a commit to see changed files"))
            .into_any_element();
    };

    // ── Files list (left sub-panel) ──────────────────────────────────────────
    let file_count = git_manager.selected_commit_files.len();
    let mut file_list = v_flex().id("commit-files-list").w_full().gap_px().p_1();

    for file in &git_manager.selected_commit_files {
        let file_path = file.path.clone();
        let display_name = file
            .path
            .rsplit('/')
            .next()
            .or_else(|| file.path.rsplit('\\').next())
            .unwrap_or(file.path.as_str())
            .to_string();
        let file_status = file.status.short_str();
        let status_color = match file.status {
            ChangeStatus::Added => success,
            ChangeStatus::Deleted => danger,
            _ => warning,
        };
        let is_selected = git_manager
            .selected_commit_file
            .as_deref()
            .map(|s| s == file.path.as_str())
            .unwrap_or(false);
        let bg = if is_selected { list_active } else { gpui::transparent_black() };

        file_list = file_list.child(
            h_flex()
                .px_1()
                .py_1()
                .rounded(radius)
                .gap_1()
                .items_center()
                .bg(bg)
                .hover(move |s| s.bg(list_hover))
                .cursor_pointer()
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _: &gpui::MouseDownEvent, _, cx| {
                        this.select_commit_file(file_path.clone(), cx);
                    }),
                )
                .child(
                    div()
                        .text_xs()
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(status_color)
                        .child(file_status),
                )
                .child(
                    div()
                        .flex_1()
                        .text_xs()
                        .text_color(foreground)
                        .overflow_hidden()
                        .child(display_name),
                ),
        );
    }

    let files_loading = git_manager.selected_commit_files.is_empty()
        && git_manager.selected_commit.is_some();

    let files_pane = v_flex()
        .w(px(220.))
        .h_full()
        .border_r_1()
        .border_color(border)
        .overflow_hidden()
        .child(
            div()
                .px_2()
                .py_1()
                .border_b_1()
                .border_color(border)
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(muted_fg)
                .child(if files_loading {
                    "Loading…".to_string()
                } else {
                    format!("{} file{}", file_count, if file_count == 1 { "" } else { "s" })
                }),
        )
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .child(file_list.scrollable(ScrollbarAxis::Vertical)),
        );

    // ── File content (right sub-panel) ───────────────────────────────────────
    let content_header_label = match &git_manager.selected_commit_file {
        Some(path) => path.clone(),
        None => "File Preview".to_string(),
    };

    let content_header = div()
        .px_3()
        .py_2()
        .border_b_1()
        .border_color(border)
        .text_xs()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(muted_fg)
        .overflow_hidden()
        .child(content_header_label);

    let content_body: AnyElement = match &git_manager.selected_commit_file {
        None => v_flex()
            .flex_1().items_center().justify_center().gap_2()
            .child(Icon::new(IconName::Code).size(px(24.)).text_color(muted_fg))
            .child(div().text_xs().text_color(muted_fg).child("Select a file to preview"))
            .into_any_element(),

        Some(_) => match (&git_manager.commit_file_viewer, &git_manager.selected_commit_file_content) {
            // Viewer ready — full height
            (Some(viewer), _) => div()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .child(TextInput::new(viewer).disabled(true))
                .into_any_element(),

            (None, Some(FileContentResult::Binary)) => v_flex()
                .flex_1().items_center().justify_center().gap_2()
                .child(Icon::new(IconName::Code).size(px(24.)).text_color(muted_fg))
                .child(div().text_sm().font_weight(gpui::FontWeight::SEMIBOLD).text_color(foreground).child("Binary file"))
                .child(div().text_xs().text_color(muted_fg).child("Cannot display as text"))
                .into_any_element(),

            (None, Some(FileContentResult::TooLong(lines))) => v_flex()
                .flex_1().items_center().justify_center().gap_2()
                .child(Icon::new(IconName::Code).size(px(24.)).text_color(muted_fg))
                .child(div().text_sm().font_weight(gpui::FontWeight::SEMIBOLD).text_color(foreground).child("File too long"))
                .child(div().text_xs().text_color(muted_fg).child(format!("{} lines — preview limited to 1 000 lines", lines)))
                .into_any_element(),

            (None, Some(FileContentResult::Error(msg))) => v_flex()
                .flex_1().items_center().justify_center().gap_2()
                .child(Icon::new(IconName::CircleX).size(px(24.)).text_color(danger))
                .child(div().text_sm().font_weight(gpui::FontWeight::SEMIBOLD).text_color(foreground).child("Could not read file"))
                .child(div().text_xs().text_color(muted_fg).child(msg.clone()))
                .into_any_element(),

            // Loading (or Text routed to viewer)
            (None, _) => v_flex()
                .flex_1().items_center().justify_center()
                .child(div().text_xs().text_color(muted_fg).child("Loading\u{2026}"))
                .into_any_element(),
        },
    };

    let content_pane = v_flex()
        .flex_1()
        .h_full()
        .overflow_hidden()
        .child(content_header)
        .child(content_body);

    h_flex()
        .size_full()
        .child(files_pane)
        .child(content_pane)
        .into_any_element()
}
