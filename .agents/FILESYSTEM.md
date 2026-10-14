# Filesystem abstraction

The engine never calls `std::fs` directly. All filesystem access goes through
`engine_fs::virtual_fs`, which routes to a pluggable `FsProvider` backend.

## The provider trait

```rust
pub trait FsProvider: Send + Sync + 'static {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>>;
    fn write_file(&self, path: &Path, content: &[u8]) -> Result<()>;
    fn create_file(&self, path: &Path, content: &[u8]) -> Result<()>;
    fn delete_path(&self, path: &Path) -> Result<()>;
    fn rename(&self, from: &Path, to: &Path) -> Result<()>;
    fn list_dir(&self, path: &Path) -> Result<Vec<FsEntry>>;
    fn create_dir_all(&self, path: &Path) -> Result<()>;
    fn exists(&self, path: &Path) -> Result<bool>;
    fn metadata(&self, path: &Path) -> Result<FsMetadata>;
    fn manifest(&self, path: &Path) -> Result<Vec<ManifestEntry>>;
    fn is_remote(&self) -> bool { false }
    fn label(&self) -> &str { "Local" }
}
```

## Available providers

| Provider | Source | Use case |
|---|---|---|
| `LocalFsProvider` | `engine_fs::providers::local` | Local disk, optional sandbox root |
| `RemoteFsProvider` | `engine_fs::providers::remote` | `pulsar-studio` HTTP API |
| `P2pFsProvider` | `engine_fs::providers::p2p` | P2P multiplayer sessions |

`LocalFsProvider` supports `with_root(path)` which canonicalizes all paths
and rejects path traversal attacks (enforced by `canonicalize()` + prefix check).

## The virtual_fs singleton

```rust
static VIRTUAL_FS: OnceLock<Arc<RwLock<Arc<dyn FsProvider>>>> = OnceLock::new();
```

Free functions (call these from engine code and plugins):

```rust
engine_fs::virtual_fs::set_provider(provider);    // swap backend at runtime
engine_fs::virtual_fs::read_file(path) -> Vec<u8>;
engine_fs::virtual_fs::write_file(path, bytes);
engine_fs::virtual_fs::create_file(path, bytes);
engine_fs::virtual_fs::delete_path(path);
engine_fs::virtual_fs::rename(from, to);
engine_fs::virtual_fs::list_dir(path) -> Vec<FsEntry>;
engine_fs::virtual_fs::exists(path) -> bool;
engine_fs::virtual_fs::metadata(path) -> FsMetadata;
engine_fs::virtual_fs::manifest(path) -> Vec<ManifestEntry>;
engine_fs::virtual_fs::current_label() -> &str;
engine_fs::virtual_fs::is_remote() -> bool;
```

## Cloud path detection

Paths prefixed with `cloud+pulsar://` trigger remote provider routing:

```rust
engine_fs::virtual_fs::path_utils::is_cloud_path(path);
engine_fs::virtual_fs::path_utils::cloud_join(base, path);  // joins with /
engine_fs::virtual_fs::path_utils::normalize_path(path);     // backslash → forward
```

## FsEvent system

File changes are broadcast via `tokio::sync::broadcast`:

```rust
pub struct FsEvent {
    pub path: PathBuf,
    pub kind: FsChangeKind,   // Created | Modified | Deleted
    pub source: FsEventSource, // Local | Remote
}

engine_fs::virtual_fs::subscribe() -> tokio::sync::broadcast::Receiver<FsEvent>;
engine_fs::virtual_fs::emit(event);
engine_fs::virtual_fs::emit_remote(event);
```

## EngineFs (high-level manager)

`EngineFs` at `crates/core/engine_fs/src/engine_fs.rs` coordinates asset
operations (CRUD by type), maintains `AssetIndex` (in-memory, DashMap-backed),
scans projects via `ProjectScanner`, and manages `UserTypeRegistry` for
user-defined `.alias.json` types.

## Blueprint trait index

`engine_fs::BlueprintTraitIndex` owns the derived project index at
`.pulsar/blueprint_trait_index.json`. Blueprint source assets declare
`blueprint_metadata.implemented_traits` as an array of normalized,
project-relative trait asset paths (for example,
`types/traits/movable.trait.json`). Missing arrays mean no implemented traits.
The index scans current authored `graph_save.json` Blueprint files in class
folders and `*.blueprint.json` assets, and only records references to valid
`.trait.json` declarations under `types/traits/`.

Call `BlueprintTraitIndex::rebuild(project_root)` after editing or saving a
Blueprint. It scans through `virtual_fs`, sorts paths and trait references, and
atomically replaces the index when the provider supports atomic writes. Remote
and P2P providers currently lack atomic replacement, so they write the
rebuildable derived index directly. `load_or_rebuild` always refreshes from
source because providers do not share a reliable content freshness contract.
Use `blueprints_for_trait("types/traits/movable.trait.json")` to find
implementers, or `traits_for_blueprint(Path::new("classes/Player/graph_save.json"))`
to retrieve one Blueprint's traits.

```rust
pub struct EngineFs {
    pub project_root: PathBuf,
    pub asset_index: Arc<AssetIndex>,
    pub user_types: Arc<UserTypeRegistry>,
    pub operations: AssetOperations,
    pub scanner: ProjectScanner,
}
```

## How to read/write files

**Inside the engine** — always use `engine_fs::virtual_fs`:
```rust
let data = engine_fs::virtual_fs::read_file(&path)?;
engine_fs::virtual_fs::write_file(&path, &data)?;
```

**Inside a plugin** — use the same `engine_fs::virtual_fs` functions if linked
against `engine_fs`. For AI tools, use `PluginToolBridge` which provides a
sandboxed `FsContext::read_only(project_root)`.

Never call `std::fs::read`/`write`/`create_dir_all` — it bypasses the virtual
layer and will not work for remote/P2P projects. The engine's project scanner
(`ProjectScanner`) uses `notify` watchers and the `FsEvent` broadcast channel
to keep `AssetIndex` up to date with on-disk changes.

## Asset index

`AssetIndex` maintains:
- `assets_by_id: DashMap<u64, AssetInfo>`
- `assets_by_name: DashMap<String, Vec<AssetInfo>>`
- `assets_by_path: DashMap<PathBuf, AssetInfo>`
- `assets_by_file_type: DashMap<FileTypeId, Vec<AssetInfo>>`

Supports `register()`, `lookup_by_id()`, `lookup_by_name()`, `lookup_by_path()`,
`lookup_by_file_type()`, `lookup_by_category()`, `search_fuzzy()`, `clear()`.

## Persistent project file index

For local projects, `open_project_file_index(project_root)` returns the
process-shared disposable SQLite `ProjectFileIndex` at
`.pulsar/cache/files.db`; the loading splash populates and reconciles it in a
dedicated startup step. It stores project-relative path, name, extension,
registered file type, directory flag, size, and modified time. Local file
watcher notifications and synchronous local `FsEvent`s keep it current, with
rename operations preserving stable-ID rows. Consumers that need to avoid
opening or reconciling the database can use `project_file_index_for_path`
after the loading task has populated it. Query with `FileIndexQuery` to combine
type, extension, modified-time, name substring, folder, and size filters and
select a stable sort order. `record_asset_id` and `paths_for_asset_id` store
plugin-defined stable IDs and surface duplicate claims. The cache folder is
ignored by Git and can be deleted to rebuild.
The cache directory is created through `virtual_fs`; SQLite opens the database
by its local path because this service is only enabled for local providers.

Remote and P2P providers do not open a local SQLite cache. Asset IDs stored in
`ui_file_manager`, asset pickers, the widget library, and skeletal libraries
use this index for local file discovery. Asset payloads remain plugin-owned;
content hashing is not part of the index. `AssetIndex` remains the in-memory
typed view.
