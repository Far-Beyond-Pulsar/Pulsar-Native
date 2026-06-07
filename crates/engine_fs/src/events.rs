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
    let _ = bus().read().send(FsEvent {
        path: path.into(),
        kind,
        source,
    });
}
