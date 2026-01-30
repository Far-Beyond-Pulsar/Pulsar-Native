use super::{ReplicationMessage, ReplicationRegistry, UserPresence};
use gpui::{App, AppContext, Global};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Global context for the current multiuser session
///
/// This tracks session-level information like who's the host,
/// what our peer ID is, and handles sending/receiving messages.
pub struct SessionContext {
    /// Our peer ID in the session
    our_peer_id: Arc<RwLock<Option<String>>>,

    /// The host's peer ID (usually the first peer)
    host_peer_id: Arc<RwLock<Option<String>>>,

    /// Whether we're currently in a multiuser session
    is_active: Arc<RwLock<bool>>,

    /// Callback to send replication messages to the network
    message_sender: Arc<RwLock<Option<Box<dyn Fn(ReplicationMessage) + Send + Sync>>>>,

    /// Callback to handle permission requests (for hosts/admins)
    permission_handler: Arc<RwLock<Option<Box<dyn Fn(&str, &str) -> bool + Send + Sync>>>>,

    /// Elements we're currently editing (element_id -> timestamp)
    active_edits: Arc<RwLock<HashMap<String, u64>>>,
}

impl Global for SessionContext {}

impl SessionContext {
    /// Create a new session context
    pub fn new() -> Self {
        Self {
            our_peer_id: Arc::new(RwLock::new(None)),
            host_peer_id: Arc::new(RwLock::new(None)),
            is_active: Arc::new(RwLock::new(false)),
            message_sender: Arc::new(RwLock::new(None)),
            permission_handler: Arc::new(RwLock::new(None)),
            active_edits: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Initialize the global session context
    pub fn init(cx: &mut App) {
        cx.set_global(Self::new());
    }

    /// Get the global session context
    pub fn global(cx: &App) -> Self {
        cx.global::<Self>().clone()
    }

    /// Set our peer ID
    pub fn set_our_peer_id(&self, peer_id: String) {
        let mut our_id = self.our_peer_id.write().unwrap();
        *our_id = Some(peer_id);
    }

    /// Get our peer ID
    pub fn our_peer_id(&self) -> Option<String> {
        self.our_peer_id.read().unwrap().clone()
    }

    /// Set the host peer ID
    pub fn set_host_peer_id(&self, peer_id: String) {
        let mut host_id = self.host_peer_id.write().unwrap();
        *host_id = Some(peer_id);
    }

    /// Get the host peer ID
    pub fn host_peer_id(&self) -> Option<String> {
        self.host_peer_id.read().unwrap().clone()
    }

    /// Check if we're the host
    pub fn are_we_host(&self) -> bool {
        let our_id = self.our_peer_id.read().unwrap();
        let host_id = self.host_peer_id.read().unwrap();

        if let (Some(our), Some(host)) = (our_id.as_ref(), host_id.as_ref()) {
            our == host
        } else {
            false
        }
    }

    /// Start a multiuser session
    pub fn start_session(&self, our_peer_id: String, host_peer_id: String) {
        self.set_our_peer_id(our_peer_id);
        self.set_host_peer_id(host_peer_id.clone());

        let mut active = self.is_active.write().unwrap();
        *active = true;

        tracing::info!("Started multiuser session (host: {})", host_peer_id);
    }

    /// End the multiuser session
    pub fn end_session(&self) {
        let mut active = self.is_active.write().unwrap();
        *active = false;

        let mut our_id = self.our_peer_id.write().unwrap();
        *our_id = None;

        let mut host_id = self.host_peer_id.write().unwrap();
        *host_id = None;

        let mut edits = self.active_edits.write().unwrap();
        edits.clear();

        tracing::info!("Ended multiuser session");
    }

    /// Check if we're in an active session
    pub fn is_active(&self) -> bool {
        *self.is_active.read().unwrap()
    }

    /// Set the message sender callback
    pub fn set_message_sender<F>(&self, sender: F)
    where
        F: Fn(ReplicationMessage) + Send + Sync + 'static,
    {
        let mut msg_sender = self.message_sender.write().unwrap();
        *msg_sender = Some(Box::new(sender));
    }

    /// Send a replication message
    pub fn send_message(&self, message: ReplicationMessage) {
        if let Some(sender) = self.message_sender.read().unwrap().as_ref() {
            sender(message);
        } else {
            tracing::warn!("Tried to send replication message but no sender configured");
        }
    }

    /// Set the permission handler (for RequestEdit mode)
    pub fn set_permission_handler<F>(&self, handler: F)
    where
        F: Fn(&str, &str) -> bool + Send + Sync + 'static,
    {
        let mut perm_handler = self.permission_handler.write().unwrap();
        *perm_handler = Some(Box::new(handler));
    }

    /// Request permission to edit an element
    pub fn request_permission(&self, element_id: &str) -> bool {
        // Only host/admin can grant permissions
        if !self.are_we_host() {
            tracing::debug!("Requesting edit permission for {} from host", element_id);
            return false;
        }

        // If we're the host, check with the handler
        if let Some(handler) = self.permission_handler.read().unwrap().as_ref() {
            let our_id = self.our_peer_id().unwrap_or_default();
            handler(element_id, &our_id)
        } else {
            // Default: auto-grant if we're the host
            true
        }
    }

    /// Track that we started editing an element
    pub fn start_editing(&self, element_id: String) {
        let mut edits = self.active_edits.write().unwrap();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        edits.insert(element_id, timestamp);
    }

    /// Track that we stopped editing an element
    pub fn stop_editing(&self, element_id: &str) {
        let mut edits = self.active_edits.write().unwrap();
        edits.remove(element_id);
    }

    /// Check if we're currently editing an element
    pub fn is_editing(&self, element_id: &str) -> bool {
        let edits = self.active_edits.read().unwrap();
        edits.contains_key(element_id)
    }

    /// Get all elements we're currently editing
    pub fn active_edits(&self) -> Vec<String> {
        let edits = self.active_edits.read().unwrap();
        edits.keys().cloned().collect()
    }
}

impl Clone for SessionContext {
    fn clone(&self) -> Self {
        Self {
            our_peer_id: Arc::clone(&self.our_peer_id),
            host_peer_id: Arc::clone(&self.host_peer_id),
            is_active: Arc::clone(&self.is_active),
            message_sender: Arc::clone(&self.message_sender),
            permission_handler: Arc::clone(&self.permission_handler),
            active_edits: Arc::clone(&self.active_edits),
        }
    }
}

impl Default for SessionContext {
    fn default() -> Self {
        Self::new()
    }
}
