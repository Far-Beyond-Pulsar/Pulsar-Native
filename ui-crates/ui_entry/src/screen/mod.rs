pub mod views;

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use gpui::*;
use parking_lot::Mutex;

use crate::core::events::*;
use crate::core::state::*;
use crate::core::types::*;
use crate::service::auth_service::AuthService;
use crate::service::cloud_service::CloudService;
use crate::service::dependency_service::DependencyService;
use crate::service::git_service::GitService;
use crate::service::plugin_service::PluginService;
use crate::service::project_service::ProjectService;
use crate::service::thumbnail_service::ThumbnailService;
use crate::screen::views::project_settings::ProjectSettingsTab;

pub struct EntryScreen {
    pub state: AppState,
    pub inputs: InputEntities,
    pub entity: Option<Entity<EntryScreen>>,
}

impl EntryScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut state = AppState::new(window, cx);
        let inputs = InputEntities::new(window, cx);
        let self_entity = cx.entity().clone();

        let status = DependencyService::check();
        state.dependency_status = Some(status);

        for proj in &state.recent_projects.projects {
            state.project_thumbnail_queue.push_back(proj.path.clone());
        }
        for tmpl in &state.templates {
            state.template_thumbnail_queue.push_back(tmpl.clone());
        }

        let oobe_marker = directories::ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
            .map(|d| d.data_dir().join("oobe_complete"));
        let is_fresh = oobe_marker.as_ref().map(|p| !p.exists()).unwrap_or(true);
        let force_oobe = crate::FORCE_OOBE.swap(false, Ordering::Relaxed);
        if is_fresh || force_oobe {
            state.ui.show_onboarding = true;
            if let Some(ref path) = oobe_marker {
                let _ = std::fs::create_dir_all(path.parent().unwrap());
                let _ = std::fs::write(path, "1");
            }
        }

        inputs.subscribe_all(self_entity.clone(), cx);

        let mut this = Self {
            state,
            inputs,
            entity: Some(self_entity),
        };
        this.load_thumbnails(cx);
        if this.state.ui.show_onboarding {
            this.refresh_plugin_registry(cx);
        }
        this
    }

    pub fn inputs(&self) -> &InputEntities {
        &self.inputs
    }

    pub(crate) fn check_dependencies_async(&mut self, cx: &mut Context<Self>) {
        let entity = self.entity.clone().unwrap();
        // TODO: async spawn
        let status = DependencyService::check();
        entity.update(cx, |this, cx| {
            this.state.dependency_status = Some(status);
            cx.notify();
        });
    }

    pub(crate) fn start_git_fetch_all(&self, cx: &mut Context<Self>) {
        let paths: Vec<(String, PathBuf)> = self.state.recent_projects.projects.iter()
            .filter(|p| p.is_git)
            .map(|p| (p.name.clone(), PathBuf::from(&p.path)))
            .collect();
        let statuses = self.state.git_fetch_statuses.clone();
        for (name, path) in paths {
            let s = statuses.clone();
            std::thread::spawn(move || {
                let _ = s.lock().insert(name.clone(), GitFetchStatus::Fetching);
                match GitService::check_for_updates(&path) {
                    Ok(0) => { let _ = s.lock().insert(name, GitFetchStatus::UpToDate); }
                    Ok(n) => { let _ = s.lock().insert(name, GitFetchStatus::UpdatesAvailable(n)); }
                    Err(e) => { let _ = s.lock().insert(name, GitFetchStatus::Error(e.to_string())); }
                }
            });
        }
    }

    pub(crate) fn pull_project_updates(&self, path: PathBuf, _cx: &mut Context<Self>) {
        let statuses = self.state.git_fetch_statuses.clone();
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        std::thread::spawn(move || {
            let _ = statuses.lock().insert(name.clone(), GitFetchStatus::Fetching);
            match GitService::pull_updates(&path) {
                Ok(()) => { let _ = statuses.lock().insert(name, GitFetchStatus::UpToDate); }
                Err(e) => { let _ = statuses.lock().insert(name, GitFetchStatus::Error(e.to_string())); }
            }
        });
    }

    pub(crate) fn open_folder_dialog(&self, cx: &mut Context<Self>) {
        let entity = self.entity.clone().unwrap();
        let recent_projects_path = self.state.recent_projects_path.clone();
        cx.spawn(async move |_handle, cx| {
            if let Some(folder) = rfd::AsyncFileDialog::new().pick_folder().await {
                let path = folder.path().to_path_buf();
                if !ProjectService::validate_project(&path) { return; }
                let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let path_str = path.to_string_lossy().to_string();
                let is_git = ProjectService::is_git_repo(&path);
                let project = crate::service::project_service::RecentProject {
                    name, path: path_str, last_opened: Some(chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()), is_git,
                };
                cx.update(|cx| {
                    entity.update(cx, |this, cx| {
                        this.state.recent_projects.add_or_update(project);
                        this.state.recent_projects.save(&recent_projects_path);
                        cx.emit(ProjectSelected { path });
                        cx.notify();
                    });
                });
            }
        }).detach();
    }

    pub(crate) fn clone_git_repo(&mut self, url: Option<String>, cx: &mut Context<Self>) {
        let repo_url = url.unwrap_or_else(|| self.state.input.git_repo_url_text.clone());
        if repo_url.is_empty() { return; }
        let entity = self.entity.clone().unwrap();
        let recent_projects_path = self.state.recent_projects_path.clone();
        cx.spawn(async move |_handle, cx| {
            if let Some(folder) = rfd::AsyncFileDialog::new().pick_folder().await {
                let parent = folder.path().to_path_buf();
                let target = parent.join(repo_url.trim_end_matches(".git").split('/').last().unwrap_or("repo"));
                let progress = Arc::new(Mutex::new(CloneProgress {
                    current: 0, total: 0, message: "Starting clone...".to_string(), completed: false, error: None,
                }));
                let p = progress.clone();
                let url = repo_url.clone();
                let t = target.clone();
                let _ = cx.background_executor().spawn(async move {
                    GitService::clone_repository(url, t, p)
                }).await;
                let show_upstream = ProjectService::is_git_repo(&target) && !GitService::has_origin_remote(&target);
                cx.update(|cx| {
                    entity.update(cx, |this, cx| {
                        this.state.clone_progress = None;
                        this.state.input.new_project_path = Some(target.clone());
                        if show_upstream {
                            let n = target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                            this.state.ui.show_git_upstream_prompt = Some((target.clone(), n));
                        } else {
                            let ps = target.to_string_lossy().to_string();
                            let n = target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                            this.state.recent_projects.add_or_update(crate::service::project_service::RecentProject {
                                name: n, path: ps, last_opened: Some(chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()), is_git: true,
                            });
                            this.state.recent_projects.save(&recent_projects_path);
                            cx.emit(ProjectSelected { path: target });
                        }
                        cx.notify();
                    });
                });
            }
        }).detach();
    }

    pub(crate) fn clone_template(&mut self, template: Template, cx: &mut Context<Self>) {
        self.clone_git_repo(Some(template.repo_url), cx);
    }

    pub(crate) fn setup_git_upstream(&mut self, cx: &mut Context<Self>) {
        let (path, _) = match &self.state.ui.show_git_upstream_prompt.take() {
            Some(pair) => pair.clone(),
            None => return,
        };
        let url = self.state.input.git_upstream_url_text.clone();
        let entity = self.entity.clone().unwrap();
        let recent_projects_path = self.state.recent_projects_path.clone();
        cx.spawn(async move |_handle, cx| {
            if !url.is_empty() {
                let p = path.clone();
                let u = url.clone();
                let _ = cx.background_executor().spawn(async move { GitService::add_user_upstream(&p, &u) }).await;
            }
            cx.update(|cx| {
                entity.update(cx, |this, cx| {
                    let ps = path.to_string_lossy().to_string();
                    let n = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    this.state.recent_projects.add_or_update(crate::service::project_service::RecentProject {
                        name: n, path: ps, last_opened: Some(chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()), is_git: true,
                    });
                    this.state.recent_projects.save(&recent_projects_path);
                    cx.emit(ProjectSelected { path });
                    cx.notify();
                });
            });
        }).detach();
    }

    pub(crate) fn launch_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let is_git = ProjectService::is_git_repo(&path);
        let project = crate::service::project_service::RecentProject {
            name, path: path.to_string_lossy().to_string(),
            last_opened: Some(chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()), is_git,
        };
        self.state.recent_projects.add_or_update(project);
        self.state.recent_projects.save(&self.state.recent_projects_path);
        cx.emit(ProjectSelected { path });
    }

    pub(crate) fn remove_recent_project(&mut self, path: &str, cx: &mut Context<Self>) {
        self.state.recent_projects.remove(path);
        self.state.recent_projects.save(&self.state.recent_projects_path);
        cx.notify();
    }

    pub(crate) fn open_git_manager(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        cx.emit(GitManagerRequested { path });
    }

    pub(crate) fn open_project_settings(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let (editor, git_tool) = ProjectService::load_tool_preferences(&path);
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let mut ps = crate::screen::views::project_settings::ProjectSettings::new(path.clone(), name);
        ps.preferred_editor = editor;
        ps.preferred_git_tool = git_tool;
        self.state.ui.project_settings = Some(ps);
        cx.notify();
    }

    pub(crate) fn close_project_settings(&mut self, cx: &mut Context<Self>) {
        self.state.ui.project_settings = None;
        cx.notify();
    }

    pub fn calculate_columns(&self, available_width: gpui::Pixels) -> usize {
        let card_width = 320.0;
        let gap = 24.0;
        let f_width: f32 = f32::from(available_width);
        let cols = ((f_width + gap) / (card_width + gap)).floor() as usize;
        cols.max(1)
    }

    pub(crate) fn change_project_settings_tab(&mut self, _tab: ProjectSettingsTab, cx: &mut Context<Self>) {
        cx.notify();
    }

    pub(crate) fn refresh_project_settings(&mut self, cx: &mut Context<Self>) {
        if let Some(ref settings) = self.state.ui.project_settings.clone() {
            let (editor, git_tool) = ProjectService::load_tool_preferences(&settings.project_path);
            let name = settings.project_path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let mut ps = crate::screen::views::project_settings::ProjectSettings::new(settings.project_path.clone(), name);
            ps.preferred_editor = editor;
            ps.preferred_git_tool = git_tool;
            self.state.ui.project_settings = Some(ps);
        }
        cx.notify();
    }

    pub(crate) fn browse_project_location(&self, cx: &mut Context<Self>) {
        let entity = self.entity.clone().unwrap();
        cx.spawn(async move |_handle, cx| {
            if let Some(folder) = rfd::AsyncFileDialog::new().pick_folder().await {
                cx.update(|cx| {
                    entity.update(cx, |this, cx| {
                        this.state.input.new_project_path = Some(folder.path().to_path_buf());
                        cx.notify();
                    });
                });
            }
        }).detach();
    }

    pub(crate) fn create_new_project(&self, cx: &mut Context<Self>) {
        let name = self.state.input.new_project_name_text.clone();
        if name.is_empty() { return; }
        let base_path = self.state.input.new_project_path.clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let project_path = base_path.join(&name);
        let entity = self.entity.clone().unwrap();
        let recent_projects_path = self.state.recent_projects_path.clone();
        let n = name.clone();
        let pp = project_path.clone();
        cx.spawn(async move |_handle, cx| {
            let _ = cx.background_executor().spawn(async move {
                let _ = std::fs::create_dir_all(&pp);
                let _ = ProjectService::create_project_dirs(&pp);
                let _ = ProjectService::write_pulsar_toml(&pp, &n);
                let _ = ProjectService::init_repository(&pp);
            }).await;
            let pstr = project_path.to_string_lossy().to_string();
            let project = crate::service::project_service::RecentProject {
                name, path: pstr, last_opened: Some(chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()), is_git: true,
            };
            cx.update(|cx| {
                entity.update(cx, |this, cx| {
                    this.state.recent_projects.add_or_update(project);
                    this.state.recent_projects.save(&recent_projects_path);
                    cx.emit(ProjectSelected { path: project_path });
                    cx.notify();
                });
            });
        }).detach();
    }

    pub(crate) fn begin_github_sign_in(&mut self, cx: &mut Context<Self>) {
        let Some(client_id) = pulsar_auth::github_client_id_from_env() else {
            self.state.auth.message = Some("Set PULSAR_GITHUB_CLIENT_ID to enable GitHub sign-in.".to_string());
            cx.notify();
            return;
        };
        self.state.auth.loading = true;
        self.state.auth.message = Some("Starting GitHub sign-in\u{2026}".to_string());
        self.state.auth.device_code = None;
        self.state.auth.device_verification_url = None;
        self.state.ui.auth_device_modal_visible = false;
        cx.notify();
        let entity = self.entity.clone().unwrap();
        cx.spawn(async move |_handle, cx| {
            let c_id = client_id.clone();
            let flow = cx.background_executor().spawn(async move {
                pulsar_auth::start_device_flow(&c_id)
            }).await;
            let Ok(flow) = flow else {
                cx.update(|cx| entity.update(cx, |this, cx| {
                    this.state.auth.loading = false;
                    this.state.auth.message = Some("Failed to start GitHub device flow.".to_string());
                    cx.notify();
                }));
                return;
            };
            let uri = flow.verification_uri.clone();
            let _ = open::that(&uri);
            cx.update(|cx| entity.update(cx, |this, cx| {
                this.state.auth.device_code = Some(flow.user_code.clone());
                this.state.auth.device_verification_url = Some(flow.verification_uri.clone());
                this.state.ui.auth_device_modal_visible = true;
                this.state.auth.loading = false;
                cx.notify();
            }));
            let c_id2 = client_id.to_string();
            let flow_clone = flow.clone();
            let token = cx.background_executor().spawn(async move {
                pulsar_auth::wait_for_device_flow_token(&c_id2, &flow_clone)
            }).await;
            let Ok(token) = token else {
                cx.update(|cx| entity.update(cx, |this, cx| {
                    this.state.auth.loading = false;
                    this.state.ui.auth_device_modal_visible = false;
                    this.state.auth.device_code = None;
                    this.state.auth.device_verification_url = None;
                    this.state.auth.message = Some("GitHub sign-in timed out or failed.".to_string());
                    cx.notify();
                }));
                return;
            };
            let token_fetch = token.clone();
            let profile = cx.background_executor().spawn(async move {
                pulsar_auth::fetch_profile(&token_fetch)
            }).await;
            let profile = match profile {
                Ok(p) => p,
                Err(e) => {
                    cx.update(|cx| entity.update(cx, |this, cx| {
                        this.state.auth.loading = false;
                        this.state.ui.auth_device_modal_visible = false;
                        this.state.auth.message = Some(format!("Failed to fetch profile: {e}"));
                        cx.notify();
                    }));
                    return;
                }
            };
            let _ = pulsar_auth::store_access_token(&token);
            let _ = pulsar_auth::save_cached_profile(&profile);
            if let Some(ec) = engine_state::EngineContext::global() {
                ec.set_auth_profile(profile.clone());
            }
            cx.update(|cx| entity.update(cx, |this, cx| {
                this.state.auth.loading = false;
                this.state.ui.auth_device_modal_visible = false;
                this.state.auth.device_code = None;
                this.state.auth.device_verification_url = None;
                this.state.auth.message = None;
                this.state.auth.profile_dropdown.update(cx, |d, cx| {
                    d.ensure_avatar_loaded(cx);
                    cx.notify();
                });
                cx.notify();
            }));
        }).detach();
    }

    pub(crate) fn handle_auth_device_code(&self, _cx: &mut Context<Self>) {
        if let Some(ref url) = self.state.auth.device_verification_url {
            let _ = open::that(url);
        }
    }

    pub(crate) fn cancel_auth(&mut self, cx: &mut Context<Self>) {
        self.state.auth.loading = false;
        self.state.auth.message = None;
        self.state.auth.device_code = None;
        self.state.auth.device_verification_url = None;
        self.state.ui.auth_device_modal_visible = false;
        cx.notify();
    }

    pub(crate) fn sign_out(&mut self, cx: &mut Context<Self>) {
        let _ = pulsar_auth::clear_cached_profile();
        let _ = pulsar_auth::clear_access_token();
        if let Some(ec) = engine_state::EngineContext::global() {
            ec.clear_auth_profile();
        }
        self.state.auth.profile_dropdown.update(cx, |d, cx| {
            d.avatar_image = None;
            d.avatar_url_loaded = None;
            d.is_open = false;
            cx.notify();
        });
        self.state.auth.onboarding_avatar = None;
        self.state.auth.onboarding_avatar_url = None;
        cx.notify();
    }

    pub(crate) fn handle_invite_response(&mut self, _accept: bool, cx: &mut Context<Self>) {
        self.state.pending_invite = None;
        cx.notify();
    }

    pub(crate) fn add_cloud_server(&mut self, cx: &mut Context<Self>) {
        let alias = self.state.input.add_server_alias_text.clone();
        let url_text = self.state.input.add_server_url_text.clone();
        let email = self.state.input.add_server_email_text.clone();
        let password = self.state.input.add_server_password_text.clone();
        self.state.add_server_logging_in = true;
        self.state.add_server_error = None;
        cx.notify();
        let entity = self.entity.clone().unwrap();
        let normalized_url = normalize_url(&url_text);
        cx.spawn(async move |_handle, cx| {
            let nu = normalized_url.clone();
            let em = email.clone();
            let pw = password.clone();
            let token = cx.background_executor().spawn(async move { CloudService::login(&nu, &em, &pw) }).await;
            cx.update(|cx| entity.update(cx, |this, cx| {
                this.state.add_server_logging_in = false;
                if let Some(t) = token {
                    this.state.cloud_servers.push(CloudServer {
                        id: uuid::Uuid::new_v4().to_string(), alias, url: normalized_url, auth_token: t,
                        status: CloudServerStatus::Unknown, projects: Vec::new(),
                    });
                    this.save_cloud_servers();
                    this.state.add_server_error = None;
                    this.state.input.add_server_alias_text.clear();
                    this.state.input.add_server_url_text.clear();
                    this.state.input.add_server_email_text.clear();
                    this.state.input.add_server_password_text.clear();
                    this.state.ui.show_add_server = false;
                } else {
                    this.state.add_server_error = Some("Login failed. Check your credentials.".to_string());
                }
                cx.notify();
            }));
        }).detach();
    }

    pub(crate) fn test_cloud_server_connection(&self, index: usize, cx: &mut Context<Self>) {
        if index >= self.state.cloud_servers.len() { return; }
        let server = self.state.cloud_servers[index].clone();
        let entity = self.entity.clone().unwrap();
        cx.spawn(async move |_handle, cx| {
            let result = cx.background_executor().spawn(async move {
                CloudService::fetch_server_info(&server.url, &server.auth_token)
            }).await;
            cx.update(|cx| entity.update(cx, |this, cx| {
                if index < this.state.cloud_servers.len() {
                    this.state.cloud_servers[index].status = result.as_ref().map(|r| r.0.clone()).unwrap_or(CloudServerStatus::Offline);
                    if let Some((_, projects)) = result {
                        this.state.cloud_servers[index].projects = projects;
                    }
                }
                cx.notify();
            }));
        }).detach();
    }

    pub(crate) fn select_cloud_server(&mut self, index: usize, cx: &mut Context<Self>) {
        self.state.selected_cloud_server = Some(index);
        self.test_cloud_server_connection(index, cx);
        cx.notify();
    }

    pub(crate) fn refresh_cloud_server(&self, index: usize, cx: &mut Context<Self>) {
        self.test_cloud_server_connection(index, cx);
    }

    pub(crate) fn prepare_cloud_project(&self, server_idx: usize, project_idx: usize, _cx: &mut Context<Self>) {
        if server_idx >= self.state.cloud_servers.len() { return; }
        let server = self.state.cloud_servers[server_idx].clone();
        if project_idx >= server.projects.len() { return; }
        let project = server.projects[project_idx].clone();
        std::thread::spawn(move || CloudService::prepare_workspace(&server.url, &project.id, &server.auth_token));
    }

    pub(crate) fn open_cloud_project(&self, server_idx: usize, project_idx: usize, _cx: &mut Context<Self>) {
        if server_idx >= self.state.cloud_servers.len() { return; }
        let server = self.state.cloud_servers[server_idx].clone();
        if project_idx >= server.projects.len() { return; }
        let project = server.projects[project_idx].clone();
        std::thread::spawn(move || CloudService::open_workspace(&server.url, &project.id, &server.auth_token));
    }

    pub(crate) fn stop_cloud_project(&self, server_idx: usize, project_idx: usize, _cx: &mut Context<Self>) {
        if server_idx >= self.state.cloud_servers.len() { return; }
        let server = self.state.cloud_servers[server_idx].clone();
        if project_idx >= server.projects.len() { return; }
        let project = server.projects[project_idx].clone();
        std::thread::spawn(move || CloudService::stop_workspace(&server.url, &project.id, &server.auth_token));
    }

    pub(crate) fn delete_cloud_project(&self, server_idx: usize, project_idx: usize, _cx: &mut Context<Self>) {
        if server_idx >= self.state.cloud_servers.len() { return; }
        let server = self.state.cloud_servers[server_idx].clone();
        if project_idx >= server.projects.len() { return; }
        let project = server.projects[project_idx].clone();
        std::thread::spawn(move || CloudService::delete_workspace(&server.url, &project.id, &server.auth_token));
    }

    pub(crate) fn create_cloud_project(&mut self, cx: &mut Context<Self>) {
        let name = self.state.input.create_project_name_text.clone();
        if name.is_empty() { return; }
        let server_idx = self.state.selected_cloud_server.unwrap_or(0);
        if server_idx >= self.state.cloud_servers.len() { return; }
        let server = self.state.cloud_servers[server_idx].clone();
        let desc = self.state.input.create_project_description_text.clone();
        std::thread::spawn(move || CloudService::create_workspace(&server.url, &name, &desc, &server.auth_token));
        self.state.ui.show_create_project = false;
        self.state.input.create_project_name_text.clear();
        self.state.input.create_project_description_text.clear();
        cx.notify();
    }

    pub(crate) fn remove_cloud_server(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.state.cloud_servers.len() {
            self.state.cloud_servers.remove(index);
            self.save_cloud_servers();
            if self.state.selected_cloud_server == Some(index) {
                self.state.selected_cloud_server = None;
            }
            cx.notify();
        }
    }

    pub(crate) fn refresh_plugin_registry(&mut self, cx: &mut Context<Self>) {
        if self.state.registry_refresh_in_progress { return; }
        self.state.registry_refresh_in_progress = true;
        cx.notify();
        let entity = self.entity.clone().unwrap();
        let registries = self.state.plugin_registries.clone();
        let registries_path = self.state.registries_path.clone();
        cx.spawn(async move |_handle, cx| {
            let regs = registries.clone();
            let rp = registries_path.clone();
            let _ = cx.background_executor().spawn(async move {
                match PluginService::clone_or_pull_registries(&regs, &rp) {
                    Ok(()) => tracing::debug!("Plugin registries cloned/pulled successfully"),
                    Err(e) => tracing::error!("Failed to clone/pull plugin registries: {e}"),
                }
            }).await;
            let regs2 = registries.clone();
            let rp2 = registries_path.clone();
            let plugins = cx.background_executor().spawn(async move {
                let list = PluginService::load_plugins_from_registries(&regs2, &rp2);
                tracing::debug!("Loaded {} plugins from registries", list.len());
                list
            }).await;
            cx.update(|cx| entity.update(cx, |this, cx| {
                this.state.registry_plugins = plugins;
                this.state.registry_refresh_in_progress = false;
                cx.notify();
            }));
        }).detach();
    }

    pub(crate) fn install_registry_plugin(&mut self, plugin: RegistryPlugin, cx: &mut Context<Self>) {
        if self.state.plugin_install_phase.is_some() { return; }
        self.state.plugin_install_phase = Some(PluginInstallPhase::FetchingMetadata);
        cx.notify();
        let entity = self.entity.clone().unwrap();
        let plugins_path = self.state.plugins_path.clone();
        let pname = plugin.name.clone();
        let purl = plugin.repo_url.clone();
        cx.spawn(async move |_handle, cx| {
            let (owner, repo) = match PluginService::parse_github_owner_repo(&purl) {
                Some(pair) => pair,
                None => {
                    cx.update(|cx| entity.update(cx, |this, cx| {
                        this.state.plugin_install_phase = Some(PluginInstallPhase::Error("Invalid repo URL".to_string()));
                        cx.notify();
                    }));
                    return;
                }
            };
            let repo_tag = repo.clone();
            let release: Option<(String, Option<String>)> = cx.background_executor().spawn(async move { PluginService::fetch_latest_release(&owner, &repo) }).await.ok().flatten();
            let Some((tag, binary_url_opt)) = release else {
                cx.update(|cx| entity.update(cx, |this, cx| {
                    this.state.plugin_install_phase = Some(PluginInstallPhase::Error("No releases found".to_string()));
                    cx.notify();
                }));
                return;
            };
            let ext = native_plugin_ext();
            if let Some(binary_url) = binary_url_opt {
                let lib_name = format!("{}_{}.{}", repo_tag, tag, ext);
                let pp = plugins_path.clone();
                let bu = binary_url.clone();
                let result = cx.background_executor().spawn(async move { PluginService::download_binary(&bu, &pp, &lib_name) }).await;
                match result {
                    Ok(lib_path) => {
                        let installed = InstalledPlugin {
                            name: pname, repo_url: purl, version: tag,
                            installed_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                            install_method: PluginInstallMethod::BinaryDownload, library_path: lib_path,
                        };
                        cx.update(|cx| entity.update(cx, |this, cx| {
                            this.state.plugin_install_phase = Some(PluginInstallPhase::Complete(installed.clone()));
                            this.state.installed_plugins.push(installed);
                            this.save_installed_plugins();
                            cx.notify();
                        }));
                    }
                    _ => {
                        cx.update(|cx| entity.update(cx, |this, cx| {
                            this.state.plugin_install_phase = Some(PluginInstallPhase::Error("Download failed".to_string()));
                            cx.notify();
                        }));
                    }
                }
            } else {
                let tag_for_installed = tag.clone();
                let pp = plugins_path.clone();
                let purl2 = purl.clone();
                let result = cx.background_executor().spawn(async move { PluginService::build_from_source(&purl2, Some(&tag), &pp, &tag) }).await;
                match result {
                    Ok((lib_path, _logs)) => {
                        let installed = InstalledPlugin {
                            name: pname, repo_url: purl, version: tag_for_installed,
                            installed_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                            install_method: PluginInstallMethod::BuiltFromSource, library_path: lib_path,
                        };
                        cx.update(|cx| entity.update(cx, |this, cx| {
                            this.state.plugin_install_phase = Some(PluginInstallPhase::Complete(installed.clone()));
                            this.state.installed_plugins.push(installed);
                            this.save_installed_plugins();
                            cx.notify();
                        }));
                    }
                    _ => {
                        cx.update(|cx| entity.update(cx, |this, cx| {
                            this.state.plugin_install_phase = Some(PluginInstallPhase::Error("Build failed".to_string()));
                            cx.notify();
                        }));
                    }
                }
            }
        }).detach();
    }

    pub(crate) fn uninstall_plugin(&mut self, _index: usize, cx: &mut Context<Self>) {
        cx.notify();
    }

    pub(crate) fn remove_plugin(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.state.installed_plugins.len() {
            let lib_path = std::path::Path::new(&self.state.installed_plugins[index].library_path).to_path_buf();
            let _ = std::fs::remove_file(&lib_path);
            self.state.installed_plugins.remove(index);
            self.save_installed_plugins();
            cx.notify();
        }
    }

    pub(crate) fn start_dependency_setup(&mut self, cx: &mut Context<Self>) {
        self.state.ui.show_dependency_setup = true;
        let entity = self.entity.clone().unwrap();
        let progress = Arc::new(std::sync::Mutex::new(InstallProgress {
            logs: Vec::new(), progress: 0.0, status: InstallStatus::Idle,
        }));
        self.state.install_progress = Some(InstallProgress {
            logs: Vec::new(), progress: 0.0, status: InstallStatus::Downloading,
        });
        cx.notify();
        let p = progress.clone();
        cx.spawn(async move |_handle, cx| {
            let pp = p.clone();
            let result = cx.background_executor().spawn(async move { DependencyService::install_rust(pp) }).await;
            match result {
                Ok(()) => {
                    let mut prog = p.lock().unwrap();
                    prog.status = InstallStatus::Complete;
                    prog.progress = 1.0;
                    let prog2 = prog.clone();
                    let status = DependencyService::check();
                    cx.update(|cx| entity.update(cx, |this, cx| {
                        this.state.install_progress = Some(prog2);
                        this.state.dependency_status = Some(status);
                        cx.notify();
                    }));
                }
                _ => {
                    let prog = p.lock().unwrap();
                    let prog2 = prog.clone();
                    cx.update(|cx| entity.update(cx, |this, cx| {
                        this.state.install_progress = Some(prog2);
                        cx.notify();
                    }));
                }
            }
        }).detach();
    }

    pub(crate) fn show_onboarding_flow(&mut self, cx: &mut Context<Self>) {
        self.state.ui.show_onboarding = true;
        self.state.ui.onboarding_tab = OnboardingTab::Theme;
        cx.notify();
    }

    pub(crate) fn dismiss_onboarding(&mut self, cx: &mut Context<Self>) {
        self.state.ui.show_onboarding = false;
        cx.notify();
    }

    pub(crate) fn switch_onboarding_tab(&mut self, tab: OnboardingTab, cx: &mut Context<Self>) {
        self.state.ui.onboarding_tab = tab;
        cx.notify();
    }

    pub(crate) fn inject_notification(&mut self, invite: PendingInvite, cx: &mut Context<Self>) {
        self.state.pending_invite = Some(invite);
        cx.notify();
    }

    pub(crate) fn load_thumbnails(&mut self, cx: &mut Context<Self>) {
        let entity = self.entity.clone().unwrap();
        if self.state.project_thumbnail_inflight == 0 && !self.state.project_thumbnail_queue.is_empty() {
            let path = self.state.project_thumbnail_queue.pop_front().unwrap();
            let path_store = path.clone();
            self.state.project_thumbnail_inflight += 1;
            cx.notify();
            let entity_proj = entity.clone();
            cx.spawn(async move |_handle, cx| {
                let result = cx.background_executor().spawn(async move {
                    ThumbnailService::load_project_thumbnail(&path)
                }).await;
                cx.update(|cx| entity_proj.update(cx, |this, cx| {
                    this.state.project_thumbnails.insert(path_store, result);
                    this.state.project_thumbnail_inflight -= 1;
                    this.load_thumbnails(cx);
                    cx.notify();
                }));
            }).detach();
        }
        if self.state.template_thumbnail_inflight == 0 && !self.state.template_thumbnail_queue.is_empty() {
            let template = self.state.template_thumbnail_queue.pop_front().unwrap();
            let name = template.name.clone();
            self.state.template_thumbnail_inflight += 1;
            cx.notify();
            let entity_tmpl = entity.clone();
            cx.spawn(async move |_handle, cx| {
                let result = cx.background_executor().spawn(async move {
                    ThumbnailService::load_template_thumbnail(&template)
                }).await;
                cx.update(|cx| entity_tmpl.update(cx, |this, cx| {
                    this.state.template_thumbnails.insert(name, result);
                    this.state.template_thumbnail_inflight -= 1;
                    this.load_thumbnails(cx);
                    cx.notify();
                }));
            }).detach();
        }
    }

    fn save_cloud_servers(&self) {
        if let Ok(json) = serde_json::to_string(&self.state.cloud_servers) {
            let _ = std::fs::write(&self.state.cloud_servers_path, json);
        }
    }

    fn save_installed_plugins(&self) {
        if let Ok(json) = serde_json::to_string(&self.state.installed_plugins) {
            let _ = std::fs::write(self.state.plugins_path.join("plugins.json"), json);
        }
    }
}

impl Render for EntryScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = crate::screen::layout::render_layout(self, window, cx);
        if self.state.ui.auth_device_modal_visible {
            div().size_full().child(content).child(views::render_auth_modal(self, cx)).into_any_element()
        } else {
            content.into_any_element()
        }
    }
}

mod layout;

impl EventEmitter<ProjectSelected> for EntryScreen {}
impl EventEmitter<GitManagerRequested> for EntryScreen {}
impl EventEmitter<SettingsRequested> for EntryScreen {}
impl EventEmitter<FabSearchRequested> for EntryScreen {}

#[cfg(target_os = "windows")]
fn native_plugin_ext() -> &'static str { "dll" }
#[cfg(target_os = "macos")]
fn native_plugin_ext() -> &'static str { "dylib" }
#[cfg(target_os = "linux")]
fn native_plugin_ext() -> &'static str { "so" }

fn normalize_url(raw: &str) -> String {
    let raw = raw.trim().trim_end_matches('/');
    if raw.starts_with("http://") || raw.starts_with("https://") {
        raw.to_string()
    } else {
        format!("http://{}", raw)
    }
}
