use gpui::*;
use std::rc::Rc;
use std::collections::HashMap;

use super::panel::AgentChatPanel;
use super::types::*;
use super::context;

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
        let render_now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let display_count = self.display_items.len();

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

    pub(crate) fn subagent_status_text(&self) -> String {
        let queued_count = self.pending_subagent_events.len();
        if self.is_processing_subagent_event {
            let active_id = self
                .processing_subagent_id
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            format!(
                "Processing subagent completion ({active_id}). {} waiting.",
                queued_count
            )
        } else if queued_count > 0 {
            let mode = if self.subagent_completion_mode == super::panel::SubagentCompletionMode::Manual {
                "manual mode"
            } else {
                "auto mode"
            };
            format!(
                "{} subagent completion(s) waiting ({})",
                queued_count, mode
            )
        } else {
            "No subagent completions waiting".to_string()
        }
    }
}
