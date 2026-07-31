use crate::git_hooks::load_git_hooks_config;
use engine_state::GlobalSettings;
use gpui::{
    App, AppContext as _, Context, Entity, ParentElement as _, Window, prelude::FluentBuilder as _,
    px,
};
use ui::{
    ContextModal as _,
    input::{InputState, NumberInputEvent, StepAction},
    modal::ModalButtonProps,
    notification::Notification,
};

use crate::{
    components::GitSettingsForm,
    utils::{
        AutoFetchSettings, DEFAULT_AUTO_FETCH_INTERVAL_MINUTES, GitIdentity, GitSettingsDraft,
        GitSettingsLoadErrors, HookDraft, MAX_AUTO_FETCH_INTERVAL_MINUTES,
        MIN_AUTO_FETCH_INTERVAL_MINUTES, SOURCE_CONTROL_OWNER, build_hooks_config,
        load_auto_fetch_settings, load_global_git_identity, save_git_settings,
        validate_auto_fetch_interval,
    },
};

pub(crate) fn handle_interval_step(
    _form: &mut GitSettingsForm,
    input: &Entity<InputState>,
    event: &NumberInputEvent,
    window: &mut Window,
    cx: &mut Context<GitSettingsForm>,
) {
    let NumberInputEvent::Step { action, .. } = event;
    input.update(cx, |input, cx| {
        let current = input
            .value()
            .trim()
            .parse::<i64>()
            .unwrap_or(DEFAULT_AUTO_FETCH_INTERVAL_MINUTES);
        let next = match action {
            StepAction::Increment => current.saturating_add(1),
            StepAction::Decrement => current.saturating_sub(1),
        }
        .clamp(
            MIN_AUTO_FETCH_INTERVAL_MINUTES,
            MAX_AUTO_FETCH_INTERVAL_MINUTES,
        );
        input.set_value(next.to_string(), window, cx);
    });
}

pub(crate) fn handle_auto_fetch_toggle(
    form: &mut GitSettingsForm,
    checked: bool,
    interval_input: &Entity<InputState>,
    window: &mut Window,
    cx: &mut Context<GitSettingsForm>,
) {
    if !checked {
        let interval = interval_input.read(cx).value().to_string();
        if validate_auto_fetch_interval(&interval).is_err() {
            interval_input.update(cx, |input, cx| {
                input.set_value(DEFAULT_AUTO_FETCH_INTERVAL_MINUTES.to_string(), window, cx);
            });
        }
    }
    form.auto_fetch = checked;
    cx.notify();
}

pub(crate) fn add_hook(
    form: &mut GitSettingsForm,
    window: &mut Window,
    cx: &mut Context<GitSettingsForm>,
) {
    let id = form.next_hook_id;
    form.next_hook_id += 1;
    form.hooks.push(GitSettingsForm::new_hook_row(
        id,
        true,
        String::new(),
        String::new(),
        window,
        cx,
    ));
    cx.notify();
}

pub(crate) fn remove_hook(
    form: &mut GitSettingsForm,
    id: usize,
    cx: &mut Context<GitSettingsForm>,
) {
    form.hooks.retain(|hook| hook.id != id);
    cx.notify();
}

pub(crate) fn handle_hook_toggle(
    form: &mut GitSettingsForm,
    id: usize,
    checked: bool,
    cx: &mut Context<GitSettingsForm>,
) {
    if let Some(hook) = form.hooks.iter_mut().find(|hook| hook.id == id) {
        hook.enabled = checked;
        cx.notify();
    }
}

fn validated_values(form: &GitSettingsForm, cx: &App) -> (GitSettingsDraft, Vec<String>) {
    let mut settings = GitSettingsDraft::default();
    let mut errors = Vec::new();

    if let Some(error) = &form.load_errors.identity {
        errors.push(protected_section_error("Identity", error));
    } else {
        let name = form.name_input.read(cx).value().to_string();
        let email = form.email_input.read(cx).value().to_string();
        match GitIdentity::update_from_input(&form.initial_identity, &name, &email) {
            Ok(identity) => settings.identity = identity,
            Err(error) => errors.push(format!("Identity: {error}")),
        }
    }

    if let Some(error) = &form.load_errors.auto_fetch {
        errors.push(protected_section_error("Background Fetch", error));
    } else {
        let interval = form.interval_input.read(cx).value().to_string();
        match validate_auto_fetch_interval(&interval) {
            Ok(interval_minutes) => {
                settings.auto_fetch = Some(AutoFetchSettings {
                    enabled: form.auto_fetch,
                    interval_minutes,
                });
            }
            Err(error) => errors.push(format!("Background Fetch: {error}")),
        }
    }

    if let Some(error) = &form.load_errors.hooks {
        errors.push(protected_section_error("Git Hooks", error));
    } else {
        match build_hooks_config(form.hooks.iter().map(|hook| HookDraft {
            enabled: hook.enabled,
            name: hook.name_input.read(cx).value().to_string(),
            content: hook.script_input.read(cx).value().to_string(),
        })) {
            Ok(hooks) => settings.hooks = Some(hooks),
            Err(error) => errors.push(format!("Git Hooks: {error}")),
        }
    }

    (settings, errors)
}

pub fn open_git_settings_modal(window: &mut Window, cx: &mut App) {
    let (identity, identity_error) = load_with_fallback(
        load_global_git_identity(),
        "Git identity could not be loaded",
        window,
        cx,
    );
    let (auto_fetch, auto_fetch_error) = load_with_fallback(
        GlobalSettings::new()
            .validate_owner_file(SOURCE_CONTROL_OWNER)
            .map_err(|error| format!("Failed to read source control settings: {error}"))
            .and_then(|_| load_auto_fetch_settings()),
        "Background fetch settings could not be loaded",
        window,
        cx,
    );
    let (hooks, hooks_error) = load_with_fallback(
        load_git_hooks_config()
            .map_err(|error| format!("Failed to load Git hooks configuration: {error}"))
            .and_then(|config| {
                config
                    .validate()
                    .map_err(|error| format!("Invalid Git hooks configuration: {error}"))?;
                Ok(config)
            }),
        "Git hooks could not be loaded",
        window,
        cx,
    );
    let load_errors = GitSettingsLoadErrors {
        identity: identity_error,
        auto_fetch: auto_fetch_error,
        hooks: hooks_error,
    };
    let has_savable_section = !load_errors.all_sections_failed();
    let form =
        cx.new(|cx| GitSettingsForm::new(identity, auto_fetch, hooks, load_errors, window, cx));

    window.open_modal(cx, move |modal, _window, _cx| {
        let form_for_save = form.clone();

        modal
            .width(px(600.0))
            .title("Global Git Settings")
            .confirm()
            .when(!has_savable_section, |modal| {
                modal.footer(|_, cancel, window, cx| vec![cancel(window, cx)])
            })
            .button_props(ModalButtonProps::default().ok_text("Save"))
            .when(has_savable_section, |modal| {
                modal.on_ok(move |_, window, cx| {
                    let (settings, mut errors) = validated_values(form_for_save.read(cx), cx);
                    let report = save_git_settings(&settings);
                    errors.extend(report.errors.iter().cloned());

                    if errors.is_empty() {
                        window.push_notification(
                            Notification::success("Global Git settings saved"),
                            cx,
                        );
                        return true;
                    }

                    let message = format_save_outcome(&report.saved_sections, &errors);
                    let notification = if report.saved_sections.is_empty() {
                        Notification::error(message).title("Global Git settings were not saved")
                    } else {
                        Notification::warning(message)
                            .title("Global Git settings were only partially saved")
                    };
                    window.push_notification(notification, cx);
                    false
                })
            })
            .child(form.clone())
    });
}

fn protected_section_error(section: &str, error: &str) -> String {
    format!("{section}: existing settings could not be loaded and were protected ({error})")
}

fn format_save_outcome(saved_sections: &[&str], errors: &[String]) -> String {
    let errors = errors
        .iter()
        .map(|error| format!("- {error}"))
        .collect::<Vec<_>>()
        .join("\n");
    if saved_sections.is_empty() {
        format!("No settings were saved:\n{errors}")
    } else {
        format!(
            "Saved: {}.\nNot saved:\n{errors}",
            saved_sections.join(", ")
        )
    }
}

fn load_with_fallback<T: Default>(
    result: Result<T, String>,
    title: &'static str,
    window: &mut Window,
    cx: &mut App,
) -> (T, Option<String>) {
    match result {
        Ok(value) => (value, None),
        Err(error) => {
            window.push_notification(Notification::error(error.clone()).title(title), cx);
            (T::default(), Some(error))
        }
    }
}
