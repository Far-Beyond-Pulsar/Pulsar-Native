# Filesystem

Every file the engine, the editor and the plugins read or write goes through
`engine_fs::virtual_fs`. It routes to the active `FsProvider` (local disk, a
remote `pulsar-studio` project, a peer-to-peer session) and composes the
read-only **mounts** over it. `std::fs`, and the `Path` methods that ask the
disk, are disallowed methods across the workspace (see "The rule" below).

## The provider trait

```rust
pub trait FsProvider: Send + Sync + 'static {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>>;
    fn read_file_prefix(&self, path: &Path, len: usize) -> Result<Vec<u8>>;
    fn write_file(&self, path: &Path, content: &[u8]) -> Result<()>;
    fn write_file_atomically(&self, path: &Path, content: &[u8]) -> Result<()>;
    fn create_file(&self, path: &Path, content: &[u8]) -> Result<()>;
    fn create_executable_file(&self, path: &Path, content: &[u8]) -> Result<()>;
    fn append_file(&self, path: &Path, content: &[u8]) -> Result<()>;
    fn copy(&self, from: &Path, to: &Path) -> Result<()>;
    fn open_read(&self, path: &Path) -> Result<Box<dyn ReadSeek>>;
    fn open_write_stream(&self, path: &Path) -> Result<Option<Box<dyn Write + Send>>>;
    fn delete_path(&self, path: &Path) -> Result<()>;
    fn rename(&self, from: &Path, to: &Path) -> Result<()>;
    fn list_dir(&self, path: &Path) -> Result<Vec<FsEntry>>;
    fn create_dir_all(&self, path: &Path) -> Result<()>;
    fn exists(&self, path: &Path) -> Result<bool>;
    fn metadata(&self, path: &Path) -> Result<FsMetadata>;
    fn canonicalize(&self, path: &Path) -> Result<PathBuf>;
    fn is_symlink(&self, path: &Path) -> Result<bool>;
    fn manifest(&self, path: &Path) -> Result<Vec<ManifestEntry>>;
    fn is_remote(&self) -> bool { false }
    fn label(&self) -> &str { "Local" }
}
```

The newer operations have fallbacks over the core ones (`append_file` reads and
rewrites, `open_read` reads into memory, `open_write_stream` is `None` and
`virtual_fs::open_write` buffers and writes at `finish`); the local provider
does each in place.

| Provider | Source | Use case |
|---|---|---|
| `LocalFsProvider` | `engine_fs::providers::local` | Local disk, optional sandbox root |
| `RemoteFsProvider` | `engine_fs::providers::remote` | `pulsar-studio` HTTP API |
| `P2pFsProvider` | `engine_fs::providers::p2p` | P2P multiplayer sessions |

`LocalFsProvider::with_root(path)` canonicalizes every path and rejects path
traversal.

## The free functions

```rust
engine_fs::virtual_fs::set_provider(provider);   // swap the backend; mounts stay
engine_fs::virtual_fs::read_file(path) / read_to_string(path) / read_file_prefix(path, n)
engine_fs::virtual_fs::write_file(path, bytes) / write_file_atomically / create_file / append_file
engine_fs::virtual_fs::open_read(path)            // Box<dyn Read + Seek>
engine_fs::virtual_fs::open_write(path)           // FsWriter: write, then finish()
engine_fs::virtual_fs::copy(from, to) / rename(from, to) / delete_path(path)
engine_fs::virtual_fs::list_dir(path) / create_dir_all(path) / manifest(path)
engine_fs::virtual_fs::exists(path) / metadata(path) / is_file / is_dir / canonicalize / is_symlink
engine_fs::virtual_fs::find_by_extension(root, ext)
engine_fs::virtual_fs::provider()                 // the active provider, mounts composed
```

The free functions clone the active layer out of its lock before calling it, so
a provider whose events call back into the VFS (the file index refreshing a
path) cannot deadlock.

## Read-only mounts

A project sees two kinds of content it did not author and cannot change
(`engine_fs::virtual_fs::mounts`):

- `<project>/pulsar/`: the engine's (its trait files, `pulsar_traits::builtin`);
- `<project>/plugins/<plugin id>/`: each enabled plugin's. Every enabled
  installed plugin has one, so the folder lists exactly the project's plugins.

The files are embedded in the binary that links their owner and registered as
`engine_fs::MountedContent` (an origin-gated contribution, category
`fs.mounted_content`). The registration's **owner** picks the mount: the
engine's (`pulsar_plugins::engine_origin!`) goes to `pulsar/`, a plugin root's
to `plugins/<its id>/`, so no plugin can put files in another's folder. The
mount list follows the enabled set (`pulsar_plugins::generation()`): a disabled
plugin's folder is gone.

```rust
inventory::submit! {
    engine_fs::MountedContent {
        origin: pulsar_plugins::origin!(),
        id: "pulsar.lights.traits",
        files: engine_fs::mounted_files!("content": "traits/light_source.trait.json"),
    }
}
```

`MountedFs` is the layer every call goes through: paths inside a mount are
answered from memory, everything else goes to the active provider, so mounts
look the same over a local, remote or P2P project. **Writing, creating,
appending, deleting or renaming** anything inside a mount fails with
`engine_fs::ReadOnlyMount { mount, path }` inside the `anyhow::Error`
(`virtual_fs::is_read_only(&error)`). A project is mounted with
`virtual_fs::mount_project(root)`: `engine_state::set_project_path` (the editor),
and the trait index's `load`/`rebuild`/`live` (so a game's runtime and tests get
them too). `virtual_fs::mount_of(path)` names the mount a path is in (the file
drawer's read-only state).

An on-disk `plugins/` or `pulsar/` folder in a project is shadowed by the mount
and reported (`virtual_fs::mount_problems(root)`): logged when the project is
mounted, and shown as a banner above the file drawer's files.

How others do it: Unreal stacks `IPlatformFile` layers (the pak layer over the
physical one) and registers a mount point per plugin's content
(`FPackageName::RegisterMountPoint`); Unity shows immutable packages under
`Packages/<name>` and refuses edits. Ours is Unreal's layering with Unity's
layout. Cost: a lookup over the mounted project roots per call (usually one).

## The rule: no `std::fs`

The root `clippy.toml` lists `std::fs`'s functions, `File::open`/`create`,
`OpenOptions::open` and the disk-asking `Path` methods (`exists`, `is_file`,
`is_dir`, `metadata`, `read_dir`, `canonicalize`, ...) as `disallowed-methods`,
and `[workspace.lints.clippy]` denies `disallowed_methods`. `just clippy` lints
the libraries and binaries (tests are exempt). Code that must touch the real
disk says `#[allow(clippy::disallowed_methods)]` and its file is listed with the
reason in `architecture/std-fs-allowed.txt`, which only shrinks
(`crates/core/engine_fs/tests/std_fs_allow_list.rs`): the local provider, build
scripts, and tools that work on the real machine (the packager's output, the
native-library loader, the engine's own install).

## FsEvent

File changes the editor makes are broadcast through `tokio::sync::broadcast`:

```rust
pub struct FsEvent { pub path: PathBuf, pub kind: FsChangeKind, pub source: FsEventSource }
engine_fs::subscribe() -> broadcast::Receiver<FsEvent>;
engine_fs::events::emit(path, kind) / emit_remote(path, kind) / emit_move(from, to, source)
```

## The project file index

For local projects, `engine_fs::open_project_file_index(project_root)` returns
the process-shared disposable SQLite `ProjectFileIndex` at
`.pulsar/cache/files.db`; the loading splash populates and reconciles it from
`virtual_fs::manifest` (mounts included). It stores project-relative path, name,
extension, registered file type, directory flag, size and modified time. The
file type comes from `engine_fs::FileTypeClassifier` registrations (the editor's
file-type registry submits one, `plugin_manager`; without it files are indexed
with no type). Query with `FileIndexQuery`. `record_asset_id` and
`paths_for_asset_id` store plugin-defined stable ids (`.pulsar/asset_ids.json` is
the durable manifest). Runtime component crates resolve asset GUIDs through it
(one resolver for packaged games is Pulsar-Native#1601).

Opening the index starts the project's watcher (`engine_fs::watchers`), which
keeps it current when files change outside the engine and publishes an
`AssetUpdated` for each changed file a plugin names a kind for
(`engine_fs::AssetChangeKind`: the animation plugin's animation files, the
materials plugin's images and materials).

## What moved out (p21)

`engine_fs`'s `editor` feature is gone, and with it every GPUI path from the
game library (`cargo tree -p pulsar_game -e normal -i gpui-ce` is empty):
`EngineFs`, `ProjectScanner`, `AssetOperations`, `templates`, `AssetIndex`,
`UserTypeRegistry` and the second watcher had no live caller and were deleted;
the thumbnail service is `ui_common::thumbnails`, the chat's file tools
`agent_chat_tools::fs_tools`. Traits, the trait index and roles are
`pulsar_traits`' ([TRAITS.md](TRAITS.md)).
