use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RelationStatus {
    #[serde(rename = "pending_inbound")]
    PendingInbound,
    #[serde(rename = "pending_outbound")]
    PendingOutbound,
    #[serde(rename = "mutual")]
    Mutual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FriendInfo {
    pub username: String,
    pub pfp: String,
    pub relation_status: RelationStatus,
    pub current_project: Option<String>,
    pub last_seen: Option<String>,
    pub home_server: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FriendsList {
    pub friends: Vec<FriendInfo>,
}

impl FriendsList {
    pub fn empty() -> Self {
        Self { friends: Vec::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineFriendsFile {
    #[serde(default)]
    pub friends: Vec<String>,
    #[serde(default)]
    pub home_servers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FriendsNotificationType {
    FriendRequest,
    FriendRequestAccepted,
    FriendRequestDeclined,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FriendNotification {
    pub notification_type: FriendsNotificationType,
    pub from_username: String,
    pub from_home_server: Option<String>,
    pub target_username: String,
    pub target_home_server: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BroadcastStatus {
    pub online: bool,
    pub project: Option<String>,
    pub project_version: Option<String>,
}

#[derive(Debug)]
pub enum FriendsError {
    NotAuthenticated,
    Network(String),
    Api(String),
    NotFound,
    RateLimited,
}
