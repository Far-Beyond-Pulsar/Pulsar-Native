//! The file index takes two locks: its SQLite connection and the durable
//! asset-id manifest. Every path must take them in the same order, or a
//! rename event on one thread and a picker creating an id on another can wait
//! on each other forever (the editor freezes).

use std::sync::mpsc;
use std::time::Duration;

const ROUNDS: usize = 400;

#[test]
fn moving_paths_and_creating_ids_concurrently_never_deadlocks() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("a.bin"), b"a").unwrap();
    std::fs::write(root.join("b.bin"), b"b").unwrap();

    let Some(index) = engine_fs::open_project_file_index(&root).unwrap() else {
        return;
    };

    let (done_tx, done_rx) = mpsc::channel();

    let mover = {
        let index = index.clone();
        let root = root.clone();
        let done = done_tx.clone();
        std::thread::spawn(move || {
            for _ in 0..ROUNDS {
                index
                    .move_path(&root.join("a.bin"), &root.join("moved.bin"))
                    .unwrap();
                index
                    .move_path(&root.join("moved.bin"), &root.join("a.bin"))
                    .unwrap();
            }
            done.send("mover").unwrap();
        })
    };
    let creator = {
        let index = index.clone();
        let root = root.clone();
        std::thread::spawn(move || {
            for _ in 0..ROUNDS {
                index
                    .ensure_typed_asset_id("image", &root.join("b.bin"))
                    .unwrap();
            }
            done_tx.send("creator").unwrap();
        })
    };

    for _ in 0..2 {
        done_rx
            .recv_timeout(Duration::from_secs(60))
            .expect("deadlock: file-index move_path and ensure_typed_asset_id wait on each other");
    }
    mover.join().unwrap();
    creator.join().unwrap();
}
