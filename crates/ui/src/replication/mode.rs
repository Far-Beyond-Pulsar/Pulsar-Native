use serde::{Deserialize, Serialize};

/// Defines how a UI element's state should be replicated across users
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReplicationMode {
    /// No replication - element state is purely local
    ///
    /// Use for: User-specific preferences, local UI state, temporary values
    /// Example: Window size, scroll position, local drafts
    NoRep,

    /// Multi-edit mode - all users can edit simultaneously with conflict resolution
    ///
    /// Use for: Collaborative text editing, shared properties
    /// Example: Script parameters, entity properties, shared notes
    ///
    /// **Behavior:**
    /// - Changes are broadcasted immediately to all users
    /// - Last-write-wins conflict resolution (with optional CRDT support)
    /// - Shows presence indicators for active editors
    /// - Cursor positions are visible to others
    MultiEdit,

    /// Locked edit - only one user can edit at a time
    ///
    /// Use for: Critical fields, exclusive operations
    /// Example: Build settings, deployment configs, asset imports
    ///
    /// **Behavior:**
    /// - First user to focus gains exclusive edit lock
    /// - Lock is released on blur or explicit unlock
    /// - Other users see read-only state with lock indicator
    /// - Shows who currently holds the lock
    LockedEdit,

    /// Request-to-edit - users must request permission to edit
    ///
    /// Use for: Moderated workflows, review-required changes
    /// Example: Production settings, release configurations
    ///
    /// **Behavior:**
    /// - Users can request edit access
    /// - Host/admin must approve before editing is allowed
    /// - Shows pending requests to authorized users
    /// - Automatic timeout for abandoned requests
    RequestEdit,

    /// Broadcast-only - changes flow one direction (host → clients)
    ///
    /// Use for: Demonstrations, tutorials, view synchronization
    /// Example: Camera following, timeline scrubbing during reviews
    ///
    /// **Behavior:**
    /// - Only the host/presenter can make changes
    /// - All clients receive updates but cannot modify
    /// - Ideal for screen-sharing-like scenarios
    /// - Clients can "break away" to local mode
    BroadcastOnly,

    /// Follow mode - users can optionally follow another user's actions
    ///
    /// Use for: Learning, pair programming, code reviews
    /// Example: Following another user's camera, viewport, or selections
    ///
    /// **Behavior:**
    /// - Users choose whose state to follow
    /// - Can switch between users or go independent
    /// - Following is one-way (no reverse sync)
    /// - Visual indicator shows who you're following
    Follow,

    /// Queue mode - changes are queued and applied in order
    ///
    /// Use for: Sequential workflows, ordered operations
    /// Example: Animation timeline, event queue, batch operations
    ///
    /// **Behavior:**
    /// - Changes are timestamped and queued
    /// - Applied in strict chronological order
    /// - Prevents race conditions
    /// - Shows queue position and pending changes
    QueuedEdit,

    /// Partition mode - field is split into user-specific sections
    ///
    /// Use for: Per-user settings within shared context
    /// Example: User-specific layers, personal annotations
    ///
    /// **Behavior:**
    /// - Each user has their own partition of the data
    /// - No conflicts possible
    /// - All partitions visible to all users
    /// - Can merge partitions when needed
    PartitionedEdit,
}

impl Default for ReplicationMode {
    fn default() -> Self {
        Self::NoRep
    }
}

impl ReplicationMode {
    /// Returns true if this mode allows concurrent editing by multiple users
    pub fn is_collaborative(&self) -> bool {
        matches!(
            self,
            ReplicationMode::MultiEdit
                | ReplicationMode::Follow
                | ReplicationMode::QueuedEdit
                | ReplicationMode::PartitionedEdit
        )
    }

    /// Returns true if this mode requires exclusive access
    pub fn is_exclusive(&self) -> bool {
        matches!(
            self,
            ReplicationMode::LockedEdit | ReplicationMode::RequestEdit | ReplicationMode::BroadcastOnly
        )
    }

    /// Returns true if this mode requires authorization/permissions
    pub fn requires_permission(&self) -> bool {
        matches!(self, ReplicationMode::RequestEdit)
    }

    /// Returns true if this mode shows presence indicators
    pub fn shows_presence(&self) -> bool {
        !matches!(self, ReplicationMode::NoRep | ReplicationMode::BroadcastOnly)
    }

    /// Returns true if changes should be immediately synchronized
    pub fn is_realtime(&self) -> bool {
        matches!(
            self,
            ReplicationMode::MultiEdit
                | ReplicationMode::BroadcastOnly
                | ReplicationMode::Follow
        )
    }

    /// Returns a human-readable description of the mode
    pub fn description(&self) -> &'static str {
        match self {
            ReplicationMode::NoRep => "Local only - not shared with other users",
            ReplicationMode::MultiEdit => "Collaborative - all users can edit simultaneously",
            ReplicationMode::LockedEdit => "Exclusive - only one user can edit at a time",
            ReplicationMode::RequestEdit => "Moderated - requires approval to edit",
            ReplicationMode::BroadcastOnly => "Presentation - host controls, clients watch",
            ReplicationMode::Follow => "Follow mode - sync with another user's view",
            ReplicationMode::QueuedEdit => "Sequential - changes applied in order",
            ReplicationMode::PartitionedEdit => "Partitioned - each user has their own section",
        }
    }

    /// Returns the icon name to represent this mode
    pub fn icon(&self) -> &'static str {
        match self {
            ReplicationMode::NoRep => "user",
            ReplicationMode::MultiEdit => "users",
            ReplicationMode::LockedEdit => "lock",
            ReplicationMode::RequestEdit => "hand",
            ReplicationMode::BroadcastOnly => "radio",
            ReplicationMode::Follow => "eye",
            ReplicationMode::QueuedEdit => "list-ordered",
            ReplicationMode::PartitionedEdit => "layers",
        }
    }
}

/// Configuration for replication behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationConfig {
    /// The replication mode
    pub mode: ReplicationMode,

    /// Whether to show presence indicators for this element
    pub show_presence: bool,

    /// Whether to show cursors for remote users (text inputs only)
    pub show_cursors: bool,

    /// Debounce time in milliseconds before syncing changes
    /// (0 = immediate, higher = less network traffic)
    pub debounce_ms: u32,

    /// Maximum number of users that can edit simultaneously
    /// (None = unlimited)
    pub max_concurrent_editors: Option<usize>,

    /// Whether to track and show edit history
    pub track_history: bool,

    /// Custom conflict resolution strategy
    /// (None = use default for mode)
    pub conflict_strategy: Option<ConflictStrategy>,
}

impl Default for ReplicationConfig {
    fn default() -> Self {
        Self {
            mode: ReplicationMode::NoRep,
            show_presence: true,
            show_cursors: true,
            debounce_ms: 100,
            max_concurrent_editors: None,
            track_history: false,
            conflict_strategy: None,
        }
    }
}

impl ReplicationConfig {
    /// Create a new config with the specified mode
    pub fn new(mode: ReplicationMode) -> Self {
        Self {
            mode,
            ..Default::default()
        }
    }

    /// Set whether to show presence indicators
    pub fn with_presence(mut self, show: bool) -> Self {
        self.show_presence = show;
        self
    }

    /// Set whether to show cursors
    pub fn with_cursors(mut self, show: bool) -> Self {
        self.show_cursors = show;
        self
    }

    /// Set debounce time in milliseconds
    pub fn with_debounce(mut self, ms: u32) -> Self {
        self.debounce_ms = ms;
        self
    }

    /// Set maximum concurrent editors
    pub fn with_max_editors(mut self, max: usize) -> Self {
        self.max_concurrent_editors = Some(max);
        self
    }

    /// Enable history tracking
    pub fn with_history(mut self) -> Self {
        self.track_history = true;
        self
    }

    /// Set conflict resolution strategy
    pub fn with_conflict_strategy(mut self, strategy: ConflictStrategy) -> Self {
        self.conflict_strategy = Some(strategy);
        self
    }
}

/// Strategy for resolving conflicts when multiple users edit simultaneously
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictStrategy {
    /// Last write wins - most recent change takes precedence
    LastWriteWins,

    /// First write wins - initial change is preserved
    FirstWriteWins,

    /// Manual resolution required - show conflict UI
    Manual,

    /// Operational transformation (for text)
    OperationalTransform,

    /// CRDT-based merge
    CRDT,
}

impl Default for ConflictStrategy {
    fn default() -> Self {
        Self::LastWriteWins
    }
}
