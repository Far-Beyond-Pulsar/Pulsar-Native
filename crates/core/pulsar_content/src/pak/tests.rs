use std::collections::BTreeMap;
use std::path::Path;

use super::*;

fn key() -> ContentKey {
    ContentKey([42u8; 32])
}

fn small() -> PackSettings {
    PackSettings {
        container_size: 64 * 1024,
        block_size: 4 * 1024,
        compression_level: 3,
        max_dead_ratio: 0.5,
    }
}

/// Half noise (incompressible), half runs: compression helps, sizes stay
/// close to the raw size.
fn payload(seed: u32, len: usize) -> Vec<u8> {
    let mut state = 0x9e37_79b9_u32 ^ seed.wrapping_mul(0x85eb_ca6b);
    (0..len)
        .map(|i| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            if (i / 64) % 2 == 0 {
                state as u8
            } else {
                (i / 64) as u8
            }
        })
        .collect()
}

fn files() -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    files.insert("Pulsar/project.json".into(), br#"{"name":"x"}"#.to_vec());
    files.insert("assets/meshes/big.mesh".into(), payload(1, 100_000));
    files.insert("assets/meshes/a.mesh".into(), payload(2, 30_000));
    files.insert("scene/default.level".into(), payload(3, 20_000));
    files.insert("empty.txt".into(), Vec::new());
    files
}

fn pack(dir: &Path, files: &BTreeMap<String, Vec<u8>>, settings: PackSettings) -> (PackReport, Vec<PackEvent>) {
    let mut events = Vec::new();
    let report = write_pak_set(dir, files, key(), settings, &mut |e| events.push(e), &|| false).unwrap();
    (report, events)
}

fn read_all(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let set = PakSet::open(dir, key()).unwrap();
    set.entries()
        .keys()
        .map(|path| ((*path).to_owned(), set.read(path).unwrap()))
        .collect()
}

fn dir_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_set_round_trips_split_into_bounded_containers() {
    let dir = tempfile::tempdir().unwrap();
    let files = files();
    let (report, _) = pack(dir.path(), &files, small());
    assert!(report.rewritten && report.written == files.len());
    assert!(report.containers.len() >= 2, "{:?}", report.containers);
    assert!(report.written_stored < report.written_raw, "compressed");
    assert_eq!(read_all(dir.path()), files);

    let set = PakSet::open(dir.path(), key()).unwrap();
    assert_eq!(set.entry("assets\\meshes/a.mesh").unwrap().len, 30_000);
    assert!(set.contains_dir("assets/meshes") && !set.contains_dir("asset"));
    assert_eq!(
        set.list_dir(""),
        vec![
            ("Pulsar".into(), true, 0),
            ("assets".into(), true, 0),
            ("empty.txt".into(), false, 0),
            ("scene".into(), true, 0)
        ]
    );
    assert!(matches!(set.read("nope"), Err(PakError::Missing(_))));
    // Each container stays within the bound unless one entry alone is bigger.
    for name in &report.containers {
        let len = std::fs::metadata(dir.path().join(name)).unwrap().len();
        assert!(len <= small().container_size || len > 100_000 / 4, "{name}: {len}");
    }
}

#[test]
fn nothing_is_stored_in_plaintext() {
    let dir = tempfile::tempdir().unwrap();
    let mut files = files();
    files.insert("secret/plain.txt".into(), b"THE-PLAINTEXT-MARKER the-plaintext-marker".to_vec());
    pack(dir.path(), &files, small());
    for name in dir_files(dir.path()) {
        let bytes = std::fs::read(dir.path().join(&name)).unwrap();
        for needle in [&b"secret/plain"[..], b"THE-PLAINTEXT-MARKER", b"default.level", b"big.mesh"] {
            assert!(
                !bytes.windows(needle.len()).any(|w| w == needle),
                "{name} holds {}",
                String::from_utf8_lossy(needle)
            );
        }
    }
}

#[test]
fn an_unchanged_set_writes_nothing_and_a_change_writes_only_itself() {
    let dir = tempfile::tempdir().unwrap();
    let mut files = files();
    let (first, _) = pack(dir.path(), &files, small());

    let (same, events) = pack(dir.path(), &files, small());
    assert_eq!((same.written, same.reused, same.rewritten), (0, files.len(), false));
    assert!(!events.iter().any(|e| matches!(e, PackEvent::ContainerWritten { .. })));
    assert_eq!(same.containers, first.containers);

    files.insert("scene/default.level".into(), payload(9, 20_000));
    files.insert("assets/new.mesh".into(), payload(5, 1_000));
    let (patched, _) = pack(dir.path(), &files, small());
    assert_eq!((patched.written, patched.rewritten), (2, false));
    assert_eq!(patched.reused, files.len() - 2);
    let new: Vec<_> = patched.containers.iter().filter(|c| !first.containers.contains(c)).collect();
    assert_eq!(new.len(), 1, "the changes went to one new container: {:?}", patched.containers);
    assert_eq!(read_all(dir.path()), files);
}

#[test]
fn removing_entries_drops_containers_nothing_uses() {
    let dir = tempfile::tempdir().unwrap();
    let mut files = files();
    pack(dir.path(), &files, small());
    files.remove("assets/meshes/big.mesh");
    let mut settings = small();
    settings.max_dead_ratio = 1.0;
    let (report, _) = pack(dir.path(), &files, settings);
    assert!(!report.rewritten);
    assert_eq!(read_all(dir.path()), files);
    let on_disk: Vec<String> = dir_files(dir.path()).into_iter().filter(|n| n.ends_with(".pak")).collect();
    assert_eq!(on_disk, report.containers, "no container nobody reads is left");
}

#[test]
fn too_much_dead_content_rewrites_the_whole_set() {
    let dir = tempfile::tempdir().unwrap();
    let mut files = files();
    let (first, _) = pack(dir.path(), &files, small());
    // Replacing the big entry leaves ~40% of the set dead: above 30%.
    let mut settings = small();
    settings.max_dead_ratio = 0.3;
    files.insert("assets/meshes/big.mesh".into(), payload(77, 100_000));
    let (report, events) = pack(dir.path(), &files, settings);
    assert!(report.rewritten, "{events:?}");
    assert!(report.containers.iter().all(|c| !first.containers.contains(c)));
    assert_eq!(report.removed.len(), first.containers.len());
    assert_eq!(read_all(dir.path()), files);
}

#[test]
fn another_key_is_refused_and_rewritten_over() {
    let dir = tempfile::tempdir().unwrap();
    let files = files();
    pack(dir.path(), &files, small());
    assert!(matches!(PakSet::open(dir.path(), ContentKey([1u8; 32])), Err(PakError::WrongKey)));
    let report = write_pak_set(dir.path(), &files, ContentKey([1u8; 32]), small(), &mut |_| {}, &|| false).unwrap();
    assert!(report.rewritten, "a set under another key is not built on");
}

#[test]
fn a_modified_block_is_caught() {
    let dir = tempfile::tempdir().unwrap();
    let files = files();
    let (report, _) = pack(dir.path(), &files, small());
    for name in &report.containers {
        let path = dir.path().join(name);
        let mut bytes = std::fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x5a;
        std::fs::write(&path, &bytes).unwrap();
    }
    let set = PakSet::open(dir.path(), key()).unwrap();
    let broken = files
        .keys()
        .filter(|path| !files[*path].is_empty())
        .filter(|path| set.read(path).is_err())
        .count();
    assert!(broken >= 1);
}

#[test]
fn a_cancelled_write_leaves_the_previous_set() {
    let dir = tempfile::tempdir().unwrap();
    let mut files = files();
    let (first, _) = pack(dir.path(), &files, small());
    files.insert("scene/default.level".into(), payload(11, 20_000));
    let result = write_pak_set(dir.path(), &files, key(), small(), &mut |_| {}, &|| true);
    assert!(result.is_err());
    let set = PakSet::open(dir.path(), key()).unwrap();
    assert_eq!(set.read("scene/default.level").unwrap(), payload(3, 20_000));
    let paks: Vec<String> = dir_files(dir.path()).into_iter().filter(|n| n.ends_with(".pak")).collect();
    assert_eq!(paks, first.containers);
}

#[test]
fn container_names_parse_back() {
    assert_eq!(container_index(&container_file_name(7)), Some(7));
    assert_eq!(container_index(&container_file_name(12_345)), Some(12_345));
    assert_eq!(container_index("game_7.pak"), None);
    assert_eq!(container_index("game_00a1.pak"), None);
    assert_eq!(container_index("game.pak"), None);
}
