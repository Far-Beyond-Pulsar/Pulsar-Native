use super::*;

impl AgentChatPanel {
    pub(super) fn rollback_chat_to_message(&mut self, message_ix: usize, cx: &mut Context<Self>) {
        if self.is_request_in_flight || message_ix >= self.messages.len() {
            return;
        }

        self.pending_rollback_confirm_ix = None;
        self.messages.truncate(message_ix + 1);
        if self.messages.is_empty() {
            self.messages.push(Self::default_system_message());
        }

        self.streaming_message_ix = None;
        self.message_row_heights.clear();
        self.save_current_chat();
        self.refresh_chat_history_list(cx);
        self.scroll_messages_to_bottom();
        cx.notify();
    }

    pub(super) fn fork_chat_here(&mut self, message_ix: usize, cx: &mut Context<Self>) {
        if self.is_request_in_flight || message_ix >= self.messages.len() {
            return;
        }

        self.pending_rollback_confirm_ix = None;
        let mut forked_messages = self.messages[..=message_ix].to_vec();
        if forked_messages.is_empty() {
            forked_messages.push(Self::default_system_message());
        }

        self.current_chat_id = format!("chat-{}", Self::now_epoch_nanos());
        self.current_chat_created_at = Self::now_epoch_secs();
        self.messages = forked_messages;
        self.streaming_message_ix = None;
        self.message_row_heights.clear();

        self.save_current_chat();
        self.refresh_chat_history_list(cx);
        self.scroll_messages_to_bottom();
        cx.notify();
    }

    pub(super) fn request_rollback_confirmation(
        &mut self,
        message_ix: usize,
        cx: &mut Context<Self>,
    ) {
        if self.is_request_in_flight || message_ix >= self.messages.len() {
            return;
        }

        self.pending_rollback_confirm_ix = Some(message_ix);
        cx.notify();
    }

    pub(super) fn cancel_rollback_confirmation(&mut self, cx: &mut Context<Self>) {
        self.pending_rollback_confirm_ix = None;
        cx.notify();
    }
}
