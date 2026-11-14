//! Sync: bring a project's generated files up to date with its inputs and
//! the engine. The one entry point every caller uses (the editor when a
//! project opens or its inputs change, Build, Play, Package, the command
//! line). Cheap when nothing changed: the SDK mirror compares stamps, the
//! generated files are rendered in memory and only differing ones are
//! written.
//!
//! Order matters for an interrupted sync: the SDK mirror first (what the
//! Cargo files point at), then the generated files, the lockfile and
//! `.gitignore`, and the generation record last.

use std::path::Path;
use std::sync::Mutex;

use engine_fs::virtual_fs;

use crate::apply::{self, ApplyReport, GenerationRecord, RECORD_FORMAT};
use crate::descriptor::ProjectDescriptor;
use crate::error::SyncError;
use crate::model::ProjectModel;
use crate::paths;
use crate::render;
use crate::sdk::{mirror, EngineSdk, MirrorRecord, MirrorReport};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncOptions {
    /// Compare every SDK file's content instead of trusting the mirror's
    /// record (and remove anything else under `.pulsar/sdk/`), and seed the
    /// lockfile again.
    pub force: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncReport {
    /// The game crate's package name.
    pub package: String,
    /// The game executable's target name.
    pub bin: String,
    /// The Play-In-Editor library's target name.
    pub lib: String,
    pub files: ApplyReport,
    pub sdk: MirrorReport,
    pub lock_seeded: bool,
    pub gitignore_updated: bool,
    /// What is wrong but does not stop the game building.
    pub problems: Vec<String>,
}

impl SyncReport {
    /// Whether the sync changed anything a build reads.
    pub fn changed(&self) -> bool {
        !self.files.written.is_empty()
            || !self.files.deleted.is_empty()
            || self.sdk.written > 0
            || self.sdk.deleted > 0
            || self.lock_seeded
    }
}

/// Sync the project at `project_root` against the running engine's SDK.
pub fn sync(project_root: &Path) -> Result<SyncReport, SyncError> {
    let sdk = EngineSdk::locate().map_err(SyncError::Sdk)?;
    sync_with(project_root, &sdk, SyncOptions::default())
}

/// Sync the project at `project_root` against `sdk`.
pub fn sync_with(
    project_root: &Path,
    sdk: &EngineSdk,
    options: SyncOptions,
) -> Result<SyncReport, SyncError> {
    // Two syncs of one process never interleave (the watcher and Play can
    // ask at once); each is atomic per file for anything else reading.
    static SYNC: Mutex<()> = Mutex::new(());
    let _guard = SYNC.lock().unwrap_or_else(|poison| poison.into_inner());

    let descriptor = ProjectDescriptor::read(project_root)?;

    let mirror_dir = paths::join(project_root, paths::SDK_DIR);
    let cached = MirrorRecord::read(&mirror_dir);
    let (index, index_key) = sdk.index(cached.as_ref()).map_err(SyncError::Sdk)?;
    let sdk_report = mirror::mirror(sdk.root(), &index, index_key, &mirror_dir, options.force)
        .map_err(SyncError::Sdk)?;

    let model = ProjectModel::resolve(project_root, &descriptor, &index)?;
    let files = render::render(&model, &index)?;
    let previous = GenerationRecord::read(project_root)?;
    let (report, hashes) = apply::apply(project_root, &files, previous.as_ref())?;

    let fingerprint = index.fingerprint();
    let lock_path = paths::join(project_root, paths::LOCKFILE);
    let lock_seeded = options.force
        || !virtual_fs::is_file(&lock_path)
        || previous.as_ref().is_none_or(|r| r.engine != fingerprint);
    if lock_seeded {
        let existing = virtual_fs::read_to_string(&lock_path).ok();
        let lock = render::lock::seed(&index.lock, existing.as_deref()).map_err(SyncError::Sdk)?;
        apply::write(&lock_path, lock.as_bytes())?;
    }

    let gitignore_updated = ensure_ignored(project_root)?;

    GenerationRecord {
        format: RECORD_FORMAT,
        engine: fingerprint,
        files: hashes,
    }
    .write(project_root)?;

    for problem in &model.problems {
        tracing::warn!(project = %project_root.display(), "{problem}");
    }
    for drifted in &report.drifted {
        tracing::warn!(
            project = %project_root.display(),
            "{drifted} was edited by hand and has been regenerated: generated files are \
             rewritten on every sync (see Pulsar.toml and the scripts crates)"
        );
    }

    Ok(SyncReport {
        package: model.package,
        bin: model.bin,
        lib: model.lib,
        files: report,
        sdk: sdk_report,
        lock_seeded,
        gitignore_updated,
        problems: model.problems,
    })
}

/// Add the lines of [`paths::IGNORED`] the project's `.gitignore` lacks.
/// The file is otherwise the user's.
fn ensure_ignored(project_root: &Path) -> Result<bool, SyncError> {
    let path = paths::join(project_root, paths::GITIGNORE);
    let existing = if virtual_fs::is_file(&path) {
        virtual_fs::read_to_string(&path).map_err(|e| SyncError::io(&path, e))?
    } else {
        String::new()
    };
    let present: Vec<&str> = existing.lines().map(str::trim).collect();
    let missing: Vec<&str> = paths::IGNORED
        .iter()
        .copied()
        .filter(|line| {
            let bare = line.trim_end_matches('/');
            !present.iter().any(|p| *p == *line || *p == bare)
        })
        .collect();
    if missing.is_empty() {
        return Ok(false);
    }
    let mut text = existing;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str("# Pulsar: build output and files each machine regenerates\n");
    for line in missing {
        text.push_str(line);
        text.push('\n');
    }
    apply::write(&path, text.as_bytes())?;
    Ok(true)
}
