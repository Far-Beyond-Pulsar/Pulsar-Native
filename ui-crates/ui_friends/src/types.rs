use friends_engine::RelationStatus;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FriendTab {
    Online,
    Pending,
    All,
}

#[derive(Clone)]
pub struct FriendEntry {
    pub username: String,
    pub pfp_url: String,
    pub relation_status: RelationStatus,
    pub current_project: Option<String>,
    pub current_project_version: Option<String>,
    pub online: bool,
    pub last_seen: Option<String>,
    pub is_self: bool,
}

pub enum AddFriendState {
    Idle,
    Sending,
    Success,
    SelfFriended,
    Error(String),
}
