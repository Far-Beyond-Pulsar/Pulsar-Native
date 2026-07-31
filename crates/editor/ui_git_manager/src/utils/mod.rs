mod git_settings;

pub(crate) use git_settings::{
    AutoFetchSettings, DEFAULT_AUTO_FETCH_INTERVAL_MINUTES, GitIdentity, GitSettingsDraft,
    GitSettingsLoadErrors, HookDraft, MAX_AUTO_FETCH_INTERVAL_MINUTES,
    MIN_AUTO_FETCH_INTERVAL_MINUTES, SOURCE_CONTROL_OWNER, build_hooks_config,
    load_auto_fetch_settings, load_global_git_identity, save_git_settings,
    validate_auto_fetch_interval,
};
