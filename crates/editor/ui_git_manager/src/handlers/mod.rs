mod git_settings;

pub use git_settings::open_git_settings_modal;
pub(crate) use git_settings::{
    add_hook, handle_auto_fetch_toggle, handle_hook_toggle, handle_interval_step, remove_hook,
};
