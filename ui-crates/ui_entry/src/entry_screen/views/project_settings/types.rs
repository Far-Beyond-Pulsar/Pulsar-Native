use std::path::PathBuf;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq)]
pub enum ProjectSettingsTab {
    General,
    GitInfo,
    GitCI,
    Metadata,
    DiskInfo,
    Performance,
    Integrations,
}

#[derive(Clone, Debug)]
pub struct ProjectSettings {
    pub project_path: PathBuf,
    pub project_name: String,
    pub active_tab: ProjectSettingsTab,
    pub git_repo_size: Option<u64>,
    pub disk_size: Option<u64>,
    pub commit_count: Option<usize>,
    pub branch_count: Option<usize>,
    pub remote_url: Option<String>,
    pub last_commit_date: Option<String>,
    pub last_commit_message: Option<String>,
    pub uncommitted_changes: Option<usize>,
    pub workflow_files: Vec<String>,
    pub current_branch: Option<String>,
    pub stash_count: Option<usize>,
    pub untracked_files: Option<usize>,
    pub preferred_editor: Option<String>,
    pub preferred_git_tool: Option<String>,
    // Cached tool lists to avoid re-detecting on every render
    pub available_tools_cache: Option<AvailableTools>,
    pub is_updating_tools: bool, // True while background detection is running
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AvailableTools {
    pub editors: Vec<ToolInfo>,
    pub git_tools: Vec<ToolInfo>,
    pub terminals: Vec<ToolInfo>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolInfo {
    pub name: String,
    pub command: String,
    pub available: bool,
}

#[derive(Default)]
struct GitInfo {
    repo_size: Option<u64>,
    commit_count: Option<usize>,
    branch_count: Option<usize>,
    remote_url: Option<String>,
    last_commit_date: Option<String>,
    last_commit_message: Option<String>,
    uncommitted_changes: Option<usize>,
    current_branch: Option<String>,
    stash_count: Option<usize>,
    untracked_files: Option<usize>,
}

impl ToolInfo {
    pub fn new(name: &str, command: &str) -> Self {
        let available = check_tool_available(command);
        Self {
            name: name.to_string(),
            command: command.to_string(),
            available,
        }
    }
}

fn check_tool_available(command: &str) -> bool {
    #[cfg(windows)]
    {
        std::process::Command::new("where")
            .arg(command)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("which")
            .arg(command)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

impl AvailableTools {
    pub fn detect() -> Self {
        Self {
            editors: vec![
                ToolInfo::new("Visual Studio Code", "code"),
                ToolInfo::new("Visual Studio", "devenv"),
                ToolInfo::new("Sublime Text", "subl"),
                ToolInfo::new("Vim", "vim"),
                ToolInfo::new("Neovim", "nvim"),
                ToolInfo::new("Emacs", "emacs"),
                ToolInfo::new("IntelliJ IDEA", "idea"),
                ToolInfo::new("CLion", "clion"),
                ToolInfo::new("Notepad++", "notepad++"),
            ],
            git_tools: vec![
                ToolInfo::new("Git GUI", "git"),
                ToolInfo::new("GitHub Desktop", "github"),
                ToolInfo::new("GitKraken", "gitkraken"),
                ToolInfo::new("SourceTree", "sourcetree"),
                ToolInfo::new("Git Cola", "git-cola"),
                ToolInfo::new("Lazygit", "lazygit"),
                ToolInfo::new("Magit (Emacs)", "emacs"),
            ],
            terminals: vec![
                ToolInfo::new("Windows Terminal", "wt"),
                ToolInfo::new("PowerShell", "pwsh"),
                ToolInfo::new("Command Prompt", "cmd"),
                ToolInfo::new("Git Bash", "bash"),
                ToolInfo::new("Alacritty", "alacritty"),
                ToolInfo::new("Kitty", "kitty"),
            ],
        }
    }
    
    /// Get path to cache file
    fn cache_path() -> PathBuf {
        // Use home directory or fallback to current directory
        let cache_dir = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(".pulsar")
            .join("launcher");
        std::fs::create_dir_all(&cache_dir).ok();
        cache_dir.join("available_tools.json")
    }
    
    /// Load cached tools from file
    pub fn load_from_cache() -> Option<Self> {
        let cache_path = Self::cache_path();
        if !cache_path.exists() {
            return None;
        }
        
        let content = std::fs::read_to_string(cache_path).ok()?;
        serde_json::from_str(&content).ok()
    }
    
    /// Save tools to cache file
    pub fn save_to_cache(&self) {
        let cache_path = Self::cache_path();
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(cache_path, json);
        }
    }
}

impl ProjectSettings {
    pub fn new(project_path: PathBuf, project_name: String) -> Self {
        Self {
            project_path,
            project_name,
            active_tab: ProjectSettingsTab::General,
            // All fields start as None - load on demand per tab
            git_repo_size: None,
            disk_size: None,
            commit_count: None,
            branch_count: None,
            remote_url: None,
            last_commit_date: None,
            last_commit_message: None,
            uncommitted_changes: None,
            workflow_files: Vec::new(),
            current_branch: None,
            stash_count: None,
            untracked_files: None,
            preferred_editor: None,
            preferred_git_tool: None,
            available_tools_cache: None,
            is_updating_tools: false,
        }
    }
    
    /// Load data for a specific tab only - called when tab is first accessed
    pub fn load_tab_data_sync(&mut self, tab: &ProjectSettingsTab) {
        match tab {
            ProjectSettingsTab::General => {
                // General data is already available (project_path, project_name)
                // Nothing to load asynchronously
            }
            ProjectSettingsTab::GitInfo => {
                if self.commit_count.is_none() {
                    self.load_git_info();
                }
            }
            ProjectSettingsTab::GitCI => {
                if self.workflow_files.is_empty() {
                    self.load_git_ci_info();
                }
            }
            ProjectSettingsTab::Metadata => {
                // Metadata is mostly file-based, quick to load
            }
            ProjectSettingsTab::DiskInfo => {
                if self.disk_size.is_none() {
                    self.load_disk_info();
                }
            }
            ProjectSettingsTab::Performance => {
                if self.disk_size.is_none() {
                    self.load_disk_info();
                }
                if self.git_repo_size.is_none() {
                    self.load_git_info();
                }
            }
            ProjectSettingsTab::Integrations => {
                if self.preferred_editor.is_none() {
                    self.load_tool_preferences();
                }
            }
        }
    }

    pub fn load_all_data(&mut self) {
        // Load all data synchronously - used for initial background load
        self.load_disk_info();
        self.load_git_info();
        self.load_git_ci_info();
        self.load_tool_preferences();
    }
    
    pub fn load_all_data_async(project_path: PathBuf) -> Self {
        // DON'T load anything here - return empty struct
        // Data will be loaded per-tab on demand
        Self::new(project_path.clone(), 
            project_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Unknown")
                .to_string()
        )
    }

    fn load_disk_info(&mut self) {
        if let Ok(size) = Self::calculate_directory_size(&self.project_path) {
            self.disk_size = Some(size);
        }
    }

    fn calculate_directory_size(path: &PathBuf) -> Result<u64, std::io::Error> {
        let mut total_size = 0u64;
        
        if path.is_dir() {
            for entry in std::fs::read_dir(path)? {
                let entry = entry?;
                let metadata = entry.metadata()?;
                
                if metadata.is_file() {
                    total_size += metadata.len();
                } else if metadata.is_dir() {
                    let dir_path = entry.path();
                    if let Ok(size) = Self::calculate_directory_size(&dir_path) {
                        total_size += size;
                    }
                }
            }
        }
        
        Ok(total_size)
    }

    fn load_git_info(&mut self) {
        let git_info = Self::load_git_info_sync(&self.project_path);
        self.git_repo_size = git_info.repo_size;
        self.commit_count = git_info.commit_count;
        self.branch_count = git_info.branch_count;
        self.remote_url = git_info.remote_url;
        self.last_commit_date = git_info.last_commit_date;
        self.last_commit_message = git_info.last_commit_message;
        self.uncommitted_changes = git_info.uncommitted_changes;
        self.current_branch = git_info.current_branch;
        self.stash_count = git_info.stash_count;
        self.untracked_files = git_info.untracked_files;
    }
    
    fn load_git_info_sync(project_path: &PathBuf) -> GitInfo {
        let mut info = GitInfo::default();
        
        let git_dir = project_path.join(".git");
        if !git_dir.exists() {
            return info;
        }

        // Get git repo size
        if let Ok(size) = Self::calculate_directory_size(&git_dir) {
            info.repo_size = Some(size);
        }

        // Get commit count
        if let Ok(output) = std::process::Command::new("git")
            .current_dir(project_path)
            .args(&["rev-list", "--count", "HEAD"])
            .output()
        {
            if let Ok(count_str) = String::from_utf8(output.stdout) {
                if let Ok(count) = count_str.trim().parse::<usize>() {
                    info.commit_count = Some(count);
                }
            }
        }

        // Get branch count
        if let Ok(output) = std::process::Command::new("git")
            .current_dir(project_path)
            .args(&["branch", "-a"])
            .output()
        {
            if let Ok(branches) = String::from_utf8(output.stdout) {
                info.branch_count = Some(branches.lines().count());
            }
        }

        // Get current branch
        if let Ok(output) = std::process::Command::new("git")
            .current_dir(project_path)
            .args(&["branch", "--show-current"])
            .output()
        {
            if let Ok(branch) = String::from_utf8(output.stdout) {
                let branch = branch.trim().to_string();
                if !branch.is_empty() {
                    info.current_branch = Some(branch);
                }
            }
        }

        // Get remote URL
        if let Ok(output) = std::process::Command::new("git")
            .current_dir(project_path)
            .args(&["config", "--get", "remote.origin.url"])
            .output()
        {
            if let Ok(url) = String::from_utf8(output.stdout) {
                let url = url.trim().to_string();
                if !url.is_empty() {
                    info.remote_url = Some(url);
                }
            }
        }

        // Get last commit info
        if let Ok(output) = std::process::Command::new("git")
            .current_dir(project_path)
            .args(&["log", "-1", "--format=%ci"])
            .output()
        {
            if let Ok(date) = String::from_utf8(output.stdout) {
                let date = date.trim().to_string();
                if !date.is_empty() {
                    info.last_commit_date = Some(date);
                }
            }
        }

        if let Ok(output) = std::process::Command::new("git")
            .current_dir(project_path)
            .args(&["log", "-1", "--format=%s"])
            .output()
        {
            if let Ok(message) = String::from_utf8(output.stdout) {
                let message = message.trim().to_string();
                if !message.is_empty() {
                    info.last_commit_message = Some(message);
                }
            }
        }

        // Get uncommitted changes
        if let Ok(output) = std::process::Command::new("git")
            .current_dir(project_path)
            .args(&["status", "--porcelain"])
            .output()
        {
            if let Ok(status) = String::from_utf8(output.stdout) {
                info.uncommitted_changes = Some(status.lines().count());
            }
        }

        // Get stash count
        if let Ok(output) = std::process::Command::new("git")
            .current_dir(project_path)
            .args(&["stash", "list"])
            .output()
        {
            if let Ok(stashes) = String::from_utf8(output.stdout) {
                info.stash_count = Some(stashes.lines().count());
            }
        }

        // Get untracked files
        if let Ok(output) = std::process::Command::new("git")
            .current_dir(project_path)
            .args(&["ls-files", "--others", "--exclude-standard"])
            .output()
        {
            if let Ok(files) = String::from_utf8(output.stdout) {
                info.untracked_files = Some(files.lines().count());
            }
        }
        
        info
    }

    fn load_git_ci_info(&mut self) {
        self.workflow_files = Self::load_git_ci_info_sync(&self.project_path);
    }
    
    fn load_tool_preferences(&mut self) {
        let (preferred_editor, preferred_git_tool) = load_project_tool_preferences(&self.project_path);
        self.preferred_editor = preferred_editor;
        self.preferred_git_tool = preferred_git_tool;
    }
    
    fn load_git_ci_info_sync(project_path: &PathBuf) -> Vec<String> {
        let workflows_dir = project_path.join(".github").join("workflows");
        if workflows_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(workflows_dir) {
                return entries
                    .filter_map(|e| e.ok())
                    .filter(|e| {
                        e.path()
                            .extension()
                            .and_then(|ext| ext.to_str())
                            .map(|ext| ext == "yml" || ext == "yaml")
                            .unwrap_or(false)
                    })
                    .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
                    .collect();
            }
        }
        Vec::new()
    }
}

pub fn format_size(size: Option<u64>) -> String {
    match size {
        Some(bytes) => {
            const KB: u64 = 1024;
            const MB: u64 = KB * 1024;
            const GB: u64 = MB * 1024;
            
            if bytes >= GB {
                format!("{:.2} GB", bytes as f64 / GB as f64)
            } else if bytes >= MB {
                format!("{:.2} MB", bytes as f64 / MB as f64)
            } else if bytes >= KB {
                format!("{:.2} KB", bytes as f64 / KB as f64)
            } else {
                format!("{} bytes", bytes)
            }
        }
        None => "N/A".to_string(),
    }
}

/// Load tool preferences for a project without loading full settings
pub fn load_project_tool_preferences(project_path: &std::path::PathBuf) -> (Option<String>, Option<String>) {
    let config_path = project_path.join("Pulsar.toml");
    if !config_path.exists() {
        return (None, None);
    }
    
    if let Ok(content) = std::fs::read_to_string(&config_path) {
        if let Ok(parsed) = toml::from_str::<toml::Value>(&content) {
            if let Some(tools_table) = parsed.get("tools").and_then(|v| v.as_table()) {
                let editor = tools_table.get("editor")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                
                let git_tool = tools_table.get("git_tool")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                
                return (editor, git_tool);
            }
        }
    }
    
    (None, None)
}
