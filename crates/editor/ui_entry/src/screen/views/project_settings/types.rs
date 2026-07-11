use std::collections::HashMap;
use std::path::PathBuf;

use crate::service::project_service::ProjectService;

#[derive(Clone, PartialEq, Eq)]
pub enum ProjectSettingsTab {
    General,
    GitInfo,
    GitCI,
    Metadata,
    DiskInfo,
    Performance,
    Integrations,
}

#[derive(Clone)]
pub struct ProjectSettings {
    pub project_path: PathBuf,
    pub project_name: String,
    pub active_tab: ProjectSettingsTab,
    pub disk_size: Option<u64>,
    pub git_repo_size: Option<u64>,
    pub commit_count: Option<usize>,
    pub branch_count: Option<usize>,
    pub remote_url: Option<String>,
    pub last_commit_date: Option<String>,
    pub last_commit_message: Option<String>,
    pub uncommitted_changes: Option<usize>,
    pub current_branch: Option<String>,
    pub stash_count: Option<usize>,
    pub untracked_files: Option<usize>,
    pub workflow_files: Vec<String>,
    pub preferred_editor: Option<String>,
    pub preferred_git_tool: Option<String>,
}

impl ProjectSettings {
    pub fn new(project_path: PathBuf, project_name: String) -> Self {
        let (editor, git_tool) = ProjectService::load_tool_preferences(&project_path);
        Self {
            project_path,
            project_name,
            active_tab: ProjectSettingsTab::General,
            disk_size: None,
            git_repo_size: None,
            commit_count: None,
            branch_count: None,
            remote_url: None,
            last_commit_date: None,
            last_commit_message: None,
            uncommitted_changes: None,
            current_branch: None,
            stash_count: None,
            untracked_files: None,
            workflow_files: Vec::new(),
            preferred_editor: editor,
            preferred_git_tool: git_tool,
        }
    }

    pub fn load_tab_data_sync(&mut self, tab: &ProjectSettingsTab) {
        match tab {
            ProjectSettingsTab::GitInfo => self.load_git_info_sync(),
            ProjectSettingsTab::GitCI => self.load_git_ci_sync(),
            ProjectSettingsTab::DiskInfo => self.load_disk_info_sync(),
            ProjectSettingsTab::Performance => {
                self.load_disk_info_sync();
                self.load_git_info_sync();
            }
            ProjectSettingsTab::Integrations => self.load_integrations_sync(),
            _ => {}
        }
    }

    pub fn load_all_data_async(project_path: PathBuf) -> Self {
        let mut settings = Self::new(project_path.clone(), String::new());
        settings.load_disk_info_sync();
        settings.load_git_info_sync();
        settings.load_git_ci_sync();
        settings.load_integrations_sync();
        settings
    }

    fn load_git_info_sync(&mut self) {
        let path = &self.project_path;
        if let Ok(mut repo) = git2::Repository::open(path) {
            self.current_branch = repo
                .head()
                .ok()
                .and_then(|h| h.shorthand().ok().map(|s| s.to_string()));
            if let Ok(remote) = repo.find_remote("origin") {
                self.remote_url = remote.url().ok().map(|s| s.to_string());
            }
            self.commit_count = repo.revwalk().ok().map(|mut w| {
                w.push_head().ok();
                w.count()
            });
            self.branch_count = Some(repo.branches(None).ok().map(|b| b.count()).unwrap_or(0));
            self.stash_count = repo
                .stash_foreach(|_, _, _| false)
                .ok()
                .map(|_| 0)
                .or(Some(0));
            if let Ok(head) = repo.head() {
                if let Some(oid) = head.target() {
                    if let Ok(commit) = repo.find_commit(oid) {
                        self.last_commit_date = Some(
                            chrono::TimeZone::from_utc_datetime(
                                &chrono::Utc,
                                &chrono::NaiveDateTime::from_timestamp_opt(
                                    commit.time().seconds(),
                                    0,
                                )
                                .unwrap_or_default(),
                            )
                            .format("%Y-%m-%d %H:%M")
                            .to_string(),
                        );
                        self.last_commit_message = Some(commit.message().unwrap_or("").to_string());
                    }
                }
            }
            self.uncommitted_changes = Some(repo.statuses(None).ok().map(|s| s.len()).unwrap_or(0));
        }
        let git_dir = path.join(".git");
        if git_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&git_dir) {
                let mut total = 0u64;
                for entry in entries.flatten() {
                    if let Ok(meta) = entry.metadata() {
                        total += meta.len();
                    }
                }
                self.git_repo_size = Some(total);
            }
        }
    }

    fn load_git_ci_sync(&mut self) {
        let workflows_dir = self.project_path.join(".github").join("workflows");
        if workflows_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&workflows_dir) {
                self.workflow_files = entries
                    .filter_map(|e| e.ok())
                    .filter(|e| {
                        e.path().extension().and_then(|ext| ext.to_str()) == Some("yml")
                            || e.path().extension().and_then(|ext| ext.to_str()) == Some("yaml")
                    })
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect();
            }
        }
    }

    fn load_disk_info_sync(&mut self) {
        if let Ok(entries) = std::fs::read_dir(&self.project_path) {
            let mut total = 0u64;
            for entry in entries.flatten() {
                if entry.path() == self.project_path.join(".git") {
                    continue;
                }
                total += Self::dir_size(&entry.path());
            }
            self.disk_size = Some(total);
        }
    }

    fn load_integrations_sync(&mut self) {
        let (editor, git_tool) = ProjectService::load_tool_preferences(&self.project_path);
        self.preferred_editor = editor;
        self.preferred_git_tool = git_tool;
    }

    fn dir_size(path: &std::path::Path) -> u64 {
        if path.is_file() {
            return std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        }
        let mut total = 0u64;
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                total += Self::dir_size(&entry.path());
            }
        }
        total
    }
}

#[derive(Clone)]
pub struct ToolInfo {
    pub name: String,
    pub path: String,
    pub is_default: bool,
}

#[derive(Clone)]
pub struct AvailableTools {
    pub editors: Vec<ToolInfo>,
    pub git_tools: Vec<ToolInfo>,
    pub terminals: Vec<ToolInfo>,
}

impl AvailableTools {
    pub fn detect() -> Self {
        Self {
            editors: Self::detect_editors(),
            git_tools: Self::detect_git_tools(),
            terminals: Self::detect_terminals(),
        }
    }

    fn detect_editors() -> Vec<ToolInfo> {
        let mut editors = Vec::new();
        for (name, cmd) in &[
            ("Visual Studio Code", "code"),
            ("VS Code Insiders", "code-insiders"),
            ("Zed", "zed"),
            ("IntelliJ IDEA", "idea"),
            ("Fleet", "fleet"),
            ("Sublime Text", "subl"),
            ("Atom", "atom"),
            ("Emacs", "emacs"),
            ("Vim", "vim"),
            ("Neovim", "nvim"),
        ] {
            if which::which(cmd).is_ok() {
                editors.push(ToolInfo {
                    name: name.to_string(),
                    path: which::which(cmd)
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_default(),
                    is_default: false,
                });
            }
        }
        editors
    }

    fn detect_git_tools() -> Vec<ToolInfo> {
        let mut tools = Vec::new();
        for (name, cmd) in &[
            ("GitHub Desktop", "github"),
            ("GitKraken", "gitkraken"),
            ("Sourcetree", "sourcetree"),
            ("Fork", "fork"),
            ("Git GUI", "git-gui"),
            ("Git Cola", "git-cola"),
        ] {
            if which::which(cmd).is_ok() {
                tools.push(ToolInfo {
                    name: name.to_string(),
                    path: which::which(cmd)
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_default(),
                    is_default: false,
                });
            }
        }
        tools
    }

    fn detect_terminals() -> Vec<ToolInfo> {
        let mut terminals = Vec::new();
        #[cfg(target_os = "windows")]
        {
            terminals.push(ToolInfo {
                name: "PowerShell".to_string(),
                path: "powershell.exe".to_string(),
                is_default: false,
            });
            terminals.push(ToolInfo {
                name: "Command Prompt".to_string(),
                path: "cmd.exe".to_string(),
                is_default: false,
            });
            if which::which("wt").is_ok() {
                terminals.push(ToolInfo {
                    name: "Windows Terminal".to_string(),
                    path: which::which("wt")
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_default(),
                    is_default: false,
                });
            }
        }
        #[cfg(target_os = "linux")]
        {
            terminals.push(ToolInfo {
                name: "GNOME Terminal".to_string(),
                path: "gnome-terminal".to_string(),
                is_default: false,
            });
            terminals.push(ToolInfo {
                name: "Konsole".to_string(),
                path: "konsole".to_string(),
                is_default: false,
            });
            terminals.push(ToolInfo {
                name: "xterm".to_string(),
                path: "xterm".to_string(),
                is_default: false,
            });
            terminals.push(ToolInfo {
                name: "Alacritty".to_string(),
                path: "alacritty".to_string(),
                is_default: false,
            });
        }
        #[cfg(target_os = "macos")]
        {
            terminals.push(ToolInfo {
                name: "Terminal".to_string(),
                path: "Terminal".to_string(),
                is_default: false,
            });
            terminals.push(ToolInfo {
                name: "iTerm2".to_string(),
                path: "iTerm2".to_string(),
                is_default: false,
            });
            terminals.push(ToolInfo {
                name: "Alacritty".to_string(),
                path: "alacritty".to_string(),
                is_default: false,
            });
        }
        terminals
    }
}
