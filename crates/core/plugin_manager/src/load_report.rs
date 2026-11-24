//! What loading the plugin libraries did: the directory, and for every library
//! whether it loaded and, when it did not, the exact reason.
//!
//! Loading logs each failure, but a log line is easy to lose among thousands.
//! The report is the one place that answers "why does the editor have four
//! plugins?": the MCP `plugins_list` tool and any other reader get the directory
//! in use, each library's outcome (the library load error with the imports it is
//! missing, the integrity failure, a version or layout refusal, a dependency or
//! construction failure) and which libraries needed their imports renamed for a
//! shared library of another build (`imports::compat_copy`).
//!
//! The manager is the host's; this report is process-wide in the host's copy of
//! this crate, so it needs no library-boundary seam.

use std::path::{Path, PathBuf};

use parking_lot::Mutex;

/// What became of one library file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LibraryOutcome {
    /// Loaded and verified; its code has not run yet.
    Prepared,
    /// Its plugin is installed.
    Loaded { plugin_id: String },
    /// It did not load. `stage` is where it stopped: `prepare` (the OS loader,
    /// the integrity manifest, the version check) or `install` (dependencies,
    /// attaching the host's state, the plugin's constructor).
    Failed { stage: &'static str, reason: String },
}

/// One library file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryReport {
    pub file: String,
    pub path: PathBuf,
    pub outcome: LibraryOutcome,
    /// Imports renamed to the libraries this engine has (`old -> new`): the
    /// plugin was built against shared libraries of another build and loaded as a
    /// renamed copy (`imports::compat_copy`).
    pub renamed_imports: Vec<String>,
}

/// The directory and every library found in it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadReport {
    /// The directory the libraries were read from; `None` before any load.
    pub directory: Option<PathBuf>,
    /// Whether that directory existed.
    pub directory_exists: bool,
    pub libraries: Vec<LibraryReport>,
}

static REPORT: Mutex<LoadReport> = Mutex::new(LoadReport {
    directory: None,
    directory_exists: false,
    libraries: Vec::new(),
});

/// The report so far.
pub fn load_report() -> LoadReport {
    REPORT.lock().clone()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn entry<'a>(report: &'a mut LoadReport, path: &Path) -> &'a mut LibraryReport {
    // By file name: the same library is named by the directory walk and, once canonicalized
    // (`\\?\` on Windows), by the install, and is one library.
    let file = file_name(path);
    if let Some(at) = report.libraries.iter().position(|l| l.file == file) {
        return &mut report.libraries[at];
    }
    report.libraries.push(LibraryReport {
        file: file_name(path),
        path: path.to_path_buf(),
        outcome: LibraryOutcome::Prepared,
        renamed_imports: Vec::new(),
    });
    report.libraries.last_mut().expect("just pushed")
}

/// A load of `dir` begins: the report starts over.
pub(crate) fn begin(dir: &Path, exists: bool) {
    *REPORT.lock() = LoadReport {
        directory: Some(dir.to_path_buf()),
        directory_exists: exists,
        libraries: Vec::new(),
    };
}

/// `path` was prepared, or refused with `reason`.
pub(crate) fn prepared(path: &Path, failure: Option<String>) {
    let mut report = REPORT.lock();
    let entry = entry(&mut report, path);
    if let Some(reason) = failure {
        entry.outcome = LibraryOutcome::Failed {
            stage: "prepare",
            reason,
        };
    }
}

/// `path` needed these imports renamed.
pub(crate) fn renamed(path: &Path, renames: Vec<String>) {
    let mut report = REPORT.lock();
    entry(&mut report, path).renamed_imports = renames;
}

/// The plugin of `path` was installed, or refused with `reason`.
pub(crate) fn installed(path: &Path, result: Result<&str, String>) {
    let mut report = REPORT.lock();
    let entry = entry(&mut report, path);
    entry.outcome = match result {
        Ok(plugin_id) => LibraryOutcome::Loaded {
            plugin_id: plugin_id.to_owned(),
        },
        Err(reason) => LibraryOutcome::Failed {
            stage: "install",
            reason,
        },
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_says_what_became_of_each_library() {
        let dir = Path::new("plugins/editor");
        begin(dir, true);
        prepared(&dir.join("a_plugin.dll"), None);
        prepared(
            &dir.join("b_plugin.dll"),
            Some("imports x.dll which is missing".into()),
        );
        renamed(
            &dir.join("a_plugin.dll"),
            vec!["scenedb_dylib-1.dll -> scenedb_dylib-2.dll".into()],
        );
        installed(&dir.join("a_plugin.dll"), Ok("pulsar.a"));
        let report = load_report();
        assert_eq!(report.directory.as_deref(), Some(dir));
        assert_eq!(report.libraries.len(), 2);
        assert_eq!(
            report.libraries[0].outcome,
            LibraryOutcome::Loaded {
                plugin_id: "pulsar.a".into()
            }
        );
        assert_eq!(report.libraries[0].renamed_imports.len(), 1);
        assert!(matches!(
            &report.libraries[1].outcome,
            LibraryOutcome::Failed { stage: "prepare", reason } if reason.contains("missing")
        ));
        // A later install failure replaces the earlier outcome of the same file.
        installed(&dir.join("a_plugin.dll"), Err("needs contract c".into()));
        assert!(matches!(
            &load_report().libraries[0].outcome,
            LibraryOutcome::Failed {
                stage: "install",
                ..
            }
        ));
    }
}
