use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use gpui::*;
use parking_lot::Mutex;

use crate::core::events::*;
use crate::core::types::*;
use crate::service::auth_service::AuthService;
use crate::service::cloud_service::CloudService;
use crate::service::dependency_service::DependencyService;
use crate::service::git_service::GitService;
use crate::service::plugin_service::PluginService;
use crate::service::project_service::ProjectService;
use crate::service::thumbnail_service::ThumbnailService;

/// Focused sub-state: navigation & UI flags
pub struct UiState {
    pub view: EntryScreenView,
    pub launched: bool,
    pub show_onboarding: bool,
    pub show_dependency_setup: bool,
    pub onboarding_tab: OnboardingTab,
    pub show_git_upstream_prompt: Option<(PathBuf, String)>,
    pub project_settings: Option<crate::screen::views::project_settings::ProjectSettings>,
    pub auth_device_modal_visible: bool,
    pub auth_device_copy_notice: Option<String>,
    pub show_add_server: bool,
    pub show_create_project: bool,
}

impl UiState {
    pub fn new() -> Self {
        Self {
            view: EntryScreenView::Recent,
            launched: false,
            show_onboarding: false,
            show_dependency_setup: false,
            onboarding_tab: OnboardingTab::default(),
            show_git_upstream_prompt: None,
            project_settings: None,
            auth_device_modal_visible: false,
            auth_device_copy_notice: None,
            show_add_server: false,
            show_create_project: false,
        }
    }
}

/// Focused sub-state: all input entities
pub struct InputEntities {
    pub git_repo_url: Entity<ui::input::InputState>,
    pub git_upstream_url: Entity<ui::input::InputState>,
    pub new_project_name: Entity<ui::input::InputState>,
    pub add_server_alias: Entity<ui::input::InputState>,
    pub add_server_url: Entity<ui::input::InputState>,
    pub add_server_email: Entity<ui::input::InputState>,
    pub add_server_password: Entity<ui::input::InputState>,
    pub create_project_name: Entity<ui::input::InputState>,
    pub create_project_description: Entity<ui::input::InputState>,
    pub plugin_search: Entity<ui::input::InputState>,
}

impl InputEntities {
    pub fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            git_repo_url: cx.new(|cx| {
                ui::input::InputState::new(window, cx)
                    .placeholder("https://github.com/user/repo.git")
            }),
            git_upstream_url: cx.new(|cx| {
                ui::input::InputState::new(window, cx)
                    .placeholder("https://github.com/your-username/your-repo.git")
            }),
            new_project_name: cx
                .new(|cx| ui::input::InputState::new(window, cx).placeholder("my_awesome_game")),
            add_server_alias: cx
                .new(|cx| ui::input::InputState::new(window, cx).placeholder("My Studio Server")),
            add_server_url: cx.new(|cx| {
                ui::input::InputState::new(window, cx).placeholder("https://studio.example.com")
            }),
            add_server_email: cx
                .new(|cx| ui::input::InputState::new(window, cx).placeholder("email@example.com")),
            add_server_password: cx
                .new(|cx| ui::input::InputState::new(window, cx).placeholder("password")),
            create_project_name: cx
                .new(|cx| ui::input::InputState::new(window, cx).placeholder("My Awesome Game")),
            create_project_description: cx.new(|cx| {
                ui::input::InputState::new(window, cx).placeholder("Optional project description")
            }),
            plugin_search: cx.new(|cx| {
                ui::input::InputState::new(window, cx).placeholder("Search plugins\u{2026}")
            }),
        }
    }

    pub fn subscribe_all(&self, screen: Entity<crate::screen::EntryScreen>, cx: &mut App) {
        use ui::input::InputEvent;
        let s = screen.clone();
        let s1 = screen.clone();
        cx.subscribe(
            &self.git_repo_url,
            move |_: Entity<ui::input::InputState>, ev: &InputEvent, cx: &mut App| {
                if let InputEvent::Change = ev {
                    s1.update(cx, |this, cx| {
                        this.state.input.git_repo_url_text =
                            this.inputs().git_repo_url.read(cx).text().to_string();
                        cx.notify();
                    });
                }
            },
        )
        .detach();
        let s2 = screen.clone();
        cx.subscribe(
            &self.git_upstream_url,
            move |_: Entity<ui::input::InputState>, ev: &InputEvent, cx: &mut App| match ev {
                InputEvent::Change => {
                    s2.update(cx, |this, cx| {
                        this.state.input.git_upstream_url_text =
                            this.inputs().git_upstream_url.read(cx).text().to_string();
                        cx.notify();
                    });
                }
                InputEvent::PressEnter { .. } => {
                    s2.update(cx, |this, cx| {
                        this.state.ui.show_git_upstream_prompt.take();
                        cx.notify();
                    });
                }
                _ => {}
            },
        )
        .detach();
        let s3 = screen.clone();
        cx.subscribe(
            &self.new_project_name,
            move |_: Entity<ui::input::InputState>, ev: &InputEvent, cx: &mut App| {
                if let InputEvent::Change = ev {
                    s3.update(cx, |this, cx| {
                        this.state.input.new_project_name_text =
                            this.inputs().new_project_name.read(cx).text().to_string();
                        cx.notify();
                    });
                }
            },
        )
        .detach();
        let s4 = screen.clone();
        cx.subscribe(
            &self.add_server_alias,
            move |_: Entity<ui::input::InputState>, ev: &InputEvent, cx: &mut App| {
                if let InputEvent::Change = ev {
                    s4.update(cx, |this, cx| {
                        this.state.input.add_server_alias_text =
                            this.inputs().add_server_alias.read(cx).text().to_string();
                        cx.notify();
                    });
                }
            },
        )
        .detach();
        let s5 = screen.clone();
        cx.subscribe(
            &self.add_server_url,
            move |_: Entity<ui::input::InputState>, ev: &InputEvent, cx: &mut App| {
                if let InputEvent::Change = ev {
                    s5.update(cx, |this, cx| {
                        this.state.input.add_server_url_text =
                            this.inputs().add_server_url.read(cx).text().to_string();
                        cx.notify();
                    });
                }
            },
        )
        .detach();
        let s6 = screen.clone();
        cx.subscribe(
            &self.add_server_email,
            move |_: Entity<ui::input::InputState>, ev: &InputEvent, cx: &mut App| {
                if let InputEvent::Change = ev {
                    s6.update(cx, |this, cx| {
                        this.state.input.add_server_email_text =
                            this.inputs().add_server_email.read(cx).text().to_string();
                        cx.notify();
                    });
                }
            },
        )
        .detach();
        let s7 = screen.clone();
        cx.subscribe(
            &self.add_server_password,
            move |_: Entity<ui::input::InputState>, ev: &InputEvent, cx: &mut App| {
                if let InputEvent::Change = ev {
                    s7.update(cx, |this, cx| {
                        this.state.input.add_server_password_text = this
                            .inputs()
                            .add_server_password
                            .read(cx)
                            .text()
                            .to_string();
                        cx.notify();
                    });
                }
            },
        )
        .detach();
        let s8 = screen.clone();
        cx.subscribe(
            &self.create_project_name,
            move |_: Entity<ui::input::InputState>, ev: &InputEvent, cx: &mut App| {
                if let InputEvent::Change = ev {
                    s8.update(cx, |this, cx| {
                        this.state.input.create_project_name_text = this
                            .inputs()
                            .create_project_name
                            .read(cx)
                            .text()
                            .to_string();
                        cx.notify();
                    });
                }
            },
        )
        .detach();
        let s9 = screen.clone();
        cx.subscribe(
            &self.create_project_description,
            move |_: Entity<ui::input::InputState>, ev: &InputEvent, cx: &mut App| {
                if let InputEvent::Change = ev {
                    s9.update(cx, |this, cx| {
                        this.state.input.create_project_description_text = this
                            .inputs()
                            .create_project_description
                            .read(cx)
                            .text()
                            .to_string();
                        cx.notify();
                    });
                }
            },
        )
        .detach();
        let s10 = screen.clone();
        cx.subscribe(
            &self.plugin_search,
            move |_: Entity<ui::input::InputState>, ev: &InputEvent, cx: &mut App| {
                if let InputEvent::Change = ev {
                    s10.update(cx, |this, cx| {
                        this.state.input.plugin_search_query =
                            this.inputs().plugin_search.read(cx).text().to_string();
                        cx.notify();
                    });
                }
            },
        )
        .detach();
    }
}

/// Aggregate input string values (mirrors text in InputState entities)
pub struct InputValues {
    pub git_repo_url_text: String,
    pub git_upstream_url_text: String,
    pub new_project_name_text: String,
    pub new_project_path: Option<PathBuf>,
    pub add_server_alias_text: String,
    pub add_server_url_text: String,
    pub add_server_email_text: String,
    pub add_server_password_text: String,
    pub create_project_name_text: String,
    pub create_project_description_text: String,
    pub plugin_search_query: String,
}

impl InputValues {
    pub fn new() -> Self {
        Self {
            git_repo_url_text: String::new(),
            git_upstream_url_text: String::new(),
            new_project_name_text: String::new(),
            new_project_path: None,
            add_server_alias_text: String::new(),
            add_server_url_text: String::new(),
            add_server_email_text: String::new(),
            add_server_password_text: String::new(),
            create_project_name_text: String::new(),
            create_project_description_text: String::new(),
            plugin_search_query: String::new(),
        }
    }
}

/// Auth-specific state
pub struct AuthState {
    pub loading: bool,
    pub message: Option<String>,
    pub device_code: Option<String>,
    pub device_verification_url: Option<String>,
    pub profile_dropdown: gpui::Entity<ui_common::ProfileDropdown>,
    pub onboarding_avatar: Option<Arc<RenderImage>>,
    pub onboarding_avatar_url: Option<String>,
}

impl AuthState {
    pub fn new(cx: &mut App) -> Self {
        Self {
            loading: false,
            message: None,
            device_code: None,
            device_verification_url: None,
            profile_dropdown: cx.new(ui_common::ProfileDropdown::new),
            onboarding_avatar: None,
            onboarding_avatar_url: None,
        }
    }
}

/// Top-level application state (replaces the 50+ field EntryScreen)
pub struct AppState {
    pub logo: Option<Arc<RenderImage>>,
    pub recent_projects: crate::service::project_service::RecentProjectsList,
    pub recent_projects_path: PathBuf,
    pub templates: Vec<Template>,

    pub ui: UiState,
    pub input: InputValues,
    pub clone_progress: Option<SharedCloneProgress>,

    pub git_fetch_statuses: Arc<Mutex<HashMap<String, GitFetchStatus>>>,
    pub is_fetching_updates: bool,

    pub cloud_servers: Vec<CloudServer>,
    pub selected_cloud_server: Option<usize>,
    pub cloud_servers_path: PathBuf,
    pub add_server_logging_in: bool,
    pub add_server_error: Option<String>,

    pub auth: AuthState,
    pub theme_picker: gpui::Entity<ui_common::ThemePicker>,
    pub friends_screen: gpui::Entity<ui_friends::FriendsScreen>,
    pub pending_invite: Option<PendingInvite>,

    pub plugin_registries: Vec<PluginRegistry>,
    pub registry_plugins: Vec<RegistryPlugin>,
    pub registry_refresh_in_progress: bool,
    pub installed_plugins: Vec<InstalledPlugin>,
    pub plugin_install_phase: Option<PluginInstallPhase>,
    pub plugins_path: PathBuf,
    pub registries_path: PathBuf,

    pub dependency_status: Option<DependencyStatus>,
    pub install_progress: Option<InstallProgress>,

    pub project_thumbnails: HashMap<String, Option<Arc<RenderImage>>>,
    pub project_thumbnail_inflight: usize,
    pub project_thumbnail_queue: VecDeque<String>,
    pub template_thumbnails: HashMap<String, Option<Arc<RenderImage>>>,
    pub template_thumbnail_inflight: usize,
    pub template_thumbnail_queue: VecDeque<Template>,
}

impl AppState {
    pub fn new(window: &mut Window, cx: &mut App) -> Self {
        let recent_projects_path = directories::ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
            .map(|d| d.data_dir().join("recent_projects.json"))
            .unwrap_or_else(|| PathBuf::from("recent_projects.json"));
        let recent_projects =
            crate::service::project_service::RecentProjectsList::load(&recent_projects_path);
        let templates = get_default_templates();

        let cloud_servers_path = directories::ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
            .map(|d| d.data_dir().join("cloud_servers.json"))
            .unwrap_or_else(|| PathBuf::from("cloud_servers.json"));
        let cloud_servers: Vec<CloudServer> = std::fs::read_to_string(&cloud_servers_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        let appdata = directories::ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
            .map(|d| d.data_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        let plugins_path = appdata.join("plugins");
        let registries_path = appdata.join("registries");
        let installed_plugins: Vec<InstalledPlugin> =
            std::fs::read_to_string(plugins_path.join("plugins.json"))
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
        let plugin_registries: Vec<PluginRegistry> =
            std::fs::read_to_string(appdata.join("plugin_registries.json"))
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(|| {
                    vec![PluginRegistry {
                        name: "Official Pulsar Plugins".to_string(),
                        url: "https://github.com/Far-Beyond-Pulsar/Plugins".to_string(),
                    }]
                });

        let logo = {
            let bytes: &[u8] = include_bytes!("../../../../../assets/images/logo_sqrkl.png");
            image::load_from_memory(bytes).ok().map(|i| {
                let rgba = i.into_rgba8();
                let frame = image::Frame::new(rgba);
                Arc::new(RenderImage::new(smallvec::smallvec![frame]))
            })
        };

        Self {
            logo,
            recent_projects,
            recent_projects_path,
            templates,
            ui: UiState::new(),
            input: InputValues::new(),
            clone_progress: None,
            git_fetch_statuses: Arc::new(Mutex::new(HashMap::new())),
            is_fetching_updates: false,
            cloud_servers,
            selected_cloud_server: None,
            cloud_servers_path,
            add_server_logging_in: false,
            add_server_error: None,
            auth: AuthState::new(cx),
            theme_picker: cx.new(|cx| ui_common::ThemePicker::new(window, cx)),
            friends_screen: cx.new(|cx| ui_friends::FriendsScreen::new_without_invite(window, cx)),
            pending_invite: None,
            plugin_registries,
            registry_plugins: Vec::new(),
            registry_refresh_in_progress: false,
            installed_plugins,
            plugin_install_phase: None,
            plugins_path,
            registries_path,
            dependency_status: None,
            install_progress: None,
            project_thumbnails: HashMap::new(),
            project_thumbnail_inflight: 0,
            project_thumbnail_queue: VecDeque::new(),
            template_thumbnails: HashMap::new(),
            template_thumbnail_inflight: 0,
            template_thumbnail_queue: VecDeque::new(),
        }
    }
}
