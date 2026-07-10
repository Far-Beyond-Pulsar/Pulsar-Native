use crate::screen::ModernSettingsScreen;
use engine_state::{GlobalSettings, ProjectSettings};
use gpui::Context;

pub fn save_pending_changes(screen: &mut ModernSettingsScreen, cx: &mut Context<ModernSettingsScreen>) {
    let global = GlobalSettings::new();
    match global.save_all() {
        Ok(_) => tracing::info!("Editor settings saved."),
        Err(e) => tracing::error!("Error saving editor settings: {e:?}"),
    }
    if let Some(ref path) = screen.project_path {
        match ProjectSettings::new(path) {
            Some(ps) => match ps.save_all() {
                Ok(_) => tracing::info!("Project settings saved."),
                Err(e) => tracing::error!("Error saving project settings: {e:?}"),
            },
            None => tracing::warn!("Project path does not exist on disk — skipping project settings."),
        }
    }
    screen.has_pending_changes = false;
    cx.notify();
}
