//! Every shortcut of one action, for the actions with more than a row holds.

use gpui::{prelude::*, *};
use ui::{
    ActiveTheme as _, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};

use super::list::render_chip;
use crate::handlers;
use crate::screen::KeybindingsWindow;

/// Row index used for the chip ids in this dialog, apart from the list's own.
const CHIP_ROW: usize = 1 << 20;

impl KeybindingsWindow {
    pub(crate) fn render_details(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(entry) = self
            .details
            .as_ref()
            .and_then(|name| self.entries.iter().find(|entry| &entry.info.name == name))
            .cloned()
        else {
            return div().into_any_element();
        };
        let (name, title) = (entry.info.name.clone(), entry.info.title.clone());
        let bindable = entry.info.bindable;

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(0.5))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| handlers::close_details(this, cx)),
            )
            .child(
                v_flex()
                    .w(px(620.0))
                    .max_h(px(560.0))
                    .gap_3()
                    .p_5()
                    .bg(theme.popover)
                    .text_color(theme.popover_foreground)
                    .border_1()
                    .border_color(theme.border)
                    .rounded(theme.radius_lg)
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(div().text_base().font_semibold().child(title.clone()))
                            .child(div().text_xs().text_color(theme.muted_foreground).child(
                                format!(
                                    "{}  ·  {} shortcuts",
                                    entry.info.name,
                                    entry.bindings.len()
                                ),
                            )),
                    )
                    .child(
                        v_flex()
                            .id("keybinding-details-list")
                            .gap_2()
                            .min_h_0()
                            .overflow_y_scroll()
                            .children(entry.bindings.iter().enumerate().map(|(ix, view)| {
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(render_chip(CHIP_ROW, ix, &entry, view, cx))
                                    .when_some(view.shadowed_by.clone(), |row, other| {
                                        row.child(div().text_xs().text_color(theme.warning).child(
                                            format!(
                                                "never fires: taken by {}",
                                                crate::utils::registry::describe_name(&other).1
                                            ),
                                        ))
                                    })
                            })),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("details-add")
                                    .label("Add shortcut")
                                    .small()
                                    .when(!bindable, |button| button.opacity(0.4))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if bindable {
                                            handlers::start_recording(
                                                this,
                                                name.clone(),
                                                title.clone(),
                                                None,
                                                window,
                                                cx,
                                            );
                                        }
                                    })),
                            )
                            .child(
                                Button::new("details-close")
                                    .label("Done")
                                    .primary()
                                    .small()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        handlers::close_details(this, cx)
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }
}
