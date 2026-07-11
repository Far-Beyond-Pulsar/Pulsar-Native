pub mod auth;
pub mod protocol;
pub mod replication;
pub mod session;
pub mod transport;

pub mod prelude {
    pub use crate::auth::{AuthError, SessionAuth};
    pub use crate::protocol::{
        ChatMessage, CursorUpdate, FileChanged, FileChunk, FileManifest, JoinRequest,
        JoinedResponse, Kicked, LeaveRequest, LockDenied, LockGranted, P2pConnectionRequest,
        P2pConnectionResponse, PeerJoined, PeerLeft, PermissionDenied, PermissionGranted,
        ProtocolError, ReleaseLock, RequestFile, RequestLock, RequestPermission, SessionMessage,
        StateUpdate,
    };
    pub use crate::replication::Replicator;
    pub use crate::session::{
        FileChangeKind, ManifestEntry, ParticipantInfo, PeerProfile, Role, SessionInfo, SessionMode,
    };
    pub use crate::transport::{SessionChannel, SessionError};
}
