//! Starter content: downloadable content packs a plugin offers a project.
//!
//! A pack is not an object of a level, so it is not a Create entry. It is a
//! [`StarterContent`] registration: a pinned archive (URL, SHA-256, size
//! cap), the licence the author is shown before installing, where in the
//! project the files go, and the plugin's `install` that turns the verified
//! archive into project files (through `engine_fs::virtual_fs`). The shell
//! owns everything generic about getting it there: the download on a worker
//! thread with progress and cancel, the checksum, the state the menu and the
//! status notification read, and failure reporting. It knows no kind of
//! pack; the Tools menu lists whatever is registered
//! ([`extend_tools_menu`]).
//!
//! ```ignore
//! inventory::submit! {
//!     ui_level_editor::extend::StarterContent {
//!         origin: pulsar_plugins::origin!(),
//!         id: "sky.deep_space",
//!         title: "Sky Panoramas (CC0 Deep-Space)",
//!         license: "CC0 1.0",
//!         archive_url: "https://...", sha256: "...", max_bytes: 256 << 20,
//!         destination: "assets/sky/deep-space",
//!         order: 0,
//!         install: sky_editor::panoramas::install,
//!         ..
//!     }
//! }
//! ```

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use plugin_editor_api::EditorMenu;
use sha2::{Digest, Sha256};

/// A content pack a plugin offers (see the module docs). The catalog lists it
/// as `editor.starter_content`.
pub struct StarterContent {
    pub origin: &'static str,
    /// Stable id within the plugin (`"sky.deep_space"`): the catalog's id.
    pub id: &'static str,
    /// The menu text (the plugin's own words).
    pub title: &'static str,
    /// One line on what the pack holds.
    pub description: &'static str,
    /// The licence the author accepts by installing (`"CC0 1.0"`), and where
    /// its text is.
    pub license: &'static str,
    pub license_url: &'static str,
    /// The pinned archive, its SHA-256 (lowercase hex; shown in the menu) and
    /// the most the download may be.
    pub archive_url: &'static str,
    pub sha256: &'static str,
    pub max_bytes: u64,
    /// Where `install` puts the files, relative to the project folder.
    pub destination: &'static str,
    pub order: i32,
    /// Turns the verified archive into files under `destination`. Report
    /// failures as text; poll [`InstallJob::cancelled`] in long work.
    pub install: fn(&InstallRequest) -> Result<Vec<PathBuf>, String>,
}

super::extension_point!(
    StarterContent: "editor.starter_content", |r| r.id, r.title;
);

/// What `install` is handed.
pub struct InstallRequest<'a> {
    /// The downloaded archive, already checked against the pack's SHA-256.
    pub archive: Vec<u8>,
    /// The folder to write into: the project folder joined with the pack's
    /// `destination`.
    pub destination: PathBuf,
    pub job: &'a InstallJob,
}

/// Where an install is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Downloading { done: u64, total: Option<u64> },
    Verifying,
    Installing,
    Done { files: Vec<PathBuf> },
    Failed(String),
    Cancelled,
}

impl Phase {
    pub fn running(&self) -> bool {
        matches!(
            self,
            Phase::Downloading { .. } | Phase::Verifying | Phase::Installing
        )
    }

    /// The menu's one-line status.
    pub fn describe(&self) -> String {
        match self {
            Phase::Downloading {
                done,
                total: Some(total),
            } if *total > 0 => format!("Downloading {}%", done * 100 / total),
            Phase::Downloading { done, .. } => format!("Downloading {} MB", done >> 20),
            Phase::Verifying => "Verifying checksum".into(),
            Phase::Installing => "Installing".into(),
            Phase::Done { files } => format!("Installed {} files", files.len()),
            Phase::Failed(reason) => format!("Failed: {reason}"),
            Phase::Cancelled => "Cancelled".into(),
        }
    }
}

/// One install's shared state: the worker writes the phase, the editor reads
/// it and may cancel.
pub struct InstallJob {
    phase: Mutex<Phase>,
    cancel: AtomicBool,
}

impl Default for InstallJob {
    fn default() -> Self {
        Self::new()
    }
}

impl InstallJob {
    /// A job that has not started (the shell makes one per install; a test
    /// drives [`run`] with its own).
    pub fn new() -> Self {
        Self {
            phase: Mutex::new(Phase::Downloading {
                done: 0,
                total: None,
            }),
            cancel: AtomicBool::new(false),
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase.lock().unwrap().clone()
    }

    fn set(&self, phase: Phase) {
        *self.phase.lock().unwrap() = phase;
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

/// Why an install did not finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallError {
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallError::Cancelled => f.write_str("cancelled"),
            InstallError::Failed(reason) => f.write_str(reason),
        }
    }
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Checks `bytes` against `expected` (lowercase hex).
pub fn verify_sha256(bytes: &[u8], expected: &str) -> Result<(), String> {
    let got = sha256_hex(bytes);
    if got == expected {
        Ok(())
    } else {
        Err(format!(
            "the archive's checksum does not match (expected {expected}, got {got})"
        ))
    }
}

/// Downloads `url` (at most `max_bytes`), reporting progress and stopping
/// when `job` is cancelled. Blocking.
fn download(url: &str, max_bytes: u64, job: &InstallJob) -> Result<Vec<u8>, InstallError> {
    let failed = |e: String| InstallError::Failed(format!("download {url}: {e}"));
    let response = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(30))
        .timeout_read(std::time::Duration::from_secs(60))
        .build()
        .get(url)
        .call()
        .map_err(|e| failed(e.to_string()))?;
    let total = response
        .header("Content-Length")
        .and_then(|length| length.parse::<u64>().ok());
    if total.is_some_and(|total| total > max_bytes) {
        return Err(failed(format!("larger than the {max_bytes} byte limit")));
    }
    let mut reader = response.into_reader();
    let mut bytes = Vec::with_capacity(total.unwrap_or(0).min(max_bytes) as usize);
    let mut chunk = vec![0u8; 64 << 10];
    loop {
        if job.cancelled() {
            return Err(InstallError::Cancelled);
        }
        let read = reader.read(&mut chunk).map_err(|e| failed(e.to_string()))?;
        if read == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.len() as u64 > max_bytes {
            return Err(failed(format!("larger than the {max_bytes} byte limit")));
        }
        job.set(Phase::Downloading {
            done: bytes.len() as u64,
            total,
        });
    }
}

/// Runs `pack` for the project at `project_root`: download, verify, install.
/// Blocking; the job's phase tracks it. The outcome is also the job's final
/// phase.
pub fn run(
    pack: &StarterContent,
    project_root: &Path,
    job: &InstallJob,
) -> Result<Vec<PathBuf>, InstallError> {
    let outcome = (|| {
        let archive = download(pack.archive_url, pack.max_bytes, job)?;
        job.set(Phase::Verifying);
        verify_sha256(&archive, pack.sha256).map_err(InstallError::Failed)?;
        if job.cancelled() {
            return Err(InstallError::Cancelled);
        }
        job.set(Phase::Installing);
        (pack.install)(&InstallRequest {
            archive,
            destination: project_root.join(pack.destination),
            job,
        })
        .map_err(InstallError::Failed)
    })();
    job.set(match &outcome {
        Ok(files) => Phase::Done {
            files: files.clone(),
        },
        Err(InstallError::Cancelled) => Phase::Cancelled,
        Err(InstallError::Failed(reason)) => Phase::Failed(reason.clone()),
    });
    outcome
}

// ── What the editor sees ────────────────────────────────────────────────────

/// Every registered pack, in order.
pub fn starter_contents() -> Vec<&'static StarterContent> {
    super::sorted::<StarterContent>(|pack| pack.order)
}

fn jobs() -> &'static Mutex<HashMap<&'static str, Arc<InstallJob>>> {
    static JOBS: OnceLock<Mutex<HashMap<&'static str, Arc<InstallJob>>>> = OnceLock::new();
    JOBS.get_or_init(Default::default)
}

/// The latest install of pack `id` in this session, if any.
pub fn job(id: &str) -> Option<Arc<InstallJob>> {
    jobs().lock().unwrap().get(id).cloned()
}

/// Starts installing pack `id` into the open project on a worker thread.
/// `Err` is what to tell the author when it cannot start.
pub fn start(id: &str) -> Result<Arc<InstallJob>, String> {
    let pack = pulsar_plugins::registered::<StarterContent>()
        .find(|pack| pack.id == id)
        .ok_or("Nothing in this project offers that content")?;
    let root = engine_state::get_project_path()
        .map(PathBuf::from)
        .ok_or("Open a project first: the content is added to its assets")?;
    let job = Arc::new(InstallJob::new());
    {
        let mut jobs = jobs().lock().unwrap();
        if jobs.get(pack.id).is_some_and(|job| job.phase().running()) {
            return Err(format!("{} is already being added", pack.title));
        }
        jobs.insert(pack.id, job.clone());
    }
    let worker = job.clone();
    std::thread::Builder::new()
        .name(format!("starter-content-{}", pack.id))
        .spawn(move || match run(pack, &root, &worker) {
            Ok(files) => tracing::info!(
                "added {}: {}",
                pack.title,
                files
                    .iter()
                    .map(|file| file.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Err(InstallError::Cancelled) => tracing::info!("cancelled adding {}", pack.title),
            Err(error) => tracing::warn!("could not add {}: {error}", pack.title),
        })
        .map_err(|e| {
            job.set(Phase::Failed(e.to_string()));
            format!("Could not start the download: {e}")
        })?;
    Ok(job)
}

// ── Actions and the Tools menu ──────────────────────────────────────────────

/// Install pack `id` (the Tools menu's entry).
#[derive(gpui::Action, Clone, PartialEq, serde::Deserialize, schemars::JsonSchema)]
#[action(namespace = level_editor)]
pub struct InstallStarterContent {
    pub id: String,
}

/// Cancel pack `id`'s running install.
#[derive(gpui::Action, Clone, PartialEq, serde::Deserialize, schemars::JsonSchema)]
#[action(namespace = level_editor)]
pub struct CancelStarterContent {
    pub id: String,
}

/// The panel's handler for [`InstallStarterContent`]: starts the install,
/// tells the author, and reports how it ended once it has.
pub fn on_install_starter_content<T: 'static>(
    _: &mut T,
    action: &InstallStarterContent,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<T>,
) {
    use ui::{notification::Notification, ContextModal as _};
    let Some(pack) = pulsar_plugins::registered::<StarterContent>().find(|p| p.id == action.id)
    else {
        return;
    };
    let job = match start(&action.id) {
        Ok(job) => job,
        Err(reason) => {
            window.push_notification(Notification::warning(reason).title(pack.title), cx);
            return;
        }
    };
    window.push_notification(
        Notification::info(format!(
            "Downloading into {} ({}). Cancel it from Tools > Starter Content.",
            pack.destination, pack.license
        ))
        .title(pack.title),
        cx,
    );
    cx.spawn_in(window, async move |_this, cx| {
        let phase = loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(250))
                .await;
            let phase = job.phase();
            if !phase.running() {
                break phase;
            }
        };
        cx.update(|window, cx| {
            let note = match &phase {
                Phase::Done { files } => Notification::success(format!(
                    "Added {} files to {}.",
                    files.len(),
                    pack.destination
                )),
                Phase::Cancelled => Notification::info("Cancelled."),
                other => Notification::error(other.describe()),
            };
            window.push_notification(note.title(pack.title), cx);
        })
        .ok();
    })
    .detach();
}

/// The panel's handler for [`CancelStarterContent`].
pub fn on_cancel_starter_content<T: 'static>(
    _: &mut T,
    action: &CancelStarterContent,
    _: &mut gpui::Window,
    _: &mut gpui::Context<T>,
) {
    if let Some(job) = job(&action.id) {
        job.cancel();
    }
}

/// Adds the "Starter Content" submenu to `menu` when any plugin registers a
/// pack: per pack its install (or cancel while running), the status, and the
/// licence and checksum it is pinned to. Built when the menu opens, so the
/// progress shown is current.
pub fn extend_tools_menu(menu: EditorMenu) -> EditorMenu {
    let packs = starter_contents();
    if packs.is_empty() {
        return menu;
    }
    let mut content = EditorMenu::new("Starter Content");
    for (index, pack) in packs.into_iter().enumerate() {
        if index > 0 {
            content = content.separator();
        }
        let phase = job(pack.id).map(|job| job.phase());
        content = match &phase {
            Some(phase) if phase.running() => content.action(
                format!("Cancel: {} ({})", pack.title, phase.describe()),
                CancelStarterContent {
                    id: pack.id.to_string(),
                },
            ),
            _ => content.action(
                format!("Add {}", pack.title),
                InstallStarterContent {
                    id: pack.id.to_string(),
                },
            ),
        };
        if let Some(phase @ (Phase::Failed(_) | Phase::Done { .. } | Phase::Cancelled)) = &phase {
            content = content.action_if(
                format!("   Last attempt: {}", phase.describe()),
                CancelStarterContent {
                    id: pack.id.to_string(),
                },
                false,
            );
        }
        let info = |text: String| {
            (
                text,
                CancelStarterContent {
                    id: pack.id.to_string(),
                },
            )
        };
        for (text, action) in [
            info(format!("   {}", pack.description)),
            info(format!(
                "   Licence: {} ({})",
                pack.license, pack.license_url
            )),
            info(format!("   Into: {}", pack.destination)),
            info(format!("   SHA-256: {}", pack.sha256)),
        ] {
            content = content.action_if(text, action, false);
        }
    }
    menu.separator().submenu(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;

    /// Serves `body` once per connection on a loopback port, slowly when
    /// `slow` (so a cancel can land mid-download).
    fn serve(body: Vec<u8>, slow: bool) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/pack.bin", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let body = body.clone();
                std::thread::spawn(move || {
                    let mut request = [0u8; 1024];
                    let _ = stream.read(&mut request);
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    for chunk in body.chunks(64 << 10) {
                        if stream.write_all(chunk).is_err() {
                            return;
                        }
                        if slow {
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                    }
                });
            }
        });
        url
    }

    fn pack(url: &'static str, sha256: &'static str, max_bytes: u64) -> StarterContent {
        StarterContent {
            origin: "test",
            id: "test.pack",
            title: "Test pack",
            description: "",
            license: "CC0 1.0",
            license_url: "",
            archive_url: url,
            sha256,
            max_bytes,
            destination: "assets/test",
            order: 0,
            install: |request| {
                std::fs::create_dir_all(&request.destination).map_err(|e| e.to_string())?;
                let file = request.destination.join("pack.bin");
                std::fs::write(&file, &request.archive).map_err(|e| e.to_string())?;
                Ok(vec![file])
            },
        }
    }

    fn leak(text: String) -> &'static str {
        Box::leak(text.into_boxed_str())
    }

    #[test]
    fn a_verified_archive_is_installed_under_the_destination() {
        let body = vec![7u8; 300_000];
        let url = leak(serve(body.clone(), false));
        let pack = pack(url, leak(sha256_hex(&body)), 1 << 20);
        let root = tempfile::tempdir().unwrap();
        let job = InstallJob::new();
        let files = run(&pack, root.path(), &job).unwrap();
        assert_eq!(files, vec![root.path().join("assets/test/pack.bin")]);
        assert_eq!(std::fs::read(&files[0]).unwrap(), body);
        assert_eq!(job.phase(), Phase::Done { files });
    }

    #[test]
    fn a_checksum_mismatch_installs_nothing_and_is_reported() {
        let url = leak(serve(vec![1u8; 1000], false));
        let pack = pack(url, "00", 1 << 20);
        let root = tempfile::tempdir().unwrap();
        let job = InstallJob::new();
        let error = run(&pack, root.path(), &job).unwrap_err();
        assert!(
            matches!(&error, InstallError::Failed(reason) if reason.contains("checksum")),
            "{error}"
        );
        assert!(matches!(job.phase(), Phase::Failed(_)));
        assert!(!root.path().join("assets").exists());
    }

    #[test]
    fn an_unreachable_server_is_a_failure_not_a_panic() {
        let pack = pack("http://127.0.0.1:1/none", "00", 1 << 20);
        let root = tempfile::tempdir().unwrap();
        let job = InstallJob::new();
        assert!(matches!(
            run(&pack, root.path(), &job),
            Err(InstallError::Failed(_))
        ));
        assert!(matches!(job.phase(), Phase::Failed(_)));
    }

    #[test]
    fn a_download_over_the_limit_is_refused() {
        let url = leak(serve(vec![1u8; 100_000], false));
        let pack = pack(url, "00", 1000);
        let root = tempfile::tempdir().unwrap();
        let job = InstallJob::new();
        let error = run(&pack, root.path(), &job).unwrap_err();
        assert!(error.to_string().contains("limit"), "{error}");
    }

    #[test]
    fn cancelling_mid_download_stops_it_and_writes_nothing() {
        let body = vec![3u8; 2 << 20];
        let url = leak(serve(body.clone(), true));
        let pack = pack(url, leak(sha256_hex(&body)), 4 << 20);
        let root = tempfile::tempdir().unwrap();
        let job = Arc::new(InstallJob::new());
        let stopper = job.clone();
        std::thread::spawn(move || {
            while !matches!(stopper.phase(), Phase::Downloading { done, .. } if done > 0) {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            stopper.cancel();
        });
        assert_eq!(
            run(&pack, root.path(), &job).unwrap_err(),
            InstallError::Cancelled
        );
        assert_eq!(job.phase(), Phase::Cancelled);
        assert!(!root.path().join("assets").exists());
    }

    #[test]
    fn progress_describes_percent_when_the_size_is_known() {
        assert_eq!(
            Phase::Downloading {
                done: 50,
                total: Some(200)
            }
            .describe(),
            "Downloading 25%"
        );
        assert_eq!(Phase::Failed("x".into()).describe(), "Failed: x");
    }
}
