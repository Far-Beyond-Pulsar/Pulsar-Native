use super::*;
use crate::virtual_fs;

// A fixture plugin whose root is this test crate, so `origin!()` here is
// its origin.
const DESCRIPTOR: &str = "id = \"test.provider\"\nname = \"Test provider\"\ndescription = \"A fixture that provides files\"\nenabled_by_default = true\nrequires = []\nroot = \"engine_fs\"\n";

pulsar_plugins::inventory::submit! {
    pulsar_plugins::InstalledPlugin { descriptor: DESCRIPTOR, root: env!("CARGO_PKG_NAME"), build: None }
}

const FILES: &[EmbeddedFile] = &[
    EmbeddedFile {
        path: "templates/one.txt",
        bytes: b"one",
    },
    EmbeddedFile {
        path: "templates/deep/two.txt",
        bytes: b"two!",
    },
];

static FOLDER: OnceLock<Arc<MemoryFolder>> = OnceLock::new();

fn open(folder: PluginFolder) -> Arc<dyn FolderSource> {
    let memory = Arc::new(MemoryFolder::with_files(folder, FILES));
    let _ = FOLDER.set(Arc::clone(&memory));
    memory
}

pulsar_plugins::inventory::submit! {
    FileProvider {
        origin: pulsar_plugins::origin!(),
        open,
        in_game: InGame::Snapshot,
    }
}

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    virtual_fs::write_file(&dir.path().join("own.txt"), b"mine").unwrap();
    mount_project(dir.path());
    dir
}

fn names(path: &Path) -> Vec<String> {
    let mut names: Vec<String> = virtual_fs::list_dir(path)
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    names.sort();
    names
}

#[test]
fn a_providing_plugin_gets_its_folder_with_its_files() {
    let dir = project();
    let root = dir.path();
    let folder = root.join("plugins/test.provider");
    assert_eq!(
        virtual_fs::read_file(&folder.join("templates/one.txt")).unwrap(),
        b"one"
    );
    assert_eq!(
        virtual_fs::read_to_string(&folder.join("templates/deep/two.txt")).unwrap(),
        "two!"
    );
    assert!(virtual_fs::is_dir(&folder.join("templates/deep")));
    assert!(!virtual_fs::exists(&folder.join("templates/none.txt")).unwrap());
    assert!(names(root).contains(&"plugins".to_owned()));
    assert!(names(root).contains(&"own.txt".to_owned()));
    assert!(names(&root.join("plugins")).contains(&"test.provider".to_owned()));
    assert_eq!(names(&folder.join("templates")), ["deep", "one.txt"]);
    let manifest: Vec<String> = virtual_fs::manifest(root)
        .unwrap()
        .into_iter()
        .map(|e| e.path)
        .collect();
    assert!(manifest.contains(&"plugins/test.provider/templates/deep/two.txt".to_owned()));
    assert!(manifest.contains(&"own.txt".to_owned()));
    assert_eq!(
        mount_of(&folder.join("templates/one.txt")).as_deref(),
        Some("plugins/test.provider")
    );
    assert_eq!(mount_of(&root.join("own.txt")), None);
}

#[test]
fn the_owner_changes_its_folder_at_runtime_and_the_change_is_announced() {
    let dir = project();
    let folder = dir.path().join("plugins/test.provider");
    // Mounting opened the provider.
    assert!(virtual_fs::exists(&folder).unwrap());
    let memory = FOLDER.get().expect("opened when mounted");
    let mut events = crate::events::subscribe();
    memory.write("generated/made.txt", b"made".to_vec());
    assert_eq!(
        virtual_fs::read_to_string(&folder.join("generated/made.txt")).unwrap(),
        "made"
    );
    // Every mounted project hears it; this one's path is among them.
    let wanted = folder.join("generated/made.txt");
    let event = loop {
        match events.try_recv() {
            Ok(event) if event.path == wanted => break event,
            Ok(_) | Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {}
            Err(error) => panic!("the write was not announced: {error}"),
        }
    };
    assert_eq!(event.kind, FsChangeKind::Created);
    memory.delete("generated");
    assert!(!virtual_fs::exists(&folder.join("generated/made.txt")).unwrap());
}

#[test]
fn everyone_else_gets_the_named_error() {
    let dir = project();
    let root = dir.path();
    let file = root.join("plugins/test.provider/templates/one.txt");
    let refused = |result: Result<()>| {
        let error = result.unwrap_err();
        assert!(is_read_only(&error), "{error:#}");
        assert!(
            error.to_string().contains("only its plugin can change"),
            "{error}"
        );
    };
    refused(virtual_fs::write_file(&file, b"x"));
    refused(virtual_fs::write_file_atomically(&file, b"x"));
    refused(virtual_fs::create_file(
        &root.join("plugins/test.provider/new.txt"),
        b"x",
    ));
    refused(virtual_fs::append_file(&file, b"x"));
    refused(virtual_fs::delete_path(&file));
    refused(virtual_fs::delete_path(&root.join("plugins")));
    refused(virtual_fs::rename(&file, &root.join("moved.txt")));
    refused(virtual_fs::rename(
        &root.join("own.txt"),
        &root.join("plugins/other/own.txt"),
    ));
    refused(virtual_fs::copy(&root.join("own.txt"), &file));
    refused(virtual_fs::create_dir_all(
        &root.join("plugins/test.provider/new_folder"),
    ));
    refused(virtual_fs::create_dir_all(
        &root.join("plugins/not_a_plugin"),
    ));
    refused(virtual_fs::open_write(&file).map(|_| ()));
    // Copying out of a plugin folder is reading it.
    virtual_fs::copy(&file, &root.join("copied.txt")).unwrap();
    assert_eq!(
        virtual_fs::read_file(&root.join("copied.txt")).unwrap(),
        b"one"
    );
    // The project's own files are the provider's as before.
    virtual_fs::write_file(&root.join("own.txt"), b"changed").unwrap();
}

#[test]
fn an_on_disk_plugins_folder_is_a_problem_and_is_shadowed() {
    let dir = tempfile::tempdir().unwrap();
    let disk = dir.path().join("plugins/stale.txt");
    crate::LocalFsProvider::new()
        .write_file(&disk, b"old")
        .unwrap();
    let problems = mount_problems(dir.path());
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("'plugins/'"), "{}", problems[0]);
    mount_project(dir.path());
    assert!(
        !virtual_fs::exists(&disk).unwrap(),
        "the virtual folder shadows it"
    );
    unmount_project(dir.path());
    assert!(virtual_fs::exists(&disk).unwrap());
}

#[test]
fn an_unmounted_project_is_the_providers_alone() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        !virtual_fs::exists(&dir.path().join("plugins/test.provider/templates/one.txt")).unwrap()
    );
    virtual_fs::write_file(&dir.path().join("plugins/real.txt"), b"x").unwrap();
}

#[test]
fn a_cache_folder_keeps_the_owners_writes_in_the_project() {
    let dir = project();
    let root = dir.path();
    let cache = CacheFolder::new(PluginFolder {
        owner: Arc::from("test.cache"),
    });
    assert!(cache.list_dir(root, "").unwrap().is_empty(), "nothing yet");
    let mut events = crate::events::subscribe();
    cache.write(root, "downloads/pack.json", b"{}").unwrap();
    assert_eq!(cache.read_file(root, "downloads/pack.json").unwrap(), b"{}");
    assert!(
        crate::LocalFsProvider::new()
            .exists(&root.join(".pulsar/plugins/test.cache/downloads/pack.json"))
            .unwrap(),
        "kept in the project's .pulsar"
    );
    let wanted = root.join("plugins/test.cache/downloads/pack.json");
    loop {
        match events.try_recv() {
            Ok(event) if event.path == wanted => break,
            Ok(_) | Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {}
            Err(error) => panic!("the write was not announced: {error}"),
        }
    }
    cache.delete(root, "downloads").unwrap();
    assert!(cache.metadata(root, "downloads/pack.json").is_none());
}

#[test]
fn the_snapshot_holds_the_folders_files_and_a_packaged_copy_serves_them() {
    let dir = project();
    let snapshot = plugin_snapshot(dir.path());
    let ours: Vec<&(String, Vec<u8>)> = snapshot
        .iter()
        .filter(|(path, _)| path.starts_with("plugins/test.provider/templates/"))
        .collect();
    assert!(ours
        .iter()
        .any(|(path, bytes)| path == "plugins/test.provider/templates/one.txt" && bytes == b"one"));
    assert!(ours
        .iter()
        .any(|(path, _)| path == "plugins/test.provider/templates/deep/two.txt"));

    // What a game serves: the snapshot written into its content.
    let game = tempfile::tempdir().unwrap();
    for (path, bytes) in &snapshot {
        crate::LocalFsProvider::new()
            .write_file(&game.path().join(path), bytes)
            .unwrap();
    }
    let packaged = PackagedFolder {
        owner: "test.provider".into(),
    };
    assert_eq!(
        packaged
            .read_file(game.path(), "templates/one.txt")
            .unwrap(),
        b"one"
    );
    let names: Vec<String> = packaged
        .list_dir(game.path(), "templates")
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    assert!(names.contains(&"deep".to_owned()) && names.contains(&"one.txt".to_owned()));
}

#[test]
fn an_owners_runtime_write_reaches_the_project_file_index_the_drawer_lists() {
    let dir = project();
    let root = dir.path().canonicalize().unwrap();
    mount_project(&root);
    let index = crate::open_project_file_index(&root)
        .unwrap()
        .expect("a local project has a file index");
    let listed = |name: &str| {
        index
            .query(&crate::FileIndexQuery {
                name_contains: Some(name.to_owned()),
                ..Default::default()
            })
            .unwrap()
            .into_iter()
            .any(|file| file.path == format!("plugins/test.provider/live/{name}"))
    };
    assert!(!listed("index_probe.txt"));
    let memory = FOLDER.get().expect("opened when mounted");
    memory.write("live/index_probe.txt", b"probe".to_vec());
    assert!(
        listed("index_probe.txt"),
        "the write's event updated the index"
    );
    memory.delete("live/index_probe.txt");
    assert!(!listed("index_probe.txt"), "and so did the delete's");
}

#[test]
fn opening_a_projects_file_index_mounts_it_and_lists_the_plugin_folders() {
    // Not mounted first: the editor's loading screen opens the index before
    // the project path is published.
    let dir = tempfile::tempdir().unwrap();
    virtual_fs::write_file(&dir.path().join("own.txt"), b"mine").unwrap();
    let index = crate::open_project_file_index(dir.path())
        .unwrap()
        .expect("a local project has a file index");
    let paths: Vec<String> = index
        .query(&crate::FileIndexQuery::default())
        .unwrap()
        .into_iter()
        .map(|file| file.path)
        .collect();
    assert!(paths.contains(&"own.txt".to_owned()), "{paths:?}");
    assert!(
        paths.contains(&"plugins/test.provider/templates/one.txt".to_owned()),
        "{paths:?}"
    );
    unmount_project(dir.path());
}

#[test]
fn a_verbatim_canonical_path_is_the_same_project() {
    // Mounted by its plain path, asked by its canonical (verbatim on Windows).
    let dir = project();
    let canonical = std::fs::canonicalize(dir.path()).unwrap();
    assert_eq!(
        virtual_fs::read_file(&canonical.join("plugins/test.provider/templates/one.txt")).unwrap(),
        b"one"
    );
    assert!(names(&canonical).contains(&"plugins".to_owned()));
    unmount_project(dir.path());
}
