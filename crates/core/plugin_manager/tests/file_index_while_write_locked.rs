//! Editors are created while the global plugin manager is write-locked
//! (`ui_core` opens a tab under `pm.write()`), and an editor's code may
//! refresh the file index. The index looks up a file's registered type
//! through the same manager, so that lookup must not wait for a read lock
//! the calling thread's own write lock is blocking: `parking_lot` locks are
//! not reentrant and the editor froze forever.

use std::sync::mpsc;
use std::time::Duration;

#[test]
fn refreshing_the_index_while_the_plugin_manager_is_write_locked_does_not_hang() {
    plugin_manager::initialize_global(plugin_manager::PluginManager::new());

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let file = root.join("sample.skeleton");
    std::fs::write(&file, b"{}").unwrap();

    let Some(index) = engine_fs::open_project_file_index(&root).unwrap() else {
        return;
    };

    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || {
        let _plugin_manager_write = plugin_manager::global().unwrap().write();
        index.refresh_path(&file).unwrap();
        done.send(()).unwrap();
    });

    finished.recv_timeout(Duration::from_secs(30)).expect(
        "deadlock: ProjectFileIndex::refresh_path waited on the plugin manager lock \
         its own thread holds for writing",
    );
}
