#![cfg(feature = "p2p")]

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;

use engine_fs::providers::p2p::P2pFsProvider;
use engine_fs::providers::FsProvider;
use pulsar_multiplayer_core::protocol::{FileChunk, FileManifest, RequestFile, SessionMessage};
use pulsar_multiplayer_core::session::ManifestEntry;
use pulsar_multiplayer_core::transport::{SessionChannel, SessionError};

struct MockChannel {
    sent: Arc<Mutex<Vec<SessionMessage>>>,
    responses: Arc<Mutex<VecDeque<SessionMessage>>>,
}

#[async_trait]
impl SessionChannel for MockChannel {
    async fn send(&self, msg: SessionMessage) -> Result<(), SessionError> {
        self.sent.lock().unwrap().push(msg);
        Ok(())
    }

    async fn recv(&self) -> Result<SessionMessage, SessionError> {
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or(SessionError::ConnectionClosed)
    }

    fn is_connected(&self) -> bool {
        true
    }

    async fn close(&self) -> Result<(), SessionError> {
        Ok(())
    }
}

#[tokio::test]
async fn test_read_file_sends_request() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let responses = Arc::new(Mutex::new(VecDeque::new()));
    responses
        .lock()
        .unwrap()
        .push_back(SessionMessage::FileChunk(FileChunk {
            path: "hello.txt".into(),
            offset: 0,
            data: b"hello".to_vec(),
            is_last: true,
        }));

    let channel = MockChannel {
        sent: sent.clone(),
        responses: responses.clone(),
    };
    let provider = P2pFsProvider::new(Arc::new(channel), "test-proj".into());

    let result = provider.read_file(Path::new("hello.txt"));
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), b"hello");

    let sent_msgs = sent.lock().unwrap();
    assert_eq!(sent_msgs.len(), 1);
    assert!(
        matches!(&sent_msgs[0], SessionMessage::RequestFile(RequestFile { path, .. }) if path == "hello.txt")
    );
}

#[tokio::test]
async fn test_read_file_assembles_chunks() {
    let responses = Arc::new(Mutex::new(VecDeque::new()));
    responses
        .lock()
        .unwrap()
        .push_back(SessionMessage::FileChunk(FileChunk {
            path: "chunked.txt".into(),
            offset: 0,
            data: b"hello ".to_vec(),
            is_last: false,
        }));
    responses
        .lock()
        .unwrap()
        .push_back(SessionMessage::FileChunk(FileChunk {
            path: "chunked.txt".into(),
            offset: 6,
            data: b"world".to_vec(),
            is_last: true,
        }));

    let channel = MockChannel {
        sent: Arc::new(Mutex::new(Vec::new())),
        responses,
    };
    let provider = P2pFsProvider::new(Arc::new(channel), "test-proj".into());

    let result = provider.read_file(Path::new("chunked.txt"));
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), b"hello world");
}

#[tokio::test]
async fn test_list_dir_filters_manifest() {
    let responses = Arc::new(Mutex::new(VecDeque::new()));
    responses
        .lock()
        .unwrap()
        .push_back(SessionMessage::FileManifest(FileManifest {
            entries: vec![
                ManifestEntry {
                    path: "src/main.rs".into(),
                    is_dir: false,
                    size: 100,
                    modified: Some(1000),
                },
                ManifestEntry {
                    path: "src/lib.rs".into(),
                    is_dir: false,
                    size: 200,
                    modified: Some(1001),
                },
                ManifestEntry {
                    path: "src/sub/mod.rs".into(),
                    is_dir: false,
                    size: 50,
                    modified: Some(1002),
                },
                ManifestEntry {
                    path: "README.md".into(),
                    is_dir: false,
                    size: 30,
                    modified: Some(999),
                },
            ],
        }));

    let channel = MockChannel {
        sent: Arc::new(Mutex::new(Vec::new())),
        responses,
    };
    let provider = P2pFsProvider::new(Arc::new(channel), "test-proj".into());

    let result = provider.list_dir(Path::new("src"));
    assert!(result.is_ok());
    let entries = result.unwrap();
    assert_eq!(
        entries.len(),
        3,
        "expected main.rs, lib.rs, sub; got {:?}",
        entries.iter().map(|e| &e.name).collect::<Vec<_>>()
    );
    assert!(entries.iter().any(|e| e.name == "main.rs"));
    assert!(entries.iter().any(|e| e.name == "lib.rs"));
    assert!(entries.iter().any(|e| e.name == "sub" && e.is_dir));
}

#[tokio::test]
async fn test_list_dir_root() {
    let responses = Arc::new(Mutex::new(VecDeque::new()));
    responses
        .lock()
        .unwrap()
        .push_back(SessionMessage::FileManifest(FileManifest {
            entries: vec![
                ManifestEntry {
                    path: "src/main.rs".into(),
                    is_dir: false,
                    size: 100,
                    modified: None,
                },
                ManifestEntry {
                    path: "README.md".into(),
                    is_dir: false,
                    size: 30,
                    modified: None,
                },
            ],
        }));

    let channel = MockChannel {
        sent: Arc::new(Mutex::new(Vec::new())),
        responses,
    };
    let provider = P2pFsProvider::new(Arc::new(channel), "test-proj".into());

    let result = provider.list_dir(Path::new(""));
    assert!(result.is_ok());
    let entries = result.unwrap();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().any(|e| e.name == "README.md"));
    assert!(entries.iter().any(|e| e.name.starts_with("src")));
}

fn make_exist_provider(entries: Vec<ManifestEntry>) -> P2pFsProvider {
    let responses = Arc::new(Mutex::new(VecDeque::new()));
    responses
        .lock()
        .unwrap()
        .push_back(SessionMessage::FileManifest(FileManifest { entries }));
    let channel = MockChannel {
        sent: Arc::new(Mutex::new(Vec::new())),
        responses,
    };
    P2pFsProvider::new(Arc::new(channel), "test-proj".into())
}

#[tokio::test]
async fn test_exists_returns_true_when_found() {
    let provider = make_exist_provider(vec![ManifestEntry {
        path: "README.md".into(),
        is_dir: false,
        size: 30,
        modified: None,
    }]);
    assert!(provider.exists(Path::new("README.md")).unwrap());
}

#[tokio::test]
async fn test_exists_returns_false_when_missing() {
    let provider = make_exist_provider(vec![ManifestEntry {
        path: "README.md".into(),
        is_dir: false,
        size: 30,
        modified: None,
    }]);
    assert!(!provider.exists(Path::new("missing.txt")).unwrap());
}

#[tokio::test]
async fn test_write_file_sends_chunk() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let channel = MockChannel {
        sent: sent.clone(),
        responses: Arc::new(Mutex::new(VecDeque::new())),
    };
    let provider = P2pFsProvider::new(Arc::new(channel), "test-proj".into());

    provider
        .write_file(Path::new("out.txt"), b"content")
        .unwrap();

    let sent_msgs = sent.lock().unwrap();
    assert_eq!(sent_msgs.len(), 1);
    match &sent_msgs[0] {
        SessionMessage::FileChunk(chunk) => {
            assert_eq!(chunk.path, "out.txt");
            assert_eq!(chunk.data, b"content");
            assert!(chunk.is_last);
        }
        _ => panic!("Expected FileChunk"),
    }
}

#[tokio::test]
async fn test_manifest_returns_all_entries() {
    let responses = Arc::new(Mutex::new(VecDeque::new()));
    responses
        .lock()
        .unwrap()
        .push_back(SessionMessage::FileManifest(FileManifest {
            entries: vec![
                ManifestEntry {
                    path: "a.rs".into(),
                    is_dir: false,
                    size: 10,
                    modified: None,
                },
                ManifestEntry {
                    path: "b.rs".into(),
                    is_dir: false,
                    size: 20,
                    modified: None,
                },
            ],
        }));

    let channel = MockChannel {
        sent: Arc::new(Mutex::new(Vec::new())),
        responses,
    };
    let provider = P2pFsProvider::new(Arc::new(channel), "test-proj".into());

    let manifest = provider.manifest(Path::new("")).unwrap();
    assert_eq!(manifest.len(), 2);
}

#[tokio::test]
async fn test_metadata_returns_entry_info() {
    let responses = Arc::new(Mutex::new(VecDeque::new()));
    responses
        .lock()
        .unwrap()
        .push_back(SessionMessage::FileManifest(FileManifest {
            entries: vec![ManifestEntry {
                path: "data.bin".into(),
                is_dir: false,
                size: 512,
                modified: Some(42),
            }],
        }));

    let channel = MockChannel {
        sent: Arc::new(Mutex::new(Vec::new())),
        responses,
    };
    let provider = P2pFsProvider::new(Arc::new(channel), "test-proj".into());

    let meta = provider.metadata(Path::new("data.bin")).unwrap();
    assert!(!meta.is_dir);
    assert_eq!(meta.size, 512);
    assert_eq!(meta.modified, Some(42));
}

#[tokio::test]
async fn test_is_remote_returns_true() {
    let channel = MockChannel {
        sent: Arc::new(Mutex::new(Vec::new())),
        responses: Arc::new(Mutex::new(VecDeque::new())),
    };
    let provider = P2pFsProvider::new(Arc::new(channel), "test-proj".into());
    assert!(provider.is_remote());
    assert_eq!(provider.label(), "P2P");
}
