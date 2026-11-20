//! Packaging a Pulsar project as a standalone game.
//!
//! ```text
//! pulsar package --project <dir> --out <dir> [--profile dev|shipping] [--target <triple>]
//!                [--loose] [--skip-build] [--startup-level <path>]
//! pulsar build-scripts --project <dir>
//! ```
//!
//! [`package`] runs, in order:
//!
//! 1. **Scripts** ([`compile`]): every linked scripting language compiles
//!    the project headlessly, then every class module is loaded into a
//!    runtime with the engine's natives, event hub and the project's
//!    capability allowlist. Any error stops packaging.
//! 2. **Cook** ([`cook`]): classes (GUID index, `class.json`, cooked
//!    `prefab.json`, the module in the binary encoding), levels (class
//!    GUIDs resolved, editor-only data dropped, asset paths
//!    content-relative), `Pulsar/scripting.json`, a cooked
//!    `Pulsar/project.json` (profile, startup level) and every referenced
//!    asset, with `Pulsar/asset_registry.json`.
//!    No shipped file may name an absolute path or `CARGO_MANIFEST_DIR`.
//! 3. **Write** the pak set in `<out>/Content/` (`pulsar_content::pak`:
//!    compressed, encrypted with the project's content key, in containers
//!    of bounded size; only changed entries are written again), or loose
//!    files with `--loose`.
//! 4. **Build** the game binary (`cargo build --release` of the project)
//!    and copy it to `<out>/` (skipped with `--skip-build`).
//!
//! The packaged game finds `Content/` next to its executable
//! (`pulsar_content::ContentRoot::discover`).

pub mod compile;
mod content;
pub mod cook;

// Links every installed plugin's registrations (record migrations among them).
use installed_plugins as _;

use content::cook_content;
pub use content::machine_path_in;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use engine_fs::virtual_fs;
use pulsar_content::{
    BuildProfile, CONTENT_DIR_NAME, ContentKey, PROJECT_SETTINGS_FILE, PackSettings,
    ProjectSettings, TOC_FILE_NAME, write_pak_set,
};
pub use pulsar_content::{PackEvent, PackReport};
use serde::Serialize;

/// What to package, and how.
#[derive(Clone, Debug)]
pub struct PackageOptions {
    /// The project directory.
    pub project: PathBuf,
    /// Output directory: gets the game executable and `Content/`.
    pub out: PathBuf,
    pub profile: BuildProfile,
    /// Plugin cooking tradeoff. Fast favors iteration; optimized asks each
    /// registered file handler and script language for production output.
    pub optimization: plugin_editor_api::CookOptimization,
    /// Rust target triple for the game binary (default: the host).
    pub target: Option<String>,
    /// Write `Content/` as loose files instead of a pak set.
    pub loose: bool,
    /// Do not build the game binary (content only).
    pub skip_build: bool,
    /// The startup level (project-relative), overriding the project's.
    pub startup_level: Option<String>,
    /// The engine's built-in assets directory (primitives referenced by
    /// levels); default: this engine checkout's `assets/`.
    pub engine_assets: Option<PathBuf>,
}

impl PackageOptions {
    pub fn new(project: impl Into<PathBuf>, out: impl Into<PathBuf>) -> Self {
        Self {
            project: project.into(),
            out: out.into(),
            profile: BuildProfile::Dev,
            optimization: plugin_editor_api::CookOptimization::Fast,
            target: None,
            loose: false,
            skip_build: false,
            startup_level: None,
            engine_assets: None,
        }
    }
}

/// Why packaging failed.
#[derive(Debug, thiserror::Error)]
pub enum PackageError {
    #[error("{0}")]
    Project(String),
    #[error("script errors:\n{}", .0.iter().map(|p| format!("  {p}")).collect::<Vec<_>>().join("\n"))]
    Scripts(Vec<compile::ScriptProblem>),
    #[error("cooking failed: {0}")]
    Cook(String),
    #[error("shipped content names machine paths:\n{}", .0.iter().map(|p| format!("  {p}")).collect::<Vec<_>>().join("\n"))]
    AbsolutePaths(Vec<String>),
    #[error("writing content failed: {0}")]
    Write(String),
    #[error("building the game failed: {0}")]
    Build(String),
}

/// What [`package`] produced.
#[derive(Debug, Default, Serialize)]
pub struct PackageReport {
    pub out: PathBuf,
    pub profile: String,
    pub optimization: plugin_editor_api::CookOptimization,
    /// The pak, or `None` for loose content.
    pub pak: Option<PathBuf>,
    /// The game executable, unless the build was skipped.
    pub executable: Option<PathBuf>,
    pub startup_level: Option<String>,
    /// Classes shipped, as `(name, GUID, has a script)`.
    pub classes: Vec<(String, String, bool)>,
    pub levels: Vec<String>,
    pub assets: usize,
    /// Every shipped file, content-relative, with its size.
    pub files: BTreeMap<String, u64>,
    pub warnings: Vec<String>,
}

/// The engine's own `assets/` directory, where built-in assets (mesh
/// primitives, the default level's meshes) live. A packaging-time lookup
/// only: shipped content never names it.
// std::fs allowed: looks up the engine's own source checkout on the real machine, not a project
#[allow(clippy::disallowed_methods)]
pub fn default_engine_assets() -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../assets");
    dir.is_dir().then(|| dir.canonicalize().unwrap_or(dir))
}

/// Compile a project's scripts and verify them (`pulsar build-scripts`).
pub fn build_scripts(
    project: &Path,
    profile: BuildProfile,
) -> Result<compile::CompileOutput, PackageError> {
    pulsar_plugins::activate_project(project, engine_state::project_plugins(project));
    let settings = project_settings(project, profile)?;
    let output = compile::compile_project(
        project,
        &settings,
        plugin_editor_api::CookOptimization::Fast,
    );
    if output.has_errors() {
        return Err(PackageError::Scripts(output.problems));
    }
    Ok(output)
}

fn project_settings(
    project: &Path,
    profile: BuildProfile,
) -> Result<ProjectSettings, PackageError> {
    let path = project.join(PROJECT_SETTINGS_FILE);
    let mut settings = match virtual_fs::read_file(&path) {
        Ok(bytes) => {
            ProjectSettings::from_json(&bytes).map_err(|e| PackageError::Project(e.to_string()))?
        }
        Err(_) => ProjectSettings::default(),
    };
    settings.profile = profile;
    Ok(settings)
}

/// Package a project. See the crate doc.
pub fn package(options: &PackageOptions) -> Result<PackageReport, PackageError> {
    let project = virtual_fs::canonicalize(&options.project).map_err(|e| {
        PackageError::Project(format!("project {}: {e}", options.project.display()))
    })?;
    let mut report = PackageReport {
        profile: options.profile.as_str().into(),
        optimization: options.optimization,
        ..Default::default()
    };
    let content = cook_content(&project, options, &mut report)?;

    // 3. Write.
    let content_dir = options.out.join(CONTENT_DIR_NAME);
    if options.loose {
        reset_output_dir(&content_dir)
            .map_err(|e| PackageError::Write(format!("{}: {e}", content_dir.display())))?;
        for (rel, bytes) in &content {
            write_loose(&content_dir, rel, bytes)?;
        }
    } else {
        let key =
            ContentKey::load_or_create(&project).map_err(|e| PackageError::Write(e.to_string()))?;
        let pack = write_pak_set(
            &content_dir,
            &content,
            key,
            pack_settings(options.profile),
            &mut |event| log_pack_event(&event),
            &|| false,
        )
        .map_err(|e| PackageError::Write(e.to_string()))?;
        log_pack_report(&pack);
        report.pak = Some(content_dir.join(TOC_FILE_NAME));
    }
    report.files = content
        .iter()
        .map(|(rel, bytes)| (rel.clone(), bytes.len() as u64))
        .collect();
    report.out = options.out.clone();

    // 4. Build.
    if options.skip_build {
        tracing::warn!("--skip-build: the game executable was not built");
    } else {
        engine_backend::services::ensure_core_bootstrap(&project).map_err(PackageError::Build)?;
        report.executable = Some(build_game(
            &project,
            &options.out,
            options.target.as_deref(),
        )?);
    }
    Ok(report)
}

/// What [`pack_content`] did.
#[derive(Debug, Default)]
pub struct PackContentReport {
    pub pack: PackReport,
    pub warnings: Vec<String>,
    pub classes: usize,
    pub levels: usize,
    pub assets: usize,
}

/// Cook the project and pack it as the pak set in `content_dir`, reusing
/// what the set there already holds (only changed entries are written; see
/// `pulsar_content::pak`). The editor's build runs this before Cargo, into
/// the `Content/` folder next to where the game binary is built. Progress
/// and results are logged under the `pack` target; `cancelled` is polled
/// while packing.
pub fn pack_content(
    project: &Path,
    content_dir: &Path,
    profile: BuildProfile,
    on_event: &mut dyn FnMut(&PackEvent),
    cancelled: &dyn Fn() -> bool,
) -> Result<PackContentReport, PackageError> {
    let project = virtual_fs::canonicalize(project)
        .map_err(|e| PackageError::Project(format!("project {}: {e}", project.display())))?;
    let mut options = PackageOptions::new(&project, content_dir.parent().unwrap_or(content_dir));
    options.profile = profile;
    let mut report = PackageReport::default();
    tracing::info!(target: "pack", "Cooking {}", project.display());
    let content = cook_content(&project, &options, &mut report)?;
    for warning in &report.warnings {
        tracing::warn!(target: "pack", "{warning}");
    }
    tracing::info!(
        target: "pack",
        "Cooked {} classes, {} levels, {} assets ({} files)",
        report.classes.len(),
        report.levels.len(),
        report.assets,
        content.len()
    );
    let key =
        ContentKey::load_or_create(&project).map_err(|e| PackageError::Write(e.to_string()))?;
    let pack = write_pak_set(
        content_dir,
        &content,
        key,
        pack_settings(profile),
        &mut |event| {
            log_pack_event(&event);
            on_event(&event);
        },
        cancelled,
    )
    .map_err(|e| PackageError::Write(e.to_string()))?;
    log_pack_report(&pack);
    Ok(PackContentReport {
        pack,
        warnings: report.warnings,
        classes: report.classes.len(),
        levels: report.levels.len(),
        assets: report.assets,
    })
}

// The editor build's pack step (`pulsar_content::packer`): this crate is the
// host that can cook, so it is the packer of any binary that links it.
pulsar_plugins::inventory::submit! {
    pulsar_content::ContentPacker {
        origin: pulsar_plugins::engine_origin!(),
        pack: pack_for_build,
    }
}

fn pack_for_build(job: &pulsar_content::PackJob<'_>) -> Result<String, String> {
    let profile = if job.shipping {
        BuildProfile::Shipping
    } else {
        BuildProfile::Dev
    };
    (job.progress)(0.0, "Cooking content".into());
    let report = pack_content(
        job.project_root,
        job.content_dir,
        profile,
        &mut |event| match event {
            PackEvent::Planned {
                changed, reused, ..
            } => (job.progress)(
                0.3,
                format!("Packing {changed} changed entries ({reused} unchanged)"),
            ),
            PackEvent::Packed { done, total } => (job.progress)(
                0.3 + 0.7 * (*done as f32 / (*total).max(1) as f32),
                format!("Packed {:.1} of {:.1} MiB", mib(*done), mib(*total)),
            ),
            PackEvent::ContainerWritten { .. } | PackEvent::Removed { .. } => {}
        },
        job.cancelled,
    )
    .map_err(|e| e.to_string())?;
    (job.progress)(1.0, String::new());
    Ok(format!(
        "{} entries in {} containers, {:.1} MiB ({} written, {} reused)",
        report.pack.entries,
        report.pack.containers.len(),
        mib(report.pack.total_stored),
        report.pack.written,
        report.pack.reused
    ))
}

fn pack_settings(profile: BuildProfile) -> PackSettings {
    match profile {
        BuildProfile::Dev => PackSettings::DEV,
        BuildProfile::Shipping => PackSettings::SHIPPING,
    }
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn log_pack_event(event: &PackEvent) {
    match event {
        PackEvent::Planned {
            entries,
            reused,
            changed,
            changed_bytes,
            rewrite,
        } => match rewrite {
            Some(why) => tracing::info!(
                target: "pack",
                "Packing all {entries} entries ({:.1} MiB): {why}",
                mib(*changed_bytes)
            ),
            None => tracing::info!(
                target: "pack",
                "Packing {changed} changed entries ({:.1} MiB); {reused} unchanged stay in place",
                mib(*changed_bytes)
            ),
        },
        PackEvent::Packed { done, total } => {
            tracing::info!(target: "pack", "Packed {:.1} of {:.1} MiB", mib(*done), mib(*total))
        }
        PackEvent::ContainerWritten { file, bytes } => {
            tracing::info!(target: "pack", "Wrote {file} ({:.1} MiB)", mib(*bytes))
        }
        PackEvent::Removed { file } => tracing::info!(target: "pack", "Removed {file}"),
    }
}

fn log_pack_report(pack: &PackReport) {
    tracing::info!(
        target: "pack",
        "Pak set: {} entries in {} containers, {:.1} MiB ({} written, {} reused{})",
        pack.entries,
        pack.containers.len(),
        mib(pack.total_stored),
        pack.written,
        pack.reused,
        if pack.rewritten { ", rewritten whole" } else { "" }
    );
}

// std::fs allowed: the packaged game's output folder is the user's chosen real machine path,
// not a project file (and not under the project's provider)
#[allow(clippy::disallowed_methods)]
fn reset_output_dir(content_dir: &Path) -> std::io::Result<()> {
    if content_dir.exists() {
        std::fs::remove_dir_all(content_dir)?;
    }
    std::fs::create_dir_all(content_dir)
}

// std::fs allowed: writes into the packaged game's output folder, a real machine path
#[allow(clippy::disallowed_methods)]
fn write_loose(content_dir: &Path, rel: &str, bytes: &[u8]) -> Result<(), PackageError> {
    let path = content_dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| PackageError::Write(format!("{}: {e}", parent.display())))?;
    }
    std::fs::write(&path, bytes)
        .map_err(|e| PackageError::Write(format!("{}: {e}", path.display())))
}

/// `cargo build --release` of the project's game binary, copied to `out`.
fn build_game(project: &Path, out: &Path, target: Option<&str>) -> Result<PathBuf, PackageError> {
    let manifest = project.join("Cargo.toml");
    let text = virtual_fs::read_to_string(&manifest).map_err(|e| {
        PackageError::Build(format!(
            "{}: {e} (is this a generated Pulsar project?)",
            manifest.display()
        ))
    })?;
    let doc: toml::Table = text
        .parse()
        .map_err(|e| PackageError::Build(format!("{}: {e}", manifest.display())))?;
    let bin = doc
        .get("bin")
        .and_then(|b| b.as_array())
        .and_then(|bins| {
            let names: Vec<&str> = bins
                .iter()
                .filter_map(|b| b.get("name")?.as_str())
                .collect();
            names
                .iter()
                .find(|n| n.ends_with("_game"))
                .or(names.first())
                .map(|n| (*n).to_owned())
        })
        .or_else(|| doc.get("package")?.get("name")?.as_str().map(str::to_owned))
        .ok_or_else(|| PackageError::Build("the project's Cargo.toml names no binary".into()))?;

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .arg("build")
        .arg("--release")
        .arg("--bin")
        .arg(&bin)
        .arg("--manifest-path")
        .arg(&manifest);
    if let Some(target) = target {
        command.arg("--target").arg(target);
    }
    // GUI launches on macOS may have no valid stdin/stdout/stderr file
    // descriptors. Do not let Cargo inherit those descriptors: posix_spawn
    // can fail with EBADF before Cargo starts. Capture output so the CLI and
    // editor can still report Cargo's diagnostics through the package result.
    command.stdin(Stdio::null());
    tracing::info!(bin = %bin, "Building the game binary (cargo build --release)");
    let output = command
        .output()
        .map_err(|e| PackageError::Build(format!("cannot run cargo: {e}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.trim().is_empty() {
        tracing::info!(cargo_stdout = %stdout, "cargo build output");
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        tracing::warn!(cargo_stderr = %stderr, "cargo build diagnostics");
    }
    if !output.status.success() {
        return Err(PackageError::Build(format!(
            "cargo build exited with {}{}",
            output.status,
            if stderr.trim().is_empty() {
                String::new()
            } else {
                format!(":\n{stderr}")
            }
        )));
    }

    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| project.join("target"));
    let mut built = target_dir;
    if let Some(target) = target {
        built.push(target);
    }
    built.push("release");
    let exe_name = if target.map_or(cfg!(windows), |t| t.contains("windows")) {
        format!("{bin}.exe")
    } else {
        bin
    };
    let built = built.join(&exe_name);
    let dest = out.join(&exe_name);
    copy_executable(&built, &dest)
        .map_err(|e| PackageError::Build(format!("copy {}: {e}", built.display())))?;
    Ok(dest)
}

// std::fs allowed: copies the built game executable from cargo's target dir into the output
// folder, both real machine paths, not project files
#[allow(clippy::disallowed_methods)]
fn copy_executable(built: &Path, dest: &Path) -> std::io::Result<u64> {
    std::fs::copy(built, dest)
}
