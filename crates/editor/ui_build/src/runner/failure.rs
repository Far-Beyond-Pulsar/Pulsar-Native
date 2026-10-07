//! The dialog shown when a build fails: one card per compiler error.

use std::rc::Rc;

use gpui::{
    ClipboardItem, Context, FontWeight, IntoElement, ListSizingBehavior, Render, Size, Window, div,
    prelude::*, px, size,
};
use ui::button::{Button, ButtonVariants as _};
use ui::{
    ActiveTheme as _, ContextModal as _, IconName, Sizable as _, VirtualListScrollHandle, h_flex,
    v_flex, v_virtual_list,
};

use super::cargo::ERROR_SEPARATOR;

/// Split a failure message into its separate errors.
pub fn parse_errors(message: &str) -> Vec<String> {
    let mut errors: Vec<String> = message
        .split(ERROR_SEPARATOR)
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect();
    if errors.is_empty() {
        errors.push(message.trim().to_owned());
    }
    errors
}

struct FailureList {
    errors: Vec<String>,
    item_sizes: Rc<Vec<Size<gpui::Pixels>>>,
    scroll_handle: VirtualListScrollHandle,
}

impl FailureList {
    fn new(errors: Vec<String>) -> Self {
        Self {
            item_sizes: Rc::new(errors.iter().map(|_| size(px(0.), px(220.))).collect()),
            errors,
            scroll_handle: VirtualListScrollHandle::new(),
        }
    }

    fn render_error(&self, index: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let error = self.errors[index].clone();
        let copy = error.clone();
        v_flex()
            .w_full()
            .h(px(208.))
            .gap_2()
            .p_2()
            .rounded_md()
            .bg(cx.theme().background.opacity(0.65))
            .border_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .child(format!("Error {}", index + 1)),
                    )
                    .child(
                        Button::new(format!("copy-build-error-{index}"))
                            .small()
                            .ghost()
                            .icon(IconName::Copy)
                            .label("Copy")
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                            }),
                    ),
            )
            .child(
                div()
                    .id(format!("build-error-body-{index}"))
                    .flex_1()
                    .w_full()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .font_family("monospace")
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(error),
                    ),
            )
            .into_any_element()
    }
}

impl Render for FailureList {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let sizes = Rc::clone(&self.item_sizes);
        v_flex()
            .w_full()
            .h(px(560.))
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "{} compiler error{} captured from Cargo.",
                        self.errors.len(),
                        if self.errors.len() == 1 { "" } else { "s" }
                    )),
            )
            .child(
                div()
                    .id("build-failure-list-container")
                    .flex_1()
                    .overflow_hidden()
                    .child(
                        v_virtual_list(
                            view,
                            "build-failure-list",
                            sizes,
                            |this, range, _window, cx| {
                                range
                                    .map(|ix| this.render_error(ix, cx))
                                    .collect::<Vec<_>>()
                            },
                        )
                        .with_sizing_behavior(ListSizingBehavior::Infer)
                        .track_scroll(&self.scroll_handle),
                    ),
            )
    }
}

/// Open the failure dialog for `message` (a cargo failure, see
/// [`super::cargo::ERROR_SEPARATOR`]).
pub fn show(message: String, title: String, window: &mut Window, cx: &mut gpui::App) {
    let errors = parse_errors(&message);
    let all = errors.join(&format!("\n\n{ERROR_SEPARATOR}\n\n"));
    window.open_modal(cx, move |modal, _, cx| {
        let list = cx.new(|_| FailureList::new(errors.clone()));
        let copy_all = all.clone();
        modal
            .width(px(900.))
            .title(title.clone())
            .show_close(true)
            .overlay_closable(true)
            .child(
                v_flex().w_full().gap_3().child(list).child(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("copy-all-build-errors")
                                .primary()
                                .icon(IconName::Copy)
                                .label("Copy All Errors")
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        copy_all.clone(),
                                    ));
                                }),
                        )
                        .child(
                            Button::new("close-build-errors")
                                .ghost()
                                .label("Close")
                                .on_click(|_, window, cx| window.close_modal(cx)),
                        ),
                ),
            )
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_are_split_on_the_separator() {
        let message =
            format!("cargo build failed:\n\na{ERROR_SEPARATOR}b\n\n{ERROR_SEPARATOR}\n\n");
        assert_eq!(parse_errors(&message), ["cargo build failed:\n\na", "b"]);
    }

    #[test]
    fn a_message_without_the_separator_is_one_error() {
        assert_eq!(parse_errors("  boom  "), ["boom"]);
    }
}
