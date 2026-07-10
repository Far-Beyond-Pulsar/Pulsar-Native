use std::path::Path;

use crate::core::types::*;
use crate::service::git_service::GitService;

fn registry_local_path(registries_root: &Path, url: &str) -> std::path::PathBuf {
    let slug = url
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .replace(['/', ':'], "_");
    registries_root.join(slug)
}

#[cfg(target_os = "windows")]
fn native_plugin_ext() -> &'static str {
    "dll"
}
#[cfg(target_os = "macos")]
fn native_plugin_ext() -> &'static str {
    "dylib"
}
#[cfg(target_os = "linux")]
fn native_plugin_ext() -> &'static str {
    "so"
}

pub struct PluginService;

impl PluginService {
    /// Clone or pull each plugin registry via git, then return the list of plugins found.
    /// Falls back to HTTP-fetching the manifest files from GitHub if the local copy
    /// does not exist (e.g. first launch, git unavailable, clone failure).
    pub fn clone_or_pull_registries(
        registries: &[PluginRegistry],
        root: &Path,
    ) -> Result<(), String> {
        let _ = std::fs::create_dir_all(root);
        for reg in registries {
            let local = registry_local_path(root, &reg.url);
            if local.join(".git").exists() {
                if let Err(e) = GitService::pull_updates(&local) {
                    tracing::warn!("git pull failed for {}: {e}", reg.url);
                }
            } else if let Err(e) = git2::Repository::clone(&reg.url, &local) {
                tracing::error!(
                    "git clone failed for {}: {e} — will fall back to HTTP fetch",
                    reg.url
                );
            }
        }
        Ok(())
    }

    /// Load all `RegistryPlugin` entries from the locally-cloned registry repos.
    /// If a registry hasn't been cloned yet, fetches its manifest JSON files over
    /// HTTPS from GitHub's raw content server as a fallback.
    pub fn load_plugins_from_registries(
        registries: &[PluginRegistry],
        registries_path: &Path,
    ) -> Vec<RegistryPlugin> {
        let http_client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .user_agent("Pulsar-Native/1.0")
            .build()
            .ok();
        let mut seen = std::collections::HashSet::new();
        let mut result = Vec::new();
        for reg in registries {
            let local_dir = registry_local_path(registries_path, &reg.url);
            let plugins_dir = local_dir.join("plugins");
            if plugins_dir.exists() {
                // Read from locally-cloned repo
                if let Ok(entries) = std::fs::read_dir(&plugins_dir) {
                    for entry in entries.filter_map(|e| e.ok()) {
                        if let Some(plugin) =
                            Self::load_single_plugin_file(entry.path(), &reg.url, &mut seen)
                        {
                            result.push(plugin);
                        }
                    }
                    continue;
                }
            }
            // Fallback: fetch manifest files over HTTPS
            if let Some(ref client) = http_client {
                if let Some(list) = Self::fetch_registry_via_http(&reg.url, client) {
                    for plugin in list {
                        if seen.insert(plugin.repo_url.clone()) {
                            result.push(plugin);
                        }
                    }
                }
            }
        }
        result
    }

    fn load_single_plugin_file(
        path: std::path::PathBuf,
        registry_url: &str,
        seen: &mut std::collections::HashSet<String>,
    ) -> Option<RegistryPlugin> {
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            return None;
        }
        let text = std::fs::read_to_string(&path).ok()?;
        let mut plugin: RegistryPlugin = serde_json::from_str(&text).ok()?;
        plugin.registry_url = registry_url.to_string();
        if !seen.insert(plugin.repo_url.clone()) {
            return None;
        }
        Some(plugin)
    }

    /// Fetch all plugin manifest `.json` files from a registry GitHub repo via
    /// the GitHub Contents API and raw file downloads.
    fn fetch_registry_via_http(
        registry_url: &str,
        client: &reqwest::blocking::Client,
    ) -> Option<Vec<RegistryPlugin>> {
        // Extract owner/repo from a URL like https://github.com/owner/repo
        let trimmed = registry_url.trim_end_matches('/').trim_end_matches(".git");
        let (owner, repo) = trimmed
            .trim_start_matches("https://github.com/")
            .trim_start_matches("http://github.com/")
            .split_once('/')?;
        // List JSON files in the plugins/ directory via GitHub Contents API
        let list_url = format!("https://api.github.com/repos/{owner}/{repo}/contents/plugins");
        let resp = client.get(&list_url).send().ok()?;
        if !resp.status().is_success() {
            tracing::warn!("GitHub API returned {} for {list_url}", resp.status());
            return None;
        }
        let files: Vec<serde_json::Value> = resp.json().ok()?;
        let mut plugins = Vec::new();
        for file in files {
            let name = file.get("name")?.as_str()?;
            if !name.ends_with(".json") {
                continue;
            }
            let download_url = file.get("download_url")?.as_str()?;
            let raw_resp = client.get(download_url).send().ok()?;
            if !raw_resp.status().is_success() {
                continue;
            }
            let text: String = raw_resp.text().ok()?;
            if let Ok(mut plugin) = serde_json::from_str::<RegistryPlugin>(&text) {
                plugin.registry_url = registry_url.to_string();
                plugins.push(plugin);
            }
        }
        Some(plugins)
    }

    pub fn parse_github_owner_repo(url: &str) -> Option<(String, String)> {
        let stripped = url
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_start_matches("github.com/");
        let mut parts = stripped.splitn(2, '/');
        let owner = parts.next()?.to_string();
        let repo = parts.next()?.trim_end_matches(".git").to_string();
        if owner.is_empty() || repo.is_empty() {
            None
        } else {
            Some((owner, repo))
        }
    }

    pub fn fetch_latest_release(
        owner: &str,
        repo: &str,
    ) -> Result<Option<(String, Option<String>)>, String> {
        let url = format!("https://api.github.com/repos/{}/{}/releases", owner, repo);
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .user_agent("Pulsar-Native/1.0")
            .build()
            .map_err(|e| e.to_string())?;
        let resp = client.get(&url).send().map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("GitHub API returned {}", resp.status()));
        }
        let releases: serde_json::Value = resp.json().map_err(|e| e.to_string())?;
        let arr = match releases.as_array() {
            Some(a) if !a.is_empty() => a,
            _ => return Ok(None),
        };
        let latest = &arr[0];
        let tag = latest
            .get("tag_name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let ext = native_plugin_ext();
        let binary_url = latest
            .get("assets")
            .and_then(|a| a.as_array())
            .and_then(|assets| {
                assets.iter().find(|a| {
                    a.get("name")
                        .and_then(|n| n.as_str())
                        .map(|n| n.ends_with(ext))
                        .unwrap_or(false)
                })
            })
            .and_then(|a| a.get("browser_download_url").and_then(|v| v.as_str()))
            .map(String::from);
        Ok(Some((tag, binary_url)))
    }

    pub fn download_binary(
        url: &str,
        plugins_dir: &Path,
        lib_name: &str,
    ) -> Result<String, String> {
        use std::io::Write;
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .user_agent("Pulsar-Native/1.0")
            .build()
            .map_err(|e| e.to_string())?;
        let resp = client.get(url).send().map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("Download failed: HTTP {}", resp.status()));
        }
        let bytes = resp.bytes().map_err(|e| e.to_string())?;
        std::fs::create_dir_all(plugins_dir).map_err(|e| e.to_string())?;
        let dest = plugins_dir.join(lib_name);
        let mut file = std::fs::File::create(&dest).map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        Ok(dest.to_string_lossy().to_string())
    }

    pub fn build_from_source(
        repo_url: &str,
        tag: Option<&str>,
        plugins_dir: &Path,
        version: &str,
    ) -> Result<(String, Vec<String>), String> {
        use std::process::Command;
        let mut logs: Vec<String> = Vec::new();
        let tmp = std::env::temp_dir().join(format!(
            "pulsar_plugin_build_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
        logs.push(format!("Cloning {}\u{2026}", repo_url));
        let mut clone = Command::new("git");
        clone.args(["clone", "--depth", "1"]);
        if let Some(t) = tag {
            clone.args(["--branch", t]);
        }
        clone.args([repo_url, tmp.to_str().unwrap()]);
        let out = clone.output().map_err(|e| format!("git clone: {e}"))?;
        logs.push(String::from_utf8_lossy(&out.stderr).into_owned());
        if !out.status.success() {
            return Err(format!("git clone failed:\n{}", logs.join("\n")));
        }
        logs.push("Building with cargo\u{2026}".to_string());
        let build = Command::new("cargo")
            .args(["build", "--release"])
            .current_dir(&tmp)
            .output()
            .map_err(|e| format!("cargo build: {e}"))?;
        logs.push(String::from_utf8_lossy(&build.stderr).into_owned());
        if !build.status.success() {
            return Err(format!("cargo build failed:\n{}", logs.join("\n")));
        }
        let ext = native_plugin_ext();
        let release = tmp.join("target").join("release");
        let lib_file = std::fs::read_dir(&release)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| {
                p.extension().and_then(|e| e.to_str()) == Some(ext)
                    && !p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .starts_with('.')
            })
            .ok_or_else(|| format!("No .{} file found in target/release/", ext))?;
        let plugin_name = repo_url
            .split('/')
            .last()
            .unwrap_or("plugin")
            .replace(['-', '.'], "_");
        let dest_name = format!("{plugin_name}_{version}.{ext}");
        std::fs::create_dir_all(plugins_dir).map_err(|e| e.to_string())?;
        let dest = plugins_dir.join(&dest_name);
        std::fs::copy(&lib_file, &dest).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_dir_all(&tmp);
        logs.push(format!("Installed to {}", dest.display()));
        Ok((dest.to_string_lossy().to_string(), logs))
    }
}
