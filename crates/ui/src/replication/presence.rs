use crate::{avatar::Avatar, h_flex, v_flex, ActiveTheme, Icon, IconName, StyledExt, Sizable};
use gpui::{
    div, prelude::FluentBuilder, px, AnyElement, App, Div, InteractiveElement, IntoElement,
    ParentElement, RenderOnce, SharedString, Styled, Window,
};
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Default color for deserialization
fn default_color() -> gpui::Hsla {
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
    pub color: gpui::Hsla,

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
    pub fn new(peer_id: impl Into<String>, display_name: impl Into<String>, color: gpui::Hsla) -> Self {
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

/// A small badge/pill that shows user presence
#[derive(IntoElement)]
pub struct PresencePill {
    presence: UserPresence,
    show_name: bool,
    show_status: bool,
    size: PresencePillSize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PresencePillSize {
    Small,  // Just color dot
    Medium, // Initials
    Large,  // Initials + name
}

impl PresencePill {
    pub fn new(presence: UserPresence) -> Self {
        Self {
            presence,
            show_name: true,
            show_status: false,
            size: PresencePillSize::Medium,
        }
    }

    pub fn small(mut self) -> Self {
        self.size = PresencePillSize::Small;
        self.show_name = false;
        self
    }

    pub fn medium(mut self) -> Self {
        self.size = PresencePillSize::Medium;
        self.show_name = false;
        self
    }

    pub fn large(mut self) -> Self {
        self.size = PresencePillSize::Large;
        self.show_name = true;
        self
    }

    pub fn with_name(mut self, show: bool) -> Self {
        self.show_name = show;
        self
    }

    pub fn with_status(mut self, show: bool) -> Self {
        self.show_status = show;
        self
    }
}

impl RenderOnce for PresencePill {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = self.presence.color;
        let is_idle = self.presence.is_idle;

        match self.size {
            PresencePillSize::Small => {
                // Just a colored dot
                div()
                    .size_2()
                    .rounded_full()
                    .bg(color)
                    .when(is_idle, |this| this.opacity(0.4))
            }
            PresencePillSize::Medium => {
                // Colored circle with initials
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size_6()
                    .rounded_full()
                    .bg(color)
                    .text_color(gpui::white())
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .when(is_idle, |this| this.opacity(0.5))
                    .child(self.presence.initials())
            }
            PresencePillSize::Large => {
                // Full pill with initials + name
                h_flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_full()
                    .bg(color.opacity(0.15))
                    .border_1()
                    .border_color(color)
                    .when(is_idle, |this| this.opacity(0.6))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size_5()
                            .rounded_full()
                            .bg(color)
                            .text_color(gpui::white())
                            .text_xs()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(self.presence.initials()),
                    )
                    .when(self.show_name, |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().foreground)
                                .child(self.presence.short_name()),
                        )
                    })
                    .when(
                        self.show_status && self.presence.status.is_some(),
                        |this| {
                            this.child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(self.presence.status.as_ref().unwrap().clone()),
                            )
                        },
                    )
            }
        }
    }
}

/// Stack of presence pills (avatars that overlap)
#[derive(IntoElement)]
pub struct PresenceStack {
    presences: Vec<UserPresence>,
    max_visible: usize,
    show_count: bool,
    size: PresencePillSize,
}

impl PresenceStack {
    pub fn new(presences: Vec<UserPresence>) -> Self {
        Self {
            presences,
            max_visible: 3,
            show_count: true,
            size: PresencePillSize::Medium,
        }
    }

    pub fn max_visible(mut self, max: usize) -> Self {
        self.max_visible = max;
        self
    }

    pub fn show_count(mut self, show: bool) -> Self {
        self.show_count = show;
        self
    }

    pub fn small(mut self) -> Self {
        self.size = PresencePillSize::Small;
        self
    }
}

impl RenderOnce for PresenceStack {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let total = self.presences.len();
        let visible = self.presences.iter().take(self.max_visible);
        let overflow = total.saturating_sub(self.max_visible);

        h_flex()
            .items_center()
            .gap_0p5() // Small gap between avatars
            .children(visible.enumerate().map(|(i, presence)| {
                let pill = PresencePill::new(presence.clone());
                let pill = match self.size {
                    PresencePillSize::Small => pill.small(),
                    PresencePillSize::Medium => pill.medium(),
                    PresencePillSize::Large => pill.large(),
                };

                pill
            }))
            .when(self.show_count && overflow > 0, |this| {
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size_6()
                        .rounded_full()
                        .bg(cx.theme().muted)
                        .border_1()
                        .border_color(cx.theme().border)
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("+{}", overflow)),
                )
            })
    }
}

/// Indicator shown on tabs to indicate user presence
#[derive(IntoElement)]
pub struct TabPresenceIndicator {
    presences: Vec<UserPresence>,
    show_count: bool,
}

impl TabPresenceIndicator {
    pub fn new(presences: Vec<UserPresence>) -> Self {
        Self {
            presences,
            show_count: true,
        }
    }

    pub fn show_count(mut self, show: bool) -> Self {
        self.show_count = show;
        self
    }
}

impl RenderOnce for TabPresenceIndicator {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        if self.presences.is_empty() {
            return div().into_any_element();
        }

        // Use a subtle colored bar at the top of the tab
        let primary_user = &self.presences[0];
        let color = primary_user.color;

        // For now, just use the primary user's color
        // TODO: Support gradients when GPUI adds gradient support
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(2.0))
            .bg(color)
            .into_any_element()
    }
}

/// Indicator shown on input fields to show who's editing
#[derive(IntoElement)]
pub struct FieldPresenceIndicator {
    presence: UserPresence,
    is_locked: bool,
}

impl FieldPresenceIndicator {
    pub fn new(presence: UserPresence) -> Self {
        Self {
            presence,
            is_locked: false,
        }
    }

    pub fn locked(mut self, locked: bool) -> Self {
        self.is_locked = locked;
        self
    }
}

impl RenderOnce for FieldPresenceIndicator {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = self.presence.color;

        h_flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_0p5()
            .rounded(cx.theme().radius)
            .bg(color.opacity(0.1))
            .border_1()
            .border_color(color)
            .when(self.is_locked, |this| {
                this.child(Icon::new(IconName::Lock).size_3().text_color(color))
            })
            .child(
                div()
                    .size_4()
                    .rounded_full()
                    .bg(color)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .text_color(gpui::white())
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(self.presence.initials()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().foreground)
                    .child(if self.is_locked {
                        format!("{} is editing", self.presence.short_name())
                    } else {
                        self.presence.short_name()
                    }),
            )
    }
}

/// Cursor indicator for remote users in text fields
#[derive(IntoElement)]
pub struct RemoteCursor {
    presence: UserPresence,
    position: usize,
}

impl RemoteCursor {
    pub fn new(presence: UserPresence, position: usize) -> Self {
        Self { presence, position }
    }
}

impl RenderOnce for RemoteCursor {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let color = self.presence.color;

        div()
            .absolute()
            .w(px(2.0))
            .h(px(18.0)) // Approximate line height
            .bg(color)
            .rounded(px(1.0))
            // Position will be calculated based on text metrics
            .child(
                // User name label
                div()
                    .absolute()
                    .top(-px(18.0))
                    .left(px(2.0))
                    .px_1()
                    .py_0p5()
                    .rounded(px(2.0))
                    .bg(color)
                    .text_xs()
                    .text_color(gpui::white())
                    .whitespace_nowrap()
                    .child(self.presence.short_name()),
            )
    }
}
