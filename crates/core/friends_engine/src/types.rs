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
        Self {
            friends: Vec::new(),
        }
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
    let arr = val
        .as_array()
        .ok_or_else(|| D::Error::custom("expected array"))?;
    let entries: Vec<GistFriendEntry> = arr
        .iter()
        .filter_map(|v| {
            if let Some(obj) = v.as_object() {
                let username = obj.get("username")?.as_str()?.to_string();
                let mutual = obj.get("mutual").and_then(|m| m.as_bool()).unwrap_or(false);
                let home_server = obj
                    .get("home_server")
                    .and_then(|h| h.as_str())
                    .map(String::from);
                Some(GistFriendEntry {
                    username,
                    mutual,
                    home_server,
                })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relation_status_uses_snake_case_wire_names() {
        assert_eq!(
            serde_json::to_string(&RelationStatus::PendingInbound).unwrap(),
            "\"pending_inbound\""
        );
        assert_eq!(
            serde_json::from_str::<RelationStatus>("\"pending_outbound\"").unwrap(),
            RelationStatus::PendingOutbound
        );
        assert_eq!(
            serde_json::from_str::<RelationStatus>("\"mutual\"").unwrap(),
            RelationStatus::Mutual
        );
    }

    #[test]
    fn friends_file_accepts_legacy_string_entries_and_objects() {
        let file: EngineFriendsFile = serde_json::from_str(
            r#"{"friends":["old-style",{"username":"new","mutual":true,"home_server":"https://r.test"}],"home_servers":["https://me.test"]}"#,
        )
        .unwrap();
        assert_eq!(
            file.friends,
            vec![
                GistFriendEntry {
                    username: "old-style".into(),
                    mutual: false,
                    home_server: None
                },
                GistFriendEntry {
                    username: "new".into(),
                    mutual: true,
                    home_server: Some("https://r.test".into())
                },
            ]
        );
        assert_eq!(file.home_servers, vec!["https://me.test".to_string()]);
    }

    #[test]
    fn friends_file_drops_malformed_entries() {
        let file: EngineFriendsFile =
            serde_json::from_str(r#"{"friends":[1,null,{"mutual":true},{"username":"ok"}]}"#)
                .unwrap();
        assert_eq!(file.friends.len(), 1);
        assert_eq!(file.friends[0].username, "ok");
        assert!(file.home_servers.is_empty());
    }

    #[test]
    fn friends_file_rejects_non_array_friends_but_defaults_when_absent() {
        assert!(serde_json::from_str::<EngineFriendsFile>(r#"{"friends":{}}"#).is_err());
        let file: EngineFriendsFile = serde_json::from_str("{}").unwrap();
        assert!(file.friends.is_empty());
    }

    #[test]
    fn gist_friend_entry_defaults_optional_fields() {
        let entry: GistFriendEntry = serde_json::from_str(r#"{"username":"x"}"#).unwrap();
        assert!(!entry.mutual);
        assert_eq!(entry.home_server, None);
    }

    #[test]
    fn friends_list_empty_and_notification_round_trip() {
        assert!(FriendsList::empty().friends.is_empty());

        let note = FriendNotification {
            notification_type: FriendsNotificationType::SessionInvite,
            from_username: "a".into(),
            from_home_server: None,
            target_username: "b".into(),
            target_home_server: "https://r.test".into(),
        };
        let back: FriendNotification =
            serde_json::from_str(&serde_json::to_string(&note).unwrap()).unwrap();
        assert!(matches!(
            back.notification_type,
            FriendsNotificationType::SessionInvite
        ));
        assert_eq!(back.target_home_server, "https://r.test");
    }
}
