use git2::{Config, ErrorCode};
use gpui::{
    App, AppContext as _, Entity, IntoElement, ParentElement as _, Styled as _, Window, div, px,
};
use ui::{
    ContextModal as _,
    input::{InputState, TextInput},
    modal::ModalButtonProps,
    notification::Notification,
    v_flex,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct GitIdentity {
    name: String,
    email: String,
}

impl GitIdentity {
    fn from_input(name: &str, email: &str) -> Result<Self, String> {
        let name = name.trim();
        let email = email.trim();

        if name.is_empty() || email.is_empty() {
            return Err("Git name and email are both required.".to_string());
        }

        Ok(Self {
            name: name.to_string(),
            email: email.to_string(),
        })
    }
}

pub fn open_git_identity_modal(window: &mut Window, cx: &mut App) {
    let identity = match load_global_git_identity() {
        Ok(identity) => identity,
        Err(error) => {
            window.push_notification(
                Notification::error(error).title("Git identity could not be loaded"),
                cx,
            );
            GitIdentity::default()
        }
    };

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

    window.open_modal(cx, move |modal, _window, _cx| {
        let name_input_for_save = name_input.clone();
        let email_input_for_save = email_input.clone();

        modal
            .width(px(480.0))
            .title("Global Git Identity")
            .confirm()
            .button_props(ModalButtonProps::default().ok_text("Save"))
            .on_ok(move |_, window, cx| {
                let name = name_input_for_save.read(cx).value().to_string();
                let email = email_input_for_save.read(cx).value().to_string();
                let result = GitIdentity::from_input(&name, &email)
                    .and_then(|identity| save_global_git_identity(&identity));

                match result {
                    Ok(()) => {
                        window.push_notification(Notification::success("Git identity saved"), cx);
                        true
                    }
                    Err(error) => {
                        window.push_notification(
                            Notification::error(error).title("Git identity was not saved"),
                            cx,
                        );
                        false
                    }
                }
            })
            .child(
                v_flex()
                    .w_full()
                    .gap_4()
                    .child(form_field("Name", &name_input))
                    .child(form_field("Email", &email_input)),
            )
    });
}

fn form_field(label: &'static str, input: &Entity<InputState>) -> impl IntoElement {
    v_flex()
        .w_full()
        .gap_2()
        .child(
            div()
                .text_sm()
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(label),
        )
        .child(
            TextInput::new(input)
                .w_full()
                .appearance(true)
                .bordered(true),
        )
}

fn load_global_git_identity() -> Result<GitIdentity, String> {
    let mut config = match Config::open_default() {
        Ok(config) => config,
        Err(error) if error.code() == ErrorCode::NotFound => return Ok(GitIdentity::default()),
        Err(error) => return Err(format!("Failed to open Git configuration: {error}")),
    };
    let global = match config.open_global() {
        Ok(global) => global,
        Err(error) if error.code() == ErrorCode::NotFound => return Ok(GitIdentity::default()),
        Err(error) => return Err(format!("Failed to open global Git configuration: {error}")),
    };

    load_git_identity(&global)
}

fn save_global_git_identity(identity: &GitIdentity) -> Result<(), String> {
    let mut config = Config::open_default()
        .map_err(|error| format!("Failed to open Git configuration: {error}"))?;
    let mut global = config
        .open_global()
        .map_err(|error| format!("Failed to open global Git configuration: {error}"))?;

    save_git_identity(&mut global, identity)
}

fn load_git_identity(config: &Config) -> Result<GitIdentity, String> {
    Ok(GitIdentity {
        name: read_config_value(config, "user.name")?,
        email: read_config_value(config, "user.email")?,
    })
}

fn read_config_value(config: &Config, key: &str) -> Result<String, String> {
    match config.get_string(key) {
        Ok(value) => Ok(value),
        Err(error) if error.code() == ErrorCode::NotFound => Ok(String::new()),
        Err(error) => Err(format!("Failed to read {key}: {error}")),
    }
}

fn save_git_identity(config: &mut Config, identity: &GitIdentity) -> Result<(), String> {
    config
        .set_str("user.name", &identity.name)
        .map_err(|error| format!("Failed to save user.name: {error}"))?;
    config
        .set_str("user.email", &identity.email)
        .map_err(|error| format!("Failed to save user.email: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use super::*;
    use git2::ConfigLevel;

    fn temporary_config() -> (tempfile::TempDir, PathBuf, Config) {
        let directory = tempfile::tempdir().expect("create temporary directory");
        let path = directory.path().join("gitconfig");
        fs::write(&path, "").expect("create temporary Git config");
        let config = Config::open(&path).expect("open temporary Git config");
        (directory, path, config)
    }

    #[test]
    fn normalizes_identity_input() {
        let identity =
            GitIdentity::from_input("  Jane Doe  ", " jane@example.com\n").expect("valid identity");

        assert_eq!(
            identity,
            GitIdentity {
                name: "Jane Doe".to_string(),
                email: "jane@example.com".to_string(),
            }
        );
    }

    #[test]
    fn rejects_blank_identity_fields() {
        assert!(GitIdentity::from_input("", "jane@example.com").is_err());
        assert!(GitIdentity::from_input("Jane Doe", "  ").is_err());
    }

    #[test]
    fn missing_config_values_load_as_empty_fields() {
        let (_directory, _path, config) = temporary_config();

        assert_eq!(
            load_git_identity(&config).expect("load empty identity"),
            GitIdentity::default()
        );
    }

    #[test]
    fn partial_config_keeps_missing_field_empty() {
        let (_directory, _path, mut config) = temporary_config();
        config
            .set_str("user.name", "Jane Doe")
            .expect("write Git name");

        assert_eq!(
            load_git_identity(&config).expect("load partial identity"),
            GitIdentity {
                name: "Jane Doe".to_string(),
                email: String::new(),
            }
        );
    }

    #[test]
    fn saves_reloads_and_overwrites_identity() {
        let (_directory, path, mut config) = temporary_config();
        let first = GitIdentity {
            name: "Jane Doe".to_string(),
            email: "jane@example.com".to_string(),
        };
        let second = GitIdentity {
            name: "Janet Doe".to_string(),
            email: "janet@example.com".to_string(),
        };

        save_git_identity(&mut config, &first).expect("save first identity");
        save_git_identity(&mut config, &second).expect("overwrite identity");
        drop(config);

        let config = Config::open(&path).expect("reopen temporary Git config");
        assert_eq!(load_git_identity(&config).expect("reload identity"), second);
    }

    #[test]
    fn save_creates_config_file_when_missing() {
        let directory = tempfile::tempdir().expect("create temporary directory");
        let path = directory.path().join("gitconfig");
        let mut config = Config::new().expect("create Git config");
        config
            .add_file(&path, ConfigLevel::Global, false)
            .expect("add missing Git config file");
        let identity = GitIdentity {
            name: "Jane Doe".to_string(),
            email: "jane@example.com".to_string(),
        };

        save_git_identity(&mut config, &identity).expect("save identity");
        drop(config);

        let config = Config::open(&path).expect("open created Git config");
        assert_eq!(
            load_git_identity(&config).expect("reload identity"),
            identity
        );
    }
}
