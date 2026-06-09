use std::sync::Arc;
use std::time::Instant;

use crate::config::Config;
use crate::projects::ProjectManager;
use crate::sessions::SessionManager;

/// The top-level application state shared across all request handlers via Axum
/// extractors. Cheap to clone (Arc-wrapped).
#[derive(Debug, Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub projects: ProjectManager,
    pub sessions: SessionManager,
    pub started_at: Arc<Instant>,
}

impl AppState {
    pub fn new(config: Config, projects: ProjectManager) -> Self {
        Self {
            config: Arc::new(config),
            projects,
            sessions: SessionManager::new(),
            started_at: Arc::new(Instant::now()),
        }
    }

    pub fn uptime_seconds(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }
}
