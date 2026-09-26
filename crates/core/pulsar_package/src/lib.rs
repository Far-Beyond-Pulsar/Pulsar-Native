//! Packaging a Pulsar project as a standalone game (Pulsar-Native#926).
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
//! 3. **Write** `<out>/Content/game.pak` (or loose files with `--loose`),
//!    then check that no shipped file names an absolute path or
//!    `CARGO_MANIFEST_DIR`.
//! 4. **Build** the game binary (`cargo build --release` of the project)
//!    and copy it to `<out>/` (skipped with `--skip-build`).
//!
//! The packaged game finds `Content/` next to its executable
//! (`pulsar_content::ContentRoot::discover`).

pub mod compile;
pub mod cook;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use pulsar_class::{ClassIndex, ClassRegistry};
use pulsar_content::{
    AssetRegistry, BuildProfile, PakWriter, ProjectSettings, ASSET_REGISTRY_FILE, CONTENT_DIR_NAME, PAK_FILE_NAME,
    PROJECT_SETTINGS_FILE,
};
use serde::Serialize;

/// What to package, and how.
#[derive(Clone, Debug)]
pub struct PackageOptions {
    /// The project directory.
    pub project: PathBuf,
    /// Output directory: gets the game executable and `Content/`.
    pub out: PathBuf,
    pub profile: BuildProfile,
    /// Rust target triple for the game binary (default: the host).
    pub target: Option<String>,
    /// Write `Content/` as loose files instead of `game.pak`.
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
pub fn default_engine_assets() -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../assets");
    dir.is_dir().then(|| dir.canonicalize().unwrap_or(dir))
}

/// Compile a project's scripts and verify them (`pulsar build-scripts`).
pub fn build_scripts(project: &Path, profile: BuildProfile) -> Result<compile::CompileOutput, PackageError> {
    let settings = project_settings(project, profile)?;
    let output = compile::compile_project(project, &settings);
    if output.has_errors() {
        return Err(PackageError::Scripts(output.problems));
    }
    Ok(output)
}

fn project_settings(project: &Path, profile: BuildProfile) -> Result<ProjectSettings, PackageError> {
    let path = project.join(PROJECT_SETTINGS_FILE);
    let mut settings = match std::fs::read(&path) {
        Ok(bytes) => ProjectSettings::from_json(&bytes).map_err(PackageError::Project)?,
        Err(_) => ProjectSettings::default(),
    };
    settings.profile = profile;
    Ok(settings)
}

/// Package a project. See the crate doc.
pub fn package(options: &PackageOptions) -> Result<PackageReport, PackageError> {
    let project = options
        .project
        .canonicalize()
        .map_err(|e| PackageError::Project(format!("project {}: {e}", options.project.display())))?;
    let mut settings = project_settings(&project, options.profile)?;
    let mut report = PackageReport { profile: options.profile.as_str().into(), ..Default::default() };

    // 1. Scripts.
    let compiled = compile::compile_project(&project, &settings);
    if compiled.has_errors() {
        return Err(PackageError::Scripts(compiled.problems));
    }
    report.warnings.extend(compiled.problems.iter().map(ToString::to_string));
    let registry = ClassRegistry::scan(&project);

    // 2. Cook.
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let engine_assets = options.engine_assets.clone().or_else(default_engine_assets);
    let resolver = cook::AssetResolver::new(&project, engine_assets);
    let mut assets = cook::AssetSet::default();
    let json = |value: &serde_json::Value| serde_json::to_vec(value).unwrap_or_default();

    for class in &compiled.classes {
        let entry = &class.entry;
        // The class directory relative to the project (`src/classes/Door`,
        // or a `Door.class/` folder anywhere in the project).
        let dir = entry
            .dir
            .strip_prefix(&project)
            .ok()
            .map(|rel| rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"))
            .filter(|rel| !rel.is_empty())
            .unwrap_or_else(|| format!("src/classes/{}", entry.name));
        files.insert(
            format!("{dir}/{}", pulsar_class::CLASS_META_FILE),
            json(&serde_json::json!({ "class_id": entry.id.as_str() })),
        );
        let prefab_path = entry.dir.join(pulsar_class::PREFAB_FILE);
        if prefab_path.is_file() {
            // Slot ids are assigned (and saved to the project) by the load.
            let prefab = pulsar_class::PrefabAsset::load_from_dir(&entry.dir).map_err(PackageError::Cook)?;
            let mut value = serde_json::to_value(&prefab).map_err(|e| PackageError::Cook(e.to_string()))?;
            if let Some(class_ref) = value.get_mut("blueprint_class").and_then(|v| v.as_object_mut()) {
                class_ref.insert("class_path".into(), serde_json::Value::String(dir.clone()));
            }
            cook::rewrite_assets(&mut value, &resolver, &mut assets, &format!("{dir}/prefab.json"));
            cook::relativize_project_paths(&mut value, &project);
            files.insert(format!("{dir}/{}", pulsar_class::PREFAB_FILE), json(&value));
        }
        if let Some(module) = &class.module {
            files.insert(
                format!("{dir}/events/.build/{}", pulsar_game::scripting::MODULE_BINARY_FILE),
                module.to_binary(),
            );
        }
        report.classes.push((entry.name.clone(), entry.id.as_str().to_owned(), class.module.is_some()));
    }
    files.insert(
        pulsar_class::CLASS_INDEX_FILE.into(),
        ClassIndex::from_registry(&registry, &project).to_json().into_bytes(),
    );
    if let Some(config) = cook::cook_scripting_config(&project, &registry).map_err(PackageError::Cook)? {
        files.insert(pulsar_game::scripting::SCRIPTING_CONFIG_FILE.into(), json(&config));
    }

    // Levels: every `.level` file, plus the startup level.
    let startup = options
        .startup_level
        .clone()
        .or_else(|| settings.startup_level.clone())
        .or_else(|| default_map(&project))
        .or_else(|| {
            ["scene/default.level", "scenes/default.level", "scenes/default_level.json"]
                .into_iter()
                .find(|rel| project.join(rel).is_file())
                .map(str::to_owned)
        })
        .map(|rel| rel.replace('\\', "/"));
    let mut levels = cook::find_levels(&project, &[options.out.clone()]);
    if let Some(startup) = &startup {
        if !project.join(startup).is_file() {
            return Err(PackageError::Project(format!("startup level {startup} does not exist")));
        }
        levels.insert(startup.clone());
    } else {
        report.warnings.push("no startup level: the game starts with an empty world".into());
    }
    for rel in &levels {
        let path = project.join(rel);
        let bytes = std::fs::read(&path).map_err(|e| PackageError::Cook(format!("{rel}: {e}")))?;
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| PackageError::Cook(format!("{rel}: {e}")))?;
        let (cooked, level_report) = cook::cook_level(value, &registry, &resolver, &mut assets, rel);
        for class in level_report.unresolved_classes {
            report.warnings.push(format!("{rel}: placed class {class} is not in the project"));
        }
        let rel = pulsar_content::normalize_rel(rel).ok_or_else(|| PackageError::Cook(format!("bad level path {rel}")))?;
        files.insert(rel.clone(), json(&cooked));
        report.levels.push(rel);
    }
    for (context, value) in &assets.unresolved {
        report.warnings.push(format!("{context}: asset `{value}` not found; not shipped"));
    }

    // Project settings, cooked.
    settings.profile = options.profile;
    settings.startup_level = startup.as_deref().and_then(pulsar_content::normalize_rel);
    if settings.name.is_empty() {
        settings.name = project_name(&project);
    }
    report.startup_level = settings.startup_level.clone();
    files.insert(PROJECT_SETTINGS_FILE.into(), settings.to_json().into_bytes());

    // Assets.
    let mut registry_out = AssetRegistry::new();
    let mut asset_bytes: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for (id, source) in &assets.assets {
        let bytes = std::fs::read(source).map_err(|e| PackageError::Cook(format!("asset {}: {e}", source.display())))?;
        registry_out.insert(cook::asset_record(id, source, &project, &bytes));
        asset_bytes.insert(id.clone(), bytes);
    }
    report.assets = asset_bytes.len();

    // 3. Write.
    for (rel, bytes) in files.iter().chain(asset_bytes.iter()) {
        if let Some(problem) = machine_path_in(rel, bytes, &project) {
            return Err(PackageError::AbsolutePaths(vec![problem]));
        }
    }
    let content_dir = options.out.join(CONTENT_DIR_NAME);
    if content_dir.exists() {
        std::fs::remove_dir_all(&content_dir).map_err(|e| PackageError::Write(format!("{}: {e}", content_dir.display())))?;
    }
    std::fs::create_dir_all(&content_dir).map_err(|e| PackageError::Write(format!("{}: {e}", content_dir.display())))?;
    if options.loose {
        for (rel, bytes) in files.iter().chain(asset_bytes.iter()) {
            write_loose(&content_dir, rel, bytes)?;
            report.files.insert(rel.clone(), bytes.len() as u64);
        }
        let registry_json = registry_out.to_json().into_bytes();
        write_loose(&content_dir, ASSET_REGISTRY_FILE, &registry_json)?;
        report.files.insert(ASSET_REGISTRY_FILE.into(), registry_json.len() as u64);
    } else {
        let pak_path = content_dir.join(PAK_FILE_NAME);
        let mut pak = PakWriter::create(&pak_path).map_err(|e| PackageError::Write(e.to_string()))?;
        // Assets first, so the registry (written last) knows their place.
        for (rel, bytes) in &asset_bytes {
            let entry = pak.add(rel, bytes).map_err(|e| PackageError::Write(e.to_string()))?;
            if let Some(record) = registry_out.assets.get_mut(rel) {
                record.pak = Some(pulsar_content::registry::PakLocation { offset: entry.offset, len: entry.len });
            }
            report.files.insert(rel.clone(), bytes.len() as u64);
        }
        for (rel, bytes) in &files {
            pak.add(rel, bytes).map_err(|e| PackageError::Write(e.to_string()))?;
            report.files.insert(rel.clone(), bytes.len() as u64);
        }
        let registry_json = registry_out.to_json().into_bytes();
        pak.add(ASSET_REGISTRY_FILE, &registry_json).map_err(|e| PackageError::Write(e.to_string()))?;
        report.files.insert(ASSET_REGISTRY_FILE.into(), registry_json.len() as u64);
        pak.finish().map_err(|e| PackageError::Write(e.to_string()))?;
        report.pak = Some(pak_path);
    }
    report.out = options.out.clone();

    // 4. Build.
    if options.skip_build {
        tracing::warn!("--skip-build: the game executable was not built");
    } else {
        report.executable = Some(build_game(&project, &options.out, options.target.as_deref())?);
    }
    Ok(report)
}

fn write_loose(content_dir: &Path, rel: &str, bytes: &[u8]) -> Result<(), PackageError> {
    let path = content_dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| PackageError::Write(format!("{}: {e}", parent.display())))?;
    }
    std::fs::write(&path, bytes).map_err(|e| PackageError::Write(format!("{}: {e}", path.display())))
}

/// A machine-specific path in a shipped text file: the project's own
/// location, `CARGO_MANIFEST_DIR`, or any JSON string that is an absolute
/// path. Binary files are skipped.
pub fn machine_path_in(rel: &str, bytes: &[u8], project: &Path) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    if text.contains("CARGO_MANIFEST_DIR") {
        return Some(format!("{rel}: mentions CARGO_MANIFEST_DIR"));
    }
    let project_text = project.display().to_string();
    if !project_text.is_empty() && text.contains(&project_text) {
        return Some(format!("{rel}: contains the project path {project_text}"));
    }
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    fn find(value: &serde_json::Value) -> Option<String> {
        match value {
            serde_json::Value::String(s) if cook::is_absolute_path(s) && s.len() > 1 => Some(s.clone()),
            serde_json::Value::Array(items) => items.iter().find_map(find),
            serde_json::Value::Object(map) => map.values().find_map(find),
            _ => None,
        }
    }
    find(&value).map(|path| format!("{rel}: absolute path `{path}`"))
}

/// The editor's `project.default_map` setting (`.pulsar/project/*.toml`).
fn default_map(project: &Path) -> Option<String> {
    pulsar_settings::register_all_settings(engine_state::settings::global_config());
    let settings = engine_state::settings::ProjectSettings::new(project)?;
    settings.load_all();
    let map = settings.get("project", "default_map")?.as_str().ok()?.to_owned();
    (!map.is_empty() && project.join(&map).is_file()).then_some(map)
}

fn project_name(project: &Path) -> String {
    project.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Pulsar Game".into())
}

/// `cargo build --release` of the project's game binary, copied to `out`.
fn build_game(project: &Path, out: &Path, target: Option<&str>) -> Result<PathBuf, PackageError> {
    let manifest = project.join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest)
        .map_err(|e| PackageError::Build(format!("{}: {e} (is this a generated Pulsar project?)", manifest.display())))?;
    let doc: toml::Table = text.parse().map_err(|e| PackageError::Build(format!("{}: {e}", manifest.display())))?;
    let bin = doc
        .get("bin")
        .and_then(|b| b.as_array())
        .and_then(|bins| {
            let names: Vec<&str> = bins.iter().filter_map(|b| b.get("name")?.as_str()).collect();
            names.iter().find(|n| n.ends_with("_game")).or(names.first()).map(|n| (*n).to_owned())
        })
        .or_else(|| doc.get("package")?.get("name")?.as_str().map(str::to_owned))
        .ok_or_else(|| PackageError::Build("the project's Cargo.toml names no binary".into()))?;

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command.arg("build").arg("--release").arg("--bin").arg(&bin).arg("--manifest-path").arg(&manifest);
    if let Some(target) = target {
        command.arg("--target").arg(target);
    }
    tracing::info!(bin = %bin, "Building the game binary (cargo build --release)");
    let status = command.status().map_err(|e| PackageError::Build(format!("cannot run cargo: {e}")))?;
    if !status.success() {
        return Err(PackageError::Build(format!("cargo build exited with {status}")));
    }

    let target_dir = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| project.join("target"));
    let mut built = target_dir;
    if let Some(target) = target {
        built.push(target);
    }
    built.push("release");
    let exe_name = if target.map_or(cfg!(windows), |t| t.contains("windows")) { format!("{bin}.exe") } else { bin };
    let built = built.join(&exe_name);
    let dest = out.join(&exe_name);
    std::fs::copy(&built, &dest).map_err(|e| PackageError::Build(format!("copy {}: {e}", built.display())))?;
    Ok(dest)
}
