use gpui::Hsla;
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn default_color() -> Hsla {
    gpui::hsla(0.5, 0.7, 0.6, 1.0)
}

/// Represents a user's presence in the collaborative session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPresence {
    /// Unique identifier for the user
    pub peer_id: String,

    /// Display name of the user
    pub display_name: String,

    /// User's assigned color for presence indicators
    #[serde(skip, default = "default_color")]
    pub color: Hsla,

    /// Current panel/tab the user is viewing
    pub current_panel: Option<String>,

    /// Element ID the user is currently editing
    pub editing_element: Option<String>,

    /// Cursor position (for text inputs)
    pub cursor_position: Option<usize>,

    /// Selection range (for text inputs)
    pub selection: Option<(usize, usize)>,

    /// Last activity timestamp
    pub last_activity: u64,

    /// Whether the user is idle (>5 minutes no activity)
    pub is_idle: bool,

    /// Custom status message
    pub status: Option<String>,
}

impl UserPresence {
    /// Create a new user presence
    pub fn new(peer_id: impl Into<String>, display_name: impl Into<String>, color: Hsla) -> Self {
        Self {
            peer_id: peer_id.into(),
            display_name: display_name.into(),
            color,
            current_panel: None,
            editing_element: None,
            cursor_position: None,
            selection: None,
            last_activity: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            is_idle: false,
            status: None,
        }
    }

    /// Update the user's activity timestamp
    pub fn touch(&mut self) {
        self.last_activity = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        self.is_idle = false;
    }

    /// Check if the user has been idle for more than the specified duration
    pub fn is_idle_for(&self, duration: Duration) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        now - self.last_activity > duration.as_secs()
    }

    /// Get a shortened display name (first name or initials)
    pub fn short_name(&self) -> String {
        self.display_name
            .split_whitespace()
            .next()
            .unwrap_or(&self.display_name)
            .to_string()
    }

    /// Get user initials (up to 2 characters)
    pub fn initials(&self) -> String {
        self.display_name
            .split_whitespace()
            .filter_map(|word| word.chars().next())
            .take(2)
            .collect::<String>()
            .to_uppercase()
    }
}
