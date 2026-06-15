mod git_operations;
mod integration_launcher;
pub mod project_selector;
pub mod recent_projects;
mod types;
pub mod views;
mod virtual_grid;

use git_operations::{
    add_user_upstream, check_for_updates, clone_repository, init_repository, is_git_repo,
    pull_updates, setup_template_remotes,
};
use types::{
    get_default_templates, CloneProgress, CloudProject, CloudProjectStatus, CloudServer,
    CloudServerStatus, EntryScreenView, GitFetchStatus, SharedCloneProgress, Template,
};

use gpui::StyledImage as _;
use gpui::{prelude::*, *};

/// Returns `true` when the `PULSAR_INSECURE_TLS` environment variable is set to `"1"`.
///
/// When enabled, TLS certificate verification is skipped for all outbound HTTPS
/// connections made by the entry screen (cloud server API calls, etc.).
/// This is useful for development environments that use self-signed certificates.
///
/// Defaults to `false` — TLS verification is always on for safety.
fn insecure_tls_enabled() -> bool {
    std::env::var("PULSAR_INSECURE_TLS").as_deref() == Ok("1")
}
use engine_backend::subsystems::networking::multiuser::MultiuserClient;
use parking_lot::Mutex;
use recent_projects::{RecentProject, RecentProjectsList};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::InputState,
    v_flex, ActiveTheme as _, Sizable as _, TitleBar, VirtualListScrollHandle,
};

static LOGO_PNG: &[u8] = include_bytes!("../../../../assets/images/logo_sqrkl.png");

fn decode_logo_png(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    let rgba = image::load_from_memory(bytes).ok()?.into_rgba8();
    let frame = image::Frame::new(rgba);
    Some(Arc::new(RenderImage::new(smallvec::smallvec![frame])))
}

/// EntryScreen: AAA-quality project manager
pub struct EntryScreen {
    pub(crate) entity: Option<Entity<EntryScreen>>,
    pub(crate) logo: Option<Arc<RenderImage>>,
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
    pub(crate) recent_projects_scroll_handle: VirtualListScrollHandle,
    pub(crate) templates_scroll_handle: VirtualListScrollHandle,
    pub(crate) show_dependency_setup: bool,
    pub(crate) dependency_status: Option<DependencyStatus>,
    pub(crate) install_progress: Option<InstallProgress>,
    // Input states for interactive text fields
    pub(crate) git_repo_url_input: Entity<InputState>,
    pub(crate) git_upstream_url_input: Entity<InputState>,
    pub(crate) new_project_name_input: Entity<InputState>,
    // Cloud Projects
    pub(crate) cloud_servers: Vec<CloudServer>,
    pub(crate) selected_cloud_server: Option<usize>,
    pub(crate) cloud_servers_path: std::path::PathBuf,
    pub(crate) show_add_server: bool,
    pub(crate) add_server_alias_input: Entity<InputState>,
    pub(crate) add_server_url_input: Entity<InputState>,
    pub(crate) add_server_email_input: Entity<InputState>,
    pub(crate) add_server_password_input: Entity<InputState>,
    pub(crate) add_server_alias: String,
    pub(crate) add_server_url: String,
    pub(crate) add_server_email: String,
    pub(crate) add_server_password: String,
    pub(crate) add_server_logging_in: bool,
    pub(crate) add_server_error: Option<String>,
    // Cloud project creation dialog
    pub(crate) show_create_project: bool,
    pub(crate) create_project_name: String,
    pub(crate) create_project_description: String,
    pub(crate) create_project_name_input: Entity<InputState>,
    pub(crate) create_project_description_input: Entity<InputState>,
    // Auth / identity (sign-in flow state — avatar + menu are in profile_dropdown)
    pub(crate) auth_loading: bool,
    pub(crate) auth_message: Option<String>,
    pub(crate) auth_device_code: Option<String>,
    pub(crate) auth_device_verification_url: Option<String>,
    pub(crate) auth_device_modal_visible: bool,
    pub(crate) auth_device_copy_notice: Option<String>,
    pub(crate) profile_dropdown: gpui::Entity<ui_common::ProfileDropdown>,
    pub(crate) theme_picker: gpui::Entity<ui_common::ThemePicker>,
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
        let git_repo_url_input = cx
            .new(|cx| InputState::new(_window, cx).placeholder("https://github.com/user/repo.git"));
        let git_upstream_url_input = cx.new(|cx| {
            InputState::new(_window, cx)
                .placeholder("https://github.com/your-username/your-repo.git")
        });
        let new_project_name_input =
            cx.new(|cx| InputState::new(_window, cx).placeholder("my_awesome_game"));

        // Cloud servers — load from disk
        let cloud_servers_path = directories::ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
            .map(|d| d.data_dir().join("cloud_servers.json"))
            .unwrap_or_else(|| PathBuf::from("cloud_servers.json"));
        let cloud_servers: Vec<CloudServer> = if cloud_servers_path.exists() {
            std::fs::read_to_string(&cloud_servers_path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let add_server_alias_input =
            cx.new(|cx| InputState::new(_window, cx).placeholder("My Studio Server"));
        let add_server_url_input =
            cx.new(|cx| InputState::new(_window, cx).placeholder("https://studio.example.com"));
        let add_server_email_input =
            cx.new(|cx| InputState::new(_window, cx).placeholder("email@example.com"));
        let add_server_password_input = cx.new(|cx| {
            InputState::new(_window, cx).placeholder("password")
        });
        let create_project_name_input =
            cx.new(|cx| InputState::new(_window, cx).placeholder("My Awesome Game"));
        let create_project_description_input =
            cx.new(|cx| InputState::new(_window, cx).placeholder("Optional project description"));

        let mut screen = Self {
            entity: None,
            logo: decode_logo_png(LOGO_PNG),
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
            recent_projects_scroll_handle: VirtualListScrollHandle::new(),
            templates_scroll_handle: VirtualListScrollHandle::new(),
            show_dependency_setup: false,
            dependency_status: None,
            install_progress: None,
            git_repo_url_input: git_repo_url_input.clone(),
            git_upstream_url_input: git_upstream_url_input.clone(),
            new_project_name_input: new_project_name_input.clone(),
            cloud_servers,
            selected_cloud_server: None,
            cloud_servers_path,
            show_add_server: false,
            add_server_alias_input: add_server_alias_input.clone(),
            add_server_url_input: add_server_url_input.clone(),
            add_server_email_input: add_server_email_input.clone(),
            add_server_password_input: add_server_password_input.clone(),
            add_server_alias: String::new(),
            add_server_url: String::new(),
            add_server_email: String::new(),
            add_server_password: String::new(),
            add_server_logging_in: false,
            add_server_error: None,
            show_create_project: false,
            create_project_name: String::new(),
            create_project_description: String::new(),
            create_project_name_input: create_project_name_input.clone(),
            create_project_description_input: create_project_description_input.clone(),
            auth_loading: false,
            auth_message: None,
            auth_device_code: None,
            auth_device_verification_url: None,
            auth_device_modal_visible: false,
            auth_device_copy_notice: None,
            profile_dropdown: cx.new(ui_common::ProfileDropdown::new),
            theme_picker: cx.new(|cx| ui_common::ThemePicker::new(_window, cx)),
        };

        // Restore persisted auth profile into engine context at launcher startup.
        if let Some(ec) = engine_state::EngineContext::global() {
            if ec.auth_profile().is_none() {
                if let Some(profile) = pulsar_auth::load_cached_profile() {
                    ec.set_auth_profile(profile);
                }
            }
        }
        // Subscribe to the profile dropdown so we can handle the sign-in flow
        // (which lives on EntryScreen because it involves the device-code modal).
        cx.subscribe(
            &screen.profile_dropdown,
            |this: &mut Self,
             _,
             event: &ui_common::ProfileDropdownEvent,
             cx| {
                match event {
                    ui_common::ProfileDropdownEvent::SignInRequested => {
                        this.begin_github_sign_in(cx);
                    }
                    ui_common::ProfileDropdownEvent::SignedOut => {
                        // Reset device-flow state if sign-out happened mid-flow.
                        this.auth_loading = false;
                        this.auth_device_code = None;
                        this.auth_device_verification_url = None;
                        this.auth_device_modal_visible = false;
                        this.auth_device_copy_notice = None;
                        this.auth_message = Some("Signed out".to_string());
                        cx.notify();
                    }
                }
            },
        )
        .detach();

        // Store own entity for virtualization helpers.
        screen.entity = Some(cx.entity().clone());

        // Subscribe to input events
        cx.subscribe(
            &git_repo_url_input,
            |this, _input, event: &ui::input::InputEvent, cx| {
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
            },
        )
        .detach();

        cx.subscribe(
            &git_upstream_url_input,
            |this, _input, event: &ui::input::InputEvent, cx| match event {
                ui::input::InputEvent::Change => {
                    this.git_upstream_url = this.git_upstream_url_input.read(cx).text().to_string();
                    cx.notify();
                }
                ui::input::InputEvent::PressEnter { .. } => {
                    this.setup_git_upstream(false, cx);
                }
                _ => {}
            },
        )
        .detach();

        cx.subscribe(
            &new_project_name_input,
            |this, _input, event: &ui::input::InputEvent, cx| {
                match event {
                    ui::input::InputEvent::Change => {
                        this.new_project_name =
                            this.new_project_name_input.read(cx).text().to_string();
                        cx.notify();
                    }
                    ui::input::InputEvent::PressEnter { .. } => {
                        // Note: Will need window parameter for create_new_project - will be handled via button click
                    }
                    _ => {}
                }
            },
        )
        .detach();

        cx.subscribe(
            &add_server_alias_input,
            |this, _input, event: &ui::input::InputEvent, cx| {
                if let ui::input::InputEvent::Change = event {
                    this.add_server_alias = this.add_server_alias_input.read(cx).text().to_string();
                }
            },
        )
        .detach();
        cx.subscribe(
            &add_server_url_input,
            |this, _input, event: &ui::input::InputEvent, cx| {
                if let ui::input::InputEvent::Change = event {
                    this.add_server_url = this.add_server_url_input.read(cx).text().to_string();
                }
            },
        )
        .detach();
        cx.subscribe(
            &add_server_email_input,
            |this, _input, event: &ui::input::InputEvent, cx| {
                if let ui::input::InputEvent::Change = event {
                    this.add_server_email = this.add_server_email_input.read(cx).text().to_string();
                }
            },
        )
        .detach();
        cx.subscribe(
            &add_server_password_input,
            |this, _input, event: &ui::input::InputEvent, cx| {
                if let ui::input::InputEvent::Change = event {
                    this.add_server_password = this.add_server_password_input.read(cx).text().to_string();
                }
            },
        )
        .detach();
        cx.subscribe(
            &create_project_name_input,
            |this, _input, event: &ui::input::InputEvent, cx| {
                if let ui::input::InputEvent::Change = event {
                    this.create_project_name =
                        this.create_project_name_input.read(cx).text().to_string();
                }
            },
        )
        .detach();
        cx.subscribe(
            &create_project_description_input,
            |this, _input, event: &ui::input::InputEvent, cx| {
                if let ui::input::InputEvent::Change = event {
                    this.create_project_description = this
                        .create_project_description_input
                        .read(cx)
                        .text()
                        .to_string();
                }
            },
        )
        .detach();

        // Check dependencies on background thread
        screen.check_dependencies_async(cx);

        screen
    }

    pub(crate) fn check_dependencies_async(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let status = cx
                .background_executor()
                .spawn(async {
                    use std::process::Command;

                    // Check for Rust
                    let rust_installed = Command::new("rustc").arg("--version").output().is_ok();

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
                })
                .await;

            cx.update(|cx| {
                this.update(cx, |screen, cx| {
                    let missing = !status.rust_installed || !status.build_tools_installed;
                    screen.dependency_status = Some(status);
                    screen.show_dependency_setup = missing;
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub(crate) fn start_git_fetch_all(&mut self, cx: &mut Context<Self>) {
        if self.is_fetching_updates {
            return;
        }

        self.is_fetching_updates = true;
        let git_projects: Vec<(String, String)> = self
            .recent_projects
            .projects
            .iter()
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
                let result = std::thread::spawn(move || check_for_updates(&path_buf)).join();

                // Update status
                {
                    let mut statuses_lock = statuses.lock();
                    match result {
                        Ok(Ok(0)) => {
                            statuses_lock.insert(path_clone.clone(), GitFetchStatus::UpToDate);
                        }
                        Ok(Ok(behind)) => {
                            statuses_lock.insert(
                                path_clone.clone(),
                                GitFetchStatus::UpdatesAvailable(behind),
                            );
                        }
                        Ok(Err(e)) => {
                            statuses_lock
                                .insert(path_clone.clone(), GitFetchStatus::Error(e.to_string()));
                        }
                        Err(_) => {
                            statuses_lock.insert(
                                path_clone.clone(),
                                GitFetchStatus::Error("Thread panicked".to_string()),
                            );
                        }
                    }
                }

                // Notify UI update
                cx.update(|cx| {
                    this.update(cx, |_, cx| cx.notify());
                });
            }

            // Mark fetch complete
            cx.update(|cx| {
                this.update(cx, |screen, cx| {
                    screen.is_fetching_updates = false;
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub(crate) fn pull_project_updates(&mut self, path: String, cx: &mut Context<Self>) {
        let path_buf = PathBuf::from(&path);
        let statuses = self.git_fetch_statuses.clone();

        cx.spawn(async move |this, cx| {
            let result = std::thread::spawn(move || pull_updates(&path_buf)).join();

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
                this.update(cx, |_, cx| cx.notify());
            });
        })
        .detach();
    }

    pub(crate) fn calculate_columns(&self, width: Pixels) -> usize {
        // Account for sidebar width (220px) + container padding (p_8 = 32px each side = 64px total)
        let sidebar_width = 220.0;
        let container_padding = 64.0;
        let card_width = 320.0;
        let gap_size = 24.0;

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

                let project_name = path
                    .file_name()
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
                    });
                });
            }
        })
        .detach();
    }

    pub(crate) fn clone_git_repo(
        &mut self,
        repo_url: String,
        target_name: String,
        is_template: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
                    this.update(cx, |_, cx| cx.notify());
                });

                let repo_url_clone = repo_url.clone();
                let progress_clone = progress.clone();
                let target_path_clone = target_path.clone();

                let repo_result = std::thread::spawn(move || {
                    clone_repository(repo_url_clone, target_path_clone, progress_clone)
                })
                .join();

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
                            last_opened: Some(
                                chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
                            ),
                            is_git: true,
                        };

                        let template_url = if is_template {
                            Some(repo_url.clone())
                        } else {
                            None
                        };

                        cx.update(|cx| {
                            this.update(cx, |screen, cx| {
                                screen.recent_projects.add_or_update(recent_project);
                                screen.recent_projects.save(&recent_projects_path);
                                screen.clone_progress = None;

                                // Show upstream prompt
                                screen.show_git_upstream_prompt =
                                    Some((target_path.clone(), template_url.unwrap_or_default()));

                                cx.notify();
                            });
                        });
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
                    this.update(cx, |_, cx| cx.notify());
                });
            } else {
                cx.update(|cx| {
                    this.update(cx, |screen, cx| {
                        screen.clone_progress = None;
                        cx.notify();
                    });
                });
            }
        })
        .detach();
    }

    pub(crate) fn clone_template(
        &mut self,
        template: &Template,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clone_git_repo(
            template.repo_url.clone(),
            template.name.clone(),
            true,
            window,
            cx,
        );
    }

    pub(crate) fn setup_git_upstream(&mut self, skip: bool, cx: &mut Context<Self>) {
        if let Some((project_path, _template_url)) = self.show_git_upstream_prompt.take() {
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

        let project_name = path
            .file_name()
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

    pub(crate) fn open_git_manager(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        cx.emit(GitManagerRequested { path });
    }

    pub(crate) fn open_project_settings(
        &mut self,
        project_path: PathBuf,
        project_name: String,
        cx: &mut Context<Self>,
    ) {
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

    pub(crate) fn change_project_settings_tab(
        &mut self,
        tab: views::ProjectSettingsTab,
        cx: &mut Context<Self>,
    ) {
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
                views::ProjectSettingsTab::Performance => {
                    settings.disk_size.is_none() || settings.git_repo_size.is_none()
                }
                views::ProjectSettingsTab::Integrations => settings.preferred_editor.is_none(),
            };

            if needs_loading {
                let project_path = settings.project_path.clone();
                let tab_for_load = tab.clone();
                let tab_for_match = tab.clone();

                // Load ONLY this tab's data in background thread
                cx.spawn(async move |this, cx| {
                    let loaded_data = std::thread::spawn(move || {
                        let mut temp_settings =
                            views::ProjectSettings::new(project_path.clone(), String::new());
                        temp_settings.load_tab_data_sync(&tab_for_load);
                        temp_settings
                    })
                    .join();

                    if let Ok(loaded) = loaded_data {
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
                                            settings.last_commit_message =
                                                loaded.last_commit_message;
                                            settings.uncommitted_changes =
                                                loaded.uncommitted_changes;
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
                })
                .detach();
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
                .join();

                if let Ok(new_settings) = loaded_settings {
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
            })
            .detach();
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
                    });
                });
            }
        })
        .detach();
    }

    pub(crate) fn create_new_project(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.new_project_name.is_empty() {
            return;
        }

        let name = self.new_project_name.clone();
        let base_path = self
            .new_project_path
            .clone()
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
                    cx.emit(crate::entry_screen::project_selector::ProjectSelected {
                        path: project_path,
                    });
                });
            });
        })
        .detach();
    }

    // ── Cloud Projects ────────────────────────────────────────────────────────

    /// Persist the cloud server list to disk.
    pub(crate) fn save_cloud_servers(&self) {
        if let Ok(json) = serde_json::to_string_pretty(&self.cloud_servers) {
            if let Some(parent) = self.cloud_servers_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&self.cloud_servers_path, json);
        }
    }

    /// Add a new server entry, persist it, and immediately start a connectivity poll.
    /// Authenticate against the server and add it to the cloud server list.
    /// On success the returned JWT is stored as `auth_token` and the server list
    /// is persisted so the user never needs to re-enter credentials.
    pub(crate) fn add_cloud_server(
        &mut self,
        alias: String,
        url: String,
        email: String,
        password: String,
        cx: &mut Context<Self>,
    ) {
        if url.trim().is_empty() || email.trim().is_empty() || password.is_empty() {
            self.add_server_error = Some("Email and password are required.".to_string());
            cx.notify();
            return;
        }
        self.add_server_logging_in = true;
        self.add_server_error = None;
        cx.notify();

        let raw = url.trim().trim_end_matches('/');
        let base_url = if raw.starts_with("http://") || raw.starts_with("https://") {
            raw.to_string()
        } else {
            format!("http://{}", raw)
        };
        let login_url = format!("{}/api/v1/auth/login", base_url);
        let login_body = serde_json::json!({ "email": email, "password": password });

        let (tx, rx) = smol::channel::bounded::<Option<String>>(1);
        std::thread::spawn(move || {
            if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                let result = rt.block_on(async move {
                    let client = reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(10))
                        .danger_accept_invalid_certs(false)
                        .build()
                        .ok()?;
                    let resp = client.post(&login_url).json(&login_body).send().await.ok()?;
                    if !resp.status().is_success() {
                        return None;
                    }
                    let data: serde_json::Value = resp.json().await.ok()?;
                    data.get("token")?.as_str().map(|s| s.to_string())
                });
                smol::block_on(tx.send(result));
            } else {
                smol::block_on(tx.send(None));
            }
        });

        let id = format!(
            "{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let alias = if alias.trim().is_empty() {
            url.clone()
        } else {
            alias
        };

        cx.spawn(async move |this, cx| {
            let token = rx.recv().await.unwrap_or(None);
            cx.update(|cx| {
                this.update(cx, |screen, cx| {
                    screen.add_server_logging_in = false;
                    match token {
                        Some(jwt) => {
                            let server = CloudServer {
                                id,
                                alias: alias.clone(),
                                url: base_url.clone(),
                                auth_token: jwt,
                                status: CloudServerStatus::Unknown,
                                projects: Vec::new(),
                            };
                            screen.cloud_servers.push(server);
                            screen.save_cloud_servers();
                            screen.show_add_server = false;
                            let new_idx = screen.cloud_servers.len() - 1;
                            screen.refresh_cloud_server(new_idx, cx);
                        }
                        None => {
                            screen.add_server_error = Some(
                                "Login failed. Check your email/password and server URL.".to_string(),
                            );
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Remove a server entry by index and persist.
    pub(crate) fn remove_cloud_server(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx >= self.cloud_servers.len() {
            return;
        }
        self.cloud_servers.remove(idx);
        // Adjust selection
        match self.selected_cloud_server {
            Some(sel) if sel == idx => self.selected_cloud_server = None,
            Some(sel) if sel > idx => self.selected_cloud_server = Some(sel - 1),
            _ => {}
        }
        self.save_cloud_servers();
        cx.notify();
    }

    /// Poll a single server for connectivity info and project list.
    pub(crate) fn refresh_cloud_server(&mut self, server_idx: usize, cx: &mut Context<Self>) {
        if server_idx >= self.cloud_servers.len() {
            return;
        }
        // Normalise URL: add http:// if no scheme is provided so bare
        // "localhost:7700" entries work without the user having to type a scheme.
        let raw = self.cloud_servers[server_idx]
            .url
            .trim()
            .trim_end_matches('/');
        let base_url = if raw.starts_with("http://") || raw.starts_with("https://") {
            raw.to_string()
        } else {
            format!("http://{}", raw)
        };
        let token = self.cloud_servers[server_idx].auth_token.clone();

        self.cloud_servers[server_idx].status = CloudServerStatus::Connecting;
        cx.notify();

        // Use a separate OS thread with its own tokio runtime so that reqwest
        // futures (which require tokio) are not run inside GPUI's smol executor.
        let (tx, rx) = smol::channel::bounded::<Option<(CloudServerStatus, Vec<CloudProject>)>>(1);
        std::thread::spawn(move || {
            let result = fetch_cloud_server_info(base_url, token);
            smol::block_on(tx.send(result));
        });

        cx.spawn(async move |this, cx| {
            if let Ok(result) = rx.recv().await {
                cx.update(|cx| {
                    this.update(cx, |screen, cx| {
                        if server_idx < screen.cloud_servers.len() {
                            match result {
                                Some((s, p)) => {
                                    screen.cloud_servers[server_idx].status = s;
                                    screen.cloud_servers[server_idx].projects = p;
                                }
                                None => {
                                    screen.cloud_servers[server_idx].status =
                                        CloudServerStatus::Offline;
                                }
                            }
                        }
                        cx.notify();
                    });
                });
            }
        })
        .detach();
    }

    /// Refresh every server that hasn't been polled yet (status == Unknown).
    pub(crate) fn refresh_all_unknown_cloud_servers(&mut self, cx: &mut Context<Self>) {
        let indices: Vec<usize> = self
            .cloud_servers
            .iter()
            .enumerate()
            .filter(|(_, s)| s.status == CloudServerStatus::Unknown)
            .map(|(i, _)| i)
            .collect();
        for idx in indices {
            self.refresh_cloud_server(idx, cx);
        }
    }

    /// Signal a specific workspace on a server to warm up (prepare), then refresh.
    pub(crate) fn prepare_cloud_project(
        &mut self,
        server_idx: usize,
        workspace_id: String,
        cx: &mut Context<Self>,
    ) {
        if server_idx >= self.cloud_servers.len() {
            return;
        }
        let raw = self.cloud_servers[server_idx]
            .url
            .trim()
            .trim_end_matches('/');
        let base_url = if raw.starts_with("http://") || raw.starts_with("https://") {
            raw.to_string()
        } else {
            format!("http://{}", raw)
        };
        let token = self.cloud_servers[server_idx].auth_token.clone();
        let post_url = format!("{}/api/v1/workspaces/{}/prepare", base_url, workspace_id);

        // Optimistic status update
        if let Some(proj) = self.cloud_servers[server_idx]
            .projects
            .iter_mut()
            .find(|p| p.id == workspace_id)
        {
            proj.status = CloudProjectStatus::Preparing;
        }
        cx.notify();

        let tok: Option<String> = if token.is_empty() { None } else { Some(token) };
        let (tx, rx) = smol::channel::bounded::<()>(1);
        std::thread::spawn(move || {
            if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                rt.block_on(async move {
                    let Ok(client) = reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(10))
                        .danger_accept_invalid_certs(insecure_tls_enabled())
                        .build()
                    else {
                        return;
                    };
                    let req = client.post(&post_url);
                    let req = if let Some(ref t) = tok {
                        req.bearer_auth(t)
                    } else {
                        req
                    };
                    let _ = req.send().await;
                });
            }
            smol::block_on(tx.send(()));
        });

        cx.spawn(async move |this, cx| {
            rx.recv().await;
            // Refresh server state after prepare to pick up new status.
            cx.update(|cx| {
                this.update(cx, |screen, cx| {
                    screen.refresh_cloud_server(server_idx, cx);
                });
            });
        })
        .detach();
    }

    /// Create a new project on a remote server via `POST /api/v1/workspaces`, then refresh.
    pub(crate) fn create_cloud_project(
        &mut self,
        server_idx: usize,
        name: String,
        description: String,
        cx: &mut Context<Self>,
    ) {
        if server_idx >= self.cloud_servers.len() || name.trim().is_empty() {
            return;
        }
        let raw = self.cloud_servers[server_idx]
            .url
            .trim()
            .trim_end_matches('/');
        let base_url = if raw.starts_with("http://") || raw.starts_with("https://") {
            raw.to_string()
        } else {
            format!("http://{}", raw)
        };
        let token = self.cloud_servers[server_idx].auth_token.clone();
        let post_url = format!("{}/api/v1/workspaces", base_url);

        self.show_create_project = false;
        cx.notify();

        let tok: Option<String> = if token.is_empty() { None } else { Some(token) };
        let (tx, rx) = smol::channel::bounded::<()>(1);
        std::thread::spawn(move || {
            if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                rt.block_on(async move {
                    let Ok(client) = reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(10))
                        .danger_accept_invalid_certs(insecure_tls_enabled())
                        .build()
                    else {
                        return;
                    };
                    let body = serde_json::json!({ "name": name, "description": description });
                    let req = client.post(&post_url).json(&body);
                    let req = if let Some(ref t) = tok {
                        req.bearer_auth(t)
                    } else {
                        req
                    };
                    let _ = req.send().await;
                });
            }
            smol::block_on(tx.send(()));
        });

        cx.spawn(async move |this, cx| {
            rx.recv().await;
            // Refresh to surface the newly created project.
            cx.update(|cx| {
                this.update(cx, |screen, cx| {
                    screen.refresh_cloud_server(server_idx, cx);
                });
            });
        })
        .detach();
    }

    /// Send `DELETE /api/v1/workspaces/{id}` to the server, then refresh.
    pub(crate) fn delete_cloud_project(
        &mut self,
        server_idx: usize,
        workspace_id: String,
        cx: &mut Context<Self>,
    ) {
        if server_idx >= self.cloud_servers.len() {
            return;
        }
        let raw = self.cloud_servers[server_idx]
            .url
            .trim()
            .trim_end_matches('/');
        let base_url = if raw.starts_with("http://") || raw.starts_with("https://") {
            raw.to_string()
        } else {
            format!("http://{}", raw)
        };
        let token = self.cloud_servers[server_idx].auth_token.clone();
        let delete_url = format!("{}/api/v1/workspaces/{}", base_url, workspace_id);

        let (tx, rx) = smol::channel::bounded::<()>(1);
        std::thread::spawn(move || {
            if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                rt.block_on(async move {
                    let Ok(client) = reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(10))
                        .danger_accept_invalid_certs(insecure_tls_enabled())
                        .build()
                    else {
                        return;
                    };
                    let req = client.delete(&delete_url);
                    let req = if token.is_empty() {
                        req
                    } else {
                        req.bearer_auth(&token)
                    };
                    let _ = req.send().await;
                });
            }
            smol::block_on(tx.send(()));
        });

        cx.spawn(async move |this, cx| {
            rx.recv().await;
            cx.update(|cx| {
                this.update(cx, |screen, cx| {
                    screen.refresh_cloud_server(server_idx, cx);
                });
            });
        })
        .detach();
    }

    /// Send `POST /api/v1/workspaces/{id}/stop` to the server, then refresh.
    pub(crate) fn stop_cloud_project(
        &mut self,
        server_idx: usize,
        workspace_id: String,
        cx: &mut Context<Self>,
    ) {
        if server_idx >= self.cloud_servers.len() {
            return;
        }
        let raw = self.cloud_servers[server_idx]
            .url
            .trim()
            .trim_end_matches('/');
        let base_url = if raw.starts_with("http://") || raw.starts_with("https://") {
            raw.to_string()
        } else {
            format!("http://{}", raw)
        };
        let token = self.cloud_servers[server_idx].auth_token.clone();
        let stop_url = format!("{}/api/v1/workspaces/{}/stop", base_url, workspace_id);

        if let Some(proj) = self.cloud_servers[server_idx]
            .projects
            .iter_mut()
            .find(|p| p.id == workspace_id)
        {
            proj.status = CloudProjectStatus::Idle;
        }
        cx.notify();

        let (tx, rx) = smol::channel::bounded::<()>(1);
        std::thread::spawn(move || {
            if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                rt.block_on(async move {
                    let Ok(client) = reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(10))
                        .danger_accept_invalid_certs(insecure_tls_enabled())
                        .build()
                    else {
                        return;
                    };
                    let req = client.post(&stop_url);
                    let req = if token.is_empty() {
                        req
                    } else {
                        req.bearer_auth(&token)
                    };
                    let _ = req.send().await;
                });
            }
            smol::block_on(tx.send(()));
        });

        cx.spawn(async move |this, cx| {
            rx.recv().await;
            cx.update(|cx| {
                this.update(cx, |screen, cx| {
                    screen.refresh_cloud_server(server_idx, cx);
                });
            });
        })
        .detach();
    }

    /// Open a Running cloud workspace in the editor by emitting a ProjectSelected event.
    pub(crate) fn open_cloud_project(
        &mut self,
        server_idx: usize,
        workspace_id: String,
        cx: &mut Context<Self>,
    ) {
        if server_idx >= self.cloud_servers.len() {
            return;
        }

        let base_url = {
            let raw = self.cloud_servers[server_idx]
                .url
                .trim()
                .trim_end_matches('/');
            if raw.starts_with("http://") || raw.starts_with("https://") {
                raw.to_string()
            } else {
                format!("http://{}", raw)
            }
        };

        let auth_token = {
            let t = self.cloud_servers[server_idx].auth_token.trim().to_string();
            if t.is_empty() {
                None
            } else {
                Some(t)
            }
        };

        // ── Set up remote virtual filesystem ─────────────────────────────────
        let remote_config = engine_fs::RemoteConfig {
            server_url: base_url.clone(),
            workspace_id: workspace_id.clone(),
            auth_token: auth_token.clone(),
        };
        engine_fs::virtual_fs::set_provider(std::sync::Arc::new(engine_fs::RemoteFsProvider::new(
            remote_config,
        )));
        tracing::info!(
            "🌐 RemoteFsProvider configured for workspace '{}' at {}",
            workspace_id,
            base_url
        );

        // ── Record connection details in engine state ─────────────────────────
        let ctx = engine_state::MultiuserContext::new_cloud_project(
            base_url.clone(),
            workspace_id.clone(),
            "local",  // peer_id (populated later on WS connect)
            "remote", // host_peer_id
        )
        .with_status(engine_state::MultiuserStatus::Connecting)
        .with_workspace_id(workspace_id.clone());

        let ctx = if let Some(ref t) = auth_token {
            ctx.with_auth_token(t.clone())
        } else {
            ctx
        };

        if let Some(ec) = engine_state::EngineContext::global() {
            ec.set_multiuser(ctx);
        } else {
            tracing::warn!("EngineContext not initialized; multiuser context not stored");
        }

        // Encode as a virtual cloud path that the editor can parse.
        let virtual_path = PathBuf::from(format!(
            "cloud+pulsar://{}/{}",
            base_url
                .trim_start_matches("http://")
                .trim_start_matches("https://"),
            workspace_id
        ));
        cx.emit(crate::entry_screen::project_selector::ProjectSelected { path: virtual_path });

        // ── Connect to the Studio session WebSocket in background ──────────
        let ws_url = base_url.clone();
        let wid = workspace_id.clone();
        let token = auth_token.clone().unwrap_or_default();
        // Use the auth token as username if available; fall back to "editor"
        let user = "editor".to_string();
        std::thread::spawn(move || {
            if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                rt.block_on(async move {
                    let mut client = MultiuserClient::new(ws_url.clone());
                    match client.connect_to_workspace(wid, token, user).await {
                        Ok(mut event_rx) => {
                            tracing::info!("Connected to Studio workspace session");
                            // Update EngineContext status on join
                            if let Some(ec) = engine_state::EngineContext::global() {
                                let _ = ec.update_multiuser(|mu| {
                                    mu.set_status(engine_state::MultiuserStatus::Connected {
                                        relay_mode: None,
                                    });
                                });
                                ec.notify_multiuser_changed();
                            }
                            // Keep connection alive by receiving events
                            while event_rx.recv().await.is_some() {}
                        }
                        Err(e) => {
                            tracing::error!("Failed to connect to workspace session: {e}");
                            if let Some(ec) = engine_state::EngineContext::global() {
                                let _ = ec.update_multiuser(|mu| {
                                    mu.set_status(engine_state::MultiuserStatus::Error(
                                        e.to_string(),
                                    ));
                                });
                                ec.notify_multiuser_changed();
                            }
                        }
                    }
                });
            }
        });
    }

    pub(crate) fn auth_profile(&self) -> Option<engine_state::AuthProfile> {
        engine_state::EngineContext::global().and_then(|ec| ec.auth_profile())
    }

    pub(crate) fn begin_github_sign_in(&mut self, cx: &mut Context<Self>) {
        if self.auth_loading {
            return;
        }

        let Some(client_id) = pulsar_auth::github_client_id_from_env() else {
            self.auth_message =
                Some("Set PULSAR_GITHUB_CLIENT_ID to enable GitHub sign-in.".to_string());
            cx.notify();
            return;
        };

        self.auth_loading = true;
        self.auth_message = Some("Starting GitHub sign-in…".to_string());
        self.auth_device_code = None;
        self.auth_device_verification_url = None;
        self.auth_device_modal_visible = false;
        self.auth_device_copy_notice = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let client_id_start = client_id.clone();
            let flow = cx
                .background_executor()
                .spawn(async move { pulsar_auth::start_device_flow(&client_id_start) })
                .await;

            let Ok(flow) = flow else {
                cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        this.auth_loading = false;
                        this.auth_message = Some("Failed to start GitHub device flow.".to_string());
                        cx.notify();
                    });
                });
                return;
            };

            cx.update(|cx| {
                this.update(cx, |this, cx| {
                    this.auth_message = Some(format!("Complete sign-in in GitHub."));
                    this.auth_device_code = Some(flow.user_code.clone());
                    this.auth_device_verification_url = Some(flow.verification_uri.clone());
                    this.auth_device_modal_visible = true;
                    this.auth_device_copy_notice = None;
                    cx.open_url(&flow.verification_uri);
                    cx.notify();
                });
            });

            let client_id_poll = client_id.clone();
            let token = cx
                .background_executor()
                .spawn(
                    async move { pulsar_auth::wait_for_device_flow_token(&client_id_poll, &flow) },
                )
                .await;

            let Ok(token) = token else {
                cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        this.auth_loading = false;
                        this.auth_device_modal_visible = false;
                        this.auth_device_code = None;
                        this.auth_device_verification_url = None;
                        this.auth_device_copy_notice = None;
                        this.auth_message = Some("GitHub sign-in timed out or failed.".to_string());
                        cx.notify();
                    });
                });
                return;
            };

            let profile = cx
                .background_executor()
                .spawn({
                    let token_for_profile = token.clone();
                    async move {
                        let profile = pulsar_auth::fetch_profile(&token_for_profile)
                            .map_err(|e| e.to_string())?;
                        pulsar_auth::store_access_token(&token_for_profile)
                            .map_err(|e| e.to_string())?;
                        pulsar_auth::save_cached_profile(&profile).map_err(|e| e.to_string())?;
                        Ok::<_, String>(profile)
                    }
                })
                .await;

            cx.update(|cx| {
                this.update(cx, |this, cx| {
                    this.auth_loading = false;
                    this.auth_device_modal_visible = false;
                    this.auth_device_code = None;
                    this.auth_device_verification_url = None;
                    this.auth_device_copy_notice = None;
                    match profile {
                        Ok(profile) => {
                            if let Some(ec) = engine_state::EngineContext::global() {
                                ec.set_auth_profile(profile.clone());
                            }
                            this.auth_message = Some(format!("Signed in as @{}", profile.login));
                            // Reset the dropdown's avatar cache so it reloads with
                            // the new profile's avatar.
                            this.profile_dropdown.update(cx, |pd, cx| {
                                pd.avatar_url_loaded = None;
                                pd.avatar_image = None;
                                pd.ensure_avatar_loaded(cx);
                            });
                        }
                        Err(err) => {
                            this.auth_message = Some(format!("GitHub sign-in failed: {err}"));
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn render_github_code_modal(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(code) = self.auth_device_code.clone() else {
            return div().into_any_element();
        };
        let verification_url = self.auth_device_verification_url.clone();

        div()
            .absolute()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(cx.theme().background.opacity(0.86))
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(460.))
                    .p_6()
                    .gap_4()
                    .rounded_xl()
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().background)
                    .shadow_lg()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(cx.theme().foreground)
                            .child("GitHub Device Code"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("Paste this 8-digit code in the browser window GitHub opened."),
                    )
                    .child(
                        div()
                            .w_full()
                            .py_3()
                            .rounded_lg()
                            .bg(cx.theme().accent.opacity(0.12))
                            .border_1()
                            .border_color(cx.theme().accent.opacity(0.35))
                            .text_center()
                            .text_2xl()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(cx.theme().foreground)
                            .child(code.clone()),
                    )
                    .when_some(self.auth_device_copy_notice.clone(), |this, notice| {
                        this.child(div().text_xs().text_color(cx.theme().success).child(notice))
                    })
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("github-device-code-close")
                                    .ghost()
                                    .label("Close")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.auth_device_modal_visible = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("github-device-code-open")
                                    .ghost()
                                    .label("Open GitHub")
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        if let Some(url) = verification_url.clone() {
                                            cx.open_url(&url);
                                        }
                                    })),
                            )
                            .child(
                                Button::new("github-device-code-copy")
                                    .primary()
                                    .label("Copy Code")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                            code.clone(),
                                        ));
                                        this.auth_device_copy_notice =
                                            Some("Code copied.".to_string());
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }
}

/// Synchronously fetch server connectivity info and project list.
/// Creates its own single-threaded tokio runtime so reqwest futures (which
/// require a tokio context) are not run inside GPUI's smol-based executor.
fn fetch_cloud_server_info(
    base_url: String,
    token: String,
) -> Option<(CloudServerStatus, Vec<CloudProject>)> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    rt.block_on(async move {
        let info_url = format!("{}/api/v1/info", base_url);
        let projects_url = format!("{}/api/v1/workspaces", base_url);
        let tok: Option<String> = if token.is_empty() { None } else { Some(token) };

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(6))
            .danger_accept_invalid_certs(insecure_tls_enabled())
            .build()
            .ok()?;

        let started = std::time::Instant::now();

        // ── Fetch /api/v1/info ──
        let info_req = {
            let r = client.get(&info_url);
            if let Some(ref t) = tok {
                r.bearer_auth(t)
            } else {
                r
            }
        };
        let status: CloudServerStatus = match info_req.send().await {
            Err(_) => return Some((CloudServerStatus::Offline, vec![])),
            Ok(resp) if resp.status() == reqwest::StatusCode::UNAUTHORIZED => {
                return Some((CloudServerStatus::Unauthorized, vec![]));
            }
            Ok(resp) if !resp.status().is_success() => {
                return Some((CloudServerStatus::Offline, vec![]));
            }
            Ok(resp) => {
                let latency_ms = started.elapsed().as_millis() as u32;
                let info = resp.json::<serde_json::Value>().await.ok()?;
                CloudServerStatus::Online {
                    latency_ms,
                    version: info
                        .get("version")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?")
                        .to_string(),
                    active_users: info
                        .get("active_users")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32,
                    active_projects: info
                        .get("active_workspaces")
                        .or_else(|| info.get("active_projects"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32,
                }
            }
        };

        // ── Fetch /api/v1/workspaces ──
        let proj_req = {
            let r = client.get(&projects_url);
            if let Some(ref t) = tok {
                r.bearer_auth(t)
            } else {
                r
            }
        };
        let projects: Vec<CloudProject> = match proj_req.send().await {
            Ok(resp) if resp.status().is_success() => resp
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|v| {
                    if let serde_json::Value::Array(arr) = v {
                        Some(arr)
                    } else {
                        None
                    }
                })
                .map(|arr| {
                    arr.into_iter()
                        .filter_map(|p| {
                            let id = p.get("id")?.as_str()?.to_string();
                            let name = p.get("name")?.as_str()?.to_string();
                            let description = p
                                .get("description")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let last_modified = p
                                .get("updated_at")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let size_bytes =
                                p.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0);
                            let owner = p
                                .get("owner_name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let user_count =
                                p.get("active_users").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                            let project_status =
                                match p.get("status").and_then(|v| v.as_str()).unwrap_or("idle") {
                                    "preparing" => CloudProjectStatus::Preparing,
                                    "running" => CloudProjectStatus::Running { user_count },
                                    "error" => CloudProjectStatus::Error(
                                        p.get("error_msg")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("unknown")
                                            .to_string(),
                                    ),
                                    _ => CloudProjectStatus::Idle,
                                };
                            Some(CloudProject {
                                id,
                                name,
                                description,
                                status: project_status,
                                last_modified,
                                size_bytes,
                                owner,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
            _ => vec![],
        };

        Some((status, projects))
    })
}

impl EventEmitter<crate::entry_screen::project_selector::ProjectSelected> for EntryScreen {}

/// Event emitted when git manager is requested
#[derive(Clone, Debug)]
pub struct GitManagerRequested {
    pub path: PathBuf,
}

impl EventEmitter<GitManagerRequested> for EntryScreen {}

/// Event emitted when the user wants to open global settings (from entry screen)
#[derive(Clone, Debug)]
pub struct SettingsRequested;

impl EventEmitter<SettingsRequested> for EntryScreen {}

/// Event emitted when the user wants to open the FAB asset marketplace search
#[derive(Clone, Debug)]
pub struct FabSearchRequested;

impl EventEmitter<FabSearchRequested> for EntryScreen {}

impl Render for EntryScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bounds = window.viewport_size();
        // Account for 220px sidebar + 64px container padding (p_8 = 32px each side)
        let width: f32 = f32::from(bounds.width);
        let available_width: f32 = (width - 220.0 - 64.0).max(0.0);
        let view = self.view;

        // Trigger git fetch when viewing recent projects
        if view == EntryScreenView::Recent && !self.is_fetching_updates {
            self.start_git_fetch_all(cx);
        }
        // Refresh cloud servers that haven't been polled yet
        if view == EntryScreenView::CloudProjects {
            self.refresh_all_unknown_cloud_servers(cx);
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
            .child(
                TitleBar::new().child(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .px_2()
                        .gap_1()
                        .items_center()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child({
                            let theme_picker = self.theme_picker.clone();
                            ui::popover::Popover::<ui_common::ThemePicker>::new(
                                "entry-theme-picker",
                            )
                            .anchor(gpui::Corner::BottomRight)
                            .trigger(
                                Button::new("entry-theme-picker-btn")
                                    .icon(ui::IconName::Palette)
                                    .small()
                                    .ghost()
                                    .tooltip("Switch theme"),
                            )
                            .content(move |_, _| theme_picker.clone())
                        })
                        .child(self.profile_dropdown.clone()),
                ),
            )
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
                            .child(match view {
                                EntryScreenView::Recent => {
                                    views::render_recent_projects(self, available_width, cx)
                                        .into_any_element()
                                }
                                EntryScreenView::Templates => {
                                    views::render_templates(self, available_width, cx)
                                        .into_any_element()
                                }
                                EntryScreenView::NewProject => {
                                    views::render_new_project(self, cx).into_any_element()
                                }
                                EntryScreenView::CloneGit => {
                                    views::render_clone_git(self, cx).into_any_element()
                                }
                                EntryScreenView::CloudProjects => {
                                    views::render_cloud_projects(self, cx).into_any_element()
                                }
                            }),
                    ),
            )
            .when(self.auth_device_modal_visible, |this| {
                this.child(self.render_github_code_modal(cx))
            })
            .into_any_element()
    }
}
