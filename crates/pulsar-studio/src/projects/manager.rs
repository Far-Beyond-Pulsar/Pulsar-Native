use anyhow::{Context as _, Result};
use chrono::Utc;
use parking_lot::RwLock;
use std::{path::PathBuf, sync::Arc};
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::types::{ProjectRecord, ProjectStatus};

/// Thread-safe manager for all project records.
///
/// Projects are stored as entries in a `projects.json` sidecar file inside
/// the data directory. Each project also gets its own subdirectory for files.
#[derive(Debug, Clone)]
pub struct ProjectManager {
    inner: Arc<RwLock<Inner>>,
}

#[derive(Debug)]
struct Inner {
    projects: Vec<ProjectRecord>,
    data_dir: PathBuf,
}

impl Inner {
    fn projects_json_path(&self) -> PathBuf {
        self.data_dir.join("projects.json")
    }

    fn project_dir(&self, id: &str) -> PathBuf {
        self.data_dir.join("projects").join(id)
    }
}

#[allow(dead_code)]
impl ProjectManager {
    /// Load existing projects from disk, creating the file if absent.
    pub fn load(data_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(data_dir.join("projects"))?;

        let path = data_dir.join("projects.json");
        let mut projects: Vec<ProjectRecord> = if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?
        } else {
            Vec::new()
        };

        // Reset all runtime statuses to Idle on startup.
        for p in &mut projects {
            p.status = ProjectStatus::Idle;
            p.error_msg.clear();
        }

        info!("Loaded {} project(s) from disk", projects.len());

        Ok(Self {
            inner: Arc::new(RwLock::new(Inner { projects, data_dir })),
        })
    }

    // ── Read operations (cheap shared lock) ──────────────────────────────────

    pub fn list(&self) -> Vec<ProjectRecord> {
        self.inner.read().projects.clone()
    }

    pub fn get(&self, id: &str) -> Option<ProjectRecord> {
        self.inner
            .read()
            .projects
            .iter()
            .find(|p| p.id == id)
            .cloned()
    }

    pub fn count(&self) -> usize {
        self.inner.read().projects.len()
    }

    pub fn active_count(&self) -> usize {
        self.inner
            .read()
            .projects
            .iter()
            .filter(|p| matches!(p.status, ProjectStatus::Running | ProjectStatus::Preparing))
            .count()
    }

    // ── Write operations ──────────────────────────────────────────────────────

    /// Create a new project and persist it.
    pub fn create(
        &self,
        name: String,
        description: String,
        owner: String,
    ) -> Result<ProjectRecord> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();

        let record = ProjectRecord {
            id: id.clone(),
            name,
            description,
            owner,
            created_at: now,
            last_modified: now,
            size_bytes: 0,
            status: ProjectStatus::Idle,
            error_msg: String::new(),
        };

        let mut guard = self.inner.write();
        let project_dir = guard.project_dir(&id);
        std::fs::create_dir_all(&project_dir)?;
        // Create the workspace directory that the file API will serve from.
        std::fs::create_dir_all(project_dir.join("workspace"))?;
        guard.projects.push(record.clone());
        Self::persist_locked(&guard)?;

        info!("Created project '{}' ({})", record.name, id);
        Ok(record)
    }

    /// Delete a project and remove its files from disk.
    pub fn delete(&self, id: &str) -> Result<bool> {
        let mut guard = self.inner.write();
        let before = guard.projects.len();
        guard.projects.retain(|p| p.id != id);
        if guard.projects.len() == before {
            return Ok(false);
        }

        let project_dir = guard.project_dir(id);
        if project_dir.exists() {
            std::fs::remove_dir_all(&project_dir)
                .with_context(|| format!("removing {}", project_dir.display()))?;
        }

        Self::persist_locked(&guard)?;
        info!("Deleted project {}", id);
        Ok(true)
    }

    /// Move a project to `Preparing` status.
    ///
    /// Returns `false` if the project is already preparing or running.
    pub fn begin_prepare(&self, id: &str) -> Result<bool> {
        let mut guard = self.inner.write();
        let Some(project) = guard.projects.iter_mut().find(|p| p.id == id) else {
            anyhow::bail!("project {} not found", id);
        };

        if matches!(
            project.status,
            ProjectStatus::Preparing | ProjectStatus::Running
        ) {
            return Ok(false); // Already active; idempotent.
        }

        project.status = ProjectStatus::Preparing;
        project.error_msg.clear();
        info!("Project '{id}' status → Preparing");
        Ok(true)
    }

    /// Transition a project from `Preparing` to `Running`.
    pub fn mark_running(&self, id: &str) {
        let mut guard = self.inner.write();
        if let Some(p) = guard.projects.iter_mut().find(|p| p.id == id) {
            p.status = ProjectStatus::Running;
            info!("Project '{id}' status → Running");
        }
    }

    /// Transition a project to `Error` state with a message.
    pub fn mark_error(&self, id: &str, msg: &str) {
        let mut guard = self.inner.write();
        if let Some(p) = guard.projects.iter_mut().find(|p| p.id == id) {
            warn!("Project '{id}' status → Error: {msg}");
            p.status = ProjectStatus::Error(msg.to_string());
            p.error_msg = msg.to_string();
        }
    }

    /// Transition a project back to `Idle` (e.g. all users left).
    pub fn mark_idle(&self, id: &str) {
        let mut guard = self.inner.write();
        if let Some(p) = guard.projects.iter_mut().find(|p| p.id == id) {
            if matches!(p.status, ProjectStatus::Running) {
                p.status = ProjectStatus::Idle;
                info!("Project '{id}' status → Idle (all users left)");
                let _ = Self::persist_locked(&guard);
            }
        }
    }

    /// Force a project to `Idle` from any active state (explicit stop request).
    ///
    /// Returns `true` if the status changed, `false` if already idle.
    pub fn stop(&self, id: &str) -> bool {
        let mut guard = self.inner.write();
        if let Some(p) = guard.projects.iter_mut().find(|p| p.id == id) {
            if !matches!(p.status, ProjectStatus::Idle) {
                let prev = p.status.as_str();
                p.status = ProjectStatus::Idle;
                p.error_msg.clear();
                info!("Project '{id}' status → Idle (force-stopped from {prev})");
                let _ = Self::persist_locked(&guard);
                return true;
            }
        }
        false
    }

    /// Update the cached disk size for a project.
    pub fn update_size(&self, id: &str) {
        let guard = self.inner.read();
        let dir = guard.project_dir(id);
        drop(guard);

        let size = dir_size(&dir).unwrap_or(0);
        debug!("Project '{id}' disk size: {} byte(s)", size);

        let mut guard = self.inner.write();
        if let Some(p) = guard.projects.iter_mut().find(|p| p.id == id) {
            p.size_bytes = size;
            p.last_modified = Utc::now();
        }
        let _ = Self::persist_locked(&guard);
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    fn persist_locked(guard: &Inner) -> Result<()> {
        let path = guard.projects_json_path();
        let json = serde_json::to_string_pretty(&guard.projects)?;
        std::fs::write(&path, json).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }
}

/// Recursively sum the byte size of all files in a directory.
fn dir_size(path: &std::path::Path) -> std::io::Result<u64> {
    let mut total = 0u64;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_file() {
            total += meta.len();
        } else if meta.is_dir() {
            total += dir_size(&entry.path()).unwrap_or(0);
        }
    }
    Ok(total)
}
