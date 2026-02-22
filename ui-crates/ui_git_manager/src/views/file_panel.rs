//! File content preview panel (right side)

use crate::{GitManager, FileContentResult};
use gpui::*;
use ui::{h_flex, v_flex, Icon, IconName, ActiveTheme as _, input::TextInput};

pub fn render_file_panel(git_manager: &GitManager, cx: &mut Context<GitManager>) -> impl IntoElement {
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

    let body: AnyElement = match &git_manager.selected_file {
        None => v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_2()
            .child(Icon::new(IconName::Code).size(px(32.)).text_color(muted_fg))
            .child(div().text_xs().text_color(muted_fg).child("Select a file to preview its contents"))
            .into_any_element(),

        Some(_) => match (&git_manager.file_viewer, &git_manager.file_content) {
            // Viewer ready — show it full height
            (Some(viewer), _) => div()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .child(
                TextInput::new(viewer)
                    
                    .h_full()
                    .w_full()
                    .font(gpui::Font {
                        family: "JetBrains Mono".to_string().into(),
                        weight: gpui::FontWeight::NORMAL,
                        style: gpui::FontStyle::Normal,
                        features: gpui::FontFeatures::default(),
                        fallbacks: Some(gpui::FontFallbacks::from_fonts(vec!["monospace".to_string()])),
                    })
                    .text_size(px(14.0))
                    .border_0()
            )
                .into_any_element(),

            (None, Some(FileContentResult::Binary)) => v_flex()
                .flex_1().items_center().justify_center().gap_2()
                .child(Icon::new(IconName::Code).size(px(32.)).text_color(muted_fg))
                .child(div().text_sm().font_weight(gpui::FontWeight::SEMIBOLD).text_color(foreground).child("Binary file"))
                .child(div().text_xs().text_color(muted_fg).child("This file cannot be displayed as text"))
                .into_any_element(),

            (None, Some(FileContentResult::TooLong(lines))) => v_flex()
                .flex_1().items_center().justify_center().gap_2()
                .child(Icon::new(IconName::Code).size(px(32.)).text_color(muted_fg))
                .child(div().text_sm().font_weight(gpui::FontWeight::SEMIBOLD).text_color(foreground).child("File too long"))
                .child(div().text_xs().text_color(muted_fg).child(format!("{} lines — preview limited to 1 000 lines", lines)))
                .into_any_element(),

            (None, Some(FileContentResult::Error(msg))) => v_flex()
                .flex_1().items_center().justify_center().gap_2()
                .child(Icon::new(IconName::CircleX).size(px(32.)).text_color(danger))
                .child(div().text_sm().font_weight(gpui::FontWeight::SEMIBOLD).text_color(foreground).child("Could not read file"))
                .child(div().text_xs().text_color(muted_fg).child(msg.clone()))
                .into_any_element(),

            // Loading (or Text variant routed to viewer — shouldn't appear here normally)
            (None, _) => v_flex()
                .flex_1().items_center().justify_center()
                .child(div().text_xs().text_color(muted_fg).child("Loading\u{2026}"))
                .into_any_element(),
        },
    };

    v_flex().size_full().child(header).child(body)
}


