//! Builds each installed plugin as a library of its own and installs it where
//! the editor loads it.
//!
//! A plugin's root crate (`pulsar-plugin.toml`, `root`) is the library: built
//! with its `dll` feature it is a `cdylib` that exports the plugin's descriptor
//! and registrations and attaches its copy of Helio, the parking lot and the
//! world crates to the host's (`plugin_editor_api::export_library_plugin!`,
//! `export_plugin_build!`). Each plugin's own repository builds and releases its
//! library the same way (`cargo build -p <root> --release --features dll`);
//! this tool builds all of them together with the editor.
//!
//! ```text
//! cargo run -p plugin_builder -- [--release] [--only <id>,..] [--out <dir>] [--no-host] [--check]
//! ```
//!
//! 1. Reads every `plugins/vendor/*/pulsar-plugin.toml` (every plugin but
//!    `render_bps`, which the editor links: `plugins/linked`).
//! 2. Builds the editor and every plugin root with `--features <root>/dll` in one
//!    cargo invocation, so the editor and its libraries resolve the engine's
//!    crates with the same features: the plugin loader refuses a library built
//!    against another layout (`--no-host` builds only the libraries).
//! 3. Copies each library to `<install>/plugins/editor/<root>.<ext>` and
//!    registers it in the install's integrity manifest and lock
//!    (`pulsar_install_layout`), and unregisters a library this tool installed
//!    earlier whose plugin is gone. `<install>` is the directory of the editor
//!    executable that was built, unless `PULSAR_INSTALL_DIR` is set (the engine
//!    honors it first) or `--out` is given. Each file is then checked against the
//!    manifest the way the engine checks it.
//!
//! `--check` builds and writes nothing: it fails when a plugin root is not set up
//! to build as a library (no `dll` feature, or no `cdylib` crate type).

use std::collections::BTreeSet;
use std::error::Error;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use engine_fs::virtual_fs;
use pulsar_install_layout::{plugin_library_extension, InstallLayout, PluginRecord};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Plugins the editor links into its binary instead of loading.
const LINKED: &[&str] = &["pulsar.render_graph"];

/// The editor's package, built with the libraries.
const EDITOR_PACKAGE: &str = "pulsar_engine";

/// The shared UI and renderer libraries (each in its owner repository): the editor and every plugin library link
/// them dynamically, so they ship beside the editor executable, not in `plugins/editor`.
const SHARED_LIBRARIES: &[&str] = &[
    "wgpui_dylib",
    "ui_dylib",
    "profiling_dylib",
    "reflection_dylib",
    "scenedb_dylib",
    "helio_dylib",
];

/// The `repo_url` of a record this tool installs: `local:<plugin id>`.
const RECORD_PREFIX: &str = "local:";

/// One installed plugin, as its descriptor and root crate say.
struct Plugin {
    id: String,
    name: String,
    /// The root crate's package name.
    root: String,
    /// The root's manifest.
    manifest: PathBuf,
    /// Whether the root builds as a library: a `dll` feature and a `cdylib` crate type.
    library_ready: bool,
}

struct Options {
    release: bool,
    only: Option<BTreeSet<String>>,
    out: Option<PathBuf>,
    with_host: bool,
    check: bool,
}

fn parse_options() -> Result<Options> {
    let mut options = Options {
        release: false,
        only: None,
        out: None,
        with_host: true,
        check: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--release" => options.release = true,
            "--no-host" => options.with_host = false,
            "--check" => options.check = true,
            "--only" => {
                let list = args.next().ok_or("--only needs a plugin id list")?;
                options.only = Some(list.split(',').map(str::to_owned).collect());
            }
            "--out" => options.out = Some(args.next().ok_or("--out needs a directory")?.into()),
            other => return Err(format!("unknown argument `{other}`").into()),
        }
    }
    Ok(options)
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// Every plugin under `plugins/vendor`, by its descriptor, in id order.
fn discover(root: &Path) -> Result<Vec<Plugin>> {
    let vendor = root.join("plugins").join("vendor");
    let mut plugins = Vec::new();
    for entry in virtual_fs::list_dir(&vendor)? {
        let dir = vendor.join(&entry.name);
        let descriptor = dir.join("pulsar-plugin.toml");
        if !entry.is_dir || !virtual_fs::is_file(&descriptor) {
            continue;
        }
        let text = virtual_fs::read_to_string(&descriptor)?;
        let table: toml::Table = text
            .parse()
            .map_err(|e| format!("{}: {e}", descriptor.display()))?;
        let field = |key: &str| -> Result<String> {
            Ok(table
                .get(key)
                .and_then(toml::Value::as_str)
                .ok_or_else(|| format!("{}: no `{key}`", descriptor.display()))?
                .to_owned())
        };
        let (id, name, root_crate) = (field("id")?, field("name")?, field("root")?);

        let manifest = dir.join("Cargo.toml");
        let manifest_text = virtual_fs::read_to_string(&manifest)?;
        let manifest_table: toml::Table = manifest_text
            .parse()
            .map_err(|e| format!("{}: {e}", manifest.display()))?;
        let package = manifest_table
            .get("package")
            .and_then(|package| package.get("name"))
            .and_then(toml::Value::as_str);
        if package != Some(root_crate.as_str()) {
            return Err(format!(
                "{}: root `{root_crate}` is not the package in {} ({package:?})",
                descriptor.display(),
                dir.display()
            )
            .into());
        }
        let has_dll_feature = manifest_table
            .get("features")
            .and_then(toml::Value::as_table)
            .is_some_and(|features| features.contains_key("dll"));
        let has_cdylib = manifest_table
            .get("lib")
            .and_then(|lib| lib.get("crate-type"))
            .and_then(toml::Value::as_array)
            .is_some_and(|types| types.iter().any(|t| t.as_str() == Some("cdylib")));
        plugins.push(Plugin {
            id,
            name,
            root: root_crate,
            manifest,
            library_ready: has_dll_feature && has_cdylib,
        });
    }
    plugins.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(plugins)
}

/// A library cargo built, and where.
struct Built {
    /// The root crate's package name.
    root: String,
    file: PathBuf,
}

struct BuildOutput {
    libraries: Vec<Built>,
    /// The shared libraries cargo built (`SHARED_LIBRARIES`).
    shared: Vec<PathBuf>,
    /// The directory of the editor executable, when the editor was built.
    editor_dir: Option<PathBuf>,
}

/// The package name in a cargo package id: `path+file:///.../fog#fog_plugin@0.1.0`, or
/// `path+file:///.../input_plugin#0.1.0` when the directory is named like the package
/// (the name is then the last path segment).
fn package_of(id: &str) -> &str {
    let (location, tail) = id.rsplit_once('#').unwrap_or((id, ""));
    match tail.split_once('@') {
        Some((name, _version)) => name,
        None => location.rsplit('/').next().unwrap_or_default(),
    }
}

/// Builds the libraries (and the editor), reading cargo's artifact messages.
fn build(plugins: &[&Plugin], options: &Options) -> Result<BuildOutput> {
    let mut command = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    command.current_dir(repo_root());
    command.args(["build", "--message-format=json-render-diagnostics"]);
    if options.release {
        command.arg("--release");
    }
    if options.with_host {
        command.args(["-p", EDITOR_PACKAGE]);
    }
    for shared in SHARED_LIBRARIES {
        command.args(["-p", shared]);
    }
    for plugin in plugins {
        command.args(["-p", &plugin.root]);
    }
    if !plugins.is_empty() {
        let features: Vec<String> = plugins
            .iter()
            .map(|plugin| format!("{}/dll", plugin.root))
            .collect();
        command.args(["--features", &features.join(",")]);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::inherit());
    let mut child = command.spawn()?;

    let wanted: BTreeSet<&str> = plugins.iter().map(|plugin| plugin.root.as_str()).collect();
    let extension = plugin_library_extension();
    let mut output = BuildOutput {
        libraries: Vec::new(),
        shared: Vec::new(),
        editor_dir: None,
    };
    let stdout = child.stdout.take().ok_or("no cargo output")?;
    for line in BufReader::new(stdout).lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(&line?) else {
            continue;
        };
        if message["reason"] != "compiler-artifact" {
            continue;
        }
        let name = message["target"]["name"].as_str().unwrap_or_default();
        let package = package_of(message["package_id"].as_str().unwrap_or_default());
        let has_kind = |wanted_kind: &str| {
            message["target"]["kind"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind == wanted_kind))
        };
        if has_kind("dylib") && SHARED_LIBRARIES.contains(&package) {
            let file = message["filenames"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|file| file.as_str())
                .find(|file| Path::new(file).extension().is_some_and(|e| e == extension));
            if let Some(file) = file {
                output.shared.push(file.into());
            }
        } else if has_kind("cdylib") && wanted.contains(package) {
            let file = message["filenames"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|file| file.as_str())
                .find(|file| Path::new(file).extension().is_some_and(|e| e == extension));
            if let Some(file) = file {
                output.libraries.push(Built {
                    root: package.to_owned(),
                    file: file.into(),
                });
            }
        } else if let Some(executable) = message["executable"].as_str() {
            if name == EDITOR_PACKAGE {
                output.editor_dir = Path::new(executable).parent().map(Path::to_path_buf);
            }
        }
    }
    if !child.wait()?.success() {
        return Err("cargo build failed".into());
    }
    Ok(output)
}

/// The toolchain's shared `std` (`std-<hash>.dll`): `-C prefer-dynamic` links every binary and
/// library to it.
fn std_library() -> Result<PathBuf> {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let output = Command::new(rustc)
        .args(["--print", "target-libdir"])
        .output()?;
    let libdir = PathBuf::from(String::from_utf8(output.stdout)?.trim());
    let suffix = format!(".{}", plugin_library_extension());
    for entry in virtual_fs::list_dir(&libdir)? {
        if entry.name.starts_with("std-") && entry.name.ends_with(&suffix) {
            return Ok(libdir.join(entry.name));
        }
    }
    Err(format!("no shared std in {}", libdir.display()).into())
}

/// Copies the shared UI and renderer libraries and `std` beside the editor executable, where the
/// loader finds what the editor and every plugin library import.
fn install_shared(destination_dir: &Path, shared: &[PathBuf]) -> Result<()> {
    if shared.len() != SHARED_LIBRARIES.len() {
        return Err(format!(
            "cargo built {} of the {} shared libraries",
            shared.len(),
            SHARED_LIBRARIES.len()
        )
        .into());
    }
    let std_dll = std_library()?;
    for file in shared.iter().chain(std::iter::once(&std_dll)) {
        let name = file.file_name().ok_or("a library without a name")?;
        let destination = destination_dir.join(name);
        // Cargo places a dylib beside the executable itself (a hard link of the build output):
        // copying a file onto itself fails, and there is nothing to do.
        let in_place = virtual_fs::is_file(&destination)
            && pulsar_install_layout::hash_file(file)? == pulsar_install_layout::hash_file(&destination)?;
        if !in_place {
            virtual_fs::copy(file, &destination)?;
        }
        eprintln!("[plugin_builder] installed {}", name.to_string_lossy());
    }
    Ok(())
}

/// Copies each built library into the install's plugin directory and
/// registers it; unregisters libraries this tool installed for plugins that
/// are gone.
fn install(layout: &InstallLayout, plugins: &[&Plugin], built: &[Built]) -> Result<()> {
    virtual_fs::create_dir_all(&layout.plugins_dir())?;
    let installed_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
        .to_string();
    let extension = plugin_library_extension();

    for plugin in plugins {
        let Some(library) = built.iter().find(|library| library.root == plugin.root) else {
            return Err(format!("cargo built no library for {}", plugin.id).into());
        };
        let file = format!("{}.{extension}", plugin.root);
        virtual_fs::copy(&library.file, &layout.plugins_dir().join(&file))?;
        layout.register_plugin(PluginRecord {
            name: plugin.name.clone(),
            repo_url: format!("{RECORD_PREFIX}{}", plugin.id),
            version: "0.1.0".into(),
            file: file.clone(),
            sha256: String::new(),
            registry_url: String::new(),
            installed_at: installed_at.clone(),
            method: "source".into(),
        })?;
        eprintln!("[plugin_builder] installed {} -> {file}", plugin.id);
    }

    // A library this tool installed whose plugin is no longer built (removed,
    // or now linked): take it out of the install.
    let current: BTreeSet<String> = plugins
        .iter()
        .map(|plugin| format!("{RECORD_PREFIX}{}", plugin.id))
        .collect();
    for record in layout.load_plugin_lock().plugins {
        if record.repo_url.starts_with(RECORD_PREFIX) && !current.contains(&record.repo_url) {
            layout.unregister_plugin(&record.repo_url)?;
            eprintln!("[plugin_builder] removed {}", record.file);
        }
    }
    Ok(())
}

/// Checks what the engine checks before it loads a library: every plugin's file is in
/// the install's plugin directory and its hash is the one in the integrity manifest.
fn verify(layout: &InstallLayout, plugins: &[&Plugin]) -> Result<()> {
    let manifest = layout.load_integrity_manifest();
    for plugin in plugins {
        let file = format!("{}.{}", plugin.root, plugin_library_extension());
        let path = layout.plugins_dir().join(&file);
        let hash = pulsar_install_layout::hash_file(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if manifest.0.get(&file) != Some(&hash) {
            return Err(format!("{file} is not in the integrity manifest with its hash").into());
        }
    }
    Ok(())
}

fn run() -> Result<()> {
    let options = parse_options()?;
    let root = repo_root();
    let all = discover(&root)?;
    let selected: Vec<&Plugin> = all
        .iter()
        .filter(|plugin| !LINKED.contains(&plugin.id.as_str()))
        .collect();

    let not_ready: Vec<String> = selected
        .iter()
        .filter(|plugin| !plugin.library_ready)
        .map(|plugin| {
            format!(
                "{} ({}): needs a `dll` feature and a `cdylib` crate type",
                plugin.id,
                plugin.manifest.display()
            )
        })
        .collect();
    if !not_ready.is_empty() {
        return Err(format!(
            "plugins that cannot build as a library:\n  {}",
            not_ready.join("\n  ")
        )
        .into());
    }
    if options.check {
        return Ok(());
    }

    let to_build: Vec<&Plugin> = match &options.only {
        None => selected,
        Some(only) => {
            if let Some(unknown) = only
                .iter()
                .find(|id| !selected.iter().any(|plugin| &plugin.id == *id))
            {
                return Err(format!("no buildable plugin `{unknown}`").into());
            }
            selected
                .into_iter()
                .filter(|plugin| only.contains(&plugin.id))
                .collect()
        }
    };

    let output = build(&to_build, &options)?;
    let editor_dir = output.editor_dir.clone();
    // Where the engine will look: `--out`, else `PULSAR_INSTALL_DIR` (which the engine
    // honors before anything else, `InstallLayout::of_running_engine`), else the folder
    // of the editor executable that was just built.
    let install_root = options
        .out
        .clone()
        .or_else(|| {
            std::env::var_os("PULSAR_INSTALL_DIR")
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from)
        })
        .or(output.editor_dir)
        .ok_or("no install directory: build the editor, or pass --out <dir>")?;
    let layout = InstallLayout::new(install_root);
    install(&layout, &to_build, &output.libraries)?;
    // The shared libraries go beside the editor executable (the loader looks in the application
    // directory), which is not the install folder when `--out` or `PULSAR_INSTALL_DIR` points
    // elsewhere (a checkout whose libraries live in the repository root).
    install_shared(editor_dir.as_deref().unwrap_or(layout.root()), &output.shared)?;
    verify(&layout, &to_build)?;
    eprintln!(
        "[plugin_builder] {} plugin librar{} ready in {}",
        to_build.len(),
        if to_build.len() == 1 { "y" } else { "ies" },
        layout.plugins_dir().display()
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("plugin_builder: {error}");
        std::process::exit(1);
    }
}
