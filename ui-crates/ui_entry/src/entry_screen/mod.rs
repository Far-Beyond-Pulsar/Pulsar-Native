mod types;
mod git_operations;
pub mod recent_projects;
mod integration_launcher;
pub mod views;
pub mod project_selector;

use types::{EntryScreenView, Template, CloneProgress, SharedCloneProgress, GitFetchStatus, get_default_templates};
use git_operations::{clone_repository, setup_template_remotes, add_user_upstream, init_repository, is_git_repo, check_for_updates, pull_updates};

use gpui::{prelude::*, *};
use ui::{h_flex, v_flex, TitleBar, ActiveTheme as _, input::InputState};
use std::path::PathBuf;
use std::collections::HashMap;
use recent_projects::{RecentProject, RecentProjectsList};
use std::sync::Arc;
use parking_lot::Mutex;

/// EntryScreen: AAA-quality project manager
pub struct EntryScreen {
    pub(crate) view: EntryScreenView,
    pub(crate) recent_projects: RecentProjectsList,
    pub(crate) templates: Vec<Template>,
    pub(crate) recent_projects_path: PathBuf,
    pub(crate) clone_progress: Option<SharedCloneProgress>,
    pub(crate) new_project_name: String,
    pub(crate) new_project_path: Option<PathBuf>,
    pub(crate) git_repo_url: String,
    pub(crate) search_query: String,
    pub(crate) launched: bool,
    pub(crate) git_fetch_statuses: Arc<Mutex<HashMap<String, GitFetchStatus>>>,
    pub(crate) is_fetching_updates: bool,
    pub(crate) show_git_upstream_prompt: Option<(PathBuf, String)>, // (project_path, template_url_if_template)
    pub(crate) git_upstream_url: String,
    pub(crate) project_settings: Option<views::ProjectSettings>,
    pub(crate) show_dependency_setup: bool,
    pub(crate) dependency_status: Option<DependencyStatus>,
    pub(crate) install_progress: Option<InstallProgress>,
    // Input states for interactive text fields
    pub(crate) git_repo_url_input: Entity<InputState>,
    pub(crate) git_upstream_url_input: Entity<InputState>,
    pub(crate) new_project_name_input: Entity<InputState>,
}

#[derive(Clone, Debug)]
pub struct DependencyStatus {
    pub rust_installed: bool,
    pub build_tools_installed: bool,
    pub compiler_info: Option<String>,
}

#[derive(Clone, Debug)]
pub struct InstallProgress {
    pub logs: Vec<String>,
    pub progress: f32,
    pub status: InstallStatus,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InstallStatus {
    Idle,
    Downloading,
    Installing,
    Complete,
    Error(String),
}

impl EntryScreen {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let recent_projects_path = directories::ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
            .map(|proj| proj.data_dir().join("recent_projects.json"))
            .unwrap_or_else(|| PathBuf::from("recent_projects.json"));

        let recent_projects = RecentProjectsList::load(&recent_projects_path);
        let templates = get_default_templates();

        // Create InputState entities for text inputs
        let git_repo_url_input = cx.new(|cx| {
            InputState::new(_window, cx)
                .placeholder("https://github.com/user/repo.git")
        });
        let git_upstream_url_input = cx.new(|cx| {
            InputState::new(_window, cx)
                .placeholder("https://github.com/your-username/your-repo.git")
        });
        let new_project_name_input = cx.new(|cx| {
            InputState::new(_window, cx)
                .placeholder("my_awesome_game")
        });

        let mut screen = Self {
            view: EntryScreenView::Recent,
            recent_projects,
            templates,
            recent_projects_path,
            clone_progress: None,
            new_project_name: String::new(),
            new_project_path: None,
            git_repo_url: String::new(),
            search_query: String::new(),
            launched: false,
            git_fetch_statuses: Arc::new(Mutex::new(HashMap::new())),
            is_fetching_updates: false,
            show_git_upstream_prompt: None,
            git_upstream_url: String::new(),
            project_settings: None,
            show_dependency_setup: false,
            dependency_status: None,
            install_progress: None,
            git_repo_url_input: git_repo_url_input.clone(),
            git_upstream_url_input: git_upstream_url_input.clone(),
            new_project_name_input: new_project_name_input.clone(),
        };

        // Subscribe to input events
        cx.subscribe(&git_repo_url_input, |this, _input, event: &ui::input::InputEvent, cx| {
            match event {
                ui::input::InputEvent::Change => {
                    this.git_repo_url = this.git_repo_url_input.read(cx).text().to_string();
                    cx.notify();
                }
                ui::input::InputEvent::PressEnter { .. } => {
                    // Note: Will need window parameter for clone_git_repo - will be handled in a later fix
                    // For now, skipping this functionality
                }
                _ => {}
            }
        }).detach();

        cx.subscribe(&git_upstream_url_input, |this, _input, event: &ui::input::InputEvent, cx| {
            match event {
                ui::input::InputEvent::Change => {
                    this.git_upstream_url = this.git_upstream_url_input.read(cx).text().to_string();
                    cx.notify();
                }
                ui::input::InputEvent::PressEnter { .. } => {
                    this.setup_git_upstream(false, cx);
                }
                _ => {}
            }
        }).detach();

        cx.subscribe(&new_project_name_input, |this, _input, event: &ui::input::InputEvent, cx| {
            match event {
                ui::input::InputEvent::Change => {
                    this.new_project_name = this.new_project_name_input.read(cx).text().to_string();
                    cx.notify();
                }
                ui::input::InputEvent::PressEnter { .. } => {
                    // Note: Will need window parameter for create_new_project - will be handled via button click
                }
                _ => {}
            }
        }).detach();

        // Check dependencies on background thread
        screen.check_dependencies_async(cx);

        screen
    }
    
    pub(crate) fn check_dependencies_async(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let status = cx.background_executor().spawn(async {
                use std::process::Command;
                
                // Check for Rust
                let rust_installed = Command::new("rustc")
                    .arg("--version")
                    .output()
                    .is_ok();
                
                // Check for build tools - accept ANY compiler toolchain
                #[cfg(target_os = "windows")]
                let (build_tools_installed, compiler_info) = {
                    // Try MSVC first
                    if Command::new("cl").arg("/?").output().is_ok() {
                        (true, Some("MSVC".to_string()))
                    } else if Command::new("gcc").arg("--version").output().is_ok() {
                        (true, Some("GCC (MinGW)".to_string()))
                    } else if Command::new("clang").arg("--version").output().is_ok() {
                        (true, Some("Clang".to_string()))
                    } else {
                        (false, None)
                    }
                };
                
                #[cfg(target_os = "linux")]
                let (build_tools_installed, compiler_info) = {
                    // Try GCC first
                    if Command::new("gcc").arg("--version").output().is_ok() {
                        (true, Some("GCC".to_string()))
                    } else if Command::new("clang").arg("--version").output().is_ok() {
                        (true, Some("Clang".to_string()))
                    } else {
                        (false, None)
                    }
                };
                
                #[cfg(target_os = "macos")]
                let (build_tools_installed, compiler_info) = {
                    // Try Clang first (standard on macOS)
                    if Command::new("clang").arg("--version").output().is_ok() {
                        (true, Some("Clang".to_string()))
                    } else if Command::new("gcc").arg("--version").output().is_ok() {
                        (true, Some("GCC".to_string()))
                    } else {
                        (false, None)
                    }
                };
                
                DependencyStatus {
                    rust_installed,
                    build_tools_installed,
                    compiler_info,
                }
            }).await;
            
            cx.update(|cx| {
                this.update(cx, |screen, cx| {
                    let missing = !status.rust_installed || !status.build_tools_installed;
                    screen.dependency_status = Some(status);
                    screen.show_dependency_setup = missing;
                    cx.notify();
                }).ok();
            }).ok();
        }).detach();
    }
    
    pub(crate) fn start_git_fetch_all(&mut self, cx: &mut Context<Self>) {
        if self.is_fetching_updates {
            return;
        }
        
        self.is_fetching_updates = true;
        let git_projects: Vec<(String, String)> = self.recent_projects.projects.iter()
            .filter(|p| p.is_git)
            .map(|p| (p.path.clone(), p.name.clone()))
            .collect();
        
        let statuses = self.git_fetch_statuses.clone();
        
        cx.spawn(async move |this, cx| {
            for (path, _name) in git_projects {
                let path_buf = PathBuf::from(&path);
                let path_clone = path.clone();
                
                // Mark as fetching
                {
                    let mut statuses_lock = statuses.lock();
                    statuses_lock.insert(path.clone(), GitFetchStatus::Fetching);
                }
                
                // Fetch in background
                let result = std::thread::spawn(move || {
                    check_for_updates(&path_buf)
                }).join();
                
                // Update status
                {
                    let mut statuses_lock = statuses.lock();
                    match result {
                        Ok(Ok(0)) => {
                            statuses_lock.insert(path_clone.clone(), GitFetchStatus::UpToDate);
                        }
                        Ok(Ok(behind)) => {
                            statuses_lock.insert(path_clone.clone(), GitFetchStatus::UpdatesAvailable(behind));
                        }
                        Ok(Err(e)) => {
                            statuses_lock.insert(path_clone.clone(), GitFetchStatus::Error(e.to_string()));
                        }
                        Err(_) => {
                            statuses_lock.insert(path_clone.clone(), GitFetchStatus::Error("Thread panicked".to_string()));
                        }
                    }
                }
                
                // Notify UI update
                cx.update(|cx| {
                    this.update(cx, |_, cx| cx.notify()).ok();
                }).ok();
            }
            
            // Mark fetch complete
            cx.update(|cx| {
                this.update(cx, |screen, cx| {
                    screen.is_fetching_updates = false;
                    cx.notify();
                }).ok();
            }).ok();
        }).detach();
    }
    
    pub(crate) fn pull_project_updates(&mut self, path: String, cx: &mut Context<Self>) {
        let path_buf = PathBuf::from(&path);
        let statuses = self.git_fetch_statuses.clone();
        
        cx.spawn(async move |this, cx| {
            let result = std::thread::spawn(move || {
                pull_updates(&path_buf)
            }).join();
            
            match result {
                Ok(Ok(())) => {
                    // Success - mark as up to date
                    {
                        let mut statuses_lock = statuses.lock();
                        statuses_lock.insert(path.clone(), GitFetchStatus::UpToDate);
                    }
                }
                Ok(Err(e)) => {
                    tracing::error!("Failed to pull updates: {}", e);
                }
                Err(_) => {
                    tracing::error!("Thread panicked during pull");
                }
            }
            
            cx.update(|cx| {
                this.update(cx, |_, cx| cx.notify()).ok();
            }).ok();
        }).detach();
    }
    
    pub(crate) fn calculate_columns(&self, width: Pixels) -> usize {
        // Account for sidebar width (72px) + container padding (.p_12() = 48px each side = 96px total) + card width (320px) + gap (24px between cards)
        let sidebar_width = 72.0;
        let container_padding = 96.0; // 48px left + 48px right from .p_12()
        let card_width = 320.0;
        let gap_size = 24.0; // .gap_6() = 6 * 4 = 24px
        
        // Convert Pixels to f32
        let width_f32: f32 = width.into();
        let available_width = width_f32 - sidebar_width - container_padding;
        
        // Calculate how many cards fit: (available_width + gap) / (card_width + gap)
        let columns = ((available_width + gap_size) / (card_width + gap_size)).floor() as usize;
        
        // Ensure at least 1 column, max 6
        columns.max(1).min(6)
    }
    
    pub(crate) fn open_folder_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let file_dialog = rfd::AsyncFileDialog::new()
            .set_title("Select Pulsar Project Folder")
            .set_directory(std::env::current_dir().unwrap_or_default());
        
        let recent_projects_path = self.recent_projects_path.clone();
        
        cx.spawn(async move |this, cx| {
            if let Some(folder) = file_dialog.pick_folder().await {
                let path = folder.path().to_path_buf();
                let toml_path = path.join("Pulsar.toml");
                
                if !toml_path.exists() {
                    tracing::error!("Invalid project: Pulsar.toml not found");
                    return;
                }
                
                let project_name = path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Unknown")
                    .to_string();
                
                let is_git = is_git_repo(&path);
                
                let recent_project = RecentProject {
                    name: project_name,
                    path: path.to_string_lossy().to_string(),
                    last_opened: Some(chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()),
                    is_git,
                };
                
                cx.update(|cx| {
                    this.update(cx, |screen, cx| {
                        screen.recent_projects.add_or_update(recent_project);
                        screen.recent_projects.save(&recent_projects_path);
                        cx.emit(crate::entry_screen::project_selector::ProjectSelected { path });
                    }).ok();
                }).ok();
            }
        }).detach();
    }
    
    pub(crate) fn clone_git_repo(&mut self, repo_url: String, target_name: String, is_template: bool, _window: &mut Window, cx: &mut Context<Self>) {
        let progress = Arc::new(Mutex::new(CloneProgress {
            current: 0,
            total: 100,
            message: "Initializing...".to_string(),
            completed: false,
            error: None,
        }));
        
        self.clone_progress = Some(progress.clone());
        let recent_projects_path = self.recent_projects_path.clone();
        
        cx.spawn(async move |this, cx| {
            let file_dialog = rfd::AsyncFileDialog::new()
                .set_title(format!("Choose location for {}", target_name))
                .set_directory(std::env::current_dir().unwrap_or_default());
            
            if let Some(folder) = file_dialog.pick_folder().await {
                let parent_path = folder.path().to_path_buf();
                let project_name = target_name.replace(" ", "_").to_lowercase();
                let target_path = parent_path.join(&project_name);
                let target_path_str = target_path.to_string_lossy().to_string();
                
                {
                    let mut prog = progress.lock();
                    prog.message = "Cloning repository...".to_string();
                    prog.current = 10;
                }
                
                cx.update(|cx| {
                    this.update(cx, |_, cx| cx.notify()).ok();
                }).ok();
                
                let repo_url_clone = repo_url.clone();
                let progress_clone = progress.clone();
                let target_path_clone = target_path.clone();
                
                let repo_result = std::thread::spawn(move || {
                    clone_repository(repo_url_clone, target_path_clone, progress_clone)
                }).join();
                
                match repo_result {
                    Ok(Ok(_repo)) => {
                        {
                            let mut prog = progress.lock();
                            prog.completed = true;
                            prog.current = prog.total;
                            prog.message = "Clone completed!".to_string();
                        }
                        
                        // If template, rename origin to template
                        if is_template {
                            if let Err(e) = setup_template_remotes(&target_path, &repo_url) {
                                tracing::error!("Failed to setup template remotes: {}", e);
                            }
                        }
                        
                        let recent_project = RecentProject {
                            name: project_name.clone(),
                            path: target_path_str,
                            last_opened: Some(chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()),
                            is_git: true,
                        };
                        
                        let template_url = if is_template { Some(repo_url.clone()) } else { None };
                        
                        cx.update(|cx| {
                            this.update(cx, |screen, cx| {
                                screen.recent_projects.add_or_update(recent_project);
                                screen.recent_projects.save(&recent_projects_path);
                                screen.clone_progress = None;
                                
                                // Show upstream prompt
                                screen.show_git_upstream_prompt = Some((
                                    target_path.clone(),
                                    template_url.unwrap_or_default(),
                                ));
                                
                                cx.notify();
                            }).ok();
                        }).ok();
                    }
                    Ok(Err(e)) => {
                        let mut prog = progress.lock();
                        prog.error = Some(format!("Clone failed: {}", e));
                        prog.message = "Error occurred".to_string();
                    }
                    Err(_) => {
                        let mut prog = progress.lock();
                        prog.error = Some("Thread panic during clone".to_string());
                    }
                }
                
                cx.update(|cx| {
                    this.update(cx, |_, cx| cx.notify()).ok();
                }).ok();
            } else {
                cx.update(|cx| {
                    this.update(cx, |screen, cx| {
                        screen.clone_progress = None;
                        cx.notify();
                    }).ok();
                }).ok();
            }
        }).detach();
    }
    
    pub(crate) fn clone_template(&mut self, template: &Template, window: &mut Window, cx: &mut Context<Self>) {
        self.clone_git_repo(template.repo_url.clone(), template.name.clone(), true, window, cx);
    }
    
    pub(crate) fn setup_git_upstream(&mut self, skip: bool, cx: &mut Context<Self>) {
        if let Some((project_path, template_url)) = self.show_git_upstream_prompt.take() {
            if !skip && !self.git_upstream_url.trim().is_empty() {
                // Add user's upstream
                if let Err(e) = add_user_upstream(&project_path, &self.git_upstream_url) {
                    tracing::error!("Failed to add upstream: {}", e);
                }
            }
            
            // Clear the upstream URL field
            self.git_upstream_url.clear();
            
            // Launch the project
            self.launch_project(project_path, cx);
        }
        cx.notify();
    }
    
    pub(crate) fn launch_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.launched {
            return;
        }
        self.launched = true;

        let project_name = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Unknown")
            .to_string();

        let is_git = is_git_repo(&path);

        let recent_project = RecentProject {
            name: project_name,
            path: path.to_string_lossy().to_string(),
            last_opened: Some(chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()),
            is_git,
        };

        self.recent_projects.add_or_update(recent_project);
        self.recent_projects.save(&self.recent_projects_path);

        // Only emit the event - the window creation is handled by the event subscriber
        cx.emit(crate::entry_screen::project_selector::ProjectSelected { path });
    }
    
    pub(crate) fn remove_recent_project(&mut self, path: String, cx: &mut Context<Self>) {
        self.recent_projects.remove(&path);
        self.recent_projects.save(&self.recent_projects_path);
        cx.notify();
    }
    
    pub(crate) fn open_project_settings(&mut self, project_path: PathBuf, project_name: String, cx: &mut Context<Self>) {
        // Create settings with NO data loaded - instant UI
        let settings = views::ProjectSettings::new(project_path.clone(), project_name);
        self.project_settings = Some(settings);
        cx.notify();
        
        // NOTE: Data will be loaded per-tab when user switches to each tab
        // This ensures instant modal opening with no freeze
    }
    
    pub(crate) fn close_project_settings(&mut self, cx: &mut Context<Self>) {
        self.project_settings = None;
        cx.notify();
    }
    
    pub(crate) fn change_project_settings_tab(&mut self, tab: views::ProjectSettingsTab, cx: &mut Context<Self>) {
        if let Some(settings) = &mut self.project_settings {
            settings.active_tab = tab.clone();
            cx.notify();
            
            // Load data for this specific tab in background if not already loaded
            let needs_loading = match &tab {
                views::ProjectSettingsTab::General => false, // Always instant
                views::ProjectSettingsTab::GitInfo => settings.commit_count.is_none(),
                views::ProjectSettingsTab::GitCI => settings.workflow_files.is_empty(),
                views::ProjectSettingsTab::Metadata => false, // Quick to render
                views::ProjectSettingsTab::DiskInfo => settings.disk_size.is_none(),
                views::ProjectSettingsTab::Performance => settings.disk_size.is_none() || settings.git_repo_size.is_none(),
                views::ProjectSettingsTab::Integrations => settings.preferred_editor.is_none(),
            };
            
            if needs_loading {
                let project_path = settings.project_path.clone();
                let tab_for_load = tab.clone();
                let tab_for_match = tab.clone();
                
                // Load ONLY this tab's data in background thread
                cx.spawn(async move |this, cx| {
                    let loaded_data = std::thread::spawn(move || {
                        let mut temp_settings = views::ProjectSettings::new(project_path.clone(), String::new());
                        temp_settings.load_tab_data_sync(&tab_for_load);
                        temp_settings
                    })
                    .join()
                    .ok();
                    
                    if let Some(loaded) = loaded_data {
                        let _ = cx.update(|cx| {
                            let _ = this.update(cx, |screen, cx| {
                                if let Some(ref mut settings) = screen.project_settings {
                                    // Merge only the loaded data for this tab
                                    match tab_for_match {
                                        views::ProjectSettingsTab::GitInfo => {
                                            settings.git_repo_size = loaded.git_repo_size;
                                            settings.commit_count = loaded.commit_count;
                                            settings.branch_count = loaded.branch_count;
                                            settings.remote_url = loaded.remote_url;
                                            settings.last_commit_date = loaded.last_commit_date;
                                            settings.last_commit_message = loaded.last_commit_message;
                                            settings.uncommitted_changes = loaded.uncommitted_changes;
                                            settings.current_branch = loaded.current_branch;
                                            settings.stash_count = loaded.stash_count;
                                            settings.untracked_files = loaded.untracked_files;
                                        }
                                        views::ProjectSettingsTab::GitCI => {
                                            settings.workflow_files = loaded.workflow_files;
                                        }
                                        views::ProjectSettingsTab::DiskInfo => {
                                            settings.disk_size = loaded.disk_size;
                                            settings.git_repo_size = loaded.git_repo_size;
                                        }
                                        views::ProjectSettingsTab::Performance => {
                                            settings.disk_size = loaded.disk_size;
                                            settings.git_repo_size = loaded.git_repo_size;
                                        }
                                        views::ProjectSettingsTab::Integrations => {
                                            settings.preferred_editor = loaded.preferred_editor;
                                            settings.preferred_git_tool = loaded.preferred_git_tool;
                                        }
                                        _ => {}
                                    }
                                }
                                cx.notify();
                            });
                        });
                    }
                }).detach();
            }
        }
    }
    
    pub(crate) fn refresh_project_settings(&mut self, cx: &mut Context<Self>) {
        if let Some(settings) = &self.project_settings {
            let project_path = settings.project_path.clone();
            
            // Load all data asynchronously in background
            cx.spawn(async move |this, cx| {
                // Run all data loading in a background thread
                let loaded_settings = std::thread::spawn(move || {
                    views::ProjectSettings::load_all_data_async(project_path)
                })
                .join()
                .ok();
                
                if let Some(new_settings) = loaded_settings {
                    let _ = cx.update(|cx| {
                        let _ = this.update(cx, |screen, cx| {
                            if let Some(ref mut settings) = screen.project_settings {
                                // Preserve active tab
                                let active_tab = settings.active_tab.clone();
                                *settings = new_settings;
                                settings.active_tab = active_tab;
                            }
                            cx.notify();
                        });
                    });
                }
            }).detach();
        }
    }
    
    pub(crate) fn browse_project_location(&mut self, cx: &mut Context<Self>) {
        let file_dialog = rfd::AsyncFileDialog::new()
            .set_title("Choose Project Location")
            .set_directory(std::env::current_dir().unwrap_or_default());
        
        cx.spawn(async move |this, cx| {
            if let Some(folder) = file_dialog.pick_folder().await {
                cx.update(|cx| {
                    this.update(cx, |screen, cx| {
                        screen.new_project_path = Some(folder.path().to_path_buf());
                        cx.notify();
                    }).ok();
                }).ok();
            }
        }).detach();
    }
    
    pub(crate) fn create_new_project(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.new_project_name.is_empty() {
            return;
        }
        
        let name = self.new_project_name.clone();
        let base_path = self.new_project_path.clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        
        let project_path = base_path.join(&name);
        let recent_projects_path = self.recent_projects_path.clone();
        
        cx.spawn(async move |this, cx| {
            if let Err(e) = std::fs::create_dir_all(&project_path) {
                tracing::error!("Failed to create project directory: {}", e);
                return;
            }
            
            let toml_content = format!(
                r#"[project]
name = "{}"
version = "0.1.0"
engine_version = "0.1.23"

[settings]
default_scene = "scenes/main.scene"
"#,
                name
            );
            
            if let Err(e) = std::fs::write(project_path.join("Pulsar.toml"), toml_content) {
                tracing::error!("Failed to create Pulsar.toml: {}", e);
                return;
            }
            
            let dirs = ["assets", "scenes", "scripts", "prefabs"];
            for dir in dirs {
                let _ = std::fs::create_dir_all(project_path.join(dir));
            }
            
            let _ = init_repository(&project_path);
            
            let recent_project = RecentProject {
                name: name.clone(),
                path: project_path.to_string_lossy().to_string(),
                last_opened: Some(chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()),
                is_git: is_git_repo(&project_path),
            };
            
            cx.update(|cx| {
                this.update(cx, |screen, cx| {
                    screen.recent_projects.add_or_update(recent_project);
                    screen.recent_projects.save(&recent_projects_path);
                    screen.new_project_name.clear();
                    screen.view = EntryScreenView::Recent;
                    cx.emit(crate::entry_screen::project_selector::ProjectSelected { path: project_path });
                }).ok();
            }).ok();
        }).detach();
    }
}

impl EventEmitter<crate::entry_screen::project_selector::ProjectSelected> for EntryScreen {}

impl Render for EntryScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bounds = window.viewport_size();
        let cols = self.calculate_columns(bounds.width);
        let view = self.view;
        
        // Trigger git fetch when viewing recent projects
        if view == EntryScreenView::Recent && !self.is_fetching_updates {
            self.start_git_fetch_all(cx);
        }
        
        // Show dependency setup if needed
        if self.show_dependency_setup {
            return views::render_dependency_setup(self, cx).into_any_element();
        }
        
        // Show upstream prompt if needed
        if self.show_git_upstream_prompt.is_some() {
            return views::render_upstream_prompt(self, cx).into_any_element();
        }
        
        // Show project settings if needed
        if let Some(ref settings) = self.project_settings {
            return views::render_project_settings(self, settings, cx).into_any_element();
        }
        
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(TitleBar::new())
            .child(
                h_flex()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    .child(views::render_sidebar(self, cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .h_full()
                            .overflow_hidden()
                            .bg(cx.theme().background)
                            .child(
                                match view {
                                    EntryScreenView::Recent => views::render_recent_projects(self, cols, cx).into_any_element(),
                                    EntryScreenView::Templates => views::render_templates(self, cols, cx).into_any_element(),
                                    EntryScreenView::NewProject => views::render_new_project(self, cx).into_any_element(),
                                    EntryScreenView::CloneGit => views::render_clone_git(self, cx).into_any_element(),
                                }
                            )
                    )
            )
            .into_any_element()
    }
}
