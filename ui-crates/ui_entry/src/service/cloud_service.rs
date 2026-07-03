use std::path::PathBuf;

use crate::core::types::*;
use engine_backend::subsystems::networking::MultiuserClient;

fn insecure_tls_enabled() -> bool {
    std::env::var("PULSAR_INSECURE_TLS").as_deref() == Ok("1")
}

fn normalize_url(raw: &str) -> String {
    let raw = raw.trim().trim_end_matches('/');
    if raw.starts_with("http://") || raw.starts_with("https://") {
        raw.to_string()
    } else {
        format!("http://{}", raw)
    }
}

/// Async (blocking) functions to interact with Pulsar Host servers
pub struct CloudService;

impl CloudService {
    /// Authenticate and return a JWT token and username, or None on failure
    pub fn login(base_url: &str, email: &str, password: &str) -> Option<(String, String)> {
        let login_url = format!("{}/api/v1/auth/login", base_url);
        let body = serde_json::json!({ "email": email, "password": password });
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .danger_accept_invalid_certs(insecure_tls_enabled())
            .build().ok()?;
        let resp = client.post(&login_url).json(&body).send().ok()?;
        if !resp.status().is_success() { return None; }
        let data: serde_json::Value = resp.json().ok()?;
        let token = data.get("token")?.as_str()?.to_string();
        let username = data.get("user")?.get("username")?.as_str()?.to_string();
        Some((token, username))
    }

    /// Fetch server info and project list
    pub fn fetch_server_info(base_url: &str, token: &str) -> Option<(CloudServerStatus, Vec<CloudProject>)> {
        let info_url = format!("{}/api/v1/info", base_url);
        let projects_url = format!("{}/api/v1/workspaces", base_url);
        let tok: Option<String> = if token.is_empty() { None } else { Some(token.to_string()) };

        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(6))
            .danger_accept_invalid_certs(insecure_tls_enabled())
            .build().ok()?;

        let started = std::time::Instant::now();
        let build_req = |url: &str| {
            let r = client.get(url);
            if let Some(ref t) = tok { r.bearer_auth(t) } else { r }
        };

        let status: CloudServerStatus = match build_req(&info_url).send() {
            Err(_) => return Some((CloudServerStatus::Offline, vec![])),
            Ok(r) if r.status() == reqwest::StatusCode::UNAUTHORIZED => return Some((CloudServerStatus::Unauthorized, vec![])),
            Ok(r) if !r.status().is_success() => return Some((CloudServerStatus::Offline, vec![])),
            Ok(r) => {
                let latency_ms = started.elapsed().as_millis() as u32;
                let info: serde_json::Value = r.json().ok()?;
                CloudServerStatus::Online {
                    latency_ms,
                    version: info.get("version").and_then(|v| v.as_str()).unwrap_or("?").to_string(),
                    active_users: info.get("active_users").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                    active_projects: info.get("active_workspaces").or_else(|| info.get("active_projects")).and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                }
            }
        };

        let projects: Vec<CloudProject> = match build_req(&projects_url).send() {
            Ok(r) if r.status().is_success() => r.json::<serde_json::Value>().ok()
                .and_then(|v| v.as_array().cloned())
                .map(|arr| arr.into_iter().filter_map(|p| {
                    let id = p.get("id")?.as_str()?.to_string();
                    let name = p.get("name")?.as_str()?.to_string();
                    let description = p.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let last_modified = p.get("updated_at").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let size_bytes = p.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0);
                    let owner = p.get("owner_name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let user_count = p.get("active_users").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                    let proj_status = match p.get("status").and_then(|v| v.as_str()).unwrap_or("idle") {
                        "preparing" => CloudProjectStatus::Preparing,
                        "running" => CloudProjectStatus::Running { user_count },
                        "error" => CloudProjectStatus::Error(p.get("error_msg").and_then(|v| v.as_str()).unwrap_or("unknown").to_string()),
                        _ => CloudProjectStatus::Idle,
                    };
                    Some(CloudProject { id, name, description, status: proj_status, last_modified, size_bytes, owner })
                }).collect())
                .unwrap_or_default(),
            _ => vec![],
        };

        Some((status, projects))
    }

    pub fn prepare_workspace(base_url: &str, workspace_id: &str, token: &str) {
        let url = format!("{}/api/v1/workspaces/{}/prepare", base_url, workspace_id);
        let tok: Option<String> = if token.is_empty() { None } else { Some(token.to_string()) };
        Self::send_post(&url, tok);
    }

    pub fn stop_workspace(base_url: &str, workspace_id: &str, token: &str) {
        let url = format!("{}/api/v1/workspaces/{}/stop", base_url, workspace_id);
        let tok: Option<String> = if token.is_empty() { None } else { Some(token.to_string()) };
        Self::send_post(&url, tok);
    }

    pub fn create_workspace(base_url: &str, name: &str, description: &str, token: &str) {
        let url = format!("{}/api/v1/workspaces", base_url);
        let body = serde_json::json!({ "name": name, "description": description });
        let tok: Option<String> = if token.is_empty() { None } else { Some(token.to_string()) };
        let Ok(client) = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .danger_accept_invalid_certs(insecure_tls_enabled())
            .build() else { return; };
        let req = client.post(&url).json(&body);
        let req = if let Some(ref t) = tok { req.bearer_auth(t) } else { req };
        let _ = req.send();
    }

    pub fn delete_workspace(base_url: &str, workspace_id: &str, token: &str) {
        let url = format!("{}/api/v1/workspaces/{}", base_url, workspace_id);
        let Ok(client) = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .danger_accept_invalid_certs(insecure_tls_enabled())
            .build() else { return; };
        let req = client.delete(&url);
        let req = if token.is_empty() { req } else { req.bearer_auth(token) };
        let _ = req.send();
    }

    fn send_post(url: &str, token: Option<String>) {
        let Ok(client) = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .danger_accept_invalid_certs(insecure_tls_enabled())
            .build() else { return; };
        let req = client.post(url);
        let req = if let Some(ref t) = token { req.bearer_auth(t) } else { req };
        let _ = req.send();
    }

    /// Open a cloud workspace by configuring the virtual filesystem and starting a session.
    /// Returns the virtual path for the editor to open.
    pub fn open_workspace(base_url: &str, workspace_id: &str, auth_token: &str, username: &str) -> PathBuf {
        let token_opt: Option<String> = if auth_token.is_empty() { None } else { Some(auth_token.to_string()) };
        let remote_config = engine_fs::RemoteConfig {
            server_url: base_url.to_string(),
            workspace_id: workspace_id.to_string(),
            auth_token: token_opt,
        };
        engine_fs::virtual_fs::set_provider(std::sync::Arc::new(engine_fs::RemoteFsProvider::new(remote_config)));
        let ctx = engine_state::MultiuserContext::new_cloud_project(
            base_url.to_string(), workspace_id.to_string(), "local", "remote")
            .with_status(engine_state::MultiuserStatus::Connecting)
            .with_workspace_id(workspace_id.to_string());
        let ctx = if !auth_token.is_empty() { ctx.with_auth_token(auth_token.to_string()) } else { ctx };
        if let Some(ec) = engine_state::EngineContext::global() { ec.set_multiuser(ctx); }
        let virtual_path = PathBuf::from(format!("cloud+pulsar://{}/{}",
            base_url.trim_start_matches("http://").trim_start_matches("https://"), workspace_id));
        let bu = base_url.to_string();
        let wid = workspace_id.to_string();
        let tok = auth_token.to_string();
        let user = username.to_string();
        std::thread::spawn(move || {
            let mut client = MultiuserClient::new(bu);
            match client.connect_to_workspace_sync(wid, tok, user) {
                Ok(mut rx) => {
                    if let Some(ec) = engine_state::EngineContext::global() {
                        let _ = ec.update_multiuser(|mu| mu.set_status(engine_state::MultiuserStatus::Connected { relay_mode: None }));
                        ec.notify_multiuser_changed();
                    }
                    while rx.blocking_recv().is_some() {}
                }
                Err(e) => tracing::error!("Cloud session connect failed: {e}"),
            }
        });
        virtual_path
    }
}
