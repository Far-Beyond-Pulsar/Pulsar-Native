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

## Plugin folders

A plugin can **provide files** into every project: content packs, templates,
default items such as the engine's gameplay-role traits. One that does
registers an `engine_fs::FileProvider` from its root crate (an origin-gated
contribution, category `fs.file_providers`), and every project then shows a
folder named after it under a virtual `plugins/` folder at its root
(`engine_fs::virtual_fs::mounts`). It is opt-in: a plugin that registers no
provider has no folder, and a disabled or absent plugin's folder is gone. The
engine provides its own files the same way, at `plugins/pulsar/` (its id and
trait namespace).

**The plugin owns its folder and fills it from any source.** When the folder is
first mounted, the engine calls the registration's `open` with a
`PluginFolder`: the capability to change that folder, which only `engine_fs`
can make and which goes to the owner alone. `open` returns the folder's
`FolderSource`, a read-only trait the plugin implements over anything it likes
(each call names the project asking, so a source can hold one set of files
for every project or its own per project): files embedded in its library (one
option, `embedded_files!`), data it generates or downloads, its own cache. At
runtime the plugin creates, writes and deletes whatever files and folders it
wants in its folder and reports each change with `PluginFolder::changed` (or
`changed_in` for one project), which emits the file event the file drawer and
the project watcher follow. Two convenience sources:

- `MemoryFolder`: in memory, the same for every project; starts with whatever
  the plugin puts in (`MemoryFolder::with_files(folder, embedded)`), changes
  through `write`, `create_dir_all` and `delete`, reports each change itself;
  gone at exit.
- `CacheFolder`: kept per project in `<project>/.pulsar/plugins/<id>/` (derived
  data version control ignores, like the rest of `.pulsar/`), written through
  the project's own provider (so a remote project keeps it too); its writes
  take the project and report themselves. What a plugin writes at runtime and
  wants next session goes here.

**A source never blocks.** Every `FolderSource` method answers from what the
source already holds: the VFS calls it from any thread, the UI's included. A
slow fetch (a download, a generator) runs on the plugin's own task and
announces what it produced through `PluginFolder::changed`. The VFS holds none
of its locks while it calls a source (the active layer is cloned out of its
lock; the project table is released before the folders are asked).

**A plugin folder is the plugin's content, not the project's.** A user who
wants to own or change one of its files copies it into the project (copying
out of a plugin folder is reading it).

```rust
fn open(folder: engine_fs::PluginFolder) -> std::sync::Arc<dyn engine_fs::FolderSource> {
    let files = std::sync::Arc::new(engine_fs::MemoryFolder::with_files(
        folder,
        engine_fs::embedded_files!("content": "traits/light_source.trait.json"),
    ));
    FILES.set(files.clone()).ok(); // to write into it later: FILES.get().unwrap().write(..)
    files
}
inventory::submit! {
    engine_fs::FileProvider {
        origin: pulsar_plugins::origin!(),
        open,
        in_game: engine_fs::InGame::Snapshot,
    }
}
```

**Nobody else writes there.** `MountedFs` is the layer every VFS call goes
through: paths under `plugins/` are answered by the owners' sources,
everything else goes to the active provider, so the folders look the same over
a local, remote or P2P project. **Writing, creating, appending, deleting or
renaming** anything under `plugins/` through the VFS fails with
`engine_fs::ReadOnlyMount { mount, path }` inside the `anyhow::Error`
(`virtual_fs::is_read_only(&error)`), whoever calls it: the editor, another
plugin, a script. Only the owner's capability changes a folder, never a name a
caller types. A project is mounted with `virtual_fs::mount_project(root)`:
`engine_state::set_project_path` (the editor), and the trait index's
`load`/`rebuild`/`live` (so a game's runtime and tests get them too).
`virtual_fs::mount_of(path)` names the folder a path is in (the file drawer's
read-only state).

An on-disk `plugins/` folder in a project is shadowed by the virtual one and
reported (`virtual_fs::mount_problems(root)`): logged when the project is
mounted, and shown as a banner above the file drawer's files.

**Packaging.** A shipped game sees the same files without depending on where
the editor's plugin got them (a download, a generator). The packager writes a
snapshot of every plugin folder whose provider says `InGame::Snapshot` (the
default) into the package (`virtual_fs::plugin_snapshot(project)`, at
`plugins/<id>/...`, after activating the project's plugin list), and a packaged
game serves each such folder from it, read-only, without calling the plugin
(`virtual_fs::serve_packaged_folders(true)`, which `pulsar_content::ContentRoot::install`
calls). A plugin that builds its folder in the game too says `InGame::Live`
(the engine's own folder does: its traits are embedded in every binary).

The capability prevents accidental writes; it is no sandbox: a native plugin
runs in-process.

How others do it: Unreal stacks `IPlatformFile` layers (the pak layer over the
physical one) and registers a mount point per plugin's content
(`FPackageName::RegisterMountPoint`), opt-in through the plugin's
`CanContainContent`; Unity shows packages under `Packages/<name>` and refuses
edits to immutable ones. Ours is Unreal's opt-in mount points over whichever
provider is active, with the plugin, not a directory, as the source of its
folder. Cost: a lookup over the mounted project roots per call (usually one).

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

Files that belong to the **machine**, not a project (the user's config and
credentials, the editor's logs, the Downloads folder, profiling databases,
libgit2's checkout), go through `engine_fs::LocalFsProvider` by name: the
physical layer, explicitly, so a remote or P2P project's provider never
captures them (Unreal's split between the pak-aware file API and the
platform's physical file for saved config and logs).

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
