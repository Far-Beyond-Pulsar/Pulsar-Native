use gpui::*;
use ui::scroll::Scrollbar;

use super::panel::AgentChatPanel;

impl AgentChatPanel {
    /// Add the scrollbar and viewport height tracker around the message area.
    pub(crate) fn apply_scrollbar(&self, content: impl IntoElement) -> impl IntoElement {
        div()
            .relative()
            .flex_1()
            .child(content)
            .child(div().absolute().inset_0().child(Scrollbar::vertical(
                &self.messages_scroll_state,
                &self.messages_scroll_handle,
            )))
    }
}
