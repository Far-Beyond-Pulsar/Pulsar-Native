//! Import jobs. Every conversion is a task in the editor task queue, so it
//! shows up (with progress, errors and cancellation) in the Tasks window.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use editor_task_queue::{TaskContext, TaskDescription, TaskDuration, TaskId};
use engine_fs::virtual_fs;
use parking_lot::Mutex;

use crate::db::{hash_file, relative_key, ImportDb, ImportRecord};
use crate::importer::{importer_by_id, importer_for, Importer, OptionValues};

const TASK_CATEGORY: &str = "Import";

/// Serializes read-modify-write of the database across concurrent tasks.
static DB_LOCK: Mutex<()> = Mutex::new(());

fn with_db(project_root: &Path, edit: impl FnOnce(&mut ImportDb)) -> Result<(), String> {
    let _guard = DB_LOCK.lock();
    let mut db = ImportDb::load(project_root);
    edit(&mut db);
    db.save(project_root).map_err(|error| format!("{error:#}"))
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
    let destination = project_root.join(TRASH_DIR).join(stamp.to_string()).join(key);
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
    importer.import(source, &asset, &options.lock())?;

    task.report_progress(0.9, "Recording");
    match mode {
        ImportMode::Link => with_db(project_root, |db| {
            db.upsert(ImportRecord {
                source: key,
                source_hash,
                source_size,
                asset: relative_key(project_root, &asset),
                importer: importer.id().to_owned(),
            })
        }),
        ImportMode::ConvertInPlace => {
            // The asset is written; only now is the source expendable. It goes
            // to the trash rather than away. The conversion has succeeded
            // either way, so a failed move is a warning, not a failed task.
            match move_to_trash(project_root, source, &key) {
                Ok(trashed) => {
                    tracing::info!("converted {}; source moved to {}", source.display(), trashed.display());
                }
                Err(error) => {
                    tracing::warn!("converted {}, but could not move it to the trash: {error:#}", source.display());
                    task.report_progress(0.95, "Converted; source left in place");
                }
            }
            with_db(project_root, |db| db.remove_source(&key))
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
            editor_task_queue::global().submit(description, move |task| {
                convert(&root, &source, mode, &options, &task)
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
    Some(editor_task_queue::global().submit(description, move |task| {
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
        let options = importer.stored_options(&asset, ext);
        importer.import(&source, &asset, &options)?;
        task.report_progress(0.9, "Recording");
        with_db(&project_root, |db| {
            db.upsert(ImportRecord {
                source_hash,
                source_size,
                ..record
            })
        })
    }))
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

#[cfg(test)]
mod tests {
    use super::*;
    use editor_task_queue::TaskStatus;

    fn wait(id: TaskId) -> (TaskStatus, Option<String>) {
        for _ in 0..600 {
            if let Some(snapshot) = editor_task_queue::global()
                .snapshots()
                .into_iter()
                .find(|snapshot| snapshot.id == id)
            {
                if !matches!(snapshot.status, TaskStatus::Queued | TaskStatus::Running) {
                    return (snapshot.status, snapshot.error);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("task did not finish");
    }

    fn cube() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../assets/meshes/primitives/SM_Cube.fbx")
    }

    #[test]
    fn link_records_the_source_and_reimport_follows_edits() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let source = root.join("cube.fbx");
        std::fs::copy(cube(), &source).unwrap();
        let options = Arc::new(Mutex::new(OptionValues::new()));

        let ids = submit_import(root.clone(), vec![source.clone()], ImportMode::Link, options);
        assert_eq!(wait(ids[0]), (TaskStatus::Succeeded, None));
        assert!(source.exists() && root.join("cube.mesh").exists());
        let db = ImportDb::load(&root);
        let record = db.find_source("cube.fbx").expect("linked");
        assert_eq!(record.asset, "cube.mesh");
        assert_eq!(db.status(&root, record), crate::LinkStatus::Current);

        let mut bytes = std::fs::read(&source).unwrap();
        bytes.push(0);
        std::fs::write(&source, bytes).unwrap();
        assert_eq!(db.status(&root, record), crate::LinkStatus::OutOfDate);
    }

    #[test]
    fn reimport_keeps_default_materials_and_refreshes_the_record() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let source = root.join("cube.fbx");
        std::fs::copy(cube(), &source).unwrap();
        let options = Arc::new(Mutex::new(OptionValues::new()));
        let ids = submit_import(root.clone(), vec![source.clone()], ImportMode::Link, options);
        assert_eq!(wait(ids[0]).0, TaskStatus::Succeeded);

        let asset = root.join("cube.mesh");
        helio_component::mesh_cache::set_default_materials(&asset, &["materials/Keep.mat".into()])
            .unwrap();
        let mut bytes = std::fs::read(&source).unwrap();
        bytes.push(0);
        std::fs::write(&source, bytes).unwrap();

        let id = submit_reimport(root.clone(), "cube.fbx").expect("linked");
        assert_eq!(wait(id), (TaskStatus::Succeeded, None));
        let (decoded, _) =
            helio_component::mesh_cache::decode_asset(&std::fs::read(&asset).unwrap()).unwrap();
        assert_eq!(decoded.material_slots[0].material_asset, "materials/Keep.mat");
        let db = ImportDb::load(&root);
        assert_eq!(
            db.status(&root, db.find_source("cube.fbx").unwrap()),
            crate::LinkStatus::Current
        );
    }

    #[test]
    fn convert_in_place_replaces_the_source_and_keeps_no_record() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let source = root.join("cube.fbx");
        std::fs::copy(cube(), &source).unwrap();
        let options = Arc::new(Mutex::new(OptionValues::new()));

        let ids = submit_import(root.clone(), vec![source.clone()], ImportMode::ConvertInPlace, options);
        assert_eq!(wait(ids[0]), (TaskStatus::Succeeded, None));
        assert!(!source.exists() && root.join("cube.mesh").exists());
        assert!(ImportDb::load(&root).records.is_empty());

        // The source is recoverable from the trash, byte for byte.
        let trashed: Vec<_> = std::fs::read_dir(root.join(TRASH_DIR))
            .unwrap()
            .map(|entry| entry.unwrap().path().join("cube.fbx"))
            .collect();
        assert_eq!(trashed.len(), 1);
        assert_eq!(std::fs::read(&trashed[0]).unwrap(), std::fs::read(cube()).unwrap());
    }
}
