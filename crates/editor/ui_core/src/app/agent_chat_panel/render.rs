use gpui::*;
use std::rc::Rc;

use super::panel::AgentChatPanel;

impl AgentChatPanel {
    pub(crate) fn render_auto_scroll_safety_net(&mut self) -> bool {
        if !self.is_request_in_flight {
            self.auto_scroll = true;
        } else if !self.auto_scroll && self.distance_from_bottom() < px(100.0) {
            self.auto_scroll = true;
        }
        !self.auto_scroll && self.distance_from_bottom() > px(100.0)
    }

    pub(crate) fn build_display_item_sizes(&self) -> Rc<Vec<gpui::Size<Pixels>>> {
        Rc::new(
            self.display_items
                .iter()
                .enumerate()
                .map(|(ix, item)| {
                    let h = self
                        .display_item_heights
                        .get(&ix)
                        .copied()
                        .unwrap_or_else(|| Self::display_item_height(item));
                    size(px(0.0), h)
                })
                .chain(std::iter::once(size(px(0.0), px(120.0))))
                .collect::<Vec<_>>(),
        )
    }
}
