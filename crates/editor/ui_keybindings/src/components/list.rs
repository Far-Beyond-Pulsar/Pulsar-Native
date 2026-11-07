//! The virtualized list of actions and their shortcuts.

use gpui::{prelude::*, *};
use ui::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    kbd::Kbd,
    scroll::Scrollbar,
    v_flex,
};

use crate::handlers;
use crate::screen::KeybindingsWindow;
use crate::utils::view_model::{ActionEntry, BindingView};

const ROW_HEIGHT: f32 = 60.0;
/// Shortcuts drawn in a row; the rest are behind "+N more".
const INLINE_CHIPS: usize = 2;

impl KeybindingsWindow {
    pub(crate) fn render_list(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let count = self.visible.len();
        if count == 0 {
            return self.render_empty(cx);
        }
        let handle = self.list_scroll.clone();
        let scrollbar = self.list_scrollbar.clone();
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .child(
                uniform_list(
                    "keybinding-list",
                    count,
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        range
                            .filter_map(|ix| {
                                let entry = this.entries.get(*this.visible.get(ix)?)?.clone();
                                Some(render_row(ix, entry, cx))
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .size_full()
                .track_scroll(&handle),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(Scrollbar::uniform_scroll(&scrollbar, &handle)),
            )
            .into_any_element()
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (icon, text) = if self.entries.is_empty() {
            (IconName::Key, "No actions are registered yet.")
        } else {
            (
                IconName::Search,
                "No action matches. Try another word or filter.",
            )
        };
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_2()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child(Icon::new(icon).size_8())
            .child(text)
            .into_any_element()
    }
}

fn render_row(ix: usize, entry: ActionEntry, cx: &mut Context<KeybindingsWindow>) -> AnyElement {
    let theme = cx.theme().clone();
    let name = entry.info.name.clone();
    let title = entry.info.title.clone();
    let conflicted_with = entry
        .bindings
        .iter()
        .find_map(|view| view.shadowed_by.clone());
    let subtitle = match (&conflicted_with, &entry.info.doc) {
        (Some(other), _) => Some((
            true,
            format!(
                "Shortcut taken by {}",
                crate::utils::registry::describe_name(other).1
            ),
        )),
        (None, Some(doc)) => Some((false, doc.lines().next().unwrap_or("").to_owned())),
        (None, None) => None,
    };

    h_flex()
        .id(("keybinding-row", ix))
        .w_full()
        .h(px(ROW_HEIGHT))
        .px_4()
        .gap_4()
        .items_center()
        .border_b_1()
        .border_color(theme.border.opacity(0.6))
        .hover(|row| row.bg(theme.list_hover.opacity(0.5)))
        .child(
            v_flex()
                .flex_1()
                .min_w(px(180.0))
                .gap_0p5()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .text_sm()
                                .font_medium()
                                .text_color(theme.foreground)
                                .truncate()
                                .child(title.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .px_1p5()
                                .rounded(theme.radius)
                                .bg(theme.muted.opacity(0.5))
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(entry.info.namespace.clone()),
                        )
                        .when(entry.customized, |row| {
                            row.child(
                                div()
                                    .flex_none()
                                    .size_1p5()
                                    .rounded_full()
                                    .bg(theme.primary),
                            )
                        }),
                )
                .child(
                    div()
                        .text_xs()
                        .truncate()
                        .text_color(match &subtitle {
                            Some((true, _)) => theme.warning,
                            _ => theme.muted_foreground,
                        })
                        .child(
                            subtitle
                                .map(|(_, text)| text)
                                .unwrap_or_else(|| name.clone()),
                        ),
                ),
        )
        .child(
            h_flex()
                .flex_none()
                .gap_2()
                .items_center()
                .when(entry.bindings.is_empty(), |row| {
                    row.child(
                        div()
                            .text_xs()
                            .italic()
                            .text_color(theme.muted_foreground)
                            .child("Unbound"),
                    )
                })
                .children(
                    entry
                        .bindings
                        .iter()
                        .take(INLINE_CHIPS)
                        .enumerate()
                        .map(|(chip_ix, view)| render_chip(ix, chip_ix, &entry, view, cx)),
                )
                .when(entry.bindings.len() > INLINE_CHIPS, |row| {
                    let name = name.clone();
                    row.child(
                        Button::new(("more-bindings", ix))
                            .label(format!("+{} more", entry.bindings.len() - INLINE_CHIPS))
                            .ghost()
                            .xsmall()
                            .tooltip("Show every shortcut of this action")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                handlers::show_details(this, name.clone(), cx);
                            })),
                    )
                })
                .child({
                    let (name, title) = (name.clone(), title.clone());
                    Button::new(("add-binding", ix))
                        .icon(IconName::Plus)
                        .ghost()
                        .xsmall()
                        .tooltip(if entry.info.bindable {
                            "Add a shortcut"
                        } else {
                            "This action needs parameters, so it cannot be bound here"
                        })
                        .opacity(if entry.info.bindable { 1.0 } else { 0.35 })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if this
                                .entries
                                .iter()
                                .any(|e| e.info.name == name && e.info.bindable)
                            {
                                handlers::start_recording(
                                    this,
                                    name.clone(),
                                    title.clone(),
                                    None,
                                    window,
                                    cx,
                                );
                            }
                        }))
                })
                .child({
                    let name = name.clone();
                    Button::new(("reset-binding", ix))
                        .icon(IconName::Undo)
                        .ghost()
                        .xsmall()
                        .tooltip("Restore the default")
                        .opacity(if entry.customized { 1.0 } else { 0.0 })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            handlers::reset_action(this, name.clone(), cx);
                        }))
                }),
        )
        .into_any_element()
}

/// One shortcut: click it to change it, the cross removes it.
pub(super) fn render_chip(
    row: usize,
    chip: usize,
    entry: &ActionEntry,
    view: &BindingView,
    cx: &mut Context<KeybindingsWindow>,
) -> AnyElement {
    let theme = cx.theme().clone();
    let binding = view.binding.clone();
    let (name, title) = (entry.info.name.clone(), entry.info.title.clone());
    let shadowed = view.shadowed_by.is_some();

    let strokes: Vec<AnyElement> = binding
        .keystrokes
        .split_whitespace()
        .filter_map(|stroke| Keystroke::parse(stroke).ok())
        .map(|stroke| Kbd::new(stroke).into_any_element())
        .collect();

    h_flex()
        .id(("keybinding-chip", row * 16 + chip))
        .h(px(28.0))
        .pl_1p5()
        .pr_0p5()
        .gap_1p5()
        .items_center()
        .rounded(theme.radius)
        .border_1()
        .border_color(if shadowed {
            theme.warning.opacity(0.6)
        } else {
            theme.border
        })
        .bg(if view.user {
            theme.primary.opacity(0.1)
        } else {
            theme.secondary.opacity(0.5)
        })
        .cursor_pointer()
        .hover(|chip| chip.border_color(theme.primary))
        .child(h_flex().gap_1().children(strokes))
        .when_some(binding.context.clone(), |chip, context| {
            chip.child(
                div()
                    .px_1()
                    .rounded(theme.radius)
                    .bg(theme.muted.opacity(0.6))
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(context),
            )
        })
        .when(shadowed, |chip| {
            chip.child(
                Icon::new(IconName::TriangleAlert)
                    .size_3p5()
                    .text_color(theme.warning),
            )
        })
        .on_click({
            let binding = binding.clone();
            cx.listener(move |this, _, window, cx| {
                handlers::start_recording(
                    this,
                    name.clone(),
                    title.clone(),
                    Some(binding.clone()),
                    window,
                    cx,
                );
            })
        })
        .child(
            Button::new(("remove-binding", row * 16 + chip))
                .icon(IconName::Close)
                .ghost()
                .xsmall()
                .tooltip("Remove this shortcut")
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    handlers::remove_binding(this, binding.clone(), cx);
                })),
        )
        .into_any_element()
}
