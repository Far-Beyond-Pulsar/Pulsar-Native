//! HTTP-backed filesystem provider for `pulsar-host` servers.
//!
//! [`RemoteFsProvider`] talks to the file API that `pulsar-host` exposes under
//! `/api/v1/projects/:id/files/…`.  All operations are synchronous (blocking)
//! so they can be used from any thread without an async runtime.

use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use tracing::debug;

use super::provider_trait::{FsEntry, FsMetadata, FsProvider, ManifestEntry};

// ── Wire types ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ApiDirEntry {
    name: String,
    is_dir: bool,
    size: u64,
    modified: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ApiReadResponse {
    /// Base64-encoded file contents.
    content: String,
    size: u64,
}

#[derive(Debug, Serialize)]
struct ApiWriteRequest<'a> {
    /// Base64-encoded file contents.
    content: &'a str,
}

#[derive(Debug, Serialize)]
struct ApiRenameRequest {
    from: String,
    to: String,
}

#[derive(Debug, Deserialize)]
struct ApiExistsResponse {
    exists: bool,
}

#[derive(Debug, Deserialize)]
struct ApiStatResponse {
    is_dir: bool,
    size: u64,
    modified: Option<u64>,
}

// ── RemoteConfig ──────────────────────────────────────────────────────────────

/// Connection details required to reach a `pulsar-host` project over HTTP.
#[derive(Debug, Clone)]
pub struct RemoteConfig {
    /// Base HTTP URL, e.g. `http://studio.example.com:7700`.
    pub server_url: String,
    /// UUID of the project on the host server.
    pub project_id: String,
    /// Optional Bearer token for password-protected servers.
    pub auth_token: Option<String>,
}

impl RemoteConfig {
    /// Parse a `cloud+pulsar://HOST/PROJECT_ID[/optional/subpath]` path.
    ///
    /// Returns `None` when the scheme does not match.
    pub fn from_cloud_path(path: &Path) -> Option<Self> {
        // Normalize backslashes so the URI scheme prefix check works on Windows
        // where PathBuf may have stored "cloud+pulsar:\\host\\proj".
        let s = path.to_string_lossy().replace('\\', "/");
        let without_scheme = s.strip_prefix("cloud+pulsar://")?;
        let slash = without_scheme.find('/')?;
        let host = &without_scheme[..slash];
        let rest = &without_scheme[slash + 1..];
        let project_id_end = rest.find('/').unwrap_or(rest.len());
        let project_id = rest[..project_id_end].to_string();
        if project_id.is_empty() {
            return None;
        }
        Some(RemoteConfig {
            server_url: format!("http://{}", host),
            project_id,
            auth_token: None,
        })
    }

    /// Builder-style setter for the auth token.
    pub fn with_token(mut self, token: Option<String>) -> Self {
        self.auth_token = token;
        self
    }
}

// ── RemoteFsProvider ──────────────────────────────────────────────────────────

/// HTTP-backed [`FsProvider`] that reads and writes files on a `pulsar-host`.
pub struct RemoteFsProvider {
    config: Arc<RemoteConfig>,
    agent: ureq::Agent,
}

impl RemoteFsProvider {
    pub fn new(config: RemoteConfig) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(30))
            .build();
        Self {
            config: Arc::new(config),
            agent,
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    fn files_base(&self) -> String {
        format!(
            "{}/api/v1/projects/{}/files",
            self.config.server_url, self.config.project_id
        )
    }

    /// Inject the `Authorization` header when the config carries a token.
    fn auth(&self, req: ureq::Request) -> ureq::Request {
        if let Some(ref tok) = self.config.auth_token {
            req.set("Authorization", &format!("Bearer {}", tok))
        } else {
            req
        }
    }

    /// Convert `path` to a forward-slash-separated string relative to the
    /// project root, stripping any leading `cloud+pulsar://HOST/PROJECT_ID`.
    ///
    /// Also rejects path-traversal attempts (`..`).
    fn to_rel(&self, path: &Path) -> Result<String> {
        // Normalise Windows backslashes → forward slashes *before* any string
        // comparisons.  PathBuf::join() on Windows inserts '\' between
        // components, which would otherwise cause the scheme-prefix strip and
        // subsequent '/' searches to silently produce an empty relative path.
        let s = path.to_string_lossy().replace('\\', "/");

        let rel = if let Some(tail) = s.strip_prefix("cloud+pulsar://") {
            // Strip  HOST / PROJECT_ID /  to get the bare relative path.
            let after_host = tail.find('/').map(|i| &tail[i + 1..]).unwrap_or("");
            let after_proj = after_host
                .find('/')
                .map(|i| &after_host[i + 1..])
                .unwrap_or("");
            after_proj.to_string()
        } else {
            // Already a relative path (or some other non-cloud form).
            s
        };

        if rel.split('/').any(|seg| seg == "..") {
            bail!("Path traversal not allowed: {}", path.display());
        }

        Ok(rel)
    }

    fn encode(s: &str) -> String {
        urlencoding::encode(s).into_owned()
    }
}

impl FsProvider for RemoteFsProvider {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>> {
        let rel = self.to_rel(path)?;
        debug!("RemoteFs: read_file {rel:?}");
        let url = format!("{}?path={}", self.files_base(), Self::encode(&rel));
        let resp = self
            .auth(self.agent.get(&url))
            .call()
            .context("RemoteFs read HTTP call failed")?;
        let body: ApiReadResponse = resp
            .into_json()
            .context("RemoteFs read: JSON parse error")?;
        decode_b64(&body.content)
    }

    fn write_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        let rel = self.to_rel(path)?;
        debug!("RemoteFs: write_file {rel:?} ({} bytes)", content.len());
        let url = format!("{}?path={}", self.files_base(), Self::encode(&rel));
        let b64 = encode_b64(content);
        self.auth(self.agent.put(&url))
            .set("Content-Type", "application/json")
            .send_json(ureq::serde_json::json!({ "content": b64 }))
            .context("RemoteFs write HTTP call failed")?;
        Ok(())
    }

    fn create_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        let rel = self.to_rel(path)?;
        debug!("RemoteFs: create_file {rel:?}");
        let url = format!(
            "{}?path={}&create=true",
            self.files_base(),
            Self::encode(&rel)
        );
        let b64 = encode_b64(content);
        self.auth(self.agent.put(&url))
            .set("Content-Type", "application/json")
            .send_json(ureq::serde_json::json!({ "content": b64 }))
            .context("RemoteFs create HTTP call failed")?;
        Ok(())
    }

    fn delete_path(&self, path: &Path) -> Result<()> {
        let rel = self.to_rel(path)?;
        debug!("RemoteFs: delete_path {rel:?}");
        let url = format!("{}?path={}", self.files_base(), Self::encode(&rel));
        self.auth(self.agent.delete(&url))
            .call()
            .context("RemoteFs delete HTTP call failed")?;
        Ok(())
    }

    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        let from_rel = self.to_rel(from)?;
        let to_rel = self.to_rel(to)?;
        debug!("RemoteFs: rename {from_rel:?} -> {to_rel:?}");
        let url = format!("{}/rename", self.files_base());
        self.auth(self.agent.post(&url))
            .set("Content-Type", "application/json")
            .send_json(ureq::serde_json::json!({ "from": from_rel, "to": to_rel }))
            .context("RemoteFs rename HTTP call failed")?;
        Ok(())
    }

    fn list_dir(&self, path: &Path) -> Result<Vec<FsEntry>> {
        let rel = self.to_rel(path)?;
        debug!("RemoteFs: list_dir {rel:?}");
        let url = if rel.is_empty() {
            format!("{}/list", self.files_base())
        } else {
            format!("{}/list?path={}", self.files_base(), Self::encode(&rel))
        };
        let resp = self
            .auth(self.agent.get(&url))
            .call()
            .context("RemoteFs list HTTP call failed")?;
        let entries: Vec<ApiDirEntry> = resp
            .into_json()
            .context("RemoteFs list: JSON parse error")?;
        Ok(entries
            .into_iter()
            .map(|e| FsEntry {
                name: e.name,
                is_dir: e.is_dir,
                size: e.size,
                modified: e.modified,
            })
            .collect())
    }

    fn create_dir_all(&self, path: &Path) -> Result<()> {
        let rel = self.to_rel(path)?;
        debug!("RemoteFs: create_dir_all {rel:?}");
        let url = format!("{}/mkdir?path={}", self.files_base(), Self::encode(&rel));
        self.auth(self.agent.post(&url))
            .call()
            .context("RemoteFs mkdir HTTP call failed")?;
        Ok(())
    }

    fn exists(&self, path: &Path) -> Result<bool> {
        let rel = self.to_rel(path)?;
        let url = format!("{}/exists?path={}", self.files_base(), Self::encode(&rel));
        match self.auth(self.agent.get(&url)).call() {
            Ok(resp) => {
                let r: ApiExistsResponse = resp
                    .into_json()
                    .context("RemoteFs exists: JSON parse error")?;
                Ok(r.exists)
            }
            Err(_) => Ok(false),
        }
    }

    fn metadata(&self, path: &Path) -> Result<FsMetadata> {
        let rel = self.to_rel(path)?;
        let url = format!("{}/stat?path={}", self.files_base(), Self::encode(&rel));
        let resp = self
            .auth(self.agent.get(&url))
            .call()
            .context("RemoteFs stat HTTP call failed")?;
        let r: ApiStatResponse = resp
            .into_json()
            .context("RemoteFs stat: JSON parse error")?;
        Ok(FsMetadata {
            is_dir: r.is_dir,
            size: r.size,
            modified: r.modified,
        })
    }

    fn manifest(&self, path: &Path) -> Result<Vec<ManifestEntry>> {
        // Hit the dedicated manifest endpoint for a single round-trip.
        let rel = self.to_rel(path)?;
        let url = if rel.is_empty() {
            format!("{}/manifest", self.files_base())
        } else {
            format!("{}/manifest?path={}", self.files_base(), Self::encode(&rel))
        };
        let resp = self
            .auth(self.agent.get(&url))
            .call()
            .context("RemoteFs manifest HTTP call failed")?;
        let entries: Vec<ManifestEntry> = resp
            .into_json()
            .context("RemoteFs manifest: JSON parse error")?;
        Ok(entries)
    }

    fn is_remote(&self) -> bool {
        true
    }

    fn label(&self) -> &str {
        "Remote"
    }
}

// ── Base64 helpers ─────────────────────────────────────────────────────────────

fn encode_b64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn decode_b64(s: &str) -> Result<Vec<u8>> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .context("base64 decode error")
}
