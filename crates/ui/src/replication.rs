//! Multi-user editing and state replication system
//!
//! Re-exports the canonical [`pulsar_replication`] crate and provides
//! UI-specific presence components and extension traits.

pub use pulsar_replication::*;

// ── UI-specific presence components ─────────────────────────────────────────

use crate::{h_flex, ActiveTheme, Icon, IconName, Sizable, StyledExt};
use gpui::{
    div, prelude::FluentBuilder, px, App, AnyElement, IntoElement, ParentElement, RenderOnce,
    Styled, Window,
};
use serde_json::{json, Value};

// ── UI-specific extension traits ─────────────────────────────────────────────

use crate::input::{InputState, RopeExt};

/// Extension trait for [`InputState`] to add replication support.
///
/// # Usage
///
/// ```ignore
/// use ui::replication::InputStateReplicationExt;
///
/// let input = cx.new(|cx| InputState::new(window, cx));
/// input.enable_replication(ReplicationMode::MultiEdit, cx);
/// input.update(cx, |state, cx| {
///     state.set_value("new value".to_string(), window, cx);
///     state.sync_if_replicated(cx);
/// });
/// ```
pub trait InputStateReplicationExt {
    fn enable_replication(&self, mode: ReplicationMode, cx: &mut App);
    fn sync_if_replicated(&self, cx: &mut App);
    fn replication_mode(&self, cx: &App) -> Option<ReplicationMode>;
    fn can_edit_replicated(&self, cx: &App) -> bool;
    fn apply_remote_state(&self, state: Value, window: &mut Window, cx: &mut App) -> Result<(), String>;
}

impl InputStateReplicationExt for gpui::Entity<InputState> {
    fn enable_replication(&self, mode: ReplicationMode, cx: &mut App) {
        let element_id = format!("input_{}", self.entity_id());
        let config = ReplicationConfig::new(mode)
            .with_debounce(100)
            .with_presence(true)
            .with_cursors(true);

        let registry = ReplicationRegistry::global(cx);
        registry.register_element(element_id, config);

        tracing::debug!("Enabled replication for input {:?}", self.entity_id());
    }

    fn sync_if_replicated(&self, cx: &mut App) {
        let element_id = format!("input_{}", self.entity_id());
        let registry = ReplicationRegistry::global(cx);

        if let Some(_elem_state) = registry.get_element_state(&element_id) {
            let session = SessionContext::global(cx);

            let text_rope = self.read(cx).text();
            let cursor_pos = self.read(cx).cursor();

            let state = json!({
                "text": text_rope.to_string(),
                "cursor": cursor_pos,
            });

            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;

            registry.update_element_state(&element_id, state.clone(), timestamp);

            if session.is_active() {
                if let Some(our_peer_id) = session.our_peer_id() {
                    let message = ReplicationMessageBuilder::state_update(
                        element_id,
                        state,
                        our_peer_id,
                    );
                    session.send_message(message);
                }
            }
        }
    }

    fn replication_mode(&self, cx: &App) -> Option<ReplicationMode> {
        let element_id = format!("input_{}", self.entity_id());
        let registry = ReplicationRegistry::global(cx);
        registry
            .get_element_state(&element_id)
            .map(|state| state.config.mode)
    }

    fn can_edit_replicated(&self, cx: &App) -> bool {
        let element_id = format!("input_{}", self.entity_id());
        let registry = ReplicationRegistry::global(cx);
        let session = SessionContext::global(cx);

        if !session.is_active() {
            return true;
        }

        let elem_state = match registry.get_element_state(&element_id) {
            Some(state) => state,
            None => return true,
        };

        let our_peer_id = match session.our_peer_id() {
            Some(id) => id,
            None => return false,
        };

        match elem_state.config.mode {
            ReplicationMode::NoRep => true,
            ReplicationMode::MultiEdit => {
                if let Some(max) = elem_state.config.max_concurrent_editors {
                    elem_state.active_editors.len() < max
                        || elem_state.active_editors.contains(&our_peer_id)
                } else {
                    true
                }
            }
            ReplicationMode::LockedEdit => {
                elem_state.locked_by.is_none()
                    || elem_state.locked_by.as_ref() == Some(&our_peer_id)
            }
            ReplicationMode::RequestEdit => elem_state.active_editors.contains(&our_peer_id),
            ReplicationMode::BroadcastOnly => session.are_we_host(),
            _ => true,
        }
    }

    fn apply_remote_state(&self, state: Value, window: &mut Window, cx: &mut App) -> Result<(), String> {
        let text = state
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or("Missing text field")?;

        let cursor = state
            .get("cursor")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        self.update(cx, |input_state, cx| {
            input_state.set_value(text.to_string(), window, cx);

            if let Some(cursor_pos) = cursor {
                let position = input_state.text().offset_to_position(cursor_pos);
                input_state.set_cursor_position(position, window, cx);
            }
        });

        Ok(())
    }
}

// ── UI-specific presence components ─────────────────────────────────────────
// These render components depend on GPUI UI primitives and cannot live in
// the engine-level pulsar_replication crate.

/// Size variant for presence pill display
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PresencePillSize {
    Small,
    Medium,
    Large,
}

/// A small colored badge representing a connected user
#[derive(IntoElement)]
pub struct PresencePill {
    presence: UserPresence,
    show_name: bool,
    show_status: bool,
    size: PresencePillSize,
}

impl PresencePill {
    pub fn new(presence: UserPresence) -> Self {
        Self { presence, show_name: true, show_status: false, size: PresencePillSize::Medium }
    }

    pub fn small(mut self) -> Self { self.size = PresencePillSize::Small; self.show_name = false; self }
    pub fn medium(mut self) -> Self { self.size = PresencePillSize::Medium; self.show_name = false; self }
    pub fn large(mut self) -> Self { self.size = PresencePillSize::Large; self.show_name = true; self }
    pub fn with_name(mut self, show: bool) -> Self { self.show_name = show; self }
    pub fn with_status(mut self, show: bool) -> Self { self.show_status = show; self }
}

impl RenderOnce for PresencePill {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = self.presence.color;
        let is_idle = self.presence.is_idle;
        match self.size {
            PresencePillSize::Small => div()
                .size_2()
                .rounded_full()
                .bg(color)
                .when(is_idle, |this| this.opacity(0.4)),
            PresencePillSize::Medium => div()
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
                .child(self.presence.initials()),
            PresencePillSize::Large => h_flex()
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
                .when(self.show_status && self.presence.status.is_some(), |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.presence.status.as_ref().unwrap().clone()),
                    )
                }),
        }
    }
}

/// Overlapping stack of presence pills showing multiple connected users
#[derive(IntoElement)]
pub struct PresenceStack {
    presences: Vec<UserPresence>,
    max_visible: usize,
    show_count: bool,
    size: PresencePillSize,
}

impl PresenceStack {
    pub fn new(presences: Vec<UserPresence>) -> Self {
        Self { presences, max_visible: 3, show_count: true, size: PresencePillSize::Medium }
    }

    pub fn max_visible(mut self, max: usize) -> Self { self.max_visible = max; self }
    pub fn show_count(mut self, show: bool) -> Self { self.show_count = show; self }
    pub fn small(mut self) -> Self { self.size = PresencePillSize::Small; self }
}

impl RenderOnce for PresenceStack {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let total = self.presences.len();
        let overflow = total.saturating_sub(self.max_visible);

        h_flex()
            .items_center()
            .gap_0p5()
            .children(self.presences.iter().take(self.max_visible).map(|presence| {
                let pill = PresencePill::new(presence.clone());
                match self.size {
                    PresencePillSize::Small => pill.small(),
                    PresencePillSize::Medium => pill.medium(),
                    PresencePillSize::Large => pill.large(),
                }
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

/// A 2px colored bar at the top of a tab showing who is present
#[derive(IntoElement)]
pub struct TabPresenceIndicator {
    presences: Vec<UserPresence>,
    show_count: bool,
}

impl TabPresenceIndicator {
    pub fn new(presences: Vec<UserPresence>) -> Self {
        Self { presences, show_count: true }
    }

    pub fn show_count(mut self, show: bool) -> Self { self.show_count = show; self }
}

impl RenderOnce for TabPresenceIndicator {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        if self.presences.is_empty() {
            return div().into_any_element();
        }

        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(2.0))
            .bg(self.presences[0].color)
            .into_any_element()
    }
}

/// Inline indicator on an input field showing who is editing (and if locked)
#[derive(IntoElement)]
pub struct FieldPresenceIndicator {
    presence: UserPresence,
    is_locked: bool,
}

impl FieldPresenceIndicator {
    pub fn new(presence: UserPresence) -> Self {
        Self { presence, is_locked: false }
    }

    pub fn locked(mut self, locked: bool) -> Self { self.is_locked = locked; self }
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
                    .child(self.presence.initials()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().foreground)
                    .child(self.presence.short_name()),
            )
    }
}
