//! The dialog that captures a new shortcut.

use std::time::Duration;

use gpui::{Animation, AnimationExt as _, bounce, ease_in_out, prelude::*, *};
use ui::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::TextInput,
    kbd::Kbd,
    v_flex,
};

use crate::handlers;
use crate::screen::KeybindingsWindow;
use crate::utils::overrides::{self, ClashKind};
use crate::utils::registry;
use crate::utils::store::KeybindingStore;

impl KeybindingsWindow {
    pub(crate) fn render_recorder(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(recorder) = &self.recorder else {
            return div().into_any_element();
        };
        let title = recorder.action_title.clone();
        let changing = recorder.replacing.is_some();
        let keys = recorder.keys.clone();
        let error = recorder.error.clone();
        let action_name = recorder.action_name.clone();
        let has_keys = !keys.is_empty();

        // Who else holds these keys, in the context typed below.
        let context = registry::normalize_context(&self.context_input.read(cx).value())
            .ok()
            .flatten();
        let clash_lines: Vec<(bool, String)> = if has_keys {
            let project = Self::project();
            let user = KeybindingStore::for_project(project.as_deref(), cx)
                .overrides
                .clone();
            let live = overrides::effective_with(&registry::defaults(cx), &user);
            overrides::find_clashes(&live, &keys.join(" "), &context, Some(&action_name))
                .into_iter()
                .map(|clash| {
                    let other = registry::describe_name(&clash.binding.action).1;
                    match clash.kind {
                        ClashKind::Same => (
                            true,
                            format!("{other} already uses this. Saving takes it from {other}."),
                        ),
                        ClashKind::StartsChord => (
                            false,
                            format!(
                                "{other} uses {}, which starts with these keys, so this shortcut will wait for more keys before it runs.",
                                clash.binding.keystrokes
                            ),
                        ),
                        ClashKind::ExtendsBinding => (
                            false,
                            format!(
                                "{other} uses {}, the first part of this chord, so this chord can never complete.",
                                clash.binding.keystrokes
                            ),
                        ),
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        let takes_over = clash_lines.iter().any(|(takes, _)| *takes);

        let capture: AnyElement = if has_keys {
            h_flex()
                .gap_2()
                .items_center()
                .justify_center()
                .children(keys.iter().enumerate().flat_map(|(ix, key)| {
                    let mut parts: Vec<AnyElement> = Vec::new();
                    if ix > 0 {
                        parts.push(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("then")
                                .into_any_element(),
                        );
                    }
                    if let Ok(stroke) = Keystroke::parse(key) {
                        parts.push(Kbd::new(stroke).into_any_element());
                    }
                    parts
                }))
                .into_any_element()
        } else {
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("Press the keys you want to use")
                .with_animation(
                    "recorder-prompt",
                    Animation::new(Duration::from_secs(2))
                        .repeat()
                        .with_easing(bounce(ease_in_out)),
                    |this, delta| this.opacity(1.0 - delta * 0.5),
                )
                .into_any_element()
        };

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(0.5))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| handlers::close_recorder(this, cx)),
            )
            .child(
                v_flex()
                    .w(px(540.0))
                    .gap_4()
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
                            .child(
                                div()
                                    .text_base()
                                    .font_semibold()
                                    .child(if changing { "Change shortcut" } else { "Add shortcut" }),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child(title),
                            ),
                    )
                    .child(
                        div()
                            .h(px(76.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(theme.radius)
                            .border_2()
                            .border_color(if error.is_some() {
                                theme.danger
                            } else {
                                theme.primary.opacity(0.6)
                            })
                            .bg(theme.background)
                            .child(capture),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("Esc cancels  ·  Enter saves  ·  Backspace removes the last key  ·  Several keys in a row make a chord"),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().text_sm().font_medium().child("Where it applies"))
                            .child(TextInput::new(&self.context_input).w_full().small())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child("Leave empty to work everywhere, or name a context such as FileManager."),
                            ),
                    )
                    .children(error.map(|message| {
                        div()
                            .text_sm()
                            .text_color(theme.danger)
                            .child(message)
                    }))
                    .children(clash_lines.into_iter().map(|(takes, text)| {
                        div()
                            .text_sm()
                            .text_color(if takes { theme.warning } else { theme.muted_foreground })
                            .child(text)
                    }))
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("recorder-cancel")
                                    .label("Cancel")
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        handlers::close_recorder(this, cx)
                                    })),
                            )
                            .child(
                                Button::new("recorder-clear")
                                    .label("Clear")
                                    .ghost()
                                    .disabled(!has_keys)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        handlers::clear_recording(this, cx)
                                    })),
                            )
                            .child(
                                Button::new("recorder-save")
                                    .label(if takes_over { "Save and take over" } else { "Save" })
                                    .primary()
                                    .disabled(!has_keys)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        handlers::save_recording(this, window, cx)
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }
}
