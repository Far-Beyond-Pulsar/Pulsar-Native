use std::collections::BTreeMap;

use crate::git_hooks::{
    GIT_HOOKS_CONFIG_VERSION, GitHookDefinition, GitHooksConfig, save_git_hooks_config,
    validate_git_hook_name,
};
use engine_state::{ConfigValue, GlobalSettings, NS_EDITOR, global_config};
use git2::{Config, ErrorCode};

pub(crate) const SOURCE_CONTROL_OWNER: &str = "source_control";
const AUTO_FETCH_KEY: &str = "auto_fetch";
const AUTO_FETCH_INTERVAL_KEY: &str = "auto_fetch_interval_minutes";
const DEFAULT_AUTO_FETCH: bool = true;
pub(crate) const DEFAULT_AUTO_FETCH_INTERVAL_MINUTES: i64 = 5;
pub(crate) const MIN_AUTO_FETCH_INTERVAL_MINUTES: i64 = 1;
pub(crate) const MAX_AUTO_FETCH_INTERVAL_MINUTES: i64 = 60;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GitIdentity {
    pub(crate) name: String,
    pub(crate) email: String,
}

impl GitIdentity {
    pub(crate) fn from_input(name: &str, email: &str) -> Result<Self, String> {
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

    pub(crate) fn update_from_input(
        initial: &Self,
        name: &str,
        email: &str,
    ) -> Result<Option<Self>, String> {
        let name = name.trim();
        let email = email.trim();
        if name == initial.name.trim() && email == initial.email.trim() {
            return Ok(None);
        }

        Self::from_input(name, email).map(Some)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AutoFetchSettings {
    pub(crate) enabled: bool,
    pub(crate) interval_minutes: i64,
}

impl Default for AutoFetchSettings {
    fn default() -> Self {
        Self {
            enabled: DEFAULT_AUTO_FETCH,
            interval_minutes: DEFAULT_AUTO_FETCH_INTERVAL_MINUTES,
        }
    }
}

pub(crate) struct HookDraft {
    pub(crate) enabled: bool,
    pub(crate) name: String,
    pub(crate) content: String,
}

#[derive(Default)]
pub(crate) struct GitSettingsDraft {
    pub(crate) identity: Option<GitIdentity>,
    pub(crate) auto_fetch: Option<AutoFetchSettings>,
    pub(crate) hooks: Option<GitHooksConfig>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GitSettingsLoadErrors {
    pub(crate) identity: Option<String>,
    pub(crate) auto_fetch: Option<String>,
    pub(crate) hooks: Option<String>,
}

impl GitSettingsLoadErrors {
    pub(crate) fn all_sections_failed(&self) -> bool {
        self.identity.is_some() && self.auto_fetch.is_some() && self.hooks.is_some()
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct GitSettingsSaveReport {
    pub(crate) saved_sections: Vec<&'static str>,
    pub(crate) errors: Vec<String>,
}

pub(crate) fn validate_auto_fetch_interval(raw: &str) -> Result<i64, String> {
    let raw = raw.trim();
    let interval = raw.parse::<i64>().map_err(|_| {
        format!(
            "Background fetch interval must be a whole number from {MIN_AUTO_FETCH_INTERVAL_MINUTES} to {MAX_AUTO_FETCH_INTERVAL_MINUTES}."
        )
    })?;

    if !(MIN_AUTO_FETCH_INTERVAL_MINUTES..=MAX_AUTO_FETCH_INTERVAL_MINUTES).contains(&interval) {
        return Err(format!(
            "Background fetch interval must be from {MIN_AUTO_FETCH_INTERVAL_MINUTES} to {MAX_AUTO_FETCH_INTERVAL_MINUTES} minutes."
        ));
    }

    Ok(interval)
}

fn validate_hook_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    let invalid_name =
        || "Git hook name must be a single, non-empty lowercase ASCII filename.".to_string();
    if !name.is_ascii() || name.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Err(invalid_name());
    }
    validate_git_hook_name(name).map_err(|_| invalid_name())?;
    Ok(name.to_string())
}

pub(crate) fn build_hooks_config(
    rows: impl IntoIterator<Item = HookDraft>,
) -> Result<GitHooksConfig, String> {
    let mut hooks = BTreeMap::new();

    for row in rows {
        if !row.enabled && row.name.trim().is_empty() && row.content.trim().is_empty() {
            continue;
        }

        let name = validate_hook_name(&row.name)?;
        if hooks.contains_key(&name) {
            return Err(format!("Git hook name '{name}' is duplicated."));
        }

        hooks.insert(
            name,
            GitHookDefinition {
                enabled: row.enabled,
                content: row.content,
            },
        );
    }

    let config = GitHooksConfig {
        version: GIT_HOOKS_CONFIG_VERSION,
        hooks,
    };
    config
        .validate()
        .map_err(|error| format!("Invalid Git hooks configuration: {error}"))?;
    Ok(config)
}

pub(crate) fn load_auto_fetch_settings() -> Result<AutoFetchSettings, String> {
    let enabled = match global_config()
        .get(NS_EDITOR, SOURCE_CONTROL_OWNER, AUTO_FETCH_KEY)
        .map_err(|error| format!("Failed to read {AUTO_FETCH_KEY}: {error}"))?
    {
        ConfigValue::Bool(value) => value,
        value => {
            return Err(format!(
                "Expected {AUTO_FETCH_KEY} to be a boolean, found {value:?}."
            ));
        }
    };
    let interval_minutes = match global_config()
        .get(NS_EDITOR, SOURCE_CONTROL_OWNER, AUTO_FETCH_INTERVAL_KEY)
        .map_err(|error| format!("Failed to read {AUTO_FETCH_INTERVAL_KEY}: {error}"))?
    {
        ConfigValue::Int(value) => value,
        value => {
            return Err(format!(
                "Expected {AUTO_FETCH_INTERVAL_KEY} to be an integer, found {value:?}."
            ));
        }
    };

    validate_auto_fetch_interval(&interval_minutes.to_string())?;

    Ok(AutoFetchSettings {
        enabled,
        interval_minutes,
    })
}

fn save_auto_fetch_settings(settings: AutoFetchSettings) -> Result<(), String> {
    let handle = global_config()
        .owner_handle(NS_EDITOR, SOURCE_CONTROL_OWNER)
        .ok_or_else(|| "Source control settings are not registered.".to_string())?;
    handle
        .set(AUTO_FETCH_KEY, ConfigValue::Bool(settings.enabled))
        .map_err(|error| format!("Failed to update {AUTO_FETCH_KEY}: {error}"))?;
    handle
        .set(
            AUTO_FETCH_INTERVAL_KEY,
            ConfigValue::Int(settings.interval_minutes),
        )
        .map_err(|error| format!("Failed to update {AUTO_FETCH_INTERVAL_KEY}: {error}"))?;
    GlobalSettings::new()
        .save_owner_keys(
            SOURCE_CONTROL_OWNER,
            &[AUTO_FETCH_KEY, AUTO_FETCH_INTERVAL_KEY],
        )
        .map_err(|error| format!("Failed to save background fetch settings: {error}"))
}

pub(crate) fn save_git_settings(settings: &GitSettingsDraft) -> GitSettingsSaveReport {
    save_git_settings_with(
        settings,
        save_global_git_identity,
        save_auto_fetch_settings,
        |hooks| {
            save_git_hooks_config(hooks)
                .map_err(|error| format!("Failed to save Git hooks configuration: {error}"))
        },
    )
}

fn save_git_settings_with(
    settings: &GitSettingsDraft,
    mut save_identity: impl FnMut(&GitIdentity) -> Result<(), String>,
    mut save_auto_fetch: impl FnMut(AutoFetchSettings) -> Result<(), String>,
    mut save_hooks: impl FnMut(&GitHooksConfig) -> Result<(), String>,
) -> GitSettingsSaveReport {
    let mut report = GitSettingsSaveReport::default();

    if let Some(identity) = &settings.identity {
        record_section_save(&mut report, "Identity", save_identity(identity));
    }
    if let Some(auto_fetch) = settings.auto_fetch {
        record_section_save(&mut report, "Background Fetch", save_auto_fetch(auto_fetch));
    }
    if let Some(hooks) = &settings.hooks {
        record_section_save(&mut report, "Git Hooks", save_hooks(hooks));
    }

    report
}

fn record_section_save(
    report: &mut GitSettingsSaveReport,
    section: &'static str,
    result: Result<(), String>,
) {
    match result {
        Ok(()) => report.saved_sections.push(section),
        Err(error) => report.errors.push(format!("{section}: {error}")),
    }
}

pub(crate) fn load_global_git_identity() -> Result<GitIdentity, String> {
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
    use std::{cell::RefCell, fs, path::PathBuf};

    use super::*;
    use git2::ConfigLevel;

    fn temporary_config() -> (tempfile::TempDir, PathBuf, Config) {
        let directory = tempfile::tempdir().expect("create temporary directory");
        let path = directory.path().join("gitconfig");
        fs::write(&path, "").expect("create temporary Git config");
        let config = Config::open(&path).expect("open temporary Git config");
        (directory, path, config)
    }

    fn hook(enabled: bool, name: &str, content: &str) -> HookDraft {
        HookDraft {
            enabled,
            name: name.to_string(),
            content: content.to_string(),
        }
    }

    #[test]
    fn save_attempts_remaining_sections_after_a_failure() {
        let settings = GitSettingsDraft {
            identity: Some(GitIdentity {
                name: "Jane Doe".to_string(),
                email: "jane@example.com".to_string(),
            }),
            auto_fetch: Some(AutoFetchSettings::default()),
            hooks: Some(GitHooksConfig::default()),
        };
        let attempts = RefCell::new(Vec::new());

        let report = save_git_settings_with(
            &settings,
            |_| {
                attempts.borrow_mut().push("Identity");
                Err("identity failure".to_string())
            },
            |_| {
                attempts.borrow_mut().push("Background Fetch");
                Ok(())
            },
            |_| {
                attempts.borrow_mut().push("Git Hooks");
                Ok(())
            },
        );

        assert_eq!(
            attempts.into_inner(),
            vec!["Identity", "Background Fetch", "Git Hooks"]
        );
        assert_eq!(report.saved_sections, vec!["Background Fetch", "Git Hooks"]);
        assert_eq!(report.errors, vec!["Identity: identity failure"]);
    }

    #[test]
    fn save_skips_protected_sections_and_saves_a_healthy_section() {
        let settings = GitSettingsDraft {
            identity: None,
            auto_fetch: Some(AutoFetchSettings::default()),
            hooks: None,
        };
        let attempts = RefCell::new(Vec::new());

        let report = save_git_settings_with(
            &settings,
            |_| panic!("protected identity must not be saved"),
            |_| {
                attempts.borrow_mut().push("Background Fetch");
                Ok(())
            },
            |_| panic!("protected hooks must not be saved"),
        );

        assert_eq!(attempts.into_inner(), vec!["Background Fetch"]);
        assert_eq!(report.saved_sections, vec!["Background Fetch"]);
        assert!(report.errors.is_empty());
    }

    #[test]
    fn save_collects_all_section_failures() {
        let settings = GitSettingsDraft {
            identity: Some(GitIdentity::default()),
            auto_fetch: Some(AutoFetchSettings::default()),
            hooks: Some(GitHooksConfig::default()),
        };

        let report = save_git_settings_with(
            &settings,
            |_| Err("identity failure".to_string()),
            |_| Err("fetch failure".to_string()),
            |_| Err("hooks failure".to_string()),
        );

        assert!(report.saved_sections.is_empty());
        assert_eq!(
            report.errors,
            vec![
                "Identity: identity failure",
                "Background Fetch: fetch failure",
                "Git Hooks: hooks failure",
            ]
        );
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
    fn blank_identity_update_is_rejected() {
        let initial = GitIdentity {
            name: "Jane Doe".to_string(),
            email: "jane@example.com".to_string(),
        };

        assert!(GitIdentity::update_from_input(&initial, "", "  ").is_err());
    }

    #[test]
    fn unchanged_partial_identity_does_not_block_other_settings() {
        let initial = GitIdentity {
            name: "Jane Doe".to_string(),
            email: String::new(),
        };

        assert_eq!(
            GitIdentity::update_from_input(&initial, " Jane Doe ", ""),
            Ok(None)
        );
    }

    #[test]
    fn edited_partial_identity_is_rejected() {
        let initial = GitIdentity {
            name: "Jane Doe".to_string(),
            email: String::new(),
        };

        assert!(GitIdentity::update_from_input(&initial, "Janet Doe", "").is_err());
    }

    #[test]
    fn completing_partial_identity_creates_an_update() {
        let initial = GitIdentity {
            name: "Jane Doe".to_string(),
            email: String::new(),
        };

        assert_eq!(
            GitIdentity::update_from_input(&initial, "Jane Doe", "jane@example.com"),
            Ok(Some(GitIdentity {
                name: "Jane Doe".to_string(),
                email: "jane@example.com".to_string(),
            }))
        );
    }

    #[test]
    fn rejects_partial_identity() {
        assert!(GitIdentity::from_input("", "jane@example.com").is_err());
        assert!(GitIdentity::from_input("Jane Doe", "  ").is_err());
    }

    #[test]
    fn validates_auto_fetch_interval_boundaries() {
        assert_eq!(validate_auto_fetch_interval("1"), Ok(1));
        assert_eq!(validate_auto_fetch_interval(" 5 "), Ok(5));
        assert_eq!(validate_auto_fetch_interval("60"), Ok(60));
    }

    #[test]
    fn rejects_invalid_auto_fetch_intervals() {
        for value in ["0", "61", "-1", "1.5", "five", ""] {
            assert!(
                validate_auto_fetch_interval(value).is_err(),
                "{value:?} should be rejected"
            );
        }
    }

    #[test]
    fn normalizes_and_validates_hook_names() {
        assert_eq!(
            validate_hook_name("  pre-commit  "),
            Ok("pre-commit".into())
        );

        for name in [
            "",
            "  ",
            ".",
            "..",
            "hooks/pre-commit",
            "hooks\\pre-commit",
            "PRE-COMMIT",
            "pre-Commit",
            "pr\u{e9}-commit",
        ] {
            assert!(
                validate_hook_name(name).is_err(),
                "{name:?} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_case_variant_hook_names() {
        assert!(
            build_hooks_config([
                hook(true, "pre-commit", "first"),
                hook(true, "PRE-COMMIT", "second"),
            ])
            .is_err()
        );
    }

    #[test]
    fn builds_hooks_config_and_preserves_disabled_hooks() {
        let config = build_hooks_config([
            hook(true, " pre-commit ", "#!/bin/sh\ncargo fmt --check\n"),
            hook(false, "pre-push", "#!/bin/sh\ncargo test\n"),
        ])
        .expect("build hooks configuration");

        assert_eq!(config.version, GIT_HOOKS_CONFIG_VERSION);
        assert_eq!(
            config.hooks.get("pre-commit"),
            Some(&GitHookDefinition {
                enabled: true,
                content: "#!/bin/sh\ncargo fmt --check\n".to_string(),
            })
        );
        assert_eq!(
            config.hooks.get("pre-push"),
            Some(&GitHookDefinition {
                enabled: false,
                content: "#!/bin/sh\ncargo test\n".to_string(),
            })
        );
    }

    #[test]
    fn skips_disabled_blank_hook_rows() {
        let config = build_hooks_config([hook(false, "", "  ")]).expect("skip blank row");

        assert!(config.hooks.is_empty());
    }

    #[test]
    fn preserves_disabled_hook_drafts_without_scripts() {
        let config =
            build_hooks_config([hook(false, "pre-rebase", "")]).expect("preserve disabled draft");

        assert_eq!(
            config.hooks.get("pre-rebase"),
            Some(&GitHookDefinition {
                enabled: false,
                content: String::new(),
            })
        );
    }

    #[test]
    fn rejects_hooks_without_scripts_or_with_duplicate_names() {
        assert!(build_hooks_config([hook(true, "pre-commit", "  ")]).is_err());
        assert!(
            build_hooks_config([
                hook(true, "pre-commit", "first"),
                hook(false, " pre-commit ", "second"),
            ])
            .is_err()
        );
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
