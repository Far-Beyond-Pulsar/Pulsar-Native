//! The engine SDK: the sources of the engine crates a game links, and the
//! index that describes them ([`SdkIndex`]).
//!
//! A project never points at the SDK. Sync mirrors it into the project's
//! `.pulsar/sdk/` ([`mirror`]), and every Cargo path in the project is
//! relative, into that mirror: the Cargo files are the same on every
//! machine, and the mirror is what each machine rebuilds.
//!
//! The SDK is found by [`EngineSdk::locate`]:
//!
//! - `PULSAR_SDK`, when set (CI, tests, a custom install);
//! - an install's `sdk/` (`pulsar_install_layout::InstallLayout::sdk_dir`):
//!   an **exported** SDK, normalized sources plus [`INDEX_FILE`], made by
//!   [`EngineSdk::export`] in the release pipeline;
//! - the source **checkout** the running engine was built in, found by
//!   walking up from the executable. Its index is built with Cargo and
//!   cached in the mirror's record, keyed by the checkout files it depends
//!   on ([`EngineSdk::index`]).

pub(crate) mod fs;
pub mod index;
pub mod mirror;
pub mod normalize;

use std::path::{Path, PathBuf};

pub use index::{SdkIndex, SdkPackage, SdkWorkspace, INDEX_FILE};
pub use mirror::{MirrorRecord, MirrorReport};

use crate::hash;

/// Overrides where the SDK is.
pub const SDK_ENV: &str = "PULSAR_SDK";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SdkKind {
    /// An engine source checkout (a development build).
    Checkout,
    /// An exported SDK (an install's `sdk/`).
    Exported,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineSdk {
    root: PathBuf,
    kind: SdkKind,
}

impl EngineSdk {
    /// The SDK of the running engine (see the module doc).
    pub fn locate() -> Result<Self, String> {
        if let Some(dir) = std::env::var_os(SDK_ENV).filter(|d| !d.is_empty()) {
            return Self::at(Path::new(&dir));
        }
        for dir in fs::executable_ancestors() {
            let install = pulsar_install_layout::InstallLayout::new(&dir).sdk_dir();
            if fs::is_file(&install.join(INDEX_FILE)) {
                return Self::at(&install);
            }
            if is_checkout(&dir) {
                return Self::at(&dir);
            }
        }
        Err(format!(
            "no engine SDK: the engine is neither an install with an sdk/ directory nor \
             running from its source checkout (set {SDK_ENV} to an SDK)"
        ))
    }

    /// The SDK at `root`: an exported SDK or an engine checkout.
    pub fn at(root: &Path) -> Result<Self, String> {
        let root = &fs::canonical(root)?;
        if fs::is_file(&root.join(INDEX_FILE)) {
            Ok(Self {
                root: root.to_path_buf(),
                kind: SdkKind::Exported,
            })
        } else if is_checkout(root) {
            Ok(Self {
                root: root.to_path_buf(),
                kind: SdkKind::Checkout,
            })
        } else {
            Err(format!(
                "{} is neither an exported engine SDK (no {INDEX_FILE}) nor an engine checkout",
                root.display()
            ))
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn kind(&self) -> SdkKind {
        self.kind
    }

    /// The SDK's index and, for a checkout, the key of the checkout state it
    /// describes. `cached` (the last mirror's record) is reused when its key
    /// still matches, so Cargo runs only after the checkout's manifests or
    /// lockfile change.
    pub fn index(&self, cached: Option<&MirrorRecord>) -> Result<(SdkIndex, Option<String>), String> {
        match self.kind {
            SdkKind::Exported => {
                let text = fs::read_string(&self.root.join(INDEX_FILE))?;
                let index = SdkIndex::from_json(&text)
                    .map_err(|e| format!("{}: {e}", self.root.join(INDEX_FILE).display()))?;
                Ok((index, None))
            }
            SdkKind::Checkout => {
                if let Some(record) = cached {
                    if let Some(key) = &record.index_key {
                        if *key == self.checkout_key(&record.index)? {
                            return Ok((record.index.clone(), Some(key.clone())));
                        }
                    }
                }
                let index = SdkIndex::build(&self.root)?;
                let key = self.checkout_key(&index)?;
                Ok((index, Some(key)))
            }
        }
    }

    /// Export this SDK to `dest` (an install's `sdk/`): its packages,
    /// normalized, and [`INDEX_FILE`].
    pub fn export(&self, dest: &Path) -> Result<SdkIndex, String> {
        let (index, _) = self.index(None)?;
        mirror::mirror(&self.root, &index, None, dest, true)?;
        fs::write(&dest.join(INDEX_FILE), index.to_json().as_bytes())?;
        Ok(index)
    }

    /// Everything a checkout's index is computed from: the workspace
    /// manifest and lockfile, the manifest and plugin descriptor of each
    /// package in `index`, and the manifest of each nested workspace. A package newly reachable from a game is only
    /// reachable through a change to one of these.
    fn checkout_key(&self, index: &SdkIndex) -> Result<String, String> {
        let mut files: Vec<(String, Vec<u8>)> = Vec::new();
        for rel in ["Cargo.toml", "Cargo.lock"] {
            files.push((rel.to_owned(), fs::read(&self.root.join(rel))?));
        }
        let package_files = index.packages.iter().flat_map(|package| {
            ["Cargo.toml", pulsar_plugins::DESCRIPTOR_FILE].map(|file| format!("{}/{file}", package.dir))
        });
        // Nested workspaces (a submodule's own root) feed normalization.
        let workspace_files = index
            .workspaces
            .keys()
            .filter(|dir| !dir.is_empty())
            .map(|dir| format!("{dir}/Cargo.toml"));
        for rel in package_files.chain(workspace_files) {
            let bytes =
                fs::read_if_exists(&crate::paths::join(&self.root, &rel))?.unwrap_or_default();
            files.push((rel, bytes));
        }
        let format = index::INDEX_FORMAT.to_le_bytes();
        Ok(hash::sha256_parts(
            std::iter::once(("index-format", format.as_slice()))
                .chain(files.iter().map(|(rel, bytes)| (rel.as_str(), bytes.as_slice()))),
        ))
    }
}

/// A Cargo workspace root whose `[workspace.dependencies]` has `pulsar_game`
/// at a path: the engine's source checkout.
fn is_checkout(dir: &Path) -> bool {
    let Ok(Some(bytes)) = fs::read_if_exists(&dir.join("Cargo.toml")) else {
        return false;
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return false;
    };
    let Ok(manifest) = toml::from_str::<toml::Table>(&text) else {
        return false;
    };
    manifest
        .get("workspace")
        .and_then(|w| w.get("dependencies"))
        .and_then(|d| d.get("pulsar_game"))
        .and_then(|g| g.get("path"))
        .is_some()
}
