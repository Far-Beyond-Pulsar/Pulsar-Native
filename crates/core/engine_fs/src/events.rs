use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use tokio::sync::broadcast;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsEventSource {
    Local,
    Remote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsChangeKind {
    Created,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsEvent {
    pub path: PathBuf,
    pub kind: FsChangeKind,
    pub source: FsEventSource,
}

static EVENT_BUS: OnceLock<Arc<RwLock<broadcast::Sender<FsEvent>>>> = OnceLock::new();

fn bus() -> Arc<RwLock<broadcast::Sender<FsEvent>>> {
    EVENT_BUS
        .get_or_init(|| {
            let (tx, _) = broadcast::channel(256);
            Arc::new(RwLock::new(tx))
        })
        .clone()
}

pub fn subscribe() -> broadcast::Receiver<FsEvent> {
    bus().read().subscribe()
}

pub fn emit(path: impl Into<PathBuf>, kind: FsChangeKind) {
    emit_with_source(path, kind, FsEventSource::Local);
}

pub fn emit_remote(path: impl Into<PathBuf>, kind: FsChangeKind) {
    emit_with_source(path, kind, FsEventSource::Remote);
}

pub fn emit_with_source(path: impl Into<PathBuf>, kind: FsChangeKind, source: FsEventSource) {
    let path = path.into();
    #[cfg(feature = "editor")]
    crate::file_index::update_open_project_file_indexes(&path, &kind, &source);
    let _ = bus().read().send(FsEvent { path, kind, source });
}

/// Publish the conventional delete/create notifications for a move while
/// preserving indexed rows and stable IDs for local projects.
pub fn emit_move(from: impl Into<PathBuf>, to: impl Into<PathBuf>, source: FsEventSource) {
    let from = from.into();
    let to = to.into();
    #[cfg(feature = "editor")]
    crate::file_index::move_open_project_file_indexes(&from, &to, &source);
    let bus = bus();
    let sender = bus.read();
    let _ = sender.send(FsEvent {
        path: from,
        kind: FsChangeKind::Deleted,
        source: source.clone(),
    });
    let _ = sender.send(FsEvent {
        path: to,
        kind: FsChangeKind::Created,
        source,
    });
}
