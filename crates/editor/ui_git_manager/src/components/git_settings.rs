use crate::git_hooks::GitHooksConfig;
use gpui::{
    AppContext as _, Context, Div, Entity, IntoElement, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};
use ui::{
    ActiveTheme as _, Disableable as _, IconName,
    alert::Alert,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{InputState, NumberInput, NumberInputEvent, TextInput},
    switch::Switch,
    v_flex,
};

use crate::{
    handlers::{
        add_hook, handle_auto_fetch_toggle, handle_hook_toggle, handle_interval_step, remove_hook,
    },
    utils::{AutoFetchSettings, GitSettingsLoadErrors},
};

pub(crate) struct HookRow {
    pub(crate) id: usize,
    pub(crate) enabled: bool,
    pub(crate) name_input: Entity<InputState>,
    pub(crate) script_input: Entity<InputState>,
}

pub(crate) struct GitSettingsForm {
    pub(crate) initial_identity: crate::utils::GitIdentity,
    pub(crate) name_input: Entity<InputState>,
    pub(crate) email_input: Entity<InputState>,
    pub(crate) auto_fetch: bool,
    pub(crate) interval_input: Entity<InputState>,
    pub(crate) hooks: Vec<HookRow>,
    pub(crate) next_hook_id: usize,
    pub(crate) load_errors: GitSettingsLoadErrors,
    pub(crate) _interval_subscription: Subscription,
}

impl GitSettingsForm {
    pub(crate) fn new(
        identity: crate::utils::GitIdentity,
        auto_fetch: AutoFetchSettings,
        hooks_config: GitHooksConfig,
        load_errors: GitSettingsLoadErrors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let initial_identity = identity.clone();
        let name_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Your name")
                .default_value(identity.name)
        });
        let email_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("you@example.com")
                .default_value(identity.email)
        });
        let interval_input = cx.new(|cx| {
            InputState::new(window, cx).default_value(auto_fetch.interval_minutes.to_string())
        });
        let interval_subscription = cx.subscribe_in(
            &interval_input,
            window,
            move |this, input, event: &NumberInputEvent, window, cx| {
                handle_interval_step(this, input, event, window, cx);
            },
        );

        let hooks = hooks_config
            .hooks
            .into_iter()
            .enumerate()
            .map(|(id, (name, definition))| {
                Self::new_hook_row(id, definition.enabled, name, definition.content, window, cx)
            })
            .collect::<Vec<_>>();
        let next_hook_id = hooks.len();

        Self {
            initial_identity,
            name_input,
            email_input,
            auto_fetch: auto_fetch.enabled,
            interval_input,
            hooks,
            next_hook_id,
            load_errors,
            _interval_subscription: interval_subscription,
        }
    }

    pub(crate) fn new_hook_row(
        id: usize,
        enabled: bool,
        name: String,
        content: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> HookRow {
        let name_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("pre-commit")
                .default_value(name)
        });
        let script_input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line()
                .auto_grow(3, 8)
                .placeholder("#!/bin/sh")
                .default_value(content)
        });

        HookRow {
            id,
            enabled,
            name_input,
            script_input,
        }
    }
}

impl Render for GitSettingsForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let form = cx.entity().clone();
        let auto_fetch = self.auto_fetch;
        let identity_error = self.load_errors.identity.clone();
        let auto_fetch_error = self.load_errors.auto_fetch.clone();
        let hooks_error = self.load_errors.hooks.clone();
        let identity_disabled = identity_error.is_some();
        let auto_fetch_disabled = auto_fetch_error.is_some();
        let hooks_disabled = hooks_error.is_some();
        let border = cx.theme().border;
        let viewport_height: f32 = window.viewport_size().height.into();
        let content_max_height = px((viewport_height - 210.0).clamp(240.0, 560.0));

        v_flex()
            .w_full()
            .max_h(content_max_height)
            .overflow_y_scroll()
            .pr_2()
            .gap_6()
            .child(
                settings_section("Identity")
                    .when_some(identity_error, |this, error| {
                        this.child(section_load_error("git-identity-load-error", error))
                    })
                    .child(
                        v_flex()
                            .w_full()
                            .gap_4()
                            .child(form_field(
                                "Name",
                                TextInput::new(&self.name_input)
                                    .w_full()
                                    .appearance(true)
                                    .bordered(true)
                                    .disabled(identity_disabled),
                            ))
                            .child(form_field(
                                "Email",
                                TextInput::new(&self.email_input)
                                    .w_full()
                                    .appearance(true)
                                    .bordered(true)
                                    .disabled(identity_disabled),
                            )),
                    ),
            )
            .child(
                settings_section("Background Fetch")
                    .when_some(auto_fetch_error, |this, error| {
                        this.child(section_load_error("git-auto-fetch-load-error", error))
                    })
                    .child(
                        v_flex()
                            .w_full()
                            .gap_4()
                            .child(
                                h_flex()
                                    .w_full()
                                    .justify_between()
                                    .gap_4()
                                    .child(field_label("Enabled"))
                                    .child(
                                        Switch::new("git-auto-fetch")
                                            .checked(auto_fetch)
                                            .disabled(auto_fetch_disabled)
                                            .on_click({
                                                let form = form.clone();
                                                let interval_input = self.interval_input.clone();
                                                move |checked, window, cx| {
                                                    form.update(cx, |this, cx| {
                                                        handle_auto_fetch_toggle(
                                                            this,
                                                            *checked,
                                                            &interval_input,
                                                            window,
                                                            cx,
                                                        );
                                                    });
                                                }
                                            }),
                                    ),
                            )
                            .child(form_field(
                                "Interval (minutes)",
                                NumberInput::new(&self.interval_input)
                                    .w_full()
                                    .disabled(auto_fetch_disabled || !auto_fetch),
                            )),
                    ),
            )
            .child(
                settings_section("Git Hooks")
                    .when_some(hooks_error, |this, error| {
                        this.child(section_load_error("git-hooks-load-error", error))
                    })
                    .children(self.hooks.iter().enumerate().map(|(index, hook)| {
                        let hook_id = hook.id;
                        let hook_enabled = hook.enabled;
                        let form_for_toggle = form.clone();
                        let form_for_remove = form.clone();

                        v_flex()
                            .w_full()
                            .gap_3()
                            .py_3()
                            .when(index > 0, |this| this.border_t_1().border_color(border))
                            .child(
                                h_flex()
                                    .w_full()
                                    .justify_between()
                                    .gap_4()
                                    .child(
                                        Switch::new(("git-hook-enabled", hook_id))
                                            .checked(hook_enabled)
                                            .label("Enabled")
                                            .disabled(hooks_disabled)
                                            .on_click(move |checked, _, cx| {
                                                form_for_toggle.update(cx, |this, cx| {
                                                    handle_hook_toggle(this, hook_id, *checked, cx);
                                                });
                                            }),
                                    )
                                    .child(
                                        Button::new(("remove-git-hook", hook_id))
                                            .ghost()
                                            .compact()
                                            .icon(IconName::Trash)
                                            .tooltip("Remove hook")
                                            .disabled(hooks_disabled)
                                            .on_click(move |_, _, cx| {
                                                form_for_remove.update(cx, |this, cx| {
                                                    remove_hook(this, hook_id, cx);
                                                });
                                            }),
                                    ),
                            )
                            .child(form_field(
                                "Hook name",
                                TextInput::new(&hook.name_input)
                                    .w_full()
                                    .appearance(true)
                                    .bordered(true)
                                    .disabled(hooks_disabled),
                            ))
                            .child(form_field(
                                "Script",
                                TextInput::new(&hook.script_input)
                                    .w_full()
                                    .appearance(true)
                                    .bordered(true)
                                    .disabled(hooks_disabled),
                            ))
                    }))
                    .when(self.hooks.is_empty(), |this| {
                        this.child(
                            div()
                                .py_3()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("No hooks configured."),
                        )
                    })
                    .child(
                        Button::new("add-git-hook")
                            .icon(IconName::Plus)
                            .label("Add Hook")
                            .disabled(hooks_disabled)
                            .on_click({
                                let form = form.clone();
                                move |_, window, cx| {
                                    form.update(cx, |this, cx| add_hook(this, window, cx));
                                }
                            }),
                    ),
            )
    }
}

fn section_load_error(id: &'static str, error: String) -> Alert {
    Alert::error(
        id,
        format!(
            "Could not load existing settings: {error}\nThis section is read-only and will not be saved."
        ),
    )
    .title("Section protected")
}

fn settings_section(label: &'static str) -> Div {
    v_flex().w_full().gap_3().child(
        div()
            .text_base()
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .child(label),
    )
}

fn form_field(label: &'static str, input: impl IntoElement) -> Div {
    v_flex()
        .w_full()
        .gap_2()
        .child(field_label(label))
        .child(input)
}

fn field_label(label: &'static str) -> Div {
    div()
        .text_sm()
        .font_weight(gpui::FontWeight::MEDIUM)
        .child(label)
}
