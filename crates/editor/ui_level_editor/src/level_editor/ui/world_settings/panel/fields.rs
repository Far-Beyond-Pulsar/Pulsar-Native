use super::*;

impl WorldSettingsPanelImpl {

    // Field rendering helpers

    pub(super) fn render_f32_field(
        &self,
        input: &Entity<InputState>,
        element_id: &str,
        label: &str,
        unit: &str,
        cx: &Context<WorldSettingsPanel>,
    ) -> impl IntoElement {
        let can_edit = input.can_edit_replicated(cx);

        // Get presence info
        let registry = ReplicationRegistry::global(cx);
        let locked_by = registry
            .get_element_state(element_id)
            .and_then(|state| state.locked_by.clone())
            .and_then(|peer_id| registry.get_user_presence(&peer_id));

        h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .child(
                div()
                    .w_1_3()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(label.to_string()),
            )
            .child(
                h_flex()
                    .flex_1()
                    .gap_1()
                    .items_center()
                    .child(
                        NumberInput::new(input)
                            .xsmall()
                            .when(!can_edit, |this| this.disabled(true)),
                    )
                    .when(!unit.is_empty(), |flex| {
                        flex.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(unit.to_string()),
                        )
                    })
                    .when_some(locked_by, |flex, user| {
                        flex.child(FieldPresenceIndicator::new(user).locked(true))
                    }),
            )
    }

    pub(super) fn render_bool_field(
        &self,
        _element_id: &str,
        label: &str,
        value: bool,
        setter: fn(&mut WorldSettingsData, bool),
        cx: &Context<WorldSettingsPanel>,
    ) -> impl IntoElement {
        let settings = self.settings.clone();

        h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .child(
                div()
                    .w_1_3()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(label.to_string()),
            )
            .child(
                div()
                    .w_9()
                    .h_5()
                    .rounded_full()
                    .bg(if value {
                        cx.theme().accent
                    } else {
                        cx.theme().muted
                    })
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |_this, _event, _window, cx| {
                            let mut s = settings.write();
                            setter(&mut s, !value);
                            s.apply();
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .size_4()
                            .mt(px(2.0))
                            .ml(if value { px(18.0) } else { px(2.0) })
                            .rounded_full()
                            .bg(white())
                            .shadow_sm(),
                    ),
            )
    }

    pub(super) fn render_vector3_display(
        &self,
        label: &str,
        values: [f32; 3],
        cx: &Context<WorldSettingsPanel>,
    ) -> impl IntoElement {
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(label.to_string()),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(Self::render_axis_display(
                        "X",
                        Hsla {
                            h: 0.0,
                            s: 0.8,
                            l: 0.5,
                            a: 1.0,
                        },
                        values[0],
                        cx,
                    ))
                    .child(Self::render_axis_display(
                        "Y",
                        Hsla {
                            h: 50.0,
                            s: 0.9,
                            l: 0.5,
                            a: 1.0,
                        },
                        values[1],
                        cx,
                    ))
                    .child(Self::render_axis_display(
                        "Z",
                        Hsla {
                            h: 220.0,
                            s: 0.8,
                            l: 0.55,
                            a: 1.0,
                        },
                        values[2],
                        cx,
                    )),
            )
    }

    pub(super) fn render_axis_display(
        axis: &str,
        axis_color: Hsla,
        value: f32,
        cx: &Context<WorldSettingsPanel>,
    ) -> impl IntoElement {
        h_flex()
            .flex_1()
            .h_7()
            .items_center()
            .rounded(px(4.0))
            .border_1()
            .border_color(cx.theme().border)
            .overflow_hidden()
            .child(
                div()
                    .w_6()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(axis_color.opacity(0.2))
                    .border_r_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(axis_color)
                            .child(axis.to_string()),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .px_2()
                    .bg(cx.theme().input)
                    .text_xs()
                    .text_color(cx.theme().foreground)
                    .child(format!("{:.2}", value)),
            )
    }
}
