//! File content preview panel (right side)

use crate::{GitManager, FileContentResult};
use gpui::*;
use ui::{h_flex, v_flex, Icon, IconName, ActiveTheme as _, StyledExt, scroll::ScrollbarAxis};

pub fn render_file_panel(git_manager: &GitManager, cx: &mut Context<GitManager>) -> impl IntoElement {
    // Copy theme values before any element building
    let border = cx.theme().border;
    let foreground = cx.theme().foreground;
    let muted_fg = cx.theme().muted_foreground;
    let danger = cx.theme().danger;

    let header_label = match &git_manager.selected_file {
        Some(path) => path.clone(),
        None => "File Preview".to_string(),
    };

    let header = h_flex()
        .px_3()
        .py_2()
        .border_b_1()
        .border_color(border)
        .child(
            div()
                .flex_1()
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(muted_fg)
                .overflow_hidden()
                .child(header_label),
        );

    let body: AnyElement = match (&git_manager.selected_file, &git_manager.file_content) {
        // No file selected
        (None, _) => v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_2()
            .child(Icon::new(IconName::Code).size(px(32.)).text_color(muted_fg))
            .child(
                div()
                    .text_xs()
                    .text_color(muted_fg)
                    .child("Select a file to preview its contents"),
            )
            .into_any_element(),

        // File selected but content not yet loaded
        (Some(_), None) => v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .child(div().text_xs().text_color(muted_fg).child("Loading…"))
            .into_any_element(),

        // Text content
        (Some(_), Some(FileContentResult::Text(text))) => div()
            .flex_1()
            .overflow_hidden()
            .child(
                div()
                    .id("git-file-scroll")
                    .size_full()
                    .p_4()
                    .scrollable(ScrollbarAxis::Both)
                    .child(
                        div()
                            .font_family("JetBrains Mono, Menlo, Monaco, Consolas, monospace")
                            .text_xs()
                            .text_color(foreground)
                            .child(text.clone()),
                    ),
            )
            .into_any_element(),

        // Binary file
        (Some(_), Some(FileContentResult::Binary)) => v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_2()
            .child(Icon::new(IconName::Code).size(px(32.)).text_color(muted_fg))
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(foreground)
                    .child("Binary file"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted_fg)
                    .child("This file cannot be displayed as text"),
            )
            .into_any_element(),

        // File too long
        (Some(_), Some(FileContentResult::TooLong(lines))) => v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_2()
            .child(Icon::new(IconName::Code).size(px(32.)).text_color(muted_fg))
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(foreground)
                    .child("File too long"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted_fg)
                    .child(format!("{} lines — preview limited to 1 000 lines", lines)),
            )
            .into_any_element(),

        // Error reading file
        (Some(_), Some(FileContentResult::Error(msg))) => v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_2()
            .child(Icon::new(IconName::CircleX).size(px(32.)).text_color(danger))
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(foreground)
                    .child("Could not read file"),
            )
            .child(div().text_xs().text_color(muted_fg).child(msg.clone()))
            .into_any_element(),
    };

    v_flex().size_full().child(header).child(body)
}
