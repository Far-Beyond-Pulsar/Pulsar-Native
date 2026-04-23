use crate::ReplicationMessage;
use gpui::{App, Global};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// All mutable session fields in one place — protected by a single lock.
struct SessionInner {
    our_peer_id: Option<String>,
    host_peer_id: Option<String>,
    is_active: bool,
    message_sender: Option<Box<dyn Fn(ReplicationMessage) + Send + Sync>>,
    permission_handler: Option<Box<dyn Fn(&str, &str) -> bool + Send + Sync>>,
    active_edits: HashMap<String, u64>,
}

impl SessionInner {
    fn new() -> Self {
        Self {
            our_peer_id: None,
            host_peer_id: None,
            is_active: false,
            message_sender: None,
            permission_handler: None,
            active_edits: HashMap::new(),
        }
    }
}

/// Global context for the current multiuser session.
///
/// All session-level information is behind a single `parking_lot::RwLock`
/// so that readers can hold the guard briefly without risking lock-ordering
/// issues or poison panics.
pub struct SessionContext {
    inner: Arc<RwLock<SessionInner>>,
}

impl Global for SessionContext {}

impl SessionContext {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(SessionInner::new())),
        }
    }

    pub fn set_our_peer_id(&self, peer_id: String) {
        self.inner.write().our_peer_id = Some(peer_id);
    }

    pub fn our_peer_id(&self) -> Option<String> {
        self.inner.read().our_peer_id.clone()
    }

    pub fn set_host_peer_id(&self, peer_id: String) {
        self.inner.write().host_peer_id = Some(peer_id);
    }

    pub fn host_peer_id(&self) -> Option<String> {
        self.inner.read().host_peer_id.clone()
    }

    pub fn are_we_host(&self) -> bool {
        let inner = self.inner.read();
        match (&inner.our_peer_id, &inner.host_peer_id) {
            (Some(our), Some(host)) => our == host,
            _ => false,
        }
    }

    pub fn start_session(&self, our_peer_id: String, host_peer_id: String) {
        tracing::info!("Started multiuser session (host: {})", host_peer_id);
        let mut inner = self.inner.write();
        inner.our_peer_id = Some(our_peer_id);
        inner.host_peer_id = Some(host_peer_id);
        inner.is_active = true;
    }

    pub fn end_session(&self) {
        tracing::info!("Ended multiuser session");
        let mut inner = self.inner.write();
        inner.is_active = false;
        inner.our_peer_id = None;
        inner.host_peer_id = None;
        inner.active_edits.clear();
    }

    pub fn is_active(&self) -> bool {
        self.inner.read().is_active
    }

    pub fn set_message_sender<F>(&self, sender: F)
    where
        F: Fn(ReplicationMessage) + Send + Sync + 'static,
    {
        self.inner.write().message_sender = Some(Box::new(sender));
    }

    pub fn send_message(&self, message: ReplicationMessage) {
        if let Some(sender) = self.inner.read().message_sender.as_ref() {
            sender(message);
        } else {
            tracing::warn!("Tried to send replication message but no sender configured");
        }
    }

    pub fn set_permission_handler<F>(&self, handler: F)
    where
        F: Fn(&str, &str) -> bool + Send + Sync + 'static,
    {
        self.inner.write().permission_handler = Some(Box::new(handler));
    }

    pub fn request_permission(&self, element_id: &str) -> bool {
        if !self.are_we_host() {
            return false;
        }
        let inner = self.inner.read();
        if let Some(handler) = inner.permission_handler.as_ref() {
            let our_id = inner.our_peer_id.clone().unwrap_or_default();
            handler(element_id, &our_id)
        } else {
            true
        }
    }

    pub fn start_editing(&self, element_id: String) {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.inner
            .write()
            .active_edits
            .insert(element_id, timestamp);
    }

    pub fn stop_editing(&self, element_id: &str) {
        self.inner.write().active_edits.remove(element_id);
    }

    pub fn is_editing(&self, element_id: &str) -> bool {
        self.inner.read().active_edits.contains_key(element_id)
    }

    pub fn active_edits(&self) -> Vec<String> {
        self.inner.read().active_edits.keys().cloned().collect()
    }

    pub fn init(cx: &mut App) {
        cx.set_global(Self::new());
    }

    pub fn global(cx: &App) -> Self {
        cx.global::<Self>().clone()
    }
}

impl Clone for SessionContext {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl Default for SessionContext {
    fn default() -> Self {
        Self::new()
    }
}
