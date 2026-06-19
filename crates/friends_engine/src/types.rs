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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GistFriendEntry {
    pub username: String,
    #[serde(default)]
    pub mutual: bool,
    #[serde(default)]
    pub home_server: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineFriendsFile {
    #[serde(default, deserialize_with = "deserialize_friends")]
    pub friends: Vec<GistFriendEntry>,
    #[serde(default)]
    pub home_servers: Vec<String>,
}

fn deserialize_friends<'de, D>(deserializer: D) -> Result<Vec<GistFriendEntry>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    let val = serde_json::Value::deserialize(deserializer)?;
    let arr = val.as_array().ok_or_else(|| D::Error::custom("expected array"))?;
    let entries: Vec<GistFriendEntry> = arr
        .iter()
        .filter_map(|v| {
            if let Some(obj) = v.as_object() {
                let username = obj.get("username")?.as_str()?.to_string();
                let mutual = obj.get("mutual").and_then(|m| m.as_bool()).unwrap_or(false);
                let home_server = obj.get("home_server").and_then(|h| h.as_str()).map(String::from);
                Some(GistFriendEntry { username, mutual, home_server })
            } else {
                let s = v.as_str()?;
                Some(GistFriendEntry {
                    username: s.to_string(),
                    mutual: false,
                    home_server: None,
                })
            }
        })
        .collect();
    Ok(entries)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FriendsNotificationType {
    FriendRequest,
    FriendRequestAccepted,
    FriendRequestDeclined,
    SessionInvite,
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
    NotFriends(String),
    RateLimited,
}
