//! The Pulsar filesystem library: every file the engine, the editor and the
//! plugins read or write goes through it.
//!
//! - [`virtual_fs`]: the free functions everything calls (`read_file`,
//!   `write_file`, `open_read`, `manifest`, ...), routed to the active
//!   [`FsProvider`] (local disk, a remote `pulsar-studio` project, a
//!   peer-to-peer session) with the plugin folders composed over it
//!   ([`virtual_fs::mounts`]: `plugins/<owner>/` in every project for each
//!   plugin that provides files, and the engine's `plugins/pulsar/`; only
//!   their owner changes them).
//! - [`providers`]: the provider trait and its implementations.
//! - [`events`]: change notifications for what the editor wrote.
//! - [`file_index`]: the persistent, disposable SQLite index of a local
//!   project's files and stable asset ids, and [`watchers`], which keeps it
//!   current and publishes the asset updates plugins ask for.
//! - [`import_options`]: per-asset import settings.
//!
//! `std::fs` is a disallowed method across the workspace (the root
//! `clippy.toml`); only the providers here, build scripts and tests touch the
//! disk directly (`architecture/std-fs-allowed.txt`, shrink-only).
//!
//! Traits and gameplay roles are `pulsar_traits`', not this crate's.

pub mod events;
pub mod file_index;
pub mod import_options;
pub mod providers;
pub mod virtual_fs;
pub mod watchers;

pub use file_index::{
    move_open_project_file_indexes, open_project_file_index, project_file_index_for_path,
    update_open_project_file_indexes, AssetDep, AssetDependent, FileIndexQuery, FileIndexSort,
    FileTypeClassifier, IndexedFile, ProjectFileIndex,
};
#[cfg(feature = "p2p")]
pub use providers::P2pFsProvider;
pub use providers::{
    FsEntry, FsMetadata, FsProvider, FsWriter, LocalFsProvider, ManifestEntry, ReadSeek,
};
#[cfg(feature = "remote")]
pub use providers::{RemoteConfig, RemoteFsProvider};
pub use virtual_fs::{
    cloud_join, is_cloud_path, CacheFolder, EmbeddedFile, FileProvider, FolderSource, InGame,
    MemoryFolder, PluginFolder, ReadOnlyMount,
};
pub use watchers::AssetChangeKind;

pub use events::{emit, subscribe, FsChangeKind, FsEvent};
