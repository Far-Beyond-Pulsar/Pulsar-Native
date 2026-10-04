//! Build configurations: named, saved recipes for building a project.
//!
//! A [`BuildConfiguration`] says *how* to build: which Rust profile, for which
//! platforms, and which steps to go through (update, clean, check, build, run).
//! A project keeps any number of them in [`BuildConfigurations`], one of which
//! is selected: the one the toolbar's Build button runs.
//!
//! The collection is a resource in the engine's [`StateStore`](crate::StateStore)
//! ([`build_configurations`]), so any view can watch it with
//! [`ResourceHandle::changed`]. It is saved per project in
//! `<project>/.pulsar/build_configurations.toml` ([`persist`]).
//!
//! This module only describes builds. Turning one into cargo invocations and
//! running them is `ui_build`'s job.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize};

use crate::{EngineContext, ResourceHandle};

// ── Platforms ────────────────────────────────────────────────────────────────

/// Groups of related platforms, for presenting a long list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PlatformFamily {
    Windows,
    Linux,
    MacOs,
    Ios,
    Android,
    Bsd,
    Unix,
    Other,
    Console,
}

impl PlatformFamily {
    pub const ALL: [Self; 9] = [
        Self::Windows,
        Self::Linux,
        Self::MacOs,
        Self::Ios,
        Self::Android,
        Self::Bsd,
        Self::Unix,
        Self::Other,
        Self::Console,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Windows => "Windows",
            Self::Linux => "Linux",
            Self::MacOs => "macOS",
            Self::Ios => "iOS",
            Self::Android => "Android",
            Self::Bsd => "BSD",
            Self::Unix => "Solaris & illumos",
            Self::Other => "Other",
            Self::Console => "Consoles",
        }
    }
}

macro_rules! platforms {
    ($($variant:ident => ($triple:expr, $family:ident, $label:literal)),* $(,)?) => {
        /// A platform a build can target. The table below is the single source
        /// of truth for each platform's id, Rust target triple, family and label.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum TargetPlatform {
            $($variant),*
        }

        impl TargetPlatform {
            /// Every platform, in presentation order.
            pub const ALL: &'static [TargetPlatform] = &[$(TargetPlatform::$variant),*];

            /// Stable identifier used when saving.
            pub fn id(self) -> &'static str {
                match self { $(Self::$variant => stringify!($variant)),* }
            }

            /// The Rust target triple, or `None` for platforms cargo cannot
            /// build for directly (consoles need their own SDK).
            pub fn triple(self) -> Option<&'static str> {
                match self { $(Self::$variant => $triple),* }
            }

            pub fn family(self) -> PlatformFamily {
                match self { $(Self::$variant => PlatformFamily::$family),* }
            }

            pub fn label(self) -> &'static str {
                match self { $(Self::$variant => $label),* }
            }
        }
    };
}

platforms! {
    WindowsX86_64Msvc => (Some("x86_64-pc-windows-msvc"), Windows, "Windows x64"),
    WindowsI686Msvc => (Some("i686-pc-windows-msvc"), Windows, "Windows x86"),
    WindowsAarch64Msvc => (Some("aarch64-pc-windows-msvc"), Windows, "Windows ARM64"),
    WindowsX86_64Gnu => (Some("x86_64-pc-windows-gnu"), Windows, "Windows x64 (GNU)"),
    WindowsI686Gnu => (Some("i686-pc-windows-gnu"), Windows, "Windows x86 (GNU)"),
    LinuxX86_64Gnu => (Some("x86_64-unknown-linux-gnu"), Linux, "Linux x64"),
    LinuxI686Gnu => (Some("i686-unknown-linux-gnu"), Linux, "Linux x86"),
    LinuxAarch64Gnu => (Some("aarch64-unknown-linux-gnu"), Linux, "Linux ARM64"),
    LinuxArmv7Gnueabihf => (Some("armv7-unknown-linux-gnueabihf"), Linux, "Linux ARMv7 (hard float)"),
    LinuxArmGnueabi => (Some("arm-unknown-linux-gnueabi"), Linux, "Linux ARM (soft float)"),
    LinuxArmGnueabihf => (Some("arm-unknown-linux-gnueabihf"), Linux, "Linux ARM (hard float)"),
    LinuxMips64Gnuabi64 => (Some("mips64-unknown-linux-gnuabi64"), Linux, "Linux MIPS64"),
    LinuxMips64elGnuabi64 => (Some("mips64el-unknown-linux-gnuabi64"), Linux, "Linux MIPS64 (LE)"),
    LinuxMipsGnu => (Some("mips-unknown-linux-gnu"), Linux, "Linux MIPS"),
    LinuxMipselGnu => (Some("mipsel-unknown-linux-gnu"), Linux, "Linux MIPS (LE)"),
    LinuxPowerpc64Gnu => (Some("powerpc64-unknown-linux-gnu"), Linux, "Linux PowerPC64"),
    LinuxPowerpc64leGnu => (Some("powerpc64le-unknown-linux-gnu"), Linux, "Linux PowerPC64 (LE)"),
    LinuxPowerpcGnu => (Some("powerpc-unknown-linux-gnu"), Linux, "Linux PowerPC"),
    LinuxRiscv64Gc => (Some("riscv64gc-unknown-linux-gnu"), Linux, "Linux RISC-V 64"),
    LinuxS390xGnu => (Some("s390x-unknown-linux-gnu"), Linux, "Linux s390x"),
    LinuxSparcv9 => (Some("sparc64-unknown-linux-gnu"), Linux, "Linux SPARC64"),
    LinuxX86_64Musl => (Some("x86_64-unknown-linux-musl"), Linux, "Linux x64 (musl)"),
    LinuxAarch64Musl => (Some("aarch64-unknown-linux-musl"), Linux, "Linux ARM64 (musl)"),
    LinuxArmv7Musleabihf => (Some("armv7-unknown-linux-musleabihf"), Linux, "Linux ARMv7 (musl)"),
    LinuxMipselMusl => (Some("mipsel-unknown-linux-musl"), Linux, "Linux MIPS LE (musl)"),
    LinuxMipsMusl => (Some("mips-unknown-linux-musl"), Linux, "Linux MIPS (musl)"),
    MacOsX86_64 => (Some("x86_64-apple-darwin"), MacOs, "macOS Intel"),
    MacOsAarch64 => (Some("aarch64-apple-darwin"), MacOs, "macOS Apple Silicon"),
    IosAarch64 => (Some("aarch64-apple-ios"), Ios, "iOS device"),
    IosX86_64 => (Some("x86_64-apple-ios"), Ios, "iOS simulator (Intel)"),
    IosAarch64Sim => (Some("aarch64-apple-ios-sim"), Ios, "iOS simulator (Apple Silicon)"),
    AndroidAarch64 => (Some("aarch64-linux-android"), Android, "Android ARM64"),
    AndroidArmv7 => (Some("armv7-linux-androideabi"), Android, "Android ARMv7"),
    AndroidI686 => (Some("i686-linux-android"), Android, "Android x86"),
    AndroidX86_64 => (Some("x86_64-linux-android"), Android, "Android x64"),
    FreeBsdX86_64 => (Some("x86_64-unknown-freebsd"), Bsd, "FreeBSD x64"),
    FreeBsdI686 => (Some("i686-unknown-freebsd"), Bsd, "FreeBSD x86"),
    NetBsdX86_64 => (Some("x86_64-unknown-netbsd"), Bsd, "NetBSD x64"),
    OpenBsdX86_64 => (Some("x86_64-unknown-openbsd"), Bsd, "OpenBSD x64"),
    DragonFlyX86_64 => (Some("x86_64-unknown-dragonfly"), Bsd, "DragonFly BSD x64"),
    SolarisSparcv9 => (Some("sparcv9-sun-solaris"), Unix, "Solaris SPARC"),
    SolarisX86_64 => (Some("x86_64-pc-solaris"), Unix, "Solaris x64"),
    IlumosX86_64 => (Some("x86_64-unknown-illumos"), Unix, "illumos x64"),
    RedoxX86_64 => (Some("x86_64-unknown-redox"), Other, "Redox x64"),
    FuchsiaAarch64 => (Some("aarch64-unknown-fuchsia"), Other, "Fuchsia ARM64"),
    FuchsiaX86_64 => (Some("x86_64-unknown-fuchsia"), Other, "Fuchsia x64"),
    PlayStationPs4 => (None, Console, "PlayStation 4"),
    PlayStationPs5 => (None, Console, "PlayStation 5"),
    XboxOne => (None, Console, "Xbox One"),
    XboxSeriesXS => (None, Console, "Xbox Series X|S"),
    NintendoSwitch => (None, Console, "Nintendo Switch"),
}

impl TargetPlatform {
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|p| p.id() == id)
    }

    /// Whether cargo can build for this platform.
    pub fn is_buildable(self) -> bool {
        self.triple().is_some()
    }

    /// The platform this editor is running on, when it is one we know.
    pub fn host() -> Option<Self> {
        use TargetPlatform::*;
        let windows = cfg!(target_os = "windows");
        let linux = cfg!(target_os = "linux");
        let macos = cfg!(target_os = "macos");
        let x64 = cfg!(target_arch = "x86_64");
        let x86 = cfg!(target_arch = "x86");
        let arm64 = cfg!(target_arch = "aarch64");
        let msvc = cfg!(target_env = "msvc");
        let musl = cfg!(target_env = "musl");

        Some(match () {
            _ if windows && x64 && msvc => WindowsX86_64Msvc,
            _ if windows && x64 => WindowsX86_64Gnu,
            _ if windows && x86 && msvc => WindowsI686Msvc,
            _ if windows && x86 => WindowsI686Gnu,
            _ if windows && arm64 => WindowsAarch64Msvc,
            _ if linux && x64 && musl => LinuxX86_64Musl,
            _ if linux && x64 => LinuxX86_64Gnu,
            _ if linux && arm64 && musl => LinuxAarch64Musl,
            _ if linux && arm64 => LinuxAarch64Gnu,
            _ if linux && x86 => LinuxI686Gnu,
            _ if macos && x64 => MacOsX86_64,
            _ if macos && arm64 => MacOsAarch64,
            _ => return None,
        })
    }
}

impl Serialize for TargetPlatform {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.id())
    }
}

/// Read a list of platform ids, dropping any this version does not know (a
/// config saved by a newer build must still load).
fn lenient_platforms<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<TargetPlatform>, D::Error> {
    let ids = Vec::<String>::deserialize(d)?;
    Ok(ids
        .iter()
        .filter_map(|id| {
            let platform = TargetPlatform::from_id(id);
            if platform.is_none() {
                tracing::warn!("build configuration: ignoring unknown platform {id:?}");
            }
            platform
        })
        .collect())
}

// ── Profile and steps ────────────────────────────────────────────────────────

/// Which Rust build mode to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildProfile {
    /// Fast compiles, unoptimised, debug assertions on.
    Debug,
    /// Optimised.
    #[default]
    Release,
    /// Release tuned for distribution: fat LTO, one codegen unit, stripped.
    Shipping,
}

impl BuildProfile {
    pub const ALL: [Self; 3] = [Self::Debug, Self::Release, Self::Shipping];

    pub fn label(self) -> &'static str {
        match self {
            Self::Debug => "Debug",
            Self::Release => "Release",
            Self::Shipping => "Shipping",
        }
    }

    pub fn summary(self) -> &'static str {
        match self {
            Self::Debug => "Fast to compile, slow to run. Debug assertions on.",
            Self::Release => "Optimised build for testing real performance.",
            Self::Shipping => "Release with fat LTO, one codegen unit and stripped symbols.",
        }
    }

    /// Arguments selecting this profile.
    pub fn cargo_args(self) -> &'static [&'static str] {
        match self {
            Self::Debug => &[],
            Self::Release | Self::Shipping => &["--release"],
        }
    }

    /// Environment overrides that turn the release profile into this one.
    pub fn cargo_env(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Shipping => &[
                ("CARGO_PROFILE_RELEASE_LTO", "fat"),
                ("CARGO_PROFILE_RELEASE_CODEGEN_UNITS", "1"),
                ("CARGO_PROFILE_RELEASE_STRIP", "symbols"),
            ],
            Self::Debug | Self::Release => &[],
        }
    }
}

/// Which steps a build goes through. They always run in this order: update,
/// clean, check, build, run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BuildSteps {
    /// `cargo update`: refresh dependencies first.
    pub update: bool,
    /// `cargo clean`: build from scratch.
    pub clean: bool,
    /// `cargo check`: type-check without producing binaries.
    pub check: bool,
    /// `cargo build`.
    pub build: bool,
    /// Launch the built game.
    pub run: bool,
}

impl BuildSteps {
    pub const fn just_build() -> Self {
        Self { update: false, clean: false, check: false, build: true, run: false }
    }

    /// Whether anything would happen.
    pub fn any(self) -> bool {
        self.update || self.clean || self.check || self.build || self.run
    }

    /// Make the combination consistent: running needs a build.
    pub fn normalized(mut self) -> Self {
        if self.run {
            self.build = true;
        }
        self
    }

    /// `Update › Clean › Build › Run`, or `Nothing` when empty.
    pub fn summary(self) -> String {
        let mut parts = Vec::new();
        if self.update {
            parts.push("Update");
        }
        if self.clean {
            parts.push("Clean");
        }
        if self.check {
            parts.push("Check");
        }
        if self.build {
            parts.push("Build");
        }
        if self.run {
            parts.push("Run");
        }
        if parts.is_empty() {
            "No steps".into()
        } else {
            parts.join(" › ")
        }
    }
}

// ── A configuration ──────────────────────────────────────────────────────────

pub type ConfigId = String;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BuildConfiguration {
    pub id: ConfigId,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub profile: BuildProfile,
    /// Platforms to build for. Empty means the one this machine runs on.
    #[serde(default, deserialize_with = "lenient_platforms")]
    pub platforms: Vec<TargetPlatform>,
    #[serde(default)]
    pub steps: BuildSteps,
    /// Cargo features to enable, comma or space separated.
    #[serde(default)]
    pub features: String,
    /// Extra arguments passed to every cargo command.
    #[serde(default)]
    pub extra_args: String,
}

impl BuildConfiguration {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            description: String::new(),
            profile: BuildProfile::Release,
            platforms: Vec::new(),
            steps: BuildSteps::just_build(),
            features: String::new(),
            extra_args: String::new(),
        }
    }

    fn with(
        name: &str,
        description: &str,
        profile: BuildProfile,
        steps: BuildSteps,
    ) -> Self {
        let mut config = Self::new(name);
        config.description = description.into();
        config.profile = profile;
        config.steps = steps;
        config
    }

    /// `Windows x64`, `Host`, or `Windows x64 +2`.
    pub fn platforms_summary(&self) -> String {
        match self.platforms.as_slice() {
            [] => "This machine".into(),
            [one] => one.label().into(),
            [first, rest @ ..] => format!("{} +{}", first.label(), rest.len()),
        }
    }

    /// One line describing it: `Release · Windows x64 · Build › Run`.
    pub fn subtitle(&self) -> String {
        format!(
            "{} · {} · {}",
            self.profile.label(),
            self.platforms_summary(),
            self.steps.summary()
        )
    }

    /// Whether this configuration matches a search `query`. Every
    /// whitespace-separated word must appear (any case) in the name,
    /// description, profile, platform names or triples, or steps.
    pub fn matches(&self, query: &str) -> bool {
        let mut haystack = format!(
            "{} {} {} {}",
            self.name,
            self.description,
            self.profile.label(),
            self.steps.summary()
        );
        for platform in &self.platforms {
            haystack.push(' ');
            haystack.push_str(platform.label());
            if let Some(triple) = platform.triple() {
                haystack.push(' ');
                haystack.push_str(triple);
            }
        }
        if self.platforms.is_empty() {
            haystack.push_str(" host");
        }
        let haystack = haystack.to_lowercase();
        query
            .split_whitespace()
            .all(|word| haystack.contains(&word.to_lowercase()))
    }

    /// The configurations a new project starts with.
    pub fn defaults() -> Vec<Self> {
        use BuildProfile::*;
        let steps = |update, clean, check, build, run| BuildSteps { update, clean, check, build, run };
        vec![
            Self::with("Build", "Optimised build.", Release, steps(false, false, false, true, false)),
            Self::with("Build & Run", "Build, then launch the game.", Release, steps(false, false, false, true, true)),
            Self::with("Check", "Type-check only; no binaries.", Debug, steps(false, false, true, false, false)),
            Self::with("Debug Build", "Fast, unoptimised build.", Debug, steps(false, false, false, true, false)),
            Self::with("Debug Run", "Fast build, then launch.", Debug, steps(false, false, false, true, true)),
            Self::with("Clean Build", "Start from scratch.", Release, steps(false, true, false, true, false)),
            Self::with("Update, Build & Run", "Refresh dependencies first, then build and launch.", Release, steps(true, false, false, true, true)),
            Self::with("Shipping", "Clean, fully optimised build for distribution.", Shipping, steps(false, true, false, true, false)),
        ]
    }
}

// ── The collection ───────────────────────────────────────────────────────────

/// All of a project's build configurations and which one is selected.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BuildConfigurations {
    configs: Vec<BuildConfiguration>,
    selected: Option<ConfigId>,
    /// The project this was loaded for, so a different project reloads.
    project: Option<PathBuf>,
}

impl BuildConfigurations {
    pub fn configs(&self) -> &[BuildConfiguration] {
        &self.configs
    }

    pub fn get(&self, id: &str) -> Option<&BuildConfiguration> {
        self.configs.iter().find(|c| c.id == id)
    }

    pub fn selected_id(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// The configuration the Build button runs.
    pub fn selected(&self) -> Option<&BuildConfiguration> {
        self.selected.as_deref().and_then(|id| self.get(id))
    }

    pub fn select(&mut self, id: &str) {
        if self.get(id).is_some() {
            self.selected = Some(id.to_owned());
        }
    }

    /// Configurations matching `query`, in list order; all when it is blank.
    pub fn search(&self, query: &str) -> Vec<&BuildConfiguration> {
        self.configs.iter().filter(|c| c.matches(query)).collect()
    }

    /// Whether `name` is used by a configuration other than `except`.
    pub fn name_taken(&self, name: &str, except: Option<&str>) -> bool {
        let name = name.trim().to_lowercase();
        self.configs
            .iter()
            .any(|c| Some(c.id.as_str()) != except && c.name.trim().to_lowercase() == name)
    }

    /// `base`, or `base 2`, `base 3`… so it is not already a name.
    pub fn unique_name(&self, base: &str) -> String {
        let base = base.trim();
        if !self.name_taken(base, None) {
            return base.to_owned();
        }
        (2..)
            .map(|n| format!("{base} {n}"))
            .find(|candidate| !self.name_taken(candidate, None))
            .expect("an unused name exists")
    }

    /// Add a new blank configuration, select it, return its id.
    pub fn add_new(&mut self) -> ConfigId {
        let config = BuildConfiguration::new(self.unique_name("New Configuration"));
        self.insert(config)
    }

    /// Add `config` (renaming it if the name is taken), select it, return its id.
    pub fn insert(&mut self, mut config: BuildConfiguration) -> ConfigId {
        config.name = self.unique_name(&config.name);
        let id = config.id.clone();
        self.configs.push(config);
        self.selected = Some(id.clone());
        id
    }

    /// Copy `id` as `<name> Copy`, selecting the copy.
    pub fn duplicate(&mut self, id: &str) -> Option<ConfigId> {
        let mut copy = self.get(id)?.clone();
        copy.id = uuid::Uuid::new_v4().to_string();
        copy.name = format!("{} Copy", copy.name);
        Some(self.insert(copy))
    }

    /// Remove `id`. If it was selected, the neighbour takes over.
    pub fn remove(&mut self, id: &str) {
        let Some(ix) = self.configs.iter().position(|c| c.id == id) else {
            return;
        };
        self.configs.remove(ix);
        if self.selected.as_deref() == Some(id) {
            self.selected = self
                .configs
                .get(ix)
                .or_else(|| self.configs.last())
                .map(|c| c.id.clone());
        }
    }

    /// Edit `id` in place. Steps are kept consistent afterwards.
    pub fn edit(&mut self, id: &str, f: impl FnOnce(&mut BuildConfiguration)) {
        if let Some(config) = self.configs.iter_mut().find(|c| c.id == id) {
            f(config);
            config.steps = config.steps.normalized();
        }
    }
}

// ── Persistence ──────────────────────────────────────────────────────────────

const FILE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct FileFormat {
    version: u32,
    #[serde(default)]
    selected: Option<ConfigId>,
    #[serde(default)]
    configuration: Vec<BuildConfiguration>,
}

/// Where a project keeps its build configurations.
pub fn configurations_path(project_root: &Path) -> PathBuf {
    project_root.join(".pulsar").join("build_configurations.toml")
}

impl BuildConfigurations {
    /// The project's saved configurations, or the defaults when it has none
    /// (or the file cannot be read, which is set aside rather than lost).
    pub fn load(project_root: &Path) -> Self {
        let path = configurations_path(project_root);
        let mut loaded = match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<FileFormat>(&text) {
                Ok(file) if file.version <= FILE_VERSION => Some(file),
                Ok(file) => {
                    tracing::warn!(
                        "build configurations were saved by a newer version ({}); using defaults",
                        file.version
                    );
                    None
                }
                Err(error) => {
                    tracing::warn!("unreadable build configurations ({error}); using defaults");
                    let _ = std::fs::rename(&path, path.with_extension("toml.corrupt"));
                    None
                }
            },
            Err(_) => None,
        }
        .map(|file| Self {
            selected: file.selected,
            configs: file.configuration,
            project: None,
        })
        .unwrap_or_else(|| {
            let configs = BuildConfiguration::defaults();
            Self {
                selected: configs.first().map(|c| c.id.clone()),
                configs,
                project: None,
            }
        });

        // A selection that no longer exists falls back to the first entry.
        if loaded.selected().is_none() {
            loaded.selected = loaded.configs.first().map(|c| c.id.clone());
        }
        loaded.project = Some(project_root.to_owned());
        loaded
    }

    /// Write to the project's file, atomically.
    pub fn save(&self, project_root: &Path) -> std::io::Result<()> {
        let file = FileFormat {
            version: FILE_VERSION,
            selected: self.selected.clone(),
            configuration: self.configs.clone(),
        };
        let text = toml::to_string_pretty(&file)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let path = configurations_path(project_root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }
}

// ── Engine resource ──────────────────────────────────────────────────────────

/// The build configurations resource.
///
/// # Panics
/// If the engine context is not initialised (it is, before any UI exists).
pub fn build_configurations() -> ResourceHandle<BuildConfigurations> {
    EngineContext::global()
        .expect("engine initialized")
        .store
        .get_or_init::<BuildConfigurations>()
}

/// Load the current project's configurations into the resource, unless that is
/// already done. Call from anything that needs them before reading.
pub fn ensure_loaded() {
    let Some(root) = crate::get_project_path().map(PathBuf::from) else {
        return;
    };
    let handle = build_configurations();
    if handle.read().project.as_deref() == Some(root.as_path()) {
        return;
    }
    handle.set(BuildConfigurations::load(&root));
}

/// Save the resource to the current project. Call after changing it.
pub fn persist() {
    let Some(root) = crate::get_project_path().map(PathBuf::from) else {
        return;
    };
    if let Err(error) = build_configurations().read().save(&root) {
        tracing::warn!("could not save build configurations: {error}");
    }
}

/// Whether a build is running now. Lives with the rest of the playback state;
/// see [`crate::playback::PlaybackState::build_running`].
#[derive(Default)]
pub struct BuildCancel(pub std::sync::Arc<std::sync::atomic::AtomicBool>);

/// The cancellation flag a running build watches.
pub fn build_cancel() -> ResourceHandle<BuildCancel> {
    EngineContext::global()
        .expect("engine initialized")
        .store
        .get_or_init::<BuildCancel>()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_project(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pulsar-bc-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn every_platform_has_a_unique_id_and_round_trips() {
        let mut seen = std::collections::HashSet::new();
        for &platform in TargetPlatform::ALL {
            assert!(seen.insert(platform.id()), "duplicate id {}", platform.id());
            assert_eq!(TargetPlatform::from_id(platform.id()), Some(platform));
        }
        assert!(TargetPlatform::from_id("NotAPlatform").is_none());
    }

    #[test]
    fn triples_are_unique_and_consoles_have_none() {
        let mut seen = std::collections::HashSet::new();
        for &platform in TargetPlatform::ALL {
            match platform.triple() {
                Some(t) => assert!(seen.insert(t), "duplicate triple {t}"),
                None => assert_eq!(platform.family(), PlatformFamily::Console),
            }
        }
    }

    #[test]
    fn defaults_are_valid_and_uniquely_named() {
        let configs = BuildConfiguration::defaults();
        let mut names = std::collections::HashSet::new();
        for c in &configs {
            assert!(c.steps.any(), "{} does nothing", c.name);
            assert_eq!(c.steps, c.steps.normalized(), "{} is inconsistent", c.name);
            assert!(names.insert(c.name.clone()), "duplicate name {}", c.name);
        }
    }

    #[test]
    fn running_implies_building() {
        let steps = BuildSteps { run: true, ..Default::default() }.normalized();
        assert!(steps.build);
    }

    #[test]
    fn a_saved_collection_loads_back_identically() {
        let dir = temp_project("roundtrip");
        let mut store = BuildConfigurations::load(&dir);
        let id = store.add_new();
        store.edit(&id, |c| {
            c.name = "Console Cert".into();
            c.profile = BuildProfile::Shipping;
            c.platforms = vec![TargetPlatform::LinuxX86_64Gnu, TargetPlatform::PlayStationPs5];
            c.steps = BuildSteps { update: true, clean: true, check: false, build: true, run: false };
            c.features = "a, b".into();
            c.extra_args = "--locked".into();
        });
        store.save(&dir).unwrap();

        let loaded = BuildConfigurations::load(&dir);
        assert_eq!(loaded.configs(), store.configs());
        assert_eq!(loaded.selected_id(), Some(id.as_str()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_gives_the_defaults_with_the_first_selected() {
        let dir = temp_project("missing");
        let store = BuildConfigurations::load(&dir);
        assert_eq!(store.configs().len(), BuildConfiguration::defaults().len());
        assert_eq!(store.selected().unwrap().name, "Build");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unknown_platform_is_dropped_not_fatal() {
        let dir = temp_project("unknown");
        let path = configurations_path(&dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"
version = 1
[[configuration]]
id = "a"
name = "From the future"
platforms = ["LinuxX86_64Gnu", "QuantumToaster"]
"#,
        )
        .unwrap();
        let store = BuildConfigurations::load(&dir);
        assert_eq!(store.get("a").unwrap().platforms, vec![TargetPlatform::LinuxX86_64Gnu]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_file_is_set_aside_and_defaults_used() {
        let dir = temp_project("corrupt");
        let path = configurations_path(&dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "this is [not toml").unwrap();
        let store = BuildConfigurations::load(&dir);
        assert!(!store.configs().is_empty());
        assert!(path.with_extension("toml.corrupt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_stay_unique() {
        let mut store = BuildConfigurations::default();
        store.insert(BuildConfiguration::new("Build"));
        let second = store.insert(BuildConfiguration::new("build"));
        assert_eq!(store.get(&second).unwrap().name, "build 2");
        assert!(store.name_taken("  BUILD ", None));
        assert!(!store.name_taken("Build", Some(&store.configs()[0].id.clone())));
    }

    #[test]
    fn removing_the_selected_one_selects_a_neighbour() {
        let mut store = BuildConfigurations::default();
        let a = store.insert(BuildConfiguration::new("A"));
        let b = store.insert(BuildConfiguration::new("B"));
        let c = store.insert(BuildConfiguration::new("C"));
        store.select(&b);
        store.remove(&b);
        assert_eq!(store.selected_id(), Some(c.as_str()), "the one that took its place");
        store.remove(&c);
        assert_eq!(store.selected_id(), Some(a.as_str()), "falls back to the last");
        store.remove(&a);
        assert_eq!(store.selected_id(), None);
    }

    #[test]
    fn duplicating_copies_and_selects() {
        let mut store = BuildConfigurations::default();
        let a = store.insert(BuildConfiguration::new("A"));
        store.edit(&a, |c| c.profile = BuildProfile::Debug);
        let copy = store.duplicate(&a).unwrap();
        assert_ne!(copy, a);
        assert_eq!(store.get(&copy).unwrap().name, "A Copy");
        assert_eq!(store.get(&copy).unwrap().profile, BuildProfile::Debug);
        assert_eq!(store.selected_id(), Some(copy.as_str()));
    }

    #[test]
    fn search_matches_every_word_across_the_fields() {
        let mut c = BuildConfiguration::new("Nightly");
        c.profile = BuildProfile::Shipping;
        c.platforms = vec![TargetPlatform::MacOsAarch64];
        assert!(c.matches(""));
        assert!(c.matches("night"));
        assert!(c.matches("shipping apple"), "profile and platform label");
        assert!(c.matches("aarch64-apple-darwin"), "triple");
        assert!(!c.matches("nightly windows"));
        // No platforms means "this machine", also reachable as "host".
        assert!(BuildConfiguration::new("x").matches("host"));
    }

    #[test]
    fn subtitles_read_naturally() {
        let mut c = BuildConfiguration::new("x");
        c.steps = BuildSteps { build: true, run: true, ..Default::default() };
        assert_eq!(c.subtitle(), "Release · This machine · Build › Run");
        c.platforms = vec![TargetPlatform::WindowsX86_64Msvc, TargetPlatform::LinuxX86_64Gnu];
        assert_eq!(c.platforms_summary(), "Windows x64 +1");
    }
}
