//! Import jobs. Every conversion is a task in the editor task queue, so it
//! shows up (with progress, errors and cancellation) in the Tasks window.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use editor_task_queue::{TaskContext, TaskDescription, TaskDuration, TaskId};
use engine_fs::virtual_fs;
use parking_lot::Mutex;

use crate::db::{hash_file, relative_key, ImportDb, ImportRecord};
use crate::importer::{importer_by_id, importer_for, Imported, Importer, OptionValues, Previous};

const TASK_CATEGORY: &str = "Import";

/// Serializes read-modify-write of the database across concurrent tasks.
static DB_LOCK: Mutex<()> = Mutex::new(());

fn with_db(project_root: &Path, edit: impl FnOnce(&mut ImportDb)) -> Result<(), String> {
    let _guard = DB_LOCK.lock();
    let (mut db, migration) = ImportDb::load_with_migration(project_root);
    edit(&mut db);
    let saved = db.save(project_root).map_err(|error| format!("{error:#}"));
    if saved.is_ok() {
        // The save carried the legacy options folded into the records; only
        // now may the legacy store drop them. This is the one place the
        // migration writes, and it holds `DB_LOCK` like every other write.
        if let Some(migration) = migration {
            migration.commit(project_root);
        }
    }
    crate::state::bump_db_revision();
    saved
}

/// What happens to the source file once its native asset is built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportMode {
    /// The native asset replaces the source file, which moves to the
    /// project's trash folder ([`TRASH_DIR`]). No record is kept, so the
    /// conversion is one-way.
    ConvertInPlace,
    /// The source stays. A record ties it to the native asset, and later
    /// changes to the source are detected.
    Link,
}

/// Where convert-in-place moves replaced sources, relative to the project
/// root. Each conversion gets its own timestamped folder, with the source at
/// its project-relative path inside it, so nothing is overwritten.
pub const TRASH_DIR: &str = ".pulsar/trash";

/// Move `source` (whose record key is `key`) into the project trash. Returns
/// where it went.
fn move_to_trash(project_root: &Path, source: &Path, key: &str) -> anyhow::Result<PathBuf> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    let destination = project_root
        .join(TRASH_DIR)
        .join(stamp.to_string())
        .join(key);
    if let Some(parent) = destination.parent() {
        virtual_fs::create_dir_all(parent)?;
    }
    virtual_fs::rename(source, &destination)?;
    Ok(destination)
}

fn file_title(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file")
        .to_owned()
}

fn importer_of(source: &Path) -> Result<&'static dyn Importer, String> {
    source
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(importer_for)
        .ok_or_else(|| format!("{} is not an importable format", source.display()))
}

fn convert(
    project_root: &Path,
    source: &Path,
    mode: ImportMode,
    options: &Mutex<OptionValues>,
    companions: &[String],
    task: &TaskContext,
) -> Result<(), String> {
    let importer = importer_of(source)?;
    let dir = source.parent().unwrap_or(project_root);
    let asset = importer.asset_path(dir, source);
    let key = relative_key(project_root, source);

    task.report_progress(0.1, "Reading source");
    let (source_hash, source_size) = hash_file(source).map_err(|error| format!("{error:#}"))?;
    if task.is_cancelled() {
        return Err("cancelled".into());
    }

    task.report_progress(0.3, "Converting");
    // What an earlier import of this source wrote, so it keeps its ids.
    let earlier = ImportDb::load(project_root)
        .find_source(&key)
        .map(|record| record.outputs.clone())
        .unwrap_or_default();
    let chosen_options = options.lock();
    let mut imported = importer.import(
        source,
        &asset,
        &chosen_options,
        &Previous {
            project_root,
            outputs: &earlier,
        },
    )?;
    let stored_options = importer.serialize_options(&chosen_options);
    drop(chosen_options);
    run_companions(
        project_root,
        importer,
        source,
        companions,
        &mut imported,
        task,
    );

    task.report_progress(0.9, "Recording");
    match mode {
        ImportMode::Link => with_db(project_root, |db| {
            db.upsert(ImportRecord {
                source: key,
                source_hash,
                source_size,
                asset: relative_key(project_root, &imported.asset),
                importer: importer.id().to_owned(),
                options: Some(stored_options),
                outputs: imported.outputs,
                companions: companions.to_vec(),
            })
        }),
        ImportMode::ConvertInPlace => {
            // The asset is written; only now is the source expendable. It goes
            // to the trash rather than away. The conversion has succeeded
            // either way, so a failed move is a warning, not a failed task.
            match move_to_trash(project_root, source, &key) {
                Ok(trashed) => {
                    tracing::info!(
                        "converted {}; source moved to {}",
                        source.display(),
                        trashed.display()
                    );
                }
                Err(error) => {
                    tracing::warn!(
                        "converted {}, but could not move it to the trash: {error:#}",
                        source.display()
                    );
                    task.report_progress(0.95, "Converted; source left in place");
                }
            }
            with_db(project_root, |db| db.remove_source(&key))
        }
    }
}

/// Run the import companions the user accepted, now that the native asset
/// exists. Their outputs join the import's, and the surfaces they made
/// something for get it as the native asset's default where it has none. A
/// companion failing never fails the import.
fn run_companions(
    project_root: &Path,
    importer: &dyn Importer,
    source: &Path,
    companions: &[String],
    imported: &mut Imported,
    task: &TaskContext,
) {
    if companions.is_empty() {
        return;
    }
    task.report_progress(0.7, "Running import companions");
    // A source that became several meshes gets companions run for each, with
    // the surfaces that mesh uses; otherwise the one mesh and all surfaces.
    let mut outputs = Vec::new();
    let mut assigned: Vec<(&Path, Vec<(u32, String)>)> = Vec::new();
    if imported.parts.is_empty() {
        let (made, assignments) = crate::companion::run_companions(
            companions,
            project_root,
            source,
            &imported.asset,
            &imported.surfaces,
        );
        outputs.extend(made);
        assigned.push((&imported.asset, assignments));
    } else {
        for part in &imported.parts {
            let (made, assignments) = crate::companion::run_companions(
                companions,
                project_root,
                source,
                &part.asset,
                &part.surfaces,
            );
            outputs.extend(made);
            assigned.push((&part.asset, assignments));
        }
    }
    if outputs.is_empty() {
        // Not an error, but the user asked for something and should see it
        // did not happen.
        let found: usize = imported.surfaces.iter().map(|s| s.images.len()).sum();
        task.report_progress(
            0.8,
            &format!(
                "Import companions made nothing ({} surface(s), {found} matching image(s))",
                imported.surfaces.len()
            ),
        );
    }
    for output in outputs {
        if !imported.outputs.iter().any(|o| o.path == output.path) {
            imported.outputs.push(output);
        }
    }
    for (asset, assignments) in assigned {
        if let Err(error) = importer.assign_default_materials(asset, &assignments) {
            tracing::warn!(
                "could not assign companion outputs to {}: {error}",
                asset.display()
            );
        }
    }
}

/// Convert `sources` (project files of importable formats), one task each,
/// with the shared `options` the configurator produced for their format.
pub fn submit_import(
    project_root: PathBuf,
    sources: Vec<PathBuf>,
    mode: ImportMode,
    options: Arc<Mutex<OptionValues>>,
) -> Vec<TaskId> {
    submit_import_with_companions(project_root, sources, mode, options, Vec::new())
}

/// [`submit_import`], also running the import companions named in
/// `companions` (ids from [`crate::offers_for`]) once each asset is written.
pub fn submit_import_with_companions(
    project_root: PathBuf,
    sources: Vec<PathBuf>,
    mode: ImportMode,
    options: Arc<Mutex<OptionValues>>,
    companions: Vec<String>,
) -> Vec<TaskId> {
    sources
        .into_iter()
        .map(|source| {
            let verb = match mode {
                ImportMode::ConvertInPlace => "Convert",
                ImportMode::Link => "Import",
            };
            let description = TaskDescription::new(
                format!("{verb} {}", file_title(&source)),
                TASK_CATEGORY,
                TaskDuration::Long,
            );
            let (root, options) = (project_root.clone(), Arc::clone(&options));
            let companions = companions.clone();
            editor_task_queue::global().submit(description, move |task| {
                convert(&root, &source, mode, &options, &companions, &task)
            })
        })
        .collect()
}

/// Scan `project_root` for importable sources as a task, so it shows in the
/// Tasks window, and hand the report to `done` on the task's thread.
pub fn submit_scan(
    project_root: PathBuf,
    done: impl FnOnce(crate::ScanReport) + Send + 'static,
) -> TaskId {
    let description = TaskDescription::new("Scan for imports", TASK_CATEGORY, TaskDuration::Short);
    editor_task_queue::global().submit(description, move |task| {
        task.report_progress(0.0, "Scanning project");
        done(crate::scan_project(&project_root));
        Ok(())
    })
}

/// Reimport the linked source `source` (a record key) with the options its
/// last import used. `None` if the source has no record.
pub fn submit_reimport(project_root: PathBuf, source: &str) -> Option<TaskId> {
    let record = ImportDb::load(&project_root).find_source(source).cloned()?;
    let description = TaskDescription::new(
        format!("Reimport {}", file_title(Path::new(&record.source))),
        TASK_CATEGORY,
        TaskDuration::Long,
    );
    Some(
        editor_task_queue::global().submit(description, move |task| {
            let importer = importer_by_id(&record.importer)
                .ok_or_else(|| format!("unknown importer '{}'", record.importer))?;
            let source = project_root.join(&record.source);
            let asset = project_root.join(&record.asset);
            let ext = source.extension().and_then(|e| e.to_str()).unwrap_or("");

            task.report_progress(0.1, "Reading source");
            let (source_hash, source_size) =
                hash_file(&source).map_err(|error| format!("{error:#}"))?;
            if task.is_cancelled() {
                return Err("cancelled".into());
            }
            task.report_progress(0.3, "Converting");
            let options = importer.stored_options(&asset, ext, record.options.as_ref());
            let mut imported = importer.import(
                &source,
                &asset,
                &options,
                &Previous {
                    project_root: &project_root,
                    outputs: &record.outputs,
                },
            )?;
            run_companions(
                &project_root,
                importer,
                &source,
                &record.companions,
                &mut imported,
                &task,
            );
            task.report_progress(0.9, "Recording");
            with_db(&project_root, |db| {
                db.upsert(ImportRecord {
                    source_hash,
                    source_size,
                    asset: relative_key(&project_root, &imported.asset),
                    options: Some(importer.serialize_options(&options)),
                    outputs: imported.outputs,
                    ..record
                })
            })
        }),
    )
}

/// Remember that the user declined to import `sources`, so scans stop
/// offering them.
pub fn ignore_sources(project_root: &Path, sources: &[PathBuf]) -> Result<(), String> {
    with_db(project_root, |db| {
        for source in sources {
            db.ignore(&relative_key(project_root, source));
        }
    })
}

/// Drop the link between the source `source` (a record key) and its native
/// asset. Both files stay; the source is no longer checked for changes, and
/// scans offer it for import again.
pub fn unlink_source(project_root: &Path, source: &str) -> Result<(), String> {
    with_db(project_root, |db| db.remove_source(source))
}

/// Forget that the user declined to import `sources`, so scans and the
/// content drawer offer them again.
pub fn unignore_sources(project_root: &Path, sources: &[PathBuf]) -> Result<(), String> {
    with_db(project_root, |db| {
        for source in sources {
            db.unignore(&relative_key(project_root, source));
        }
    })
}
