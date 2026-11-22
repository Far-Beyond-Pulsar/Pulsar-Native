//! Builds each installed plugin as a library of its own and installs it where
//! the editor loads it.
//!
//! A plugin's root crate (`pulsar-plugin.toml`, `root`) is an ordinary library
//! crate: the one place the plugin registers what it contributes, through its
//! `build`. The library the editor loads is a three-line crate generated here
//! (`plugins/dll/<root>/`) that links the root and exports it
//! (`plugin_editor_api::export_library_plugin!`) and attaches its copy of
//! Helio to the host's (`helio_core::export_runtime_attach!`).
//!
//! ```text
//! cargo run -p plugin_builder -- [--release] [--only <id>,..] [--out <dir>] [--no-host] [--check] [--generate]
//! ```
//!
//! 1. Reads every `plugins/vendor/*/pulsar-plugin.toml` and writes the
//!    generated crate of each plugin but `render_bps`, which the editor links
//!    (`plugins/linked`). A crate whose content did not change is not touched.
//! 2. Builds the editor and every generated crate in one cargo invocation, so
//!    the editor and its libraries resolve the engine's crates with the same
//!    features: the plugin loader refuses a library built against another
//!    layout (`--no-host` builds only the libraries).
//! 3. Copies each library to `<install>/plugins/editor/<root>.<ext>` and
//!    registers it in the install's integrity manifest and lock
//!    (`pulsar_install_layout`), and unregisters a library this tool
//!    installed earlier whose plugin is gone. `<install>` is the directory of
//!    the editor executable that was built (or `--out`).
//!
//! `--generate` only writes the generated crates (the workspace needs them
//! before it can build anything).
//!
//! `--check` writes nothing and builds nothing: it fails when a generated
//! crate is out of date.

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

/// The `repo_url` of a record this tool installs: `local:<plugin id>`.
const RECORD_PREFIX: &str = "local:";

/// One installed plugin, as its descriptor and root crate say.
struct Plugin {
    id: String,
    name: String,
    /// The root crate's package name.
    root: String,
    /// Whether the root has an `editor` feature, which the library enables.
    editor_feature: bool,
}

impl Plugin {
    fn crate_name(&self) -> String {
        format!("{}_dll", self.root)
    }
}

struct Options {
    release: bool,
    only: Option<BTreeSet<String>>,
    out: Option<PathBuf>,
    with_host: bool,
    check: bool,
    generate_only: bool,
}

fn parse_options() -> Result<Options> {
    let mut options = Options {
        release: false,
        only: None,
        out: None,
        with_host: true,
        check: false,
        generate_only: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--release" => options.release = true,
            "--no-host" => options.with_host = false,
            "--check" => options.check = true,
            "--generate" => options.generate_only = true,
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
        let editor_feature = manifest_table
            .get("features")
            .and_then(toml::Value::as_table)
            .is_some_and(|features| features.contains_key("editor"));
        plugins.push(Plugin {
            id,
            name,
            root: root_crate,
            editor_feature,
        });
    }
    plugins.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(plugins)
}

/// The generated crate's `Cargo.toml`.
fn manifest_of(plugin: &Plugin) -> String {
    let features = if plugin.editor_feature {
        ", features = [\"editor\"]"
    } else {
        ""
    };
    format!(
        "# Generated by `cargo run -p plugin_builder`: do not edit.\n\
         [package]\n\
         name = \"{name}\"\n\
         version = \"0.1.0\"\n\
         edition = \"2021\"\n\
         publish = false\n\
         description = {description:?}\n\
         \n\
         [lib]\n\
         crate-type = [\"cdylib\"]\n\
         test = false\n\
         doctest = false\n\
         \n\
         [dependencies]\n\
         {root} = {{ workspace = true{features} }}\n\
         plugin_editor_api = {{ workspace = true, features = [\"editor\"] }}\n\
         helio-core = {{ workspace = true }}\n",
        name = plugin.crate_name(),
        description = format!("{} (`{}`) as a library of its own", plugin.name, plugin.id),
        root = plugin.root,
    )
}

/// The generated crate's `src/lib.rs`.
fn source_of(plugin: &Plugin) -> String {
    format!(
        "//! Generated by `cargo run -p plugin_builder`: do not edit.\n\
         //!\n\
         //! {name} (`{id}`) as a library of its own: the root's descriptor and\n\
         //! registrations, exported for the editor to load.\n\
         \n\
         use {root} as _;\n\
         \n\
         plugin_editor_api::export_library_plugin!({id:?}, {name:?});\n\
         helio_core::export_runtime_attach!();\n",
        name = plugin.name,
        id = plugin.id,
        root = plugin.root,
    )
}

/// Writes `content` to `path` unless it already is that; whether it changed.
fn write_if_changed(path: &Path, content: &str) -> Result<bool> {
    if virtual_fs::read_to_string(path).is_ok_and(|current| current == content) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        virtual_fs::create_dir_all(parent)?;
    }
    virtual_fs::write_file(path, content.as_bytes())?;
    Ok(true)
}

/// Brings every plugin's generated crate up to date (or, with `check`, says
/// which are not). Returns the stale ones.
fn generate(root: &Path, plugins: &[&Plugin], check: bool) -> Result<Vec<String>> {
    let mut stale = Vec::new();
    for plugin in plugins {
        let dir = root.join("plugins").join("dll").join(&plugin.root);
        let files = [
            (dir.join("Cargo.toml"), manifest_of(plugin)),
            (dir.join("src").join("lib.rs"), source_of(plugin)),
        ];
        for (path, content) in files {
            if check {
                if !virtual_fs::read_to_string(&path).is_ok_and(|current| current == content) {
                    stale.push(path.display().to_string());
                }
            } else if write_if_changed(&path, &content)? {
                eprintln!("[plugin_builder] wrote {}", path.display());
            }
        }
    }
    Ok(stale)
}

/// A library cargo built, and where.
struct Built {
    /// The generated crate's package name.
    package: String,
    file: PathBuf,
}

struct BuildOutput {
    libraries: Vec<Built>,
    /// The directory of the editor executable, when the editor was built.
    editor_dir: Option<PathBuf>,
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
    for plugin in plugins {
        command.args(["-p", &plugin.crate_name()]);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::inherit());
    let mut child = command.spawn()?;

    let wanted: BTreeSet<String> = plugins.iter().map(|plugin| plugin.crate_name()).collect();
    let extension = plugin_library_extension();
    let mut output = BuildOutput {
        libraries: Vec::new(),
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
        let is_cdylib = message["target"]["kind"]
            .as_array()
            .is_some_and(|kinds| kinds.iter().any(|kind| kind == "cdylib"));
        if is_cdylib && wanted.contains(name) {
            let file = message["filenames"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|file| file.as_str())
                .find(|file| Path::new(file).extension().is_some_and(|e| e == extension));
            if let Some(file) = file {
                output.libraries.push(Built {
                    package: name.to_owned(),
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
        let Some(library) = built
            .iter()
            .find(|library| library.package == plugin.crate_name())
        else {
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

fn run() -> Result<()> {
    let options = parse_options()?;
    let root = repo_root();
    let all = discover(&root)?;
    let selected: Vec<&Plugin> = all
        .iter()
        .filter(|plugin| !LINKED.contains(&plugin.id.as_str()))
        .collect();

    // Generated crates exist for every plugin, so the workspace is one
    // whatever `--only` builds.
    let stale = generate(&root, &selected, options.check)?;
    if options.generate_only {
        return Ok(());
    }
    if options.check {
        if stale.is_empty() {
            return Ok(());
        }
        return Err(format!(
            "generated plugin crates are out of date (run `cargo run -p plugin_builder`):\n  {}",
            stale.join("\n  ")
        )
        .into());
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
    let install_root = options
        .out
        .clone()
        .or(output.editor_dir)
        .ok_or("no install directory: build the editor, or pass --out <dir>")?;
    install(&InstallLayout::new(install_root), &to_build, &output.libraries)
}

fn main() {
    if let Err(error) = run() {
        eprintln!("plugin_builder: {error}");
        std::process::exit(1);
    }
}
