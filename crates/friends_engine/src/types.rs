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
