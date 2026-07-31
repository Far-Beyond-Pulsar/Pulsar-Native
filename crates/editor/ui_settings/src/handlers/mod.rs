use crate::screen::ModernSettingsScreen;
use engine_state::{GlobalSettings, ProjectSettings};
use gpui::{Context, Window};
use ui::{notification::Notification, ContextModal as _};

pub fn save_pending_changes(
    screen: &mut ModernSettingsScreen,
    window: &mut Window,
    cx: &mut Context<ModernSettingsScreen>,
) {
    let mut errors = Vec::new();
    let global = GlobalSettings::new();
    match global.save_all() {
        Ok(_) => tracing::info!("Editor settings saved."),
        Err(error) => {
            tracing::error!("Error saving editor settings: {error:?}");
            errors.push(format!("Editor settings: {error}"));
        }
    }
    if let Some(ref path) = screen.project_path {
        let project_settings = ProjectSettings::new(path);
        if project_settings.is_none() {
            errors.push("Project settings: project path is unavailable.".to_string());
        }
        match project_settings {
            Some(ps) => match ps.save_all() {
                Ok(_) => tracing::info!("Project settings saved."),
                Err(error) => {
                    tracing::error!("Error saving project settings: {error:?}");
                    errors.push(format!("Project settings: {error}"));
                }
            },
            None => {
                tracing::warn!("Project path does not exist on disk — skipping project settings.")
            }
        }
    }
    screen.has_pending_changes = !errors.is_empty();
    if !errors.is_empty() {
        window.push_notification(
            Notification::error(errors.join("\n")).title("Settings were not saved"),
            cx,
        );
    }
    cx.notify();
}
